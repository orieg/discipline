//! C# language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::Result;
use tree_sitter::Node;

use super::ci_condition::{read_skip, Grammar, SkipRead};
use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// C# language pack implementing [`LanguagePack`].
pub struct CSharpPack;

impl LanguagePack for CSharpPack {
    fn id(&self) -> &'static str {
        "csharp"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "C#"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("cs"))
    }

    fn is_test_path(&self, path: &str) -> bool {
        super::functions::is_test_file(path, Some(is_csharp_test_path))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let tree = crate::ast::source_text::parse_file_as(
            &tree_sitter_c_sharp::LANGUAGE.into(),
            "the C#",
            path,
            src,
        )?;
        let root = tree.root_node();

        let (has_errors, first_line, error_count) = super::collect_error_nodes_info(root);
        let mut extractor = CSharpExtractor {
            dead: super::reach::dead_ranges(root, src, &CS_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_csharp_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: has_errors,
                first_parse_error_line: first_line,
                skipped_error_nodes_count: error_count,
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers();
        CSHARP_PACK.shared_facts(root, src, path, vocab, &mut extractor.facts);
        super::caught_assertions::csharp(root, src, &mut extractor.facts.tests, vocab);
        super::expected_exceptions::csharp(root, src, &mut extractor.facts.tests);
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "comment",
                "string_literal",
                "verbatim_string_literal",
                "raw_string_literal",
            ],
        );
        Ok(extractor.facts)
    }
}

/// Determines whether a path is conventionally a C# test file.
pub fn is_csharp_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename.strip_suffix(".cs").unwrap_or(filename);
    super::functions::ends_with_word(stem, "Test")
        || super::functions::ends_with_word(stem, "Tests")
        || super::functions::starts_with_test_word(stem)
        || super::functions::has_dir(path, "test", true)
        || super::functions::has_dir(path, "tests", true)
        || path.contains(".Tests/")
        || path.contains(".Test/")
}

struct CSharpExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

/// The comments that suppress a C# analyser; a `#pragma` directive and a
/// `SuppressMessage` attribute are read from their own nodes.
const CS_SUPPRESSIONS: super::CommentSuppressions = super::CommentSuppressions {
    hash_comments: false,
    markers: &[
        "#pragma warning disable",
        "pragma warning disable",
        "NOLINT",
    ],
};

impl<'a> CSharpExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        let src = self.src;
        super::collect_comment_suppressions(
            node,
            src,
            &CS_SUPPRESSIONS,
            &mut |node, sites| {
                let kind = node.kind();
                let text = node.utf8_text(src).unwrap_or("");
                let line = node.start_position().row + 1;
                // C# preprocessor directive: pragma_directive / preproc_pragma
                if (kind == "pragma_directive" || kind == "preproc_pragma")
                    && text.contains("warning disable")
                {
                    sites.push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: text.trim().to_string(),
                        snippet: text.to_string(),
                    });
                }
                // C# SuppressMessageAttribute on declarations
                if kind == "attribute" {
                    let attr_name = node
                        .child_by_field_name("name")
                        .map(|n| n.utf8_text(src).unwrap_or(""))
                        .unwrap_or("");
                    if attr_name == "SuppressMessage" || attr_name == "SuppressMessageAttribute" {
                        sites.push(EscapeHatchSite::LinterDisable {
                            line,
                            rule: text.to_string(),
                            snippet: text.to_string(),
                        });
                    }
                }
                true
            },
            &mut self.facts.escape_hatches,
        );
    }

    fn visit_root(&mut self, root: Node) {
        self.walk_scope(root, false);
    }

    /// Reads every declaration under `root` in source order. The scopes still open are
    /// kept in a list, not on the thread's stack, so a deep tree costs the walk no stack
    /// (`source_text::TREE_DEPTH_LIMIT`).
    fn walk_scope(&mut self, root: Node, class_ignored: bool) {
        fn children(scope: Node) -> std::vec::IntoIter<Node> {
            let mut cursor = scope.walk();
            scope.children(&mut cursor).collect::<Vec<_>>().into_iter()
        }
        let mut open = vec![(children(root), class_ignored)];
        while let Some((rest, class_ignored)) = open.last_mut() {
            let class_ignored = *class_ignored;
            let Some(child) = rest.next() else {
                open.pop();
                continue;
            };
            if let Some((scope, ignored)) = self.read_member(child, class_ignored) {
                open.push((children(scope), ignored));
            }
        }
    }

    /// Reads one child of a scope. `Some` is a scope under it to read next, and whether
    /// the class around that scope is ignored.
    fn read_member<'t>(
        &mut self,
        child: Node<'t>,
        class_ignored: bool,
    ) -> Option<(Node<'t>, bool)> {
        let kind = child.kind();
        if matches!(
            kind,
            "class_declaration"
                | "struct_declaration"
                | "record_declaration"
                | "interface_declaration"
        ) {
            let is_ignored = class_ignored || self.has_ignore_attribute(child);
            child
                .child_by_field_name("body")
                .map(|body| (body, is_ignored))
        } else if kind == "method_declaration" {
            let name_node = child.child_by_field_name("name");
            let method_name = name_node.map(|n| self.text(n)).unwrap_or("");
            if let Some((test_fn, calls)) = self.try_extract_method_test(child, class_ignored) {
                self.facts.tests.push(test_fn);
                self.test_calls.push(calls);
            } else if self.is_test_path {
                let mut helper_fn = TestFn::default();
                let mut dummy_calls = Vec::new();
                let mut wrap_body = None;
                if let Some(body) = child.child_by_field_name("body") {
                    wrap_body = Some(body);
                    self.extract_assertions_in_body(body, &mut helper_fn, &mut dummy_calls);
                } else {
                    let mut cursor = child.walk();
                    for c in child.children(&mut cursor) {
                        if c.kind() == "arrow_expression_clause" {
                            wrap_body = Some(c);
                            self.extract_assertions_in_body(c, &mut helper_fn, &mut dummy_calls);
                        }
                    }
<<<<<<< HEAD
=======
                    helper_fn.total_asserts += super::count_failure_exits(
                        child,
                        self.src,
                        &["throw_statement", "throw_expression"],
                        &[],
                        &["lambda_expression", "local_function_statement"],
                    );
                    let facts = super::HelperFacts::from_scan(
                        &helper_fn,
                        wrap_body.and_then(|b| {
                            super::forwarding_wrapper_callee(
                                b,
                                &CS_WRAPPER,
                                &CS_LOCALS,
                                &dummy_calls,
                                self.src,
                            )
                        }),
                    );
                    self.helpers.insert(method_name.to_string(), facts);
                    let line = child.start_position().row + 1;
                    let end_line = child.end_position().row + 1;
                    self.facts.push_helper(
                        super::TestHelperFacts::from_scan(
                            method_name.to_string(),
                            line,
                            end_line,
                            &helper_fn,
                        ),
                        dummy_calls,
                    );
>>>>>>> origin/main
                }
                helper_fn.total_asserts += super::count_failure_exits(
                    child,
                    self.src,
                    &["throw_statement", "throw_expression"],
                    &[],
                    &["lambda_expression", "local_function_statement"],
                );
                let facts = super::HelperFacts::from_scan(
                    &helper_fn,
                    wrap_body.and_then(|b| {
                        super::forwarding_wrapper_callee(
                            b,
                            &CS_WRAPPER,
                            &CS_LOCALS,
                            &dummy_calls,
                            self.src,
                        )
                    }),
                );
                self.helpers.insert(method_name.to_string(), facts);
                let line = child.start_position().row + 1;
                let end_line = child.end_position().row + 1;
                self.facts.push_helper(
                    super::TestHelperFacts::from_scan(
                        method_name.to_string(),
                        line,
                        end_line,
                        &helper_fn,
                    ),
                    dummy_calls,
                );
            }
            None
        } else {
            Some((child, class_ignored))
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

    /// What an xUnit attribute with `Skip = ".."` and `SkipWhen = nameof(X)` or
    /// `SkipUnless = nameof(X)` does: a skip under the member of this file it names.
    /// Returns whether the attribute has an argument named `Skip`, and the conditional
    /// skip when it carries such a pair. The argument is found by its name in the tree:
    /// a display name whose text contains `Skip` is not one.
    fn attribute_skip_condition(&self, attr: Node) -> (bool, Option<SkipRead>) {
        let mut cursor = attr.walk();
        let Some(list) = attr
            .children(&mut cursor)
            .find(|c| c.kind() == "attribute_argument_list")
        else {
            return (false, None);
        };
        let mut skips = false;
        let mut condition = None;
        let mut cursor = list.walk();
        for arg in list.named_children(&mut cursor) {
            let name = arg
                .child_by_field_name("name")
                .map(|n| self.text(n))
                .unwrap_or("");
            let mut inner = arg.walk();
            let value = arg.named_children(&mut inner).last();
            match (name, value) {
                ("Skip", _) => skips = true,
                ("SkipWhen", Some(value)) => condition = Some((value, false)),
                ("SkipUnless", Some(value)) => condition = Some((value, true)),
                _ => {}
            }
        }
        let read = condition
            .filter(|_| skips)
            .map(|condition| read_skip(Grammar::CSharp, attr, Some(condition), self.src));
        (skips, read)
    }

    /// The condition a call that skips under one takes, with whether the test runs when
    /// it holds: NUnit `Assume.That(c)`, xUnit `Assert.SkipWhen(c, ..)` and
    /// `Assert.SkipUnless(c, ..)`, `Skip.If(c)` and `Skip.IfNot(c)`. An `Assume.That`
    /// with a constraint is read through its whole argument list: a CI variable in it
    /// decides the skip in a way that is not read further.
    fn condition_taking_skip<'n>(
        &self,
        call: Node<'n>,
        class_name: &str,
        method_name: &str,
    ) -> Option<(Node<'n>, bool)> {
        let list = call.child_by_field_name("arguments")?;
        let mut cursor = list.walk();
        let args: Vec<Node<'n>> = list
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "argument")
            .collect();
        let first = *args.first()?;
        match (class_name, method_name) {
            ("Assume", "That") => {
                let plain = args.get(1).is_none_or(|second| {
                    second
                        .named_child(0)
                        .is_some_and(|v| v.kind() == "string_literal")
                });
                let constraint = args.get(1).map(|a| self.text(*a).trim()).unwrap_or("");
                match constraint {
                    _ if plain => Some((first, true)),
                    "Is.True" | "Is.Not.Null" => Some((first, true)),
                    "Is.False" | "Is.Null" => Some((first, false)),
                    _ => Some((list, false)),
                }
            }
            ("Assert", "SkipWhen") | ("Skip", "If") => Some((first, false)),
            ("Assert", "SkipUnless") | ("Skip", "IfNot") => Some((first, true)),
            _ => None,
        }
    }

    fn try_extract_method_test(
        &self,
        node: Node,
        class_ignored: bool,
    ) -> Option<(TestFn, Vec<String>)> {
        let name_node = node.child_by_field_name("name")?;
        let method_name = self.text(name_node);

        let mut is_test = false;
        let mut is_ignored = class_ignored;
        let mut attribute_skips = Vec::new();

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
                        // `[NUnit.Framework.Test]` names the same attribute as `[Test]`.
                        let attr_name = attr_name.rsplit('.').next().unwrap_or(attr_name);
                        let attr_name = attr_name.strip_suffix("Attribute").unwrap_or(attr_name);

                        if matches!(
                            attr_name,
                            "Fact"
                                | "Theory"
                                | "Test"
                                | "TestCase"
                                | "TestCaseSource"
                                | "TestMethod"
                                | "DataTestMethod"
                        ) {
                            is_test = true;
                            // xUnit `Skip = "..."` skips the test; with `SkipWhen` or
                            // `SkipUnless` beside it, under the condition they name.
                            match self.attribute_skip_condition(attr) {
                                (_, Some(read)) => attribute_skips.push(read),
                                (true, None) => is_ignored = true,
                                (false, None) => {}
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

        // xUnit, NUnit and MSTest discover tests by attribute only: a method
        // named `TestConnection` without one is a helper, never run as a test.
        if !is_test {
            return None;
        }

        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let (cases, non_literal_cases, case_rows) =
            super::test_cases::extract_csharp_cases(node, self.src).into_parts();
        let mut test_fn = TestFn {
            name: method_name.to_string(),
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: is_ignored,
            should_panic: None,
            cases,
            non_literal_cases,
            case_rows,
            ..Default::default()
        };
        for read in attribute_skips {
            test_fn.record_skip(read);
        }

        let mut direct_calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.extract_assertions_in_body(body, &mut test_fn, &mut direct_calls);
            super::dispatch_calls(body, self.src, &CS_DISPATCH, &mut direct_calls);
        } else {
            // Check for expression-bodied method (arrow_expression_clause)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "arrow_expression_clause" {
                    self.extract_assertions_in_body(child, &mut test_fn, &mut direct_calls);
                    super::dispatch_calls(child, self.src, &CS_DISPATCH, &mut direct_calls);
                }
            }
        }

        Some((test_fn, direct_calls))
    }

    /// Records where the tautologies counted under `node` are (`TestFn::mark_tautologies`).
    fn extract_assertions_in_body(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        direct_calls: &mut Vec<String>,
    ) {
        let mark = test_fn.tautology_mark();
        self.extract_assertions_in_body_unmarked(node, test_fn, direct_calls);
        test_fn.mark_tautologies(mark, node);
    }

    fn extract_assertions_in_body_unmarked(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        direct_calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        let kind = node.kind();

        if kind == "invocation_expression" {
            let fn_node = node.child_by_field_name("function");
            if let Some(func) = fn_node {
                let (class_name, method_name) = self.inspect_invocation_target(func);

                if class_name.is_empty() || class_name == "this" {
                    direct_calls.push(method_name.to_string());
                }

                if (class_name == "Assert" || class_name == "ClassicAssert")
                    && (method_name == "Skip" || method_name == "Ignore")
                {
                    test_fn.record_skip(read_skip(Grammar::CSharp, node, None, self.src));
                    return;
                }
                if let Some(own) = self.condition_taking_skip(node, class_name, method_name) {
                    test_fn.record_skip(read_skip(Grammar::CSharp, node, Some(own), self.src));
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

                if self
                    .vocab
                    .helper_fns
                    .iter()
                    .any(|h| super::helper_call_matches(method_name, h))
                {
                    test_fn.total_asserts += 1;
                    return;
                }
                if self.vocab.extra_macros.iter().any(|m| m == method_name) {
                    test_fn.total_asserts += 1;
                    test_fn.strong_asserts += 1;
                    return;
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_assertions_in_body(child, test_fn, direct_calls);
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
                // NUnit's classic model and MSTest: `Assert.AreEqual(expected, actual)`.
                | "AreEqual"
                | "AreNotEqual"
                | "AreSame"
                | "AreNotSame"
                | "Contains"
                | "DoesNotContain"
                | "Matches"
                | "DoesNotMatch"
                | "Throws"
                | "ThrowsAsync"
                | "ThrowsAny"
                | "ThrowsAnyAsync"
                // MSTest's spellings of the same assertion.
                | "ThrowsExactly"
                | "ThrowsExactlyAsync"
                | "ThrowsException"
                | "ThrowsExceptionAsync"
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

        // NUnit's constraint model: `Assert.That(actual, Is.EqualTo(expected))` and
        // `Is.SameAs(expected)` are equality assertions; any other constraint is counted
        // as it was.
        if method_name == "That" {
            if let Some(expected) = args.get(1).and_then(|c| self.nunit_equality_operand(*c)) {
                if super::self_comparison::note(
                    &mut test_fn.equality_operands,
                    args[0],
                    expected,
                    self.src,
                ) {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            return;
        }

        if is_strong {
            test_fn.strong_asserts += 1;
            // Equality tautology check: 2 args with the same tokens
            if matches!(
                method_name,
                "Equal"
                    | "StrictEqual"
                    | "Same"
                    | "Equivalent"
                    | "AreEquivalent"
                    | "AreEqual"
                    | "AreSame"
            ) && args.len() >= 2
                && super::self_comparison::note(
                    &mut test_fn.equality_operands,
                    args[0],
                    args[1],
                    self.src,
                )
            {
                test_fn.tautologies += 1;
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

    /// The operand of `Is.EqualTo(x)` / `Is.SameAs(x)`, written as exactly that call.
    fn nunit_equality_operand<'b>(&self, constraint: Node<'b>) -> Option<Node<'b>> {
        if constraint.kind() != "invocation_expression" {
            return None;
        }
        let callee = constraint.child_by_field_name("function")?;
        let callee: String = self.text(callee).split_whitespace().collect();
        if !matches!(callee.as_str(), "Is.EqualTo" | "Is.SameAs") {
            return None;
        }
        let args = self.get_invocation_arguments(constraint);
        (args.len() == 1).then(|| args[0])
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

fn csharp_fn_skip(node: tree_sitter::Node, src: &str) -> bool {
    let mut cursor = node.walk();
    let is_abstract = node.children(&mut cursor).any(|c| {
        c.kind() == "modifier"
            && c.utf8_text(src.as_bytes())
                .is_ok_and(|t| t == "abstract" || t == "extern" || t == "partial")
    });
    if is_abstract {
        return true;
    }
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "interface_declaration" {
            return true;
        }
        cur = p.parent();
    }
    false
}

fn csharp_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    if functions::is_test_file(path, Some(is_csharp_test_path)) {
        return true;
    }
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| {
        c.kind() == "attribute_list"
            && c.utf8_text(src.as_bytes()).is_ok_and(|t| {
                t.contains("Fact")
                    || t.contains("Theory")
                    || t.contains("TestMethod")
                    || t.contains("Test]")
            })
    });
    found
}

/// What the steps every pack shares read of this pack (`PackSpec::shared_facts`).
const CSHARP_PACK: super::PackSpec = super::PackSpec {
    functions: &CSHARP_FUNCTIONS,
    own_test_path: Some(is_csharp_test_path),
    handlers: &CSHARP_HANDLERS,
    constants: Some(&CSHARP_CONSTANTS),
    retries: Some(&CSHARP_RETRIES),
    receiver_calls: &CSHARP_RECEIVER_CALLS,
    helper_loops: &super::helper_loops::CSHARP,
    calls: &CSHARP_MOCKS,
    vocabs: super::calls::SLEEPS_AND_TRIVIAL_ASSERTS,
    judged: None,
};

pub const CSHARP_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &[
        "method_declaration",
        "constructor_declaration",
        "local_function_statement",
    ],
    name_fields: &["name"],
    body_fields: &["body", "block", "arrow_expression_clause"],
    ignored_kinds: &["comment"],
    skip: csharp_fn_skip,
    is_test: csharp_fn_is_test,
    classify: functions::classify_jvm,
};

/// A method called on a receiver (`method_checks`).
pub const CSHARP_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[(
            "invocation_expression",
            "function",
            "member_access_expression",
            "name",
        )],
        direct: &[],
        bare: &[],
        tokens: &[],
    };

pub const CSHARP_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["invocation_expression", "object_creation_expression"],
    callee_fields: &["function"],
};

pub const CSHARP_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["catch_clause"],
    arm_of: &[],
    body_fields: &["body", "block"],
    ignored_kinds: &["comment"],
    trivial: &[
        "return",
        "return null",
        "return false",
        "return 0",
        "return \"\"",
        "return string.Empty",
        "return String.Empty",
        "return default",
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
/// `ops = 150000.0`, `this.ops = -1`, `raw[0] = 2.5e5`, `return 150000`,
/// `return new double[] {1.5, 2.0}`, `return new[] {1.5, 2.0}`, `return [1.5, 2.0]`. `null`
/// and `double.NaN` are not numeric literals.
pub const CSHARP_CONSTANTS: super::handlers::ConstantSpec = super::handlers::ConstantSpec {
    blocks: &["block"],
    wrappers: &[
        "expression_statement",
        "parenthesized_expression",
        "collection_element",
        "expression_element",
    ],
    numbers: &["integer_literal", "real_literal"],
    signs: &["prefix_unary_expression"],
    assignments: &["assignment_expression"],
    targets: &[
        "identifier",
        "member_access_expression",
        "element_access_expression",
    ],
    calls: &["invocation_expression", "object_creation_expression"],
    returns: &["return_statement"],
    value_is_last_expression: false,
    collections: &["initializer_expression", "collection_expression"],
    collection_holders: &[
        "array_creation_expression",
        "implicit_array_creation_expression",
    ],
    pairs: &[],
    keys: &[],
};

pub const CSHARP_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["attribute_list"],
};

pub const CS_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["block"],
    ignored_kinds: &["comment"],
    terminators: &["return", "throw"],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const CS_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["initializer_expression", "collection_expression"],
    names: &["identifier"],
    references: &[],
};

/// A local a C# wrapper computes and forwards: `var b = Loc(d);`.
pub const CS_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &["local_declaration_statement"],
    binders: &["variable_declarator"],
    pattern: &["name"],
    value: &[],
    names: &["identifier"],
    holders: &[],
    refused: &[],
};

/// A C# helper whose body is one call: `{ Check(x, true); }`, `=> Check(x, true)`.
pub const CS_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &[
        "block",
        "expression_statement",
        "return_statement",
        "arrow_expression_clause",
    ],
    calls: &["invocation_expression"],
    arguments: &["argument_list", "argument"],
    references: &[],
    plain: &["this", "this_expression"],
    skip: &["comment"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole-file handler rule and the function rule are the shared-or-own
    /// superset: an empty handler is silent, and a plain method counts as a test
    /// function, in a file only the shared rule recognises (`benches/`) and in one
    /// only the pack's own `Test` prefix recognises; elsewhere the handler fires
    /// and the method is not a test.
    #[test]
    fn whole_file_and_fn_rules_are_the_shared_or_own_superset() {
        let handler_src =
            "class Helper {\n  void M() {\n    try {\n      G();\n    } catch (System.Exception) {}\n  }\n}\n";
        let swallowed = |path: &str| {
            CSharpPack
                .extract(path, handler_src, &AssertVocabulary::default())
                .unwrap()
                .swallowed
                .len()
        };
        assert_eq!(swallowed("benches/Helper.cs"), 0, "shared rule only");
        assert_eq!(swallowed("src/TestHelper.cs"), 0, "own rule only");
        assert_eq!(swallowed("src/Helper.cs"), 1, "negative control");

        let plain_src = "class Helper {\n  void M() { G(); }\n}\n";
        let is_test = |path: &str| {
            CSharpPack
                .extract(path, plain_src, &AssertVocabulary::default())
                .unwrap()
                .functions
                .iter()
                .map(|f| f.is_test)
                .collect::<Vec<_>>()
        };
        assert_eq!(is_test("benches/Helper.cs"), vec![true], "shared rule only");
        assert_eq!(is_test("src/TestHelper.cs"), vec![true], "own rule only");
        assert_eq!(is_test("src/Helper.cs"), vec![false], "negative control");
    }

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"public class WrapTest {
  void Checked(int x, bool strict) {
    if (strict && x != 1) { throw new System.Exception("x"); }
  }
  void Noop(int x, int n) {}
  void Via(int x) => Checked(x, true);
  void Hollow(int x) { Noop(x, 1); }
  void Busy(int x) {
    Checked(x, true);
    Prepare(x);
  }
  void Ping(int x) { Pong(x, 1); }
  void Pong(int x, int n) => Ping(x);
  [Fact] public void Direct() { Checked(1, true); }
  [Fact] public void ViaWrapper() { Via(1); }
  [Fact] public void HollowWrapper() { Hollow(1); }
  [Fact] public void BusyHelper() { Busy(1); }
  [Fact] public void WrapperCycle() { Ping(1); }
}
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&CSharpPack, "tests/WrapTest.cs", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
    }

    #[test]
    fn test_csharp_pack_registration_and_extension_matching() {
        let pack = CSharpPack;
        assert_eq!(pack.id(), "csharp");
        assert_eq!(pack.name(), "C#");
        assert!(pack.matches("ExampleMapTests.cs"));
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
    fn test_example_dotnet_map_tests_fixture() {
        let src = r#"
using System;
using System.Collections.Generic;
using System.Linq;
using Xunit;

namespace Example.Tests;

public class ExampleMapTests
{
    [Fact]
    public void BasicCrudAndIndexer()
    {
        using var map = new ExampleMap();
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
                "bindings/dotnet/tests/Example.NET.Tests/ExampleMapTests.cs",
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

    /// A class's `[Ignore]` reaches the tests of the classes nested in it and no test
    /// after it: the scope walk reads a class's members before the next sibling, and
    /// the classes read after an ignored one are not ignored.
    #[test]
    fn an_ignored_class_ignores_the_tests_nested_in_it_and_none_after_it() {
        let src = r#"
using NUnit.Framework;

public class Before
{
    [Test]
    public void First() { Assert.AreEqual(1, One()); }
}

[Ignore("not run")]
public class Skipped
{
    [Test]
    public void Second() { Assert.AreEqual(1, One()); }

    public class Inner
    {
        public class Innermost
        {
            [Test]
            public void Third() { Assert.AreEqual(1, One()); }
        }
    }

    [Test]
    public void Fourth() { Assert.AreEqual(1, One()); }
}

public class After
{
    public class Inner
    {
        [Test]
        public void Fifth() { Assert.AreEqual(1, One()); }
    }
}
"#;
        let facts = CSharpPack
            .extract("tests/ScopeTests.cs", src, &AssertVocabulary::default())
            .expect("extract succeeds");
        let read: Vec<(&str, bool)> = facts
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.ignored))
            .collect();
        assert_eq!(
            read,
            [
                ("First", false),
                ("Second", true),
                ("Third", true),
                ("Fourth", true),
                ("Fifth", false),
            ]
        );
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

    #[test]
    fn test_csharp_helper_functions_and_same_file_resolution() {
        let src = r#"
public class HelperTests
{
    private void VerifyAnswer(int actual, int expected)
    {
        Assert.Equal(expected, actual);
    }

    [Fact]
    public void TestCalculation()
    {
        VerifyAnswer(1 + 1, 2);
    }
}
"#;
        let pack = CSharpPack;
        let facts = pack
            .extract("tests/HelperTests.cs", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(
            facts.tests.len(),
            1,
            "only TestCalculation must be extracted; VerifyAnswer is a helper"
        );
        let t = &facts.tests[0];
        assert_eq!(t.name, "TestCalculation");
        assert_eq!(t.total_asserts, 1);
        assert_eq!(t.strong_asserts, 1);
        assert!(!t.is_vacuous());
    }

    #[test]
    fn unattributed_test_named_method_is_not_collected() {
        // xUnit, NUnit and MSTest discover by attribute only; a helper named
        // `TestConnection` in a test project is never run as a test.
        let src = r#"
public class DbFixture
{
    public bool TestConnection()
    {
        return true;
    }

    public void test_seed_data()
    {
    }
}

public class DbTests
{
    [Fact]
    public void Connects()
    {
        Assert.True(new DbFixture().TestConnection());
    }

    [NUnit.Framework.Test]
    public void Seeds()
    {
        Assert.Equal(1, 1 + 0);
    }

    [DataTestMethod]
    public void Rows()
    {
        Assert.AreEqual(2, 1 + 1);
    }
}
"#;
        let facts = CSharpPack
            .extract("tests/DbTests.cs", src, &AssertVocabulary::default())
            .expect("extract succeeds");
        let mut names: Vec<&str> = facts.tests.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["Connects", "Rows", "Seeds"]);
    }
}
