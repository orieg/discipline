//! Go language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Go language pack implementing [`LanguagePack`].
pub struct GoPack;

impl LanguagePack for GoPack {
    fn id(&self) -> &'static str {
        "go"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Go"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("go"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Go grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = GoExtractor {
            dead: super::reach::dead_ranges(root, src, &GO_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_go_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &GO_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &GO_MOCKS,
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
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &GO_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &GO_RETRIES);
        if super::functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &GO_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &GO_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        super::bounds::go(root, src, &mut extractor.facts.tests);
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "comment",
                "interpreted_string_literal",
                "raw_string_literal",
            ],
        );
        Ok(extractor.facts)
    }
}

/// Determines whether a function name matches the Go test runner's convention (TestXxx or FuzzXxx).
pub fn is_go_test_function_name(name: &str) -> bool {
    for prefix in &["Test", "Fuzz"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if rest.is_empty() {
                return true;
            }
            if let Some(first) = rest.chars().next() {
                if !first.is_lowercase() {
                    return true;
                }
            }
        }
    }
    false
}

/// Determines whether a path is conventionally a Go test file.
pub fn is_go_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename.ends_with("_test.go")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
}

struct GoExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

impl<'a> GoExtractor<'a> {
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

            if trimmed.starts_with("nolint")
                || trimmed.starts_with("lint:ignore")
                || trimmed.starts_with("revive:disable")
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
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "function_declaration" {
                self.visit_function(child);
            }
        }
    }

    fn visit_function(&mut self, node: Node) {
        let name_node = node.child_by_field_name("name");
        let func_name = name_node.map(|n| self.text(n)).unwrap_or("");

        // Benchmarks (BenchmarkXxx taking *testing.B) are performance harnesses, not assertion-bearing unit tests.
        if func_name.starts_with("Benchmark") || self.is_benchmark_signature(node) {
            return;
        }

        let is_test = is_go_test_function_name(func_name) && self.is_unit_test_signature(node);

        if is_test {
            let line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            let mut test_fn = TestFn {
                name: func_name.to_string(),
                line,
                end_line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: false,
                should_panic: false,
                ..Default::default()
            };

            let mut direct_calls = Vec::new();
            if let Some(body) = node.child_by_field_name("body") {
                self.scan_block(body, &mut test_fn, func_name, &mut direct_calls);
                super::dispatch_calls(body, self.src, &GO_DISPATCH, &mut direct_calls);
            }

            self.facts.tests.push(test_fn);
            self.test_calls.push(direct_calls);
        } else if self.is_test_path {
            if let Some(body) = node.child_by_field_name("body") {
                let mut helper_fn = TestFn::default();
                let mut dummy_calls = Vec::new();
                self.scan_block(body, &mut helper_fn, func_name, &mut dummy_calls);
                helper_fn.total_asserts += super::count_failure_exits(
                    body,
                    self.src,
                    &["call_expression"],
                    &["panic("],
                    &["func_literal"],
                );
                let facts = super::HelperFacts {
                    total_asserts: helper_fn.total_asserts,
                    strong_asserts: helper_fn.strong_asserts,
                    tautologies: helper_fn.tautologies,
                    fatal_asserts: helper_fn.fatal_asserts,
                };
                self.helpers.insert(func_name.to_string(), facts);
            }
        }
    }

    fn is_benchmark_signature(&self, func_node: Node) -> bool {
        let Some(params) = func_node.child_by_field_name("parameters") else {
            return false;
        };
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            if child.kind() == "parameter_declaration" {
                let type_text = child
                    .child_by_field_name("type")
                    .map(|n| self.text(n).trim())
                    .unwrap_or("");
                if type_text.contains("testing.B") || type_text.ends_with("*B") {
                    return true;
                }
            }
        }
        false
    }

    fn is_unit_test_signature(&self, func_node: Node) -> bool {
        let Some(params) = func_node.child_by_field_name("parameters") else {
            return false;
        };
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            if child.kind() == "parameter_declaration" {
                let type_text = child
                    .child_by_field_name("type")
                    .map(|n| self.text(n).trim())
                    .unwrap_or("");
                if type_text.ends_with("testing.TB")
                    || type_text.ends_with("*testing.TB")
                    || type_text == "TB"
                    || type_text == "*TB"
                {
                    return false;
                }
                if type_text.ends_with("testing.T")
                    || type_text.ends_with("*testing.T")
                    || type_text.ends_with("testing.F")
                    || type_text.ends_with("*testing.F")
                    || type_text == "*T"
                    || type_text == "*F"
                    || type_text == "T"
                    || type_text == "F"
                {
                    return true;
                }
            }
        }
        false
    }

    fn scan_block(
        &mut self,
        block: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        let mut cursor = block.walk();
        for child in block.children(&mut cursor) {
            self.scan_node(child, test_fn, parent_name, direct_calls);
        }
    }

    fn scan_node(
        &mut self,
        node: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "call_expression" => {
                self.inspect_call(node, test_fn, parent_name, direct_calls);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.scan_node(child, test_fn, parent_name, direct_calls);
                }
            }
        }
    }

    fn inspect_call(
        &mut self,
        node: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        let Some(func_node) = node.child_by_field_name("function") else {
            return;
        };

        let call_text = self.text(func_node);
        let args_node = node.child_by_field_name("arguments");
        let args = Self::collect_arguments(args_node);

        // Subtests: t.Run("subtest", func(t *testing.T) { ... })
        if (call_text.ends_with(".Run") || call_text == "Run") && args.len() >= 2 {
            let sub_title = self.extract_string_literal(args[0]);
            let sub_name = if sub_title.is_empty() {
                format!("{}/subtest", parent_name)
            } else {
                format!("{}/{}", parent_name, sub_title)
            };

            let line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            let mut sub_test = TestFn {
                name: sub_name.clone(),
                line,
                end_line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: test_fn.ignored,
                should_panic: false,
                ..Default::default()
            };

            let mut sub_calls = Vec::new();
            // Recursively scan callback body
            if let Some(func_lit) = args.iter().find(|a| a.kind() == "func_literal") {
                if let Some(sub_body) = func_lit.child_by_field_name("body") {
                    self.scan_block(sub_body, &mut sub_test, &sub_name, &mut sub_calls);
                    super::dispatch_calls(sub_body, self.src, &GO_DISPATCH, &mut sub_calls);
                }
            }

            self.facts.tests.push(sub_test);
            self.test_calls.push(sub_calls);
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        // Check for test skip: t.Skip(...), t.Skipf(...), t.SkipNow()
        if call_text.ends_with(".Skip")
            || call_text.ends_with(".Skipf")
            || call_text.ends_with(".SkipNow")
            || call_text == "Skip"
            || call_text == "Skipf"
        {
            // `if testing.Short() { t.Skip(...) }` runs in a full run: a conditional skip,
            // reported as a note. A constant condition skips every run.
            match enclosing_if_condition(node, self.src) {
                Some(cond) if cond != "true" => test_fn.conditional_ignore = Some(cond),
                _ => test_fn.ignored = true,
            }
            return;
        }

        // Standard testing methods: t.Fatalf, t.Fatal, t.Errorf, t.Error, t.FailNow
        if call_text.ends_with(".Fatalf")
            || call_text.ends_with(".Fatal")
            || call_text.ends_with(".FailNow")
        {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            test_fn.fatal_asserts += 1;
            return;
        }
        if call_text.ends_with(".Errorf") || call_text.ends_with(".Error") {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        if call_text.ends_with(".Fail") {
            test_fn.total_asserts += 1;
            // Weak assertion
            return;
        }

        // Testify assert / require
        let method_name = call_text.rsplit('.').next().unwrap_or(call_text);
        if call_text.starts_with("assert.") || call_text.starts_with("require.") {
            let is_require = call_text.starts_with("require.");
            if is_require {
                test_fn.fatal_asserts += 1;
            }
            match method_name {
                "True" => {
                    test_fn.total_asserts += 1;
                    // First arg is typically `t`, second is condition
                    if args.len() >= 2 && self.text(args[1]).trim() == "true" {
                        test_fn.tautologies += 1;
                    }
                }
                "False" => {
                    test_fn.total_asserts += 1;
                    if args.len() >= 2 && self.text(args[1]).trim() == "false" {
                        test_fn.tautologies += 1;
                    }
                }
                "Equal" | "Same" => {
                    test_fn.total_asserts += 1;
                    if args.len() >= 3 {
                        let a = self.text(args[1]).trim();
                        let b = self.text(args[2]).trim();
                        if a == b {
                            test_fn.tautologies += 1;
                        } else {
                            test_fn.strong_asserts += 1;
                        }
                    } else {
                        test_fn.strong_asserts += 1;
                    }
                }
                "Nil" | "NotNil" => {
                    test_fn.total_asserts += 1;
                }
                "NotEqual" | "NotSame" | "NoError" | "Error" | "Contains" | "NotContains"
                | "Len" | "Panics" | "NotPanics" | "ElementsMatch" => {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
                _ => {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
            }
            return;
        }

        // Configured helper functions
        if self.vocab.helper_fns.iter().any(|h| h == method_name) {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        // Track potential call to helper
        if func_node.kind() == "identifier" {
            let id_text = self.text(func_node);
            direct_calls.push(id_text.to_string());
        }

        // Recursively inspect child nodes
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child != func_node {
                self.scan_node(child, test_fn, parent_name, direct_calls);
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
                        if h.total_asserts > h.tautologies {
                            test.helper_checks += 1;
                        }
                    }
                }
            }
        }
    }

    fn collect_arguments(args_node: Option<Node>) -> Vec<Node> {
        let mut result = Vec::new();
        let Some(args) = args_node else {
            return result;
        };
        for i in 0..args.child_count() {
            if let Some(c) = args.child(i) {
                if c.is_named() {
                    result.push(c);
                }
            }
        }
        result
    }

    fn extract_string_literal(&self, node: Node) -> String {
        let t = self.text(node).trim();
        if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('`') && t.ends_with('`')) {
            t[1..t.len() - 1].to_string()
        } else {
            t.to_string()
        }
    }
}

fn go_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    path.ends_with("_test.go") || is_go_test_function_name(name)
}

/// The condition of the nearest `if` whose body holds `node`, stopping at the function
/// the call is in; `None` when the call runs unconditionally.
fn enclosing_if_condition(node: Node, src: &[u8]) -> Option<String> {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "function_declaration" | "method_declaration" | "func_literal" => return None,
            "if_statement" => {
                let cond = p.child_by_field_name("condition")?;
                let in_body = p
                    .child_by_field_name("consequence")
                    .is_some_and(|c| c.id() == cur.id());
                if in_body {
                    return cond.utf8_text(src).ok().map(|t| t.trim().to_string());
                }
            }
            _ => {}
        }
        cur = p;
    }
    None
}

pub const GO_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["function_declaration", "method_declaration"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    skip: functions::skip_none,
    is_test: go_fn_is_test,
    classify: functions::classify_go,
};

pub const GO_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
};

pub const GO_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &[],
    arm_of: &[],
    body_fields: &[],
    ignored_kinds: &["comment"],
    trivial: &[],
    discard_kinds: &["assignment_statement", "short_var_declaration"],
    discards: super::handlers::go_discards,
    classify_discard: Some(super::handlers::go_discard_class),
    call_value_kinds: &[],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
};

pub const GO_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["call_expression"],
};

pub const GO_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    // A Go block holds its statements in a `statement_list`.
    block_kinds: &["statement_list"],
    ignored_kinds: &["comment"],
    terminators: &["return", "panic(", "t.FailNow()", "os.Exit("],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const GO_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["literal_value"],
    names: &["identifier"],
    references: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_go_std_test_extraction_and_assertions() {
        let src = r#"
package main

import "testing"

func TestCalculator(t *testing.T) {
	got := 1 + 1
	if got != 2 {
		t.Fatalf("expected 2, got %d", got)
	}
	if 2 + 2 != 4 {
		t.Errorf("expected 4")
	}
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("calc_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "TestCalculator");
        assert_eq!(t.total_asserts, 2);
        assert_eq!(t.strong_asserts, 2);
        assert!(!t.is_vacuous());
        assert!(!t.ignored);
    }

    #[test]
    fn a_skip_under_an_if_is_conditional_and_an_unconditional_one_is_ignored() {
        let src = "package p\n\nimport \"testing\"\n\nfunc TestShort(t *testing.T) {\n\tif testing.Short() {\n\t\tt.Skip(\"short mode\")\n\t}\n\tif 1+1 != 2 {\n\t\tt.Fatal(\"math\")\n\t}\n}\n\nfunc TestAlways(t *testing.T) {\n\tt.Skip(\"later\")\n}\n\nfunc TestConstant(t *testing.T) {\n\tif true {\n\t\tt.Skip(\"later\")\n\t}\n}\n";
        let facts = GoPack
            .extract("p_test.go", src, &AssertVocabulary::default())
            .unwrap();
        let by = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();
        let short = by("TestShort");
        assert!(!short.ignored);
        assert_eq!(short.conditional_ignore.as_deref(), Some("testing.Short()"));
        assert!(by("TestAlways").ignored && by("TestAlways").conditional_ignore.is_none());
        assert!(by("TestConstant").ignored && by("TestConstant").conditional_ignore.is_none());
    }

    #[test]
    fn test_go_vacuous_test_detected() {
        let src = r#"
package main

import "testing"

func TestEmpty(t *testing.T) {
	// does nothing
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("empty_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        assert!(facts.tests[0].is_vacuous());
    }

    #[test]
    fn test_go_test_skip_detected() {
        let src = r#"
package main

import "testing"

func TestSkipped(t *testing.T) {
	t.Skip("skipping for now")
	if 1 != 2 {
		t.Fatal("fail")
	}
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("skip_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        assert!(facts.tests[0].ignored);
    }

    #[test]
    fn test_go_testify_and_tautologies() {
        let src = r#"
package main

import (
	"testing"
	"github.com/stretchr/testify/assert"
)

func TestTestify(t *testing.T) {
	assert.Equal(t, 2, 1+1)
	assert.True(t, true) // tautology
	assert.Equal(t, 42, 42) // tautology
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("testify_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.total_asserts, 3);
        assert_eq!(t.strong_asserts, 1);
        assert_eq!(t.tautologies, 2);
        assert_eq!(t.effective_asserts(), 1);
        assert!(!t.is_vacuous());
    }

    #[test]
    fn test_go_subtests_with_t_run() {
        let src = r#"
package main

import "testing"

func TestSuite(t *testing.T) {
	t.Run("subA", func(t *testing.T) {
		t.Fatal("fail")
	})
	t.Run("subB", func(t *testing.T) {
		t.Skip("skip B")
	})
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("sub_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 3);
        assert_eq!(facts.tests[0].name, "TestSuite/subA");
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert!(!facts.tests[0].ignored);

        assert_eq!(facts.tests[1].name, "TestSuite/subB");
        assert!(facts.tests[1].ignored);

        assert_eq!(facts.tests[2].name, "TestSuite");
    }

    #[test]
    fn test_go_benchmarks_distinguished_from_unit_tests() {
        let src = r#"
package main

import "testing"

func TestReal(t *testing.T) {
	t.Fatal("fail")
}

func BenchmarkSearch(b *testing.B) {
	for i := 0; i < b.N; i++ {
		// Benchmark timing loop without assertions
	}
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("bench_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(
            facts.tests.len(),
            1,
            "only TestReal must be extracted as a test"
        );
        assert_eq!(facts.tests[0].name, "TestReal");
    }

    #[test]
    fn test_go_testify_require_fatal_and_nil_checks() {
        let src = r#"
package main

import (
	"testing"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestFatalAndNil(t *testing.T) {
	x := 1
	y := 2
	assert.NotNil(t, t)       // weak assert (not strong), not fatal
	require.NotNil(t, t)      // weak assert (not strong), fatal
	assert.Equal(t, 1, x)     // strong assert, not fatal
	require.Equal(t, 2, y)    // strong assert, fatal
	t.Fatalf("fatal abort")   // strong assert, fatal
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("require_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.total_asserts, 5);
        // NotNil are 2 weak asserts, 2 Equal + 1 Fatalf are 3 strong
        assert_eq!(t.strong_asserts, 3);
        // require.NotNil + require.Equal + t.Fatalf are 3 fatal
        assert_eq!(t.fatal_asserts, 3);
    }

    #[test]
    fn test_go_helper_functions_and_same_file_resolution() {
        let src = r#"
package main

import "testing"

func assertValue(t *testing.T, got, want int) {
	if got != want {
		t.Fatalf("expected %d, got %d", want, got)
	}
}

func testHelperUnused(_ testing.TB) {
	// helper not starting with Test
}

func TestCaller(t *testing.T) {
	assertValue(t, 1+1, 2)
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("caller_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(
            facts.tests.len(),
            1,
            "only TestCaller must be extracted as a test; assertValue and testHelperUnused are helpers"
        );
        let t = &facts.tests[0];
        assert_eq!(t.name, "TestCaller");
        assert_eq!(t.total_asserts, 1, "must resolve helper assertion");
        assert_eq!(t.strong_asserts, 1);
        assert_eq!(t.fatal_asserts, 1);
        assert!(!t.is_vacuous());
    }
}
