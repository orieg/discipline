//! C# language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn};

/// C# language pack implementing [`LanguagePack`].
pub struct CSharpPack;

impl LanguagePack for CSharpPack {
    fn id(&self) -> &'static str {
        "csharp"
    }

    fn name(&self) -> &'static str {
        "C#"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("cs"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the C# grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = CSharpExtractor {
            src: src.as_bytes(),
            vocab,
            is_test_path: is_csharp_test_path(path),
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

/// Determines whether a path is conventionally a C# test file.
pub fn is_csharp_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename.ends_with("Test.cs")
        || filename.ends_with("Tests.cs")
        || filename.ends_with("_test.cs")
        || filename.ends_with("_tests.cs")
        || filename.starts_with("Test")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
        || path.contains(".Tests/")
        || path.contains(".Test/")
}

struct CSharpExtractor<'a> {
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
}

impl<'a> CSharpExtractor<'a> {
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

            if trimmed.starts_with("#pragma warning disable")
                || trimmed.starts_with("pragma warning disable")
                || trimmed.starts_with("NOLINT")
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

        // C# preprocessor directive: pragma_directive / preproc_pragma
        if kind == "pragma_directive" || kind == "preproc_pragma" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            if text.contains("warning disable") {
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: text.trim().to_string(),
                        snippet: text.to_string(),
                    });
            }
        }

        // C# SuppressMessageAttribute on declarations
        if kind == "attribute" {
            let attr_name = node
                .child_by_field_name("name")
                .map(|n| self.text(n))
                .unwrap_or("");
            if attr_name == "SuppressMessage" || attr_name == "SuppressMessageAttribute" {
                let line = node.start_position().row + 1;
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: self.text(node).to_string(),
                        snippet: self.text(node).to_string(),
                    });
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments_and_escape_hatches(child);
        }
    }

    fn visit_root(&mut self, root: Node) {
        self.walk_scope(root, false);
    }

    fn walk_scope(&mut self, scope: Node, class_ignored: bool) {
        let mut cursor = scope.walk();
        for child in scope.children(&mut cursor) {
            let kind = child.kind();
            if matches!(
                kind,
                "class_declaration"
                    | "struct_declaration"
                    | "record_declaration"
                    | "interface_declaration"
            ) {
                let is_ignored = class_ignored || self.has_ignore_attribute(child);
                if let Some(body) = child.child_by_field_name("body") {
                    self.walk_scope(body, is_ignored);
                }
            } else if kind == "method_declaration" {
                if let Some(test_fn) = self.try_extract_method_test(child, class_ignored) {
                    self.facts.tests.push(test_fn);
                }
            } else {
                self.walk_scope(child, class_ignored);
            }
        }
    }

    fn has_ignore_attribute(&self, node: Node) -> bool {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "attribute_list" {
                let mut attr_cursor = child.walk();
                for attr in child.children(&mut attr_cursor) {
                    if attr.kind() == "attribute" {
                        let name = attr
                            .child_by_field_name("name")
                            .map(|n| self.text(n))
                            .unwrap_or("");
                        if name == "Ignore" || name == "IgnoreAttribute" {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn try_extract_method_test(&self, node: Node, class_ignored: bool) -> Option<TestFn> {
        let name_node = node.child_by_field_name("name")?;
        let method_name = self.text(name_node);

        let mut is_test = false;
        let mut is_ignored = class_ignored;

        // Inspect attributes on the method
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "attribute_list" {
                let mut attr_cursor = child.walk();
                for attr in child.children(&mut attr_cursor) {
                    if attr.kind() == "attribute" {
                        let attr_name = attr
                            .child_by_field_name("name")
                            .map(|n| self.text(n))
                            .unwrap_or("");

                        if matches!(
                            attr_name,
                            "Fact"
                                | "FactAttribute"
                                | "Theory"
                                | "TheoryAttribute"
                                | "Test"
                                | "TestAttribute"
                                | "TestCase"
                                | "TestCaseAttribute"
                                | "TestMethod"
                                | "TestMethodAttribute"
                        ) {
                            is_test = true;
                            // Check for xUnit Skip = "..." in attribute arguments
                            if let Some(args) = attr.child_by_field_name("parameters") {
                                let mut arg_cursor = args.walk();
                                for arg in args.children(&mut arg_cursor) {
                                    let txt = self.text(arg);
                                    if txt.contains("Skip") {
                                        is_ignored = true;
                                    }
                                }
                            } else {
                                // Sometimes arguments are children without field name
                                let mut arg_cursor = attr.walk();
                                for arg in attr.children(&mut arg_cursor) {
                                    if arg.kind() == "attribute_argument_list" {
                                        let txt = self.text(arg);
                                        if txt.contains("Skip") {
                                            is_ignored = true;
                                        }
                                    }
                                }
                            }
                        }

                        if matches!(
                            attr_name,
                            "Ignore" | "IgnoreAttribute" | "Skipped" | "SkippedAttribute"
                        ) {
                            is_ignored = true;
                        }
                    }
                }
            }
        }

        // Check naming convention in test files if no attributes present
        if !is_test
            && self.is_test_path
            && (method_name.starts_with("Test") || method_name.starts_with("test_"))
        {
            is_test = true;
        }

        if !is_test {
            return None;
        }

        let line = node.start_position().row + 1;
        let mut test_fn = TestFn {
            name: method_name.to_string(),
            line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: is_ignored,
            should_panic: false,
        };

        if let Some(body) = node.child_by_field_name("body") {
            self.extract_assertions_in_body(body, &mut test_fn);
        } else {
            // Check for expression-bodied method (arrow_expression_clause)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "arrow_expression_clause" {
                    self.extract_assertions_in_body(child, &mut test_fn);
                }
            }
        }

        Some(test_fn)
    }

    fn extract_assertions_in_body(&self, node: Node, test_fn: &mut TestFn) {
        let kind = node.kind();

        if kind == "invocation_expression" {
            let fn_node = node.child_by_field_name("function");
            if let Some(func) = fn_node {
                let (class_name, method_name) = self.inspect_invocation_target(func);

                if (class_name == "Assert" || class_name == "ClassicAssert")
                    && (method_name == "Skip" || method_name == "Ignore")
                {
                    test_fn.ignored = true;
                    return;
                }

                let args = self.get_invocation_arguments(node);

                if matches!(
                    class_name,
                    "Assert" | "StringAssert" | "CollectionAssert" | "ClassicAssert"
                ) {
                    self.handle_assert_call(method_name, &args, test_fn);
                    return;
                }

                if self.vocab.extra_macros.iter().any(|m| m == method_name)
                    || self.vocab.helper_fns.iter().any(|h| h == method_name)
                {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                    return;
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_assertions_in_body(child, test_fn);
        }
    }

    fn inspect_invocation_target(&self, node: Node) -> (&'a str, &'a str) {
        let kind = node.kind();
        if kind == "member_access_expression" {
            let expr = node
                .child_by_field_name("expression")
                .map(|e| self.text(e))
                .unwrap_or("");
            let raw_name = node
                .child_by_field_name("name")
                .map(|n| {
                    if n.kind() == "generic_name" {
                        if let Some(id) = n.child(0) {
                            return self.text(id);
                        }
                    }
                    self.text(n)
                })
                .unwrap_or("");
            let name = raw_name.split('<').next().unwrap_or(raw_name);
            return (expr, name);
        }
        if kind == "generic_name" {
            if let Some(id) = node.child(0) {
                return ("", self.text(id));
            }
        }
        if kind == "identifier" {
            return ("", self.text(node));
        }
        ("", self.text(node))
    }

    fn get_invocation_arguments<'b>(&self, invocation: Node<'b>) -> Vec<Node<'b>> {
        let Some(args_node) = invocation.child_by_field_name("arguments") else {
            return Vec::new();
        };
        let mut cursor = args_node.walk();
        args_node
            .children(&mut cursor)
            .filter(|c| c.is_named())
            .map(|c| {
                if c.kind() == "argument" {
                    c.child(0).unwrap_or(c)
                } else {
                    c
                }
            })
            .collect()
    }

    fn handle_assert_call(&self, method_name: &str, args: &[Node], test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;

        let is_strong = matches!(
            method_name,
            "Equal"
                | "NotEqual"
                | "StrictEqual"
                | "NotStrictEqual"
                | "Same"
                | "NotSame"
                | "Contains"
                | "DoesNotContain"
                | "Matches"
                | "DoesNotMatch"
                | "Throws"
                | "ThrowsAsync"
                | "ThrowsAny"
                | "ThrowsAnyAsync"
                | "Single"
                | "Empty"
                | "NotEmpty"
                | "InRange"
                | "NotInRange"
                | "IsType"
                | "IsNotType"
                | "Equivalent"
                | "AreEquivalent"
        );

        if is_strong {
            test_fn.strong_asserts += 1;
            // Equality tautology check: 2 args with identical text
            if matches!(
                method_name,
                "Equal" | "StrictEqual" | "Same" | "Equivalent" | "AreEquivalent"
            ) && args.len() >= 2
            {
                let left = self.text(args[0]).trim();
                let right = self.text(args[1]).trim();
                if !left.is_empty() && left == right {
                    test_fn.tautologies += 1;
                }
            }
        } else if method_name == "True" {
            if let Some(arg) = args.first() {
                if self.is_literal_true(*arg) || self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            }
        } else if method_name == "False" {
            if let Some(arg) = args.first() {
                if self.is_literal_false(*arg) || self.is_tautology_comparison(*arg) {
                    test_fn.tautologies += 1;
                }
            }
        }
    }

    fn is_literal_true(&self, node: Node) -> bool {
        let txt = self.text(node).trim();
        txt == "true" || node.kind() == "boolean_literal" && txt == "true"
    }

    fn is_literal_false(&self, node: Node) -> bool {
        let txt = self.text(node).trim();
        txt == "false" || node.kind() == "boolean_literal" && txt == "false"
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csharp_pack_registration_and_extension_matching() {
        let pack = CSharpPack;
        assert_eq!(pack.id(), "csharp");
        assert_eq!(pack.name(), "C#");
        assert!(pack.matches("ExpanseMapTests.cs"));
        assert!(pack.matches("tests/UnitTest.cs"));
        assert!(!pack.matches("test.cpp"));
        assert!(!pack.matches("test.java"));
    }

    #[test]
    fn test_xunit_test_extraction_and_assertions() {
        let src = r#"
using Xunit;

public class CalcTests
{
    [Fact]
    public void TestAdd()
    {
        Assert.Equal(3, 1 + 2);
        Assert.True(1 + 2 == 3);
        Assert.Throws<ArgumentException>(() => {});
    }

    [Theory]
    [InlineData(1, 2)]
    public void TestTheory(int a, int b)
    {
        Assert.Equal(a, b);
    }

    [Fact(Skip = "Temporarily disabled")]
    public void TestSkipped()
    {
        Assert.Equal(1, 1);
    }
}
"#;
        let pack = CSharpPack;
        let facts = pack
            .extract("tests/CalcTests.cs", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 3);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "TestAdd");
        assert_eq!(t0.total_asserts, 3);
        assert_eq!(t0.strong_asserts, 2); // Assert.Equal and Assert.Throws
        assert!(!t0.ignored);
        assert!(!t0.is_vacuous());

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "TestTheory");
        assert_eq!(t1.total_asserts, 1);
        assert_eq!(t1.strong_asserts, 1);
        assert!(!t1.ignored);

        let t2 = &facts.tests[2];
        assert_eq!(t2.name, "TestSkipped");
        assert!(t2.ignored);
    }

    #[test]
    fn test_expanse_dotnet_map_tests_fixture() {
        let src = r#"
using System;
using System.Collections.Generic;
using System.Linq;
using Xunit;

namespace Expanse.Tests;

public class ExpanseMapTests
{
    [Fact]
    public void BasicCrudAndIndexer()
    {
        using var map = new ExpanseMap();
        Assert.Equal(0, map.Count);
        Assert.True(map.IsEmpty);

        map[10] = 100;
        Assert.Equal(1, map.Count);
        Assert.Equal(100UL, map[10]);
        Assert.True(map.ContainsKey(10));
        Assert.False(map.ContainsKey(11));

        // Overwrite
        map[10] = 200;
        Assert.Equal(1, map.Count);
        Assert.Equal(200UL, map[10]);

        // Insert with oldValue tracking
        Assert.False(map.Insert(10, 300, out ulong oldVal));
        Assert.Equal(200UL, oldVal);
        Assert.Equal(300UL, map[10]);

        Assert.True(map.Insert(20, 400, out oldVal));
        Assert.Equal(2, map.Count);
    }
}
"#;
        let pack = CSharpPack;
        let facts = pack
            .extract(
                "bindings/dotnet/tests/Expanse.NET.Tests/ExpanseMapTests.cs",
                src,
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "BasicCrudAndIndexer");
        assert_eq!(t.line, 11);
        assert_eq!(t.total_asserts, 13);
        assert_eq!(t.strong_asserts, 8); // 8 Assert.Equal calls
        assert_eq!(t.tautologies, 0);
        assert_eq!(t.effective_asserts(), 13);
        assert!(!t.is_vacuous());
        assert!(!t.ignored);
    }

    #[test]
    fn test_nunit_and_mstest_extraction() {
        let src = r#"
using NUnit.Framework;

[TestFixture]
public class NUnitTests
{
    [Test]
    public void NUnitTest()
    {
        Assert.AreEqual(42, 42); // tautology
    }

    [Test]
    [Ignore("Skipped by attribute")]
    public void NUnitIgnored()
    {
        Assert.AreEqual(1, 2);
    }
}
"#;
        let pack = CSharpPack;
        let facts = pack
            .extract("tests/NUnitTests.cs", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 2);
        assert_eq!(facts.tests[0].name, "NUnitTest");
        assert!(facts.tests[1].ignored);
    }

    #[test]
    fn test_csharp_tautologies_and_vacuous_tests() {
        let src = r#"
public class VacuousTests
{
    [Fact]
    public void EmptyTest()
    {
    }

    [Fact]
    public void TautologyTest()
    {
        Assert.True(true);
        Assert.Equal(1, 1);
        Assert.Equal(x, x);
        Assert.True(y == y);
    }
}
"#;
        let pack = CSharpPack;
        let facts = pack
            .extract("tests/VacuousTests.cs", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 2);
        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "EmptyTest");
        assert_eq!(t0.total_asserts, 0);
        assert!(t0.is_vacuous());

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "TautologyTest");
        assert_eq!(t1.total_asserts, 4);
        assert_eq!(t1.tautologies, 4);
        assert_eq!(t1.effective_asserts(), 0);
        assert!(t1.is_vacuous());
    }

    #[test]
    fn test_csharp_escape_hatches_and_custom_vocab() {
        let src = r#"
#pragma warning disable CS8618
[System.Diagnostics.CodeAnalysis.SuppressMessage("Rule", "ID")]
public class Suppressed
{
    // #pragma warning disable CS0168
    [Fact]
    public void CustomTest()
    {
        CUSTOM_CHECK(42);
    }
}
"#;
        let vocab = AssertVocabulary {
            extra_macros: vec!["CUSTOM_CHECK".to_string()],
            ..Default::default()
        };
        let pack = CSharpPack;
        let facts = pack
            .extract("tests/Suppressed.cs", src, &vocab)
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "CustomTest");
        assert_eq!(t.total_asserts, 1);
        assert_eq!(t.strong_asserts, 1);

        assert!(!facts.escape_hatches.is_empty());
    }
}
