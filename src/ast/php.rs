//! PHP language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// PHP language pack implementing [`LanguagePack`].
pub struct PhpPack;

impl LanguagePack for PhpPack {
    fn id(&self) -> &'static str {
        "php"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "PHP"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("php" | "phtml" | "inc"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
            .map_err(|e| anyhow!("failed to load the PHP grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = PhpExtractor {
            dead: super::reach::dead_ranges(root, src, &PHP_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_php_test_path(path),
            declared_test_path: functions::declared_test_path(path, &vocab.test_paths),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            test_calls: Vec::new(),
            helpers: std::collections::HashMap::new(),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.facts.functions = functions::extract(root, src, path, &PHP_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &PHP_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            let whole_file = is_php_test_path(path)
                || functions::test_path(path)
                || functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &PHP_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &PHP_RETRIES);
        if functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &PHP_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &PHP_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &["comment", "string", "encapsed_string", "heredoc", "nowdoc"],
        );
        Ok(extractor.facts)
    }
}

fn php_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    let mut cursor = node.walk();
    let attributed = node.children(&mut cursor).any(|c| {
        c.kind() == "attribute_list" && c.utf8_text(src.as_bytes()).unwrap_or("").contains("Test")
    });
    name.starts_with("test") || attributed || is_php_test_path(path) || functions::test_path(path)
}

pub const PHP_FUNCTIONS: FunctionSpec = FunctionSpec {
    // An abstract or interface method has no `body` and is never described.
    function_kinds: &["function_definition", "method_declaration"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    skip: functions::skip_none,
    is_test: php_fn_is_test,
    classify: functions::classify_php,
};

pub const PHP_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &[
        "function_call_expression",
        "member_call_expression",
        "scoped_call_expression",
        "object_creation_expression",
    ],
    callee_fields: &["function", "name"],
};

pub const PHP_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_clause"],
    arm_of: &[],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    trivial: &["return", "return null", "return false", "continue"],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    // `@call()`: the error-control operator drops every diagnostic the call raises.
    silence_kinds: &["error_suppression_expression"],
    silences: super::handlers::php_silences,
};

pub const PHP_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["attribute_list", "comment"],
};

/// Determines whether a path is conventionally a PHP test file.
pub fn is_php_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename.ends_with("Test.php")
        || filename.ends_with("_test.php")
        || filename.starts_with("test_")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
}

struct PhpExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    /// Under a `[tests] paths` glob: a `test*` top-level function there is a test.
    declared_test_path: bool,
    facts: ParsedFileFacts,
    /// Same-file callees of each test (`Class::method` or `function`), in test order.
    test_calls: Vec<Vec<String>>,
    /// Non-test methods and functions a test may call.
    helpers: std::collections::HashMap<String, super::HelperFacts>,
}

/// Closure nodes whose body runs only when called.
const PHP_CLOSURE_KINDS: &[&str] = &[
    "anonymous_function",
    "anonymous_function_creation_expression",
    "arrow_function",
    "function_definition",
    "method_declaration",
    "class_declaration",
];

impl<'a> PhpExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        let kind = node.kind();
        if kind == "comment" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            let trimmed = text
                .trim_start_matches("//")
                .trim_start_matches('#')
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim();

            if trimmed.starts_with("@psalm-suppress")
                || trimmed.starts_with("@phpstan-ignore")
                || trimmed.starts_with("phpstan-ignore")
                || trimmed.starts_with("phpcs:ignore")
                || trimmed.starts_with("psalm-suppress")
            {
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: trimmed.to_string(),
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
        self.walk_top_level(root);
        self.resolve_same_file_helpers();
    }

    /// Records a non-test method or function as a helper: its assertions and its
    /// `throw`s are the failure paths a test inherits when it calls it.
    fn record_helper(&mut self, key: String, node: Node) {
        if self.helpers.contains_key(&key) {
            return;
        }
        let mut h = TestFn::default();
        if let Some(body) = node.child_by_field_name("body") {
            self.scan_block(body, &mut h);
            h.total_asserts += super::count_failure_exits(
                body,
                self.src,
                &["throw_expression", "throw_statement"],
                &[],
                PHP_CLOSURE_KINDS,
            );
        }
        self.helpers.insert(
            key,
            super::HelperFacts {
                total_asserts: h.total_asserts,
                strong_asserts: h.strong_asserts,
                tautologies: h.tautologies,
                fatal_asserts: h.fatal_asserts,
            },
        );
    }

    /// The same-file callees a test body runs: `$this->m()`, `self::m()`,
    /// `static::m()` resolve to a method of `class_name`, `f()` to a function. A
    /// closure assigned and not called runs nothing.
    fn collect_calls(&self, node: Node, class_name: &str, calls: &mut Vec<String>) {
        if PHP_CLOSURE_KINDS.contains(&node.kind())
            && node
                .parent()
                .is_some_and(|p| p.kind() == "assignment_expression")
        {
            return;
        }
        match node.kind() {
            "member_call_expression" => {
                let on_this = node
                    .child_by_field_name("object")
                    .is_some_and(|o| self.text(o) == "$this");
                if let (true, Some(n)) = (on_this, node.child_by_field_name("name")) {
                    calls.push(format!("{class_name}::{}", self.text(n)));
                }
            }
            "scoped_call_expression" => {
                let scope = node
                    .child_by_field_name("scope")
                    .map(|s| self.text(s))
                    .unwrap_or("");
                if matches!(scope, "self" | "static") || scope == class_name {
                    if let Some(n) = node.child_by_field_name("name") {
                        calls.push(format!("{class_name}::{}", self.text(n)));
                    }
                }
            }
            "function_call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if f.kind() == "name" {
                        calls.push(self.text(f).to_string());
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_calls(child, class_name, calls);
        }
    }

    /// Adds each called helper's failure paths to the test. One level: a helper's
    /// own callees are not followed.
    fn resolve_same_file_helpers(&mut self) {
        for (test, calls) in self.facts.tests.iter_mut().zip(&self.test_calls) {
            for call in calls {
                let Some(h) = self.helpers.get(call) else {
                    continue;
                };
                let leaf = call.rsplit("::").next().unwrap_or(call);
                // A configured assertion helper was already counted at the call.
                if self.vocab.helper_fns.iter().any(|n| n == leaf) {
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

    fn walk_top_level(&mut self, node: Node) {
        let kind = node.kind();
        if kind == "class_declaration" {
            let name_node = node.child_by_field_name("name");
            let c_name = name_node.map(|n| self.text(n)).unwrap_or("");
            let (_, class_ignored) = self.check_doc_or_attrs_for_test(node);
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for child in body.children(&mut cursor) {
                    if child.kind() == "method_declaration" {
                        self.visit_method(child, c_name, class_ignored);
                    }
                }
            }
            return;
        }

        // tree-sitter-php names a top-level function `function_definition`; the older
        // `function_declaration` kind is kept so a grammar bump cannot silently drop them.
        if kind == "function_definition" || kind == "function_declaration" {
            self.visit_function(node);
            return;
        }

        // Pest PHP top-level test('...', function() { ... }) or it('...', function() { ... })
        if kind == "expression_statement" {
            if let Some(child) = node.child(0) {
                if child.kind() == "function_call_expression" {
                    self.try_pest_test(child);
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk_top_level(child);
        }
    }

    fn check_doc_or_attrs_for_test(&self, node: Node) -> (bool, bool) {
        let mut is_test = false;
        let mut is_ignored = false;

        // Check attributes (PHP 8 #[Test], #[Requires*])
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "attribute_list" {
                let attr_text = self.text(child);
                if attr_text.contains("Test") {
                    is_test = true;
                }
                if attr_text.contains("Requires") || attr_text.contains("Skip") {
                    is_ignored = true;
                }
            }
        }

        // Check preceding comments / docblocks
        if let Some(prev) = node.prev_sibling() {
            if prev.kind() == "comment" {
                let comment = self.text(prev);
                if comment.contains("@test") {
                    is_test = true;
                }
                if comment.contains("@requires")
                    || comment.contains("@skip")
                    || comment.contains("@group skip")
                    || comment.contains("@group-skip")
                {
                    is_ignored = true;
                }
            }
        }

        (is_test, is_ignored)
    }

    fn visit_method(&mut self, node: Node, class_name: &str, class_ignored: bool) {
        let name_node = node.child_by_field_name("name");
        let method_name = name_node.map(|n| self.text(n)).unwrap_or("");

        let (annotated_as_test, is_ignored) = self.check_doc_or_attrs_for_test(node);
        let name_is_test = method_name.starts_with("test");

        if !name_is_test && !annotated_as_test {
            self.record_helper(format!("{class_name}::{method_name}"), node);
            return;
        }

        let full_name = if class_name.is_empty() {
            method_name.to_string()
        } else {
            format!("{class_name}::{method_name}")
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
            ignored: class_ignored || is_ignored,
            should_panic: false,
            ..Default::default()
        };

        let mut calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.scan_block(body, &mut test_fn);
            self.collect_calls(body, class_name, &mut calls);
        }

        self.facts.tests.push(test_fn);
        self.test_calls.push(calls);
    }

    fn visit_function(&mut self, node: Node) {
        let name_node = node.child_by_field_name("name");
        let func_name = name_node.map(|n| self.text(n)).unwrap_or("");

        let (annotated_as_test, is_ignored) = self.check_doc_or_attrs_for_test(node);
        // No PHP runner collects a top-level function by its name, so `testsCovering()`
        // in `examples/` is not a test; the name counts only under a declared test path.
        let name_is_test = func_name.starts_with("test") && self.declared_test_path;

        // Any top-level function can be called from a test; in a test path it is also
        // collected as a test itself, as before.
        self.record_helper(func_name.to_string(), node);
        if !name_is_test && !annotated_as_test && !self.is_test_path {
            return;
        }

        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let mut test_fn = TestFn {
            name: func_name.to_string(),
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: is_ignored,
            should_panic: false,
            ..Default::default()
        };

        let mut calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.scan_block(body, &mut test_fn);
            self.collect_calls(body, "", &mut calls);
        }

        self.facts.tests.push(test_fn);
        self.test_calls.push(calls);
    }

    fn try_pest_test(&mut self, node: Node) {
        let func_node = node.child_by_field_name("function");
        let func_text = func_node.map(|n| self.text(n)).unwrap_or("");

        if func_text != "test" && func_text != "it" {
            return;
        }

        let Some(args_node) = node.child_by_field_name("arguments") else {
            return;
        };

        let args = self.collect_arguments(args_node);
        if args.is_empty() {
            return;
        }

        let test_label = self
            .text(args[0])
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
        let test_name = format!("{func_text}: {test_label}");
        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let mut test_fn = TestFn {
            name: test_name,
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };

        // If second argument is a closure or arrow function
        if args.len() >= 2 {
            let mut closure_or_fn = args[1];
            if closure_or_fn.kind() == "argument" {
                if let Some(c) = closure_or_fn.child(0) {
                    closure_or_fn = c;
                }
            }
            let mut calls = Vec::new();
            if let Some(body) = closure_or_fn.child_by_field_name("body") {
                self.scan_block(body, &mut test_fn);
                self.collect_calls(body, "", &mut calls);
            } else {
                let mut cursor = closure_or_fn.walk();
                for child in closure_or_fn.children(&mut cursor) {
                    if child.kind() == "compound_statement" {
                        self.scan_block(child, &mut test_fn);
                        self.collect_calls(child, "", &mut calls);
                    }
                }
            }
            self.facts.tests.push(test_fn);
            self.test_calls.push(calls);
            return;
        }

        self.facts.tests.push(test_fn);
        self.test_calls.push(Vec::new());
    }

    fn collect_arguments<'b>(&self, args_node: Node<'b>) -> Vec<Node<'b>> {
        let mut out = Vec::new();
        let mut cursor = args_node.walk();
        for child in args_node.children(&mut cursor) {
            let k = child.kind();
            if k == "argument" {
                if let Some(inner) = child.child(0) {
                    out.push(inner);
                } else {
                    out.push(child);
                }
            } else if k != "(" && k != ")" && k != "," && !k.is_empty() {
                out.push(child);
            }
        }
        out
    }

    fn scan_block(&self, node: Node, test_fn: &mut TestFn) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        let kind = node.kind();

        if kind == "member_call_expression"
            || kind == "scoped_call_expression"
            || kind == "function_call_expression"
        {
            self.scan_call(node, test_fn);
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.scan_block(child, test_fn);
        }
    }

    fn scan_call(&self, node: Node, test_fn: &mut TestFn) {
        let kind = node.kind();
        let name_node = node.child_by_field_name("name");
        let call_name = name_node.map(|n| self.text(n)).unwrap_or("");

        let args = if let Some(args_node) = node.child_by_field_name("arguments") {
            self.collect_arguments(args_node)
        } else {
            Vec::new()
        };

        // Test skip: $this->markTestSkipped(...), $this->markTestIncomplete(...)
        if call_name == "markTestSkipped" || call_name == "markTestIncomplete" {
            test_fn.ignored = true;
            return;
        }

        // Pest skip: ->skip(...)
        if call_name == "skip" && kind == "member_call_expression" {
            test_fn.ignored = true;
            return;
        }

        // PHPUnit assertion methods
        if call_name.starts_with("assert") {
            self.classify_phpunit_assertion(call_name, &args, test_fn);
            return;
        }

        // Exception expectation: $this->expectException(...)
        if call_name == "expectException"
            || call_name == "expectExceptionMessage"
            || call_name == "expectExceptionCode"
        {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            test_fn.should_panic = true;
            return;
        }

        // Pest expect matchers: expect($val)->toBe(...), ->toEqual(...)
        if kind == "member_call_expression" {
            match call_name {
                "toBe" | "toEqual" | "toMatch" | "toContain" | "toHaveCount" => {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                    return;
                }
                "toBeTrue" => {
                    test_fn.total_asserts += 1;
                    // Check if receiver is expect(true)
                    if let Some(obj) = node.child_by_field_name("object") {
                        if self.text(obj).contains("expect(true)") {
                            test_fn.tautologies += 1;
                        }
                    }
                    return;
                }
                "toBeFalse" | "toBeNull" | "toBeEmpty" => {
                    test_fn.total_asserts += 1;
                    return;
                }
                _ => {}
            }
        }

        // Custom helpers configured via vocabulary
        if !call_name.is_empty() && self.vocab.helper_fns.iter().any(|h| h == call_name) {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
        }
    }

    fn classify_phpunit_assertion(&self, name: &str, args: &[Node], test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;

        match name {
            // Strong assertions: Equality, Same, Count, Type, Regex, Exceptions
            "assertEquals"
            | "assertSame"
            | "assertEqualsCanonicalizing"
            | "assertEqualsIgnoringCase"
            | "assertEqualsWithDelta"
            | "assertNotEquals"
            | "assertNotSame"
            | "assertCount"
            | "assertInstanceOf"
            | "assertStringMatchesFormat"
            | "assertMatchesRegularExpression"
            | "assertJsonStringEqualsJsonString"
            | "assertFileEquals" => {
                test_fn.strong_asserts += 1;
                // Check tautologies: assertEquals($x, $x), assertSame(1, 1)
                if args.len() >= 2 {
                    let a0 = self.text(args[0]).trim();
                    let a1 = self.text(args[1]).trim();
                    if a0 == a1 {
                        test_fn.tautologies += 1;
                    }
                }
            }

            // Weak assertions: Truthiness / Null / Empty
            "assertTrue" => {
                if !args.is_empty() {
                    let arg0 = self.text(args[0]).trim();
                    if arg0 == "true" {
                        test_fn.tautologies += 1;
                    }
                }
            }
            "assertFalse" => {
                if !args.is_empty() {
                    let arg0 = self.text(args[0]).trim();
                    if arg0 == "false" {
                        test_fn.tautologies += 1;
                    }
                }
            }
            "assertNull" | "assertNotNull" | "assertEmpty" | "assertNotEmpty" | "assertIsArray"
            | "assertIsString" | "assertIsInt" | "assertIsBool" => {
                // Weak assertions: counted in total_asserts but not strong_asserts
            }

            // Other assert* functions: default to strong if not recognized
            _ => {
                test_fn.strong_asserts += 1;
            }
        }
    }
}

pub const PHP_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["compound_statement"],
    ignored_kinds: &["comment"],
    terminators: &["return", "throw", "exit(", "die("],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phpunit_test_extraction_and_assertions() {
        let src = r#"<?php
class CalcTest extends PHPUnit\Framework\TestCase {
    public function testAdd() {
        $this->assertEquals(4, 2 + 2);
        $this->assertSame(4, 2 + 2);
    }
    public function testCheck() {
        $this->assertTrue($x > 0);
        $this->assertFalse($y < 0);
    }
}
"#;
        let pack = PhpPack;
        let vocab = AssertVocabulary::default();
        let facts = pack.extract("tests/CalcTest.php", src, &vocab).unwrap();

        assert_eq!(facts.tests.len(), 2);
        assert_eq!(facts.tests[0].name, "CalcTest::testAdd");
        assert_eq!(facts.tests[0].total_asserts, 2);
        assert_eq!(facts.tests[0].strong_asserts, 2);
        assert!(!facts.tests[0].is_vacuous());

        assert_eq!(facts.tests[1].name, "CalcTest::testCheck");
        assert_eq!(facts.tests[1].total_asserts, 2);
        assert_eq!(facts.tests[1].strong_asserts, 0);
        assert!(!facts.tests[1].is_vacuous());
    }

    #[test]
    fn test_example_php_test_parsing() {
        let src = r#"<?php
class ExampleTest extends PHPUnit\Framework\TestCase
{
    public function testSet()
    {
        $set = new Set();
        $this->assertTrue($set->add(42));
        $this->assertFalse($set->add(42));
        $this->assertTrue($set->contains(42));
        $this->assertEquals(1, count($set));
    }
}
"#;
        let pack = PhpPack;
        let vocab = AssertVocabulary::default();
        let facts = pack
            .extract("bindings/php/tests/ExampleTest.php", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "ExampleTest::testSet");
        assert_eq!(facts.tests[0].total_asserts, 4);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn top_level_test_named_function_outside_a_test_path_is_not_a_test() {
        let src = "<?php\nfunction testsCovering(array $c, int $line): array\n{\n    return $c[$line] ?? [];\n}\n/** @test */\nfunction checks_cover(): void { assert(testsCovering([], 1) === []); }\n";
        let pack = PhpPack;
        let vocab = AssertVocabulary::default();
        let names = |path: &str, vocab: &AssertVocabulary| -> Vec<String> {
            let facts = pack.extract(path, src, vocab).unwrap();
            facts.tests.iter().map(|t| t.name.clone()).collect()
        };
        assert_eq!(
            names("examples/coverage-index.php", &vocab),
            vec!["checks_cover"]
        );
        assert_eq!(
            names("tests/coverage.php", &vocab),
            vec!["testsCovering", "checks_cover"]
        );
        let declared = AssertVocabulary {
            test_paths: vec!["qa/**".to_string()],
            ..Default::default()
        };
        assert_eq!(
            names("qa/coverage.php", &declared),
            vec!["testsCovering", "checks_cover"]
        );
    }

    #[test]
    fn test_php_tautologies_and_vacuous_tests() {
        let src = r#"<?php
class EmptyTest extends TestCase {
    public function testEmpty() {
        // No assertions
        $x = 1 + 1;
    }
    public function testTautology() {
        $this->assertTrue(true);
    }
    public function testEqualTautology() {
        $this->assertEquals($var, $var);
    }
}
"#;
        let pack = PhpPack;
        let vocab = AssertVocabulary::default();
        let facts = pack.extract("tests/EmptyTest.php", src, &vocab).unwrap();

        assert_eq!(facts.tests.len(), 3);
        assert!(facts.tests[0].is_vacuous());
        assert_eq!(facts.tests[0].total_asserts, 0);

        assert!(facts.tests[1].is_vacuous());
        assert_eq!(facts.tests[1].total_asserts, 1);
        assert_eq!(facts.tests[1].tautologies, 1);

        assert!(facts.tests[2].is_vacuous());
        assert_eq!(facts.tests[2].total_asserts, 1);
        assert_eq!(facts.tests[2].tautologies, 1);
    }

    #[test]
    fn test_php_skipped_tests_and_annotations() {
        let src = r#"<?php
class SkipTest extends TestCase {
    public function testSkippedInCode() {
        $this->markTestSkipped('skip message');
        $this->assertEquals(1, 1);
    }

    /**
     * @test
     * @requires PHP >= 8.2
     */
    public function customNamedTest() {
        $this->assertEquals(1, 2);
    }
}
"#;
        let pack = PhpPack;
        let vocab = AssertVocabulary::default();
        let facts = pack.extract("tests/SkipTest.php", src, &vocab).unwrap();

        assert_eq!(facts.tests.len(), 2);
        assert!(facts.tests[0].ignored);
        assert_eq!(facts.tests[1].name, "SkipTest::customNamedTest");
        assert!(facts.tests[1].ignored);
    }

    #[test]
    fn test_pest_php_tests_and_matchers() {
        let src = r#"<?php
test('basic arithmetic', function () {
    expect(1 + 1)->toBe(2);
});

it('checks condition', function () {
    expect('hello')->toEqual('hello');
});
"#;
        let pack = PhpPack;
        let vocab = AssertVocabulary::default();
        let facts = pack
            .extract("tests/Feature/ExampleTest.php", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 2);
        assert_eq!(facts.tests[0].name, "test: basic arithmetic");
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());

        assert_eq!(facts.tests[1].name, "it: checks condition");
        assert_eq!(facts.tests[1].strong_asserts, 1);
        assert!(!facts.tests[1].is_vacuous());
    }

    #[test]
    fn same_file_helpers_resolve_for_phpunit_and_pest() {
        let src = r#"<?php
class RowTest extends TestCase {
    private function checkRow(array $r): void {
        $this->assertSame(1, $r['id']);
    }
    private static function checkRole(array $r): void {
        if ($r['role'] !== 'writer') { throw new RuntimeException('role'); }
    }
    public function testRow(): void {
        $r = load();
        $this->checkRow($r);
        self::checkRole($r);
    }
    public function testDefinesButNeverRuns(): void {
        $later = function () { $this->checkRow(load()); };
    }
}

function expectValid(array $r): void {
    if (!isset($r['id'])) { throw new InvalidArgumentException('id'); }
}

test('row is valid', function () {
    expectValid(load());
});
"#;
        let facts = PhpPack
            .extract("tests/RowTest.php", src, &AssertVocabulary::default())
            .unwrap();
        let by = |n: &str| {
            facts
                .tests
                .iter()
                .find(|t| t.name == n)
                .unwrap_or_else(|| panic!("{n}: {:?}", facts.tests))
        };
        let row = by("RowTest::testRow");
        assert_eq!(
            (row.total_asserts, row.strong_asserts, row.helper_checks),
            (2, 1, 2),
            "{row:?}"
        );
        assert!(by("RowTest::testDefinesButNeverRuns").is_vacuous());
        let pest = by("test: row is valid");
        assert_eq!((pest.total_asserts, pest.helper_checks), (1, 1), "{pest:?}");
    }

    #[test]
    fn test_php_class_level_skips_propagate_to_methods() {
        let src = r#"<?php
/**
 * @group skip
 */
class ClassWideSkipTest extends TestCase {
    public function testMethodOne() {
        $this->assertEquals(1, 1);
    }
    public function testMethodTwo() {
        $this->assertTrue(true);
    }
}
"#;
        let pack = PhpPack;
        let facts = pack
            .extract(
                "tests/ClassWideSkipTest.php",
                src,
                &AssertVocabulary::default(),
            )
            .unwrap();

        assert_eq!(facts.tests.len(), 2);
        assert!(
            facts.tests[0].ignored,
            "method 1 must inherit class-level skip"
        );
        assert!(
            facts.tests[1].ignored,
            "method 2 must inherit class-level skip"
        );
    }
}
