//! Java language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::ci_condition::{CiVerdict, Lang, SkipCondition};
use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Java language pack implementing [`LanguagePack`].
pub struct JavaPack;

impl LanguagePack for JavaPack {
    fn id(&self) -> &'static str {
        "java"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Java"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("java"))
    }

    fn is_test_path(&self, path: &str) -> bool {
        super::functions::is_test_file(path, Some(is_java_test_path))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Java grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = JavaExtractor {
            dead: super::reach::dead_ranges(root, src, &JAVA_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_java_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
            class_skips: Vec::new(),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &JAVA_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &JAVA_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            // A file matching the shared test-path conventions, this pack's own test-file
            // convention, or one the repository declares as test scope, is test code
            // line for line.
            let whole_file = super::functions::is_test_file(path, Some(is_java_test_path))
                || super::functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            (
                extractor.facts.swallowed,
                extractor.facts.constant_fallbacks,
            ) = super::handlers::extract_with_constants(
                root,
                src,
                &JAVA_HANDLERS,
                Some(&JAVA_CONSTANTS),
                &is_test_line,
            );
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &JAVA_RETRIES);
        if super::functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::method_checks::count(root, src, &mut extractor.facts, &JAVA_RECEIVER_CALLS);
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &JAVA_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &JAVA_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        super::caught_assertions::java(root, src, &mut extractor.facts.tests);
        super::expected_exceptions::java(root, src, &mut extractor.facts.tests);
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "line_comment",
                "block_comment",
                "string_literal",
                "text_block",
            ],
        );
        Ok(extractor.facts)
    }
}

/// Determines whether a path is conventionally a Java test file.
pub fn is_java_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename.strip_suffix(".java").unwrap_or(filename);
    ["Test", "Tests", "TestCase"]
        .iter()
        .any(|word| super::functions::ends_with_word(stem, word))
        || super::functions::starts_with_test_word(stem)
        || super::functions::has_dir(path, "test", true)
        || super::functions::has_dir(path, "tests", true)
}

struct JavaExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
    /// Conditional skips of the enclosing classes (`@DisabledOnOs(..)` on a class),
    /// outermost first: the annotation as reported and what a CI variable decides.
    class_skips: Vec<Vec<(String, CiVerdict)>>,
}

impl<'a> JavaExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        let kind = node.kind();
        if kind == "annotation" || kind == "marker_annotation" {
            let name = node
                .child_by_field_name("name")
                .map(|n| self.text(n))
                .unwrap_or("");
            if name == "SuppressWarnings" {
                let rule = node
                    .child_by_field_name("arguments")
                    .map(|a| {
                        self.text(a)
                            .trim_matches(['(', ')'])
                            .trim()
                            .trim_matches('"')
                            .to_string()
                    })
                    .unwrap_or_else(|| "all".to_string());
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line: node.start_position().row + 1,
                        rule,
                        snippet: self.text(node).to_string(),
                    });
            }
            return;
        }
        // A comment is never a suppression: `@SuppressWarnings` named in a comment or a
        // Javadoc `{@code ...}` suppresses nothing. Only the annotation node above counts.
        if kind == "line_comment" || kind == "block_comment" {
            return;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments_and_escape_hatches(child);
        }
    }

    fn visit_root(&mut self, root: Node) {
        let mut class_stack = Vec::new();
        self.visit_node(root, &mut class_stack, false);
    }

    fn visit_node(&mut self, node: Node, class_stack: &mut Vec<String>, parent_ignored: bool) {
        match node.kind() {
            "class_declaration" | "record_declaration" => {
                let class_name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_else(|| "Anonymous".to_string());

                let (class_has_disabled, _) = self.check_modifiers_for_test_and_ignore(node);
                let (always, conditional) = self.conditional_annotations(node);
                let is_class_ignored = parent_ignored || class_has_disabled || always;

                class_stack.push(class_name);
                self.class_skips.push(conditional);
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for child in body.children(&mut cursor) {
                        self.visit_node(child, class_stack, is_class_ignored);
                    }
                }
                self.class_skips.pop();
                class_stack.pop();
            }
            "method_declaration" => {
                self.visit_method(node, class_stack, parent_ignored);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_node(child, class_stack, parent_ignored);
                }
            }
        }
    }

    fn get_modifiers(node: Node) -> Option<Node> {
        for i in 0..node.child_count() {
            if let Some(c) = node.child(i) {
                if c.kind() == "modifiers" {
                    return Some(c);
                }
            }
        }
        None
    }

    /// JUnit 5 conditional annotations on a class or method
    /// (`@DisabledIfEnvironmentVariable(named = "CI", ..)`, `@DisabledOnOs(..)`): whether
    /// one always skips, and each conditional skip as the annotation and its verdict.
    fn conditional_annotations(&self, node: Node) -> (bool, Vec<(String, CiVerdict)>) {
        let mut always = false;
        let mut conditional = Vec::new();
        let Some(modifiers) = Self::get_modifiers(node) else {
            return (always, conditional);
        };
        let mut cursor = modifiers.walk();
        for annotation in modifiers.children(&mut cursor) {
            if !matches!(annotation.kind(), "annotation" | "marker_annotation") {
                continue;
            }
            let name = annotation
                .child_by_field_name("name")
                .map(|n| self.text(n))
                .unwrap_or("");
            let name = name.rsplit('.').next().unwrap_or(name);
            let (mut named, mut matches) = (None, None);
            if let Some(arguments) = annotation.child_by_field_name("arguments") {
                let mut pairs = arguments.walk();
                for pair in arguments.named_children(&mut pairs) {
                    if pair.kind() != "element_value_pair" {
                        continue;
                    }
                    let key = pair.child_by_field_name("key").map(|k| self.text(k));
                    let value = pair
                        .child_by_field_name("value")
                        .filter(|v| v.kind() == "string_literal")
                        .map(|v| self.text(v).trim_matches('"'));
                    match key {
                        Some("named") => named = value,
                        Some("matches") => matches = value,
                        _ => {}
                    }
                }
            }
            match super::ci_condition::jvm_annotation(name, named, matches) {
                Some(SkipCondition::Always) => always = true,
                Some(SkipCondition::When(verdict)) => {
                    conditional.push((self.text(annotation).trim().to_string(), verdict));
                }
                Some(SkipCondition::Never) | None => {}
            }
        }
        (always, conditional)
    }

    /// JUnit assumptions among the statements of a test body (`assumeTrue(..)`,
    /// `Assumptions.assumeFalse(..)`, `assumingThat(..)`), read by their condition.
    fn record_assumptions(&self, body: Node, test: &mut TestFn) {
        let mut cursor = body.walk();
        for statement in body.named_children(&mut cursor) {
            match super::ci_condition::jvm_assumption(Lang::Java, statement, self.src) {
                Some((_, SkipCondition::Always)) => test.ignored = true,
                Some((text, SkipCondition::When(verdict))) => {
                    test.record_conditional_skip(text, verdict);
                }
                Some((_, SkipCondition::Never)) | None => {}
            }
        }
    }

    fn check_modifiers_for_test_and_ignore(&self, node: Node) -> (bool, bool) {
        // Returns (is_ignored, is_test_annotated)
        let mut is_ignored = false;
        let mut is_test_annotated = false;

        let mut check_child = |child: Node| {
            match child.kind() {
                "marker_annotation" | "annotation" => {
                    let anno_name = child
                        .child_by_field_name("name")
                        .map(|n| self.text(n))
                        .unwrap_or("");
                    let short_name = anno_name.rsplit('.').next().unwrap_or(anno_name);

                    match short_name {
                        "Disabled" | "Ignore" => {
                            is_ignored = true;
                        }
                        "Test" => {
                            is_test_annotated = true;
                            // Check if TestNG `@Test(enabled = false)` or JUnit 4 `@Test(expected = ...)`
                            if child.kind() == "annotation" {
                                let text = self.text(child);
                                if text.contains("enabled = false")
                                    || text.contains("enabled=false")
                                {
                                    is_ignored = true;
                                }
                            }
                        }
                        "ParameterizedTest" | "RepeatedTest" | "TestFactory" | "TestTemplate" => {
                            is_test_annotated = true;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        };

        if let Some(modifiers) = Self::get_modifiers(node) {
            let mut cursor = modifiers.walk();
            for child in modifiers.children(&mut cursor) {
                check_child(child);
            }
        }

        // Also check direct children of node for annotations (in some grammar variants)
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            check_child(child);
        }

        (is_ignored, is_test_annotated)
    }

    fn visit_method(&mut self, node: Node, class_stack: &[String], parent_ignored: bool) {
        let method_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");

        let (method_ignored, is_test_annotated) = self.check_modifiers_for_test_and_ignore(node);

        // A method is a test if:
        // 1. It carries a test annotation (@Test, @ParameterizedTest, etc.), OR
        // 2. We are in a test path/class, and the method name begins with "test" and takes 0 params (JUnit 3 style)
        let is_test = is_test_annotated
            || (self.is_test_path
                && method_name.starts_with("test")
                && Self::has_zero_parameters(node));

        if is_test {
            let full_name = if class_stack.is_empty() {
                method_name.to_string()
            } else {
                format!("{}.{}", class_stack.join("."), method_name)
            };

            let line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            let should_panic = self.parse_expected_exception(node);

            let (cases, non_literal_cases) = Self::get_modifiers(node)
                .map(|m| super::test_cases::extract_java_cases(m, self.src))
                .unwrap_or((None, false));

            let mut test_fn = TestFn {
                name: full_name,
                line,
                end_line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: parent_ignored || method_ignored,
                should_panic: should_panic.clone(),
                expected_exceptions: should_panic.into_iter().collect(),
                cases,
                non_literal_cases,
                ..Default::default()
            };

            let mut direct_calls = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                self.scan_method_body(body, &mut test_fn, &mut direct_calls);
                super::dispatch_calls(body, self.src, &JAVA_DISPATCH, &mut direct_calls);
            }
            let (always, conditional) = self.conditional_annotations(node);
            test_fn.ignored |= always;
            if let Some(body) = node.child_by_field_name("body") {
                self.record_assumptions(body, &mut test_fn);
            }
            if !test_fn.ignored {
                let inherited = self.class_skips.iter().flatten().cloned();
                for (text, verdict) in inherited.chain(conditional) {
                    test_fn.record_conditional_skip(text, verdict);
                }
            }

            self.facts.tests.push(test_fn);
            self.test_calls.push(direct_calls);
        } else if self.is_test_path {
            if let Some(body) = node.child_by_field_name("body") {
                let mut helper_fn = TestFn::default();
                let mut dummy_calls = Vec::new();
                self.scan_method_body(body, &mut helper_fn, &mut dummy_calls);
                helper_fn.total_asserts += super::count_failure_exits(
                    body,
                    self.src,
                    &["throw_statement"],
                    &[],
                    &["lambda_expression", "class_body"],
                );
                let facts = super::HelperFacts {
                    total_asserts: helper_fn.total_asserts,
                    strong_asserts: helper_fn.strong_asserts,
                    tautologies: helper_fn.tautologies,
                    fatal_asserts: helper_fn.fatal_asserts,
                    wraps: super::forwarding_wrapper_callee(
                        body,
                        &JAVA_WRAPPER,
                        &JAVA_LOCALS,
                        &dummy_calls,
                        self.src,
                    ),
                };
                self.helpers.insert(method_name.to_string(), facts);
                let line = node.start_position().row + 1;
                let end_line = node.end_position().row + 1;
                self.facts.push_helper(
                    super::TestHelperFacts {
                        name: method_name.to_string(),
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
    }

    fn has_zero_parameters(method_node: Node) -> bool {
        if let Some(params) = method_node.child_by_field_name("parameters") {
            let mut cursor = params.walk();
            for child in params.children(&mut cursor) {
                if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
                    return false;
                }
            }
        }
        true
    }

    fn parse_expected_exception(
        &self,
        node: Node,
    ) -> Option<super::expected_exceptions::ExpectedException> {
        if let Some(modifiers) = Self::get_modifiers(node) {
            let mut cursor = modifiers.walk();
            for child in modifiers.children(&mut cursor) {
                if child.kind() == "annotation" {
                    if let Some(exp) = super::expected_exceptions::java_annotation_expected(
                        child,
                        std::str::from_utf8(self.src).unwrap_or(""),
                    ) {
                        return Some(exp);
                    }
                }
            }
        }
        None
    }

    fn scan_method_body(&self, body: Node, test_fn: &mut TestFn, direct_calls: &mut Vec<String>) {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            self.scan_statement_or_expr(child, test_fn, direct_calls);
        }
    }

    /// Records where the tautologies counted under `node` are (`TestFn::mark_tautologies`).
    fn scan_statement_or_expr(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        direct_calls: &mut Vec<String>,
    ) {
        let mark = test_fn.tautology_mark();
        self.scan_statement_or_expr_unmarked(node, test_fn, direct_calls);
        test_fn.mark_tautologies(mark, node);
    }

    fn scan_statement_or_expr_unmarked(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        direct_calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "assert_statement" => {
                test_fn.total_asserts += 1;
                // Java assert expression; or assert expr : message;
                let mut cursor = node.walk();
                let expr = node
                    .children(&mut cursor)
                    .find(|c| c.kind() != "assert" && c.kind() != ";" && c.kind() != ":");

                if let Some(e) = expr {
                    let expr_text = self.text(e).trim();
                    if expr_text == "true" {
                        test_fn.tautologies += 1;
                    } else {
                        test_fn.strong_asserts += 1;
                    }
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "method_invocation" => {
                self.inspect_method_invocation(node, test_fn, direct_calls);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.scan_statement_or_expr(child, test_fn, direct_calls);
                }
            }
        }
    }

    fn inspect_method_invocation(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        direct_calls: &mut Vec<String>,
    ) {
        let method_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");

        let object = node.child_by_field_name("object");
        let is_local_call = match object {
            None => true,
            Some(obj) => {
                let t = self.text(obj).trim();
                t == "this" || t == "super"
            }
        };
        if is_local_call {
            direct_calls.push(method_name.to_string());
        }

        let args_node = node.child_by_field_name("arguments");
        let args = Self::collect_arguments(args_node);

        match method_name {
            // A configured helper counts one total and no strong assertion, whatever else
            // its name looks like.
            m if self
                .vocab
                .helper_fns
                .iter()
                .any(|h| super::helper_call_matches(m, h)) =>
            {
                test_fn.total_asserts += 1;
            }
            "assertTrue" => {
                test_fn.total_asserts += 1;
                if let Some(first) = args.first() {
                    let t = self.text(*first).trim();
                    if t == "true" {
                        test_fn.tautologies += 1;
                    }
                }
            }
            "assertFalse" => {
                test_fn.total_asserts += 1;
                if let Some(first) = args.first() {
                    let t = self.text(*first).trim();
                    if t == "false" {
                        test_fn.tautologies += 1;
                    }
                }
            }
            "assertEquals" | "assertSame" => {
                test_fn.total_asserts += 1;
                if args.len() >= 2 {
                    let a = self.text(args[0]).trim();
                    let b = self.text(args[1]).trim();
                    if a == b {
                        test_fn.tautologies += 1;
                    } else {
                        test_fn.strong_asserts += 1;
                    }
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "assertNotEquals" | "assertNotSame" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "assertNull" => {
                test_fn.total_asserts += 1;
                if let Some(first) = args.first() {
                    let t = self.text(*first).trim();
                    if t == "null" {
                        test_fn.tautologies += 1;
                    } else {
                        test_fn.strong_asserts += 1;
                    }
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "assertNotNull" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "assertThrows" | "assertThrowsExactly" | "assertDoesNotThrow" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "assertArrayEquals" | "assertIterableEquals" | "assertLinesMatch" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "assertInstanceOf" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "fail" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "assertThat" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "assertThatThrownBy" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            other => {
                let is_custom_assert = other.starts_with("assert")
                    && other.len() > 6
                    && other.chars().nth(6).is_some_and(|c| c.is_ascii_uppercase());
                if is_custom_assert {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
            }
        }

        // Recursively check children (e.g. arguments or object call chains)
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child != args_node.unwrap_or(child) {
                self.scan_statement_or_expr(child, test_fn, direct_calls);
            } else {
                // Also scan inside arguments for lambdas or nested assert calls (like assertThrows(() -> ...))
                let mut arg_cursor = child.walk();
                for arg_child in child.children(&mut arg_cursor) {
                    self.scan_statement_or_expr(arg_child, test_fn, direct_calls);
                }
            }
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (i, test) in self.facts.tests.iter_mut().enumerate() {
            if let Some(calls) = self.test_calls.get(i) {
                super::resolve_test_same_file_helpers(test, calls, self.vocab, |call| {
                    super::helper_through_wrappers(call, &self.helpers)
                });
            }
        }
    }

    fn collect_arguments(args_node: Option<Node>) -> Vec<Node> {
        let mut result = Vec::new();
        let Some(args) = args_node else {
            return result;
        };
        let mut cursor = args.walk();
        for child in args.children(&mut cursor) {
            if child.is_named() {
                result.push(child);
            }
        }
        result
    }
}

/// A method without a `body` field (abstract, interface) never reaches the classifier;
/// a `default` interface method or a class method does.
fn java_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    if functions::is_test_file(path, Some(is_java_test_path)) {
        return true;
    }
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| {
        c.kind() == "modifiers"
            && c.utf8_text(src.as_bytes())
                .is_ok_and(|t| t.contains("@Test") || t.contains("@ParameterizedTest"))
    });
    found
}

pub const JAVA_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["method_declaration", "constructor_declaration"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["line_comment", "block_comment"],
    skip: functions::skip_none,
    is_test: java_fn_is_test,
    classify: functions::classify_jvm,
};

/// A method called on a receiver (`method_checks`).
pub const JAVA_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[],
        direct: &[("method_invocation", "object", "name")],
    };

pub const JAVA_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["method_invocation", "object_creation_expression"],
    callee_fields: &["name"],
};

pub const JAVA_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_clause"],
    arm_of: &[],
    body_fields: &["body", "block"],
    ignored_kinds: &["line_comment", "block_comment"],
    trivial: &[
        "return",
        "return null",
        "return false",
        "return 0",
        "return \"\"",
        "return Collections.emptyList()",
        "return Collections.emptyMap()",
        "return Collections.emptySet()",
        "return List.of()",
        "return Map.of()",
        "return Set.of()",
        "return Optional.empty()",
        "return new ArrayList<>()",
        "return new HashMap<>()",
        "continue",
    ],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
    silence_node: None,
};

/// A handler statement that puts a number in place of the result (`constant-fallback`):
/// `ops = 150000.0`, `this.ops = -1`, `raw[0] = 2.5e5`, `return 150000L`,
/// `return new double[] {1.5, 2.0}`. `null` and `Double.NaN` are not numeric literals;
/// `List.of(1.5)` is a call.
pub const JAVA_CONSTANTS: super::handlers::ConstantSpec = super::handlers::ConstantSpec {
    blocks: &["block"],
    wrappers: &["expression_statement", "parenthesized_expression"],
    numbers: &[
        "decimal_integer_literal",
        "hex_integer_literal",
        "octal_integer_literal",
        "binary_integer_literal",
        "decimal_floating_point_literal",
        "hex_floating_point_literal",
    ],
    signs: &["unary_expression"],
    assignments: &["assignment_expression"],
    targets: &["identifier", "field_access", "array_access"],
    calls: &["method_invocation", "object_creation_expression"],
    returns: &["return_statement"],
    value_is_last_expression: false,
    collections: &["array_initializer"],
    collection_holders: &["array_creation_expression"],
    pairs: &[],
    keys: &[],
};

pub const JAVA_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["annotation", "marker_annotation"],
};

pub const JAVA_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["block"],
    ignored_kinds: &["line_comment", "block_comment"],
    terminators: &["return", "throw"],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const JAVA_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &[],
    names: &[],
    references: &["method_reference"],
};

/// A local a Java wrapper computes and forwards: `String b = loc(d);`.
pub const JAVA_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &["local_variable_declaration"],
    binders: &["variable_declarator"],
    pattern: &["name"],
    value: &["value"],
    names: &["identifier"],
    holders: &[],
    refused: &[],
};

/// A Java helper whose body is one call: `{ return check(x, true); }`.
pub const JAVA_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &["block", "expression_statement", "return_statement"],
    calls: &["method_invocation"],
    arguments: &["argument_list"],
    references: &[],
    plain: &["true", "false", "this", "super"],
    skip: &["line_comment", "block_comment"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole-file handler rule is the shared-or-own superset: an empty handler is
    /// silent in a file only the shared rule recognises (`benches/`) and in one only
    /// the pack's own `Test` prefix recognises, and fires elsewhere.
    #[test]
    fn whole_file_handler_rule_is_the_shared_or_own_superset() {
        let src = "class Helper {\n  void m() {\n    try {\n      g();\n    } catch (Exception e) {}\n  }\n}\n";
        let swallowed = |path: &str| {
            JavaPack
                .extract(path, src, &AssertVocabulary::default())
                .unwrap()
                .swallowed
                .len()
        };
        assert_eq!(swallowed("benches/Helper.java"), 0, "shared rule only");
        assert_eq!(
            swallowed("src/main/java/TestHelper.java"),
            0,
            "own rule only"
        );
        assert_eq!(
            swallowed("src/main/java/Helper.java"),
            1,
            "negative control"
        );
    }

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"class WrapTest {
  void checked(int x, boolean strict) {
    if (strict && x != 1) { throw new IllegalStateException("x"); }
  }
  void noop(int x, int n) {}
  void via(int x) { checked(x, true); }
  void hollow(int x) { this.noop(x, 1); }
  void busy(int x) {
    checked(x, true);
    prepare(x);
  }
  void ping(int x) { pong(x, 1); }
  void pong(int x, int n) { ping(x); }
  @Test void direct() { checked(1, true); }
  @Test void viaWrapper() { via(1); }
  @Test void hollowWrapper() { hollow(1); }
  @Test void busyHelper() { busy(1); }
  @Test void wrapperCycle() { ping(1); }
}
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&JavaPack, "src/test/java/WrapTest.java", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
    }

    #[test]
    fn test_junit5_test_extraction_and_assertion_counting() {
        let src = r#"
package io.github.example.project;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class ExampleMapTest {
    @Test
    void testSlotSegment() {
        assertEquals(99L, 99L + 0);
        assertTrue(true); // tautology
        assertFalse(false); // tautology
        assertThrows(IllegalStateException.class, () -> {
            throw new IllegalStateException();
        });
    }
}
"#;
        let pack = JavaPack;
        let facts = pack
            .extract("ExampleMapTest.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "ExampleMapTest.testSlotSegment");
        assert_eq!(t.total_asserts, 4);
        assert_eq!(t.strong_asserts, 2); // assertEquals + assertThrows
        assert_eq!(t.tautologies, 2); // assertTrue(true) + assertFalse(false)
        assert_eq!(t.effective_asserts(), 2);
        assert!(!t.is_vacuous());
        assert!(!t.ignored);
    }

    #[test]
    fn suppress_warnings_counts_as_an_annotation_never_in_a_comment() {
        let src = "class A {\n  // no @SuppressWarnings(\"unchecked\") here\n  /** see {@code @SuppressWarnings(\"rawtypes\")} */\n  @SuppressWarnings(\"deprecation\")\n  void f() {}\n}\n";
        let facts = JavaPack
            .extract("A.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");
        let rules: Vec<(usize, String)> = facts
            .escape_hatches
            .iter()
            .filter_map(|h| match h {
                EscapeHatchSite::LinterDisable { line, rule, .. } => Some((*line, rule.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(rules, [(4, "deprecation".to_string())]);
    }

    #[test]
    fn test_vacuous_java_test_detected() {
        let src = r#"
class VacuousTest {
    @Test
    void emptyTest() {}

    @Test
    void tautologicalTest() {
        assertTrue(true);
    }

    @Test
    void identicalEquals() {
        assertEquals(42, 42);
    }
}
"#;
        let pack = JavaPack;
        let facts = pack
            .extract("VacuousTest.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 3);
        assert!(facts.tests[0].is_vacuous());
        assert!(facts.tests[1].is_vacuous());
        assert!(facts.tests[2].is_vacuous());
    }

    #[test]
    fn test_disabled_and_ignored_annotations() {
        let src = r#"
import org.junit.jupiter.api.Disabled;
import org.junit.jupiter.api.Test;
import org.junit.Ignore;

class DisabledTest {
    @Test
    @Disabled("temporarily skip")
    void disabledJunit5() {
        assertEquals(1, 2);
    }

    @Test
    @Ignore
    void ignoredJunit4() {
        assertEquals(1, 2);
    }

    @Test
    void activeTest() {
        assertEquals(1, 2);
    }
}
"#;
        let pack = JavaPack;
        let facts = pack
            .extract("DisabledTest.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 3);
        assert!(facts.tests[0].ignored);
        assert!(facts.tests[1].ignored);
        assert!(!facts.tests[2].ignored);
    }

    #[test]
    fn test_class_level_disabled_ignores_all_methods() {
        let src = r#"
import org.junit.jupiter.api.Disabled;
import org.junit.jupiter.api.Test;

@Disabled
class EntireClassDisabledTest {
    @Test
    void methodA() {
        assertEquals(1, 2);
    }

    @Test
    void methodB() {
        assertEquals(3, 4);
    }
}
"#;
        let pack = JavaPack;
        let facts = pack
            .extract(
                "EntireClassDisabledTest.java",
                src,
                &AssertVocabulary::default(),
            )
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 2);
        assert!(facts.tests[0].ignored);
        assert!(facts.tests[1].ignored);
    }

    /// The helper is defined in no file the pack reads, so only the configured
    /// vocabulary can make its call count.
    #[test]
    fn test_assertj_and_custom_vocab() {
        let src = r#"
import static org.assertj.core.api.Assertions.assertThat;
import org.junit.jupiter.api.Test;

class AssertJTest extends SharedChecks {
    @Test
    void testAssertJ() {
        assertThat("foo").isEqualTo("foo");
        customCheckHelper(42);
    }
}
"#;
        let vocab = AssertVocabulary {
            helper_fns: vec!["customCheckHelper".to_string()],
            ..Default::default()
        };
        let pack = JavaPack;
        let facts = pack
            .extract("AssertJTest.java", src, &vocab)
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        // assertThat(..).isEqualTo(..) is one strong assertion; the configured helper
        // adds one to the total and none to the strong count.
        assert_eq!(t.total_asserts, 2);
        assert_eq!(t.strong_asserts, 1);
        assert!(!t.is_vacuous());

        // Control: without the vocabulary the helper call is not an assertion.
        let plain = pack
            .extract("AssertJTest.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");
        assert_eq!(plain.tests.len(), 1);
        assert_eq!(plain.tests[0].total_asserts, 1);
        assert_eq!(plain.tests[0].strong_asserts, 1);
    }

    #[test]
    fn test_java_assert_prefix_and_helper_resolution() {
        let src = r#"
import org.junit.jupiter.api.Test;

class DomainTest {
    private void verifyItem(String val) {
        assertEquals("expected", val);
    }

    @Test
    void testCustomAssertions() {
        assertPreconditionViolationFor("foo");
        assertMetaDataIsEqualTo("meta");
        verifyItem("test");
    }
}
"#;
        let pack = JavaPack;
        let facts = pack
            .extract("DomainTest.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "DomainTest.testCustomAssertions");
        // assertPreconditionViolationFor (1) + assertMetaDataIsEqualTo (1) + verifyItem helper (1) = 3
        assert_eq!(t.total_asserts, 3);
        assert_eq!(t.strong_asserts, 3);
        assert!(!t.is_vacuous());
    }
}

/// Conditional skips of JUnit: annotations and assumptions (#597). The sources are
/// fixtures, kept out of the test bodies.
#[cfg(test)]
mod conditional_skip_tests {
    use super::*;

    const ANNOTATED: &str = r#"
@DisabledOnOs(OS.WINDOWS)
class QTest {
    @Test
    @DisabledIfEnvironmentVariable(named = "CI", matches = "true")
    void inCi() { assertEquals(2, 1 + 1); }

    @Test
    @EnabledIfEnvironmentVariable(named = "CI", matches = "true")
    void outsideCi() { assertEquals(2, 1 + 1); }

    @Test
    @DisabledIfSystemProperty(named = "os.arch", matches = ".*32.*")
    void property() { assertEquals(2, 1 + 1); }

    @Test
    void inherits() { assertEquals(2, 1 + 1); }

    @Test
    @Disabled("x")
    void off() { assertEquals(2, 1 + 1); }
}
"#;

    const ASSUMED: &str = r#"
class QTest {
    private static boolean inCi() {
        return System.getenv("CI") != null;
    }

    @Test
    void skipsInCi() {
        assumeFalse(inCi());
        assertEquals(2, 1 + 1);
    }

    @Test
    void runsOnlyInCi() {
        Assumptions.assumeTrue("true".equals(System.getenv("GITHUB_ACTIONS")));
        assertEquals(2, 1 + 1);
    }

    @Test
    void platform() {
        assumeTrue(System.getProperty("os.name").startsWith("Linux"));
        assertEquals(2, 1 + 1);
    }

    @Test
    void otherSpellings() {
        assumeTrue(System.getenv().get("CI") == null);
        assumeFalse(Boolean.parseBoolean(System.getenv("TRAVIS")));
        assumeTrue(Objects.isNull(System.getenv("CIRCLECI")));
        assumeFalse(System.getenv().containsKey("BUILDKITE"));
        assumeTrue(System.getenv("GITLAB_CI").isEmpty());
        assumeFalse(Boolean.getBoolean("ci"));
        assertEquals(2, 1 + 1);
    }

    @Test
    void never() {
        assumeTrue(false);
        assertEquals(2, 1 + 1);
    }

    @Test
    void always() {
        assumeTrue(true);
        assertEquals(2, 1 + 1);
    }
}
"#;

    /// `(name, unconditionally skipped, condition, skips in CI)` of each test.
    fn read(src: &str) -> Vec<(String, bool, Option<String>, bool)> {
        JavaPack
            .extract(
                "src/test/java/QTest.java",
                src,
                &AssertVocabulary::default(),
            )
            .unwrap()
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    t.ignored,
                    t.conditional_ignore.clone(),
                    t.is_ci_skip(),
                )
            })
            .collect()
    }

    fn row(
        name: &str,
        ignored: bool,
        cond: Option<&str>,
        ci: bool,
    ) -> (String, bool, Option<String>, bool) {
        (name.to_string(), ignored, cond.map(str::to_string), ci)
    }

    #[test]
    fn conditional_annotations_are_read_on_a_method_and_inherited_from_its_class() {
        assert_eq!(
            read(ANNOTATED),
            vec![
                row(
                    "QTest.inCi",
                    false,
                    Some("@DisabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")"),
                    true
                ),
                // The class condition comes first; neither is a skip in CI.
                row(
                    "QTest.outsideCi",
                    false,
                    Some("@DisabledOnOs(OS.WINDOWS)"),
                    false
                ),
                row(
                    "QTest.property",
                    false,
                    Some("@DisabledOnOs(OS.WINDOWS)"),
                    false
                ),
                row(
                    "QTest.inherits",
                    false,
                    Some("@DisabledOnOs(OS.WINDOWS)"),
                    false
                ),
                row("QTest.off", true, None, false),
            ]
        );
    }

    #[test]
    fn assumptions_are_read_by_their_condition() {
        assert_eq!(
            read(ASSUMED),
            vec![
                row("QTest.skipsInCi", false, Some("assumeFalse(inCi())"), true),
                row(
                    "QTest.runsOnlyInCi",
                    false,
                    Some("Assumptions.assumeTrue(\"true\".equals(System.getenv(\"GITHUB_ACTIONS\")))"),
                    false
                ),
                row(
                    "QTest.platform",
                    false,
                    Some("assumeTrue(System.getProperty(\"os.name\").startsWith(\"Linux\"))"),
                    false
                ),
                row(
                    "QTest.otherSpellings",
                    false,
                    Some("assumeTrue(System.getenv().get(\"CI\") == null)"),
                    true
                ),
                row("QTest.never", true, None, false),
                row("QTest.always", false, None, false),
            ]
        );
        // Each spelling of the test above is a skip in CI on its own variable.
        let facts = JavaPack
            .extract(
                "src/test/java/QTest.java",
                ASSUMED,
                &AssertVocabulary::default(),
            )
            .unwrap();
        let spellings = facts
            .tests
            .iter()
            .find(|t| t.name == "QTest.otherSpellings")
            .unwrap();
        assert_eq!(
            spellings.ci_skip_vars(),
            vec!["CI", "TRAVIS", "CIRCLECI", "BUILDKITE", "GITLAB_CI"]
        );
    }

    #[test]
    fn junit_conditional_annotations_are_read_by_name_and_variable() {
        use crate::ast::ci_condition::jvm_annotation;
        let skips = |var: &str| Some(SkipCondition::When(CiVerdict::Skips(vec![var.to_string()])));
        let other = Some(SkipCondition::When(CiVerdict::NotCi));
        for (name, named, matches, want) in [
            (
                "DisabledIfEnvironmentVariable",
                Some("CI"),
                Some("true"),
                skips("CI"),
            ),
            (
                "DisabledIfEnvironmentVariable",
                Some("CI_JOB_ID"),
                Some(".*"),
                skips("CI_JOB_ID"),
            ),
            (
                "DisabledIfEnvironmentVariable",
                Some("SLOW"),
                Some(".*"),
                other.clone(),
            ),
            ("DisabledIfEnvironmentVariable", None, None, other.clone()),
            (
                "EnabledIfEnvironmentVariable",
                Some("CI"),
                Some("true"),
                other.clone(),
            ),
            (
                "EnabledIfEnvironmentVariable",
                Some("CI"),
                Some("false"),
                skips("CI"),
            ),
            (
                "EnabledIfEnvironmentVariable",
                Some("DB_URL"),
                Some("false"),
                other.clone(),
            ),
            (
                "DisabledIfSystemProperty",
                Some("ci"),
                Some("true"),
                skips("CI"),
            ),
            (
                "DisabledIfSystemProperty",
                Some("CI_JOB_ID"),
                Some(".*"),
                other.clone(),
            ),
            (
                "DisabledIfSystemProperty",
                Some("os.arch"),
                Some(".*"),
                other.clone(),
            ),
            (
                "EnabledIfSystemProperty",
                Some("ci"),
                Some("true"),
                other.clone(),
            ),
            (
                "EnabledIfSystemProperty",
                Some("ci"),
                Some("false"),
                skips("CI"),
            ),
            ("DisabledOnOs", None, None, other.clone()),
            ("EnabledOnOs", None, None, other.clone()),
            ("DisabledOnJre", None, None, other.clone()),
            ("EnabledForJreRange", None, None, other.clone()),
            ("DisabledIf", None, None, other.clone()),
            ("Disabled", None, None, None),
            ("Tag", Some("CI"), None, None),
        ] {
            assert_eq!(
                jvm_annotation(name, named, matches),
                want,
                "{name} {named:?} {matches:?}"
            );
        }
    }
}
