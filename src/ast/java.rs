//! Java language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn};

/// Java language pack implementing [`LanguagePack`].
pub struct JavaPack;

impl LanguagePack for JavaPack {
    fn id(&self) -> &'static str {
        "java"
    }

    fn name(&self) -> &'static str {
        "Java"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("java"))
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
            src: src.as_bytes(),
            vocab,
            is_test_path: is_java_test_path(path),
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

/// Determines whether a path is conventionally a Java test file.
pub fn is_java_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename.strip_suffix(".java").unwrap_or(filename);
    if stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.ends_with("TestCase")
        || stem.starts_with("Test")
    {
        return true;
    }
    path.starts_with("src/test/")
        || path.contains("/src/test/")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
}

struct JavaExtractor<'a> {
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
}

impl<'a> JavaExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        let kind = node.kind();
        if kind == "line_comment" || kind == "block_comment" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            let trimmed = text
                .trim_start_matches("//")
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim();

            if trimmed.contains("@SuppressWarnings") {
                let rule = if let Some(start) = trimmed.find('(') {
                    if let Some(end) = trimmed[start..].find(')') {
                        trimmed[start + 1..start + end]
                            .trim()
                            .trim_matches('"')
                            .to_string()
                    } else {
                        "all".to_string()
                    }
                } else {
                    "all".to_string()
                };
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule,
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
                let is_class_ignored = parent_ignored || class_has_disabled;

                class_stack.push(class_name);
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for child in body.children(&mut cursor) {
                        self.visit_node(child, class_stack, is_class_ignored);
                    }
                }
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

    fn check_modifiers_for_test_and_ignore(&self, node: Node) -> (bool, bool) {
        // Returns (is_ignored, is_test_annotated)
        let mut is_ignored = false;
        let mut is_test_annotated = false;

        let Some(modifiers) = Self::get_modifiers(node) else {
            return (false, false);
        };

        let mut cursor = modifiers.walk();
        for child in modifiers.children(&mut cursor) {
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

        if !is_test {
            return;
        }

        let full_name = if class_stack.is_empty() {
            method_name.to_string()
        } else {
            format!("{}.{}", class_stack.join("."), method_name)
        };

        let line = node.start_position().row + 1;
        let should_panic = self.has_expected_exception(node);

        let mut test_fn = TestFn {
            name: full_name,
            line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: parent_ignored || method_ignored,
            should_panic,
        };

        if let Some(body) = node.child_by_field_name("body") {
            self.scan_method_body(body, &mut test_fn);
        }

        self.facts.tests.push(test_fn);
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

    fn has_expected_exception(&self, node: Node) -> bool {
        if let Some(modifiers) = Self::get_modifiers(node) {
            let mut cursor = modifiers.walk();
            for child in modifiers.children(&mut cursor) {
                if child.kind() == "annotation" {
                    let text = self.text(child);
                    if text.contains("expected =") || text.contains("expected=") {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn scan_method_body(&self, body: Node, test_fn: &mut TestFn) {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            self.scan_statement_or_expr(child, test_fn);
        }
    }

    fn scan_statement_or_expr(&self, node: Node, test_fn: &mut TestFn) {
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
                self.inspect_method_invocation(node, test_fn);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.scan_statement_or_expr(child, test_fn);
                }
            }
        }
    }

    fn inspect_method_invocation(&self, node: Node, test_fn: &mut TestFn) {
        let method_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");

        let args_node = node.child_by_field_name("arguments");
        let args = Self::collect_arguments(args_node);

        match method_name {
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
                // Check if user configured this helper function
                if self.vocab.helper_fns.iter().any(|h| h == other) {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                }
            }
        }

        // Recursively check children (e.g. arguments or object call chains)
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child != args_node.unwrap_or(child) {
                self.scan_statement_or_expr(child, test_fn);
            } else {
                // Also scan inside arguments for lambdas or nested assert calls (like assertThrows(() -> ...))
                let mut arg_cursor = child.walk();
                for arg_child in child.children(&mut arg_cursor) {
                    self.scan_statement_or_expr(arg_child, test_fn);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_junit5_test_extraction_and_assertion_counting() {
        let src = r#"
package io.github.orieg.expanse;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class ExpanseMapTest {
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
            .extract("ExpanseMapTest.java", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "ExpanseMapTest.testSlotSegment");
        assert_eq!(t.total_asserts, 4);
        assert_eq!(t.strong_asserts, 2); // assertEquals + assertThrows
        assert_eq!(t.tautologies, 2); // assertTrue(true) + assertFalse(false)
        assert_eq!(t.effective_asserts(), 2);
        assert!(!t.is_vacuous());
        assert!(!t.ignored);
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

    #[test]
    fn test_assertj_and_custom_vocab() {
        let src = r#"
import static org.assertj.core.api.Assertions.assertThat;
import org.junit.jupiter.api.Test;

class AssertJTest {
    @Test
    void testAssertJ() {
        assertThat("foo").isEqualTo("foo");
        customAssertHelper(42);
    }
}
"#;
        let vocab = AssertVocabulary {
            helper_fns: vec!["customAssertHelper".to_string()],
            ..Default::default()
        };
        let pack = JavaPack;
        let facts = pack
            .extract("AssertJTest.java", src, &vocab)
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.total_asserts, 2);
        assert_eq!(t.strong_asserts, 2);
        assert!(!t.is_vacuous());
    }
}
