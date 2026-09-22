//! Kotlin language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.
//!
//! JUnit 4 / 5 and TestNG annotations, kotlin.test and JUnit `assert*`, AssertJ / Truth
//! `assertThat` chains, Kotest matchers (`shouldBe` as an infix or a call) and Kotest
//! specs (`StringSpec`, `FunSpec`, `DescribeSpec`, `ShouldSpec`, `ExpectSpec`,
//! `FeatureSpec`, `BehaviorSpec`), `@Disabled` / `@Ignore` / `@Test(enabled = false)`,
//! `@Suppress` as an escape hatch.
//!
//! Grammar note: the grammar reads `assertThrows<E> { ... }` (a generic call with a
//! trailing lambda and no parentheses) as two comparisons; that shape is recognised by
//! its text.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Kotlin language pack implementing [`LanguagePack`].
pub struct KotlinPack;

impl LanguagePack for KotlinPack {
    fn id(&self) -> &'static str {
        "kotlin"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Kotlin"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("kt" | "kts"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Kotlin grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = KotlinExtractor {
            src: src.as_bytes(),
            vocab,
            is_test_path: is_kotlin_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_escape_hatches(root);
        extractor.visit_node(root, &mut Vec::new(), false);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &KOTLIN_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &KOTLIN_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            let whole_file = is_kotlin_test_path(path)
                || functions::test_path(path)
                || functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &KOTLIN_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &KOTLIN_RETRIES);
        if functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &KOTLIN_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &KOTLIN_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "line_comment",
                "block_comment",
                "string_literal",
                "multiline_string_literal",
            ],
        );
        Ok(extractor.facts)
    }
}

/// Determines whether a path is conventionally a Kotlin test file.
pub fn is_kotlin_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename
        .strip_suffix(".kt")
        .or_else(|| filename.strip_suffix(".kts"))
        .unwrap_or(filename);
    stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.ends_with("Spec")
        || stem.ends_with("IT")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
        || path.contains("/androidTest/")
}

/// Test-defining calls of the Kotest spec styles, and the containers that nest them.
const KOTEST_TESTS: &[&str] = &["test", "it", "should", "expect", "then", "scenario"];
const KOTEST_CONTAINERS: &[&str] = &[
    "describe",
    "context",
    "given",
    "when",
    "feature",
    "and",
    "xdescribe",
    "xcontext",
];

/// Matchers that hold for nearly any value.
const WEAK_MATCHERS: &[&str] = &[
    "shouldNotBeNull",
    "shouldBeNull",
    "shouldBeTrue",
    "shouldBeFalse",
    "shouldBeEmpty",
    "shouldNotBeEmpty",
    "shouldBeInstanceOf",
];

/// `assertThrows<E> { }` and its relatives, as the grammar shows them: a comparison chain
/// whose text starts with the call name and a type argument.
const GENERIC_LAMBDA_ASSERTS: &[&str] = &[
    "assertThrows<",
    "assertThrowsExactly<",
    "assertDoesNotThrow<",
    "assertFailsWith<",
    "assertIs<",
    "assertIsNot<",
    "shouldThrow<",
    "shouldThrowExactly<",
    "shouldNotThrow<",
];

struct KotlinExtractor<'a> {
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

impl<'a> KotlinExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    /// The first identifier under an annotation: `Disabled` in `@Disabled("slow")`.
    fn annotation_name(&self, node: Node) -> &'a str {
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if n.kind() == "identifier" {
                return self.text(n);
            }
            let mut cursor = n.walk();
            let children: Vec<Node> = n.named_children(&mut cursor).collect();
            for c in children.into_iter().rev() {
                stack.push(c);
            }
        }
        ""
    }

    /// Annotations on a declaration: (name, whole text).
    fn annotations(&self, node: Node) -> Vec<(&'a str, &'a str)> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                let mut inner = child.walk();
                for m in child.children(&mut inner) {
                    if m.kind() == "annotation" {
                        out.push((self.annotation_name(m), self.text(m)));
                    }
                }
            }
        }
        out
    }

    fn collect_escape_hatches(&mut self, node: Node) {
        if node.kind() == "annotation" {
            let name = self.annotation_name(node);
            if matches!(name, "Suppress" | "SuppressWarnings" | "SuppressLint") {
                let text = self.text(node);
                let rule = text
                    .split_once('(')
                    .map(|(_, r)| r.trim_end_matches(')').trim().trim_matches('"').to_string())
                    .unwrap_or_else(|| "all".to_string());
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line: node.start_position().row + 1,
                        rule,
                        snippet: text.to_string(),
                    });
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_escape_hatches(child);
        }
    }

    fn visit_node(&mut self, node: Node, class_stack: &mut Vec<String>, parent_ignored: bool) {
        match node.kind() {
            "class_declaration" | "object_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_else(|| "Anonymous".to_string());
                let ignored = parent_ignored
                    || self
                        .annotations(node)
                        .iter()
                        .any(|(n, _)| matches!(*n, "Disabled" | "Ignore"));
                class_stack.push(name);
                // A Kotest spec: `class X : StringSpec({ ... })`.
                let mut cursor = node.walk();
                let children: Vec<Node> = node.children(&mut cursor).collect();
                for child in &children {
                    if child.kind() == "delegation_specifiers" {
                        self.visit_spec_lambdas(*child, class_stack, ignored);
                    }
                }
                for child in children {
                    if child.kind() == "class_body" {
                        let mut inner = child.walk();
                        let body: Vec<Node> = child.children(&mut inner).collect();
                        for c in body {
                            self.visit_node(c, class_stack, ignored);
                        }
                    }
                }
                class_stack.pop();
            }
            "function_declaration" => self.visit_function(node, class_stack, parent_ignored),
            _ => {
                let mut cursor = node.walk();
                let children: Vec<Node> = node.children(&mut cursor).collect();
                for child in children {
                    self.visit_node(child, class_stack, parent_ignored);
                }
            }
        }
    }

    /// Every lambda passed to a supertype constructor holds Kotest test definitions.
    fn visit_spec_lambdas(&mut self, node: Node, class_stack: &[String], ignored: bool) {
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if n.kind() == "lambda_literal" {
                self.visit_spec_body(n, class_stack, ignored);
                continue;
            }
            let mut cursor = n.walk();
            let children: Vec<Node> = n.named_children(&mut cursor).collect();
            for c in children.into_iter().rev() {
                stack.push(c);
            }
        }
    }

    /// Statements of a spec lambda: `"name" { }`, `test("name") { }`, `describe("x") { }`.
    fn visit_spec_body(&mut self, lambda: Node, class_stack: &[String], ignored: bool) {
        let mut cursor = lambda.walk();
        let stmts: Vec<Node> = lambda.named_children(&mut cursor).collect();
        for stmt in stmts {
            if stmt.kind() != "call_expression" {
                continue;
            }
            let Some(callee) = stmt.named_child(0) else {
                continue;
            };
            let Some(body) = self.trailing_lambda(stmt) else {
                continue;
            };
            let head = self.text(stmt).lines().next().unwrap_or("");
            let configured_off = head.contains("enabled = false") || head.contains("enabled=false");
            let (desc, kind): (String, &str) = match callee.kind() {
                // StringSpec: the string is the test.
                "string_literal" => (self.text(callee).trim_matches('"').to_string(), "test"),
                // FunSpec and friends: `test("name")`, `describe("name")`.
                "call_expression" => {
                    let Some(f) = callee.named_child(0) else {
                        continue;
                    };
                    if f.kind() != "identifier" {
                        continue;
                    }
                    let fname = self.text(f);
                    let desc = self.first_string_argument(callee).unwrap_or(fname);
                    let base = fname.strip_prefix('x').unwrap_or(fname);
                    if KOTEST_TESTS.contains(&base) {
                        (
                            desc.to_string(),
                            if fname.starts_with('x') {
                                "xtest"
                            } else {
                                "test"
                            },
                        )
                    } else if KOTEST_CONTAINERS.contains(&fname)
                        || KOTEST_CONTAINERS.contains(&base)
                    {
                        (
                            desc.to_string(),
                            if fname.starts_with('x') {
                                "xcontainer"
                            } else {
                                "container"
                            },
                        )
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };
            let bang = desc.starts_with('!');
            let desc = desc.trim_start_matches('!').to_string();
            let this_ignored = ignored || bang || configured_off || kind.starts_with('x');
            let full_name = if class_stack.is_empty() {
                desc
            } else {
                format!("{}.{}", class_stack.join("."), desc)
            };
            if kind.ends_with("container") {
                self.visit_spec_body(body, class_stack, this_ignored);
                continue;
            }
            let mut test_fn = TestFn {
                name: full_name,
                line: stmt.start_position().row + 1,
                end_line: stmt.end_position().row + 1,
                ignored: this_ignored,
                ..Default::default()
            };
            let mut direct_calls = Vec::new();
            self.scan_node(body, &mut test_fn, &mut direct_calls);
            self.facts.tests.push(test_fn);
            self.test_calls.push(direct_calls);
        }
    }

    fn trailing_lambda<'b>(&self, call: Node<'b>) -> Option<Node<'b>> {
        let mut cursor = call.walk();
        let found = call
            .named_children(&mut cursor)
            .find(|c| c.kind() == "annotated_lambda")
            .and_then(|a| {
                let mut inner = a.walk();
                let l = a
                    .named_children(&mut inner)
                    .find(|c| c.kind() == "lambda_literal");
                l
            });
        found
    }

    fn first_string_argument(&self, call: Node) -> Option<&'a str> {
        let mut cursor = call.walk();
        let args = call
            .named_children(&mut cursor)
            .find(|c| c.kind() == "value_arguments")?;
        let mut inner = args.walk();
        let found = args
            .named_children(&mut inner)
            .filter_map(|a| a.named_child(0))
            .find(|v| v.kind() == "string_literal")
            .map(|v| self.text(v).trim_matches('"'));
        found
    }

    fn visit_function(&mut self, node: Node, class_stack: &[String], parent_ignored: bool) {
        let name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).trim_matches('`'))
            .unwrap_or("");
        let mut annotated_test = false;
        let mut ignored = parent_ignored;
        for (aname, atext) in self.annotations(node) {
            match aname {
                "Test" => {
                    annotated_test = true;
                    if atext.contains("enabled = false") || atext.contains("enabled=false") {
                        ignored = true;
                    }
                }
                "ParameterizedTest" | "RepeatedTest" | "TestFactory" | "TestTemplate" => {
                    annotated_test = true;
                }
                "Disabled" | "Ignore" => ignored = true,
                _ => {}
            }
        }
        let is_test = annotated_test
            || (self.is_test_path && name.starts_with("test") && Self::has_zero_parameters(node));
        if is_test {
            let full_name = if class_stack.is_empty() {
                name.to_string()
            } else {
                format!("{}.{}", class_stack.join("."), name)
            };
            let mut test_fn = TestFn {
                name: full_name,
                line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                ignored,
                ..Default::default()
            };
            let mut direct_calls = Vec::new();
            if let Some(body) = Self::function_body(node) {
                self.scan_node(body, &mut test_fn, &mut direct_calls);
            }
            self.facts.tests.push(test_fn);
            self.test_calls.push(direct_calls);
        } else if self.is_test_path {
            if let Some(body) = Self::function_body(node) {
                let mut helper_fn = TestFn::default();
                let mut dummy_calls = Vec::new();
                self.scan_node(body, &mut helper_fn, &mut dummy_calls);
                self.helpers.insert(
                    name.to_string(),
                    super::HelperFacts {
                        total_asserts: helper_fn.total_asserts,
                        strong_asserts: helper_fn.strong_asserts,
                        tautologies: helper_fn.tautologies,
                        fatal_asserts: helper_fn.fatal_asserts,
                    },
                );
            }
        }
    }

    fn function_body(node: Node) -> Option<Node> {
        let mut cursor = node.walk();
        let found = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "function_body");
        found
    }

    fn has_zero_parameters(node: Node) -> bool {
        let mut cursor = node.walk();
        let params = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "function_value_parameters");
        params.is_none_or(|p| p.named_child_count() == 0)
    }

    /// The name a call is judged by: `assertEquals(...)`, `Assertions.assertEquals(...)`,
    /// `assertThat(x).isEqualTo(3)` (judged as `isEqualTo`, then `assertThat` on recursion).
    fn callee_name(&self, call: Node) -> (&'a str, bool) {
        let Some(c0) = call.named_child(0) else {
            return ("", false);
        };
        match c0.kind() {
            "identifier" => (self.text(c0), true),
            "navigation_expression" => {
                let mut cursor = c0.walk();
                let last = c0
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() == "identifier")
                    .last()
                    .map(|n| self.text(n))
                    .unwrap_or("");
                (last, false)
            }
            _ => ("", false),
        }
    }

    fn arguments<'b>(&self, call: Node<'b>) -> Vec<Node<'b>> {
        let mut cursor = call.walk();
        let Some(args) = call
            .named_children(&mut cursor)
            .find(|c| c.kind() == "value_arguments")
        else {
            return Vec::new();
        };
        let mut inner = args.walk();
        args.named_children(&mut inner)
            .map(|a| {
                a.named_child(a.named_child_count().saturating_sub(1))
                    .unwrap_or(a)
            })
            .collect()
    }

    fn scan_node(&self, node: Node, test_fn: &mut TestFn, direct_calls: &mut Vec<String>) {
        match node.kind() {
            "call_expression" => self.inspect_call(node, test_fn, direct_calls),
            "infix_expression" => {
                let mut cursor = node.walk();
                let parts: Vec<Node> = node.named_children(&mut cursor).collect();
                if parts.len() == 3 && parts[1].kind() == "identifier" {
                    let op = self.text(parts[1]);
                    if op.starts_with("should") {
                        self.count_matcher(op, self.text(parts[0]), self.text(parts[2]), test_fn);
                    }
                }
            }
            "binary_expression" => {
                // The outer comparison carries the lambda; the inner `name < E` does not.
                let t = self.text(node);
                if t.contains('{') && GENERIC_LAMBDA_ASSERTS.iter().any(|g| t.starts_with(g)) {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.scan_node(child, test_fn, direct_calls);
        }
    }

    fn count_matcher(&self, op: &str, lhs: &str, rhs: &str, test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;
        if WEAK_MATCHERS.contains(&op) {
            return;
        }
        if matches!(op, "shouldBe" | "shouldBeEqual" | "shouldBeSameInstanceAs")
            && lhs.trim() == rhs.trim()
        {
            test_fn.tautologies += 1;
        } else {
            test_fn.strong_asserts += 1;
        }
    }

    fn inspect_call(&self, node: Node, test_fn: &mut TestFn, direct_calls: &mut Vec<String>) {
        let (name, is_local) = self.callee_name(node);
        if is_local && !name.is_empty() {
            direct_calls.push(name.to_string());
        }
        let args = self.arguments(node);
        let arg = |i: usize| args.get(i).map(|a| self.text(*a).trim()).unwrap_or("");
        match name {
            "assertTrue" | "assertFalse" => {
                test_fn.total_asserts += 1;
                let literal = if name == "assertTrue" {
                    "true"
                } else {
                    "false"
                };
                if arg(0) == literal {
                    test_fn.tautologies += 1;
                }
            }
            "assert" => {
                test_fn.total_asserts += 1;
                if arg(0) == "true" {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "assertEquals" | "assertSame" | "assertContentEquals" => {
                test_fn.total_asserts += 1;
                if args.len() >= 2 && arg(0) == arg(1) {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "assertNull" => {
                test_fn.total_asserts += 1;
                if arg(0) == "null" {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "assertNotEquals"
            | "assertNotSame"
            | "assertNotNull"
            | "assertThrows"
            | "assertThrowsExactly"
            | "assertDoesNotThrow"
            | "assertFailsWith"
            | "assertFails"
            | "assertThat"
            | "assertThatThrownBy"
            | "assertAll"
            | "assertArrayEquals"
            | "assertIterableEquals"
            | "assertLinesMatch"
            | "assertContains"
            | "assertIs"
            | "assertIsNot"
            | "assertInstanceOf"
            | "fail" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            other if other.starts_with("should") => {
                // `x.shouldBe(y)`: the receiver is the navigation's first part.
                let receiver = node
                    .named_child(0)
                    .and_then(|c| c.named_child(0))
                    .map(|r| self.text(r))
                    .unwrap_or("");
                self.count_matcher(other, receiver, arg(0), test_fn);
            }
            other => {
                let custom = other.starts_with("assert")
                    && other.chars().nth(6).is_some_and(|c| c.is_ascii_uppercase());
                if custom || self.vocab.helper_fns.iter().any(|h| h == other) {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
            }
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (i, test) in self.facts.tests.iter_mut().enumerate() {
            if let Some(calls) = self.test_calls.get(i) {
                for call in calls {
                    if let Some(h) = self.helpers.get(call) {
                        if self.vocab.helper_fns.iter().any(|name| name == call) {
                            test.total_asserts = test.total_asserts.saturating_sub(1);
                            test.strong_asserts = test.strong_asserts.saturating_sub(1);
                        }
                        test.total_asserts += h.total_asserts;
                        test.strong_asserts += h.strong_asserts;
                        test.tautologies += h.tautologies;
                        test.fatal_asserts += h.fatal_asserts;
                    }
                }
            }
        }
    }
}

fn kotlin_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    let mut cursor = node.walk();
    let annotated = node.children(&mut cursor).any(|c| {
        c.kind() == "modifiers" && {
            let t = c.utf8_text(src.as_bytes()).unwrap_or("");
            t.contains("@Test") || t.contains("Test\n") || t.contains("@ParameterizedTest")
        }
    });
    annotated
        || (name.starts_with("test") && (is_kotlin_test_path(path) || functions::test_path(path)))
        || is_kotlin_test_path(path)
        || functions::test_path(path)
}

pub const KOTLIN_FUNCTIONS: FunctionSpec = FunctionSpec {
    // An abstract or interface member without a body has no `function_body`.
    function_kinds: &["function_declaration"],
    name_fields: &["name"],
    body_fields: &["function_body"],
    ignored_kinds: &["line_comment", "block_comment"],
    skip: functions::skip_none,
    is_test: kotlin_fn_is_test,
    classify: functions::classify_kotlin,
};

pub const KOTLIN_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    // The grammar has no callee field; the call's own text, cut at `(` or `{`, is judged.
    call_kinds: &["call_expression"],
    callee_fields: &[],
};

pub const KOTLIN_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_block"],
    body_fields: &["block"],
    ignored_kinds: &["line_comment", "block_comment"],
    // `try { } catch (e: E) { null }`: the try is an expression and `null` is its value.
    trivial: &[
        "null",
        "Unit",
        "return",
        "return null",
        "return false",
        "continue",
        "break",
    ],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    call_value_kinds: &[],
    // `runCatching { }.getOrNull()` / `.getOrDefault(x)` replace the failure with a value.
    silence_kinds: &["call_expression"],
    silences: super::handlers::kotlin_silences,
};

pub const KOTLIN_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["annotation"],
};

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(path: &str, src: &str) -> ParsedFileFacts {
        KotlinPack
            .extract(path, src, &AssertVocabulary::default())
            .expect("extract succeeds")
    }

    #[test]
    fn junit_tests_assertions_tautologies_and_skips() {
        let src = "import kotlin.test.*\n\nclass RepoTest {\n    @Test\n    fun `finds an item`() {\n        assertEquals(1, repo.find(1).id)\n        assertTrue(true)\n        Assertions.assertNotNull(repo)\n        assertThat(x).isEqualTo(3)\n    }\n\n    @Test\n    @Disabled(\"slow\")\n    fun disabled() {\n        assertEquals(1, 1)\n    }\n\n    @Ignore\n    @Test\n    fun old() {\n    }\n\n    @ParameterizedTest\n    @ValueSource(ints = [1, 2])\n    fun param(i: Int) {\n        assertNotNull(i)\n    }\n\n    @Test(enabled = false)\n    fun off() {\n        assertEquals(1, 2)\n    }\n\n    @Test\n    fun throws() {\n        assertThrows<IllegalStateException> { g() }\n        assertFailsWith<E> { g() }\n    }\n\n    private fun helper() {\n        assertEquals(1, 2)\n    }\n\n    @Test\n    fun viaHelper() {\n        helper()\n    }\n}\n";
        let f = facts("src/test/kotlin/RepoTest.kt", src);
        let names: Vec<&str> = f.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "RepoTest.finds an item",
                "RepoTest.disabled",
                "RepoTest.old",
                "RepoTest.param",
                "RepoTest.off",
                "RepoTest.throws",
                "RepoTest.viaHelper"
            ]
        );
        let t = &f.tests[0];
        assert_eq!(
            (t.total_asserts, t.strong_asserts, t.tautologies),
            (4, 3, 1)
        );
        assert!(!t.ignored && !t.is_vacuous());
        assert!(f.tests[1].ignored && f.tests[2].ignored && f.tests[4].ignored);
        assert!(!f.tests[3].ignored);
        assert_eq!(f.tests[5].strong_asserts, 2, "{:?}", f.tests[5]);
        assert_eq!(f.tests[6].total_asserts, 1, "{:?}", f.tests[6]);
        assert_eq!(f.tests[6].strong_asserts, 1);
    }

    #[test]
    fn kotest_specs_matchers_and_bang_or_x_skips() {
        let src = "class Adds : StringSpec({\n    \"adds\" {\n        (1 + 1) shouldBe 2\n        x shouldBe x\n        y.shouldBe(3)\n        z.shouldNotBeNull()\n    }\n    \"!skipped\" {\n        1 shouldBe 1\n    }\n})\n\nclass Fun : FunSpec({\n    test(\"works\") {\n        x shouldBe 1\n    }\n    xtest(\"later\") {\n    }\n    context(\"nested\") {\n        test(\"inner\") {\n            shouldThrow<E> { g() }\n        }\n    }\n})\n\nclass D : DescribeSpec({\n    describe(\"outer\") {\n        it(\"inner\") {\n            1 shouldBe 2\n        }\n        xit(\"off\") {\n        }\n    }\n})\n";
        let f = facts("src/test/kotlin/AddsSpec.kt", src);
        let rows: Vec<(&str, usize, usize, usize, bool)> = f
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.total_asserts,
                    t.strong_asserts,
                    t.tautologies,
                    t.ignored,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Adds.adds", 4, 2, 1, false),
                ("Adds.skipped", 1, 0, 1, true),
                ("Fun.works", 1, 1, 0, false),
                ("Fun.later", 0, 0, 0, true),
                ("Fun.inner", 1, 1, 0, false),
                ("D.inner", 1, 1, 0, false),
                ("D.off", 0, 0, 0, true),
            ]
        );
    }

    #[test]
    fn class_level_disabled_suppress_and_test_path_conventions() {
        let src = "@Disabled\nclass SlowTest {\n    @Suppress(\"UNCHECKED_CAST\")\n    @Test\n    fun a() {\n        assertEquals(1, 2)\n    }\n\n    fun testLegacy() {\n        assert(x > 1)\n    }\n}\n";
        let f = facts("app/src/test/kotlin/SlowTest.kt", src);
        assert_eq!(f.tests.len(), 2);
        assert!(f.tests[0].ignored && f.tests[1].ignored);
        assert_eq!(f.tests[1].strong_asserts, 1);
        assert_eq!(f.escape_hatches.len(), 1);
        assert!(matches!(
            &f.escape_hatches[0],
            EscapeHatchSite::LinterDisable { rule, .. } if rule == "UNCHECKED_CAST"
        ));
        assert!(is_kotlin_test_path("src/test/kotlin/a/B.kt"));
        assert!(is_kotlin_test_path("lib/FooSpec.kt"));
        assert!(!is_kotlin_test_path("src/main/kotlin/Foo.kt"));
        // Outside a test path, a `test*` function without an annotation is not a test.
        let g = facts(
            "src/main/kotlin/Foo.kt",
            "class Foo {\n    fun testValue() = 1\n}\n",
        );
        assert!(g.tests.is_empty());
    }
}
