//! JavaScript and TypeScript language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::ci_condition::SkipCondition;
use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// JavaScript & TypeScript language pack implementing [`LanguagePack`].
pub struct JavaScriptPack;

impl LanguagePack for JavaScriptPack {
    fn id(&self) -> &'static str {
        "javascript"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests
                | Fact::EscapeHatches
                | Fact::Functions
                | Fact::Handlers
                | Fact::Prose
                | Fact::Budgets
        )
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
        let tree = crate::ast::source_text::parse_file(&mut parser, path, src)?;
        let root = tree.root_node();

        let mut extractor = JsExtractor {
            dead: super::reach::dead_ranges(root, src, &JS_REACH),
            src: src.as_bytes(),
            vocab,
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            test_calls: Vec::new(),
            suite_cases: Vec::new(),
            suite_skips: Vec::new(),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers(root);
        extractor.facts.functions = functions::extract(root, src, path, &JS_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &JS_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            // A file in a test directory, or one the repository declares as test scope, is
            // test code line for line.
            let whole_file = super::functions::test_path(path)
                || super::functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            (
                extractor.facts.swallowed,
                extractor.facts.constant_fallbacks,
            ) = super::handlers::extract_with_constants(
                root,
                src,
                &JS_HANDLERS,
                Some(&JS_CONSTANTS),
                &is_test_line,
            );
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &JS_RETRIES);
        if super::functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::method_checks::count(root, src, &mut extractor.facts, &JS_RECEIVER_CALLS);
        super::helper_loops::count(
            root,
            src,
            &mut extractor.facts,
            &super::helper_loops::JAVASCRIPT,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &JS_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &JS_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        super::bounds::javascript(root, src, &mut extractor.facts.tests);
        super::expectations::javascript(root, src, &mut extractor.facts.tests);
        super::caught_assertions::javascript(root, src, &mut extractor.facts.tests, vocab);
        super::expected_exceptions::javascript(root, src, &mut extractor.facts.tests);
        extractor.facts.prose =
            super::prose::extract(root, src, &["comment", "string", "template_string"]);
        extractor.facts.budgets = super::budgets::extract(root, src, &JS_BUDGETS);
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
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    facts: ParsedFileFacts,
    /// Same-file callees of each test, in `facts.tests` order.
    test_calls: Vec<Vec<String>>,
    /// The case counts of the enclosing suites: a `describe.each` table runs every test
    /// of its suite once per row.
    suite_cases: Vec<super::test_cases::CaseList>,
    /// Conditional skips of the enclosing suites (`describe.skipIf(..)`), outermost first:
    /// the condition as reported and what a CI variable decides about it.
    suite_skips: Vec<Vec<(String, super::ci_condition::CiVerdict)>>,
}

/// Function nodes whose body runs only when called.
const JS_FUNCTION_KINDS: &[&str] = &[
    "arrow_function",
    "function_expression",
    "function",
    "function_declaration",
    "generator_function_declaration",
    "method_definition",
    "class_declaration",
];

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
                let (is_test, is_suite, mut is_ignored, is_todo) = self.classify_call(func_node);
                // `test.skipIf(<condition>)`, `describe.runIf(<condition>)`: a conditional
                // skip, read by its condition. The text rule above reads `.skipIf` as
                // `.skip`, so the modifiers of the chain are read again from the tree.
                let mut conditional = Vec::new();
                if is_test || is_suite {
                    let (plain_skip, conditions) = self.chain_modifiers(func_node);
                    if !conditions.is_empty() {
                        is_ignored = plain_skip;
                    }
                    for (text, condition) in conditions {
                        match condition {
                            SkipCondition::Always => is_ignored = true,
                            SkipCondition::Never => {}
                            SkipCondition::When(verdict) => conditional.push((text, verdict)),
                        }
                    }
                }
                if is_suite {
                    let title = self.extract_first_arg_title(node);
                    scope.push(title);
                    self.suite_cases
                        .push(super::test_cases::extract_javascript_cases(
                            func_node, self.src,
                        ));
                    self.suite_skips.push(conditional);
                    if let Some(args) = node.child_by_field_name("arguments") {
                        if let Some(callback) = Self::find_callback(args) {
                            self.visit_node(callback, scope, parent_ignored || is_ignored);
                        }
                    }
                    self.suite_skips.pop();
                    self.suite_cases.pop();
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
                    let (cases, non_literal_cases, case_rows) = self
                        .suite_cases
                        .iter()
                        .fold(
                            super::test_cases::extract_javascript_cases(func_node, self.src),
                            |own, suite| super::test_cases::multiply_cases(suite.clone(), own),
                        )
                        .into_parts();

                    let mut test_fn = TestFn {
                        name: full_name,
                        line,
                        end_line,
                        total_asserts: 0,
                        strong_asserts: 0,
                        tautologies: 0,
                        ignored: parent_ignored || is_ignored || is_todo,
                        should_panic: None,
                        cases,
                        non_literal_cases,
                        case_rows,
                        ..Default::default()
                    };

                    if !test_fn.ignored {
                        let inherited = self.suite_skips.iter().flatten().cloned();
                        for (text, verdict) in inherited.chain(conditional) {
                            test_fn.record_conditional_skip(text, verdict);
                        }
                    }

                    let mut calls = Vec::new();
                    if let Some(args) = node.child_by_field_name("arguments") {
                        if let Some(callback) = Self::find_callback(args) {
                            self.scan_test_body(callback, &mut test_fn);
                            if let Some(body) = callback.child_by_field_name("body") {
                                self.collect_calls(body, &mut calls);
                                super::dispatch_calls(body, self.src, &JS_DISPATCH, &mut calls);
                                if !test_fn.ignored {
                                    self.record_conditional_early_exits(body, &mut test_fn);
                                }
                            }
                        }
                    }

                    self.facts.tests.push(test_fn);
                    self.test_calls.push(calls);
                    return;
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit_node(child, scope, parent_ignored);
        }
    }

    /// The same-file callees a test body runs: `name(...)`. A function defined in the
    /// body and not called there (`const f = () => helper()`) runs nothing.
    fn collect_calls(&self, node: Node, calls: &mut Vec<String>) {
        if JS_FUNCTION_KINDS.contains(&node.kind())
            && node
                .parent()
                .is_some_and(|p| p.kind() == "variable_declarator")
        {
            return;
        }
        if node.kind() == "call_expression" {
            if let Some(f) = node.child_by_field_name("function") {
                if f.kind() == "identifier" {
                    calls.push(self.text(f).to_string());
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_calls(child, calls);
        }
    }

    /// Named functions in the file: `function f() {}` and `const f = () => {}`.
    fn named_functions<'t>(&self, node: Node<'t>, out: &mut Vec<(String, Node<'t>)>) {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(n) = node.child_by_field_name("name") {
                    out.push((self.text(n).to_string(), node));
                }
            }
            "variable_declarator" => {
                if let (Some(n), Some(v)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                ) {
                    if matches!(
                        v.kind(),
                        "arrow_function" | "function_expression" | "function"
                    ) {
                        out.push((self.text(n).to_string(), v));
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.named_functions(child, out);
        }
    }

    /// Adds the failure paths of each same-file helper a test calls: its `expect` /
    /// `assert` calls and its `throw` statements. One level: a helper's own callees
    /// are not followed, except through a thin wrapper (`super::helper_through_wrappers`,
    /// bounded and cycle-safe).
    fn resolve_same_file_helpers(&mut self, root: Node) {
        let mut named = Vec::new();
        self.named_functions(root, &mut named);
        let mut helpers: std::collections::HashMap<String, super::HelperFacts> =
            std::collections::HashMap::new();
        for (name, func) in named {
            if helpers.contains_key(&name) {
                continue;
            }
            let mut h = TestFn::default();
            self.scan_test_body(func, &mut h);
            let mut wraps = None;
            let mut calls = Vec::new();
            if let Some(body) = func.child_by_field_name("body") {
                self.collect_calls(body, &mut calls);
                wraps = super::forwarding_wrapper_callee(
                    body,
                    &JS_WRAPPER,
                    &JS_LOCALS,
                    &calls,
                    self.src,
                );
                h.total_asserts += super::count_failure_exits(
                    body,
                    self.src,
                    &["throw_statement"],
                    &[],
                    JS_FUNCTION_KINDS,
                );
            }
            let line = func.start_position().row + 1;
            let end_line = func.end_position().row + 1;
            self.facts.push_helper(
                super::TestHelperFacts {
                    name: name.clone(),
                    line,
                    end_line,
                    total_asserts: h.total_asserts,
                    strong_asserts: h.strong_asserts,
                    tautologies: h.tautologies,
                    fatal_asserts: h.fatal_asserts,
                    helper_checks: 0,
                },
                calls,
            );
            helpers.insert(
                name,
                super::HelperFacts {
                    total_asserts: h.total_asserts,
                    strong_asserts: h.strong_asserts,
                    tautologies: h.tautologies,
                    fatal_asserts: h.fatal_asserts,
                    wraps,
                },
            );
        }
        // A class's methods (`class Checker { check(r) { expect(..) } }`) are tracked as
        // `Class.method`. A call through an object is not resolved to one: which class the
        // object is, is not known where the call is read.
        let mut methods = Vec::new();
        self.class_methods(root, None, &mut methods);
        for (name, body) in methods {
            let mut h = TestFn::default();
            self.scan_test_body(body, &mut h);
            let mut calls = Vec::new();
            self.collect_calls(body, &mut calls);
            h.total_asserts += super::count_failure_exits(
                body,
                self.src,
                &["throw_statement"],
                &[],
                JS_FUNCTION_KINDS,
            );
            self.facts.push_helper(
                super::TestHelperFacts {
                    name,
                    line: body.start_position().row + 1,
                    end_line: body.end_position().row + 1,
                    total_asserts: h.total_asserts,
                    strong_asserts: h.strong_asserts,
                    tautologies: h.tautologies,
                    fatal_asserts: h.fatal_asserts,
                    helper_checks: 0,
                },
                calls,
            );
        }
        for (test, calls) in self.facts.tests.iter_mut().zip(&self.test_calls) {
            super::resolve_test_same_file_helpers(test, calls, self.vocab, |call| {
                super::helper_through_wrappers(call, &helpers)
            });
        }
    }

    /// The methods with a body of every named class: `(Class.method, body)`.
    fn class_methods<'t>(
        &self,
        node: Node<'t>,
        class: Option<&str>,
        out: &mut Vec<(String, Node<'t>)>,
    ) {
        let mut class = class;
        match node.kind() {
            "class_declaration" | "class" | "abstract_class_declaration" => {
                class = node.child_by_field_name("name").map(|n| self.text(n));
            }
            "method_definition" => {
                if let (Some(c), Some(n), Some(body)) = (
                    class,
                    node.child_by_field_name("name"),
                    node.child_by_field_name("body"),
                ) {
                    out.push((format!("{c}.{}", self.text(n)), body));
                }
                // A class declared inside the method is found under its own name.
                class = None;
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        for child in children {
            self.class_methods(child, class, out);
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
        if super::reach::is_dead(&self.dead, body_or_fn.start_byte()) {
            return;
        }
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
                if !test.ignored {
                    self.record_this_skip(body_or_fn, test);
                }
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

    /// The modifiers of a test or suite call (`test.skip.each`, `test.skipIf(c)`,
    /// `describe.runIf(c)`), read from the member chain of its function: whether one is
    /// exactly `skip`, `only` or `todo`, and each condition a `skipIf` / `runIf` takes,
    /// as the condition under which the test is skipped and what it does.
    fn chain_modifiers(&self, func: Node) -> (bool, Vec<(String, SkipCondition)>) {
        use super::ci_condition::{self, Lang};
        let mut plain_skip = false;
        let mut conditions = Vec::new();
        let mut cur = func;
        loop {
            match cur.kind() {
                "call_expression" => {
                    let Some(callee) = cur.child_by_field_name("function") else {
                        break;
                    };
                    let modifier = (callee.kind() == "member_expression")
                        .then(|| callee.child_by_field_name("property"))
                        .flatten()
                        .map(|p| self.text(p));
                    if let Some(name @ ("skipIf" | "runIf")) = modifier {
                        let condition = cur.child_by_field_name("arguments").and_then(|args| {
                            let mut cursor = args.walk();
                            let first = args.named_children(&mut cursor).next();
                            first
                        });
                        match condition {
                            Some(condition) => {
                                let run_if = name == "runIf";
                                let text = self.text(condition).trim();
                                conditions.push((
                                    if run_if {
                                        format!("!({text})")
                                    } else {
                                        text.to_string()
                                    },
                                    ci_condition::skip_condition(
                                        Lang::JavaScript,
                                        condition,
                                        self.src,
                                        run_if,
                                    ),
                                ));
                            }
                            // `test.skipIf()`: no condition to read.
                            None => plain_skip = true,
                        }
                    }
                    cur = callee;
                }
                "member_expression" => {
                    if let Some(property) = cur.child_by_field_name("property") {
                        if matches!(self.text(property), "skip" | "only" | "todo") {
                            plain_skip = true;
                        }
                    }
                    let Some(object) = cur.child_by_field_name("object") else {
                        break;
                    };
                    cur = object;
                }
                _ => break,
            }
        }
        (plain_skip, conditions)
    }

    /// Mocha's `this.skip()`. As a statement of the test it is an unconditional skip;
    /// under an `if` it is a conditional skip read by its condition, and in the `else`
    /// branch of a condition on no CI variable it is unconditional, as a skip call is in
    /// the other packs. Elsewhere (a loop, a nested callback) it is not read.
    fn record_this_skip(&self, call: Node, test: &mut TestFn) {
        use super::ci_condition::{self, Lang};
        let Some(func) = call.child_by_field_name("function") else {
            return;
        };
        if func.kind() != "member_expression"
            || func.child_by_field_name("object").map(|o| o.kind()) != Some("this")
            || func.child_by_field_name("property").map(|p| self.text(p)) != Some("skip")
        {
            return;
        }
        match ci_condition::site(Lang::JavaScript, call, self.src) {
            Some(site) if site.in_else && !site.related => test.ignored = true,
            Some(site) => test.record_conditional_skip(site.text, site.verdict),
            None => {
                // `this.skip();` directly in the body of the test callback.
                let statement = call.parent().filter(|p| p.kind() == "expression_statement");
                let block = statement
                    .and_then(|s| s.parent())
                    .filter(|b| b.kind() == "statement_block");
                let callback = block.and_then(|b| b.parent()).filter(|f| {
                    matches!(
                        f.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                });
                let test_call = callback
                    .and_then(|f| f.parent())
                    .filter(|args| args.kind() == "arguments")
                    .and_then(|args| args.parent());
                if test_call.is_some_and(|c| {
                    c.start_position().row + 1 == test.line
                        && c.end_position().row + 1 == test.end_line
                }) {
                    test.ignored = true;
                }
            }
        }
    }

    /// Early exits under a condition: the first `if` the text rule below accepts, then
    /// every `return` under an `if` (nested and `else` branches included) that a CI
    /// variable is involved in, through a variable, constant or helper of this file.
    fn record_conditional_early_exits(&self, body: Node, test: &mut TestFn) {
        use super::ci_condition::{self, CiVerdict, Lang};
        if let Some((cond, consequence)) = self.detect_js_conditional_early_exit(body) {
            let verdict = ci_condition::site(Lang::JavaScript, consequence, self.src)
                .map_or(CiVerdict::NotCi, |s| s.verdict);
            test.record_conditional_skip(cond, verdict);
        }
        for exit in ci_condition::exits_under_if(body, &|n| n.kind() == "return_statement") {
            if let Some(site) = ci_condition::site(Lang::JavaScript, exit, self.src) {
                if site.related {
                    test.record_conditional_skip(site.text, site.verdict);
                }
            }
        }
    }

    fn detect_js_conditional_early_exit<'t>(&self, body: Node<'t>) -> Option<(String, Node<'t>)> {
        if body.kind() != "statement_block" {
            return None;
        }
        let mut cursor = body.walk();
        let mut env_bindings = std::collections::HashSet::new();

        for child in body.children(&mut cursor) {
            if child.kind() == "lexical_declaration" || child.kind() == "variable_declaration" {
                let text = self.text(child);
                if is_js_env_check(text) {
                    let mut decl_cursor = child.walk();
                    for decl in child.children(&mut decl_cursor) {
                        if decl.kind() == "variable_declarator" {
                            if let Some(name_node) = decl.child_by_field_name("name") {
                                let name = self.text(name_node).trim();
                                if !name.is_empty() {
                                    env_bindings.insert(name.to_string());
                                }
                            }
                        }
                    }
                }
            }

            if child.kind() == "if_statement" {
                let cond_node = child.child_by_field_name("condition")?;
                let cond_text = self.text(cond_node).trim();
                let unwrapped = cond_text
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(cond_text)
                    .trim();

                let is_env_check = is_js_env_check(unwrapped)
                    || env_bindings.iter().any(|v| {
                        unwrapped == v
                            || unwrapped
                                .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
                                .any(|t| t == v)
                    });

                if is_env_check {
                    let consequence = child.child_by_field_name("consequence")?;
                    if js_consequence_returns_early(consequence) {
                        return Some((unwrapped.to_string(), consequence));
                    }
                }
            }
        }
        None
    }

    /// Records where the tautologies counted under `call` are (`TestFn::mark_tautologies`).
    fn check_assertion_call(&self, call: Node, test: &mut TestFn) {
        let mark = test.tautology_mark();
        self.check_assertion_call_unmarked(call, test);
        test.mark_tautologies(mark, call);
    }

    fn check_assertion_call_unmarked(&self, call: Node, test: &mut TestFn) {
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
                .any(|h| super::helper_call_matches(func_text, h))
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

fn is_js_env_check(text: &str) -> bool {
    text.contains("process.env") || text.contains("process?.env") || super::is_ci_condition(text)
}

fn js_consequence_returns_early(consequence: Node) -> bool {
    if consequence.kind() == "return_statement" {
        return true;
    }
    if consequence.kind() == "statement_block" {
        let mut cursor = consequence.walk();
        for child in consequence.children(&mut cursor) {
            if child.kind() == "return_statement" {
                return true;
            }
        }
    }
    false
}

/// Overload signatures, abstract members and `declare` blocks carry no body.
fn js_fn_skip(node: tree_sitter::Node, src: &str) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        match p.kind() {
            "ambient_declaration" | "interface_declaration" | "abstract_class_declaration"
                if node.kind() != "method_definition" =>
            {
                return true
            }
            "ambient_declaration" | "interface_declaration" => return true,
            _ => {}
        }
        cur = p.parent();
    }
    let t = node.utf8_text(src.as_bytes()).unwrap_or("");
    t.trim_start().starts_with("abstract ") || t.trim_start().starts_with("declare ")
}

fn js_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    if functions::test_path(path) {
        return true;
    }
    // A callback passed to `it(` / `test(` / `describe(`.
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "call_expression" {
            let callee = p
                .child_by_field_name("function")
                .and_then(|f| f.utf8_text(src.as_bytes()).ok())
                .unwrap_or("");
            let leaf = callee.rsplit('.').next().unwrap_or(callee);
            if matches!(
                leaf,
                "it" | "test" | "describe" | "beforeEach" | "afterEach"
            ) {
                return true;
            }
        }
        cur = p.parent();
    }
    false
}

pub const JS_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &[
        "function_declaration",
        "method_definition",
        "arrow_function",
        "function_expression",
        "generator_function_declaration",
    ],
    name_fields: &["name"],
    body_fields: &["body", "statement_block"],
    ignored_kinds: &["comment"],
    skip: js_fn_skip,
    is_test: js_fn_is_test,
    classify: functions::classify_javascript,
};

/// A method called on a receiver (`method_checks`).
pub const JS_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[(
            "call_expression",
            "function",
            "member_expression",
            "property",
        )],
        direct: &[],
        bare: &[],
        tokens: &[],
    };

pub const JS_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
};

pub const JS_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_clause"],
    arm_of: &[],
    body_fields: &["body", "statement_block"],
    ignored_kinds: &["comment"],
    trivial: &[
        "return",
        "return null",
        "return undefined",
        "return false",
        "return 0",
        "return \"\"",
        "return ''",
        "return []",
        "return {}",
        "continue",
    ],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    // `p.catch(() => {})`: a rejection handler that does nothing, or only logs, drops the error.
    silence_kinds: &["call_expression"],
    silences: super::handlers::js_catch_text,
    silence_node: Some(super::handlers::js_catch_site_kind),
};

/// A handler statement that puts a number in place of the result (`constant-fallback`):
/// `ops = 150000.0`, `rec.ops = -1`, `rec["ops"] = 2.5e5`, `return 150000`, `return [1.5, 2]`,
/// `return { ops: 1.5 }`. `null`, `undefined`, `NaN` and `Number.NaN` are not numeric
/// literals; a `let` or `const` in the handler is a declaration, not an assignment.
pub const JS_CONSTANTS: super::handlers::ConstantSpec = super::handlers::ConstantSpec {
    blocks: &["statement_block"],
    wrappers: &["expression_statement", "parenthesized_expression"],
    numbers: &["number"],
    signs: &["unary_expression"],
    assignments: &["assignment_expression"],
    targets: &["identifier", "member_expression", "subscript_expression"],
    calls: &["call_expression", "new_expression"],
    returns: &["return_statement"],
    value_is_last_expression: false,
    collections: &["array", "object"],
    collection_holders: &[],
    pairs: &["pair"],
    keys: &["property_identifier", "string", "number"],
};

pub const JS_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["call_expression", "decorator"],
};

pub const JS_BUDGETS: super::budgets::BudgetSpec = super::budgets::BudgetSpec {
    key_values: &[super::budgets::KeyValueShape {
        kind: "pair",
        key_field: "key",
        value_field: "value",
    }],
    keys: &[("numRuns", "fast-check numRuns")],
    call_kind: "call_expression",
    callee_field: "function",
    arguments_field: "arguments",
    methods: &[],
    integer_kinds: &["number"],
    token_tree_kinds: &[],
};

pub const JS_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["statement_block"],
    ignored_kinds: &["comment"],
    terminators: &["return", "throw"],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const JS_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["array"],
    names: &["identifier"],
    references: &[],
};

/// A local a JavaScript / TypeScript wrapper computes and forwards: `const b = loc(d);`.
pub const JS_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &["lexical_declaration", "variable_declaration"],
    binders: &["variable_declarator"],
    pattern: &["name"],
    value: &["value"],
    names: &["identifier"],
    holders: &[],
    refused: &[],
};

/// A JS / TS helper whose body is one call: `{ return check(x, true); }`, `(x) => check(x)`.
pub const JS_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &[
        "statement_block",
        "expression_statement",
        "return_statement",
        "await_expression",
        "parenthesized_expression",
    ],
    calls: &["call_expression"],
    arguments: &["arguments", "array", "object", "pair"],
    references: &[],
    plain: &[
        "true",
        "false",
        "null",
        "undefined",
        "this",
        "number",
        "string",
    ],
    skip: &["comment"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"function checked(x, strict) {
  if (strict && x !== 1) { throw new Error('x'); }
}
function noop(x, n) {}
const via = (x) => checked(x, true);
function hollow(x) { return noop(x, 1); }
function busy(x) {
  checked(x, true);
  prepare(x);
}
function ping(x) { pong(x, 1); }
function pong(x, n) { ping(x); }
test('direct', () => { checked(1, true); });
test('via wrapper', () => { via(1); });
test('hollow wrapper', () => { hollow(1); });
test('busy helper', () => { busy(1); });
test('wrapper cycle', () => { ping(1); });
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&JavaScriptPack, "test/wrap.test.js", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
    }

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
    fn fixture_same_file_helpers_resolve_and_a_defined_lambda_runs_nothing() {
        // `assertValidUser` is a same-file helper: its `expect` counts once, for the
        // direct call. `const check = (u) => assertValidUser(u)` defines a lambda the
        // test never calls, so it adds nothing.
        let src = include_str!("../../tests/fixtures/javascript/helpers_and_lambdas.js");
        let unconfigured = JavaScriptPack
            .extract("test/helpers.test.js", src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(unconfigured.tests.len(), 1);
        let t = &unconfigured.tests[0];
        assert_eq!(
            (t.total_asserts, t.strong_asserts, t.helper_checks),
            (1, 1, 1),
            "{t:?}"
        );

        // Configuring the helper does not count the resolved call twice. (A configured
        // name is also counted inside a lambda, as it was before resolution existed.)
        let configured_vocab = AssertVocabulary {
            helper_fns: vec!["assertValidUser".to_string()],
            ..Default::default()
        };
        let configured = JavaScriptPack
            .extract("test/helpers.test.js", src, &configured_vocab)
            .unwrap();
        assert_eq!(configured.tests.len(), 1);
        assert!(!configured.tests[0].is_vacuous());
        assert_eq!(configured.tests[0].helper_checks, 1);
    }

    #[test]
    fn a_throwing_helper_counts_and_a_helper_only_in_a_lambda_does_not() {
        let src = "function checkRow(r) {\n  if (r.id !== 1) { throw new Error('id'); }\n}\nconst noop = () => {};\n\ntest('checks the row', () => {\n  checkRow(load());\n  noop();\n});\n\ntest('defines but never runs', () => {\n  const later = () => checkRow(load());\n});\n";
        let facts = JavaScriptPack
            .extract("test/row.test.js", src, &AssertVocabulary::default())
            .unwrap();
        let by = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();
        let direct = by("checks the row");
        assert_eq!(
            (direct.total_asserts, direct.helper_checks),
            (1, 1),
            "{direct:?}"
        );
        assert!(by("defines but never runs").is_vacuous());
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

    #[test]
    fn early_exit_in_js_test_under_env_or_ci_check_detected() {
        let src = r#"
test('ci dot check', () => {
    if (process.env.CI) {
        return;
    }
    expect(1).toBe(1);
});

it('ci bracket check', () => {
    if (process.env['GITHUB_ACTIONS']) return;
    expect(1).toBe(1);
});

test('generic env check', () => {
    if (process.env.SKIP_SLOW) {
        return;
    }
    expect(1).toBe(1);
});

test('guard without exit', () => {
    if (process.env.CI) {
        console.log("running in CI");
    }
    expect(1).toBe(1);
});

function helperGuard() {
    if (process.env.CI) return;
}
"#;
        let pack = JavaScriptPack;
        let facts = pack
            .extract("test/env_check.test.js", src, &AssertVocabulary::default())
            .unwrap();

        let ci_dot = facts
            .tests
            .iter()
            .find(|t| t.name == "ci dot check")
            .unwrap();
        assert!(!ci_dot.ignored);
        assert_eq!(ci_dot.conditional_ignore.as_deref(), Some("process.env.CI"));

        let ci_bracket = facts
            .tests
            .iter()
            .find(|t| t.name == "ci bracket check")
            .unwrap();
        assert!(!ci_bracket.ignored);
        assert_eq!(
            ci_bracket.conditional_ignore.as_deref(),
            Some("process.env['GITHUB_ACTIONS']")
        );

        let generic = facts
            .tests
            .iter()
            .find(|t| t.name == "generic env check")
            .unwrap();
        assert!(!generic.ignored);
        assert_eq!(
            generic.conditional_ignore.as_deref(),
            Some("process.env.SKIP_SLOW")
        );

        let no_exit = facts
            .tests
            .iter()
            .find(|t| t.name == "guard without exit")
            .unwrap();
        assert!(!no_exit.ignored);
        assert_eq!(no_exit.conditional_ignore, None);

        assert!(facts.tests.iter().all(|t| t.name != "helperGuard"));
    }
}

/// `skipIf` / `runIf` modifiers and Mocha's `this.skip()` (#597). The sources are
/// fixtures, kept out of the test bodies.
#[cfg(test)]
mod conditional_skip_tests {
    use super::*;

    const MODIFIERS: &str = r#"
const isCI = !!process.env.CI;
test.skipIf(isCI)('skip in ci', () => { expect(1).toBe(1); });
test.skipIf(process.platform === 'win32')('skip on windows', () => { expect(1).toBe(1); });
test.skipIf(true)('skip always', () => { expect(1).toBe(1); });
test.skipIf(false)('skip never', () => { expect(1).toBe(1); });
test.runIf(isCI)('run in ci', () => { expect(1).toBe(1); });
test.skipIf(isCI).each([1, 2])('each in ci', (n) => { expect(n).toBe(n); });
test.skip.each([1, 2])('each skipped', (n) => { expect(n).toBe(n); });
describe.skipIf(isCI)('suite in ci', () => {
  test('inner', () => { expect(1).toBe(1); });
  test.skipIf(process.platform === 'win32')('inner windows', () => { expect(1).toBe(1); });
});
describe.runIf(process.platform === 'linux')('suite on linux', () => {
  test('inner', () => { expect(1).toBe(1); });
});
describe.runIf(false)('suite never', () => {
  test('inner', () => { expect(1).toBe(1); });
});
"#;

    const THIS_SKIP: &str = r#"
it('top', function () {
  this.skip();
  expect(1).toBe(1);
});
it('under if', function () {
  if (process.platform === 'win32') { this.skip(); }
  expect(1).toBe(1);
});
it('else of another condition', function () {
  if (haveDb) { setup(); } else { this.skip(); }
  expect(1).toBe(1);
});
it('nested callback', function () {
  items.forEach(function () { this.skip(); });
  expect(1).toBe(1);
});
it('another receiver', function () {
  queue.skip();
  expect(1).toBe(1);
});
"#;

    /// `(name, unconditionally skipped, condition, skips in CI)` of each test.
    fn read(src: &str) -> Vec<(String, bool, Option<String>, bool)> {
        JavaScriptPack
            .extract("a.test.js", src, &AssertVocabulary::default())
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
    fn skip_if_and_run_if_are_read_by_their_condition_on_a_test_and_a_suite() {
        assert_eq!(
            read(MODIFIERS),
            vec![
                row("skip in ci", false, Some("isCI"), true),
                row(
                    "skip on windows",
                    false,
                    Some("process.platform === 'win32'"),
                    false
                ),
                row("skip always", true, None, false),
                row("skip never", false, None, false),
                row("run in ci", false, Some("!(isCI)"), false),
                row("each in ci", false, Some("isCI"), true),
                row("each skipped", true, None, false),
                row("suite in ci > inner", false, Some("isCI"), true),
                // The suite's condition is the one a CI variable decides: it is kept.
                row("suite in ci > inner windows", false, Some("isCI"), true),
                row(
                    "suite on linux > inner",
                    false,
                    Some("!(process.platform === 'linux')"),
                    false
                ),
                row("suite never > inner", true, None, false),
            ]
        );
    }

    #[test]
    fn this_skip_is_unconditional_as_a_statement_and_conditional_under_an_if() {
        assert_eq!(
            read(THIS_SKIP),
            vec![
                row("top", true, None, false),
                row(
                    "under if",
                    false,
                    Some("process.platform === 'win32'"),
                    false
                ),
                row("else of another condition", true, None, false),
                row("nested callback", false, None, false),
                row("another receiver", false, None, false),
            ]
        );
    }

    /// `(unconditional skip, conditional skip, a CI variable decides it)` of a test
    /// under `test.skipIf(<condition>)`.
    fn skip_if(condition: &str) -> (bool, bool, bool) {
        let src =
            format!("test.skipIf({condition})('adds', () => {{\n  expect(1 + 1).toBe(2);\n}});\n");
        let facts = JavaScriptPack
            .extract("a.test.js", &src, &AssertVocabulary::default())
            .unwrap();
        let test = &facts.tests[0];
        (
            test.ignored,
            test.conditional_ignore.is_some(),
            test.is_ci_skip(),
        )
    }

    #[test]
    fn a_constant_skip_if_condition_is_an_unconditional_skip_or_no_skip() {
        for condition in [
            "true",
            "!false",
            "1 === 1",
            "1",
            "(true)",
            "true || isSlow()",
        ] {
            assert_eq!(skip_if(condition), (true, false, false), "{condition}");
        }
        for condition in ["false", "!true", "0", "1 === 2", "false && isSlow()"] {
            assert_eq!(skip_if(condition), (false, false, false), "{condition}");
        }
        // Controls: a condition, not a constant.
        assert_eq!(
            skip_if("process.platform === 'win32'"),
            (false, true, false)
        );
        assert_eq!(skip_if("true && isSlow()"), (false, true, false));
        assert_eq!(skip_if("process.env.CI"), (false, true, true));
        assert_eq!(skip_if("!process.env.CI"), (false, true, false));
    }
}
