//! Scala language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.
//!
//! ScalaTest (`FunSuite` `test("x") { }` / `ignore("x") { }`, `FlatSpec` `"x" should "y" in
//! { }` / `ignore`, `WordSpec` nesting, `FunSpec` `describe` / `it`), MUnit (`test("x") { }`,
//! `test("x".ignore)`), specs2 (`"x" >> { }`) and JUnit `@Test` methods; `assert`,
//! `assertEquals`, `assertResult`, `intercept[E]`, `assertThrows[E]`, `fail`, and the
//! `should` / `must` matchers; `@nowarn`, `@SuppressWarnings` and `scalastyle:off` /
//! `scalafix:off` comments as escape hatches; an empty `catch` arm and `Try(...)
//! .getOrElse(...)` / `.toOption` as swallowed errors; `???` as a stub.

use super::ancestry::Ancestry;
use anyhow::Result;
use tree_sitter::Node;

use super::ci_condition::{read_skip, Grammar};
use super::functions::{self, FunctionSpec};
use super::suppressions::{
    AnnotationName, AnnotationRule, AnnotationSuppressions, CommentRule, Reports, RuleText,
    Suppressions,
};
use super::{AssertVocabulary, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Scala language pack implementing [`LanguagePack`].
pub struct ScalaPack;

impl LanguagePack for ScalaPack {
    fn id(&self) -> &'static str {
        "scala"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Scala"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("scala" | "sc"))
    }

    fn is_test_path(&self, path: &str) -> bool {
        functions::is_test_file(path, Some(is_scala_test_path))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let tree = crate::ast::source_text::parse_file_as(
            &tree_sitter_scala::LANGUAGE.into(),
            "the Scala",
            path,
            src,
        )?;
        let root = tree.root_node();

        let anc = Ancestry::new(root);
        let mut extractor = ScalaExtractor {
            dead: super::reach::dead_ranges(root, src, &SCALA_REACH),
            src: src.as_bytes(),
            anc: &anc,
            vocab,
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        SCALA_SUPPRESSIONS.collect(root, extractor.src, &mut extractor.facts.escape_hatches);
        extractor.visit_node(root, &mut Vec::new(), false);
        extractor.resolve_same_file_helpers();
        SCALA_PACK.shared_facts(root, &anc, src, path, vocab, &mut extractor.facts);
        super::expected_exceptions::scala(root, &anc, src, &mut extractor.facts.tests);
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &["comment", "block_comment", "string", "interpolated_string"],
        );
        Ok(extractor.facts)
    }
}

/// Whether a path is conventionally Scala test code (`src/test/`, `*Spec`, `*Suite`,
/// `*Test`).
pub fn is_scala_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let stem = filename
        .strip_suffix(".scala")
        .or_else(|| filename.strip_suffix(".sc"))
        .unwrap_or(filename);
    ["Spec", "Suite", "Test", "Tests"]
        .iter()
        .any(|word| functions::ends_with_word(stem, word))
        || functions::has_dir(path, "test", true)
        || functions::has_dir(path, "it", true)
}

/// Verbs of a spec description (`"A cart" should "sum"`, `"x" must { }`, `"x" when { }`).
const SPEC_VERBS: &[&str] = &["should", "must", "can", "when", "which", "that"];
/// Operators that attach a test body to a description.
const SPEC_BODIES: &[&str] = &["in", "ignore", "is", ">>", "taggedAs"];
/// FunSpec / FreeSpec containers: `describe("x") { }`.
const CONTAINERS: &[&str] = &["describe", "context", "feature"];
/// Test-defining calls: `test("x") { }`, `it("x") { }`, `scenario("x") { }`.
const TEST_CALLS: &[&str] = &["test", "it", "they", "scenario", "property"];

struct ScalaExtractor<'a> {
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    /// The ancestors of the nodes of the file's tree (`super::ancestry`).
    anc: &'a Ancestry<'a>,
    vocab: &'a AssertVocabulary,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

/// `@nowarn`, `@SuppressWarnings` and `@unchecked` under any qualifier
/// (`@scala.annotation.nowarn`), and the comments that turn
/// scalastyle or scalafix off; the rule of a comment is the first word after the marker.
const SCALA_SUPPRESSIONS: Suppressions = Suppressions {
    comments: &["comment", "block_comment"],
    rules: &[CommentRule::opens(
        &["scalastyle:off", "scalafix:off", "scalafix:ok"],
        Reports::Rest(RuleText {
            after: &["scalastyle:off", "scalafix:off", "scalafix:ok"],
            then: &[],
            first_word: true,
            empty_is_all: false,
        }),
    )],
    annotations: Some(&AnnotationSuppressions {
        kinds: &["annotation"],
        name: AnnotationName::LastSegment,
        names: &["nowarn", "SuppressWarnings", "unchecked"],
        rule: AnnotationRule::Arguments {
            trimmed: false,
            empty_is_all: true,
        },
    }),
    ..Suppressions::SLASH_COMMENTS
};

impl<'a> ScalaExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn unquote(t: &str) -> String {
        t.trim().trim_matches('"').to_string()
    }

    fn visit_node(&mut self, node: Node<'a>, scope: &mut Vec<String>, ignored: bool) {
        match node.kind() {
            "class_definition" | "object_definition" | "trait_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_default();
                scope.push(name);
                if let Some(body) = node.child_by_field_name("body") {
                    self.visit_children(body, scope, ignored);
                }
                scope.pop();
                return;
            }
            "function_definition" => {
                self.visit_function(node, scope, ignored);
                return;
            }
            "call_expression" if self.visit_call_test(node, scope, ignored) => return,
            "infix_expression" if self.visit_infix_test(node, scope, ignored) => return,
            _ => {}
        }
        self.visit_children(node, scope, ignored);
    }

    fn visit_children(&mut self, node: Node<'a>, scope: &mut Vec<String>, ignored: bool) {
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children {
            self.visit_node(child, scope, ignored);
        }
    }

    /// `test("x") { }`, `ignore("x") { }`, `test("x".ignore) { }`, `it("x") { }`,
    /// `describe("x") { }`: a call whose callee is itself a call with a name argument,
    /// and whose own argument is the body.
    fn visit_call_test(&mut self, node: Node<'a>, scope: &mut Vec<String>, ignored: bool) -> bool {
        let (Some(inner), Some(body)) = (
            node.child_by_field_name("function"),
            node.child_by_field_name("arguments"),
        ) else {
            return false;
        };
        if inner.kind() != "call_expression" || body.kind() != "block" {
            return false;
        }
        let Some(f) = inner.child_by_field_name("function") else {
            return false;
        };
        let fname = self.text(f);
        let Some(first) = inner
            .child_by_field_name("arguments")
            .and_then(|a| a.named_child(0))
        else {
            return false;
        };
        let (desc, marked_ignore) = match first.kind() {
            "string" => (Self::unquote(self.text(first)), false),
            // MUnit: `test("x".ignore)` / `.flaky` / `.only`.
            "field_expression" => {
                let value = first
                    .child_by_field_name("value")
                    .map(|v| Self::unquote(self.text(v)))
                    .unwrap_or_default();
                let field = first
                    .child_by_field_name("field")
                    .map(|v| self.text(v))
                    .unwrap_or("");
                (value, field == "ignore")
            }
            _ => return false,
        };
        if CONTAINERS.contains(&fname) {
            scope.push(desc);
            self.visit_children(body, scope, ignored);
            scope.pop();
            return true;
        }
        if TEST_CALLS.contains(&fname) || fname == "ignore" {
            let is_ignored = ignored || marked_ignore || fname == "ignore";
            self.push_test(desc, node, body, scope, is_ignored);
            return true;
        }
        false
    }

    /// `"A cart" should "sum" in { }`, `it should "x" ignore { }`, `"x" in { }`, `"x" >> { }`,
    /// and WordSpec containers `"A cart" should { }`.
    fn visit_infix_test(&mut self, node: Node<'a>, scope: &mut Vec<String>, ignored: bool) -> bool {
        let (Some(left), Some(op), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("operator"),
            node.child_by_field_name("right"),
        ) else {
            return false;
        };
        if right.kind() != "block" {
            return false;
        }
        let op = self.text(op);
        let desc = match left.kind() {
            "string" => Self::unquote(self.text(left)),
            "infix_expression" => {
                let verb = left
                    .child_by_field_name("operator")
                    .map(|o| self.text(o))
                    .unwrap_or("");
                if !SPEC_VERBS.contains(&verb) {
                    return false;
                }
                let subject = left
                    .child_by_field_name("left")
                    .map(|l| Self::unquote(self.text(l)))
                    .unwrap_or_default();
                let object = left
                    .child_by_field_name("right")
                    .map(|r| Self::unquote(self.text(r)))
                    .unwrap_or_default();
                format!("{subject} {verb} {object}")
            }
            _ => return false,
        };
        if SPEC_VERBS.contains(&op) && left.kind() == "string" {
            // WordSpec container: `"A cart" should { ... }`.
            scope.push(format!("{desc} {op}"));
            self.visit_children(right, scope, ignored);
            scope.pop();
            return true;
        }
        if SPEC_BODIES.contains(&op) {
            let is_ignored = ignored || op == "ignore";
            self.push_test(desc, node, right, scope, is_ignored);
            return true;
        }
        false
    }

    fn push_test(
        &mut self,
        desc: String,
        node: Node<'a>,
        body: Node<'a>,
        scope: &[String],
        ignored: bool,
    ) {
        let name = if scope.is_empty() {
            desc
        } else {
            format!("{}.{}", scope.join("."), desc)
        };
        let mut test_fn = TestFn {
            name,
            line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            ignored,
            ..Default::default()
        };
        let mut calls = Vec::new();
        self.scan_node(body, &mut test_fn, &mut calls);
        super::dispatch_calls(body, self.anc, self.src, &SCALA_DISPATCH, &mut calls);
        self.facts.tests.push(test_fn);
        self.test_calls.push(calls);
    }

    fn annotated_test(&self, node: Node<'a>) -> (bool, bool) {
        let mut test = false;
        let mut ignored = false;
        let mut cursor = node.walk();
        for c in node.children(&mut cursor) {
            if c.kind() == "annotation" {
                let name = c
                    .child_by_field_name("name")
                    .map(|n| self.text(n))
                    .unwrap_or("");
                match name {
                    "Test" => test = true,
                    "Ignore" | "Disabled" => ignored = true,
                    _ => {}
                }
            }
        }
        (test, ignored)
    }

    fn visit_function(&mut self, node: Node<'a>, scope: &[String], ignored: bool) {
        let name = node
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let (annotated, annotated_ignore) = self.annotated_test(node);
        if annotated {
            self.push_test(
                name.to_string(),
                node,
                body,
                scope,
                ignored || annotated_ignore,
            );
            return;
        }
        let mut helper = TestFn::default();
        let mut dummy = Vec::new();
        self.scan_node(body, &mut helper, &mut dummy);
        helper.total_asserts += super::count_failure_exits(
            body,
            self.src,
            &["throw_expression"],
            &[],
            &[
                "lambda_expression",
                "function_definition",
                "class_definition",
            ],
        );
        self.helpers
            .entry(name.to_string())
            .or_insert(super::HelperFacts::from_scan(
                &helper,
                super::forwarding_wrapper_callee(
                    body,
                    &SCALA_WRAPPER,
                    &SCALA_LOCALS,
                    &dummy,
                    self.src,
                ),
            ));
        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        self.facts.push_helper(
            super::TestHelperFacts::from_scan(name.to_string(), line, end_line, &helper),
            dummy,
        );
    }

    fn args<'b>(&self, call: Node<'b>) -> Vec<Node<'b>> {
        let Some(a) = call.child_by_field_name("arguments") else {
            return Vec::new();
        };
        let mut cursor = a.walk();
        a.named_children(&mut cursor).collect()
    }

    /// Whether a condition compares a value with itself (`x == x`) or is `true`.
    fn is_tautology(&self, cond: Node<'a>, test_fn: &mut TestFn) -> bool {
        match cond.kind() {
            "boolean_literal" => self.text(cond) == "true",
            "infix_expression" => {
                let op = cond
                    .child_by_field_name("operator")
                    .map(|n| self.text(n))
                    .unwrap_or("");
                match (
                    cond.child_by_field_name("left"),
                    cond.child_by_field_name("right"),
                ) {
                    (Some(l), Some(r)) if matches!(op, "==" | "===" | "eq") => {
                        super::self_comparison::note(
                            &mut test_fn.equality_operands,
                            l,
                            r,
                            self.anc,
                            self.src,
                        )
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn scan_node(&self, node: Node<'a>, test_fn: &mut TestFn, calls: &mut Vec<String>) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "call_expression" => self.inspect_call(node, test_fn, calls),
            "infix_expression" => self.inspect_matcher(node, test_fn),
            "identifier" if self.text(node) == "pending" => {
                // A statement of a block, or the whole branch of an `if`.
                let standalone = self.anc.parent(node).is_some_and(|p| {
                    matches!(p.kind(), "block" | "template_body")
                        || (p.kind() == "if_expression"
                            && p.child_by_field_name("condition")
                                .is_none_or(|c| c.id() != node.id()))
                });
                if standalone {
                    test_fn.record_skip(read_skip(Grammar::Scala, node, self.anc, None, self.src));
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.scan_node(child, test_fn, calls);
        }
    }

    /// `x shouldBe y`, `x should equal (y)`, `x mustBe y`: an assertion; strong unless it
    /// compares a value with itself or with a bare boolean.
    fn inspect_matcher(&self, node: Node<'a>, test_fn: &mut TestFn) {
        let op = node
            .child_by_field_name("operator")
            .map(|o| self.text(o))
            .unwrap_or("");
        if !(op.starts_with("should") || op.starts_with("must")) {
            return;
        }
        test_fn.total_asserts += 1;
        let r = node
            .child_by_field_name("right")
            .map(|n| self.text(n).trim());
        // `x shouldBe x` is recorded; the same operands under another matcher word are
        // counted as before.
        let same = match (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) {
            (Some(a), Some(b))
                if matches!(op, "shouldBe" | "shouldEqual" | "mustBe" | "mustEqual") =>
            {
                super::self_comparison::note(
                    &mut test_fn.equality_operands,
                    a,
                    b,
                    self.anc,
                    self.src,
                )
            }
            (Some(a), Some(b)) => super::self_comparison::same(a, b, self.src),
            _ => false,
        };
        if same {
            test_fn.tautologies += 1;
        } else if !matches!(r, Some("true" | "false" | "be (true)" | "be (false)")) {
            test_fn.strong_asserts += 1;
        }
    }

    fn inspect_call(&self, node: Node<'a>, test_fn: &mut TestFn, calls: &mut Vec<String>) {
        let Some(f) = node.child_by_field_name("function") else {
            return;
        };
        let name = match f.kind() {
            "identifier" => {
                calls.push(self.text(f).to_string());
                self.text(f)
            }
            // `intercept[E] { }`, `assertThrows[E] { }`.
            "generic_function" => f
                .child_by_field_name("function")
                .map(|n| self.text(n))
                .unwrap_or(""),
            _ => return,
        };
        let args = self.args(node);
        match name {
            "assert" => {
                test_fn.total_asserts += 1;
                match args.first() {
                    Some(c) if self.is_tautology(*c, test_fn) => test_fn.tautologies += 1,
                    Some(c) if c.kind() == "infix_expression" => test_fn.strong_asserts += 1,
                    _ => {}
                }
            }
            "assertEquals" | "assertResult" | "expectResult" | "assertNotEquals" => {
                test_fn.total_asserts += 1;
                let same = args.len() >= 2
                    && if name == "assertNotEquals" {
                        super::self_comparison::same(args[0], args[1], self.src)
                    } else {
                        super::self_comparison::note(
                            &mut test_fn.equality_operands,
                            args[0],
                            args[1],
                            self.anc,
                            self.src,
                        )
                    };
                if same {
                    test_fn.tautologies += 1;
                } else {
                    test_fn.strong_asserts += 1;
                }
            }
            "intercept"
            | "assertThrows"
            | "assertThrowsWithMessage"
            | "fail"
            | "interceptMessage" => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
            "cancel" => {
                test_fn.record_skip(read_skip(Grammar::Scala, node, self.anc, None, self.src))
            }
            // `assume(c)` cancels the test when `c` does not hold.
            "assume" => {
                let own = args.first().map(|c| (*c, true));
                test_fn.record_skip(read_skip(Grammar::Scala, node, self.anc, own, self.src));
            }
            other => {
                if self
                    .vocab
                    .helper_fns
                    .iter()
                    .any(|h| super::helper_call_matches(other, h))
                {
                    test_fn.total_asserts += 1;
                }
            }
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (test, calls) in self.facts.tests.iter_mut().zip(&self.test_calls) {
            super::resolve_test_same_file_helpers(test, calls, self.vocab, |call| {
                super::helper_through_wrappers(call, &self.helpers)
            });
        }
    }
}

fn scala_fn_is_test<'t>(node: Node<'t>, _: &Ancestry<'t>, src: &str, path: &str) -> bool {
    let mut cursor = node.walk();
    let annotated = node.children(&mut cursor).any(|c| {
        c.kind() == "annotation"
            && c.utf8_text(src.as_bytes())
                .unwrap_or("")
                .starts_with("@Test")
    });
    annotated || functions::is_test_file(path, Some(is_scala_test_path))
}

/// What the steps every pack shares read of this pack (`PackSpec::shared_facts`).
const SCALA_PACK: super::PackSpec = super::PackSpec {
    functions: &SCALA_FUNCTIONS,
    own_test_path: Some(is_scala_test_path),
    handlers: &SCALA_HANDLERS,
    constants: Some(&SCALA_CONSTANTS),
    retries: Some(&SCALA_RETRIES),
    receiver_calls: &SCALA_RECEIVER_CALLS,
    helper_loops: &super::helper_loops::SCALA,
    calls: &SCALA_MOCKS,
    vocabs: super::calls::SLEEPS_AND_TRIVIAL_ASSERTS,
    judged: None,
};

pub const SCALA_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["function_definition"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["comment", "block_comment"],
    skip: functions::skip_none,
    is_test: scala_fn_is_test,
    classify: functions::classify_scala,
};

/// A method called on a receiver (`method_checks`).
pub const SCALA_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[("call_expression", "function", "field_expression", "field")],
        direct: &[],
        bare: &[("field_expression", "field")],
        tokens: &[],
    };

pub const SCALA_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
};

pub const SCALA_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    // A `catch` is a block of `case` arms; each arm is judged on its own.
    handler_kinds: &["case_clause"],
    arm_of: &["catch_clause"],
    body_fields: &["body"],
    ignored_kinds: &["comment", "block_comment"],
    trivial: &["()", "None", "null", "false", "0", "Nil", "return"],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    // `Try(f).getOrElse(x)` and `Try(f).toOption` turn any failure into a value.
    silence_kinds: &["call_expression", "field_expression"],
    silences: super::handlers::scala_silences,
    silence_node: None,
};

/// A handler statement that puts a number in place of the result (`constant-fallback`):
/// `ops = 150000.0`, `rec.ops = -1` (the grammar reads `-1` as one literal),
/// `return 150000.0`, and a number or a tuple of numbers as the arm's last expression.
/// `None` and `Double.NaN` are not numeric literals; `d(0) = 3` is a call on the left and
/// `List(1.5)` a call, so neither is read.
pub const SCALA_CONSTANTS: super::handlers::ConstantSpec = super::handlers::ConstantSpec {
    blocks: &["block", "indented_block"],
    wrappers: &["parenthesized_expression"],
    numbers: &["integer_literal", "floating_point_literal"],
    signs: &["prefix_expression"],
    assignments: &["assignment_expression"],
    targets: &["identifier", "field_expression"],
    calls: &["call_expression", "instance_expression"],
    returns: &["return_expression"],
    value_is_last_expression: true,
    collections: &["tuple_expression"],
    collection_holders: &[],
    pairs: &[],
    keys: &[],
};

pub const SCALA_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["annotation"],
};

pub const SCALA_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_expression"],
    block_kinds: &["block"],
    ignored_kinds: &["comment", "block_comment"],
    terminators: &["return", "throw", "???"],
};

/// Functions a test body runs through a dispatch table: `List(check _, other _)`.
pub const SCALA_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["method_value"],
    names: &["identifier"],
    references: &[],
};

/// A local a Scala wrapper computes and forwards: `val b = loc(d)`.
pub const SCALA_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &[],
    binders: &["val_definition", "var_definition"],
    pattern: &["pattern"],
    value: &["value"],
    names: &["identifier"],
    holders: &[],
    refused: &[],
};

/// A Scala helper whose body is one call: `= check(x, true)`, `= { check(x) }`.
pub const SCALA_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &["block", "return_expression"],
    calls: &["call_expression"],
    arguments: &["arguments"],
    references: &[],
    plain: &["this"],
    skip: &["comment", "block_comment"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"class WrapSuite extends AnyFunSuite {
  private def checked(x: Int, strict: Boolean): Unit = if (strict && x != 1) throw new IllegalStateException("x")
  private def noop(x: Int, n: Int): Unit = {}
  private def via(x: Int): Unit = checked(x, true)
  private def hollow(x: Int): Unit = { noop(x, 1) }
  private def busy(x: Int): Unit = {
    checked(x, true)
    prepare(x)
  }
  private def ping(x: Int): Unit = pong(x, 1)
  private def pong(x: Int, n: Int): Unit = ping(x)
  test("direct") { checked(1, true) }
  test("via wrapper") { via(1) }
  test("hollow wrapper") { hollow(1) }
  test("busy helper") { busy(1) }
  test("wrapper cycle") { ping(1) }
}
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&ScalaPack, "src/test/scala/WrapSuite.scala", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
    }

    fn facts(path: &str, src: &str) -> ParsedFileFacts {
        ScalaPack
            .extract(path, src, &AssertVocabulary::default())
            .expect("extract succeeds")
    }

    #[test]
    fn funsuite_munit_and_flatspec_tests_assertions_and_skips() {
        let src = "class CartSuite extends AnyFunSuite with Matchers {\n  test(\"total\") {\n    assert(cart.total == 3)\n    assertEquals(cart.count, 2)\n    cart.total shouldBe 3\n    assert(true)\n    intercept[IllegalStateException] { cart.pay() }\n  }\n  ignore(\"slow\") { assert(x == 1) }\n  test(\"later\".ignore) { assert(x == 1) }\n  test(\"empty\") { }\n  \"A cart\" should \"sum\" in { assert(total == 3) }\n  it should \"skip\" ignore { assert(x == 1) }\n}\n";
        let f = facts("src/test/scala/CartSuite.scala", src);
        let by = |n: &str| {
            f.tests
                .iter()
                .find(|t| t.name == n)
                .unwrap_or_else(|| panic!("{n}: {:?}", f.tests))
        };
        let total = by("CartSuite.total");
        assert_eq!(
            (total.total_asserts, total.strong_asserts, total.tautologies),
            (5, 4, 1),
            "{total:?}"
        );
        assert!(by("CartSuite.slow").ignored);
        assert!(by("CartSuite.later").ignored);
        assert!(by("CartSuite.empty").is_vacuous());
        assert_eq!(by("CartSuite.A cart should sum").strong_asserts, 1);
        assert!(by("CartSuite.it should skip").ignored);
    }

    #[test]
    fn wordspec_funspec_nesting_helpers_and_junit() {
        let src = "class RowSpec extends AnyWordSpec {\n  private def check(r: Row): Unit = if (r.id != 1) throw new IllegalStateException(\"id\")\n  \"A row\" should {\n    \"validate\" in { check(load()) }\n  }\n  describe(\"parser\") {\n    it(\"parses\") { assertResult(1)(parse(\"1\")) }\n  }\n  @Test def junit(): Unit = { assertEquals(1, f()) }\n}\n";
        let f = facts("src/test/scala/RowSpec.scala", src);
        let by = |n: &str| {
            f.tests
                .iter()
                .find(|t| t.name == n)
                .unwrap_or_else(|| panic!("{n}: {:?}", f.tests))
        };
        let v = by("RowSpec.A row should.validate");
        assert_eq!((v.total_asserts, v.helper_checks), (1, 1), "{v:?}");
        assert_eq!(by("RowSpec.parser.parses").total_asserts, 1);
        assert_eq!(by("RowSpec.junit").strong_asserts, 1);
    }

    #[test]
    fn swallowed_errors_stubs_and_suppressions() {
        let src = "// scalastyle:off magic.number\n@nowarn(\"cat=deprecation\")\nobject Store {\n  def save(): Unit = {\n    try { write() } catch { case _: Exception => }\n    try { write() } catch { case e: Exception => throw new StoreError(e) }\n    val v = Try(load()).getOrElse(0)\n    val o = Try(load()).toOption\n    val kept = Try(load()).recover { case _ => fallback() }\n  }\n  def stub(): Int = ???\n}\n";
        let f = facts("src/main/scala/Store.scala", src);
        let kinds: Vec<(usize, &str)> = f.swallowed.iter().map(|s| (s.line, s.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (5, "empty-handler"),
                (7, "silenced-error"),
                (8, "silenced-error")
            ]
        );
        assert_eq!(f.escape_hatches.len(), 2);
        let stub = f.functions.iter().find(|x| x.name == "stub").unwrap();
        assert!(
            matches!(stub.shape, functions::BodyShape::Stub(_)),
            "{stub:?}"
        );
    }

    #[test]
    fn a_match_arm_outside_a_catch_is_not_a_handler() {
        let src = "object A {\n  def f(x: Int): Int = x match {\n    case 1 =>\n    case _ => 2\n  }\n}\n";
        assert!(facts("src/main/scala/A.scala", src).swallowed.is_empty());
    }
}
