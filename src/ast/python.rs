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

fn statement_has_skip_mark(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("pytestmark") {
        let rest = trimmed.strip_prefix("pytestmark").unwrap().trim_start();
        if rest.starts_with('=') {
            return rest.contains("skip") || rest.contains("xfail");
        }
    }
    false
}

fn is_assertion_context_manager(text: &str) -> bool {
    text.contains("pytest.raises")
        || text.contains("pytest.warns")
        || text.contains("assertRaises")
        || text.contains("assertRaisesRegex")
        || text.contains("assertWarns")
        || text.contains("assertWarnsRegex")
        || text.contains("assertLogs")
        || text.contains("assertNoLogs")
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
        let mut module_ignored = false;
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if statement_has_skip_mark(self.text(child)) {
                module_ignored = true;
                break;
            }
        }
        let mut scope = Vec::new();
        self.visit_node(root, &mut scope, module_ignored);
    }

    fn visit_node(&mut self, node: Node, scope: &mut Vec<String>, parent_ignored: bool) {
        match node.kind() {
            "function_definition" => {
                self.process_function_with_inherited_ignore(node, scope, None, parent_ignored);
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
                        self.process_function_with_inherited_ignore(
                            def,
                            scope,
                            Some(&decorators),
                            parent_ignored,
                        );
                    } else if def.kind() == "class_definition" {
                        self.process_class(def, scope, Some(&decorators), parent_ignored);
                    }
                }
            }
            "class_definition" => {
                self.process_class(node, scope, None, parent_ignored);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_node(child, scope, parent_ignored);
                }
            }
        }
    }

    fn process_class(
        &mut self,
        node: Node,
        scope: &mut Vec<String>,
        class_decorators: Option<&[Node]>,
        parent_ignored: bool,
    ) {
        let class_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).to_string())
            .unwrap_or_default();

        let class_has_skip_mark = if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            let has_skip = body
                .children(&mut cursor)
                .any(|c| statement_has_skip_mark(self.text(c)));
            has_skip
        } else {
            false
        };

        let class_is_ignored = parent_ignored
            || class_has_skip_mark
            || class_decorators
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
                    self.visit_node(child, scope, class_is_ignored);
                }
            }
        }

        scope.pop();
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

        // In Python unittest and pytest:
        // A test function/method MUST have a name starting with `test_` or equal to `test` (or inside a test file / test class, start with `test`).
        // It is NEVER a test function if:
        // 1. Its name starts with `_` (e.g. `_cell`, `_helper`, `__init__`)
        // 2. It is a unittest lifecycle method: setUp, tearDown, setUpClass, tearDownClass, setUpModule, tearDownModule
        // 3. It has a non-test decorator like @staticmethod, @classmethod, @property
        if fn_name.starts_with('_')
            || matches!(
                fn_name.as_str(),
                "setUp"
                    | "tearDown"
                    | "setUpClass"
                    | "tearDownClass"
                    | "setUpModule"
                    | "tearDownModule"
            )
        {
            return;
        }

        if let Some(decs) = decorators {
            if decs.iter().any(|d| self.is_non_test_decorator(*d)) {
                return;
            }
        }

        let is_test_name = fn_name == "self_test"
            || fn_name.starts_with("test_")
            || fn_name == "test"
            || (self.is_test_path && fn_name.starts_with("test"));

        if !is_test_name {
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
        let end_line = node.end_position().row + 1;
        let mut test_fn = TestFn {
            name: full_name,
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored,
            should_panic: false,
            ..Default::default()
        };

        if let Some(body) = node.child_by_field_name("body") {
            self.scan_test_body(body, &mut test_fn);
        }

        self.facts.tests.push(test_fn);
    }

    fn is_skip_decorator(&self, dec: Node) -> bool {
        let text = self.text(dec).trim();
        text.contains("pytest.mark.skip")
            || text.contains("pytest.mark.xfail")
            || text.contains("unittest.skip")
            || text.contains("skipIf")
            || text.contains("skipUnless")
            || text.starts_with("@skip")
            || text.starts_with("@unittest.skip")
            || text.starts_with("@pytest.mark.skip")
            || text.starts_with("@pytest.mark.xfail")
    }

    fn is_non_test_decorator(&self, dec: Node) -> bool {
        let text = self.text(dec).trim();
        text.contains("staticmethod") || text.contains("classmethod") || text.contains("property")
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
                let mut found_items = 0;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "with_clause" {
                        let mut c2 = child.walk();
                        for item in child.children(&mut c2) {
                            if item.kind() == "with_item" {
                                found_items += 1;
                                if is_assertion_context_manager(self.text(item)) {
                                    test.total_asserts += 1;
                                    test.strong_asserts += 1;
                                }
                            }
                        }
                    } else if child.kind() == "with_item" {
                        found_items += 1;
                        if is_assertion_context_manager(self.text(child)) {
                            test.total_asserts += 1;
                            test.strong_asserts += 1;
                        }
                    } else if child.kind() == "block" {
                        self.scan_test_body(child, test);
                    }
                }
                if found_items == 0 {
                    let header_text = if let Some(block) = node.child_by_field_name("body") {
                        let start = node.start_byte();
                        let end = block.start_byte().min(self.src.len());
                        std::str::from_utf8(&self.src[start..end]).unwrap_or("")
                    } else {
                        self.text(node)
                    };
                    if is_assertion_context_manager(header_text) {
                        test.total_asserts += 1;
                        test.strong_asserts += 1;
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
                | "self.assertRaisesRegex"
                | "self.assertWarns"
                | "self.assertWarnsRegex"
                | "self.assertLogs"
                | "self.assertNoLogs"
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
            let mut cursor = args.walk();
            let arg_nodes: Vec<_> = args
                .children(&mut cursor)
                .filter(|n| {
                    n.kind() != "(" && n.kind() != ")" && n.kind() != "," && n.kind() != "comment"
                })
                .collect();
            if !arg_nodes.is_empty() {
                let first = self.text(arg_nodes[0]).trim();
                if func_name == "self.assertTrue" && (first == "True" || first == "1") {
                    return true;
                }
                if func_name == "self.assertFalse" && (first == "False" || first == "0") {
                    return true;
                }
            }
            if (func_name == "self.assertEqual" || func_name == "self.assertIs")
                && arg_nodes.len() >= 2
            {
                let a = self.text(arg_nodes[0]).trim();
                let b = self.text(arg_nodes[1]).trim();
                if a == b && !a.is_empty() {
                    return true;
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

    #[test]
    fn parses_unittest_context_managers_and_pytestmark() {
        let src = r#"
import unittest
import pytest

pytestmark = pytest.mark.skip("module skipped")

def test_module_skipped():
    pass

class TestContextManagers(unittest.TestCase):
    def test_assert_raises(self):
        with self.assertRaises(ValueError):
            int("abc")

    def test_assert_raises_regex(self):
        with self.assertRaisesRegex(ValueError, r"invalid literal"):
            int("xyz")

    def test_assert_warns(self):
        with self.assertWarns(UserWarning):
            do_warn()

    def test_assert_logs(self):
        with self.assertLogs("app", level="INFO"):
            log_info()

@unittest.skip("class skipped")
class TestSkippedClass(unittest.TestCase):
    def test_one(self):
        pass

class TestClassPytestmark(unittest.TestCase):
    pytestmark = pytest.mark.skip("class skip via mark")

    def test_two(self):
        pass
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack.extract("tests/test_f5.py", src, &vocab).unwrap();

        let by_name = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();

        // 1. Module pytestmark marks top-level test ignored
        assert!(by_name("test_module_skipped").ignored);

        // 2. with self.assertRaises counted as assertion (not vacuous, strong)
        let t_raises = by_name("TestContextManagers::test_assert_raises");
        assert_eq!(t_raises.total_asserts, 1);
        assert_eq!(t_raises.strong_asserts, 1);
        assert!(!t_raises.is_vacuous());

        // 3. with self.assertRaisesRegex counted
        let t_regex = by_name("TestContextManagers::test_assert_raises_regex");
        assert_eq!(t_regex.total_asserts, 1);
        assert_eq!(t_regex.strong_asserts, 1);
        assert!(!t_regex.is_vacuous());

        // 4. with self.assertWarns counted
        let t_warns = by_name("TestContextManagers::test_assert_warns");
        assert_eq!(t_warns.total_asserts, 1);
        assert_eq!(t_warns.strong_asserts, 1);
        assert!(!t_warns.is_vacuous());

        // 5. with self.assertLogs counted
        let t_logs = by_name("TestContextManagers::test_assert_logs");
        assert_eq!(t_logs.total_asserts, 1);
        assert_eq!(t_logs.strong_asserts, 1);
        assert!(!t_logs.is_vacuous());

        // 6. @unittest.skip on class marks method ignored
        assert!(by_name("TestSkippedClass::test_one").ignored);

        // 7. pytestmark on class marks method ignored
        assert!(by_name("TestClassPytestmark::test_two").ignored);
    }

    #[test]
    fn fixture_negative_control_clean_suite_passes() {
        let src = include_str!("../../tests/fixtures/python/clean_suite.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/clean.py", src, &vocab)
            .expect("extract clean");
        assert!(!facts.has_parse_errors);
        assert_eq!(facts.tests.len(), 3);
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(!t.ignored, "test {} should not be ignored", t.name);
            assert!(t.total_asserts >= 1);
            assert!(t.strong_asserts >= 1);
        }
    }

    #[test]
    fn fixture_vacuous_and_tautologies_detected() {
        let src = include_str!("../../tests/fixtures/python/vacuous_suite.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/vacuous.py", src, &vocab)
            .expect("extract vacuous");
        assert_eq!(facts.tests.len(), 5);
        for t in &facts.tests {
            assert!(t.is_vacuous(), "test {} should be vacuous", t.name);
        }
        let empty = facts.tests.iter().find(|t| t.name == "test_empty").unwrap();
        assert_eq!(empty.total_asserts, 0);

        let t_bool = facts
            .tests
            .iter()
            .find(|t| t.name == "test_tautology_bool")
            .unwrap();
        assert!(t_bool.tautologies >= 1);

        let t_math = facts
            .tests
            .iter()
            .find(|t| t.name == "test_tautology_math")
            .unwrap();
        assert!(t_math.tautologies >= 1);

        let t_eq = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("test_tautology_assert_equal"))
            .unwrap();
        assert!(t_eq.tautologies >= 1);

        let t_true = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("test_tautology_assert_true"))
            .unwrap();
        assert!(t_true.tautologies >= 1);
    }

    #[test]
    fn fixture_implicit_assertions_detected() {
        let src = include_str!("../../tests/fixtures/python/implicit_asserts.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/implicit.py", src, &vocab)
            .expect("extract implicit");
        assert_eq!(facts.tests.len(), 5);
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(t.total_asserts >= 1);
            assert!(t.strong_asserts >= 1);
        }
    }

    #[test]
    fn fixture_skips_at_all_levels_detected() {
        let src = include_str!("../../tests/fixtures/python/skips_suite.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/skips.py", src, &vocab)
            .expect("extract skips");
        assert_eq!(facts.tests.len(), 4);
        for t in &facts.tests {
            assert!(t.ignored, "test {} should be marked ignored", t.name);
        }
    }

    #[test]
    fn fixture_comments_and_strings_not_counted_as_assertions() {
        let src = include_str!("../../tests/fixtures/python/comments_and_strings.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/comments.py", src, &vocab)
            .expect("extract comments");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_helpers_and_lambdas_require_configuration() {
        let src = include_str!("../../tests/fixtures/python/helpers_and_lambdas.py");
        let unconfigured_vocab = AssertVocabulary::default();
        let unconfigured = PythonPack
            .extract("tests/helpers.py", src, &unconfigured_vocab)
            .unwrap();
        assert_eq!(unconfigured.tests.len(), 1);
        assert!(unconfigured.tests[0].is_vacuous());

        let configured_vocab = AssertVocabulary {
            helper_fns: vec!["assert_positive".to_string()],
            ..Default::default()
        };
        let configured = PythonPack
            .extract("tests/helpers.py", src, &configured_vocab)
            .unwrap();
        assert_eq!(configured.tests[0].total_asserts, 2);
        assert!(!configured.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_syntax_error_surfaces_parse_error() {
        let src = include_str!("../../tests/fixtures/python/syntax_error.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/broken.py", src, &vocab)
            .expect("extract broken");
        assert!(facts.has_parse_errors);
    }

    #[test]
    fn test_class_helpers_and_lifecycle_hooks_are_ignored() {
        let src = r#"
class TestWriterTarget930:
    @staticmethod
    def _cell(arg):
        return arg * 2

    def setUp(self):
        self.x = 1

    def helper_calc(self):
        return 42

    def test_real_case(self):
        assert self.helper_calc() == 42
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/test_writer.py", src, &vocab)
            .expect("extract test_writer");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "TestWriterTarget930::test_real_case");
        assert!(!facts.tests[0].is_vacuous());
    }
}
