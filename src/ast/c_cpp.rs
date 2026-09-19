//! C and C++ language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn};

fn collect_error_nodes_info(root: Node) -> (bool, Option<usize>, usize) {
    if !root.has_error() {
        return (false, None, 0);
    }
    let mut first_line = None;
    let mut count = 0;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            count += 1;
            let line = node.start_position().row + 1;
            if first_line.is_none() || Some(line) < first_line {
                first_line = Some(line);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.has_error() || child.is_error() || child.is_missing() {
                stack.push(child);
            }
        }
    }
    (true, first_line.or(Some(1)), count.max(1))
}

/// C language pack implementing [`LanguagePack`].
pub struct CPack;

impl LanguagePack for CPack {
    fn id(&self) -> &'static str {
        "c"
    }

    fn name(&self) -> &'static str {
        "C"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("c" | "h"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the C grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let (has_errors, first_line, error_count) = collect_error_nodes_info(root);
        let mut extractor = CCppExtractor {
            src: src.as_bytes(),
            vocab,
            is_test_path: is_c_cpp_test_path(path),
            test_spans: Vec::new(),
            facts: ParsedFileFacts {
                has_parse_errors: has_errors,
                first_parse_error_line: first_line,
                skipped_error_nodes_count: error_count,
                ..Default::default()
            },
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        Ok(extractor.facts)
    }
}

/// C++ language pack implementing [`LanguagePack`].
pub struct CppPack;

impl LanguagePack for CppPack {
    fn id(&self) -> &'static str {
        "cpp"
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
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let (has_errors, first_line, error_count) = collect_error_nodes_info(root);
        let mut extractor = CCppExtractor {
            src: src.as_bytes(),
            vocab,
            is_test_path: is_c_cpp_test_path(path),
            test_spans: Vec::new(),
            facts: ParsedFileFacts {
                has_parse_errors: has_errors,
                first_parse_error_line: first_line,
                skipped_error_nodes_count: error_count,
                ..Default::default()
            },
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        Ok(extractor.facts)
    }
}

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
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    test_spans: Vec<std::ops::Range<usize>>,
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
        self.collect_compile_time_asserts(root);
        self.facts.build_compile_time_test();
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
                                let mut test_fn = TestFn {
                                    name,
                                    line,
                                    total_asserts: 0,
                                    strong_asserts: 0,
                                    tautologies: 0,
                                    ignored: is_ignored,
                                    should_panic: false,
                                    ..Default::default()
                                };
                                self.extract_assertions_in_body(body, &mut test_fn);
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

    fn try_extract_function_definition_test(&self, node: Node) -> Option<TestFn> {
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
            let mut test_fn = TestFn {
                name,
                line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: is_ignored,
                should_panic: false,
                ..Default::default()
            };
            self.extract_assertions_in_body(body, &mut test_fn);
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
            let mut test_fn = TestFn {
                name,
                line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: is_ignored,
                should_panic: false,
                ..Default::default()
            };
            self.extract_assertions_in_body(body, &mut test_fn);
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
            let mut test_fn = TestFn {
                name: fn_name.to_string(),
                line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: false,
                should_panic: false,
                ..Default::default()
            };
            self.extract_assertions_in_body(body, &mut test_fn);
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

    fn extract_assertions_in_body(&self, node: Node, test_fn: &mut TestFn) {
        let kind = node.kind();

        if kind == "static_assert_declaration" {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        if kind == "call_expression" {
            let fn_name = self.get_call_fn_name(node);

            // Skip detection inside test body
            if matches!(fn_name, "GTEST_SKIP" | "SKIP") {
                test_fn.ignored = true;
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
            self.extract_assertions_in_body(child, test_fn);
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
    fn test_c_abi_smoke_modern_api_expanse_fixture() {
        let src = r#"
/* Compile-and-run check of include/expanse.h against the built library. */
#include <expanse.h>
#include <stdio.h>
#include <string.h>
#include <assert.h>

int main(void) {
    printf("libexpanse %s\n", expanse_version());

    expanse_map_t *m = expanse_map_new();
    for (uint64_t k = 0; k < 1000; k++) {
        uint64_t *slot = expanse_map_ins_slot(m, k * 3);
        *slot = k;
    }
    uint64_t v = 0, key = 0;
    assert(expanse_map_get(m, 300, &v) && v == 100);
    assert(expanse_map_count_range(m, 0, 299) == 100);
    assert(expanse_map_by_count(m, 10, &key, &v) && key == 30 && v == 10);
    assert(expanse_map_next_after(m, 30, &key, NULL) && key == 33);
    printf("map: len=%llu mem=%zu rank/select ok\n",
           (unsigned long long) expanse_map_len(m), expanse_map_mem_used(m));
    expanse_map_free(m);

    expanse_sync_map_t *s = expanse_sync_map_new();
    expanse_sync_map_insert(s, 7, 77, NULL);
    expanse_sync_map_reader_t *r = expanse_sync_map_reader_new(s);
    assert(expanse_sync_map_reader_get(r, 7, &v) && v == 77);
    expanse_sync_map_reader_free(r);
    expanse_sync_map_free(s);
    printf("sync: concurrent reader ok\n");

    expanse_bytesmap_t *b = expanse_bytesmap_new();
    expanse_bytesmap_insert(b, "a\0b", 3, 5, NULL);
    assert(expanse_bytesmap_get(b, "a\0b", 3, &v) && v == 5);
    assert(!expanse_bytesmap_get(b, "a", 1, &v));
    expanse_bytesmap_free(b);
    printf("bytesmap: embedded NUL ok\nok\n");
    return 0;
}
"#;
        let c_pack = CPack;
        let facts = c_pack
            .extract(
                "crates/expanse-capi/smoke/modern_api_smoke.c",
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
}
