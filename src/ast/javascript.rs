//! JavaScript and TypeScript language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use super::ancestry::{Above, Ancestry};
use anyhow::Result;
use tree_sitter::Node;

use super::ci_condition::SkipCondition;
use super::functions::{self, FunctionSpec};
use super::suppressions::{CommentRule, Reports, RuleText, Suppressions};
use super::{AssertVocabulary, Fact, LanguagePack, ParsedFileFacts, TestFn};

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
        let ext = super::extension(path).unwrap_or("js");
        let lang = match ext {
            "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "tsx" | "jsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_javascript::LANGUAGE.into(),
        };
        let tree = crate::ast::source_text::parse_file_as(&lang, "JS/TS", path, src)?;
        let root = tree.root_node();

        let anc = Ancestry::new(root);
        let mut extractor = JsExtractor {
            dead: super::reach::dead_ranges(root, src, &JS_REACH),
            src: src.as_bytes(),
            anc: &anc,
            vocab,
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            test_calls: Vec::new(),
            suite_cases: Vec::new(),
            suite_skips: Vec::new(),
            runner_names: runner_names(root, &anc, src),
            callee_heads: super::CodeHeads::new(CALLEE_NOT_NAMES),
            deno_global: !binds_name(root, &anc, src, "Deno"),
            std_asserts: std_assert_names(root, &anc, src),
        };

        JS_SUPPRESSIONS.collect(root, extractor.src, &mut extractor.facts.escape_hatches);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers(root);
        JS_PACK.shared_facts(root, &anc, src, path, vocab, &mut extractor.facts);
        super::bounds::javascript(root, src, &mut extractor.facts.tests);
        super::expectations::javascript(root, src, &mut extractor.facts.tests);
        super::caught_assertions::javascript(root, &anc, src, &mut extractor.facts.tests, vocab);
        super::expected_exceptions::javascript(root, &anc, src, &mut extractor.facts.tests);
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
            if name.starts_with("to") {
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

/// The node kinds under a callee that are not the names of its chain.
const CALLEE_NOT_NAMES: &[&str] = &["arguments", "template_string", "string", "comment"];

/// The callees `JsExtractor::classify_call` reads as a test or a suite when they are the
/// whole of the callee.
const RUNNER_CALLEES: &[&str] = &[
    "it",
    "test",
    "xit",
    "xtest",
    "describe",
    "context",
    "xdescribe",
    "xcontext",
];

/// How a callee begins when it is a test or a suite with modifiers.
const RUNNER_CHAINS: &[&str] = &["describe.", "it.", "test."];

struct JsExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    /// The ancestors of the nodes of the file's tree (`super::ancestry`).
    anc: &'a Ancestry<'a>,
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
    /// The runner function names this file binds to something else
    /// ([`runner_names`]): `.each` on one is not a case source.
    runner_names: super::test_cases::RunnerNames,
    /// How the callee of each call begins (`super::CodeHeads`), for `classify_call`.
    callee_heads: super::CodeHeads,
    /// `Deno` is the runtime's global here: the file binds nothing else to the name.
    deno_global: bool,
    /// The names the file imports from Deno's standard assertion module, each with the
    /// `node:assert` call it is counted as ([`std_assert_names`]).
    std_asserts: Vec<(String, &'static str)>,
}

/// Whether a module specifier names Deno's standard assertion module: `jsr:@std/assert`
/// (with or without a version or a sub-path), the same through an import map
/// (`@std/assert`), or `https://deno.land/std@<version>/assert/...` and its older
/// `testing/asserts.ts`.
pub(super) fn is_std_assert_module(module: &str) -> bool {
    let bare = module.strip_prefix("jsr:").unwrap_or(module);
    let bare = bare.strip_prefix('/').unwrap_or(bare);
    if let Some(rest) = bare.strip_prefix("@std/assert") {
        return rest.is_empty() || rest.starts_with(['@', '/']);
    }
    module.starts_with("https://deno.land/std")
        && (module.contains("/assert/") || module.contains("/testing/asserts"))
}

/// The names the file imports from Deno's standard assertion module, each with the
/// `node:assert` call the pack counts it as: `assertEquals(a, b)` is counted, and read
/// for a tautology, as `assert.equal(a, b)` is. A function of the module with no such
/// counterpart (`assertExists`, `assertInstanceOf`) is counted as a plain `assert.*`
/// call. Only a name that starts with `assert` is read: an import under another name
/// (`assertEquals as eq`) and a namespace import are not.
fn std_assert_names<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
) -> Vec<(String, &'static str)> {
    super::expected_exceptions::js_bindings(root, anc, src)
        .into_iter()
        .filter(|(bound, module)| bound.starts_with("assert") && is_std_assert_module(module))
        .map(|(bound, _)| {
            let counted_as = match bound.as_str() {
                "assert" => "assert",
                "assertEquals" => "assert.equal",
                "assertStrictEquals" => "assert.strictEqual",
                "assertNotEquals" => "assert.notEqual",
                "assertNotStrictEquals" => "assert.notStrictEqual",
                "assertThrows" => "assert.throws",
                "assertRejects" => "assert.rejects",
                "assertMatch" => "assert.match",
                _ => "assert.ok",
            };
            (bound, counted_as)
        })
        .collect()
}

/// Whether the file binds `name` itself: an import or a `require`, a declaration, or a
/// parameter of that name.
fn binds_name<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, name: &str) -> bool {
    if !src.contains(name) {
        return false;
    }
    if super::expected_exceptions::js_bindings(root, anc, src)
        .iter()
        .any(|(bound, _)| bound == name)
    {
        return true;
    }
    let text = |n: Node| n.utf8_text(src.as_bytes()).unwrap_or("");
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let declared = match node.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "variable_declarator" => node.child_by_field_name("name"),
            "required_parameter" | "optional_parameter" => node.child_by_field_name("pattern"),
            "identifier"
                if anc
                    .parent(node)
                    .is_some_and(|p| p.kind() == "formal_parameters") =>
            {
                Some(node)
            }
            _ => None,
        };
        if declared.is_some_and(|n| n.kind() == "identifier" && text(n) == name) {
            return true;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    false
}

/// Reads which of the runner's function names (`it`, `test`, `describe`, ..) the file
/// declares as something that is not a runner's: a function, class or parameter of that
/// name, and a variable of that name whose value is not derived from a runner's
/// (`const test = base.extend({..})`, where `base` is a runner's name, stays the
/// runner's); and which it imports or requires from a module that is not a runner.
fn runner_names<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
) -> super::test_cases::RunnerNames {
    use super::test_cases::{RUNNER_FUNCTIONS, RUNNER_MODULES};
    let text = |n: Node| n.utf8_text(src.as_bytes()).unwrap_or("");
    let bindings = super::expected_exceptions::js_bindings(root, anc, src);
    let from_runner = |name: &str| {
        bindings
            .iter()
            .any(|(bound, module)| bound == name && RUNNER_MODULES.contains(&module.as_str()))
    };
    let mut not_runner: Vec<String> = Vec::new();
    let from_elsewhere: Vec<String> = bindings
        .iter()
        .filter(|(bound, module)| {
            RUNNER_FUNCTIONS.contains(&bound.as_str()) && !RUNNER_MODULES.contains(&module.as_str())
        })
        .map(|(bound, _)| bound.clone())
        .collect();
    let imported = |name: &str| bindings.iter().any(|(bound, _)| bound == name);
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let declared: Option<Node> = match node.kind() {
            "function_declaration" | "generator_function_declaration" | "class_declaration" => {
                node.child_by_field_name("name")
            }
            "required_parameter" | "optional_parameter" => node.child_by_field_name("pattern"),
            "identifier"
                if anc
                    .parent(node)
                    .is_some_and(|p| p.kind() == "formal_parameters") =>
            {
                Some(node)
            }
            "variable_declarator" => {
                let name = node.child_by_field_name("name");
                // `const test = base.extend({..})`: derived from a runner's function.
                let mut on = node.child_by_field_name("value");
                while let Some(value) = on {
                    on = match value.kind() {
                        "call_expression" => value.child_by_field_name("function"),
                        "member_expression" => value.child_by_field_name("object"),
                        _ => break,
                    };
                }
                let derived = on.is_some_and(|base| {
                    base.kind() == "identifier"
                        && (from_runner(text(base))
                            || (RUNNER_FUNCTIONS.contains(&text(base)) && !imported(text(base))))
                });
                name.filter(|_| !derived)
            }
            _ => None,
        };
        if let Some(name) = declared.filter(|n| n.kind() == "identifier") {
            let name = text(name);
            if RUNNER_FUNCTIONS.contains(&name) && !imported(name) {
                not_runner.push(name.to_string());
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    super::test_cases::RunnerNames {
        not_runner,
        imported: from_elsewhere,
    }
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

/// The comments that suppress the TypeScript checker, ESLint or a coverage tool. A
/// coverage marker counts anywhere in the comment.
const JS_SUPPRESSIONS: Suppressions = Suppressions {
    rules: &[
        CommentRule::opens(
            &["@ts-ignore", "@ts-expect-error", "@ts-nocheck"],
            Reports::TypeIgnore("typescript"),
        ),
        CommentRule::opens(
            &["eslint-disable"],
            Reports::Rest(RuleText {
                after: &[
                    "eslint-disable-line",
                    "eslint-disable-next-line",
                    "eslint-disable",
                ],
                then: &[],
                first_word: false,
                empty_is_all: true,
            }),
        ),
        CommentRule {
            markers: &["istanbul ignore", "c8 ignore"],
            anywhere: true,
            reports: Reports::Rule("coverage"),
        },
    ],
    ..Suppressions::SLASH_COMMENTS
};

impl<'a> JsExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn visit_root(&mut self, root: Node<'a>) {
        let mut scope = Vec::new();
        self.visit_node(root, &mut scope, false);
    }

    fn visit_node(&mut self, node: Node<'a>, scope: &mut Vec<String>, parent_ignored: bool) {
        if node.kind() == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                if let Some(modifier_skips) = self.deno_test_callee(func_node) {
                    self.record_deno_test(node, None, scope, parent_ignored || modifier_skips);
                    return;
                }
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
                            func_node,
                            self.src,
                            &self.runner_names,
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
                            super::test_cases::extract_javascript_cases(
                                func_node,
                                self.src,
                                &self.runner_names,
                            ),
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
                                super::dispatch_calls(
                                    body,
                                    self.anc,
                                    self.src,
                                    &JS_DISPATCH,
                                    &mut calls,
                                );
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

    /// Whether `func` is the callee of a Deno test registration, read from the tree:
    /// `Deno.test` (`Some(false)`), or `Deno.test.only` / `Deno.test.ignore`
    /// (`Some(true)`: a test the run leaves out or that makes the run fail, flagged as
    /// `it.only` and `it.skip` are). `None` for anything else, and in a file that binds
    /// `Deno` to something of its own.
    fn deno_test_callee(&self, func: Node<'a>) -> Option<bool> {
        if !self.deno_global || func.kind() != "member_expression" {
            return None;
        }
        let object = func.child_by_field_name("object")?;
        let property = self.text(func.child_by_field_name("property")?);
        let is_deno_test = |node: Node| {
            node.kind() == "member_expression"
                && node
                    .child_by_field_name("object")
                    .is_some_and(|o| o.kind() == "identifier" && self.text(o) == "Deno")
                && node
                    .child_by_field_name("property")
                    .is_some_and(|p| self.text(p) == "test")
        };
        if is_deno_test(func) {
            return Some(false);
        }
        (is_deno_test(object) && matches!(property, "only" | "ignore")).then_some(true)
    }

    /// The `key: value` pairs and methods of the object literal among the arguments of
    /// a Deno test or step registration: `(key, value or method)`.
    fn deno_options<'t>(&self, args: Node<'t>) -> Vec<(&'a str, Node<'t>)> {
        let mut out = Vec::new();
        let mut cursor = args.walk();
        for arg in args.named_children(&mut cursor) {
            if arg.kind() != "object" {
                continue;
            }
            let mut inner = arg.walk();
            for member in arg.named_children(&mut inner) {
                match member.kind() {
                    "pair" => {
                        if let (Some(key), Some(value)) = (
                            member.child_by_field_name("key"),
                            member.child_by_field_name("value"),
                        ) {
                            let key = self.text(key).trim_matches(['"', '\'']);
                            out.push((key, value));
                        }
                    }
                    "method_definition" => {
                        if let Some(name) = member.child_by_field_name("name") {
                            out.push((self.text(name), member));
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Records one `Deno.test(...)` registration, or one `t.step(...)` of it when
    /// `parent` names the test around it, as a test: `("name", fn)`, `("name", {
    /// options }, fn)`, `({ name, fn })`, `({ options }, fn)` and `(function name() {})`.
    /// `ignore: true` and `only: true` in the options flag it as the `.ignore` and
    /// `.only` forms do; any other `ignore` value is a conditional skip, read by its
    /// condition. Each step of the callback's first parameter is then recorded as a
    /// subtest named `<test> > <step>`, as a Go `t.Run` is: `deno test` reports steps
    /// beside the tests, not among them.
    fn record_deno_test(
        &mut self,
        call: Node<'a>,
        parent: Option<&str>,
        scope: &[String],
        mut ignored: bool,
    ) {
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let options = self.deno_options(args);
        let option = |key: &str| options.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let callback = Self::find_callback(args).or_else(|| {
            option("fn").filter(|f| {
                matches!(
                    f.kind(),
                    "arrow_function" | "function_expression" | "function" | "method_definition"
                )
            })
        });
        let literal = |node: Node| {
            matches!(node.kind(), "string" | "template_string")
                .then(|| self.text(node).trim_matches(['\'', '"', '`']).to_string())
        };
        let mut cursor = args.walk();
        let first = args
            .named_children(&mut cursor)
            .find(|n| n.kind() != "comment");
        let title = first
            .and_then(literal)
            .or_else(|| option("name").and_then(literal))
            .or_else(|| {
                let name = callback?.child_by_field_name("name")?;
                // The key of a `fn() {}` method is not the name of the test.
                (callback?.kind() != "method_definition").then(|| self.text(name).to_string())
            })
            .unwrap_or_else(|| "unnamed".to_string());
        let own_name = match parent {
            Some(parent) => format!("{parent} > {title}"),
            None if scope.is_empty() => title,
            None => format!("{} > {}", scope.join(" > "), title),
        };
        let mut conditional = None;
        if let Some(value) = option("ignore") {
            use super::ci_condition::{self, Lang};
            match ci_condition::skip_condition(Lang::JavaScript, value, self.anc, self.src, false) {
                SkipCondition::Always => ignored = true,
                SkipCondition::Never => {}
                SkipCondition::When(verdict) => {
                    conditional = Some((self.text(value).trim().to_string(), verdict));
                }
            }
        }
        if option("only").is_some_and(|value| self.text(value) == "true") {
            ignored = true;
        }
        let mut test_fn = TestFn {
            name: own_name.clone(),
            line: call.start_position().row + 1,
            end_line: call.end_position().row + 1,
            ignored,
            ..Default::default()
        };
        if !test_fn.ignored {
            let inherited = self.suite_skips.iter().flatten().cloned();
            for (text, verdict) in inherited.chain(conditional) {
                test_fn.record_conditional_skip(text, verdict);
            }
        }
        let mut calls = Vec::new();
        let body = callback.and_then(|c| c.child_by_field_name("body"));
        if let Some(callback) = callback {
            if callback.kind() == "method_definition" {
                if let Some(body) = body {
                    self.scan_test_body(body, &mut test_fn);
                }
            } else {
                self.scan_test_body(callback, &mut test_fn);
            }
        }
        if let Some(body) = body {
            self.collect_calls(body, &mut calls);
            super::dispatch_calls(body, self.anc, self.src, &JS_DISPATCH, &mut calls);
            if !test_fn.ignored {
                self.record_conditional_early_exits(body, &mut test_fn);
            }
        }
        self.facts.tests.push(test_fn);
        self.test_calls.push(calls);

        // The steps the callback registers on its first parameter.
        let context = callback
            .and_then(|c| c.child_by_field_name("parameters"))
            .and_then(|p| p.named_child(0))
            .map(|first| first.child_by_field_name("pattern").unwrap_or(first))
            .filter(|name| name.kind() == "identifier")
            .map(|name| self.text(name));
        if let (Some(context), Some(body)) = (context, body) {
            let mut steps = Vec::new();
            self.deno_steps(body, context, &mut steps);
            for step in steps {
                self.record_deno_test(step, Some(&own_name), scope, ignored);
            }
        }
    }

    /// The `<context>.step(...)` calls under `node`, outermost only: a step inside
    /// another step's callback is that step's own.
    fn deno_steps<'t>(&self, node: Node<'t>, context: &str, out: &mut Vec<Node<'t>>) {
        if node.kind() == "call_expression" {
            let is_step = node.child_by_field_name("function").is_some_and(|f| {
                f.kind() == "member_expression"
                    && f.child_by_field_name("object")
                        .is_some_and(|o| o.kind() == "identifier" && self.text(o) == context)
                    && f.child_by_field_name("property")
                        .is_some_and(|p| self.text(p) == "step")
            });
            if is_step {
                out.push(node);
                return;
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        for child in children {
            self.deno_steps(child, context, out);
        }
    }

    /// The same-file callees a test body runs: `name(...)`. A function defined in the
    /// body and not called there (`const f = () => helper()`) runs nothing.
    fn collect_calls(&self, node: Node<'a>, calls: &mut Vec<String>) {
        // Only a function asks what it stands under, and below `node` the walk knows.
        let above = JS_FUNCTION_KINDS
            .contains(&node.kind())
            .then(|| self.anc.parent(node))
            .flatten()
            .map(|p| p.kind());
        self.collect_calls_under(node, above, calls);
    }

    /// [`Self::collect_calls`] for `node`, a child of a node of the kind `above`.
    fn collect_calls_under(
        &self,
        node: Node<'a>,
        above: Option<&'static str>,
        calls: &mut Vec<String>,
    ) {
        if JS_FUNCTION_KINDS.contains(&node.kind()) && above == Some("variable_declarator") {
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
            self.collect_calls_under(child, Some(node.kind()), calls);
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
    fn resolve_same_file_helpers(&mut self, root: Node<'a>) {
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
                super::TestHelperFacts::from_scan(name.clone(), line, end_line, &h),
                calls,
            );
            helpers.insert(name, super::HelperFacts::from_scan(&h, wraps));
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
                super::TestHelperFacts::from_scan(
                    name,
                    body.start_position().row + 1,
                    body.end_position().row + 1,
                    &h,
                ),
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

    fn classify_call(&self, func: Node<'a>) -> (bool, bool, bool, bool) {
        // (is_test, is_suite, is_ignored, is_todo)
        // How the callee begins says that it is none of them, for all but the calls of
        // a runner: the whole of it is read only for those.
        let head = self.callee_heads.of(func, self.src);
        if let Some(begins) = head.bytes() {
            let named =
                head.is_whole() && RUNNER_CALLEES.iter().any(|name| name.as_bytes() == begins);
            let chained = RUNNER_CHAINS
                .iter()
                .any(|name| begins.starts_with(name.as_bytes()));
            if !named && !chained {
                return (false, false, false, false);
            }
        }
        // The names of the chain only: `test.each([".skip"])` is `test.each`, not a skip.
        let text = super::text_without(func, self.src, CALLEE_NOT_NAMES);
        super::ancestry::count(text.len());
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        match text.as_str() {
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

    fn extract_first_arg_title(&self, call_node: Node<'a>) -> String {
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

    fn scan_test_body(&self, body_or_fn: Node<'a>, test: &mut TestFn) {
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
    fn chain_modifiers(&self, func: Node<'a>) -> (bool, Vec<(String, SkipCondition)>) {
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
                                        self.anc,
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
    /// under an `if` it is a conditional skip read by its condition, in the `else` branch
    /// by the negated condition, as a skip call is in the other packs. Elsewhere (a loop,
    /// a nested callback) it is not read.
    fn record_this_skip(&self, call: Node<'a>, test: &mut TestFn) {
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
        match ci_condition::site(Lang::JavaScript, call, self.anc, self.src) {
            Some(site) if site.always => test.ignored = true,
            Some(site) => test.record_conditional_skip(site.text, site.verdict),
            None => {
                // `this.skip();` directly in the body of the test callback.
                let statement = self
                    .anc
                    .parent(call)
                    .filter(|p| p.kind() == "expression_statement");
                let block = statement
                    .and_then(|s| self.anc.parent(s))
                    .filter(|b| b.kind() == "statement_block");
                let callback = block.and_then(|b| self.anc.parent(b)).filter(|f| {
                    matches!(
                        f.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                });
                let test_call = callback
                    .and_then(|f| self.anc.parent(f))
                    .filter(|args| args.kind() == "arguments")
                    .and_then(|args| self.anc.parent(args));
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
    fn record_conditional_early_exits(&self, body: Node<'a>, test: &mut TestFn) {
        use super::ci_condition::{self, CiVerdict, Lang};
        if let Some((cond, consequence)) = self.detect_js_conditional_early_exit(body) {
            let verdict = ci_condition::site(Lang::JavaScript, consequence, self.anc, self.src)
                .map_or(CiVerdict::NotCi, |s| s.verdict);
            test.record_conditional_skip(cond, verdict);
        }
        for exit in ci_condition::exits_under_if(body, &|n| n.kind() == "return_statement") {
            if let Some(site) = ci_condition::site(Lang::JavaScript, exit, self.anc, self.src) {
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
            if (child.kind() == "lexical_declaration" || child.kind() == "variable_declaration")
                && is_js_env_check(child, self.src)
            {
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

            if child.kind() == "if_statement" {
                let cond_node = child.child_by_field_name("condition")?;
                let cond_text = self.text(cond_node).trim();
                let unwrapped = cond_text
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(cond_text)
                    .trim();

                let is_env_check = is_js_env_check(cond_node, self.src)
                    || super::code_names_one_of(cond_node, self.src, &JS_NOT_CODE, &env_bindings);

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
    fn check_assertion_call(&self, call: Node<'a>, test: &mut TestFn) {
        let mark = test.tautology_mark();
        self.check_assertion_call_unmarked(call, test);
        test.mark_tautologies(mark, call);
    }

    fn check_assertion_call_unmarked(&self, call: Node<'a>, test: &mut TestFn) {
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
                        if self.is_tautological_expect(call, prop_name, test) {
                            test.tautologies += 1;
                        }
                        return;
                    }
                }
            }

            // A function of Deno's standard assertion module, counted as the
            // `node:assert` call it corresponds to.
            let func_text = match self
                .std_asserts
                .iter()
                .find(|(bound, _)| func.kind() == "identifier" && bound == func_text)
            {
                Some((_, counted_as)) => counted_as,
                None => func_text,
            };

            // assert / assert.* calls
            if func_text == "assert" || func_text.starts_with("assert.") {
                test.total_asserts += 1;
                if self.is_strong_assert_fn(func_text) {
                    test.strong_asserts += 1;
                }
                if self.is_tautological_assert_call(call, func_text, test) {
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

    fn is_tautological_expect(&self, call: Node<'a>, prop_name: &str, test: &mut TestFn) -> bool {
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
                            // `expect(subject).toBe(expected)`: the receiver is the
                            // `expect` call itself, with one argument.
                            if let Some(subject) = self.expect_subject(obj) {
                                if super::self_comparison::note(
                                    &mut test.equality_operands,
                                    subject,
                                    arg_nodes[0],
                                    self.anc,
                                    self.src,
                                ) {
                                    return true;
                                }
                            }
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

    /// The one argument of a receiver that is the call `expect(subject)`.
    fn expect_subject<'b>(&self, receiver: Node<'b>) -> Option<Node<'b>> {
        if receiver.kind() != "call_expression"
            || receiver
                .child_by_field_name("function")
                .is_none_or(|f| self.text(f) != "expect")
        {
            return None;
        }
        let args = receiver.child_by_field_name("arguments")?;
        if args.named_child_count() != 1 {
            return None;
        }
        args.named_child(0).filter(|a| a.kind() != "comment")
    }

    fn is_tautological_assert_call(
        &self,
        call: Node<'a>,
        func_text: &str,
        test: &mut TestFn,
    ) -> bool {
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
            } else if matches!(
                func_text,
                "assert.equal"
                    | "assert.strictEqual"
                    | "assert.deepEqual"
                    | "assert.deepStrictEqual"
            ) && arg_nodes.len() >= 2
                && super::self_comparison::note(
                    &mut test.equality_operands,
                    arg_nodes[0],
                    arg_nodes[1],
                    self.anc,
                    self.src,
                )
            {
                return true;
            }
        }
        false
    }
}

const JS_NOT_CODE: super::NotCode = super::NotCode {
    strings: &["string", "template_string", "regex"],
    comments: &["comment"],
    interpolations: &["template_substitution"],
};

/// Whether `node` is a candidate for a condition on the environment: its code, outside
/// string literals and comments, spells an environment read or names a CI variable. A CI
/// variable named in a string (`os.Getenv("CI")`, `lookup("CI")`) is read by
/// `ci_condition::site`, from the tree.
fn is_js_env_check(node: Node, src: &[u8]) -> bool {
    let code = super::code_text(node, src, &JS_NOT_CODE);
    code.contains("process.env") || code.contains("process?.env") || super::is_ci_condition(&code)
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
fn js_fn_skip<'t>(node: tree_sitter::Node<'t>, anc: &Ancestry<'t>, src: &str) -> bool {
    // A method of an abstract class has a body; anything else in one, and anything in an
    // ambient declaration or an interface, does not.
    let declared = if node.kind() == "method_definition" {
        anc.nearest(node, Above::JsDeclaration, |above, _| {
            matches!(
                above.kind(),
                "ambient_declaration" | "interface_declaration"
            )
        })
    } else {
        anc.nearest(node, Above::JsDeclarationOrAbstractClass, |above, _| {
            matches!(
                above.kind(),
                "ambient_declaration" | "interface_declaration" | "abstract_class_declaration"
            )
        })
    };
    if declared.is_some() {
        return true;
    }
    let t = node.utf8_text(src.as_bytes()).unwrap_or("");
    t.trim_start().starts_with("abstract ") || t.trim_start().starts_with("declare ")
}

fn js_fn_is_test<'t>(
    node: tree_sitter::Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    path: &str,
) -> bool {
    if functions::test_path(path) {
        return true;
    }
    // A callback passed to `it(` / `test(` / `describe(`.
    anc.nearest(node, Above::JsRunnerCall, |above, _| {
        if above.kind() != "call_expression" {
            return false;
        }
        let callee = above
            .child_by_field_name("function")
            .and_then(|f| f.utf8_text(src.as_bytes()).ok())
            .unwrap_or("");
        let leaf = callee.rsplit('.').next().unwrap_or(callee);
        matches!(
            leaf,
            "it" | "test" | "describe" | "beforeEach" | "afterEach"
        )
    })
    .is_some()
}

/// What the steps every pack shares read of this pack (`PackSpec::shared_facts`).
const JS_PACK: super::PackSpec = super::PackSpec {
    functions: &JS_FUNCTIONS,
    own_test_path: None,
    handlers: &JS_HANDLERS,
    constants: Some(&JS_CONSTANTS),
    retries: Some(&JS_RETRIES),
    receiver_calls: &JS_RECEIVER_CALLS,
    helper_loops: &super::helper_loops::JAVASCRIPT,
    calls: &JS_MOCKS,
    vocabs: super::calls::SLEEPS_AND_TRIVIAL_ASSERTS,
    judged: None,
};

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
    use crate::ast::EscapeHatchSite;

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

    /// The tests of `src` as `(name, ignored)`, in the order of the source.
    fn tests_read(src: &str) -> Vec<(String, bool)> {
        JavaScriptPack
            .extract("tests/m.test.js", src, &AssertVocabulary::default())
            .unwrap()
            .tests
            .iter()
            .map(|t| (t.name.clone(), t.ignored))
            .collect()
    }

    /// A call is a test or a suite by the names of its callee, wherever white space, a
    /// comment, a string or an argument list stands among them.
    #[test]
    fn a_call_is_sorted_by_the_names_of_its_callee() {
        let read = |call: &str| tests_read(&format!("{call}\n"));
        let one = |name: &str, ignored: bool| vec![(name.to_string(), ignored)];
        for (call, want) in [
            ("it('a', () => {});", one("a", false)),
            ("test('a', () => {});", one("a", false)),
            ("xit('a', () => {});", one("a", true)),
            ("xtest('a', () => {});", one("a", true)),
            ("it.skip('a', () => {});", one("a", true)),
            ("it.only('a', () => {});", one("a", true)),
            ("test.todo('a', () => {});", one("a", true)),
            ("test.concurrent('a', () => {});", one("a", false)),
            ("it /* c */ .skip('a', () => {});", one("a", true)),
            ("it\n  .skip('a', () => {});", one("a", true)),
            ("it\u{b}.skip('a', () => {});", one("a", true)),
            ("test.each([1])('a', () => {});", one("a", false)),
            ("test.each(['.skip'])('a', () => {});", one("a", false)),
            ("test.skip.each([1])('a', () => {});", one("a", true)),
            (
                "test.concurrent.only.each([1])('a', () => {});",
                one("a", true),
            ),
            (
                "describe('s', () => { it('a', () => {}); });",
                one("s > a", false),
            ),
            (
                "context('s', () => { it('a', () => {}); });",
                one("s > a", false),
            ),
            (
                "xdescribe('s', () => { it('a', () => {}); });",
                one("s > a", true),
            ),
            (
                "xcontext('s', () => { it('a', () => {}); });",
                one("s > a", true),
            ),
            (
                "describe.skip('s', () => { it('a', () => {}); });",
                one("s > a", true),
            ),
            (
                "describe.each([1])('s', () => { it('a', () => {}); });",
                one("s > a", false),
            ),
            // Not a runner: a longer name, another object, a call or a string first.
            ("its('a', () => {});", vec![]),
            (
                "xdescribes('s', () => { it('a', () => {}); });",
                one("a", false),
            ),
            (
                "describes.skip('s', () => { it('a', () => {}); });",
                one("a", false),
            ),
            ("suite.it('a', () => {});", vec![]),
            ("my.test.skip('a', () => {});", vec![]),
            ("f().it('a', () => {});", vec![]),
            ("'it'.skip('a', () => {});", vec![]),
            ("it2('a', () => {});", vec![]),
            ("ït('a', () => {});", vec![]),
            ("it.ï('a', () => {});", one("a", false)),
            ("it\u{a0}.skip('a', () => {});", one("a", true)),
            // A test in the callback of a call that is none of them is read.
            (
                "x.a().b(() => { it('a', () => {}); }).c();",
                one("a", false),
            ),
            (
                "x.a().b().c(() => { describe.skip('s', () => { it('a', () => {}); }); });",
                one("s > a", true),
            ),
        ] {
            assert_eq!(read(call), want, "{call}");
        }
    }

    /// The head kept for a node is how `text_without` of the node begins, for every node
    /// of trees with strings, comments, bytes outside ASCII and errors, asked from the
    /// root down and from the leaves up.
    #[test]
    fn the_head_of_a_node_is_how_its_text_begins() {
        let sources = [
            "it.skip('a', () => { expect(a /* c */ . b).toBe(`x${y}`); });\n",
            "describe /* c */ . each ( [1] ) ( 's' , () => { x.a().b().c().d(); } ) ;\n",
            "'it'.skip(1); f(1)(2)(3).g; ït('a'); it.ï('b'); it\u{a0}.skip('c'); it\u{b}.only('d');\n",
            "a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p(1).q(2);\nxdescribe ('s', function () {});\n",
            "test('a', () => { expect(a).toBe(1) ; }) ; x . y ( ( ) => ( { } ) ) ;\n",
            "it('a', () => { expect(a.toBe(1); });\ndescribe.each`\n  a | b\n`('s', () => {});\n",
        ];
        let (mut heads_read, mut whole, mut cut, mut not_ascii) = (0, 0, 0, 0);
        for src in sources {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&tree_sitter_javascript::LANGUAGE.into())
                .unwrap();
            let tree = crate::ast::source_text::parse(&mut parser, src).unwrap();
            let mut nodes = Vec::new();
            let mut stack = vec![tree.root_node()];
            while let Some(n) = stack.pop() {
                nodes.push(n);
                let mut cursor = n.walk();
                stack.extend(n.children(&mut cursor));
            }
            let mut up = nodes.clone();
            up.reverse();
            for order in [nodes, up] {
                let heads = crate::ast::CodeHeads::new(CALLEE_NOT_NAMES);
                for n in order {
                    let text: String =
                        crate::ast::text_without(n, src.as_bytes(), CALLEE_NOT_NAMES)
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                    let head = heads.of(n, src.as_bytes());
                    heads_read += 1;
                    match head.bytes() {
                        Some(begins) => {
                            let kept = begins.len();
                            assert_eq!(
                                Some(begins),
                                text.as_bytes().get(..kept),
                                "{:?} of {src:?}",
                                n.kind()
                            );
                            assert_eq!(head.is_whole(), kept == text.len(), "{text:?}");
                            assert!(head.is_whole() || kept == 12, "{text:?}");
                            if head.is_whole() {
                                whole += 1;
                            } else {
                                cut += 1;
                            }
                        }
                        None => {
                            assert!(!head.is_whole());
                            let raw = n.utf8_text(src.as_bytes()).unwrap();
                            assert!(!raw.is_ascii(), "{raw:?}");
                            not_ascii += 1;
                        }
                    }
                }
            }
        }
        // Each kind of answer was compared many times.
        assert!(heads_read > 600, "{heads_read}");
        assert!(
            whole > 300 && cut > 60 && not_ascii > 8,
            "{whole} {cut} {not_ascii}"
        );
    }

    /// A chain of calls in a file costs steps in proportion to its links. Before #672
    /// the callee of each link was read whole to sort the call, and the callee of a link
    /// holds every link before it, so four times the links cost about sixteen times the
    /// steps.
    #[test]
    fn a_chain_of_calls_costs_steps_in_proportion_to_its_links() {
        // A chain is as deep a tree as it has links: read on the stack the binary uses.
        let steps_of = |path: &str, src: &str| -> u64 {
            crate::deep_stack::on_deep_stack(|| {
                let (facts, counted) = crate::ast::ancestry::steps(|| {
                    JavaScriptPack.extract(path, src, &AssertVocabulary::default())
                });
                facts.unwrap();
                counted
            })
            .unwrap()
        };
        let chain = |n: usize| format!("function run() {{\n  x{};\n}}\n", ".a()".repeat(n));
        let (few, many) = (
            steps_of("src/m.js", &chain(50)),
            steps_of("src/m.js", &chain(200)),
        );
        assert!(many < 5 * few, "{few} steps for 50 links, {many} for 200");
        // The same chain with a test in the callback of its last link.
        let holding = |n: usize| {
            format!(
                "x{}.b(() => {{ it('a', () => {{ expect(1).toBe(1); }}); }});\n",
                ".a()".repeat(n)
            )
        };
        let read = crate::deep_stack::on_deep_stack(|| tests_read(&holding(200))).unwrap();
        assert_eq!(read, vec![("a".to_string(), false)]);
        let (few, many) = (
            steps_of("tests/m.test.js", &holding(50)),
            steps_of("tests/m.test.js", &holding(200)),
        );
        assert!(many < 5 * few, "{few} steps for 50 links, {many} for 200");
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
                row("else of another condition", false, Some("!(haveDb)"), false),
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

    const DENO_FILE: &str = r#"import { assert, assertEquals, assertExists, assertEquals as same } from "jsr:@std/assert@1";
import { fail } from "https://deno.land/std@0.224.0/assert/mod.ts";

Deno.test("named", () => { assertEquals(add(1, 2), 3); });
Deno.test({ name: "object", fn() { assertExists(find()); } });
Deno.test(function byName() { assert(ready()); });
Deno.test("tautology", () => { assertEquals(x, x); });
Deno.test("alias and fail", () => { same(1, 2); fail("no"); });
Deno.test.ignore("parked", () => { assertEquals(1, 2); });
Deno.test.only("alone", () => { assertEquals(1, 2); });
Deno.test({ name: "off", ignore: true, fn: () => {} });
Deno.test({ name: "on windows", ignore: Deno.build.os === "windows", fn: () => {} });
Deno.test("steps", async (t) => {
  await t.step("first", () => { assertEquals(a(), 1); });
  await t.step("outer", async (inner) => {
    await inner.step("deep", () => { assertEquals(b(), 2); });
  });
  await other.step("not a step of t", () => {});
});
"#;

    /// `deno test` (deno 2.6) counts each `Deno.test` registration as one test, whatever
    /// its shape, and reports steps beside them.
    #[test]
    fn deno_test_registrations_and_steps_are_read() {
        let facts = JavaScriptPack
            .extract("mod_test.ts", DENO_FILE, &AssertVocabulary::default())
            .unwrap();
        let names: Vec<&str> = facts.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "named",
                "object",
                "byName",
                "tautology",
                "alias and fail",
                "parked",
                "alone",
                "off",
                "on windows",
                "steps",
                "steps > first",
                "steps > outer",
                "steps > outer > deep",
            ]
        );
        let test = |name: &str| facts.tests.iter().find(|t| t.name == name).unwrap();
        // The module's functions count as the `node:assert` calls they correspond to.
        assert_eq!(
            (test("named").total_asserts, test("named").strong_asserts),
            (1, 1)
        );
        assert_eq!(
            (test("object").total_asserts, test("object").strong_asserts),
            (1, 0)
        );
        assert_eq!(test("byName").total_asserts, 1);
        assert_eq!(test("tautology").tautologies, 1);
        // An import under another name, and a function that is not an `assert*`.
        assert_eq!(test("alias and fail").total_asserts, 0);
        for ignored in ["parked", "alone", "off"] {
            assert!(test(ignored).ignored, "{ignored}");
        }
        let conditional = test("on windows");
        assert!(!conditional.ignored);
        assert!(conditional.conditional_ignore.is_some());
        // A step holds its own assertions; the test around it holds them all.
        assert_eq!(test("steps > first").total_asserts, 1);
        assert_eq!(test("steps > outer > deep").total_asserts, 1);
        assert_eq!(test("steps").total_asserts, 2);
    }

    #[test]
    fn deno_test_is_read_from_the_tree_and_only_for_the_runtimes_global() {
        let count = |src: &str| {
            JavaScriptPack
                .extract("mod_test.ts", src, &AssertVocabulary::default())
                .unwrap()
                .tests
                .len()
        };
        assert_eq!(count("Deno.test('a', () => {});\n"), 1);
        // The words in a comment or a string, another object, another member.
        assert_eq!(count("// Deno.test('a', () => {});\n"), 0);
        assert_eq!(count("const s = \"Deno.test('a', () => {})\";\n"), 0);
        assert_eq!(count("NotDeno.test('a', () => {});\n"), 0);
        assert_eq!(count("Deno.tests('a', () => {});\n"), 0);
        assert_eq!(count("Deno.test.each('a', () => {});\n"), 0);
        assert_eq!(count("x.Deno.test('a', () => {});\n"), 0);
        // A file that binds the name itself.
        for bound in [
            "import { Deno } from './shim.ts';",
            "const Deno = require('./shim');",
            "const Deno = { test() {} };",
            "function Deno() {}",
            "function run(Deno) { Deno.test('b', () => {}); }",
        ] {
            assert_eq!(
                count(&format!("{bound}\nDeno.test('a', () => {{}});\n")),
                0,
                "{bound}"
            );
        }
    }

    #[test]
    fn only_the_standard_assertion_module_supplies_bare_assertions() {
        let asserts = |import: &str| {
            let src = format!("{import}\nDeno.test('a', () => {{ assertEquals(f(), 1); }});\n");
            JavaScriptPack
                .extract("mod_test.ts", &src, &AssertVocabulary::default())
                .unwrap()
                .tests[0]
                .total_asserts
        };
        for module in [
            "jsr:@std/assert",
            "jsr:@std/assert@^1.0.0",
            "jsr:@std/assert/equals",
            "@std/assert",
            "https://deno.land/std@0.224.0/assert/mod.ts",
            "https://deno.land/std@0.150.0/testing/asserts.ts",
        ] {
            assert_eq!(
                asserts(&format!("import {{ assertEquals }} from '{module}';")),
                1,
                "{module}"
            );
        }
        for module in [
            "./helpers.ts",
            "jsr:@std/assertions",
            "jsr:@std/testing/bdd",
        ] {
            assert_eq!(
                asserts(&format!("import {{ assertEquals }} from '{module}';")),
                0,
                "{module}"
            );
        }
        assert_eq!(asserts(""), 0);
    }
}
