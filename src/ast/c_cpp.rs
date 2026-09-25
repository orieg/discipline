//! C and C++ language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{
    collect_error_nodes_info, AssertVocabulary, EscapeHatchSite, Fact, LanguagePack,
    ParsedFileFacts, TestFn,
};

/// Functions a test or helper body runs through a table (`super::dispatch_calls`):
/// `std::vector<std::pair<std::string, void (*)(Scope)>> tests = {{"get", TestGet}}`,
/// `void (*checks[])(void) = {check_a, &check_b}`. A table declared at file scope is
/// outside the body and is not read.
pub const C_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["initializer_list"],
    names: &["identifier"],
    // `&check_b` in a table: the name's grandparent is the list, so `names` finds it.
    references: &[],
};

/// C language pack implementing [`LanguagePack`].
pub struct CPack;

impl LanguagePack for CPack {
    fn id(&self) -> &'static str {
        "c"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "C"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("c" | "h"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let facts = self.extract_as_c(path, src, vocab)?;
        // A `.h` header may be C++ (a class, a namespace): when the C grammar leaves error
        // regions, the C++ reading is kept if it leaves fewer.
        if facts.has_parse_errors && super::extension(path) == Some("h") {
            let cpp = CppPack.extract(path, src, vocab)?;
            if cpp.skipped_error_nodes_count < facts.skipped_error_nodes_count {
                return Ok(cpp);
            }
        }
        Ok(facts)
    }
}

impl CPack {
    fn extract_as_c(
        &self,
        path: &str,
        src: &str,
        vocab: &AssertVocabulary,
    ) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the C grammar: {e}"))?;
        let masked = mask_macros(src, vocab);
        let src = masked.as_deref().unwrap_or(src);
        let guarded = super::c_macros::mask_cplusplus_guards(src);
        let src = guarded.as_deref().unwrap_or(src);
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let (has_errors, first_line, error_count) = collect_error_nodes_info(root);
        let mut extractor = CCppExtractor {
            dead: super::reach::dead_ranges(root, src, &C_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_c_cpp_test_path(path),
            test_spans: Vec::new(),
            helpers: std::collections::HashMap::new(),
            helper_calls: std::collections::HashMap::new(),
            test_calls: Vec::new(),
            facts: ParsedFileFacts {
                has_parse_errors: has_errors,
                first_parse_error_line: first_line,
                skipped_error_nodes_count: error_count,
                ..Default::default()
            },
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        shared_facts(root, src, path, vocab, &mut extractor.facts);
        Ok(extractor.facts)
    }
}

/// C++ language pack implementing [`LanguagePack`].
pub struct CppPack;

impl LanguagePack for CppPack {
    fn id(&self) -> &'static str {
        "cpp"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "C++"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(
            super::extension(path),
            Some("cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx")
        )
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the C++ grammar: {e}"))?;
        let masked = mask_macros(src, vocab);
        let src = masked.as_deref().unwrap_or(src);
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let (has_errors, first_line, error_count) = collect_error_nodes_info(root);
        let mut extractor = CCppExtractor {
            dead: super::reach::dead_ranges(root, src, &C_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_c_cpp_test_path(path),
            test_spans: Vec::new(),
            helpers: std::collections::HashMap::new(),
            helper_calls: std::collections::HashMap::new(),
            test_calls: Vec::new(),
            facts: ParsedFileFacts {
                has_parse_errors: has_errors,
                first_parse_error_line: first_line,
                skipped_error_nodes_count: error_count,
                ..Default::default()
            },
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        shared_facts(root, src, path, vocab, &mut extractor.facts);
        Ok(extractor.facts)
    }
}

/// `src` with the extension macros the grammar cannot read rewritten, byte for byte
/// (`super::c_macros`); `None` when the file uses none.
fn mask_macros(src: &str, vocab: &AssertVocabulary) -> Option<String> {
    use super::c_macros::{lists, mask, BUILTIN_FUNCTION_MACROS, BUILTIN_MACROS};
    mask(
        src,
        &lists(&vocab.c_macros, BUILTIN_MACROS),
        &lists(&vocab.c_function_macros, BUILTIN_FUNCTION_MACROS),
    )
}

/// The facts the shared walkers supply, for both grammars (they share node kinds).
fn shared_facts(
    root: Node,
    src: &str,
    path: &str,
    vocab: &AssertVocabulary,
    facts: &mut ParsedFileFacts,
) {
    facts.functions = functions::extract(root, src, path, &C_FUNCTIONS);
    super::mocks::count(
        root,
        src,
        &mut facts.tests,
        &C_MOCKS,
        &vocab.mock_setup_fns,
        &vocab.mock_assert_fns,
    );
    {
        let spans: Vec<(usize, usize)> = facts
            .tests
            .iter()
            .map(|t| (t.line, t.end_line.max(t.line)))
            .collect();
        let whole_file = is_c_cpp_test_path(path)
            || functions::test_path(path)
            || functions::declared_test_path(path, &vocab.test_paths);
        let is_test_line = |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
        facts.swallowed = super::handlers::extract(root, src, &C_HANDLERS, &is_test_line);
    }
    super::retries::mark(root, src, &mut facts.tests, &C_RETRIES);
    if functions::declared_test_path(path, &vocab.test_paths) {
        for f in &mut facts.functions {
            f.is_test = true;
        }
    }
    super::calls::count(
        root,
        src,
        &mut facts.tests,
        &C_MOCKS,
        super::calls::SLEEP_VOCAB,
        super::calls::sleeps,
    );
    super::calls::count(
        root,
        src,
        &mut facts.tests,
        &C_MOCKS,
        super::calls::TRIVIAL_ASSERT_VOCAB,
        super::calls::trivial_asserts,
    );
    facts.prose = super::prose::extract(
        root,
        src,
        &["comment", "string_literal", "raw_string_literal"],
    );
}

/// A test-framework macro body (`TEST(Suite, Name) {}`) parses as a function definition
/// whose declarator is the macro call; a C driver's `test_*` / `*_smoke` function or
/// anything in a test path is a test too.
fn c_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let decl = node
        .child_by_field_name("declarator")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    let name = decl
        .split(['(', ' '])
        .next()
        .unwrap_or("")
        .trim_start_matches('*');
    const MACROS: &[&str] = &[
        "TEST",
        "TEST_F",
        "TEST_P",
        "TYPED_TEST",
        "TYPED_TEST_P",
        "TEST_CASE",
        "TEST_CASE_METHOD",
        "SCENARIO",
        "TEST_CASE_TEMPLATE",
    ];
    MACROS.contains(&name)
        || name.starts_with("test_")
        || name.ends_with("_test")
        || name.starts_with("smoke_")
        || name.ends_with("_smoke")
        || is_c_cpp_test_path(path)
        || functions::test_path(path)
}

pub const C_FUNCTIONS: FunctionSpec = FunctionSpec {
    // `= default` / `= delete` and a pure-virtual declaration have no `body`.
    function_kinds: &["function_definition"],
    name_fields: &["declarator"],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    skip: functions::skip_none,
    is_test: c_fn_is_test,
    classify: functions::classify_c,
};

pub const C_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
};

pub const C_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    // C has no `catch_clause`; the kind never matches there.
    handler_kinds: &["catch_clause"],
    arm_of: &[],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    trivial: &[
        "return",
        "return false",
        "return nullptr",
        "return NULL",
        "return {}",
        "continue",
        "break",
    ],
    // `(void)call()` throws the result away; `(void)x` of a variable is not a call.
    discard_kinds: &["cast_expression"],
    discards: super::handlers::c_discards,
    classify_discard: Some(super::handlers::c_discard_class),
    call_value_kinds: &["call_expression"],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
};

pub const C_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["call_expression"],
};

/// Determines whether a path is conventionally a C or C++ test file.
pub fn is_c_cpp_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename.ends_with("Test.cpp")
        || filename.ends_with("Test.cc")
        || filename.ends_with("Test.cxx")
        || filename.ends_with("Test.c")
        || filename.ends_with("_test.cpp")
        || filename.ends_with("_test.cc")
        || filename.ends_with("_test.cxx")
        || filename.ends_with("_test.c")
        || filename.ends_with("_smoke.c")
        || filename.ends_with("_smoke.cpp")
        || filename.starts_with("test_")
        || filename.starts_with("smoke_")
        || filename == "test.c"
        || filename == "test.cpp"
        || filename == "test.cc"
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
        || path.starts_with("smoke/")
        || path.contains("/smoke/")
        || path.starts_with("testing/")
        || path.contains("/testing/")
}

struct CCppExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    test_spans: Vec<std::ops::Range<usize>>,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    /// The calls each same-file helper makes: `main` -> `check_seek` -> `require` is a
    /// test's checks two levels down.
    helper_calls: std::collections::HashMap<String, Vec<String>>,
    test_calls: Vec<Vec<String>>,
    facts: ParsedFileFacts,
}

impl<'a> CCppExtractor<'a> {
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
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim();

            if trimmed.starts_with("NOLINT") {
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
        self.walk_scope(root);
        self.resolve_same_file_helpers();
        self.collect_compile_time_asserts(root);
        self.facts.build_compile_time_test();
    }

    fn resolve_same_file_helpers(&mut self) {
        let (helpers, helper_calls) = (&self.helpers, &self.helper_calls);
        for (i, test) in self.facts.tests.iter_mut().enumerate() {
            if let Some(calls) = self.test_calls.get(i) {
                for call in calls {
                    let mut path = Vec::new();
                    if let Some(h) =
                        super::transitive_helper(call, helpers, helper_calls, &mut path).as_ref()
                    {
                        if self.vocab.helper_fns.iter().any(|name| name == call) {
                            test.total_asserts = test.total_asserts.saturating_sub(1);
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
    }

    fn is_compile_time_assert_node(&self, node: Node) -> bool {
        let kind = node.kind();
        if kind == "static_assert_declaration" {
            return true;
        }
        if kind == "call_expression" {
            let name = self.get_call_fn_name(node);
            if matches!(name, "static_assert" | "_Static_assert") {
                return true;
            }
        }
        false
    }

    fn collect_compile_time_asserts(&mut self, node: Node) {
        let byte_pos = node.start_byte();
        let in_test = self.test_spans.iter().any(|r| r.contains(&byte_pos));
        if !in_test && self.is_compile_time_assert_node(node) {
            self.facts.compile_time_asserts += 1;
            if self.facts.compile_time_assert_line.is_none() {
                self.facts.compile_time_assert_line = Some(node.start_position().row + 1);
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_compile_time_asserts(child);
        }
    }

    fn walk_scope(&mut self, scope: Node) {
        let mut cursor = scope.walk();
        let children: Vec<Node> = scope.children(&mut cursor).collect();
        let mut i = 0;
        while i < children.len() {
            let node = children[i];
            let kind = node.kind();

            // 1. function_definition (GoogleTest macros, C ABI smoke test main, test_* fns)
            if kind == "function_definition" {
                if let Some(test_fn) = self.try_extract_function_definition_test(node) {
                    self.test_spans.push(node.start_byte()..node.end_byte());
                    self.facts.tests.push(test_fn);
                } else if let (Some(decl), Some(body)) = (
                    node.child_by_field_name("declarator"),
                    node.child_by_field_name("body"),
                ) {
                    let (fn_name, _) = self.inspect_declarator(decl);
                    if !fn_name.is_empty() {
                        let mut helper_fn = TestFn::default();
                        let mut dummy_calls = Vec::new();
                        self.extract_assertions_in_body(body, &mut helper_fn, &mut dummy_calls);
                        super::dispatch_calls(body, self.src, &C_DISPATCH, &mut dummy_calls);
                        self.helpers.insert(
                            fn_name.to_string(),
                            super::HelperFacts {
                                total_asserts: helper_fn.total_asserts,
                                strong_asserts: helper_fn.strong_asserts,
                                tautologies: helper_fn.tautologies,
                                fatal_asserts: helper_fn.fatal_asserts,
                            },
                        );
                        self.helper_calls.insert(fn_name.to_string(), dummy_calls);
                    }
                }
                i += 1;
                continue;
            }

            // 2. Catch2 top-level pattern:
            // expression_statement (call_expression TEST_CASE(...)) followed by compound_statement
            if kind == "expression_statement" {
                if let Some(call) = node.child(0) {
                    if call.kind() == "call_expression" {
                        let fn_name = self.get_call_fn_name(call);
                        if matches!(fn_name, "TEST_CASE" | "TEST_CASE_METHOD") {
                            // Check next sibling for compound_statement
                            if i + 1 < children.len()
                                && children[i + 1].kind() == "compound_statement"
                            {
                                let body = children[i + 1];
                                let (name, is_ignored) = self.extract_catch2_metadata(call);
                                let line = call.start_position().row + 1;
                                let end_line = body.end_position().row + 1;
                                let mut test_fn = TestFn {
                                    name,
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
                                self.extract_assertions_in_body(body, &mut test_fn, &mut calls);
                                super::dispatch_calls(body, self.src, &C_DISPATCH, &mut calls);
                                self.test_calls.push(calls);
                                self.test_spans.push(call.start_byte()..body.end_byte());
                                self.facts.tests.push(test_fn);
                                i += 2; // skip compound_statement
                                continue;
                            }
                        }
                    }
                }
            }

            // 3. Recurse into namespaces, linkage specs, declaration lists
            if matches!(
                kind,
                "namespace_definition" | "linkage_specification" | "declaration_list"
            ) {
                if let Some(body) = node.child_by_field_name("body") {
                    self.walk_scope(body);
                } else {
                    self.walk_scope(node);
                }
            }

            i += 1;
        }
    }

    fn try_extract_function_definition_test(&mut self, node: Node) -> Option<TestFn> {
        let declarator_node = node.child_by_field_name("declarator")?;
        let body = node.child_by_field_name("body")?;

        // Check if declarator is a GoogleTest or test macro:
        // Tree-sitter may parse TEST(Suite, Name) as function_declarator or call_expression.
        let (fn_name, param_nodes) = self.inspect_declarator(declarator_node);

        if matches!(
            fn_name,
            "TEST" | "TEST_F" | "TEST_P" | "TYPED_TEST" | "TYPED_TEST_P"
        ) {
            let (suite, case, is_ignored) = self.extract_gtest_params(&param_nodes);
            let name = if suite.is_empty() && case.is_empty() {
                "GoogleTest".to_string()
            } else if suite.is_empty() {
                case
            } else if case.is_empty() {
                suite
            } else {
                format!("{suite}::{case}")
            };
            let line = declarator_node.start_position().row + 1;
            let end_line = body.end_position().row + 1;
            let mut test_fn = TestFn {
                name,
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
            self.extract_assertions_in_body(body, &mut test_fn, &mut calls);
            super::dispatch_calls(body, self.src, &C_DISPATCH, &mut calls);
            self.test_calls.push(calls);
            return Some(test_fn);
        }

        if matches!(fn_name, "TEST_CASE" | "TEST_CASE_METHOD") {
            let name = param_nodes
                .first()
                .map(|n| self.text(*n).trim_matches('"').to_string())
                .unwrap_or_else(|| "Catch2Test".to_string());
            let is_ignored = param_nodes.iter().any(|n| {
                let txt = self.text(*n);
                txt.contains("[.") || txt.contains("[!hide]")
            });
            let line = declarator_node.start_position().row + 1;
            let end_line = body.end_position().row + 1;
            let mut test_fn = TestFn {
                name,
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
            self.extract_assertions_in_body(body, &mut test_fn, &mut calls);
            super::dispatch_calls(body, self.src, &C_DISPATCH, &mut calls);
            self.test_calls.push(calls);
            return Some(test_fn);
        }

        // C ABI smoke test (main) or test_* functions
        let is_main_test = fn_name == "main" && self.is_test_path;
        let is_test_fn = fn_name.starts_with("test_")
            || fn_name.ends_with("_test")
            || fn_name.starts_with("smoke_")
            || fn_name.ends_with("_smoke");

        if is_main_test || is_test_fn {
            let line = declarator_node.start_position().row + 1;
            let end_line = body.end_position().row + 1;
            let mut test_fn = TestFn {
                name: fn_name.to_string(),
                line,
                end_line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: false,
                should_panic: false,
                ..Default::default()
            };
            let mut calls = Vec::new();
            self.extract_assertions_in_body(body, &mut test_fn, &mut calls);
            super::dispatch_calls(body, self.src, &C_DISPATCH, &mut calls);
            self.test_calls.push(calls);
            return Some(test_fn);
        }

        None
    }

    fn inspect_declarator<'b>(&self, declarator: Node<'b>) -> (&'a str, Vec<Node<'b>>) {
        let kind = declarator.kind();
        if kind == "function_declarator" {
            let inner_decl = declarator.child_by_field_name("declarator");
            let name = inner_decl.map(|n| self.text(n)).unwrap_or("");
            let params = declarator
                .child_by_field_name("parameters")
                .map(|p| {
                    let mut cursor = p.walk();
                    p.children(&mut cursor)
                        .filter(|c| c.is_named())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return (name, params);
        }

        if kind == "call_expression" {
            let name = declarator
                .child_by_field_name("function")
                .map(|f| self.text(f))
                .unwrap_or("");
            let args = declarator
                .child_by_field_name("arguments")
                .map(|a| {
                    let mut cursor = a.walk();
                    a.children(&mut cursor)
                        .filter(|c| c.is_named())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return (name, args);
        }

        if kind == "pointer_declarator" {
            if let Some(child) = declarator.child_by_field_name("declarator") {
                return self.inspect_declarator(child);
            }
        }

        (self.text(declarator), Vec::new())
    }

    fn extract_gtest_params(&self, params: &[Node]) -> (String, String, bool) {
        let suite = params
            .first()
            .map(|n| self.clean_param_text(*n))
            .unwrap_or_default();
        let case = params
            .get(1)
            .map(|n| self.clean_param_text(*n))
            .unwrap_or_default();
        let is_ignored = suite.starts_with("DISABLED_") || case.starts_with("DISABLED_");
        (suite, case, is_ignored)
    }

    fn clean_param_text(&self, node: Node) -> String {
        let raw = self.text(node).trim();
        raw.trim_matches('"').to_string()
    }

    fn extract_catch2_metadata(&self, call: Node) -> (String, bool) {
        let args = call
            .child_by_field_name("arguments")
            .map(|a| {
                let mut cursor = a.walk();
                a.children(&mut cursor)
                    .filter(|c| c.is_named())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let name = args
            .first()
            .map(|n| self.clean_param_text(*n))
            .unwrap_or_else(|| "Catch2Test".to_string());
        let is_ignored = args.iter().any(|n| {
            let txt = self.text(*n);
            txt.contains("[.") || txt.contains("[!hide]")
        });
        (name, is_ignored)
    }

    fn get_call_fn_name(&self, call: Node) -> &'a str {
        let fn_node = call.child_by_field_name("function");
        let Some(f) = fn_node else { return "" };
        let kind = f.kind();
        if kind == "identifier" {
            return self.text(f);
        }
        if kind == "field_expression" {
            if let Some(field) = f.child_by_field_name("field") {
                return self.text(field);
            }
        }
        if kind == "qualified_identifier" {
            if let Some(name) = f.child_by_field_name("name") {
                return self.text(name);
            }
        }
        self.text(f)
    }

    fn extract_assertions_in_body(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        let kind = node.kind();

        if kind == "static_assert_declaration" {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        if kind == "throw_statement" {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        if kind == "return_statement" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    let text = self.text(child).trim();
                    if text != "0"
                        && text != "EXIT_SUCCESS"
                        && text != "NULL"
                        && text != "nullptr"
                        && text != "false"
                        && text != "{}"
                    {
                        test_fn.total_asserts += 1;
                        test_fn.strong_asserts += 1;
                        return;
                    }
                }
            }
        }

        if kind == "call_expression" {
            let fn_name = self.get_call_fn_name(node);
            if !fn_name.is_empty() {
                calls.push(fn_name.to_string());
            }

            // Skip detection inside test body
            if matches!(fn_name, "GTEST_SKIP" | "SKIP") {
                test_fn.ignored = true;
                return;
            }

            // Aborting and terminating calls (failure path in C/C++ drivers)
            if matches!(
                fn_name,
                "abort"
                    | "exit"
                    | "_exit"
                    | "_Exit"
                    | "quick_exit"
                    | "terminate"
                    | "__builtin_trap"
                    | "panic"
                    | "fail"
                    | "fatal"
            ) || fn_name.ends_with("::abort")
                || fn_name.ends_with("::exit")
                || fn_name.ends_with("::terminate")
            {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
                // Ending the process is as fatal as `assert` / `ASSERT_*`; `fail` means
                // different things in different frameworks.
                if fn_name != "fail" {
                    test_fn.fatal_asserts += 1;
                }
                return;
            }

            let args = node
                .child_by_field_name("arguments")
                .map(|a| {
                    let mut cursor = a.walk();
                    a.children(&mut cursor)
                        .filter(|c| c.is_named())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // 1. GoogleTest macros: EXPECT_* / ASSERT_*
            if fn_name.starts_with("EXPECT_") || fn_name.starts_with("ASSERT_") {
                self.handle_gtest_assertion(fn_name, &args, test_fn);
                return;
            }

            // 2. Catch2 / doctest macros
            if matches!(
                fn_name,
                "REQUIRE"
                    | "CHECK"
                    | "REQUIRE_FALSE"
                    | "CHECK_FALSE"
                    | "REQUIRE_THAT"
                    | "CHECK_THAT"
                    | "REQUIRE_THROWS"
                    | "CHECK_THROWS"
                    | "REQUIRE_THROWS_AS"
                    | "CHECK_THROWS_AS"
                    | "REQUIRE_NOTHROW"
                    | "CHECK_NOTHROW"
                    | "WARN"
            ) {
                self.handle_catch2_assertion(fn_name, &args, test_fn);
                return;
            }

            // 3. C standard assert(...)
            if fn_name == "assert" {
                self.handle_c_assert(&args, test_fn);
                return;
            }

            // 4. Unity / CppUTest / Boost.Test
            if fn_name.starts_with("TEST_ASSERT")
                || fn_name.starts_with("CHECK_")
                || fn_name.starts_with("BOOST_")
            {
                self.handle_generic_framework_assertion(fn_name, &args, test_fn);
                return;
            }

            // 5. Configured extra macros or helper fns
            if self.vocab.extra_macros.iter().any(|m| m == fn_name)
                || self.vocab.helper_fns.iter().any(|h| h == fn_name)
            {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
                return;
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_assertions_in_body(child, test_fn, calls);
        }
    }

    fn handle_gtest_assertion(&self, fn_name: &str, args: &[Node], test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;
        if fn_name.starts_with("ASSERT_") {
            test_fn.fatal_asserts += 1;
        }

        let is_strong = fn_name.contains("_EQ")
            || fn_name.contains("_NE")
            || fn_name.contains("_STREQ")
            || fn_name.contains("_STRNE")
            || fn_name.contains("_STRCASEEQ")
            || fn_name.contains("_STRCASENE")
            || fn_name.contains("_LT")
            || fn_name.contains("_LE")
            || fn_name.contains("_GT")
            || fn_name.contains("_GE")
            || fn_name.contains("_NEAR")
            || fn_name.contains("_THAT")
            || fn_name.contains("_THROW")
            || fn_name.contains("_NO_THROW")
            || fn_name.contains("_ANY_THROW")
            || fn_name.contains("_DOUBLE_EQ")
            || fn_name.contains("_FLOAT_EQ")
            || fn_name.contains("_DEATH")
            || fn_name.contains("_EXIT");

        if is_strong {
            test_fn.strong_asserts += 1;
            // Check equality tautology if 2 args
            if (fn_name.contains("_EQ") || fn_name.contains("_NE")) && args.len() >= 2 {
                let left = self.text(args[0]).trim();
                let right = self.text(args[1]).trim();
                if !left.is_empty() && left == right {
                    test_fn.tautologies += 1;
                }
            }
        } else if fn_name.ends_with("_TRUE") {
            if let Some(arg) = args.first() {
                if self.is_literal_true(*arg) || self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            }
        } else if fn_name.ends_with("_FALSE") {
            if let Some(arg) = args.first() {
                if self.is_literal_false(*arg) || self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            }
        }
    }

    fn handle_catch2_assertion(&self, fn_name: &str, args: &[Node], test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;
        if fn_name.starts_with("REQUIRE") {
            test_fn.fatal_asserts += 1;
        }

        if fn_name.contains("_THROWS") || fn_name.contains("_THAT") || fn_name.contains("_NOTHROW")
        {
            test_fn.strong_asserts += 1;
            return;
        }

        if fn_name == "REQUIRE" || fn_name == "CHECK" {
            if let Some(arg) = args.first() {
                if self.contains_comparison(*arg) {
                    test_fn.strong_asserts += 1;
                    if self.is_tautology_comparison(*arg) {
                        test_fn.tautologies += 1;
                    }
                } else if self.is_literal_true(*arg) {
                    test_fn.tautologies += 1;
                }
            }
        } else if fn_name == "REQUIRE_FALSE" || fn_name == "CHECK_FALSE" {
            if let Some(arg) = args.first() {
                if self.is_literal_false(*arg) || self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            }
        }
    }

    fn handle_c_assert(&self, args: &[Node], test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;
        test_fn.fatal_asserts += 1;
        if let Some(arg) = args.first() {
            if self.contains_comparison(*arg) {
                test_fn.strong_asserts += 1;
                if self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            } else if self.is_literal_true(*arg) {
                test_fn.tautologies += 1;
            }
        }
    }

    fn handle_generic_framework_assertion(
        &self,
        fn_name: &str,
        args: &[Node],
        test_fn: &mut TestFn,
    ) {
        test_fn.total_asserts += 1;
        let is_strong = fn_name.contains("EQUAL")
            || fn_name.contains("_EQ")
            || fn_name.contains("_NE")
            || fn_name.contains("MATCH");
        if is_strong {
            test_fn.strong_asserts += 1;
            if args.len() >= 2 {
                let left = self.text(args[0]).trim();
                let right = self.text(args[1]).trim();
                if !left.is_empty() && left == right {
                    test_fn.tautologies += 1;
                }
            }
        } else if let Some(arg) = args.first() {
            if self.contains_comparison(*arg) {
                test_fn.strong_asserts += 1;
                if self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            } else if self.is_literal_true(*arg) {
                test_fn.tautologies += 1;
            }
        }
    }

    fn contains_comparison(&self, node: Node) -> bool {
        let kind = node.kind();
        if kind == "binary_expression" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let op = self.text(child).trim();
                if matches!(op, "==" | "!=" | "<" | ">" | "<=" | ">=") {
                    return true;
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if self.contains_comparison(child) {
                return true;
            }
        }
        false
    }

    fn is_tautology_comparison(&self, node: Node) -> bool {
        let kind = node.kind();
        if kind == "binary_expression" {
            if let (Some(left), Some(right)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("right"),
            ) {
                let mut cursor = node.walk();
                let is_comp = node.children(&mut cursor).any(|c| {
                    let op = self.text(c).trim();
                    matches!(op, "==" | "!=")
                });
                if is_comp {
                    let left_txt = self.text(left).trim();
                    let right_txt = self.text(right).trim();
                    if !left_txt.is_empty() && left_txt == right_txt {
                        return true;
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if self.is_tautology_comparison(child) {
                return true;
            }
        }
        false
    }

    fn is_literal_true(&self, node: Node) -> bool {
        let txt = self.text(node).trim();
        txt == "true" || txt == "1" || node.kind() == "true"
    }

    fn is_literal_false(&self, node: Node) -> bool {
        let txt = self.text(node).trim();
        txt == "false" || txt == "0" || node.kind() == "false"
    }
}

pub const C_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["compound_statement"],
    ignored_kinds: &["comment"],
    terminators: &["return", "abort()", "exit(", "_exit(", "throw"],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_c_cpp_pack_registration_and_extension_matching() {
        let c_pack = CPack;
        assert_eq!(c_pack.id(), "c");
        assert_eq!(c_pack.name(), "C");
        assert!(c_pack.matches("smoke.c"));
        assert!(c_pack.matches("header.h"));
        assert!(!c_pack.matches("test.cpp"));

        let cpp_pack = CppPack;
        assert_eq!(cpp_pack.id(), "cpp");
        assert_eq!(cpp_pack.name(), "C++");
        assert!(cpp_pack.matches("foo.cpp"));
        assert!(cpp_pack.matches("bar.cc"));
        assert!(cpp_pack.matches("baz.cxx"));
        assert!(cpp_pack.matches("hdr.hpp"));
        assert!(cpp_pack.matches("hdr.hh"));
        assert!(cpp_pack.matches("hdr.hxx"));
        assert!(!cpp_pack.matches("foo.c"));
    }

    #[test]
    fn test_googletest_extraction_and_assertions() {
        let src = r#"
TEST(VectorSuite, PushBack) {
    EXPECT_EQ(v.size(), 1);
    ASSERT_NE(v.data(), nullptr);
    EXPECT_TRUE(v.empty() == false);
}

TEST_F(FixtureSuite, DISABLED_SlowTest) {
    EXPECT_EQ(1, 2);
}

TEST_P(ParamSuite, SkipDynamically) {
    GTEST_SKIP() << "Skipping this parameter";
    EXPECT_EQ(1, 1);
}
"#;
        let cpp_pack = CppPack;
        let facts = cpp_pack
            .extract("tests/vector_test.cpp", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 3);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "VectorSuite::PushBack");
        assert_eq!(t0.total_asserts, 3);
        assert_eq!(t0.strong_asserts, 2); // EXPECT_EQ, ASSERT_NE
        assert!(!t0.ignored);
        assert!(!t0.is_vacuous());

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "FixtureSuite::DISABLED_SlowTest");
        assert!(t1.ignored);

        let t2 = &facts.tests[2];
        assert_eq!(t2.name, "ParamSuite::SkipDynamically");
        assert!(t2.ignored);
    }

    #[test]
    fn test_catch2_extraction_and_assertions() {
        let src = r#"
TEST_CASE("vector operations", "[vector]") {
    REQUIRE(vec.size() == 0);
    CHECK(vec.empty());
    REQUIRE(1 == 1); // tautology
}

TEST_CASE("skipped case", "[.hidden]") {
    REQUIRE(1 == 2);
}
"#;
        let cpp_pack = CppPack;
        let facts = cpp_pack
            .extract("tests/catch2_test.cpp", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 2);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "vector operations");
        assert_eq!(t0.total_asserts, 3);
        assert_eq!(t0.strong_asserts, 2); // REQUIRE(vec.size() == 0) and REQUIRE(1 == 1)
        assert_eq!(t0.tautologies, 1); // 1 == 1
        assert_eq!(t0.effective_asserts(), 2);
        assert!(!t0.ignored);

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "skipped case");
        assert!(t1.ignored);
    }

    #[test]
    fn test_c_abi_smoke_modern_api_example_fixture() {
        let src = r#"
/* Compile-and-run check of include/example.h against the built library. */
#include <example.h>
#include <stdio.h>
#include <string.h>
#include <assert.h>

int main(void) {
    printf("libexample %s\n", example_version());

    example_map_t *m = example_map_new();
    for (uint64_t k = 0; k < 1000; k++) {
        uint64_t *slot = example_map_ins_slot(m, k * 3);
        *slot = k;
    }
    uint64_t v = 0, key = 0;
    assert(example_map_get(m, 300, &v) && v == 100);
    assert(example_map_count_range(m, 0, 299) == 100);
    assert(example_map_by_count(m, 10, &key, &v) && key == 30 && v == 10);
    assert(example_map_next_after(m, 30, &key, NULL) && key == 33);
    printf("map: len=%llu mem=%zu rank/select ok\n",
           (unsigned long long) example_map_len(m), example_map_mem_used(m));
    example_map_free(m);

    example_sync_map_t *s = example_sync_map_new();
    example_sync_map_insert(s, 7, 77, NULL);
    example_sync_map_reader_t *r = example_sync_map_reader_new(s);
    assert(example_sync_map_reader_get(r, 7, &v) && v == 77);
    example_sync_map_reader_free(r);
    example_sync_map_free(s);
    printf("sync: concurrent reader ok\n");

    example_bytesmap_t *b = example_bytesmap_new();
    example_bytesmap_insert(b, "a\0b", 3, 5, NULL);
    assert(example_bytesmap_get(b, "a\0b", 3, &v) && v == 5);
    assert(!example_bytesmap_get(b, "a", 1, &v));
    example_bytesmap_free(b);
    printf("bytesmap: embedded NUL ok\nok\n");
    return 0;
}
"#;
        let c_pack = CPack;
        let facts = c_pack
            .extract(
                "crates/example-capi/smoke/modern_api_smoke.c",
                src,
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "main");
        assert_eq!(t.line, 8);
        assert_eq!(t.total_asserts, 7);
        assert_eq!(t.strong_asserts, 6);
        assert_eq!(t.tautologies, 0);
        assert_eq!(t.effective_asserts(), 7);
        assert!(!t.is_vacuous());
        assert!(!t.ignored);
    }

    #[test]
    fn test_tautologies_and_vacuous_tests_in_c_and_cpp() {
        let src = r#"
TEST(VacuousSuite, EmptyTest) {
    // No assertions at all
}

TEST(VacuousSuite, TautologyOnly) {
    EXPECT_TRUE(true);
    EXPECT_EQ(x, x);
    assert(1);
    assert(y == y);
}
"#;
        let cpp_pack = CppPack;
        let facts = cpp_pack
            .extract("tests/vacuous_test.cpp", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 2);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "VacuousSuite::EmptyTest");
        assert_eq!(t0.total_asserts, 0);
        assert!(t0.is_vacuous());

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "VacuousSuite::TautologyOnly");
        assert_eq!(t1.total_asserts, 4);
        assert_eq!(t1.tautologies, 4);
        assert_eq!(t1.effective_asserts(), 0);
        assert!(t1.is_vacuous());
    }

    #[test]
    fn test_escape_hatches_and_custom_vocab() {
        let src = r#"
// NOLINTBEGIN(readability-magic-numbers)
void test_custom() {
    // NOLINTNEXTLINE
    CUSTOM_CHECK(val);
}
"#;
        let vocab = AssertVocabulary {
            extra_macros: vec!["CUSTOM_CHECK".to_string()],
            ..Default::default()
        };
        let c_pack = CPack;
        let facts = c_pack
            .extract("tests/test_custom.c", src, &vocab)
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "test_custom");
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);

        assert_eq!(facts.escape_hatches.len(), 2);
        assert!(matches!(
            &facts.escape_hatches[0],
            EscapeHatchSite::LinterDisable { rule, .. } if rule.contains("NOLINTBEGIN")
        ));
        assert!(matches!(
            &facts.escape_hatches[1],
            EscapeHatchSite::LinterDisable { rule, .. } if rule.contains("NOLINTNEXTLINE")
        ));
    }

    #[test]
    fn test_c_cpp_compile_time_assertions_outside_tests_are_extracted() {
        let cpp_src = r#"
struct InvariantStruct {
    int x;
    long long y;
    static_assert(sizeof(int) == 4, "int size");
};

static_assert(sizeof(InvariantStruct) >= 8, "struct minimum size");

TEST(MySuite, MyTest) {
    EXPECT_EQ(1, 1);
}
"#;
        let cpp_pack = CppPack;
        let facts = cpp_pack
            .extract("src/foo.cpp", cpp_src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.compile_time_asserts, 2);
        assert_eq!(facts.compile_time_assert_line, Some(5));
        assert!(facts.compile_time_test.is_some());
        let ctt = facts.compile_time_test.unwrap();
        assert_eq!(ctt.name, "compile-time-assertions");
        assert_eq!(ctt.total_asserts, 2);
        assert_eq!(ctt.strong_asserts, 2);
        assert_eq!(facts.tests.len(), 1);

        let c_src = r#"
_Static_assert(sizeof(int) == 4, "int size");
_Static_assert(sizeof(long) >= 4, "long size");

int test_foo(void) {
    assert(1);
    return 0;
}
"#;
        let c_pack = CPack;
        let c_facts = c_pack
            .extract("tests/test_c.c", c_src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(c_facts.compile_time_asserts, 2);
        assert_eq!(c_facts.compile_time_assert_line, Some(2));
        assert_eq!(c_facts.tests.len(), 1);
    }

    #[test]
    fn test_c_cpp_fatal_assertions_gtest_and_catch2() {
        let gtest_src = r#"
TEST(Suite, Case) {
    int x = 10;
    EXPECT_EQ(x, 10);
    ASSERT_EQ(x, 10);
}
"#;
        let facts = CppPack
            .extract("test_gtest.cpp", gtest_src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.total_asserts, 2);
        assert_eq!(t.fatal_asserts, 1);

        let catch2_src = r#"
TEST_CASE("Catch2 fatal vs nonfatal") {
    int x = 5;
    CHECK(x == 5);
    REQUIRE(x == 5);
}
"#;
        let c2_facts = CppPack
            .extract("test_catch2.cpp", catch2_src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(c2_facts.tests.len(), 1);
        let c2_t = &c2_facts.tests[0];
        assert_eq!(c2_t.total_asserts, 2);
        assert_eq!(c2_t.fatal_asserts, 1);
    }

    #[test]
    fn test_c_cpp_main_failure_paths_and_helper_resolution() {
        let src_nonzero_return = r#"
int main() {
    if (!check_ready()) {
        return 1;
    }
    return 0;
}
"#;
        let facts = CppPack
            .extract(
                "tests/test_driver.cc",
                src_nonzero_return,
                &AssertVocabulary::default(),
            )
            .unwrap();
        assert_eq!(facts.tests.len(), 1);
        assert!(!facts.tests[0].is_vacuous());
        assert_eq!(facts.tests[0].strong_asserts, 1);

        let src_helper_abort = r#"
void fail_if_bad(int v) {
    if (v < 0) {
        abort();
    }
}

int main() {
    fail_if_bad(-5);
    return 0;
}
"#;
        let facts2 = CppPack
            .extract(
                "tests/test_memtable.cc",
                src_helper_abort,
                &AssertVocabulary::default(),
            )
            .unwrap();
        assert_eq!(facts2.tests.len(), 1);
        assert!(!facts2.tests[0].is_vacuous());
        assert_eq!(facts2.tests[0].strong_asserts, 1);
    }

    fn parse_errors(src: &str, vocab: &AssertVocabulary) -> usize {
        CPack
            .extract("ext/judy.c", src, vocab)
            .unwrap()
            .skipped_error_nodes_count
    }

    #[test]
    fn extension_macros_parse_without_error_regions() {
        let v = AssertVocabulary::default();
        for src in [
            "PHP_METHOD(Judy, size)\n{\n\tRETURN_LONG(1);\n}\n",
            "ZEND_DECLARE_MODULE_GLOBALS(judy)\n\nstatic int f(void) { return 1; }\n",
            "ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_x, 0, 0, IS_LONG, 0)\n\tZEND_ARG_TYPE_INFO(0, i, IS_LONG, 0)\nZEND_END_ARG_INFO()\n",
            "static void f(zval *z) {\n\tZEND_PARSE_PARAMETERS_START(1, 1)\n\t\tZ_PARAM_ZVAL(z)\n\tZEND_PARSE_PARAMETERS_END();\n}\n",
            "static const zend_function_entry m[] = {\n\tPHP_ME(Judy, size, arginfo_x, ZEND_ACC_PUBLIC)\n\tPHP_FE_END\n};\n",
            "typedef struct {\n\tPyObject_HEAD\n\tint n;\n} Box;\n",
        ] {
            assert_eq!(parse_errors(src, &v), 0, "{src}");
        }
        // Negative control: a macro no list names is still an error region, and becomes
        // readable once configured.
        let own = "MYEXT_METHOD(Judy, size)\n{\n\tMYEXT_CHECK(1)\n\treturn;\n}\n";
        assert!(parse_errors(own, &v) > 0);
        let configured = AssertVocabulary {
            c_macros: vec!["MYEXT_CHECK".to_string()],
            c_function_macros: vec!["MYEXT_METHOD".to_string()],
            ..Default::default()
        };
        assert_eq!(parse_errors(own, &configured), 0);
    }

    #[test]
    fn findings_inside_a_macro_function_keep_their_lines() {
        let src = "ZEND_BEGIN_ARG_INFO_EX(arginfo_clear, 0, 0, 0)\nZEND_END_ARG_INFO()\n\nPHP_METHOD(Judy, clear)\n{\n\tZEND_PARSE_PARAMETERS_NONE();\n\t(void)zend_hash_clean(h);\n}\n\nPHP_METHOD(Judy, later)\n{\n}\n";
        let facts = CPack
            .extract("ext/judy.c", src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.skipped_error_nodes_count, 0);
        let lines: Vec<usize> = facts.swallowed.iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![7], "{:?}", facts.swallowed);
        assert!(facts.swallowed[0]
            .snippet
            .starts_with("(void)zend_hash_clean"));
        let later = facts
            .functions
            .iter()
            .find(|f| f.name == "Judy_later")
            .unwrap_or_else(|| panic!("{:?}", facts.functions));
        assert_eq!(
            (later.line, later.shape.clone()),
            (10, functions::BodyShape::Empty)
        );
    }

    #[test]
    fn helpers_are_followed_three_calls_deep_and_recursion_stops() {
        let v = AssertVocabulary::default();
        let asserts =
            |src: &str| CppPack.extract("tests/t.cc", src, &v).unwrap().tests[0].total_asserts;
        let require = "namespace {\nvoid Require(bool c) { if (!c) std::abort(); }\n";
        // main -> CheckA -> Require: the checks moved two levels down still count.
        let two = format!("{require}void CheckA() {{ Require(f()); Require(g()); }}\n}}  // namespace\nint main() {{ CheckA(); return 0; }}\n");
        assert_eq!(asserts(&two), 2);
        // Three levels is the limit; a fourth is not followed.
        let three = format!("{require}void CheckA() {{ Require(f()); }}\nvoid Suite() {{ CheckA(); }}\n}}\nint main() {{ Suite(); return 0; }}\n");
        assert_eq!(asserts(&three), 1);
        let four = format!("{require}void CheckA() {{ Require(f()); }}\nvoid Suite() {{ CheckA(); }}\nvoid All() {{ Suite(); }}\n}}\nint main() {{ All(); return 0; }}\n");
        assert_eq!(asserts(&four), 0);
        // Mutual recursion terminates and counts each helper once per path.
        let cycle = "void A(int n);\nvoid B(int n) { if (n) A(n - 1); assert(n >= 0); }\nvoid A(int n) { if (n) B(n - 1); }\nint main() { A(3); return 0; }\n";
        assert_eq!(asserts(cycle), 1);
        // A self-recursive helper's checks count once, not once per level.
        let recursive = "void Walk(int n) { assert(n >= 0); if (n) Walk(n - 1); }\nint main() { Walk(3); return 0; }\n";
        assert_eq!(asserts(recursive), 1);
    }

    #[test]
    fn c_headers_with_extern_c_guards_and_cpp_headers_parse() {
        let v = AssertVocabulary::default();
        let guarded = "#ifndef X_H\n#define X_H\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint x_open(const char *p);\n#ifdef __cplusplus\n}\n#endif\n#endif\n";
        assert_eq!(
            CPack
                .extract("include/x.h", guarded, &v)
                .unwrap()
                .skipped_error_nodes_count,
            0
        );
        let cpp = "#pragma once\n#include <string>\nnamespace db {\nclass Table {\n public:\n  explicit Table(std::string name);\n  bool Contains(const std::string& k) const;\n private:\n  std::string name_;\n};\n}  // namespace db\n";
        let facts = CPack.extract("include/table.h", cpp, &v).unwrap();
        assert_eq!(facts.skipped_error_nodes_count, 0);
        // A `.c` file is never re-read as C++.
        assert!(
            CPack
                .extract("src/table.c", cpp, &v)
                .unwrap()
                .skipped_error_nodes_count
                > 0
        );
    }

    #[test]
    fn a_test_main_calling_its_tests_through_a_table_counts_their_checks() {
        let v = AssertVocabulary::default();
        let asserts =
            |src: &str| CppPack.extract("tests/t.cc", src, &v).unwrap().tests[0].total_asserts;
        let tests =
            "void TestFirst() { assert(a()); assert(b()); }\nvoid TestSecond() { assert(c()); }\n";
        let direct = format!("{tests}int main() {{ TestFirst(); TestSecond(); return 0; }}\n");
        let table = format!("{tests}int main() {{\n  const std::vector<std::pair<std::string, void (*)()>> tests = {{{{\"first\", TestFirst}}, {{\"second\", &TestSecond}}}};\n  for (const auto& t : tests) t.second();\n  return 0;\n}}\n");
        assert_eq!(asserts(&direct), 3);
        assert_eq!(asserts(&table), 3);
        // A name in a table that is not a same-file helper counts nothing.
        let other = "int main() { const int codes[] = {ERR_A, ERR_B}; use(codes); return 0; }\n";
        assert_eq!(asserts(other), 0);
    }
}
