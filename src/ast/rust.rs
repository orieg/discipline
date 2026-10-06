//! Rust language pack: tree-sitter AST extraction of tests, assertions, and unsafe sites.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{
    AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn, UnsafeSite,
};

/// Rust language pack implementing [`LanguagePack`].
pub struct RustPack;

impl LanguagePack for RustPack {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests
                | Fact::EscapeHatches
                | Fact::UnsafeSites
                | Fact::Functions
                | Fact::Handlers
                | Fact::Prose
                | Fact::Budgets
        )
    }

    fn name(&self) -> &'static str {
        "Rust"
    }

    fn matches(&self, path: &str) -> bool {
        super::extension(path) == Some("rs")
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Rust grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let (owning_features, manifest_error) =
            vocab.runner_rules.rust.find_owning_crate_features(path);
        let mut facts = ParsedFileFacts {
            has_parse_errors: root.has_error(),
            ..Default::default()
        };
        if let Some(err_path) = manifest_error {
            facts
                .notes
                .push(format!("failed to parse manifest at '{err_path}'"));
        }

        let mut cx = Extractor {
            dead: super::reach::dead_ranges(root, src, &RS_REACH),
            src: src.as_bytes(),
            lines: src.lines().collect(),
            line_starts: std::iter::once(0)
                .chain(src.match_indices('\n').map(|(i, _)| i + 1))
                .collect(),
            vocab,
            comments: Vec::new(),
            in_fn: 0,
            in_const: 0,
            in_test: 0,
            test_file: super::functions::test_path(path)
                || super::functions::declared_test_path(path, &vocab.test_paths),
            library_helper: false,
            facts,
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
            owning_features,
        };
        cx.collect_comments(root);
        cx.visit(root, &mut Vec::new());
        cx.resolve_same_file_helpers();
        cx.facts.build_compile_time_test();
        cx.facts.functions = functions::extract(root, src, path, &RUST_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut cx.facts.tests,
            &RUST_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &cx.facts.tests;
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
            cx.facts.swallowed = super::handlers::extract(root, src, &RUST_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut cx.facts.tests, &RUST_RETRIES);
        if super::functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut cx.facts.functions {
                f.is_test = true;
            }
        }
        super::method_checks::count(root, src, &mut cx.facts, &RUST_RECEIVER_CALLS);
        super::helper_loops::count(root, src, &mut cx.facts, &super::helper_loops::RUST);
        let counted: [(super::calls::Vocab, super::calls::Pick); 2] = [
            (super::calls::SLEEP_VOCAB, super::calls::sleeps),
            (
                super::calls::TRIVIAL_ASSERT_VOCAB,
                super::calls::trivial_asserts,
            ),
        ];
        for (counted_vocab, pick) in counted {
            super::calls::count_with(
                root,
                src,
                &mut cx.facts.tests,
                &RUST_MOCKS,
                counted_vocab,
                pick,
                // A macro's arguments are a token tree, not call nodes.
                &|node| {
                    (node.kind() == "macro_invocation")
                        .then(|| macro_code(node, src, counted_vocab, &vocab.extra_macros))
                },
            );
        }
        super::bounds::rust(root, src, &mut cx.facts.tests);
        super::expectations::rust(root, src, &mut cx.facts.tests);
        super::caught_assertions::rust(root, src, &mut cx.facts.tests);
        cx.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "line_comment",
                "block_comment",
                "string_literal",
                "raw_string_literal",
            ],
        );
        cx.facts.budgets = super::budgets::extract(root, src, &RS_BUDGETS);
        Ok(cx.facts)
    }
}

use super::HelperFacts;

struct Comment {
    start_row: usize,
    end_row: usize,
    start_byte: usize,
    end_byte: usize,
    has_safety: bool,
}

struct Extractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    lines: Vec<&'a str>,
    /// Byte offset of each line start (robust to CRLF, unlike summing `lines`).
    line_starts: Vec<usize>,
    vocab: &'a AssertVocabulary,
    comments: Vec<Comment>,
    in_fn: usize,
    in_const: usize,
    in_test: usize,
    /// The whole file is test code (a test directory or a declared test path).
    test_file: bool,
    /// Counting a library function's checks: one outside `#[cfg(test)]` in a non-test
    /// file. Its `unwrap` / `expect` is the library's own error handling, and its `?`
    /// the library's own error propagation, not a check a test that calls it makes (#392,
    /// #422); its assertions and panics still count.
    library_helper: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, HelperFacts>,
    test_calls: Vec<Vec<String>>,
    owning_features: Option<HashSet<String>>,
}

impl<'a> Extractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    /// A callee chosen at the call, `(if c { a } else { b })(x)`, named by every function
    /// it can be, joined by `|` (resolved in `ast::resolve_helper`); `None` for any other
    /// callee.
    fn selected_callee(&self, f: Node) -> Option<String> {
        if f.kind() != "parenthesized_expression" {
            return None;
        }
        let mut names = Vec::new();
        branch_names(f, self.src, &mut names)?;
        (!names.is_empty()).then(|| names.join("|"))
    }

    fn collect_comments(&mut self, node: Node) {
        if matches!(node.kind(), "line_comment" | "block_comment") {
            let text = self.text(node);
            // A line comment's end position sits at column 0 of the next row; that row is
            // not part of the comment, or a run walk pairs each row with the comment above.
            let end = node.end_position();
            let end_row = if end.column == 0 && end.row > node.start_position().row {
                end.row - 1
            } else {
                end.row
            };
            self.comments.push(Comment {
                start_row: node.start_position().row,
                end_row,
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                has_safety: is_safety_doc_section(text)
                    || has_valid_safety_comment_with_placeholders(
                        text,
                        &self.vocab.safety_placeholders,
                    ),
            });
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments(child);
        }
    }

    fn visit(&mut self, node: Node, mods: &mut Vec<String>) {
        match node.kind() {
            "attribute_item" | "inner_attribute_item" => {
                let text = self.text(node);
                let name = attribute_name(text);
                if name == "allow" || name == "expect" {
                    let rule = text
                        .split_once('(')
                        .map(|(_, r)| r.trim_end_matches(']').trim_end_matches(')').trim())
                        .unwrap_or("")
                        .to_string();
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
            "mod_item" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_default();
                mods.push(name);
                self.visit_children(node, mods);
                mods.pop();
                return;
            }
            "function_item" => {
                let mut direct_calls = Vec::new();
                let test_opt = self.test_fn(node, mods, &mut direct_calls);
                let is_test = test_opt.is_some();
                if let Some(test) = test_opt {
                    self.facts.tests.push(test);
                    self.test_calls.push(direct_calls);
                } else if let Some(name_node) = node.child_by_field_name("name") {
                    let fn_name = self.text(name_node).to_string();
                    let mut helper_test = TestFn::default();
                    let is_fallible_return = node
                        .child_by_field_name("return_type")
                        .map(|rt| {
                            let text = self.text(rt);
                            text.contains("Result") || text.contains("Option")
                        })
                        .unwrap_or(false);
                    let mut dummy_calls = Vec::new();
                    if let Some(body) = node.child_by_field_name("body") {
                        let src = std::str::from_utf8(self.src).unwrap_or("");
                        self.library_helper = !self.test_file
                            && !rust_fn_skip(node, src)
                            && !has_cfg_test_attribute(node, src);
                        self.count_asserts(
                            body,
                            &mut helper_test,
                            is_fallible_return,
                            &mut dummy_calls,
                        );
                        self.library_helper = false;
                        helper_test.total_asserts += super::count_failure_exits(
                            body,
                            self.src,
                            &["macro_invocation"],
                            RUST_FAILURE_EXITS,
                            &["function_item", "closure_expression"],
                        );
                    }
                    let facts = HelperFacts {
                        total_asserts: helper_test.total_asserts,
                        strong_asserts: helper_test.strong_asserts,
                        tautologies: helper_test.tautologies,
                        fatal_asserts: helper_test.fatal_asserts,
                        wraps: node.child_by_field_name("body").and_then(|b| {
                            super::forwarding_wrapper_callee(
                                b,
                                &RS_WRAPPER,
                                &RS_LOCALS,
                                &dummy_calls,
                                self.src,
                            )
                        }),
                    };
                    self.helpers.insert(fn_name.clone(), facts);
                    let line = node.start_position().row + 1;
                    let end_line = node.end_position().row + 1;
                    self.facts.push_helper(
                        super::TestHelperFacts {
                            name: fn_name,
                            line,
                            end_line,
                            total_asserts: helper_test.total_asserts,
                            strong_asserts: helper_test.strong_asserts,
                            tautologies: helper_test.tautologies,
                            fatal_asserts: helper_test.fatal_asserts,
                            helper_checks: 0,
                        },
                        dummy_calls,
                    );
                }
                self.in_fn += 1;
                if is_test {
                    self.in_test += 1;
                }
                self.visit_children(node, mods);
                if is_test {
                    self.in_test -= 1;
                }
                self.in_fn -= 1;
                return;
            }
            "const_item" => {
                self.in_const += 1;
                self.visit_children(node, mods);
                self.in_const -= 1;
                return;
            }
            "macro_invocation" => {
                if self.in_test == 0 {
                    if let Some(m) = node.child_by_field_name("macro") {
                        let full_name = self.text(m);
                        let short_name = last_segment(full_name);
                        if self.in_fn == 0
                            && (short_name == "proptest" || short_name == "quickcheck")
                        {
                            self.extract_property_tests(node, short_name, mods);
                        }
                        let is_cta = if self.in_const > 0 {
                            self.is_assert_macro(short_name)
                                || short_name.starts_with("const_assert")
                                || full_name.contains("static_assertions")
                        } else if self.in_fn == 0 {
                            short_name.starts_with("const_assert")
                                || full_name.contains("static_assertions")
                                || short_name.starts_with("assert_")
                        } else {
                            false
                        };
                        if is_cta {
                            self.facts.compile_time_asserts += 1;
                            if self.facts.compile_time_assert_line.is_none() {
                                self.facts.compile_time_assert_line =
                                    Some(node.start_position().row + 1);
                            }
                        }
                    }
                }
            }
            "unsafe_block" => self.unsafe_site(node, "unsafe block"),
            "impl_item" => {
                let mut cursor = node.walk();
                if node.children(&mut cursor).any(|c| c.kind() == "unsafe") {
                    self.unsafe_site(node, "unsafe impl");
                }
                self.visit_children(node, mods);
                return;
            }
            "trait_item" => {
                let mut cursor = node.walk();
                if node.children(&mut cursor).any(|c| c.kind() == "unsafe") {
                    self.unsafe_site(node, "unsafe trait");
                }
                self.visit_children(node, mods);
                return;
            }
            _ => {}
        }
        self.visit_children(node, mods);
    }

    fn visit_children(&mut self, node: Node, mods: &mut Vec<String>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, mods);
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

    fn test_fn(
        &self,
        node: Node,
        mods: &[String],
        direct_calls: &mut Vec<String>,
    ) -> Option<TestFn> {
        let mut is_test = node
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .is_some_and(|name| self.vocab.test_functions.iter().any(|f| f == name));
        let mut ignored = false;
        let mut conditional_ignore = None;
        let mut ci_verdict = None;
        let mut should_panic = None;
        let mut has_commented_out_test = false;
        let mut prev = node.prev_sibling();
        while let Some(p) = prev {
            match p.kind() {
                "attribute_item" => {
                    let text = self.text(p);
                    let name = attribute_name(text);
                    let attr_line = p.start_position().row + 1;
                    let mut check_attr = |n: &str| match n {
                        "test" | "rstest" | "test_case" | "quickcheck" => is_test = true,
                        "ignore" => ignored = true,
                        "should_panic" => {
                            should_panic =
                                Some(super::expected_exceptions::parse_rust_should_panic(
                                    text, attr_line,
                                ));
                        }
                        _ => {}
                    };
                    check_attr(&name);
                    if name == "cfg_attr" {
                        if let Some((cond, subs)) = parse_cfg_attr(text) {
                            for sub in subs {
                                let sub_name = attribute_name(&sub);
                                if matches!(
                                    sub_name.as_str(),
                                    "test" | "rstest" | "test_case" | "quickcheck"
                                ) {
                                    is_test = true;
                                } else if sub_name == "ignore" {
                                    if let Some(verdict) = self.apply_cfg_attr_ignore(
                                        p,
                                        &cond,
                                        &mut ignored,
                                        &mut conditional_ignore,
                                    ) {
                                        ci_verdict = Some(verdict);
                                    }
                                } else if sub_name == "should_panic" {
                                    should_panic =
                                        Some(super::expected_exceptions::parse_rust_should_panic(
                                            &sub, attr_line,
                                        ));
                                }
                            }
                        }
                    }
                    if name == "cfg" {
                        self.apply_cfg(p, &mut ignored, &mut conditional_ignore);
                    }
                    prev = p.prev_sibling();
                }
                "line_comment" | "block_comment" => {
                    let text = self.text(p);
                    if is_commented_out_test(text) {
                        has_commented_out_test = true;
                    }
                    prev = p.prev_sibling();
                }
                _ => break,
            }
        }
        let (mod_ign, mod_cond) = self.eval_parent_mod_cfgs(node);
        if mod_ign {
            ignored = true;
            conditional_ignore = None;
        } else if let Some(cond_str) = mod_cond {
            if !ignored && conditional_ignore.is_none() {
                conditional_ignore = Some(cond_str);
            }
        }
        if !is_test && has_commented_out_test {
            is_test = true;
            ignored = true;
        }
        if !is_test {
            return None;
        }

        let name = self.text(node.child_by_field_name("name")?);
        let qualified = mods
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect::<Vec<_>>()
            .join("::");

        let (cases, non_literal_cases) = super::test_cases::extract_rust_cases(node, self.src);

        let mut test = TestFn {
            name: qualified,
            line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored,
            ci_verdict: ci_verdict.filter(|_| conditional_ignore.is_some()),
            conditional_ignore,
            fatal_asserts: 0,
            should_panic: should_panic.clone(),
            mock_setups: 0,
            mock_asserts: 0,
            retries: None,
            sleeps: 0,
            trivial_asserts: 0,
            helper_checks: 0,
            bounds: Vec::new(),
            expectations: Vec::new(),
            caught_assertions: Vec::new(),
            expected_exceptions: should_panic.into_iter().collect(),
            cases,
            non_literal_cases,
            direct_calls: Vec::new(),
            tautology_spans: Vec::new(),
            method_checks: 0,
            counted_helper_calls: Vec::new(),
            helper_reach: super::HelperReach::default(),
        };
        let is_fallible_return = node
            .child_by_field_name("return_type")
            .map(|rt| {
                let text = self.text(rt);
                text.contains("Result") || text.contains("Option")
            })
            .unwrap_or(false);
        if let Some(body) = node.child_by_field_name("body") {
            self.count_asserts(body, &mut test, is_fallible_return, direct_calls);
            self.macro_argument_calls(body, &mut test, direct_calls);
            super::dispatch_calls(body, self.src, &RS_DISPATCH, direct_calls);
            if !test.ignored {
                self.record_conditional_early_exits(body, &mut test);
            }
        }
        Some(test)
    }

    /// Early exits under a condition: the first `if` the text rule below accepts, then
    /// every `return` under an `if` (nested and `else` branches included) that a CI
    /// variable is involved in, through a variable, constant or helper of this file.
    fn record_conditional_early_exits(&self, body: Node, test: &mut TestFn) {
        use super::ci_condition::{self, CiVerdict, Lang};
        if let Some((cond, consequence)) = self.detect_conditional_early_exit(body) {
            let verdict = ci_condition::site(Lang::Rust, consequence, self.src)
                .map_or(CiVerdict::NotCi, |s| s.verdict);
            test.record_conditional_skip(cond, verdict);
        }
        let is_return = |n: Node| {
            n.kind() == "return_expression"
                || (n.kind() == "expression_statement"
                    && n.named_child(0)
                        .is_some_and(|c| c.kind() == "return_expression"))
        };
        for exit in ci_condition::exits_under_if(body, &is_return) {
            if let Some(site) = ci_condition::site(Lang::Rust, exit, self.src) {
                if site.related {
                    test.record_conditional_skip(site.text, site.verdict);
                }
            }
        }
    }

    fn detect_conditional_early_exit<'t>(&self, body: Node<'t>) -> Option<(String, Node<'t>)> {
        let mut cursor = body.walk();
        let mut env_bindings = std::collections::HashSet::new();

        for child in body.children(&mut cursor) {
            let stmt = if child.kind() == "expression_statement" {
                child.child(0).unwrap_or(child)
            } else {
                child
            };

            if stmt.kind() == "let_declaration" {
                let text = self.text(stmt);
                if is_rust_env_check(text) {
                    if let Some(pat) = stmt.child_by_field_name("pattern") {
                        let name = self.text(pat).trim();
                        if !name.is_empty() {
                            env_bindings.insert(name.to_string());
                        }
                    }
                }
            }

            if stmt.kind() == "if_expression" {
                let cond_node = stmt.child_by_field_name("condition")?;
                let cond_text = self.text(cond_node).trim();

                let is_env_check = is_rust_env_check(cond_text)
                    || super::is_ci_condition(cond_text)
                    || env_bindings.iter().any(|v| {
                        cond_text == v
                            || cond_text
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .any(|t| t == v)
                    });

                if is_env_check {
                    let consequence = stmt.child_by_field_name("consequence")?;
                    if rust_block_returns_early(consequence) {
                        return Some((cond_text.to_string(), consequence));
                    }
                }
            }
        }
        None
    }

    /// A definitely false cfg is an unconditional ignore. An undecided cfg naming a Cargo
    /// feature is a conditional skip (the feature may be off in the run); any other cfg
    /// (`unix`, `target_feature`, ...) says nothing about whether the test runs.
    fn consume_cfg_result(
        val: super::runner_collection::CfgValue,
        cond_str: String,
        names_feature: bool,
        ignored: &mut bool,
        conditional_ignore: &mut Option<String>,
    ) {
        match val {
            super::runner_collection::CfgValue::False => {
                *ignored = true;
                *conditional_ignore = None;
            }
            super::runner_collection::CfgValue::Unknown
                if names_feature && !*ignored && conditional_ignore.is_none() =>
            {
                *conditional_ignore = Some(cond_str);
            }
            _ => {}
        }
    }

    fn apply_cfg(
        &self,
        attr_node: Node,
        ignored: &mut bool,
        conditional_ignore: &mut Option<String>,
    ) {
        let text = self.text(attr_node);
        // `not(test)` is left to the evaluator below, which reads where it stands in the
        // predicate: `any(not(test), unix)` still builds under `cargo test`.
        if is_cfg_ci_suppression(text) {
            *ignored = true;
            *conditional_ignore = None;
            return;
        }
        let (val, cond_str) = super::runner_collection::evaluate_rust_cfg(
            attr_node,
            self.src,
            self.owning_features.as_ref(),
        );
        let names_feature = super::runner_collection::cfg_mentions_feature(attr_node, self.src);
        Self::consume_cfg_result(val, cond_str, names_feature, ignored, conditional_ignore);
    }

    fn apply_cfg_attr_ignore(
        &self,
        attr_node: Node,
        fallback_cond_str: &str,
        ignored: &mut bool,
        conditional_ignore: &mut Option<String>,
    ) -> Option<super::ci_condition::CiVerdict> {
        let (val, desc) = super::runner_collection::evaluate_rust_cfg(
            attr_node,
            self.src,
            self.owning_features.as_ref(),
        );
        let cond_str = if desc.is_empty() {
            fallback_cond_str.to_string()
        } else {
            desc
        };
        match val {
            super::runner_collection::CfgValue::True => {
                *ignored = true;
                *conditional_ignore = None;
            }
            super::runner_collection::CfgValue::Unknown => {
                if !*ignored && conditional_ignore.is_none() {
                    *conditional_ignore = Some(cond_str);
                    // `cfg_attr(not(ci), ignore)` ignores the test outside CI only.
                    return super::runner_collection::rust_cfg_ci_verdict(attr_node, self.src);
                }
            }
            super::runner_collection::CfgValue::False => {}
        }
        None
    }

    fn eval_parent_mod_cfgs(&self, node: Node) -> (bool, Option<String>) {
        let mut ignored = false;
        let mut conditional_ignore = None;
        let mut cur = node.parent();
        while let Some(p) = cur {
            if p.kind() == "mod_item" {
                let mut prev_mod = p.prev_sibling();
                while let Some(a) = prev_mod {
                    // A comment between the attribute and the module changes nothing.
                    if matches!(a.kind(), "line_comment" | "block_comment") {
                        prev_mod = a.prev_sibling();
                        continue;
                    }
                    if a.kind() != "attribute_item" {
                        break;
                    }
                    let text = a.utf8_text(self.src).unwrap_or("");
                    let name = attribute_name(text);
                    if name == "cfg" {
                        self.apply_cfg(a, &mut ignored, &mut conditional_ignore);
                    }
                    prev_mod = a.prev_sibling();
                }
            }
            // `#![cfg(..)]` inside a module body, or at the top of the file, gates
            // everything in it.
            if matches!(p.kind(), "declaration_list" | "source_file") {
                let mut cursor = p.walk();
                let inner: Vec<Node> = p
                    .children(&mut cursor)
                    .filter(|c| c.kind() == "inner_attribute_item")
                    .collect();
                for a in inner {
                    if attribute_name(a.utf8_text(self.src).unwrap_or("")) == "cfg" {
                        self.apply_cfg(a, &mut ignored, &mut conditional_ignore);
                    }
                }
            }
            cur = p.parent();
        }
        (ignored, conditional_ignore)
    }

    fn extract_property_tests(&mut self, node: Node, macro_name: &str, mods: &[String]) {
        let Some(token_tree) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "token_tree")
        else {
            self.facts.notes.push(format!(
                "macro `{macro_name}` near line {}: missing token tree (not analysed)",
                node.start_position().row + 1
            ));
            return;
        };

        let mut cursor = token_tree.walk();
        let children: Vec<Node> = token_tree.children(&mut cursor).collect();
        let mut i = 0;
        let mut current_attrs: Vec<(Node, String, usize)> = Vec::new();
        let mut attr_start_line: Option<usize> = None;
        let mut found_any_fn = false;

        while i < children.len() {
            let child = children[i];
            let kind = child.kind();

            if matches!(kind, "line_comment" | "block_comment" | "{" | "}" | ";") {
                i += 1;
                continue;
            }

            // Inner attribute: # ! [ ... ]
            if kind == "#" && i + 1 < children.len() && children[i + 1].kind() == "!" {
                i += 2;
                if i < children.len() && children[i].kind() == "token_tree" {
                    i += 1;
                }
                continue;
            }

            // Outer attribute: # [ ... ]
            if kind == "#" && i + 1 < children.len() && children[i + 1].kind() == "token_tree" {
                let attr_line = child.start_position().row + 1;
                if attr_start_line.is_none() {
                    attr_start_line = Some(attr_line);
                }
                let attr_text = format!("#{}", self.text(children[i + 1]));
                current_attrs.push((children[i + 1], attr_text, attr_line));
                i += 2;
                continue;
            }

            // Skip visibility and async qualifiers: pub, pub(...), async
            if kind == "visibility_modifier" || kind == "async" {
                i += 1;
                continue;
            }
            if kind == "identifier" && self.text(child) == "pub" {
                i += 1;
                if i < children.len()
                    && children[i].kind() == "token_tree"
                    && self.text(children[i]).starts_with('(')
                {
                    i += 1;
                }
                continue;
            }

            if kind == "fn" {
                found_any_fn = true;
                let fn_line = attr_start_line
                    .take()
                    .unwrap_or_else(|| child.start_position().row + 1);
                i += 1;

                while i < children.len()
                    && matches!(children[i].kind(), "line_comment" | "block_comment")
                {
                    i += 1;
                }

                if i >= children.len() || children[i].kind() != "identifier" {
                    self.facts.notes.push(format!(
                        "macro `{macro_name}` near line {fn_line}: property test missing function name (not analysed)"
                    ));
                    current_attrs.clear();
                    attr_start_line = None;
                    continue;
                }

                let name_node = children[i];
                let fn_name = self.text(name_node).to_string();
                i += 1;

                while i < children.len()
                    && matches!(children[i].kind(), "line_comment" | "block_comment")
                {
                    i += 1;
                }

                if i >= children.len()
                    || children[i].kind() != "token_tree"
                    || !self.text(children[i]).starts_with('(')
                {
                    self.facts.notes.push(format!(
                        "macro `{macro_name}` near line {fn_line}: property test `{fn_name}` missing parameter list (not analysed)"
                    ));
                    current_attrs.clear();
                    attr_start_line = None;
                    continue;
                }
                i += 1;

                let mut return_type = String::new();
                while i < children.len() {
                    let k = children[i].kind();
                    if k == "token_tree" && self.text(children[i]).starts_with('{') {
                        break;
                    }
                    if !matches!(k, "line_comment" | "block_comment") {
                        return_type.push_str(self.text(children[i]));
                        return_type.push(' ');
                    }
                    i += 1;
                }

                if i >= children.len() {
                    self.facts.notes.push(format!(
                        "macro `{macro_name}` near line {fn_line}: property test `{fn_name}` missing body (not analysed)"
                    ));
                    current_attrs.clear();
                    attr_start_line = None;
                    continue;
                }

                let body_node = children[i];
                let body_text = self.text(body_node);
                let end_line = body_node.end_position().row + 1;
                i += 1;

                let test_name = mods
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(fn_name.as_str()))
                    .collect::<Vec<_>>()
                    .join("::");

                let mut ignored = false;
                let mut conditional_ignore = None;
                let mut ci_verdict = None;
                let mut should_panic = None;

                let (mod_ign, mod_cond) = self.eval_parent_mod_cfgs(node);
                if mod_ign {
                    ignored = true;
                } else if let Some(cond_str) = mod_cond {
                    conditional_ignore = Some(cond_str);
                }

                for (attr_node, attr_text, attr_line) in current_attrs.drain(..) {
                    let name = attribute_name(&attr_text);
                    if name == "ignore" {
                        ignored = true;
                        conditional_ignore = None;
                    } else if name == "should_panic" {
                        should_panic = Some(super::expected_exceptions::parse_rust_should_panic(
                            &attr_text, attr_line,
                        ));
                    } else if name == "cfg" {
                        self.apply_cfg(attr_node, &mut ignored, &mut conditional_ignore);
                    } else if name == "cfg_attr" {
                        if let Some((cond, subs)) = parse_cfg_attr(&attr_text) {
                            for sub in subs {
                                let sub_name = attribute_name(&sub);
                                if sub_name == "ignore" {
                                    if let Some(verdict) = self.apply_cfg_attr_ignore(
                                        attr_node,
                                        &cond,
                                        &mut ignored,
                                        &mut conditional_ignore,
                                    ) {
                                        ci_verdict = Some(verdict);
                                    }
                                } else if sub_name == "should_panic" {
                                    should_panic =
                                        Some(super::expected_exceptions::parse_rust_should_panic(
                                            &sub, attr_line,
                                        ));
                                }
                            }
                        }
                    }
                }
                attr_start_line = None;

                let mut test = TestFn {
                    name: test_name,
                    line: fn_line,
                    end_line,
                    ignored,
                    ci_verdict: ci_verdict.filter(|_| conditional_ignore.is_some()),
                    conditional_ignore,
                    should_panic: should_panic.clone(),
                    expected_exceptions: should_panic.into_iter().collect(),
                    ..Default::default()
                };

                let fake_fn = format!("fn __discipline_prop() {body_text}");
                let mut p = Parser::new();
                let mut direct_calls = Vec::new();

                if p.set_language(&tree_sitter_rust::LANGUAGE.into()).is_ok() {
                    if let Some(tree) = p.parse(&fake_fn, None) {
                        let root = tree.root_node();
                        // The macro's token tree accepts tokens that are not a function
                        // body. A body the grammar cannot read is reported like any other
                        // source that parsed with errors, not counted as far as it went.
                        if let (true, error_line, _) = super::collect_error_nodes_info(root) {
                            self.facts.has_parse_errors = true;
                            if self.facts.first_parse_error_line.is_none() {
                                self.facts.first_parse_error_line =
                                    Some(body_node.start_position().row + error_line.unwrap_or(1));
                            }
                        }
                        // Reachability of the body as written, in the offsets of the
                        // re-parsed text.
                        let dead = super::reach::dead_ranges(root, &fake_fn, &RS_REACH);
                        if let Some(fn_item) = root.child(0) {
                            if let Some(body) = fn_item.child_by_field_name("body") {
                                let is_qc = macro_name == "quickcheck"
                                    || return_type.contains("bool")
                                    || return_type.contains("TestResult");
                                self.count_property_body_asserts(
                                    body,
                                    fake_fn.as_bytes(),
                                    &dead,
                                    &mut test,
                                    &mut direct_calls,
                                    is_qc,
                                );
                            }
                        }
                    }
                }

                self.facts.tests.push(test);
                self.test_calls.push(direct_calls);
                continue;
            }

            i += 1;
        }

        if !found_any_fn {
            let has_meaningful_tokens = children
                .iter()
                .any(|c| !matches!(c.kind(), "{" | "}" | "line_comment" | "block_comment" | ";"));
            if has_meaningful_tokens {
                self.facts.notes.push(format!(
                    "macro `{macro_name}` near line {}: token tree does not contain valid property tests (not analysed)",
                    node.start_position().row + 1
                ));
            }
        }
    }

    fn count_property_body_asserts(
        &self,
        node: Node,
        src: &[u8],
        dead: &super::reach::DeadRanges,
        test: &mut TestFn,
        direct_calls: &mut Vec<String>,
        is_quickcheck: bool,
    ) {
        // An assertion no execution reaches (`if false { .. }`, after an early `return`)
        // checks nothing, in a property body as in an ordinary test.
        if super::reach::is_dead(dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "function_item" => {
                return;
            }
            "try_expression" => {
                test.total_asserts += 1;
            }
            "macro_invocation" => {
                if let Some(m) = node.child_by_field_name("macro") {
                    let macro_ident = last_segment(m.utf8_text(src).unwrap_or(""));
                    if self.is_assert_macro(macro_ident) {
                        test.total_asserts += 1;
                        if is_strong(macro_ident) {
                            test.strong_asserts += 1;
                        }
                        let args = node
                            .children(&mut node.walk())
                            .find(|c| c.kind() == "token_tree")
                            .and_then(|t| t.utf8_text(src).ok())
                            .unwrap_or("");
                        if is_tautology(macro_ident, args) {
                            test.tautologies += 1;
                        }
                    }
                    let mut cursor = node.walk();
                    for tree in node
                        .children(&mut cursor)
                        .filter(|c| c.kind() == "token_tree")
                    {
                        let mut tree_cursor = tree.walk();
                        let tokens: Vec<Node> = tree.children(&mut tree_cursor).collect();
                        for j in 0..tokens.len() {
                            if tokens[j].kind() == "identifier" {
                                if let Ok(callee_name) = tokens[j].utf8_text(src) {
                                    if j + 1 < tokens.len()
                                        && tokens[j + 1].kind() == "token_tree"
                                        && !NON_EVALUATING_MACROS.contains(&callee_name)
                                    {
                                        direct_calls.push(callee_name.to_string());
                                        if self
                                            .vocab
                                            .helper_fns
                                            .iter()
                                            .any(|h| super::helper_call_matches(callee_name, h))
                                        {
                                            test.total_asserts += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if f.kind() == "field_expression" {
                        if let Some(field) = f.child_by_field_name("field") {
                            let method = field.utf8_text(src).unwrap_or("");
                            if method == "unwrap" || method == "expect" {
                                test.total_asserts += 1;
                            }
                        }
                    }
                    let callee = f.utf8_text(src).unwrap_or("");
                    let callee_name = last_segment(callee).to_string();
                    if self.vocab.helper_fns.iter().any(|h| {
                        super::helper_call_matches(callee, h)
                            || super::helper_call_matches(&callee_name, h)
                    }) {
                        test.total_asserts += 1;
                    }
                    direct_calls.push(callee_name);
                }
            }
            _ => {}
        }

        if is_quickcheck && test.total_asserts == 0 {
            if node.kind() == "block" {
                let mut cursor = node.walk();
                let children: Vec<Node> = node.children(&mut cursor).collect();
                if let Some(last) = children.iter().rev().find(|c| c.is_named()) {
                    if last.kind() != "expression_statement"
                        && !matches!(last.kind(), "line_comment" | "block_comment")
                    {
                        self.eval_quickcheck_expr(*last, src, test);
                    }
                }
            } else if node.kind() == "return_expression" {
                if let Some(expr) = node.children(&mut node.walk()).find(|c| c.is_named()) {
                    self.eval_quickcheck_expr(expr, src, test);
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.count_property_body_asserts(child, src, dead, test, direct_calls, is_quickcheck);
        }
    }

    fn eval_quickcheck_expr(&self, expr: Node, src: &[u8], test: &mut TestFn) {
        match expr.kind() {
            "binary_expression" => {
                test.total_asserts += 1;
                let mut cursor = expr.walk();
                for child in expr.children(&mut cursor) {
                    if child.kind() == "==" || child.kind() == "!=" {
                        test.strong_asserts += 1;
                        break;
                    }
                }
            }
            "boolean_literal" => {
                let text = expr.utf8_text(src).unwrap_or("");
                test.total_asserts += 1;
                if text == "true" {
                    test.tautologies += 1;
                }
            }
            "call_expression" | "unary_expression" => {
                test.total_asserts += 1;
            }
            _ => {}
        }
    }

    /// Records where the tautologies counted under `node` are (`TestFn::mark_tautologies`).
    fn count_asserts(
        &self,
        node: Node,
        test: &mut TestFn,
        is_fallible_return: bool,
        direct_calls: &mut Vec<String>,
    ) {
        let mark = test.tautology_mark();
        self.count_asserts_unmarked(node, test, is_fallible_return, direct_calls);
        test.mark_tautologies(mark, node);
    }

    fn count_asserts_unmarked(
        &self,
        node: Node,
        test: &mut TestFn,
        is_fallible_return: bool,
        direct_calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "function_item" => {
                // Do not recurse into nested function items.
                return;
            }
            "try_expression" => {
                // A library function's `?` hands its error to the caller, as its `expect`
                // is its own handling (#392): the test checks that error, not the `?`.
                if is_fallible_return && !self.library_helper {
                    test.total_asserts += 1;
                }
            }
            "macro_invocation" => {
                if let Some(m) = node.child_by_field_name("macro") {
                    let name = last_segment(self.text(m));
                    if self.is_assert_macro(name) {
                        test.total_asserts += 1;
                        if is_strong(name) {
                            test.strong_asserts += 1;
                        }
                        let args = node
                            .children(&mut node.walk())
                            .find(|c| c.kind() == "token_tree")
                            .map(|t| self.text(t))
                            .unwrap_or("");
                        if is_tautology(name, args) {
                            test.tautologies += 1;
                        }
                    }
                }
            }
            "call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if f.kind() == "field_expression" {
                        if let Some(field) = f.child_by_field_name("field") {
                            let method = self.text(field);
                            if (method == "unwrap" || method == "expect") && !self.library_helper {
                                test.total_asserts += 1;
                            }
                        }
                    }
                    let full_call = self.text(f);
                    let name = self
                        .selected_callee(f)
                        .unwrap_or_else(|| last_segment(full_call).to_string());
                    if self.vocab.helper_fns.iter().any(|h| {
                        super::helper_call_matches(full_call, h)
                            || super::helper_call_matches(&name, h)
                    }) {
                        test.total_asserts += 1;
                    }
                    direct_calls.push(name);
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.count_asserts(child, test, is_fallible_return, direct_calls);
        }
    }

    /// Calls a test makes inside macro arguments (`assert!(run(1) > 0)`), which the
    /// grammar keeps as a flat token tree: a name followed by a parenthesised tree, not
    /// after a `.` (a method call, not resolved outside macros either). They are added
    /// to `direct_calls` as calls outside a macro are. A macro in `NON_EVALUATING_MACROS`
    /// runs none of its arguments, so its tree is not read. Only test bodies are read:
    /// a helper's collected calls must match its call nodes (`forwarding_wrapper_callee`).
    fn macro_argument_calls(&self, node: Node, test: &mut TestFn, direct_calls: &mut Vec<String>) {
        if super::reach::is_dead(&self.dead, node.start_byte()) || node.kind() == "function_item" {
            return;
        }
        if node.kind() == "macro_invocation" {
            let evaluates = node
                .child_by_field_name("macro")
                .is_some_and(|m| !NON_EVALUATING_MACROS.contains(&last_segment(self.text(m))));
            if evaluates {
                let mut cursor = node.walk();
                for tree in node
                    .children(&mut cursor)
                    .filter(|c| c.kind() == "token_tree")
                {
                    self.token_tree_calls(tree, test, direct_calls);
                }
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.macro_argument_calls(child, test, direct_calls);
        }
    }

    fn token_tree_calls(&self, tree: Node, test: &mut TestFn, direct_calls: &mut Vec<String>) {
        let mut cursor = tree.walk();
        let tokens: Vec<Node> = tree.children(&mut cursor).collect();
        let mut skip = None;
        for (i, token) in tokens.iter().enumerate() {
            let next = tokens.get(i + 1);
            if token.kind() == "identifier" {
                let name = self.text(*token);
                let after_dot = i > 0 && tokens[i - 1].kind() == ".";
                if next.is_some_and(|n| n.kind() == "!") {
                    if NON_EVALUATING_MACROS.contains(&name) {
                        skip = Some(i + 2);
                    }
                } else if !after_dot
                    && next.is_some_and(|n| {
                        n.kind() == "token_tree" && n.child(0).is_some_and(|c| c.kind() == "(")
                    })
                {
                    direct_calls.push(name.to_string());
                    if self
                        .vocab
                        .helper_fns
                        .iter()
                        .any(|h| super::helper_call_matches(name, h))
                    {
                        test.total_asserts += 1;
                    }
                }
            }
            if token.kind() == "token_tree" && skip != Some(i) {
                self.token_tree_calls(*token, test, direct_calls);
            }
        }
    }

    fn is_assert_macro(&self, name: &str) -> bool {
        is_assert_macro_name(name, &self.vocab.extra_macros)
    }

    fn unsafe_site(&mut self, node: Node, kind: &'static str) {
        let documented = self.is_documented(node);
        let row = node.start_position().row;
        let snippet = self
            .lines
            .get(row)
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        let site = UnsafeSite {
            line: row + 1,
            kind,
            documented,
            snippet: snippet.clone(),
        };
        self.facts.unsafe_sites.push(site);
        self.facts
            .escape_hatches
            .push(EscapeHatchSite::UnsafeBlock {
                line: row + 1,
                kind,
                documented,
                snippet,
            });
    }

    /// A site is documented when a `SAFETY:` comment sits in the contiguous
    /// comment / attribute run directly above the unsafe node or above any
    /// ancestor up to its enclosing statement, or inline between the start of
    /// that statement and the `unsafe` keyword.
    fn is_documented(&self, node: Node) -> bool {
        let mut rows = vec![node.start_position().row];
        let mut statement = node;
        while let Some(parent) = statement.parent() {
            if matches!(
                parent.kind(),
                "block" | "source_file" | "declaration_list" | "unsafe_block"
            ) {
                break;
            }
            statement = parent;
            rows.push(statement.start_position().row);
        }
        rows.dedup();

        if rows.iter().any(|&row| self.safety_run_above(row)) {
            return true;
        }
        self.comments.iter().any(|c| {
            c.has_safety
                && c.start_byte >= statement.start_byte()
                && c.end_byte <= node.start_byte()
        })
    }

    fn safety_run_above(&self, row: usize) -> bool {
        let mut row = row;
        let mut run_comments = Vec::new();
        while row > 0 {
            row -= 1;
            let line = self.lines.get(row).copied().unwrap_or("").trim_start();
            if let Some(c) = self.comment_on_row(row) {
                run_comments.push(c);
                row = c.start_row;
            } else if line.starts_with("#[") {
                continue;
            } else {
                break;
            }
        }
        if run_comments.is_empty() {
            return false;
        }
        if run_comments.iter().any(|c| c.has_safety) {
            return true;
        }
        run_comments.reverse();
        let mut combined = String::new();
        for c in run_comments {
            let start = c.start_byte;
            let end = c.end_byte;
            if end <= self.src.len() && start < end {
                if let Ok(text) = std::str::from_utf8(&self.src[start..end]) {
                    combined.push_str(text);
                    combined.push('\n');
                }
            }
        }
        has_valid_safety_comment_with_placeholders(&combined, &self.vocab.safety_placeholders)
    }

    /// A comment node that *begins the line* on `row` (or spans it), so a
    /// string literal that merely contains `// SAFETY:` does not qualify.
    fn comment_on_row(&self, row: usize) -> Option<&Comment> {
        let line = self.lines.get(row).copied().unwrap_or("");
        let indent = line.len() - line.trim_start().len();
        let line_start = self.line_starts.get(row).copied().unwrap_or(0);
        self.comments.iter().find(|c| {
            (c.start_row < row && c.end_row >= row)
                || (c.start_row == row && c.start_byte == line_start + indent)
        })
    }
}

#[allow(dead_code)]
pub(crate) fn has_valid_safety_comment(text: &str) -> bool {
    has_valid_safety_comment_with_placeholders(text, &[])
}

pub(crate) fn has_valid_safety_comment_with_placeholders(
    text: &str,
    custom_placeholders: &[String],
) -> bool {
    let default_list = crate::config::DEFAULT_SAFETY_PLACEHOLDERS;
    let placeholders: Vec<String> = if custom_placeholders.is_empty() {
        default_list.iter().map(|s| s.to_lowercase()).collect()
    } else {
        custom_placeholders
            .iter()
            .map(|s| s.to_lowercase())
            .collect()
    };

    let mut multi_word = Vec::new();
    let mut single_word = std::collections::HashSet::new();
    for p in &placeholders {
        let p = p.trim();
        if p.contains(' ') {
            multi_word.push(p.to_string());
        } else if !p.is_empty() {
            single_word.insert(p.to_string());
        }
    }

    let mut search_from = 0;
    while let Some(rel_idx) = text[search_from..].find("SAFETY:") {
        let idx = search_from + rel_idx + "SAFETY:".len();
        let after = &text[idx..];

        let mut comment_lines = Vec::new();
        for line in after.lines() {
            let trimmed = line
                .trim()
                .trim_start_matches('/')
                .trim_start_matches('*')
                .trim_end_matches('*')
                .trim_end_matches('/')
                .trim();
            if trimmed.is_empty() && !comment_lines.is_empty() {
                break;
            }
            comment_lines.push(trimmed);
        }
        let mut comment_text = comment_lines.join(" ").to_lowercase();

        for mw in &multi_word {
            comment_text = comment_text.replace(mw, " ");
        }

        let substantive_words = comment_text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_'))
            .filter(|w| !w.is_empty())
            .filter(|w| {
                let w_lower = w.to_lowercase();
                if single_word.contains(&w_lower) {
                    return false;
                }
                if w_lower == "unsafe" || w_lower == "safety" {
                    return false;
                }
                true
            })
            .count();

        if substantive_words > 0 {
            return true;
        }
        search_from = idx;
    }
    false
}

fn is_cfg_test_suppression(attr_text: &str) -> bool {
    let normalized: String = attr_text.chars().filter(|c| !c.is_whitespace()).collect();
    normalized.to_lowercase().contains("not(test)") || is_cfg_ci_suppression(attr_text)
}

/// A cfg whose text names a CI flag as the condition under which the item is left out.
fn is_cfg_ci_suppression(attr_text: &str) -> bool {
    let normalized: String = attr_text.chars().filter(|c| !c.is_whitespace()).collect();
    let lower = normalized.to_lowercase();
    lower.contains("not(ci)")
        || lower.contains("not(any(ci")
        || lower.contains("not(all(ci")
        || lower.contains("skip_ci")
        || lower.contains("ci_skip")
}

fn is_commented_out_test(comment_text: &str) -> bool {
    for line in comment_text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches('/')
            .trim_start_matches('*')
            .trim_end_matches('*')
            .trim_end_matches('/')
            .trim();
        if trimmed.starts_with("#[") || trimmed.starts_with("#![") {
            let attr = attribute_name(trimmed);
            if matches!(
                attr.as_str(),
                "test" | "rstest" | "test_case" | "quickcheck"
            ) {
                return true;
            }
        }
    }
    false
}

fn parse_cfg_attr(attr_text: &str) -> Option<(String, Vec<String>)> {
    let start = attr_text.find("cfg_attr")?;
    let rest = &attr_text[start + "cfg_attr".len()..];
    let open_paren = rest.find('(')?;
    let inside = &rest[open_paren + 1..];

    // Find first comma at paren depth 0 (relative to inside)
    let mut depth = 0;
    let mut condition_end = None;
    for (i, c) in inside.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                condition_end = Some(i);
                break;
            }
            _ => {}
        }
    }

    let comma_pos = condition_end?;
    let condition = inside[..comma_pos].trim().to_string();

    let sub_attrs_text = &inside[comma_pos + 1..];
    let mut sub_attrs = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in sub_attrs_text.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' if depth == 0 => {
                break;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    sub_attrs.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        sub_attrs.push(trimmed.to_string());
    }

    Some((condition, sub_attrs))
}

fn attribute_name(attr_text: &str) -> String {
    // `#[tokio::test(flavor = "multi_thread")]` -> `test`
    let inner = attr_text
        .trim_start_matches('#')
        .trim_start_matches('!')
        .trim_start_matches('[')
        .trim();
    let path: String = inner
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | ':'))
        .collect();
    last_segment(&path).to_string()
}

fn is_rust_env_check(text: &str) -> bool {
    text.contains("env::var")
        || text.contains("std::env::var")
        || text.contains("option_env!")
        || text.contains("env::var_os")
        || text.contains("std::env::var_os")
        || text.contains("var_os")
        || super::is_ci_condition(text)
}

fn rust_block_returns_early(consequence: Node) -> bool {
    let mut cursor = consequence.walk();
    for child in consequence.children(&mut cursor) {
        let node = if child.kind() == "expression_statement" {
            child.child(0).unwrap_or(child)
        } else {
            child
        };
        if node.kind() == "return_expression" {
            return true;
        }
    }
    false
}

/// Macros that run none of their arguments: they read them as tokens or names.
const NON_EVALUATING_MACROS: &[&str] = &[
    "stringify",
    "concat",
    "concat_idents",
    "env",
    "option_env",
    "cfg",
    "include",
    "include_str",
    "include_bytes",
    "compile_error",
    "module_path",
];

/// Every name a branch of a callee chosen at the call can take: `(if c { a } else { b })`
/// or `(match m { .. => a, .. => b })`, nested and parenthesised alike. `None` when a
/// branch is anything but a function name, or an `if` has no `else`.
fn branch_names(n: Node, src: &[u8], out: &mut Vec<String>) -> Option<()> {
    match n.kind() {
        "parenthesized_expression" => branch_names(n.named_child(0)?, src, out),
        "if_expression" => {
            branch_names(n.child_by_field_name("consequence")?, src, out)?;
            let otherwise = n.child_by_field_name("alternative")?.named_child(0)?;
            branch_names(otherwise, src, out)
        }
        "match_expression" => {
            let body = n.child_by_field_name("body")?;
            let mut cursor = body.walk();
            let arms: Vec<Node> = body
                .named_children(&mut cursor)
                .filter(|a| a.kind() == "match_arm")
                .collect();
            if arms.is_empty() {
                return None;
            }
            for arm in arms {
                branch_names(arm.child_by_field_name("value")?, src, out)?;
            }
            Some(())
        }
        "block" => {
            let mut cursor = n.walk();
            let parts: Vec<Node> = n
                .named_children(&mut cursor)
                .filter(|c| !matches!(c.kind(), "line_comment" | "block_comment"))
                .collect();
            let [only] = parts.as_slice() else {
                return None;
            };
            branch_names(*only, src, out)
        }
        "identifier" | "scoped_identifier" | "generic_function" => {
            let name = last_segment(n.utf8_text(src).ok()?).to_string();
            if !out.contains(&name) {
                out.push(name);
            }
            Some(())
        }
        _ => None,
    }
}

fn last_segment(path: &str) -> &str {
    let path = without_type_arguments(path.trim());
    path.rsplit("::").next().unwrap_or(path).trim()
}

/// `path` without a trailing turbofish: `insert_mode::<false>` is `insert_mode`, so a
/// call with type arguments names the function it calls. A path whose `<` does not
/// follow `::` (a comparison, a malformed path) is kept as it is.
fn without_type_arguments(path: &str) -> &str {
    if !path.ends_with('>') {
        return path;
    }
    let mut depth = 0usize;
    for (i, b) in path.bytes().enumerate().rev() {
        match b {
            b'>' => depth += 1,
            b'<' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return path[..i].strip_suffix("::").unwrap_or(path);
                }
            }
            _ => {}
        }
    }
    path
}

fn is_strong(name: &str) -> bool {
    name.contains("_eq") || name.contains("_ne") || name.contains("matches")
}

fn is_tautology(name: &str, token_tree: &str) -> bool {
    let inner = token_tree
        .trim()
        .trim_start_matches(['(', '[', '{'])
        .trim_end_matches([')', ']', '}']);
    let args = split_top_level(inner);
    if name.contains("_eq") {
        if args.len() >= 2 && args[0].trim() == args[1].trim() && !args[0].trim().is_empty() {
            return true;
        }
        return args.len() >= 2 && is_constant_argument(args[0]) && is_constant_argument(args[1]);
    }
    if name.contains("_ne") || name.contains("matches") {
        return args.len() >= 2 && is_constant_argument(args[0]) && is_constant_argument(args[1]);
    }
    if let Some(first) = args.first() {
        return is_constant_argument(first);
    }
    false
}

fn is_constant_argument(arg: &str) -> bool {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed == "true" || trimmed == "false" {
        return true;
    }
    reparsed_expression(trimmed, |value_node, _| {
        let mut has_literal = false;
        let mut has_forbidden = false;
        check_constant_node(value_node, &mut has_literal, &mut has_forbidden);
        has_literal && !has_forbidden
    })
    .unwrap_or(false)
}

/// Reads one macro argument as an expression. A macro's arguments are a token tree, so
/// the argument's text is parsed again as the value of a `let`; `read` gets the value's
/// node and the text it was parsed from. `None` when the argument is not an expression
/// (a pattern, a format specification, tokens of the macro's own grammar).
fn reparsed_expression<R>(arg: &str, read: impl FnOnce(Node, &str) -> R) -> Option<R> {
    let code = format!("fn _discipline_check() {{ let _ = ({arg}); }}");
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(&code, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let body = root.child(0)?.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let let_node = body
        .children(&mut cursor)
        .find(|c| c.kind() == "let_declaration")?;
    let value_node = let_node.child_by_field_name("value")?;
    Some(read(value_node, &code))
}

fn is_assert_macro_name(name: &str, extra: &[String]) -> bool {
    name.starts_with("assert")
        || name.starts_with("debug_assert")
        || name.starts_with("prop_assert")
        || extra.iter().any(|m| m == name)
}

/// The code a call counter (`calls::count_with`) judges a macro invocation by.
///
/// A delay is counted wherever the macro's tokens spell one, so the whole invocation is
/// read, less its string literals and comments. A vocabulary that describes what an
/// assertion checks (`Vocab::assertions`) is matched against asserted conditions only:
/// the condition arguments of this macro when it is an assertion, or of the assertion
/// macros its tokens invoke (`select! { r = f => assert!(r.is_ok()) }`). The message
/// arguments of an assertion and the arguments of any other macro (`format!`, `vec!`,
/// `println!`) are not what a test asserts. A macro that runs none of its arguments
/// (`NON_EVALUATING_MACROS`) spells no code.
fn macro_code(node: Node, src: &str, vocab: super::calls::Vocab, extra: &[String]) -> String {
    let text = |n: Node| n.utf8_text(src.as_bytes()).unwrap_or("");
    let name = node
        .child_by_field_name("macro")
        .map(|m| last_segment(text(m)))
        .unwrap_or("");
    if NON_EVALUATING_MACROS.contains(&name) {
        return String::new();
    }
    if !vocab.assertions {
        return super::calls::code_text(node, src);
    }
    let mut out = String::new();
    let mut cursor = node.walk();
    for tree in node
        .children(&mut cursor)
        .filter(|c| c.kind() == "token_tree")
    {
        asserted_conditions(name, tree, src, extra, &mut out);
    }
    out
}

/// How many leading arguments of an assertion macro are what it asserts: the rest is the
/// failure message. A macro whose arguments are not known (`assert_matches!`, a
/// configured one) is read whole.
fn condition_arguments(name: &str) -> usize {
    let plain = name
        .strip_prefix("debug_")
        .or_else(|| name.strip_prefix("prop_"))
        .unwrap_or(name);
    match plain {
        "assert" => 1,
        "assert_eq" | "assert_ne" => 2,
        _ => usize::MAX,
    }
}

/// Appends to `out` the asserted conditions under the token tree `tree` of the macro
/// `name` (empty for a group of tokens that is no macro's arguments), one per line.
///
/// Arguments are the token runs between the commas that are direct children of the tree.
/// Each is read as an expression (`reparsed_expression`) and contributes its code
/// (`calls::code_text`: no string literal, no comment). An argument that is not an
/// expression contributes its tokens with the string and comment tokens blanked.
fn asserted_conditions(name: &str, tree: Node, src: &str, extra: &[String], out: &mut String) {
    let mut cursor = tree.walk();
    let tokens: Vec<Node> = tree.children(&mut cursor).collect();
    if is_assert_macro_name(name, extra) {
        let inner = tokens
            .get(1..tokens.len().saturating_sub(1))
            .unwrap_or_default();
        let blanked = super::calls::code_text(tree, src);
        for argument in inner
            .split(|t| t.kind() == ",")
            .filter(|a| !a.is_empty())
            .take(condition_arguments(name))
        {
            let (start, end) = (
                argument[0].start_byte(),
                argument[argument.len() - 1].end_byte(),
            );
            let code = src
                .get(start..end)
                .and_then(|arg| {
                    reparsed_expression(arg, |value, code| {
                        // The value is the argument in the parentheses it was parsed in.
                        let inner = value.named_child(0).unwrap_or(value);
                        super::calls::code_text(inner, code)
                    })
                })
                .or_else(|| {
                    blanked
                        .get(start - tree.start_byte()..end - tree.start_byte())
                        .map(str::to_string)
                })
                .unwrap_or_default();
            out.push_str(&code);
            out.push('\n');
        }
        return;
    }
    for (i, token) in tokens.iter().enumerate() {
        if token.kind() != "token_tree" {
            continue;
        }
        let invoked =
            (i >= 2 && tokens[i - 1].kind() == "!" && tokens[i - 2].kind() == "identifier")
                .then(|| tokens[i - 2].utf8_text(src.as_bytes()).unwrap_or(""));
        match invoked {
            Some(nested) if NON_EVALUATING_MACROS.contains(&nested) => {}
            Some(nested) => asserted_conditions(nested, *token, src, extra, out),
            None => asserted_conditions("", *token, src, extra, out),
        }
    }
}

fn check_constant_node(n: Node, has_literal: &mut bool, has_forbidden: &mut bool) {
    match n.kind() {
        "integer_literal" | "float_literal" | "boolean_literal" | "string_literal"
        | "char_literal" | "raw_string_literal" => {
            *has_literal = true;
        }
        "identifier"
        | "field_identifier"
        | "type_identifier"
        | "call_expression"
        | "field_expression"
        | "index_expression"
        | "macro_invocation"
        | "closure_expression"
        | "scoped_identifier"
        | "generic_type_with_arguments"
        | "ERROR" => {
            *has_forbidden = true;
            return;
        }
        _ => {}
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        check_constant_node(child, has_literal, has_forbidden);
        if *has_forbidden {
            return;
        }
    }
}

fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start, mut in_str) = (0i32, 0usize, false);
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' if i == 0 || bytes[i - 1] != b'\\' => in_str = !in_str,
            _ if in_str => {}
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// A rustdoc comment carrying a `# Safety` section: the convention for `unsafe trait` and
/// `unsafe fn` that clippy's `missing_safety_doc` checks.
fn is_safety_doc_section(comment: &str) -> bool {
    let c = comment.trim_start();
    (c.starts_with("///") || c.starts_with("//!") || c.starts_with("/**"))
        && c.lines().any(|l| {
            l.trim_start_matches(['/', '!', '*', ' '])
                .trim()
                .eq_ignore_ascii_case("# safety")
        })
}

/// A `#[test]`-like attribute precedes the function.
fn rust_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    if functions::test_path(path) {
        return true;
    }
    let mut prev = node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let name = attribute_name(p.utf8_text(src.as_bytes()).unwrap_or(""));
                if matches!(
                    name.as_str(),
                    "test" | "rstest" | "test_case" | "quickcheck" | "bench"
                ) {
                    return true;
                }
                prev = p.prev_sibling();
            }
            "line_comment" | "block_comment" => prev = p.prev_sibling(),
            _ => break,
        }
    }
    false
}

/// The item itself carries `#[cfg(test)]` (an attribute right above it).
fn has_cfg_test_attribute(node: tree_sitter::Node, src: &str) -> bool {
    let mut prev = node.prev_sibling();
    while let Some(a) = prev {
        if a.kind() != "attribute_item" {
            break;
        }
        let text = a.utf8_text(src.as_bytes()).unwrap_or("");
        if is_cfg_test_suppression(text) || text.replace(' ', "") == "#[cfg(test)]" {
            return true;
        }
        prev = a.prev_sibling();
    }
    false
}

/// A trait method with a default body is a real body; one without is not a `function_item`
/// with a `body` field, so nothing to skip here beyond `#[cfg(test)]` modules.
fn rust_fn_skip(node: tree_sitter::Node, src: &str) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "mod_item" {
            let mut prev = p.prev_sibling();
            while let Some(a) = prev {
                if a.kind() != "attribute_item" {
                    break;
                }
                if is_cfg_test_suppression(a.utf8_text(src.as_bytes()).unwrap_or(""))
                    || a.utf8_text(src.as_bytes()).unwrap_or("").replace(' ', "") == "#[cfg(test)]"
                {
                    return true;
                }
                prev = a.prev_sibling();
            }
        }
        cur = p.parent();
    }
    false
}

pub const RUST_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["function_item"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["line_comment", "block_comment"],
    skip: rust_fn_skip,
    is_test: rust_fn_is_test,
    classify: functions::classify_rust,
};

/// A method called on a receiver (`method_checks`).
pub const RUST_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[("call_expression", "function", "field_expression", "field")],
        direct: &[],
    };

pub const RUST_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression", "macro_invocation"],
    callee_fields: &["function", "macro"],
};

pub const RUST_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &[],
    arm_of: &[],
    body_fields: &[],
    ignored_kinds: &["line_comment", "block_comment"],
    trivial: &[],
    discard_kinds: &["let_declaration", "expression_statement"],
    discards: super::handlers::rust_discards,
    classify_discard: Some(super::handlers::rust_discard_class),
    call_value_kinds: &[
        "call_expression",
        "macro_invocation",
        "await_expression",
        "try_expression",
    ],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
    silence_node: None,
};

pub const RUST_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["attribute_item"],
};

pub const RS_BUDGETS: super::budgets::BudgetSpec = super::budgets::BudgetSpec {
    key_values: &[super::budgets::KeyValueShape {
        kind: "field_initializer",
        key_field: "field",
        value_field: "value",
    }],
    keys: &[
        ("cases", "proptest cases"),
        ("max_shrink_iters", "proptest max_shrink_iters"),
        ("tests", "quickcheck tests"),
        ("gen_size", "quickcheck gen_size"),
    ],
    call_kind: "call_expression",
    callee_field: "function",
    arguments_field: "arguments",
    methods: &[
        ("with_cases", "proptest cases"),
        ("tests", "quickcheck tests"),
        ("gen_size", "quickcheck gen_size"),
        ("max_shrink_iters", "proptest max_shrink_iters"),
    ],
    integer_kinds: &["integer_literal"],
    token_tree_kinds: &["token_tree"],
};

/// Macros that end a same-file helper on a failure path: a helper that panics on a
/// mismatch is a check, the way a Python helper that raises is.
const RUST_FAILURE_EXITS: &[&str] = &["panic!", "std::panic!", "core::panic!", "unreachable!"];

pub const RS_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_expression"],
    block_kinds: &["block"],
    ignored_kinds: &["line_comment", "block_comment"],
    terminators: &[
        "return",
        "panic!(",
        "unreachable!(",
        "todo!(",
        "unimplemented!(",
        "std::process::exit(",
        "process::exit(",
    ],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const RS_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["array_expression"],
    names: &["identifier"],
    references: &[],
};

/// A Rust helper whose body is one call: `{ install_with(agent, root, false) }`.
pub const RS_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &[
        "block",
        "expression_statement",
        "return_expression",
        "try_expression",
        "await_expression",
    ],
    calls: &["call_expression"],
    arguments: &[
        "arguments",
        "reference_expression",
        "array_expression",
        "tuple_expression",
    ],
    references: &[],
    plain: &["self", "unit_expression"],
    skip: &["line_comment", "block_comment", "mutable_specifier"],
};

/// A local a Rust wrapper computes and forwards: `let bin = locate(dir).unwrap();`.
pub const RS_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &[],
    binders: &["let_declaration"],
    pattern: &["pattern"],
    value: &["value"],
    names: &["identifier"],
    holders: &[],
    refused: &["alternative"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// What the counters read of a macro: `macro_code` of the first macro in `body`.
    fn read_macro(body: &str, vocab: crate::ast::calls::Vocab) -> String {
        let src = format!("fn t() {{ {body} }}");
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&src, None).unwrap();
        let mut found = None;
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "macro_invocation" {
                found = Some(n);
                break;
            }
            let mut cursor = n.walk();
            let children: Vec<Node> = n.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
        let extra = vec!["verify".to_string()];
        macro_code(found.expect("a macro"), &src, vocab, &extra)
    }

    #[test]
    fn an_assertion_vocabulary_reads_the_asserted_condition_of_a_macro() {
        let read = |body: &str| read_macro(body, crate::ast::calls::TRIVIAL_ASSERT_VOCAB);
        // The condition, re-parsed as an expression: its strings and comments are blank.
        assert_eq!(read("assert!(r.is_ok());"), "r.is_ok()\n");
        assert_eq!(
            read("assert!(out.contains(\"x.is_ok()\") /* y.is_some() */);"),
            "out.contains(           )\n"
        );
        // The failure message and its arguments are not what is asserted.
        assert_eq!(read("assert!(n == 3, \"ok={}\", r.is_ok());"), "n == 3\n");
        assert_eq!(
            read("assert_eq!(a.is_some(), b, \"{}\", c.is_ok());"),
            "a.is_some()\nb\n"
        );
        assert_eq!(
            read("debug_assert_ne!(a, b.is_ok(), \"m\");"),
            "a\nb.is_ok()\n"
        );
        // A macro whose arguments are not known is read whole; a configured one too.
        assert_eq!(
            read("assert_matches!(r.is_ok(), true);"),
            "r.is_ok()\ntrue\n"
        );
        assert_eq!(read("verify!(r.is_ok());"), "r.is_ok()\n");
        // An argument that is not an expression keeps its tokens, less the strings.
        assert_eq!(
            read("assert_matches!(r, Some(\"x.is_ok()\") | None);"),
            "r\nSome(           ) | None\n"
        );
        // Any other macro asserts nothing itself; an assertion among its tokens does.
        assert_eq!(read("println!(\"{}\", r.is_ok());"), "");
        assert_eq!(read("let s = format!(\"assert!(r.is_ok())\");"), "");
        assert_eq!(
            read("tokio::select! { r = f() => { assert!(r.is_ok(), \"m\") } }"),
            "r.is_ok()\n"
        );
        assert_eq!(read("wrap! { stringify!(assert!(r.is_ok())) }"), "");
    }

    #[test]
    fn a_delay_vocabulary_reads_the_code_tokens_of_a_macro() {
        let read = |body: &str| read_macro(body, crate::ast::calls::SLEEP_VOCAB);
        assert_eq!(
            read("select! { _ = sleep(d) => {} }"),
            "select! { _ = sleep(d) => {} }"
        );
        assert_eq!(
            read("format!(\"thread::sleep(d)\" /* sleep(1) */)"),
            "format!(                                 )"
        );
        assert_eq!(read("stringify!(thread::sleep(d))"), "");
    }

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"fn checked(x: u32, strict: bool) {
    if strict && x != 1 {
        panic!("x");
    }
}
fn noop(_x: u32, _n: u32) {}
fn via(x: u32) {
    checked(x, true)
}
fn hollow(x: u32) {
    noop(x, 1)
}
fn busy(x: u32) {
    checked(x, true);
    prepare(x)
}
fn ping(x: u32) {
    pong(x, 1)
}
fn pong(x: u32, _n: u32) {
    ping(x)
}
#[test]
fn direct() {
    checked(1, true);
}
#[test]
fn via_wrapper() {
    via(1);
}
#[test]
fn hollow_wrapper() {
    hollow(1);
}
#[test]
fn busy_helper() {
    busy(1);
}
#[test]
fn wrapper_cycle() {
    ping(1);
}
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&RustPack, "tests/wrap.rs", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
    }

    /// A wrapper that computes locals and forwards them, with its own parameters, to one
    /// same-file call keeps the credit of that call's checks (the `tests/test_lease.rs`
    /// refactor): its own `unwrap` counts once and `run_in`'s once, beside the test's
    /// `assert!`. A wrapper that does
    /// other work first, binds a local it never forwards, computes an argument in the
    /// call, or ends without the call is not a wrapper and keeps only its own check.
    #[test]
    fn a_wrapper_forwarding_its_parameters_and_locals_resolves_to_the_call() {
        let src = r##"fn run_in(dir: &Path, args: &[&str], bin: &Path) -> Output {
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir).args(args);
    cmd.output().unwrap()
}
fn forwards(dir: &Path, args: &[&str]) -> Output {
    let bin = Path::new(env!("BIN")).parent().unwrap().to_path_buf();
    run_in(dir, args, &bin)
}
fn chained(dir: &Path, args: &[&str]) -> Output {
    let base = Path::new(env!("BIN")).parent().unwrap().to_path_buf();
    let bin = base.join("x");
    run_in(dir, args, &bin)
}
fn busy(dir: &Path, args: &[&str]) -> Output {
    std::fs::create_dir_all(dir).ok();
    let bin = Path::new(env!("BIN")).parent().unwrap().to_path_buf();
    run_in(dir, args, &bin)
}
fn unforwarded(dir: &Path, args: &[&str]) -> Output {
    let bin = Path::new(env!("BIN")).parent().unwrap().to_path_buf();
    run_in(dir, args, dir)
}
fn computed(dir: &Path, args: &[&str]) -> Output {
    let bin = Path::new(env!("BIN")).parent().unwrap().to_path_buf();
    run_in(dir, args, &bin.join("x"))
}
fn dropped(dir: &Path, args: &[&str]) -> PathBuf {
    let bin = Path::new(env!("BIN")).parent().unwrap().to_path_buf();
    bin
}
#[test]
fn t_forwards() {
    let out = forwards(Path::new("."), &[]);
    assert!(out.status.success());
}
#[test]
fn t_chained() {
    let out = chained(Path::new("."), &[]);
    assert!(out.status.success());
}
#[test]
fn t_busy() {
    let out = busy(Path::new("."), &[]);
    assert!(out.status.success());
}
#[test]
fn t_unforwarded() {
    let out = unforwarded(Path::new("."), &[]);
    assert!(out.status.success());
}
#[test]
fn t_computed() {
    let out = computed(Path::new("."), &[]);
    assert!(out.status.success());
}
#[test]
fn t_dropped() {
    let bin = dropped(Path::new("."), &[]);
    assert!(bin.exists());
}
"##;
        let f = test_file_facts(src);
        let counts: Vec<(&str, usize, usize)> = f
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.total_asserts, t.helper_checks))
            .collect();
        assert_eq!(
            counts,
            vec![
                ("t_forwards", 3, 1),
                ("t_chained", 3, 1),
                ("t_busy", 2, 1),
                ("t_unforwarded", 2, 1),
                ("t_computed", 2, 1),
                ("t_dropped", 2, 1),
            ]
        );
    }

    /// A helper called with type arguments (`insert_mode::<false>(x)`) is the helper
    /// `insert_mode`: its checks count as a plain call's do, directly, through a path,
    /// with nested type arguments (`store::<Vec<u8>>`), and through a thin wrapper (the refactor that moved a body into a generic
    /// `*_mode` function and left the named function as a wrapper). A generic call to a
    /// function that is not a same-file helper (`Vec::<u32>::new()`) adds nothing.
    #[test]
    fn a_helper_called_with_type_arguments_counts_its_checks() {
        let src = r##"fn plain(x: u32) -> u32 {
    debug_assert!(x < 10);
    assert_eq!(x % 2, 0);
    x
}
fn insert_mode<const REPLACE: bool>(x: u32) -> u32 {
    debug_assert!(x < 10);
    assert_eq!(x % 2, 0);
    x
}
fn insert(x: u32) -> u32 {
    insert_mode::<false>(x)
}
fn store<T: Default>(x: u32) -> u32 {
    debug_assert!(x < 10);
    assert_eq!(x % 2, 0);
    x
}
#[test]
fn t_plain() {
    plain(2);
}
#[test]
fn t_generic() {
    insert_mode::<true>(2);
}
#[test]
fn t_path() {
    self::insert_mode::<true>(2);
}
#[test]
fn t_wrapper() {
    insert(2);
}
#[test]
fn t_nested() {
    store::<Vec<u8>>(2);
}
#[test]
fn t_not_a_helper() {
    let v = Vec::<u32>::new();
}
"##;
        let f = facts(src);
        let counts = |name: &str| {
            let t = f.tests.iter().find(|t| t.name == name).unwrap();
            (t.total_asserts, t.strong_asserts, t.helper_checks)
        };
        let plain = counts("t_plain");
        assert!(plain.0 > 0 && plain.2 == 1, "{plain:?}");
        for name in ["t_generic", "t_path", "t_wrapper", "t_nested"] {
            assert_eq!(counts(name), plain, "{name}");
        }
        assert_eq!(counts("t_not_a_helper"), (0, 0, 0));
    }

    /// A callee chosen at the call (`(if SHARED { a } else { b })(x)`, or a `match`) runs
    /// one of its choices: the checks every choice runs count, directly and through a
    /// forwarding wrapper (the expanse `map_insert_mode` shape). A weaker choice lowers
    /// the count to its own; a choice that is not a same-file helper leaves none.
    #[test]
    fn a_callee_chosen_at_the_call_counts_the_checks_every_choice_runs() {
        let src = r##"fn plain(x: u32, s: &mut u32) -> u32 {
    debug_assert!(x < 10);
    assert_eq!(x % 2, 0);
    *s += 1;
    x
}
fn shared(x: u32, s: &mut u32) -> u32 {
    debug_assert!(x < 10);
    assert_eq!(x % 2, 0);
    *s += 2;
    x
}
fn weak(x: u32, s: &mut u32) -> u32 {
    debug_assert!(x < 10);
    *s += 3;
    x
}
fn insert_mode<const SHARED: bool>(x: u32) -> u32 {
    let mut s = 0;
    (if SHARED { shared } else { plain })(x, &mut s)
}
fn insert(x: u32) -> u32 {
    insert_mode::<false>(x)
}
#[test]
fn t_plain() {
    let mut s = 0;
    plain(2, &mut s);
}
#[test]
fn t_if() {
    let mut s = 0;
    (if FAST { plain } else { shared })(2, &mut s);
}
#[test]
fn t_match() {
    let mut s = 0;
    (match MODE { Mode::A => plain, Mode::B => self::shared, _ => plain })(2, &mut s);
}
#[test]
fn t_wrapper() {
    insert(2);
}
#[test]
fn t_weaker() {
    let mut s = 0;
    (if FAST { plain } else { weak })(2, &mut s);
}
#[test]
fn t_weak_only() {
    let mut s = 0;
    weak(2, &mut s);
}
#[test]
fn t_not_all_helpers() {
    let mut s = 0;
    (if FAST { plain } else { other_crate::run })(2, &mut s);
}
"##;
        let f = facts(src);
        let counts = |name: &str| {
            let t = f.tests.iter().find(|t| t.name == name).unwrap();
            (t.total_asserts, t.strong_asserts, t.helper_checks)
        };
        let plain = counts("t_plain");
        assert!(plain.0 > 0 && plain.2 == 1, "{plain:?}");
        for name in ["t_if", "t_match", "t_wrapper"] {
            assert_eq!(counts(name), plain, "{name}");
        }
        assert_eq!(counts("t_weaker"), counts("t_weak_only"));
        assert!(counts("t_weaker").0 < plain.0);
        assert_eq!(counts("t_not_all_helpers"), (0, 0, 0));
    }

    /// A same-file helper called inside a macro's arguments (`assert!(run(1) > 0)`) runs
    /// as surely as one called outside, so its checks count: through a path, a nested
    /// macro, and a non-assert macro alike. A macro that does not evaluate its arguments
    /// (`stringify!`), a method call, and a helper-free call add nothing.
    #[test]
    fn a_helper_called_inside_a_macro_counts_its_checks() {
        let src = r##"fn run(x: u32) -> u32 {
    x.checked_add(1).unwrap()
}
#[test]
fn t_assert() {
    assert!(run(1) > 0);
}
#[test]
fn t_path() {
    assert_eq!(self::run(1), 2);
}
#[test]
fn t_nested() {
    assert!(vec![run(1)].len() == 1);
}
#[test]
fn t_other_macro() {
    let s = format!("{}", run(1));
    assert!(!s.is_empty());
}
#[test]
fn t_stringify() {
    assert!(!stringify!(run(1)).is_empty());
}
#[test]
fn t_method() {
    let r = Runner;
    assert!(r.run(1) > 0);
}
#[test]
fn t_other_call() {
    assert!(other(1) > 0);
}
"##;
        let f = test_file_facts(src);
        let counts: Vec<(&str, usize, usize)> = f
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.total_asserts, t.helper_checks))
            .collect();
        assert_eq!(
            counts,
            vec![
                ("t_assert", 2, 1),
                ("t_path", 2, 1),
                ("t_nested", 2, 1),
                ("t_other_macro", 2, 1),
                ("t_stringify", 1, 0),
                ("t_method", 1, 0),
                ("t_other_call", 1, 0),
            ]
        );
    }

    fn facts(src: &str) -> ParsedFileFacts {
        RustPack
            .extract("test.rs", src, &AssertVocabulary::default())
            .expect("analyze")
    }

    #[test]
    fn a_library_functions_expect_is_not_a_check_of_the_test_that_calls_it() {
        // #392: `parse` is library code, so its `expect` is its own error handling; the
        // same `expect` in a test helper is a check.
        let src = r##"pub fn parse(s: &str) -> u32 {
    s.parse().expect("digits")
}
#[cfg(test)]
fn marked(s: &str) -> u32 {
    s.parse().expect("digits")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn helper(s: &str) -> u32 {
        s.parse().expect("digits")
    }
    #[test]
    fn t_library() {
        assert_eq!(parse("7"), 7);
    }
    #[test]
    fn t_test_helper() {
        assert_eq!(helper("7"), 7);
    }
    #[test]
    fn t_marked_helper() {
        assert_eq!(marked("7"), 7);
    }
}
"##;
        let f = facts(src);
        let counts: Vec<(&str, usize, usize)> = f
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.total_asserts, t.helper_checks))
            .collect();
        assert_eq!(
            counts,
            vec![
                ("tests::t_library", 1, 0),
                ("tests::t_test_helper", 2, 1),
                ("tests::t_marked_helper", 2, 1),
            ]
        );
        // In a test file every helper is test code, so its `expect` counts.
        let f = test_file_facts(src);
        assert_eq!(f.tests[0].total_asserts, 2, "{:?}", f.tests[0]);
    }

    #[test]
    fn a_library_functions_question_mark_is_not_a_check_of_the_test_that_calls_it() {
        // #422: `rules` is library code, so its `?`s hand the error to the caller; moving
        // them to another module must not lower `t_library`. Its `assert!` still counts. A
        // test helper's `?` and a fallible test's own `?` are checks.
        let src = r##"pub fn rules() -> Result<u32, std::num::ParseIntError> {
    let a: u32 = "1".parse()?;
    let b: u32 = "2".parse()?;
    assert!(a < b);
    Ok(a + b)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn helper() -> Result<u32, std::num::ParseIntError> {
        let a: u32 = "1".parse()?;
        Ok(a)
    }
    #[test]
    fn t_library() {
        assert_eq!(rules().unwrap(), 3);
    }
    #[test]
    fn t_test_helper() {
        assert_eq!(helper().unwrap(), 1);
    }
    #[test]
    fn t_fallible() -> Result<(), std::num::ParseIntError> {
        let n = rules()?;
        let m: u32 = "4".parse()?;
        assert_eq!(n + m, 7);
        Ok(())
    }
}
"##;
        let f = facts(src);
        let counts: Vec<(&str, usize, usize)> = f
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.total_asserts, t.helper_checks))
            .collect();
        assert_eq!(
            counts,
            vec![
                ("tests::t_library", 2, 1),
                ("tests::t_test_helper", 2, 1),
                ("tests::t_fallible", 4, 1),
            ]
        );
        // In a test file every helper is test code, so its `?` counts.
        let f = test_file_facts(src);
        assert_eq!(f.tests[0].total_asserts, 4, "{:?}", f.tests[0]);
    }

    /// `facts` for a file in a test directory, where top-level helpers are test code.
    fn test_file_facts(src: &str) -> ParsedFileFacts {
        RustPack
            .extract("tests/t.rs", src, &AssertVocabulary::default())
            .expect("analyze")
    }

    #[test]
    fn counts_assertions_per_test_and_ignores_comments_and_strings() {
        let f = facts(
            r#"
#[test]
fn real() {
    // assert_eq!(1, 2);
    let _s = "assert!(false)";
    let x = 1;
    assert_eq!(x + 1, 2);
    assert!(x > 0);
}
fn not_a_test() { assert!(true); }
"#,
        );
        assert_eq!(f.tests.len(), 1);
        assert_eq!(f.tests[0].name, "real");
        assert_eq!(f.tests[0].total_asserts, 2);
        assert_eq!(f.tests[0].strong_asserts, 1);
        assert!(!f.tests[0].is_vacuous());
    }

    #[test]
    fn detects_vacuous_and_tautological_tests() {
        let f = facts(
            r#"
mod tests {
    #[test] fn empty() {}
    #[test] fn tautology() { assert!(true); assert_eq!(1, 1); }
    #[test] #[should_panic(expected = "boom")] fn panics() { boom(); }
    #[tokio::test] async fn real() { assert_eq!(f().await, 3); }
}
"#,
        );
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();
        assert!(by_name("tests::empty").is_vacuous());
        assert!(by_name("tests::tautology").is_vacuous());
        assert_eq!(by_name("tests::tautology").tautologies, 2);
        assert!(!by_name("tests::panics").is_vacuous());
        assert!(!by_name("tests::real").is_vacuous());
    }

    #[test]
    fn helper_fns_and_extra_macros_count_only_when_configured() {
        let src = "#[test] fn t() { check_invariants(&x); verify!(x); }";
        assert!(facts(src).tests[0].is_vacuous());
        let vocab = AssertVocabulary {
            extra_macros: vec!["verify".into()],
            helper_fns: vec!["check_invariants".into()],
            ..Default::default()
        };
        let f = RustPack.extract("test.rs", src, &vocab).unwrap();
        assert_eq!(f.tests[0].total_asserts, 2);
    }

    #[test]
    fn proptest_and_quickcheck_property_tests_are_extracted() {
        let src = r#"
mod tests {
    use super::*;
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]
        #[test]
        fn commutes(a: u32, b: u32) {
            prop_assume!(a > 0);
            prop_assert_eq!(add(a, b), add(b, a));
            prop_assert!(add(a, b) >= a as u64);
        }

        #[ignore]
        fn ignored_prop(x in 0..10) {
            prop_assert!(x < 10);
        }

        fn vacuous_prop(x in 0..10) {}
    }

    quickcheck! {
        fn prop_reverse(xs: Vec<isize>) -> bool {
            xs == xs.into_iter().rev().rev().collect::<Vec<_>>()
        }

        fn prop_tautology(xs: Vec<isize>) -> bool {
            true
        }
    }

    prop_compose! {
        fn arb_point()(x in 0..10, y in 0..10) -> (i32, i32) {
            (x, y)
        }
    }
}
proptest! {
    fn top_level_prop(x in 0..10) {
        assert!(x < 20);
    }
}
proptest! {
    invalid tokens not a function
}
"#;
        let f = facts(src);
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();

        // 1. proptest commutes: prop_assume is not an assertion, prop_assert_eq and prop_assert are
        let commutes = by_name("tests::commutes");
        assert_eq!(commutes.total_asserts, 2);
        assert_eq!(commutes.strong_asserts, 1);
        assert_eq!(commutes.tautologies, 0);
        assert!(!commutes.ignored);
        assert!(!commutes.is_vacuous());

        // 2. proptest ignored
        let ignored_prop = by_name("tests::ignored_prop");
        assert!(ignored_prop.ignored);
        assert_eq!(ignored_prop.total_asserts, 1);

        // 3. proptest vacuous
        let vacuous_prop = by_name("tests::vacuous_prop");
        assert_eq!(vacuous_prop.total_asserts, 0);
        assert!(vacuous_prop.is_vacuous());

        // 4. quickcheck bool check
        let prop_reverse = by_name("tests::prop_reverse");
        assert_eq!(prop_reverse.total_asserts, 1);
        assert_eq!(prop_reverse.strong_asserts, 1);
        assert!(!prop_reverse.is_vacuous());

        // 5. quickcheck tautology
        let prop_tautology = by_name("tests::prop_tautology");
        assert_eq!(prop_tautology.total_asserts, 1);
        assert_eq!(prop_tautology.tautologies, 1);
        assert!(prop_tautology.is_vacuous());

        // 6. top-level proptest naming
        let top_level = by_name("top_level_prop");
        assert_eq!(top_level.total_asserts, 1);
        assert!(!top_level.is_vacuous());

        // 7. prop_compose is not extracted as a test
        assert!(f.tests.iter().all(|t| !t.name.contains("arb_point")));

        // 8. total tests count: exactly 6
        assert_eq!(f.tests.len(), 6);

        // 9. Malformed proptest token tree emits a note ("not analysed")
        assert!(f
            .notes
            .iter()
            .any(|n| n.contains("proptest") && n.contains("not analysed")));
    }

    #[test]
    fn ignore_attribute_is_detected() {
        let f = facts("#[test]\n#[ignore = \"flaky\"]\nfn t() { assert_eq!(a(), 1); }");
        assert!(f.tests[0].ignored);

        let f2 = facts("#[test]\n#[cfg_attr(all(), ignore)]\nfn t2() { assert_eq!(a(), 1); }");
        assert!(f2.tests[0].ignored);

        let f3 = facts(
            "#[cfg_attr(feature = \"ignore_something\", test)]\nfn t3() { assert_eq!(a(), 1); }",
        );
        assert!(!f3.tests[0].ignored);
        assert_eq!(f3.tests.len(), 1);

        let f4 = facts("// #[test]\nfn t4() { assert_eq!(a(), 1); }");
        assert_eq!(f4.tests.len(), 1);
        assert!(f4.tests[0].ignored);

        let f5 = facts("/* #[test] */\nfn t5() { assert_eq!(a(), 1); }");
        assert_eq!(f5.tests.len(), 1);
        assert!(f5.tests[0].ignored);

        let f6 = facts("#[test]\n#[cfg(not(ci))]\nfn t6() { assert_eq!(a(), 1); }");
        assert_eq!(f6.tests.len(), 1);
        assert!(f6.tests[0].ignored);

        let f7 = facts("#[test]\n#[cfg(not(test))]\nfn t7() { assert_eq!(a(), 1); }");
        assert_eq!(f7.tests.len(), 1);
        assert!(f7.tests[0].ignored);
    }

    #[test]
    fn safety_comment_above_block_or_statement_documents_it() {
        let f = facts(
            r#"
fn a(p: *const u8) -> u8 {
    // SAFETY: p is valid for reads.
    unsafe { *p }
}
fn b(p: *const u8) -> u8 {
    // SAFETY: p is valid for reads.
    let v = unsafe { *p };
    v
}
fn c(p: *const u8) -> u8 {
    let v = /* SAFETY: pointer is valid for reads */ unsafe { *p };
    v
}
// SAFETY: T is safe to send across threads.
unsafe impl Send for X {}
"#,
        );
        assert_eq!(f.unsafe_sites.len(), 4);
        assert!(
            f.unsafe_sites.iter().all(|s| s.documented),
            "{:?}",
            f.unsafe_sites
        );
        assert_eq!(f.escape_hatches.len(), 4);
    }

    #[test]
    fn undocumented_unsafe_is_flagged_even_without_a_space_before_the_brace() {
        let f = facts(
            r#"
fn a(p: *const u8) -> u8 { let v = unsafe{ *p }; v }
fn b(p: *const u8) -> u8 {
    // Safety: lower-case label does not count.
    unsafe { *p }
}
fn c(p: *const u8) -> u8 {
    let _doc = "// SAFETY: a string is not a comment";
    unsafe { *p }
}
fn d(p: *const u8) -> u8 {
    unsafe { *p } // SAFETY: trailing comment is after the fact
}
fn e(p: *const u8) -> u8 {
    // SAFETY: ok
    unsafe { *p }
}
fn f(p: *const u8) -> u8 {
    // SAFETY: valid
    unsafe { *p }
}
fn g(p: *const u8) -> u8 {
    // SAFETY: this is totally fine ok
    unsafe { *p }
}
unsafe impl Sync for X {}
"#,
        );
        assert_eq!(f.unsafe_sites.len(), 8);
        assert!(
            f.unsafe_sites.iter().all(|s| !s.documented),
            "{:?}",
            f.unsafe_sites
        );
        assert_eq!(f.escape_hatches.len(), 8);
    }

    #[test]
    fn the_word_unsafe_in_comments_and_strings_is_not_a_site() {
        let f = facts("// unsafe { }\nfn a() { let _ = \"unsafe { x }\"; }");
        assert!(f.unsafe_sites.is_empty());
        assert!(f.escape_hatches.is_empty());
    }

    #[test]
    fn parse_errors_are_surfaced() {
        assert!(facts("fn broken( {").has_parse_errors);
        assert!(!facts("fn fine() {}").has_parse_errors);
    }

    #[test]
    fn constant_expression_tautologies_are_flagged() {
        let f = facts(
            r#"
mod tests {
    #[test] fn t_const_eq() { assert_eq!(1, 1); }
    #[test] fn t_const_math() { assert!(1 + 1 > 0); }
    #[test] fn t_const_binary() { assert!(1 == 1); }
    #[test] fn t_const_ne() { assert_ne!(1, 2); }
    #[test] fn t_const_strings() { assert_ne!("a", "b"); }
    #[test] fn t_const_with_msg() { assert!(1 == 1, "failed with: {}", x); }
    #[test] fn t_ident_eq() { assert_eq!(x, x); }

    // NOT tautologies:
    #[test] fn real_call() { assert!(f() == 1); }
    #[test] fn real_ident() { assert_eq!(N, 4); }
    #[test] fn real_method() { assert!(x.len() > 0); }
    #[test] fn real_ne_ident() { assert_ne!(x, y); }
}
"#,
        );
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();
        assert!(
            by_name("tests::t_const_eq").is_vacuous(),
            "t_const_eq should be vacuous"
        );
        assert!(
            by_name("tests::t_const_math").is_vacuous(),
            "t_const_math should be vacuous"
        );
        assert!(
            by_name("tests::t_const_binary").is_vacuous(),
            "t_const_binary should be vacuous"
        );
        assert!(
            by_name("tests::t_const_ne").is_vacuous(),
            "t_const_ne should be vacuous"
        );
        assert!(
            by_name("tests::t_const_strings").is_vacuous(),
            "t_const_strings should be vacuous"
        );
        assert!(
            by_name("tests::t_const_with_msg").is_vacuous(),
            "t_const_with_msg should be vacuous"
        );
        assert!(
            by_name("tests::t_ident_eq").is_vacuous(),
            "t_ident_eq should be vacuous"
        );

        assert!(
            !by_name("tests::real_call").is_vacuous(),
            "real_call should not be vacuous"
        );
        assert!(
            !by_name("tests::real_ident").is_vacuous(),
            "real_ident should not be vacuous"
        );
        assert!(
            !by_name("tests::real_method").is_vacuous(),
            "real_method should not be vacuous"
        );
        assert!(
            !by_name("tests::real_ne_ident").is_vacuous(),
            "real_ne_ident should not be vacuous"
        );
    }

    #[test]
    fn idiomatic_result_option_and_unwrap_assertions() {
        let f = facts(
            r#"
mod tests {
    #[test]
    fn parses() -> Result<(), Box<dyn std::error::Error>> {
        let _n: i32 = "4".parse()?;
        Ok(())
    }

    #[test]
    fn options() -> Option<()> {
        let _v = map.get(&k)?;
        Some(())
    }

    #[test]
    fn result_no_assert_is_vacuous() -> Result<(), Box<dyn std::error::Error>> {
        let _n = 4;
        Ok(())
    }

    #[test]
    fn unwrap_counts_as_assertion() {
        let _n: i32 = "4".parse().unwrap();
    }

    #[test]
    fn expect_counts_as_assertion() {
        let _n: i32 = "4".parse().expect("valid int");
    }
}
"#,
        );
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();
        assert!(
            !by_name("tests::parses").is_vacuous(),
            "parses with ? should not be vacuous"
        );
        assert_eq!(by_name("tests::parses").total_asserts, 1);
        assert_eq!(by_name("tests::parses").strong_asserts, 0);

        assert!(
            !by_name("tests::options").is_vacuous(),
            "options with ? should not be vacuous"
        );
        assert_eq!(by_name("tests::options").total_asserts, 1);

        assert!(
            by_name("tests::result_no_assert_is_vacuous").is_vacuous(),
            "result without ? or assert must be vacuous"
        );
        assert_eq!(
            by_name("tests::result_no_assert_is_vacuous").total_asserts,
            0
        );

        assert!(
            !by_name("tests::unwrap_counts_as_assertion").is_vacuous(),
            "unwrap should count as assertion"
        );
        assert_eq!(
            by_name("tests::unwrap_counts_as_assertion").total_asserts,
            1
        );

        assert!(
            !by_name("tests::expect_counts_as_assertion").is_vacuous(),
            "expect should count as assertion"
        );
        assert_eq!(
            by_name("tests::expect_counts_as_assertion").total_asserts,
            1
        );
    }

    #[test]
    fn safety_comment_placeholders_and_substantive_discrimination() {
        // Substantive short comments pass regardless of word count
        assert!(has_valid_safety_comment(
            "// SAFETY: caller-checked non-null."
        ));
        assert!(has_valid_safety_comment(
            "// SAFETY: pointer is valid for reads"
        ));
        assert!(has_valid_safety_comment(
            "// SAFETY: index is bounded by length"
        ));

        // Hollow padding and placeholders fail
        assert!(!has_valid_safety_comment(
            "// SAFETY: this is totally fine ok"
        ));
        assert!(!has_valid_safety_comment("// SAFETY: todo"));
        assert!(!has_valid_safety_comment("// SAFETY: tbd"));
        assert!(!has_valid_safety_comment("// SAFETY: n/a"));
        assert!(!has_valid_safety_comment("// SAFETY: safe"));
        assert!(!has_valid_safety_comment("// SAFETY: ok"));
        assert!(!has_valid_safety_comment("// SAFETY: fine"));
        assert!(!has_valid_safety_comment("// SAFETY: trust me"));
        assert!(!has_valid_safety_comment("// SAFETY: safety"));
        assert!(!has_valid_safety_comment("// SAFETY: unsafe"));

        // Custom configurable placeholders
        let custom = vec!["custom_filler".to_string()];
        assert!(!has_valid_safety_comment_with_placeholders(
            "// SAFETY: custom_filler",
            &custom
        ));
        assert!(has_valid_safety_comment_with_placeholders(
            "// SAFETY: custom_filler with non_null pointer",
            &custom
        ));
    }

    #[test]
    fn compile_time_assertions_outside_tests_are_extracted() {
        let src = r#"
struct MyStruct {
    a: u64,
    b: u64,
}

const _: () = assert!(std::mem::size_of::<MyStruct>() == 16);
const _: () = {
    assert!(std::mem::align_of::<MyStruct>() == 8);
    assert_eq!(std::mem::size_of::<u64>(), 8);
};

static_assertions::assert_eq_size!(MyStruct, [u8; 16]);
const_assert!(std::mem::size_of::<MyStruct>() > 0);

fn runtime_fn(x: i32) {
    assert!(x > 0);
}

#[test]
fn normal_test() {
    assert_eq!(1, 1);
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("src/lib.rs", src, &AssertVocabulary::default())
            .unwrap();

        assert_eq!(facts.compile_time_asserts, 5);
        assert_eq!(facts.compile_time_assert_line, Some(7));
        assert!(facts.compile_time_test.is_some());
        let ctt = facts.compile_time_test.unwrap();
        assert_eq!(ctt.name, "compile-time-assertions");
        assert_eq!(ctt.total_asserts, 5);
        assert_eq!(ctt.strong_asserts, 5);
        assert_eq!(facts.tests.len(), 1);
    }

    #[test]
    fn cfg_attr_miri_ignore_is_conditional_not_unconditional() {
        let src = r#"
#[test]
#[cfg_attr(miri, ignore)]
fn test_miri_skipped() {
    assert_eq!(1, 1);
}

#[test]
#[cfg_attr(all(), ignore)]
fn test_unconditional_skipped() {
    assert_eq!(1, 1);
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("tests/cfg_attr.rs", src, &AssertVocabulary::default())
            .unwrap();

        let miri_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_miri_skipped")
            .unwrap();
        assert!(
            !miri_test.ignored,
            "miri test must NOT be unconditionally ignored"
        );
        assert_eq!(miri_test.conditional_ignore.as_deref(), Some("miri"));

        let unspec_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_unconditional_skipped")
            .unwrap();
        assert!(
            unspec_test.ignored,
            "all() condition must be unconditionally ignored"
        );
    }

    #[test]
    fn same_file_helper_functions_are_resolved_for_tests() {
        let src = r#"
fn assert_roundtrip(x: i32) {
    assert_eq!(x, x);
    assert_ne!(x, x + 1);
}

#[test]
fn test_via_helper() {
    assert_roundtrip(42);
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("tests/helper.rs", src, &AssertVocabulary::default())
            .unwrap();

        let test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_via_helper")
            .unwrap();
        assert!(
            !test.is_vacuous(),
            "test calling helper must not be vacuous"
        );
        assert_eq!(test.total_asserts, 2);
        assert_eq!(test.strong_asserts, 2);
    }

    #[test]
    fn early_exit_in_test_under_env_or_ci_check_detected() {
        let src = r#"
#[test]
fn test_ci_return() {
    if std::env::var("CI").is_ok() {
        return;
    }
    assert_eq!(1, 1);
}

#[test]
fn test_option_env_return() {
    if option_env!("GITHUB_ACTIONS").is_some() {
        return ();
    }
    assert_eq!(1, 1);
}

#[test]
fn test_generic_env_return() {
    if env::var("SKIP_SLOW").is_ok() {
        return;
    }
    assert_eq!(1, 1);
}

#[test]
fn test_guard_without_exit() {
    if std::env::var("CI").is_ok() {
        println!("in CI");
    }
    assert_eq!(1, 1);
}

fn helper_guard() {
    if std::env::var("CI").is_ok() {
        return;
    }
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("tests/env_check.rs", src, &AssertVocabulary::default())
            .unwrap();

        let ci_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_ci_return")
            .unwrap();
        assert!(!ci_test.ignored);
        assert_eq!(
            ci_test.conditional_ignore.as_deref(),
            Some("std::env::var(\"CI\").is_ok()")
        );

        let opt_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_option_env_return")
            .unwrap();
        assert!(!opt_test.ignored);
        assert_eq!(
            opt_test.conditional_ignore.as_deref(),
            Some("option_env!(\"GITHUB_ACTIONS\").is_some()")
        );

        let generic_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_generic_env_return")
            .unwrap();
        assert!(!generic_test.ignored);
        assert_eq!(
            generic_test.conditional_ignore.as_deref(),
            Some("env::var(\"SKIP_SLOW\").is_ok()")
        );

        let no_exit_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_guard_without_exit")
            .unwrap();
        assert!(!no_exit_test.ignored);
        assert_eq!(no_exit_test.conditional_ignore, None);

        assert!(facts.tests.iter().all(|t| t.name != "helper_guard"));
    }

    #[test]
    fn test_cfg_features_and_any_evaluation() {
        let src = r#"
#[test]
#[cfg(feature = "undeclared_feat")]
fn test_undeclared() {
    assert_eq!(1, 1);
}

#[test]
#[cfg(feature = "declared_feat")]
fn test_declared() {
    assert_eq!(1, 1);
}

#[test]
#[cfg(any())]
fn test_any_empty() {
    assert_eq!(1, 1);
}

#[test]
#[cfg(all(any()))]
fn test_all_any() {
    assert_eq!(1, 1);
}

#[cfg(feature = "undeclared_feat")]
mod mod_tests {
    #[test]
    fn test_in_mod() {
        assert_eq!(1, 1);
    }
}
"#;
        let vocab = AssertVocabulary {
            runner_rules: crate::ast::runner_collection::RunnerCollectionRules::from_files(|p| {
                (p == "Cargo.toml").then(|| {
                    "[package]\nname = \"p\"\nversion = \"0.1.0\"\n\n[features]\ndeclared_feat = []\n"
                        .to_string()
                })
            }),
            ..Default::default()
        };

        let pack = RustPack;
        let facts = pack.extract("tests/cfg_tests.rs", src, &vocab).unwrap();

        let undeclared = facts
            .tests
            .iter()
            .find(|t| t.name == "test_undeclared")
            .unwrap();
        assert!(undeclared.ignored);
        assert_eq!(undeclared.conditional_ignore, None);

        let declared = facts
            .tests
            .iter()
            .find(|t| t.name == "test_declared")
            .unwrap();
        assert!(!declared.ignored);
        assert_eq!(
            declared.conditional_ignore.as_deref(),
            Some(r#"feature = "declared_feat""#)
        );

        let any_empty = facts
            .tests
            .iter()
            .find(|t| t.name == "test_any_empty")
            .unwrap();
        assert!(any_empty.ignored);
        assert_eq!(any_empty.conditional_ignore, None);

        let all_any = facts
            .tests
            .iter()
            .find(|t| t.name == "test_all_any")
            .unwrap();
        assert!(all_any.ignored);
        assert_eq!(all_any.conditional_ignore, None);

        let in_mod = facts
            .tests
            .iter()
            .find(|t| t.name == "mod_tests::test_in_mod")
            .unwrap();
        assert!(in_mod.ignored);
        assert_eq!(in_mod.conditional_ignore, None);
    }

    #[test]
    fn a_module_cfg_is_read_past_a_comment_and_as_an_inner_attribute() {
        let ignored = |src: &str| {
            let f = facts(src);
            assert_eq!(f.tests.len(), 1, "{src}");
            f.tests[0].ignored
        };
        let test = "#[test]\n    fn t() { assert_eq!(a(), 1); }";
        assert!(ignored(&format!(
            "#[cfg(any())]\n// parked\nmod m {{\n    {test}\n}}\n"
        )));
        assert!(ignored(&format!(
            "#[cfg(any())]\n/* parked */\nmod m {{\n    {test}\n}}\n"
        )));
        assert!(ignored(&format!(
            "mod m {{\n    #![cfg(any())]\n    {test}\n}}\n"
        )));
        assert!(ignored(&format!("#![cfg(any())]\n\n{test}\n")));
        assert!(ignored(&format!("#[cfg(false)]\n{test}\n")));
        // Controls: no cfg, a cfg that holds, and an inner attribute that is not a cfg.
        assert!(!ignored(&format!("// parked\nmod m {{\n    {test}\n}}\n")));
        assert!(!ignored(&format!(
            "mod m {{\n    #![cfg(test)]\n    {test}\n}}\n"
        )));
        assert!(!ignored(&format!("#![allow(dead_code)]\n\n{test}\n")));
        assert!(!ignored(&format!("#[cfg(true)]\n{test}\n")));
    }

    #[test]
    fn not_test_is_read_where_it_stands_in_the_predicate() {
        let f = facts("#[cfg(any(not(test), unix))]\n#[test]\nfn t() { assert_eq!(a(), 1); }");
        assert!(!f.tests[0].ignored);
        // Controls: alone it never builds under test, and a CI flag still parks the test.
        let f = facts("#[cfg(not(test))]\n#[test]\nfn t() { assert_eq!(a(), 1); }");
        assert!(f.tests[0].ignored);
        let f = facts("#[cfg(not(ci))]\n#[test]\nfn t() { assert_eq!(a(), 1); }");
        assert!(f.tests[0].ignored);
    }
}
