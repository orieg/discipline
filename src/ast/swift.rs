//! Swift language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.
//!
//! XCTest (`func test*()` in an `XCTestCase` subclass, `XCTAssert*`, `XCTFail`,
//! `XCTUnwrap`, `XCTSkip*`) and Swift Testing (`@Test` functions, `#expect`, `#require`,
//! the `.disabled` trait), `// swiftlint:disable` as an escape hatch, `catch { }` and a
//! discarded `try?` as swallowed errors.

use anyhow::Result;
use tree_sitter::Node;

use super::ci_condition::{read_skip, Grammar, SkipCondition, SkipRead};
use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// The Swift parser, reachable only through a text that ends in a line break.
///
/// tree-sitter-swift 0.7.3 matches a compiler directive after `#` against the table `if`,
/// `elseif`, `else`, `endif` one character at a time (`src/scanner.c`,
/// `find_possible_compiler_directive`). A candidate stays possible while the character the
/// lexer is looking at equals the candidate's byte at the current index, and that test is
/// made on the byte that ends the candidate's string too. The lexer's character is 0 at
/// the end of the file and at a NUL byte, so there the terminator compares equal, the
/// index moves on, and the next pass reads the byte after the string: index 3 of `if`,
/// 5 of `else`, 6 of `endif`, 7 of `elseif`, and further for as long as the bytes it
/// finds are 0 (#621). That needs the character 0 right after a whole directive, which is
/// a file ending in `#if`, `#else`, `#elseif` or `#endif` with no line break after it, or
/// a NUL byte after one.
///
/// So the parser is given neither. The text it parses ends in a line break, so the end of
/// the file follows a line break and never a directive, and `crate::ast::source_text`
/// replaces every NUL byte in the copy the grammar reads. A file that already ends in a
/// line break is parsed as it is. One that does not is read as the same file with the
/// line break added: no byte offset or line of the file moves, and the tree of a
/// well-formed file is the same below its root. The pack reads node text from the
/// line-ended text, because a node the grammar marks as missing at the end of a broken
/// file can start at the added byte.
///
/// The compiler holds this: `LineEndedSource` has a private field, so outside this module
/// the only way to make one is `line_ended`, and `parse`, the only function that names
/// the grammar, takes nothing else.
mod swift_tree {
    use std::borrow::Cow;

    use anyhow::{anyhow, Result};

    /// A source that ends in a line break.
    pub(super) struct LineEndedSource<'a>(Cow<'a, str>);

    impl LineEndedSource<'_> {
        pub(super) fn as_str(&self) -> &str {
            &self.0
        }
    }

    /// `src`, with a line break added when it does not end in one.
    pub(super) fn line_ended(src: &str) -> LineEndedSource<'_> {
        LineEndedSource(if src.ends_with('\n') {
            Cow::Borrowed(src)
        } else {
            Cow::Owned(format!("{src}\n"))
        })
    }

    /// The grammar, for a caller that reads its node kinds and parses nothing.
    pub(super) fn language() -> tree_sitter::Language {
        tree_sitter_swift::LANGUAGE.into()
    }

    /// The syntax tree of the text; its byte ranges are the text's.
    pub(super) fn parse(text: &LineEndedSource) -> Result<tree_sitter::Tree> {
        debug_assert!(
            text.0.ends_with('\n'),
            "the Swift scanner must not reach the end of the file right after a directive"
        );
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&language())
            .map_err(|e| anyhow!("failed to load the Swift grammar: {e}"))?;
        crate::ast::source_text::parse(&mut parser, &text.0)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))
    }
}

/// The Swift grammar's node kinds, for tests that check a list of kinds against it.
/// Nothing is parsed through this: parsing goes through `swift_tree::parse` only.
#[cfg(test)]
pub(crate) fn grammar_for_node_kinds() -> tree_sitter::Language {
    swift_tree::language()
}

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

    fn is_test_path(&self, path: &str) -> bool {
        functions::is_test_file(path, Some(is_swift_test_path))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        // Everything below reads the line-ended text: the tree's byte ranges are its own.
        let text = swift_tree::line_ended(src);
        let src = text.as_str();
        let tree = swift_tree::parse(&text)?;
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
            inherited_skips: Vec::new(),
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
            let whole_file = functions::is_test_file(path, Some(is_swift_test_path))
                || functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            (
                extractor.facts.swallowed,
                extractor.facts.constant_fallbacks,
            ) = super::handlers::extract_with_constants(
                root,
                src,
                &SWIFT_HANDLERS,
                Some(&SWIFT_CONSTANTS),
                &is_test_line,
            );
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &SWIFT_RETRIES);
        if functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::method_checks::count(root, src, &mut extractor.facts, &SWIFT_RECEIVER_CALLS);
        super::helper_loops::count(root, src, &mut extractor.facts, &super::helper_loops::SWIFT);
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
    /// Conditional skips of the enclosing suites (`@Suite(.disabled(if: ..))`).
    inherited_skips: Vec<SkipRead>,
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

    /// What the Swift Testing traits of a `@Test` or `@Suite` attribute on `node` do:
    /// `.disabled(..)` skips, `.disabled(if: c)` skips when `c` holds, `.enabled(if: c)`
    /// when it does not. A trait that takes its condition another way (a closure) is
    /// read through the whole trait.
    fn trait_skips(&self, node: Node, attribute: &str) -> Vec<SkipRead> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for modifiers in node.children(&mut cursor) {
            if modifiers.kind() != "modifiers" {
                continue;
            }
            let mut inner = modifiers.walk();
            for attr in modifiers.children(&mut inner) {
                let name = attr.named_child(0).map(|t| self.text(t).trim());
                if attr.kind() != "attribute" || name != Some(attribute) {
                    continue;
                }
                let mut args = attr.walk();
                for call in attr.named_children(&mut args) {
                    if call.kind() != "call_expression" {
                        continue;
                    }
                    let callee = call
                        .named_child(0)
                        .filter(|c| c.kind() == "prefix_expression")
                        .and_then(|c| c.child_by_field_name("target"))
                        .map(|t| self.text(t));
                    let enabled = match callee {
                        Some("disabled") => false,
                        Some("enabled") => true,
                        _ => continue,
                    };
                    let mut stack = vec![call];
                    let mut labelled = None;
                    let mut trailing_closure = false;
                    while let Some(n) = stack.pop() {
                        if n.kind() == "lambda_literal" {
                            trailing_closure = true;
                            continue;
                        }
                        if n.kind() == "value_argument" {
                            let label = n.child_by_field_name("name").map(|l| self.text(l));
                            if label.map(str::trim) == Some("if") {
                                labelled = Some(n);
                            }
                            continue;
                        }
                        if n.id() == call.id()
                            || matches!(n.kind(), "call_suffix" | "value_arguments")
                        {
                            let mut c = n.walk();
                            stack.extend(n.named_children(&mut c));
                        }
                    }
                    let own = match labelled {
                        Some(condition) => Some((condition, enabled)),
                        None if enabled || trailing_closure => Some((call, false)),
                        None => None,
                    };
                    out.push(read_skip(Grammar::Swift, attr, own, self.src));
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
                let suite = self.trait_skips(node, "Suite");
                let ignored =
                    parent_ignored || suite.iter().any(|r| r.outcome == SkipCondition::Always);
                let inherited = self.inherited_skips.len();
                self.inherited_skips.extend(suite);
                type_stack.push(name);
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    let members: Vec<Node> = body.children(&mut cursor).collect();
                    for m in members {
                        self.visit_node(m, type_stack, xctest, ignored);
                    }
                }
                self.inherited_skips.truncate(inherited);
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
        let ignored = parent_ignored;
        for (aname, _) in self.attributes(node) {
            if aname == "Test" {
                annotated = true;
            }
        }
        let mut skips = self.inherited_skips.clone();
        skips.extend(self.trait_skips(node, "Test"));
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
            for read in skips {
                test_fn.record_skip(read);
            }
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
                    wraps: super::forwarding_wrapper_callee(
                        body,
                        &SWIFT_WRAPPER,
                        &SWIFT_LOCALS,
                        &dummy_calls,
                        self.src,
                    ),
                });
            let line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            self.facts.push_helper(
                super::TestHelperFacts {
                    name: name.to_string(),
                    line,
                    end_line,
                    total_asserts: helper_fn.total_asserts,
                    strong_asserts: helper_fn.strong_asserts,
                    tautologies: helper_fn.tautologies,
                    fatal_asserts: helper_fn.fatal_asserts,
                    helper_checks: 0,
                },
                dummy_calls,
            );
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
                // A thrown call is read where the call is; this is a thrown value.
                let t = self.text(node);
                let mut cursor = node.walk();
                let throws_call = node
                    .named_children(&mut cursor)
                    .any(|c| c.kind() == "call_expression");
                if t.starts_with("throw") && t.contains("XCTSkip") && !throws_call {
                    test_fn.record_skip(read_skip(Grammar::Swift, node, None, self.src));
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
            // `XCTSkipIf(c)` skips when `c` holds, `XCTSkipUnless(c)` when it does not.
            let condition = self.arguments(node).first().copied();
            let own = match name {
                "XCTSkipIf" => condition.map(|c| (c, false)),
                "XCTSkipUnless" => condition.map(|c| (c, true)),
                _ => None,
            };
            test_fn.record_skip(read_skip(Grammar::Swift, node, own, self.src));
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
            // Waiting on XCTest expectations fails the test when one is not fulfilled (or is
            // fulfilled fewer times than `expectedFulfillmentCount`). `wait` counts only with
            // its `for:` label: `semaphore.wait()` is not a check.
            "fulfillment" | "waitForExpectations" => test_fn.total_asserts += 1,
            "wait" if self.text(node).contains("(for:") => test_fn.total_asserts += 1,
            other => {
                if self
                    .vocab
                    .helper_fns
                    .iter()
                    .any(|h| super::helper_call_matches(other, h))
                {
                    test_fn.total_asserts += 1;
                }
            }
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (test, calls) in self.facts.tests.iter_mut().zip(&self.test_calls) {
            super::resolve_test_same_file_helpers(test, calls, self.vocab, |call| {
                super::helper_through_wrappers(call, &self.helpers)
            });
        }
    }
}

fn swift_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let mut cursor = node.walk();
    let annotated = node.children(&mut cursor).any(|c| {
        c.kind() == "modifiers" && c.utf8_text(src.as_bytes()).unwrap_or("").contains("@Test")
    });
    annotated || functions::is_test_file(path, Some(is_swift_test_path))
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

/// A method called on a receiver (`method_checks`).
pub const SWIFT_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[("call_expression", "", "navigation_expression", "suffix")],
        direct: &[],
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
    silence_node: None,
};

/// A handler statement that puts a number in place of the result (`constant-fallback`):
/// `ops = 150000.0`, `rec.ops = -1`, `return 150000.0`, `return [1.5, 2.0]`,
/// `return ["ops": 1.5]`. `nil` and `Double.nan` are not numeric literals; the grammar
/// reads `raw[0]` as a call, so a subscript on the left is not read.
pub const SWIFT_CONSTANTS: super::handlers::ConstantSpec = super::handlers::ConstantSpec {
    blocks: &["statements"],
    wrappers: &["directly_assignable_expression"],
    numbers: &[
        "integer_literal",
        "real_literal",
        "hex_literal",
        "oct_literal",
        "bin_literal",
    ],
    signs: &["prefix_expression"],
    assignments: &["assignment"],
    targets: &["simple_identifier", "navigation_expression"],
    calls: &["call_expression"],
    returns: &["control_transfer_statement"],
    value_is_last_expression: false,
    collections: &["array_literal", "dictionary_literal"],
    collection_holders: &[],
    pairs: &[],
    keys: &["line_string_literal", "integer_literal"],
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

/// A local a Swift wrapper computes and forwards: `let b = loc(d)`.
pub const SWIFT_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &[],
    binders: &["property_declaration"],
    pattern: &["name"],
    value: &["value"],
    names: &["simple_identifier"],
    holders: &["pattern"],
    refused: &["computed_property", "willset_didset_block"],
};

/// A Swift helper whose body is one call: `{ check(x, flag: true) }`, `{ return try check(x) }`.
pub const SWIFT_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &[
        "function_body",
        "statements",
        "control_transfer_statement",
        "try_expression",
        "await_expression",
    ],
    calls: &["call_expression"],
    arguments: &[
        "call_suffix",
        "value_arguments",
        "value_argument",
        "value_argument_label",
    ],
    references: &[("prefix_expression", "&")],
    plain: &["self_expression"],
    skip: &["comment", "multiline_comment", "try_operator"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"import XCTest

final class WrapTests: XCTestCase {
    func checked(_ x: Int, strict: Bool) {
        if strict && x != 1 { fatalError("x") }
    }
    func noop(_ x: Int, _ n: Int) {}
    func via(_ x: Int) {
        checked(x, strict: true)
    }
    func hollow(_ x: Int) {
        noop(x, 1)
    }
    func busy(_ x: Int) {
        checked(x, strict: true)
        prepare(x)
    }
    func ping(_ x: Int) {
        pong(x, 1)
    }
    func pong(_ x: Int, _ n: Int) {
        ping(x)
    }
    func testDirect() {
        checked(1, strict: true)
    }
    func testViaWrapper() {
        via(1)
    }
    func testHollowWrapper() {
        hollow(1)
    }
    func testBusyHelper() {
        busy(1)
    }
    func testWrapperCycle() {
        ping(1)
    }
}
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&SwiftPack, "Tests/WrapTests/WrapTests.swift", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
    }

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
        // `isCI` is not bound in the file: a conditional skip, and not a CI one.
        assert!(!skipped.ignored);
        assert_eq!(skipped.conditional_ignore.as_deref(), Some("isCI"));
        assert!(!skipped.is_ci_skip());
        assert_eq!(skipped.tautologies, 1);
        assert!(by("CartTests.testEmpty").is_vacuous());
        let via = by("CartTests.testViaHelper");
        assert_eq!((via.total_asserts, via.helper_checks), (1, 1));
        assert_eq!(f.tests.len(), 4, "the parameterised helper is not a test");
    }

    #[test]
    fn a_thrown_skip_is_reported_as_ignored_not_as_vacuous() {
        let src = "final class ATests: XCTestCase {\n  func testLater() throws {\n    throw XCTSkip(\"later\")\n    XCTAssertEqual(a, 1)\n  }\n}\n";
        let f = facts("Tests/ATests.swift", src);
        let t = &f.tests[0];
        assert!(t.ignored);
        assert_eq!(
            t.total_asserts, 1,
            "the assertion after a skip is not dead code: {t:?}"
        );
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

    #[test]
    fn waiting_on_expectations_is_an_assertion_and_a_semaphore_wait_is_not() {
        let v = AssertVocabulary::default();
        let src = "import XCTest\nfinal class StreamTests: XCTestCase {\n    func testEvents() async {\n        let received = expectation(description: \"received\")\n        received.expectedFulfillmentCount = 4\n        for await _ in stream() { received.fulfill() }\n        await fulfillment(of: [received])\n    }\n    func testLegacy() {\n        let done = expectation(description: \"done\")\n        run { done.fulfill() }\n        wait(for: [done], timeout: 1)\n    }\n    func testOldest() {\n        run { }\n        waitForExpectations(timeout: 1)\n    }\n    func testSemaphore() {\n        let s = DispatchSemaphore(value: 0)\n        s.wait()\n    }\n}\n";
        let facts = SwiftPack
            .extract("Tests/StreamTests.swift", src, &v)
            .unwrap();
        let by = |n: &str| {
            facts
                .tests
                .iter()
                .find(|t| t.name.ends_with(n))
                .unwrap()
                .total_asserts
        };
        assert_eq!(
            (by("testEvents"), by("testLegacy"), by("testOldest")),
            (1, 1, 1)
        );
        assert_eq!(by("testSemaphore"), 0);
    }

    /// What `find_possible_compiler_directive` (tree-sitter-swift 0.7.3, `src/scanner.c`)
    /// reads for the text that follows a `#`: how many times it reads a directive's string
    /// at an index past its terminating byte. The lexer's character is 0 at the end of
    /// the text and at a NUL byte. A byte past a string is taken as unequal to the
    /// character, which ends that candidate, so the count is a lower bound.
    fn reads_past_a_directive_string(after_hash: &[u8]) -> usize {
        const DIRECTIVES: [&[u8]; 4] = [b"if\0", b"elseif\0", b"else\0", b"endif\0"];
        let mut possible = [true; 4];
        let mut past = 0;
        let mut index = 0;
        loop {
            let lookahead = after_hash.get(index).copied().unwrap_or(0);
            for (candidate, directive) in DIRECTIVES.iter().enumerate() {
                if !possible[candidate] {
                    continue;
                }
                let Some(&expected) = directive.get(index) else {
                    past += 1;
                    possible[candidate] = false;
                    continue;
                };
                if expected != lookahead {
                    possible[candidate] = false;
                }
            }
            if !possible.contains(&true) {
                return past;
            }
            index += 1;
        }
    }

    /// The reads past a directive string over every `#` of `text`.
    fn reads_past_in(text: &str) -> usize {
        text.match_indices('#')
            .map(|(at, _)| reads_past_a_directive_string(&text.as_bytes()[at + 1..]))
            .sum()
    }

    /// The text the grammar is handed for `src`.
    fn parsed_text(src: &str) -> String {
        crate::ast::source_text::parse_text(swift_tree::line_ended(src).as_str()).into_owned()
    }

    /// The scan reads past a directive's string exactly when the character 0 follows a
    /// whole directive: the end of the file, or a NUL byte. The text the parser is given
    /// has neither. Controls: a directive followed by anything else, and a `#` that is
    /// not a directive, read nothing past a string.
    #[test]
    fn the_parser_is_never_given_a_directive_followed_by_the_character_zero() {
        for directive in ["if", "elseif", "else", "endif"] {
            let at_end = format!("let a = 1\n#{directive}");
            let before_nul = format!("#{directive}\0 X\nlet a = 1\n");
            assert_eq!(reads_past_in(&at_end), 1, "{at_end:?}");
            assert_eq!(reads_past_in(&before_nul), 1, "{before_nul:?}");
            for src in [at_end, before_nul] {
                let text = parsed_text(&src);
                assert_eq!(reads_past_in(&text), 0, "{text:?}");
                assert!(text.ends_with('\n') && !text.contains('\0'), "{text:?}");
            }
            for control in [
                format!("#{directive}\n"),
                format!("#{directive} X\n"),
                format!("#{directive}x"),
            ] {
                assert_eq!(reads_past_in(&control), 0, "{control:?}");
            }
        }
        for control in ["#", "#e", "#els", "#selector(f)", "#\0", "let s = #\"x\"#"] {
            assert_eq!(reads_past_in(control), 0, "{control:?}");
        }
    }

    #[test]
    fn a_source_gains_a_line_break_only_when_it_has_none() {
        assert_eq!(swift_tree::line_ended("#endif").as_str(), "#endif\n");
        assert_eq!(swift_tree::line_ended("").as_str(), "\n");
        assert_eq!(swift_tree::line_ended("#endif\r").as_str(), "#endif\r\n");
        for kept in ["#endif\n", "\n", "a\r\n"] {
            assert_eq!(swift_tree::line_ended(kept).as_str(), kept);
        }
    }

    /// A file that ends in a directive, with and without the line break, reads the same:
    /// tests, their lines and assertions, functions, and no parse error.
    #[test]
    fn a_file_ending_in_a_directive_reads_as_the_same_file_with_a_line_break() {
        let body = "import XCTest\n#if DEBUG\nfinal class ATests: XCTestCase {\n    func testA() {\n        XCTAssertEqual(f(), 1)\n    }\n}\n#endif";
        let bare = facts("Tests/ATests.swift", body);
        let ended = facts("Tests/ATests.swift", &format!("{body}\n"));
        assert_eq!(format!("{bare:?}"), format!("{ended:?}"));
        assert!(!bare.has_parse_errors);
        assert_eq!(bare.tests.len(), 1);
        assert_eq!(
            (bare.tests[0].line, bare.tests[0].total_asserts),
            (4, 1),
            "{:?}",
            bare.tests[0]
        );
        // A broken file whose missing token the grammar places at the added byte still
        // reads, and is named as parsed with errors.
        for broken in ["#if", "let a = \"open", "class A {\n  func f() {", "#"] {
            assert!(
                facts("Sources/A.swift", broken).has_parse_errors,
                "{broken:?}"
            );
        }
    }

    /// The fuzz seeds for this pack (`fuzz/corpus/language_packs/swift_*`): each one
    /// selects this pack in the fuzz target, makes the unguarded scan read past a
    /// directive string, and is read by the pack without that.
    #[test]
    fn the_fuzz_seeds_reach_the_directive_scan_and_are_read_without_it() {
        let root = env!("CARGO_MANIFEST_DIR");
        let target =
            std::fs::read_to_string(format!("{root}/fuzz/fuzz_targets/language_packs.rs")).unwrap();
        let paths: Vec<&str> = target
            .split_once("const PATHS: &[&str] = &[")
            .and_then(|(_, rest)| rest.split_once("];"))
            .map(|(list, _)| list.split('"').skip(1).step_by(2).collect())
            .expect("the fuzz target's path table");
        let mut seeds = 0;
        for entry in std::fs::read_dir(format!("{root}/fuzz/corpus/language_packs")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("swift_") {
                continue;
            }
            seeds += 1;
            let bytes = std::fs::read(&path).unwrap();
            let (selector, body) = bytes.split_first().unwrap();
            assert_eq!(
                paths[(selector & 0x1f) as usize % paths.len()],
                "m.swift",
                "{name}"
            );
            let src = String::from_utf8_lossy(body);
            assert!(reads_past_in(&src) > 0, "{name} does not reproduce");
            assert_eq!(reads_past_in(&parsed_text(&src)), 0, "{name}");
            let file = if selector & 0x20 != 0 {
                "tests/m.swift"
            } else {
                "src/m.swift"
            };
            let read = facts(file, &src);
            if name == "swift_endif_at_end_of_file" || name == "swift_nul_after_endif" {
                assert_eq!(read.tests.len(), 1, "{name}");
                assert_eq!(read.tests[0].total_asserts, 1, "{name}");
            }
        }
        assert_eq!(seeds, 6);
    }

    /// `swift_tree::parse` takes a `LineEndedSource`, whose field is private to that
    /// module, so the compiler refuses any other text. What it cannot refuse is a second
    /// parser built somewhere else: the grammar is named once in `src/`, inside
    /// `swift_tree`, and this file builds no other parser.
    #[test]
    fn the_swift_grammar_is_named_only_inside_the_guarded_parser() {
        let grammar = ["tree_sitter", "_swift"].concat();
        let parser = ["Parser", "::new"].concat();
        let mut named = Vec::new();
        let mut dirs = vec![std::path::PathBuf::from(format!(
            "{}/src",
            env!("CARGO_MANIFEST_DIR")
        ))];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    named.extend(
                        text.match_indices(&grammar)
                            .map(|(at, _)| (path.clone(), text[..at].lines().count())),
                    );
                }
            }
        }
        assert_eq!(named.len(), 1, "{named:?}");
        assert!(named[0].0.ends_with("src/ast/swift.rs"), "{named:?}");
        let here = std::fs::read_to_string(&named[0].0).unwrap();
        let module = here
            .split_once("\nmod swift_tree {\n")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map(|(body, _)| body)
            .expect("the module");
        assert!(module.contains(&grammar));
        assert!(module.contains("fn parse(text: &LineEndedSource)"));
        assert!(module.contains("pub(super) struct LineEndedSource<'a>(Cow<'a, str>);"));
        assert!(module.contains("crate::ast::source_text::parse(&mut parser, &text.0)"));
        assert_eq!(here.matches(&parser).count(), 1);
        assert_eq!(module.matches(&parser).count(), 1);
    }
}
