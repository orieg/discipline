//! Swift language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.
//!
//! XCTest (`func test*()` in an `XCTestCase` subclass, `XCTAssert*`, `XCTFail`,
//! `XCTUnwrap`, `XCTSkip*`) and Swift Testing (`@Test` functions, `#expect`, `#require`,
//! the `.disabled` trait), `// swiftlint:disable` as an escape hatch, `catch { }` and a
//! discarded `try?` as swallowed errors.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Swift language pack implementing [`LanguagePack`].
pub struct SwiftPack;

impl LanguagePack for SwiftPack {
    fn id(&self) -> &'static str {
        "swift"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Swift"
    }

    fn matches(&self, path: &str) -> bool {
        super::extension(path) == Some("swift")
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_swift::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Swift grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = SwiftExtractor {
            dead: super::reach::dead_ranges(root, src, &SWIFT_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_swift_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_escape_hatches(root);
        extractor.visit_node(root, &mut Vec::new(), false, false);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &SWIFT_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &SWIFT_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            let whole_file = is_swift_test_path(path)
                || functions::test_path(path)
                || functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &SWIFT_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &SWIFT_RETRIES);
        if functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &SWIFT_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &SWIFT_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "comment",
                "multiline_comment",
                "line_string_literal",
                "multi_line_string_literal",
            ],
        );
        Ok(extractor.facts)
    }
}

/// Whether a path is conventionally Swift test code: `Tests/` (SwiftPM), a target named
/// `*Tests`, or a file named `*Tests.swift` / `*Test.swift`.
pub fn is_swift_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename.strip_suffix(".swift").unwrap_or(filename);
    stem.ends_with("Tests")
        || stem.ends_with("Test")
        || path.starts_with("Tests/")
        || path.contains("/Tests/")
        || path
            .split('/')
            .any(|seg| seg.ends_with("Tests") && seg != filename)
}

/// XCTest assertions that compare two values.
const XCT_EQUALITY: &[&str] = &[
    "XCTAssertEqual",
    "XCTAssertIdentical",
    "XCTAssertEqualObjects",
];

struct SwiftExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

impl<'a> SwiftExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    /// Attributes on a declaration: (name, whole text). `@Test(.disabled())` is
    /// `("Test", "@Test(.disabled())")`.
    fn attributes(&self, node: Node) -> Vec<(&'a str, &'a str)> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                let mut inner = child.walk();
                for m in child.children(&mut inner) {
                    if m.kind() == "attribute" {
                        let name = m.named_child(0).map(|t| self.text(t).trim()).unwrap_or("");
                        out.push((name, self.text(m)));
                    }
                }
            }
        }
        out
    }

    /// `// swiftlint:disable rule` and `// swiftlint:disable:next rule` comments.
    fn collect_escape_hatches(&mut self, node: Node) {
        if matches!(node.kind(), "comment" | "multiline_comment") {
            let text = self.text(node);
            let body = text
                .trim_start_matches("//")
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim();
            if let Some(rest) = body.strip_prefix("swiftlint:disable") {
                let rule = rest
                    .trim_start_matches(":next")
                    .trim_start_matches(":this")
                    .trim_start_matches(":previous")
                    .split_whitespace()
                    .next()
                    .unwrap_or("all")
                    .to_string();
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

    /// Whether a type inherits from `XCTestCase` (directly, as the grammar shows it).
    fn is_xctest_case(&self, node: Node) -> bool {
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .any(|c| c.kind() == "inheritance_specifier" && self.text(c).trim() == "XCTestCase");
        found
    }

    fn visit_node(
        &mut self,
        node: Node,
        type_stack: &mut Vec<String>,
        in_xctest: bool,
        parent_ignored: bool,
    ) {
        match node.kind() {
            "class_declaration" | "protocol_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_else(|| "Anonymous".to_string());
                let xctest = self.is_xctest_case(node);
                let ignored = parent_ignored
                    || self
                        .attributes(node)
                        .iter()
                        .any(|(n, t)| *n == "Suite" && t.contains(".disabled"));
                type_stack.push(name);
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    let members: Vec<Node> = body.children(&mut cursor).collect();
                    for m in members {
                        self.visit_node(m, type_stack, xctest, ignored);
                    }
                }
                type_stack.pop();
            }
            "function_declaration" => {
                self.visit_function(node, type_stack, in_xctest, parent_ignored)
            }
            _ => {
                let mut cursor = node.walk();
                let children: Vec<Node> = node.children(&mut cursor).collect();
                for child in children {
                    self.visit_node(child, type_stack, in_xctest, parent_ignored);
                }
            }
        }
    }

    fn has_parameters(node: Node) -> bool {
        let mut cursor = node.walk();
        let found = node.children(&mut cursor).any(|c| c.kind() == "parameter");
        found
    }

    fn function_body(node: Node) -> Option<Node> {
        node.child_by_field_name("body")
    }

    fn visit_function(
        &mut self,
        node: Node,
        type_stack: &[String],
        in_xctest: bool,
        parent_ignored: bool,
    ) {
        let name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).trim_matches('`'))
            .unwrap_or("");
        let mut annotated = false;
        let mut ignored = parent_ignored;
        for (aname, atext) in self.attributes(node) {
            if aname == "Test" {
                annotated = true;
                if atext.contains(".disabled") {
                    ignored = true;
                }
            }
        }
        // XCTest runs `func test*()` with no parameters in an `XCTestCase` subclass (or
        // any type in a test path, whose superclass may live in another file).
        let xctest = name.starts_with("test")
            && !Self::has_parameters(node)
            && (in_xctest || (self.is_test_path && !type_stack.is_empty()));
        if annotated || xctest {
            let full_name = if type_stack.is_empty() {
                name.to_string()
            } else {
                format!("{}.{}", type_stack.join("."), name)
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
                super::dispatch_calls(body, self.src, &SWIFT_DISPATCH, &mut direct_calls);
            }
            self.facts.tests.push(test_fn);
            self.test_calls.push(direct_calls);
        } else if let Some(body) = Self::function_body(node) {
            let mut helper_fn = TestFn::default();
            let mut dummy_calls = Vec::new();
            self.scan_node(body, &mut helper_fn, &mut dummy_calls);
            helper_fn.total_asserts += super::count_failure_exits(
                body,
                self.src,
                &["control_transfer_statement", "call_expression"],
                &["throw ", "throw\n", "fatalError(", "preconditionFailure("],
                &[
                    "lambda_literal",
                    "function_declaration",
                    "class_declaration",
                ],
            );
            self.helpers
                .entry(name.to_string())
                .or_insert(super::HelperFacts {
                    total_asserts: helper_fn.total_asserts,
                    strong_asserts: helper_fn.strong_asserts,
                    tautologies: helper_fn.tautologies,
                    fatal_asserts: helper_fn.fatal_asserts,
                });
        }
    }

    /// The callee of a call or macro: `XCTAssertEqual(...)`, `#expect(...)` as `expect`,
    /// `self.check(...)` as `check`. The flag says whether it resolves to this type.
    fn callee(&self, call: Node) -> (&'a str, bool) {
        let Some(c0) = call.named_child(0) else {
            return ("", false);
        };
        match c0.kind() {
            "simple_identifier" => (self.text(c0), true),
            "navigation_expression" => {
                let target_self = c0
                    .child_by_field_name("target")
                    .is_some_and(|t| self.text(t) == "self");
                let suffix = c0
                    .child_by_field_name("suffix")
                    .and_then(|s| s.child_by_field_name("suffix"))
                    .map(|s| self.text(s))
                    .unwrap_or("");
                (suffix, target_self)
            }
            _ => ("", false),
        }
    }

    fn arguments<'b>(&self, call: Node<'b>) -> Vec<Node<'b>> {
        let mut out = Vec::new();
        let mut stack = vec![call];
        while let Some(n) = stack.pop() {
            if n.kind() == "value_arguments" {
                let mut cursor = n.walk();
                for a in n.named_children(&mut cursor) {
                    if let Some(v) = a.child_by_field_name("value") {
                        out.push(v);
                    }
                }
                break;
            }
            if n.kind() == "call_suffix" || n == call {
                let mut cursor = n.walk();
                let children: Vec<Node> = n.named_children(&mut cursor).collect();
                for c in children.into_iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }

    fn scan_node(&self, node: Node, test_fn: &mut TestFn, direct_calls: &mut Vec<String>) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "call_expression" => self.inspect_call(node, test_fn, direct_calls),
            "macro_invocation" => self.inspect_macro(node, test_fn),
            "control_transfer_statement" => {
                // `throw XCTSkip("...")` skips the test.
                let t = self.text(node);
                if t.starts_with("throw") && t.contains("XCTSkip") {
                    test_fn.ignored = true;
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.scan_node(child, test_fn, direct_calls);
        }
    }

    /// `#expect(cond)` / `#require(value)`: strong when the condition compares two
    /// values, a tautology when it is a literal `true` or compares a value with itself.
    fn inspect_macro(&self, node: Node, test_fn: &mut TestFn) {
        let (name, _) = self.callee(node);
        if !matches!(name, "expect" | "require") {
            return;
        }
        test_fn.total_asserts += 1;
        let args = self.arguments(node);
        let Some(first) = args.first() else {
            return;
        };
        match first.kind() {
            "boolean_literal" if self.text(*first) == "true" => test_fn.tautologies += 1,
            "equality_expression" | "comparison_expression" => {
                let lhs = first.child_by_field_name("lhs").map(|n| self.text(n));
                let rhs = first.child_by_field_name("rhs").map(|n| self.text(n));
                if lhs.is_some() && lhs == rhs {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            _ if name == "require" => test_fn.strong_asserts += 1,
            _ => {}
        }
    }

    fn inspect_call(&self, node: Node, test_fn: &mut TestFn, direct_calls: &mut Vec<String>) {
        let (name, local) = self.callee(node);
        if local && !name.is_empty() {
            direct_calls.push(name.to_string());
        }
        if name.starts_with("XCTSkip") {
            test_fn.ignored = true;
            return;
        }
        let args = self.arguments(node);
        let arg = |i: usize| args.get(i).map(|a| self.text(*a).trim()).unwrap_or("");
        match name {
            n if XCT_EQUALITY.contains(&n) => {
                test_fn.total_asserts += 1;
                if args.len() >= 2 && arg(0) == arg(1) {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "XCTAssertTrue" | "XCTAssert" | "XCTAssertFalse" => {
                test_fn.total_asserts += 1;
                let literal = if name == "XCTAssertFalse" {
                    "false"
                } else {
                    "true"
                };
                if arg(0) == literal {
                    test_fn.tautologies += 1;
                }
            }
            "XCTAssertNil" | "XCTAssertNotNil" => test_fn.total_asserts += 1,
            "XCTAssertNotEqual"
            | "XCTAssertGreaterThan"
            | "XCTAssertGreaterThanOrEqual"
            | "XCTAssertLessThan"
            | "XCTAssertLessThanOrEqual"
            | "XCTAssertEqualWithAccuracy"
            | "XCTAssertThrowsError"
            | "XCTAssertNoThrow"
            | "XCTUnwrap"
            | "XCTFail" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            other => {
                if self.vocab.helper_fns.iter().any(|h| h == other) {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
            }
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (test, calls) in self.facts.tests.iter_mut().zip(&self.test_calls) {
            for call in calls {
                let Some(h) = self.helpers.get(call) else {
                    continue;
                };
                if self.vocab.helper_fns.iter().any(|n| n == call) {
                    test.total_asserts = test.total_asserts.saturating_sub(1);
                    test.strong_asserts = test.strong_asserts.saturating_sub(1);
                }
                test.total_asserts += h.total_asserts;
                test.strong_asserts += h.strong_asserts;
                test.tautologies += h.tautologies;
                test.fatal_asserts += h.fatal_asserts;
                if h.total_asserts > h.tautologies {
                    test.helper_checks += 1;
                }
            }
        }
    }
}

fn swift_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    let mut cursor = node.walk();
    let annotated = node.children(&mut cursor).any(|c| {
        c.kind() == "modifiers" && c.utf8_text(src.as_bytes()).unwrap_or("").contains("@Test")
    });
    annotated
        || (name.starts_with("test") && is_swift_test_path(path))
        || is_swift_test_path(path)
        || functions::test_path(path)
}

pub const SWIFT_FUNCTIONS: FunctionSpec = FunctionSpec {
    // A protocol requirement has no body.
    function_kinds: &["function_declaration"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["comment", "multiline_comment"],
    skip: functions::skip_none,
    is_test: swift_fn_is_test,
    classify: functions::classify_swift,
};

pub const SWIFT_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &[],
};

pub const SWIFT_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_block"],
    arm_of: &[],
    body_fields: &["statements"],
    ignored_kinds: &["comment", "multiline_comment"],
    trivial: &["return", "return nil", "return false", "continue", "break"],
    // `try? f()` as a statement, or bound to `_`, throws the error away.
    discard_kinds: &["try_expression"],
    discards: super::handlers::swift_discards,
    classify_discard: Some(super::handlers::swift_discard_class),
    call_value_kinds: &[],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
};

pub const SWIFT_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["attribute"],
};

pub const SWIFT_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["statements"],
    ignored_kinds: &["comment", "multiline_comment"],
    terminators: &["return", "throw", "fatalError(", "preconditionFailure("],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const SWIFT_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["array_literal"],
    names: &["simple_identifier"],
    references: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(path: &str, src: &str) -> ParsedFileFacts {
        SwiftPack
            .extract(path, src, &AssertVocabulary::default())
            .expect("extract succeeds")
    }

    #[test]
    fn xctest_methods_assertions_tautologies_and_skips() {
        let src = "import XCTest\n\nfinal class CartTests: XCTestCase {\n    func testTotal() throws {\n        XCTAssertEqual(cart.total, 3)\n        XCTAssertTrue(true)\n        XCTAssertNotNil(cart)\n    }\n\n    func testSkipped() throws {\n        try XCTSkipIf(isCI)\n        XCTAssertEqual(1, 1)\n    }\n\n    func testEmpty() {\n    }\n\n    func helper(_ x: Int) {\n        XCTAssertEqual(x, 1)\n    }\n\n    func testViaHelper() {\n        helper(load())\n    }\n}\n";
        let f = facts("Tests/CartTests/CartTests.swift", src);
        let by = |n: &str| {
            f.tests
                .iter()
                .find(|t| t.name == n)
                .unwrap_or_else(|| panic!("{n}: {:?}", f.tests))
        };
        let total = by("CartTests.testTotal");
        assert_eq!(
            (total.total_asserts, total.strong_asserts, total.tautologies),
            (3, 1, 1)
        );
        let skipped = by("CartTests.testSkipped");
        assert!(skipped.ignored);
        assert_eq!(skipped.tautologies, 1);
        assert!(by("CartTests.testEmpty").is_vacuous());
        let via = by("CartTests.testViaHelper");
        assert_eq!((via.total_asserts, via.helper_checks), (1, 1));
        assert_eq!(f.tests.len(), 4, "the parameterised helper is not a test");
    }

    #[test]
    fn swift_testing_expect_require_and_the_disabled_trait() {
        let src = "import Testing\n\n@Suite struct Parser {\n    @Test func parses() throws {\n        let v = try #require(parse(\"1\"))\n        #expect(v == 1)\n        #expect(true)\n    }\n\n    @Test(.disabled(\"flaky\")) func later() {\n        #expect(v == v)\n    }\n}\n\n@Test func free() { #expect(add(1, 1) == 2) }\n";
        let f = facts("Sources/App/ParserSpec.swift", src);
        let by = |n: &str| {
            f.tests
                .iter()
                .find(|t| t.name == n)
                .unwrap_or_else(|| panic!("{n}: {:?}", f.tests))
        };
        let p = by("Parser.parses");
        assert_eq!(
            (p.total_asserts, p.strong_asserts, p.tautologies),
            (3, 2, 1)
        );
        let later = by("Parser.later");
        assert!(later.ignored);
        assert_eq!(later.tautologies, 1);
        assert_eq!(by("free").strong_asserts, 1);
    }

    #[test]
    fn swallowed_errors_stubs_and_swiftlint_disables() {
        let src = "// swiftlint:disable force_cast\nfunc save() {\n    do { try store.write() } catch { }\n    do { try store.write() } catch { print(error) ; throw error }\n    try? store.flush()\n    _ = try? store.sync()\n    let v = try? store.read()\n}\n\nfunc load() -> Int {\n    fatalError(\"not implemented\")\n}\n";
        let f = facts("Sources/App/Store.swift", src);
        let kinds: Vec<(usize, &str)> = f.swallowed.iter().map(|s| (s.line, s.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (3, "empty-handler"),
                (5, "discarded-result"),
                (6, "discarded-result")
            ]
        );
        assert!(matches!(
            &f.escape_hatches[0],
            EscapeHatchSite::LinterDisable { rule, .. } if rule == "force_cast"
        ));
        let load = f.functions.iter().find(|x| x.name == "load").unwrap();
        assert!(
            matches!(load.shape, functions::BodyShape::Stub(_)),
            "{load:?}"
        );
    }

    #[test]
    fn test_paths_follow_swiftpm_and_xcode_conventions() {
        assert!(is_swift_test_path("Tests/AppTests/CartTests.swift"));
        assert!(is_swift_test_path("AppTests/Cart.swift"));
        assert!(is_swift_test_path("Sources/App/CartTests.swift"));
        assert!(!is_swift_test_path("Sources/App/Cart.swift"));
    }
}
