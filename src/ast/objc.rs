//! Objective-C language pack (`.m`, `.mm`): tree-sitter AST extraction of tests,
//! assertions, and escape hatches.
//!
//! XCTest (`- (void)test*` with no parameters in an `XCTestCase` subclass, or in a type
//! in a test path), `XCTAssert*` / `XCTFail` / `XCTSkip*` macros, `#pragma clang
//! diagnostic ignored` and `// NOLINT` as escape hatches, an empty `@catch { }`, an
//! `error:nil` argument and `(void)call()` as swallowed errors, and
//! `doesNotRecognizeSelector:` / `@throw` / `NSAssert(NO, ...)` bodies as stubs.
//!
//! Grammar note: the grammar reads Objective-C, not Objective-C++. A `.mm` file whose
//! C++ constructs it cannot read reports its parse errors like any other file.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Objective-C language pack implementing [`LanguagePack`].
pub struct ObjcPack;

/// `src` with the Objective-C macros the grammar cannot read rewritten byte for byte:
/// enum heads, Apple's annotation macros plus `[languages.c]`, `extern "C"` guards.
fn mask_objc_macros(src: &str, vocab: &AssertVocabulary) -> Option<String> {
    use super::c_macros::{lists, mask, mask_cplusplus_guards, mask_enum_heads, APPLE_MACROS};
    let enums = mask_enum_heads(src);
    let step = enums.as_deref().unwrap_or(src);
    let macros = mask(
        step,
        &lists(&vocab.c_macros, APPLE_MACROS),
        &lists(&vocab.c_function_macros, &[]),
    );
    let step2 = macros.as_deref().unwrap_or(step);
    let guards = mask_cplusplus_guards(step2);
    guards.or(macros).or(enums)
}

impl LanguagePack for ObjcPack {
    fn id(&self) -> &'static str {
        "objc"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Objective-C"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("m" | "mm"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_objc::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Objective-C grammar: {e}"))?;
        // The C preprocessor habits of Objective-C, rewritten byte for byte before the parse
        // (`super::c_macros`): `typedef NS_ENUM(T, Name)`, Apple's annotation macros, the
        // repository's own `[languages.c]` macros, `extern "C"` guards.
        let masked = mask_objc_macros(src, vocab);
        let src = masked.as_deref().unwrap_or(src);
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();
        let (has_errors, first_line, error_count) = super::collect_error_nodes_info(root);

        let mut extractor = ObjcExtractor {
            dead: super::reach::dead_ranges(root, src, &OBJC_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_objc_test_path(path),
            xctest_classes: Vec::new(),
            facts: ParsedFileFacts {
                has_parse_errors: has_errors,
                first_parse_error_line: first_line,
                skipped_error_nodes_count: error_count,
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_escape_hatches(root);
        extractor.collect_xctest_classes(root);
        extractor.visit_node(root, "");
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &OBJC_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &OBJC_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            let whole_file = is_objc_test_path(path)
                || functions::test_path(path)
                || functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &OBJC_HANDLERS, &is_test_line);
        }
        if functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &OBJC_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        extractor.facts.prose = super::prose::extract(root, src, &["comment", "string_literal"]);
        Ok(extractor.facts)
    }
}

/// Whether a path is conventionally Objective-C test code (`Tests/`, a `*Tests` target, a
/// `*Tests.m` / `*Test.m` file).
pub fn is_objc_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename
        .strip_suffix(".mm")
        .or_else(|| filename.strip_suffix(".m"))
        .unwrap_or(filename);
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
    "XCTAssertEqualObjects",
    "XCTAssertIdentical",
];

struct ObjcExtractor<'a> {
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    /// Classes declared `@interface X : XCTestCase` in this file.
    xctest_classes: Vec<String>,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

impl<'a> ObjcExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn first_identifier(&self, node: Node) -> &'a str {
        let mut cursor = node.walk();
        let found = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "identifier")
            .map(|c| self.text(c))
            .unwrap_or("");
        found
    }

    /// `#pragma clang diagnostic ignored "-W..."` and `// NOLINT...`.
    fn collect_escape_hatches(&mut self, node: Node) {
        match node.kind() {
            "preproc_call" => {
                let text = self.text(node);
                if text.contains("diagnostic ignored") {
                    let rule = text.split('"').nth(1).unwrap_or("all").to_string();
                    self.facts
                        .escape_hatches
                        .push(EscapeHatchSite::LinterDisable {
                            line: node.start_position().row + 1,
                            rule,
                            snippet: text.trim().to_string(),
                        });
                }
                return;
            }
            "comment" => {
                let text = self.text(node);
                let body = text
                    .trim_start_matches("//")
                    .trim_start_matches("/*")
                    .trim_end_matches("*/")
                    .trim();
                if body.starts_with("NOLINT") {
                    self.facts
                        .escape_hatches
                        .push(EscapeHatchSite::LinterDisable {
                            line: node.start_position().row + 1,
                            rule: body.to_string(),
                            snippet: text.to_string(),
                        });
                }
                return;
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_escape_hatches(child);
        }
    }

    fn collect_xctest_classes(&mut self, node: Node) {
        if node.kind() == "class_interface" {
            let superclass = node
                .child_by_field_name("superclass")
                .map(|s| self.text(s))
                .unwrap_or("");
            if superclass == "XCTestCase" {
                self.xctest_classes
                    .push(self.first_identifier(node).to_string());
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_xctest_classes(child);
        }
    }

    fn visit_node(&mut self, node: Node, class: &str) {
        match node.kind() {
            "class_implementation" => {
                let name = self.first_identifier(node).to_string();
                let mut cursor = node.walk();
                let children: Vec<Node> = node.children(&mut cursor).collect();
                for c in children {
                    self.visit_node(c, &name);
                }
            }
            "method_definition" => self.visit_method(node, class),
            "function_definition" => self.record_function_helper(node),
            _ => {
                let mut cursor = node.walk();
                let children: Vec<Node> = node.children(&mut cursor).collect();
                for c in children {
                    self.visit_node(c, class);
                }
            }
        }
    }

    fn body(node: Node) -> Option<Node> {
        let mut cursor = node.walk();
        let found = node
            .children(&mut cursor)
            .find(|c| c.kind() == "compound_statement");
        found
    }

    fn visit_method(&mut self, node: Node, class: &str) {
        let selector = self.first_identifier(node);
        let mut cursor = node.walk();
        let has_params = node
            .children(&mut cursor)
            .any(|c| c.kind() == "method_parameter");
        let returns_void = node
            .named_child(0)
            .is_some_and(|t| t.kind() == "method_type" && self.text(t).contains("void"));
        let in_xctest = self.xctest_classes.iter().any(|c| c == class)
            || class.ends_with("Tests")
            || class.ends_with("Test")
            || (self.is_test_path && !class.is_empty());
        let Some(body) = Self::body(node) else {
            return;
        };
        if selector.starts_with("test") && !has_params && returns_void && in_xctest {
            let mut test_fn = TestFn {
                name: format!("{class}.{selector}"),
                line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                ..Default::default()
            };
            let mut calls = Vec::new();
            self.scan_node(body, &mut test_fn, &mut calls);
            self.facts.tests.push(test_fn);
            self.test_calls.push(calls);
        } else {
            self.record_helper(selector.to_string(), body);
        }
    }

    fn record_function_helper(&mut self, node: Node) {
        let name = node
            .child_by_field_name("declarator")
            .and_then(|d| d.child_by_field_name("declarator"))
            .map(|n| self.text(n).to_string())
            .unwrap_or_default();
        if let (false, Some(body)) = (name.is_empty(), node.child_by_field_name("body")) {
            self.record_helper(name, body);
        }
    }

    fn record_helper(&mut self, name: String, body: Node) {
        let mut helper = TestFn::default();
        let mut dummy = Vec::new();
        self.scan_node(body, &mut helper, &mut dummy);
        helper.total_asserts += super::count_failure_exits(
            body,
            self.src,
            &["throw_statement", "call_expression"],
            &["@throw", "abort("],
            &["block_literal"],
        );
        self.helpers.entry(name).or_insert(super::HelperFacts {
            total_asserts: helper.total_asserts,
            strong_asserts: helper.strong_asserts,
            tautologies: helper.tautologies,
            fatal_asserts: helper.fatal_asserts,
        });
    }

    fn args<'b>(&self, call: Node<'b>) -> Vec<Node<'b>> {
        let Some(a) = call.child_by_field_name("arguments") else {
            return Vec::new();
        };
        let mut cursor = a.walk();
        a.named_children(&mut cursor).collect()
    }

    fn scan_node(&self, node: Node, test_fn: &mut TestFn, calls: &mut Vec<String>) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "call_expression" => self.inspect_call(node, test_fn, calls),
            "message_expression" => {
                let on_self = node
                    .child_by_field_name("receiver")
                    .is_some_and(|r| self.text(r) == "self");
                if on_self {
                    if let Some(m) = node.child_by_field_name("method") {
                        calls.push(self.text(m).to_string());
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.scan_node(child, test_fn, calls);
        }
    }

    fn inspect_call(&self, node: Node, test_fn: &mut TestFn, calls: &mut Vec<String>) {
        let Some(f) = node.child_by_field_name("function") else {
            return;
        };
        if f.kind() != "identifier" {
            return;
        }
        let name = self.text(f);
        calls.push(name.to_string());
        if name.starts_with("XCTSkip") {
            test_fn.ignored = true;
            return;
        }
        let args = self.args(node);
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
                    ["NO", "false"]
                } else {
                    ["YES", "true"]
                };
                if literal.contains(&arg(0)) {
                    test_fn.tautologies += 1;
                }
            }
            "XCTAssertNil" | "XCTAssertNotNil" => test_fn.total_asserts += 1,
            "XCTAssertNotEqual"
            | "XCTAssertNotEqualObjects"
            | "XCTAssertGreaterThan"
            | "XCTAssertGreaterThanOrEqual"
            | "XCTAssertLessThan"
            | "XCTAssertLessThanOrEqual"
            | "XCTAssertEqualWithAccuracy"
            | "XCTAssertThrows"
            | "XCTAssertThrowsSpecific"
            | "XCTAssertThrowsSpecificNamed"
            | "XCTAssertNoThrow"
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

fn objc_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let first = {
        let mut cursor = node.walk();
        let found = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "identifier")
            .and_then(|c| c.utf8_text(src.as_bytes()).ok())
            .unwrap_or("");
        found
    };
    (node.kind() == "method_definition" && first.starts_with("test") && is_objc_test_path(path))
        || is_objc_test_path(path)
        || functions::test_path(path)
}

pub const OBJC_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["method_definition", "function_definition"],
    // A method's selector is its first `identifier` child; a C function's name is in
    // its declarator.
    name_fields: &["declarator", "identifier"],
    body_fields: &["body", "compound_statement"],
    ignored_kinds: &["comment"],
    skip: functions::skip_none,
    is_test: objc_fn_is_test,
    classify: functions::classify_objc,
};

pub const OBJC_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
};

pub const OBJC_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_clause"],
    arm_of: &[],
    body_fields: &["compound_statement"],
    ignored_kinds: &["comment"],
    trivial: &[
        "return",
        "return nil",
        "return NO",
        "return 0",
        "return NULL",
    ],
    // `(void)call()` as in C, and `error:nil` on a message that reports failure through
    // an `NSError **`.
    discard_kinds: &["cast_expression", "message_expression"],
    discards: super::handlers::objc_discards,
    classify_discard: Some(super::handlers::objc_discard_class),
    call_value_kinds: &["call_expression", "message_expression"],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
};

pub const OBJC_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["compound_statement"],
    ignored_kinds: &["comment"],
    terminators: &["return", "@throw", "abort("],
};

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(path: &str, src: &str) -> ParsedFileFacts {
        ObjcPack
            .extract(path, src, &AssertVocabulary::default())
            .expect("extract succeeds")
    }

    #[test]
    fn xctest_methods_assertions_tautologies_skips_and_helpers() {
        let src = "@interface CartTests : XCTestCase\n@end\n@implementation CartTests\n- (void)testTotal {\n    XCTAssertEqual(cart.total, 3);\n    XCTAssertTrue(YES);\n    XCTAssertNotNil(cart);\n}\n- (void)testEmpty {\n}\n- (void)testSkip {\n    XCTSkipIf(YES);\n    XCTAssertEqual(1, 1);\n}\n- (void)checkRow:(Row *)r {\n    if (r.id != 1) { XCTFail(@\"id\"); }\n}\n- (void)testViaHelper {\n    [self checkRow:load()];\n}\n@end\n";
        let f = facts("AppTests/CartTests.m", src);
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
        assert!(by("CartTests.testEmpty").is_vacuous());
        assert!(by("CartTests.testSkip").ignored);
        let via = by("CartTests.testViaHelper");
        assert_eq!((via.total_asserts, via.helper_checks), (1, 1), "{via:?}");
        assert_eq!(f.tests.len(), 4, "a method with parameters is not a test");
    }

    #[test]
    fn swallowed_errors_stubs_and_suppressions() {
        let src = "#pragma clang diagnostic ignored \"-Wdeprecated-declarations\"\n@implementation Store\n- (void)save {\n    @try { [self write]; } @catch (NSException *e) { }\n    [data writeToFile:path options:0 error:nil];\n    [data writeToFile:path options:0 error:&error];\n    (void)fclose(fp);\n}\n- (void)load {\n    [self doesNotRecognizeSelector:_cmd];\n}\n@end\n";
        let f = facts("App/Store.m", src);
        let kinds: Vec<(usize, &str)> = f.swallowed.iter().map(|s| (s.line, s.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (4, "empty-handler"),
                (5, "discarded-result"),
                (7, "discarded-result")
            ]
        );
        assert!(matches!(
            &f.escape_hatches[0],
            EscapeHatchSite::LinterDisable { rule, .. } if rule == "-Wdeprecated-declarations"
        ));
        let load = f.functions.iter().find(|x| x.name == "load").unwrap();
        assert!(
            matches!(load.shape, functions::BodyShape::Stub(_)),
            "{load:?}"
        );
    }

    #[test]
    fn test_paths_follow_xcode_conventions() {
        assert!(is_objc_test_path("AppTests/CartTests.m"));
        assert!(is_objc_test_path("Tests/Cart.mm"));
        assert!(!is_objc_test_path("App/Cart.m"));
    }

    #[test]
    fn apple_macros_and_enum_heads_parse_without_error_regions() {
        let v = AssertVocabulary::default();
        let src = "#import <Foundation/Foundation.h>\nNS_ASSUME_NONNULL_BEGIN\ntypedef NS_ENUM(NSInteger, SDCacheType) {\n    SDCacheTypeNone,\n    SDCacheTypeDisk,\n};\nstatic CGImageRef SDCopy(CGImageRef image) CF_RETURNS_RETAINED {\n    return image;\n}\n@implementation SDCache\n- (void)clear API_DEPRECATED(\"use clearAll\", ios(8.0, API_TO_BE_DEPRECATED)) {\n}\n@end\nNS_ASSUME_NONNULL_END\n";
        let facts = ObjcPack
            .extract("SDWebImage/Core/SDCache.m", src, &v)
            .unwrap();
        assert_eq!(facts.skipped_error_nodes_count, 0);
        // A project macro no list names is still an error region until configured.
        let own = "static BOOL SDIs8Bit(CGImageRef cg_nullable image) {\n    return YES;\n}\n";
        assert!(
            ObjcPack
                .extract("SDWebImage/Core/SDImage.m", own, &v)
                .unwrap()
                .skipped_error_nodes_count
                > 0
        );
        let configured = AssertVocabulary {
            c_macros: vec!["cg_nullable".to_string()],
            ..Default::default()
        };
        assert_eq!(
            ObjcPack
                .extract("SDWebImage/Core/SDImage.m", own, &configured)
                .unwrap()
                .skipped_error_nodes_count,
            0
        );
    }
}
