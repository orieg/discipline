//! JavaScript and TypeScript language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn};

/// JavaScript & TypeScript language pack implementing [`LanguagePack`].
pub struct JavaScriptPack;

impl LanguagePack for JavaScriptPack {
    fn id(&self) -> &'static str {
        "javascript"
    }

    fn name(&self) -> &'static str {
        "JavaScript/TypeScript"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(
            super::extension(path),
            Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts")
        )
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        let ext = super::extension(path).unwrap_or("js");
        let lang = match ext {
            "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "tsx" | "jsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_javascript::LANGUAGE.into(),
        };

        parser
            .set_language(&lang)
            .map_err(|e| anyhow!("failed to load JS/TS grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = JsExtractor {
            src: src.as_bytes(),
            vocab,
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

/// Matcher strength classification for JavaScript and TypeScript testing frameworks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherClass {
    /// Strong matchers asserting specific values, equality, structures, patterns, or exceptions.
    Strong,
    /// Weak matchers asserting truthiness, definedness, existence, or calls without argument checks.
    Weak,
    /// Unknown or non-matcher methods.
    Unknown,
}

/// Classifies a matcher property name into strong, weak, or unknown.
pub fn classify_matcher(name: &str) -> MatcherClass {
    match name {
        // Strong equality / identity matchers
        "toBe"
        | "toEqual"
        | "toStrictEqual"
        | "toReturnWith"
        | "toHaveReturnedWith"
        | "toHaveBeenLastCalledWith"
        | "toHaveBeenNthCalledWith"
        | "toHaveBeenCalledWith"
        | "toBeCalledWith"
        | "equal"
        | "deepEqual"
        | "strictEqual"
        | "eq" => MatcherClass::Strong,

        // Strong pattern / snapshot / structural / numeric matchers
        "toMatch"
        | "toMatchObject"
        | "toMatchSnapshot"
        | "toMatchInlineSnapshot"
        | "toContain"
        | "toContainEqual"
        | "include"
        | "members"
        | "deepMembers"
        | "toHaveLength"
        | "toHaveProperty"
        | "lengthOf"
        | "property"
        | "toBeCloseTo"
        | "toBeGreaterThan"
        | "toBeGreaterThanOrEqual"
        | "toBeLessThan"
        | "toBeLessThanOrEqual"
        | "toBeInstanceOf"
        | "toThrow"
        | "toThrowError"
        | "throws"
        | "toBeNull"
        | "toBeNaN"
        | "toHaveBeenCalledTimes"
        | "toBeCalledTimes"
        | "toHaveReturnedTimes" => MatcherClass::Strong,

        // Weak matchers: existence / truthiness / broad check without payload
        "toBeTruthy" | "toBeFalsy" | "toBeDefined" | "toBeUndefined" | "exist" | "ok"
        | "toHaveBeenCalled" | "toBeCalled" | "toHaveReturned" | "toReturn" => MatcherClass::Weak,

        _ => {
            if name.starts_with("toBe") || name.starts_with("to") || name.starts_with("toHave") {
                MatcherClass::Strong
            } else {
                MatcherClass::Unknown
            }
        }
    }
}

/// True if `name` is a recognized or plausible expect matcher name.
pub fn is_matcher(name: &str) -> bool {
    classify_matcher(name) != MatcherClass::Unknown
        || name.starts_with("to")
        || name.starts_with("toHave")
        || name.starts_with("toBe")
}

/// Returns true if `node` is part of an `expect(...)` call or method chain.
fn is_expect_chain_node(mut node: Node, src: &[u8]) -> bool {
    while node.kind() == "member_expression" {
        if let Some(obj) = node.child_by_field_name("object") {
            node = obj;
        } else {
            break;
        }
    }
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            return func.utf8_text(src).unwrap_or("") == "expect";
        }
    }
    false
}

struct JsExtractor<'a> {
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    facts: ParsedFileFacts,
}

impl<'a> JsExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        if node.kind() == "comment" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            let trimmed = text
                .trim_start_matches("//")
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim();

            if trimmed.starts_with("@ts-ignore")
                || trimmed.starts_with("@ts-expect-error")
                || trimmed.starts_with("@ts-nocheck")
            {
                self.facts.escape_hatches.push(EscapeHatchSite::TypeIgnore {
                    line,
                    tool: "typescript".to_string(),
                    snippet: text.to_string(),
                });
            } else if trimmed.starts_with("eslint-disable") {
                let rule = if let Some(rest) = trimmed.strip_prefix("eslint-disable-line") {
                    rest.trim().to_string()
                } else if let Some(rest) = trimmed.strip_prefix("eslint-disable-next-line") {
                    rest.trim().to_string()
                } else if let Some(rest) = trimmed.strip_prefix("eslint-disable") {
                    rest.trim().to_string()
                } else {
                    "all".to_string()
                };
                let rule = if rule.is_empty() {
                    "all".to_string()
                } else {
                    rule
                };
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule,
                        snippet: text.to_string(),
                    });
            } else if trimmed.contains("istanbul ignore") || trimmed.contains("c8 ignore") {
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: "coverage".to_string(),
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
        self.visit_node(root, &mut scope, false);
    }

    fn visit_node(&mut self, node: Node, scope: &mut Vec<String>, parent_ignored: bool) {
        if node.kind() == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                let (is_test, is_suite, is_ignored, is_todo) = self.classify_call(func_node);
                if is_suite {
                    let title = self.extract_first_arg_title(node);
                    scope.push(title);
                    if let Some(args) = node.child_by_field_name("arguments") {
                        if let Some(callback) = Self::find_callback(args) {
                            self.visit_node(callback, scope, parent_ignored || is_ignored);
                        }
                    }
                    scope.pop();
                    return;
                } else if is_test {
                    let title = self.extract_first_arg_title(node);
                    let full_name = if scope.is_empty() {
                        title
                    } else {
                        format!("{} > {}", scope.join(" > "), title)
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
                        ignored: parent_ignored || is_ignored || is_todo,
                        should_panic: false,
                        ..Default::default()
                    };

                    if let Some(args) = node.child_by_field_name("arguments") {
                        if let Some(callback) = Self::find_callback(args) {
                            self.scan_test_body(callback, &mut test_fn);
                        }
                    }

                    self.facts.tests.push(test_fn);
                    return;
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit_node(child, scope, parent_ignored);
        }
    }

    fn classify_call(&self, func: Node) -> (bool, bool, bool, bool) {
        // (is_test, is_suite, is_ignored, is_todo)
        let text = self.text(func);
        match text {
            "it" | "test" => (true, false, false, false),
            "xit" | "xtest" => (true, false, true, false),
            "describe" | "context" => (false, true, false, false),
            "xdescribe" | "xcontext" => (false, true, true, false),
            _ => {
                if text.starts_with("describe.") {
                    let is_ignored = text.contains(".skip") || text.contains(".only");
                    (false, true, is_ignored, false)
                } else if text.starts_with("it.") || text.starts_with("test.") {
                    let is_todo = text.contains(".todo");
                    let is_ignored = text.contains(".skip") || text.contains(".only") || is_todo;
                    (true, false, is_ignored, is_todo)
                } else {
                    (false, false, false, false)
                }
            }
        }
    }

    fn extract_first_arg_title(&self, call_node: Node) -> String {
        if let Some(args) = call_node.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.kind() == "string" || child.kind() == "template_string" {
                    let s = self.text(child);
                    return s
                        .trim_matches('\'')
                        .trim_matches('"')
                        .trim_matches('`')
                        .to_string();
                }
            }
        }
        "unnamed".to_string()
    }

    fn find_callback<'tree>(args: Node<'tree>) -> Option<Node<'tree>> {
        let mut cursor = args.walk();
        for child in args.children(&mut cursor) {
            match child.kind() {
                "arrow_function" | "function_expression" | "function" => return Some(child),
                _ => {}
            }
        }
        None
    }

    fn scan_test_body(&self, body_or_fn: Node, test: &mut TestFn) {
        match body_or_fn.kind() {
            "arrow_function" | "function_expression" | "function" => {
                if let Some(body) = body_or_fn.child_by_field_name("body") {
                    self.scan_test_body(body, test);
                }
            }
            "statement_block" => {
                let mut cursor = body_or_fn.walk();
                for child in body_or_fn.children(&mut cursor) {
                    self.scan_test_body(child, test);
                }
            }
            "call_expression" => {
                self.check_assertion_call(body_or_fn, test);
                let mut cursor = body_or_fn.walk();
                for child in body_or_fn.children(&mut cursor) {
                    self.scan_test_body(child, test);
                }
            }
            _ => {
                let mut cursor = body_or_fn.walk();
                for child in body_or_fn.children(&mut cursor) {
                    self.scan_test_body(child, test);
                }
            }
        }
    }

    fn check_assertion_call(&self, call: Node, test: &mut TestFn) {
        let text = self.text(call);
        if let Some(func) = call.child_by_field_name("function") {
            let func_text = self.text(func);

            // expect(x).matcher() chain
            if is_expect_chain_node(func, self.src) || func_text.starts_with("expect(") {
                if let Some(prop) = func.child_by_field_name("property") {
                    let prop_name = self.text(prop);
                    let class = classify_matcher(prop_name);
                    if class != MatcherClass::Unknown || is_matcher(prop_name) {
                        test.total_asserts += 1;
                        if class == MatcherClass::Strong {
                            test.strong_asserts += 1;
                        }
                        if self.is_tautological_expect(call, prop_name) {
                            test.tautologies += 1;
                        }
                        return;
                    }
                }
            }

            // assert / assert.* calls
            if func_text == "assert" || func_text.starts_with("assert.") {
                test.total_asserts += 1;
                if self.is_strong_assert_fn(func_text) {
                    test.strong_asserts += 1;
                }
                if self.is_tautological_assert_call(call, func_text) {
                    test.tautologies += 1;
                }
                return;
            }

            // Configured helper functions
            if self
                .vocab
                .helper_fns
                .iter()
                .any(|h| func_text == h || func_text.ends_with(&format!(".{h}")))
            {
                test.total_asserts += 1;
            }
        } else if text.contains("expect(") && (text.contains(".to") || text.contains(".toHave")) {
            test.total_asserts += 1;
        }
    }

    fn is_strong_assert_fn(&self, func_text: &str) -> bool {
        matches!(
            func_text,
            "assert.equal"
                | "assert.strictEqual"
                | "assert.deepEqual"
                | "assert.deepStrictEqual"
                | "assert.notEqual"
                | "assert.notStrictEqual"
                | "assert.throws"
                | "assert.rejects"
                | "assert.match"
        )
    }

    fn is_tautological_expect(&self, call: Node, prop_name: &str) -> bool {
        // Find subject in expect(subject)
        // Structure of call: expect(subject).toBe(expected)
        let call_text = self.text(call);
        if prop_name == "toBe" || prop_name == "toEqual" || prop_name == "toStrictEqual" {
            if let Some(args) = call.child_by_field_name("arguments") {
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
                if arg_nodes.len() == 1 {
                    let expected = self.text(arg_nodes[0]).trim();
                    // Check if subject inside expect(...) matches expected
                    if let Some(func) = call.child_by_field_name("function") {
                        if let Some(obj) = func.child_by_field_name("object") {
                            let obj_text = self.text(obj);
                            if let Some(inner) = obj_text.strip_prefix("expect(") {
                                if let Some(subject) = inner.strip_suffix(')') {
                                    if subject.trim() == expected && !expected.is_empty() {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if (prop_name == "toBeTruthy" && call_text.contains("expect(true)"))
            || (prop_name == "toBeFalsy" && call_text.contains("expect(false)"))
        {
            return true;
        }
        false
    }

    fn is_tautological_assert_call(&self, call: Node, func_text: &str) -> bool {
        if let Some(args) = call.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            let arg_nodes: Vec<_> = args
                .children(&mut cursor)
                .filter(|n| {
                    n.kind() != "(" && n.kind() != ")" && n.kind() != "," && n.kind() != "comment"
                })
                .collect();
            if (func_text == "assert" || func_text == "assert.ok") && !arg_nodes.is_empty() {
                let text = self.text(arg_nodes[0]).trim();
                if text == "true" || text == "1" {
                    return true;
                }
            } else if (func_text == "assert.equal" || func_text == "assert.strictEqual")
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
    fn parses_nested_describe_and_it_blocks() {
        let src = r#"
// @ts-ignore
describe("AuthService", () => {
    // eslint-disable-next-line
    it("logs in valid user", () => {
        expect(1 + 1).toBe(2);
        expect(true).toBeTruthy();
    });

    it("empty test", () => {
    });

    it.skip("skipped test", () => {
        expect(1).toBe(1);
    });
});
"#;
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/auth.test.ts", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 3);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "AuthService > logs in valid user");
        assert_eq!(t0.total_asserts, 2);
        assert_eq!(t0.strong_asserts, 1);
        assert!(!t0.is_vacuous());
        assert!(!t0.ignored);

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "AuthService > empty test");
        assert_eq!(t1.total_asserts, 0);
        assert!(t1.is_vacuous());

        let t2 = &facts.tests[2];
        assert_eq!(t2.name, "AuthService > skipped test");
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
    fn parses_assert_and_helpers() {
        let src = r#"
test("assert tests", () => {
    assert.strictEqual(2 * 3, 6);
    customAssert(true);
});
"#;
        let vocab = AssertVocabulary {
            helper_fns: vec!["customAssert".to_string()],
            ..Default::default()
        };
        let facts = JavaScriptPack
            .extract("test/calc.test.js", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 2);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn parses_tsx_and_jsx() {
        let src = r#"
describe("<Button />", () => {
    it("renders children", () => {
        const el = <Button>Click me</Button>;
        expect(el).toBeDefined();
        expect(1).toBe(1);
    });
});
"#;
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/Button.test.tsx", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "<Button /> > renders children");
        assert_eq!(facts.tests[0].total_asserts, 2);
        assert_eq!(facts.tests[0].tautologies, 1);
    }

    #[test]
    fn js_matcher_classes_discriminate_strong_and_weak() {
        let strong_src = r#"
test("strong matchers", () => {
    expect(a).toBe(42);
    expect(b).toEqual({ id: 1 });
    expect(c).toHaveLength(3);
    expect(d).toHaveProperty("foo", "bar");
    expect(e).toHaveBeenCalledWith(1, 2);
});
"#;
        let weak_src = r#"
test("weak matchers", () => {
    expect(a).toBeTruthy();
    expect(b).toBeDefined();
    expect(c).toHaveBeenCalled();
    expect(d).toBeFalsy();
    expect(e).toBeUndefined();
});
"#;
        let vocab = AssertVocabulary::default();
        let strong_facts = JavaScriptPack
            .extract("test/strong.test.js", strong_src, &vocab)
            .unwrap();
        let weak_facts = JavaScriptPack
            .extract("test/weak.test.js", weak_src, &vocab)
            .unwrap();

        assert_eq!(strong_facts.tests[0].total_asserts, 5);
        assert_eq!(strong_facts.tests[0].strong_asserts, 5);

        assert_eq!(weak_facts.tests[0].total_asserts, 5);
        assert_eq!(weak_facts.tests[0].strong_asserts, 0);
    }

    #[test]
    fn fixture_negative_control_clean_suite_passes() {
        let src = include_str!("../../tests/fixtures/javascript/clean_suite.js");
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/clean.test.js", src, &vocab)
            .expect("extract clean");
        assert!(!facts.has_parse_errors);
        assert_eq!(facts.tests.len(), 2);
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(!t.ignored, "test {} should not be ignored", t.name);
            assert!(t.total_asserts >= 1);
        }
    }

    #[test]
    fn fixture_vacuous_and_tautologies_detected() {
        let src = include_str!("../../tests/fixtures/javascript/vacuous_suite.js");
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/vacuous.test.js", src, &vocab)
            .expect("extract vacuous");
        assert_eq!(facts.tests.len(), 4);
        for t in &facts.tests {
            assert!(t.is_vacuous(), "test {} should be vacuous", t.name);
        }
        let empty = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("empty test"))
            .unwrap();
        assert_eq!(empty.total_asserts, 0);

        let t_lit = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("tautology literal"))
            .unwrap();
        assert!(t_lit.tautologies >= 1);

        let t_bool = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("tautology boolean"))
            .unwrap();
        assert!(t_bool.tautologies >= 1);

        let t_str = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("tautology string"))
            .unwrap();
        assert!(t_str.tautologies >= 1);
    }

    #[test]
    fn fixture_implicit_assertions_detected() {
        let src = include_str!("../../tests/fixtures/javascript/implicit_asserts.js");
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/implicit.test.js", src, &vocab)
            .expect("extract implicit");
        assert_eq!(facts.tests.len(), 3);
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(t.total_asserts >= 1);
        }
    }

    #[test]
    fn fixture_skips_at_all_levels_detected() {
        let src = include_str!("../../tests/fixtures/javascript/skips_suite.js");
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/skips.test.js", src, &vocab)
            .expect("extract skips");
        assert_eq!(facts.tests.len(), 4);
        for t in &facts.tests {
            assert!(t.ignored, "test {} should be marked ignored", t.name);
        }
    }

    #[test]
    fn fixture_comments_and_strings_not_counted_as_assertions() {
        let src = include_str!("../../tests/fixtures/javascript/comments_and_strings.js");
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/comments.test.js", src, &vocab)
            .expect("extract comments");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_helpers_and_lambdas_require_configuration() {
        let src = include_str!("../../tests/fixtures/javascript/helpers_and_lambdas.js");
        let unconfigured_vocab = AssertVocabulary::default();
        let unconfigured = JavaScriptPack
            .extract("test/helpers.test.js", src, &unconfigured_vocab)
            .unwrap();
        assert_eq!(unconfigured.tests.len(), 1);
        assert!(unconfigured.tests[0].is_vacuous());

        let configured_vocab = AssertVocabulary {
            helper_fns: vec!["assertValidUser".to_string()],
            ..Default::default()
        };
        let configured = JavaScriptPack
            .extract("test/helpers.test.js", src, &configured_vocab)
            .unwrap();
        assert_eq!(configured.tests.len(), 1);
        assert!(configured.tests[0].total_asserts >= 1);
        assert!(!configured.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_syntax_error_surfaces_parse_error() {
        let src = include_str!("../../tests/fixtures/javascript/syntax_error.js");
        let vocab = AssertVocabulary::default();
        let facts = JavaScriptPack
            .extract("test/broken.test.js", src, &vocab)
            .expect("extract broken");
        assert!(facts.has_parse_errors);
    }
}
