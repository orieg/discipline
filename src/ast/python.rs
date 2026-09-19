//! Python language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn};

/// Python language pack implementing [`LanguagePack`].
pub struct PythonPack;

impl LanguagePack for PythonPack {
    fn id(&self) -> &'static str {
        "python"
    }

    fn name(&self) -> &'static str {
        "Python"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("py" | "pyi"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Python grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = PythonExtractor {
            src: src.as_bytes(),
            vocab,
            is_test_path: is_python_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        Ok(extractor.facts)
    }
}

/// Determines whether a path is conventionally a Python test file.
pub fn is_python_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    if filename.starts_with("test_") || filename.ends_with("_test.py") {
        return true;
    }
    path.starts_with("tests/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.contains("/test/")
}

struct PythonExtractor<'a> {
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
}

impl<'a> PythonExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        if node.kind() == "comment" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            let trimmed = text.trim_start_matches('#').trim();

            if trimmed.starts_with("type: ignore") {
                self.facts.escape_hatches.push(EscapeHatchSite::TypeIgnore {
                    line,
                    tool: "mypy".to_string(),
                    snippet: text.to_string(),
                });
            } else if trimmed.starts_with("noqa") || trimmed.starts_with("ruff: noqa") {
                let rule = if let Some(rest) = trimmed.strip_prefix("noqa:") {
                    rest.trim().to_string()
                } else if let Some(rest) = trimmed.strip_prefix("ruff: noqa:") {
                    rest.trim().to_string()
                } else {
                    "all".to_string()
                };
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule,
                        snippet: text.to_string(),
                    });
            } else if trimmed.starts_with("pragma: no cover") {
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: "coverage".to_string(),
                        snippet: text.to_string(),
                    });
            } else if trimmed.starts_with("pylint: disable=") {
                let rule = trimmed
                    .strip_prefix("pylint: disable=")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule,
                        snippet: text.to_string(),
                    });
            }
            return;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments_and_escape_hatches(child);
        }
    }

    fn visit_root(&mut self, root: Node) {
        let mut scope = Vec::new();
        self.visit_node(root, &mut scope);
    }

    fn visit_node(&mut self, node: Node, scope: &mut Vec<String>) {
        match node.kind() {
            "function_definition" => {
                self.process_function(node, scope, None);
            }
            "decorated_definition" => {
                let mut decorators = Vec::new();
                let mut inner_def = None;

                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "decorator" {
                        decorators.push(child);
                    } else if child.kind() == "function_definition"
                        || child.kind() == "class_definition"
                    {
                        inner_def = Some(child);
                    }
                }

                if let Some(def) = inner_def {
                    if def.kind() == "function_definition" {
                        self.process_function(def, scope, Some(&decorators));
                    } else if def.kind() == "class_definition" {
                        self.process_class(def, scope, Some(&decorators));
                    }
                }
            }
            "class_definition" => {
                self.process_class(node, scope, None);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_node(child, scope);
                }
            }
        }
    }

    fn process_class(
        &mut self,
        node: Node,
        scope: &mut Vec<String>,
        class_decorators: Option<&[Node]>,
    ) {
        let class_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).to_string())
            .unwrap_or_default();

        let class_is_ignored = class_decorators
            .map(|decs| decs.iter().any(|d| self.is_skip_decorator(*d)))
            .unwrap_or(false);

        scope.push(class_name);

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                if child.kind() == "function_definition" {
                    self.process_function_with_inherited_ignore(
                        child,
                        scope,
                        None,
                        class_is_ignored,
                    );
                } else if child.kind() == "decorated_definition" {
                    let mut decorators = Vec::new();
                    let mut inner_fn = None;
                    let mut c = child.walk();
                    for grandchild in child.children(&mut c) {
                        if grandchild.kind() == "decorator" {
                            decorators.push(grandchild);
                        } else if grandchild.kind() == "function_definition" {
                            inner_fn = Some(grandchild);
                        }
                    }
                    if let Some(f) = inner_fn {
                        self.process_function_with_inherited_ignore(
                            f,
                            scope,
                            Some(&decorators),
                            class_is_ignored,
                        );
                    }
                } else {
                    self.visit_node(child, scope);
                }
            }
        }

        scope.pop();
    }

    fn process_function(&mut self, node: Node, scope: &[String], decorators: Option<&[Node]>) {
        self.process_function_with_inherited_ignore(node, scope, decorators, false);
    }

    fn process_function_with_inherited_ignore(
        &mut self,
        node: Node,
        scope: &[String],
        decorators: Option<&[Node]>,
        parent_ignored: bool,
    ) {
        let fn_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).to_string())
            .unwrap_or_default();

        // Check if this is a test function:
        // 1. Name starts with `test_` or `test`
        // 2. Or is inside a test class (`Test*`) and name starts with `test`
        // 3. Or file is a test path and function name starts with `test`
        let is_test_name = fn_name.starts_with("test_")
            || fn_name == "test"
            || (self.is_test_path && fn_name.starts_with("test"));

        let is_in_test_class = scope
            .iter()
            .any(|s| s.starts_with("Test") || s.ends_with("Test") || s.ends_with("Tests"));

        if !is_test_name && !is_in_test_class {
            return;
        }

        // Check decorators for skips
        let mut ignored = parent_ignored;
        if let Some(decs) = decorators {
            for dec in decs {
                if self.is_skip_decorator(*dec) {
                    ignored = true;
                }
            }
        }

        let full_name = if scope.is_empty() {
            fn_name
        } else {
            format!("{}::{}", scope.join("::"), fn_name)
        };

        let line = node.start_position().row + 1;
        let mut test_fn = TestFn {
            name: full_name,
            line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored,
            should_panic: false,
        };

        if let Some(body) = node.child_by_field_name("body") {
            self.scan_test_body(body, &mut test_fn);
        }

        self.facts.tests.push(test_fn);
    }

    fn is_skip_decorator(&self, dec: Node) -> bool {
        let text = self.text(dec);
        text.contains("pytest.mark.skip")
            || text.contains("pytest.mark.xfail")
            || text.contains("unittest.skip")
            || text.contains("skipIf")
            || text.contains("skipUnless")
    }

    fn scan_test_body(&self, body: Node, test: &mut TestFn) {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            self.visit_body_node(child, test);
        }
    }

    fn visit_body_node(&self, node: Node, test: &mut TestFn) {
        match node.kind() {
            "assert_statement" => {
                test.total_asserts += 1;
                let mut cursor = node.walk();
                let children: Vec<_> = node.children(&mut cursor).collect();
                // In Python: assert condition, message
                // Find condition expression
                let condition = children
                    .iter()
                    .copied()
                    .find(|c| c.kind() != "assert" && c.kind() != "," && c.kind() != "comment");

                if let Some(cond) = condition {
                    if self.is_tautological(cond) {
                        test.tautologies += 1;
                    }
                    if self.is_strong_assertion(cond) {
                        test.strong_asserts += 1;
                    }
                }
            }
            "with_statement" => {
                // Check if with item is pytest.raises(...)
                let text = self.text(node);
                if text.contains("pytest.raises") {
                    test.total_asserts += 1;
                    test.strong_asserts += 1;
                }
                // Recursively check with body
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "block" {
                        self.scan_test_body(child, test);
                    }
                }
            }
            "call" => {
                let call_text = self.text(node);
                if let Some(func_node) = node.child_by_field_name("function") {
                    let func_name = self.text(func_node);
                    if func_name.starts_with("self.assert") {
                        test.total_asserts += 1;
                        if self.is_strong_unittest_assert(func_name) {
                            test.strong_asserts += 1;
                        }
                        if self.is_tautological_call(node, func_name) {
                            test.tautologies += 1;
                        }
                    } else if func_name == "pytest.skip"
                        || func_name == "pytest.xfail"
                        || func_name == "self.skipTest"
                    {
                        test.ignored = true;
                    } else if self
                        .vocab
                        .helper_fns
                        .iter()
                        .any(|h| func_name == h || func_name.ends_with(&format!(".{h}")))
                    {
                        test.total_asserts += 1;
                    }
                }
                if call_text.contains("pytest.raises") && !call_text.starts_with("with ") {
                    test.total_asserts += 1;
                    test.strong_asserts += 1;
                }
            }
            "block" => {
                self.scan_test_body(node, test);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_body_node(child, test);
                }
            }
        }
    }

    fn is_strong_assertion(&self, cond: Node) -> bool {
        match cond.kind() {
            "comparison_operator" => {
                // Check operator: ==, !=, in, not in, is, is not
                let text = self.text(cond);
                text.contains("==")
                    || text.contains("!=")
                    || text.contains(" in ")
                    || text.contains(" not in ")
                    || text.contains(" is ")
                    || text.contains(" is not ")
            }
            "call" => {
                let text = self.text(cond);
                text.contains("isinstance") || text.contains("issubclass")
            }
            _ => false,
        }
    }

    fn is_strong_unittest_assert(&self, func_name: &str) -> bool {
        matches!(
            func_name,
            "self.assertEqual"
                | "self.assertNotEqual"
                | "self.assertIn"
                | "self.assertNotIn"
                | "self.assertIs"
                | "self.assertIsNot"
                | "self.assertIsNone"
                | "self.assertIsNotNone"
                | "self.assertIsInstance"
                | "self.assertNotIsInstance"
                | "self.assertAlmostEqual"
                | "self.assertNotAlmostEqual"
                | "self.assertCountEqual"
                | "self.assertRegex"
                | "self.assertNotRegex"
                | "self.assertRaises"
        )
    }

    fn is_tautological(&self, cond: Node) -> bool {
        let text = self.text(cond).trim();
        if text == "True" || text == "1" || text == "\"\"" || text == "''" {
            return true;
        }
        if cond.kind() == "comparison_operator" {
            let mut cursor = cond.walk();
            let children: Vec<_> = cond.children(&mut cursor).collect();
            if children.len() >= 3 {
                let left = self.text(children[0]).trim();
                let op = self.text(children[1]).trim();
                let right = self.text(children[2]).trim();
                if (op == "==" || op == "is") && left == right && !left.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn is_tautological_call(&self, call: Node, func_name: &str) -> bool {
        if let Some(args) = call.child_by_field_name("arguments") {
            let text = self.text(args).trim();
            if func_name == "self.assertTrue" && text == "(True)" {
                return true;
            }
            if func_name == "self.assertFalse" && text == "(False)" {
                return true;
            }
            if func_name == "self.assertEqual" || func_name == "self.assertIs" {
                let mut cursor = args.walk();
                let arg_nodes: Vec<_> = args
                    .children(&mut cursor)
                    .filter(|n| {
                        n.kind() != "("
                            && n.kind() != ")"
                            && n.kind() != ","
                            && n.kind() != "comment"
                    })
                    .collect();
                if arg_nodes.len() >= 2 {
                    let a = self.text(arg_nodes[0]).trim();
                    let b = self.text(arg_nodes[1]).trim();
                    if a == b && !a.is_empty() {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pytest_functions_and_assertions() {
        let src = r#"
# type: ignore
def test_addition():
    # noqa: E501
    x = 1
    assert x + 1 == 2
    assert x > 0

def test_vacuous():
    pass

@pytest.mark.skip(reason="unimplemented")
def test_skipped():
    assert 1 == 1
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/test_calc.py", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 3);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "test_addition");
        assert_eq!(t0.total_asserts, 2);
        assert_eq!(t0.strong_asserts, 1);
        assert!(!t0.is_vacuous());
        assert!(!t0.ignored);

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "test_vacuous");
        assert_eq!(t1.total_asserts, 0);
        assert!(t1.is_vacuous());

        let t2 = &facts.tests[2];
        assert_eq!(t2.name, "test_skipped");
        assert!(t2.ignored);
        assert_eq!(t2.tautologies, 1);

        assert_eq!(facts.escape_hatches.len(), 2);
        assert!(matches!(
            facts.escape_hatches[0],
            EscapeHatchSite::TypeIgnore { .. }
        ));
        assert!(matches!(
            facts.escape_hatches[1],
            EscapeHatchSite::LinterDisable { .. }
        ));
    }

    #[test]
    fn parses_unittest_classes_and_methods() {
        let src = r#"
import unittest

class TestMath(unittest.TestCase):
    def test_multiplication(self):
        self.assertEqual(2 * 3, 6)
        self.assertTrue(6 > 0)

    @unittest.skip("skip reason")
    def test_skipped_method(self):
        self.assertEqual(1, 1)
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/test_math.py", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 2);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "TestMath::test_multiplication");
        assert_eq!(t0.total_asserts, 2);
        assert_eq!(t0.strong_asserts, 1);
        assert!(!t0.ignored);

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "TestMath::test_skipped_method");
        assert!(t1.ignored);
        assert_eq!(t1.tautologies, 1);
    }

    #[test]
    fn parses_pytest_raises_and_helpers() {
        let src = r#"
import pytest

def test_errors():
    with pytest.raises(ValueError):
        int("abc")

def test_helpers():
    custom_assert(123)
"#;
        let vocab = AssertVocabulary {
            helper_fns: vec!["custom_assert".to_string()],
            ..Default::default()
        };
        let facts = PythonPack.extract("test_err.py", src, &vocab).unwrap();

        assert_eq!(facts.tests.len(), 2);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());

        assert_eq!(facts.tests[1].total_asserts, 1);
        assert!(!facts.tests[1].is_vacuous());
    }
}
