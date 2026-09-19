//! Go language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn};

/// Go language pack implementing [`LanguagePack`].
pub struct GoPack;

impl LanguagePack for GoPack {
    fn id(&self) -> &'static str {
        "go"
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
            src: src.as_bytes(),
            vocab,
            is_test_path: is_go_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        Ok(extractor.facts)
    }
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
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
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

            if trimmed.starts_with("nolint") || trimmed.starts_with("revive:disable") {
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

        let is_test_name = func_name.starts_with("Test") || func_name.starts_with("Fuzz");
        if !is_test_name && !self.is_test_path {
            return;
        }

        if !self.is_unit_test_signature(node) {
            return;
        }

        let line = node.start_position().row + 1;
        let mut test_fn = TestFn {
            name: func_name.to_string(),
            line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: false,
            should_panic: false,
        };

        if let Some(body) = node.child_by_field_name("body") {
            self.scan_block(body, &mut test_fn, func_name);
        }

        self.facts.tests.push(test_fn);
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
                    .map(|n| self.text(n))
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
                    .map(|n| self.text(n))
                    .unwrap_or("");
                if type_text.contains("testing.T")
                    || type_text.contains("testing.F")
                    || type_text.ends_with("*T")
                    || type_text.ends_with("*F")
                {
                    return true;
                }
            }
        }
        false
    }

    fn scan_block(&mut self, block: Node, test_fn: &mut TestFn, parent_name: &str) {
        let mut cursor = block.walk();
        for child in block.children(&mut cursor) {
            self.scan_node(child, test_fn, parent_name);
        }
    }

    fn scan_node(&mut self, node: Node, test_fn: &mut TestFn, parent_name: &str) {
        match node.kind() {
            "call_expression" => {
                self.inspect_call(node, test_fn, parent_name);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.scan_node(child, test_fn, parent_name);
                }
            }
        }
    }

    fn inspect_call(&mut self, node: Node, test_fn: &mut TestFn, parent_name: &str) {
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
            let mut sub_test = TestFn {
                name: sub_name.clone(),
                line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: test_fn.ignored,
                should_panic: false,
            };

            // Recursively scan callback body
            if let Some(func_lit) = args.iter().find(|a| a.kind() == "func_literal") {
                if let Some(sub_body) = func_lit.child_by_field_name("body") {
                    self.scan_block(sub_body, &mut sub_test, &sub_name);
                }
            }

            self.facts.tests.push(sub_test);
            return;
        }

        // Check for test skip: t.Skip(...), t.Skipf(...), t.SkipNow()
        if call_text.ends_with(".Skip")
            || call_text.ends_with(".Skipf")
            || call_text.ends_with(".SkipNow")
            || call_text == "Skip"
            || call_text == "Skipf"
        {
            test_fn.ignored = true;
            return;
        }

        // Standard testing methods: t.Fatalf, t.Fatal, t.Errorf, t.Error, t.FailNow
        if call_text.ends_with(".Fatalf")
            || call_text.ends_with(".Fatal")
            || call_text.ends_with(".FailNow")
            || call_text.ends_with(".Errorf")
            || call_text.ends_with(".Error")
        {
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
                "NotEqual" | "NotSame" | "NoError" | "Error" | "Contains" | "NotContains"
                | "Len" | "Panics" | "NotPanics" | "Nil" | "NotNil" | "ElementsMatch" => {
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

        // Recursively inspect child nodes
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child != func_node {
                self.scan_node(child, test_fn, parent_name);
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
}
