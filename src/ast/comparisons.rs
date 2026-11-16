//! Comparison operators inside assertions: `assert f() > 6`, `assert "a" in row`,
//! `if Sum(1, 2) != 3 { t.Fatal() }`.
//!
//! An assertion whose operator moves the loose way keeps its count, its class and its
//! literal, so neither a count nor a bound (`super::bounds`) sees `> 6` become `>= 6`,
//! `== 0` become `!= 1`, or `in` become `not in` (#934). Each comparison is recorded with
//! its *skeleton* (the assertion's text with that operator replaced by `@` and the
//! literal it is compared with by `#`, whitespace collapsed), the operator, the literal
//! and whether the comparison is a failure condition; `assertion-reduction` pairs a
//! test's base and head comparisons by skeleton and reports an operator that now accepts
//! more.
//!
//! Read for Python `assert` statements, Go `if` conditions whose block fails the test,
//! Rust `assert!` / `debug_assert!` / `prop_assert!` conditions, JS/TS `expect(..)`
//! matchers and `assert(..)` calls, and Java, C#, C/C++, PHP, Kotlin, Ruby, Swift, Scala
//! and Objective-C assertions (#1070).
//!
//! The polarity of a Python assertion is recorded the same way (#1049): the method of a
//! `self.assert*` call that has a negated twin (`assertTrue` / `assertFalse`, `assertIs` /
//! `assertIsNot`, `assertIn` / `assertNotIn`, ..) with the method name as the operator, and
//! a bare `assert x` / `assert not x` with `assert` / `assert not` as the operator. A pair
//! whose polarity was inverted checks the outcome the test was written to reject. Rust
//! records `assert!(x)` / `assert!(!x)` the same way, and reads `assert_eq!` /
//! `assert_ne!` (any `debug_` or `prop_` prefix kept in the skeleton) as the comparisons
//! `==` / `!=`, so `assert_eq!(n, 0)` -> `assert_ne!(n, 1)` is `== 0` -> `!= 1`. JS/TS
//! reads a comparison matcher (`toBe`, `toBeGreaterThan`, ..) as its operator, `.not`
//! as its negation, the `.not` of any other matcher of a closed list as a polarity, and
//! `assert(x)` / `assert(!x)` and the `assert.*` twins the same way. Java reads the
//! comparisons of an `assert` statement and of a JUnit / TestNG `assertTrue` /
//! `assertFalse` condition, `assertEquals` / `assertNotEquals` as `==` / `!=`, the
//! JUnit twins (`assertTrue` / `assertFalse`, `assertNull` / `assertNotNull`, ..) as a
//! polarity, and in an AssertJ / Truth `assertThat(..)` chain the comparison methods
//! (`isGreaterThan`, ..) as their operator and the methods with a negated twin
//! (`contains` / `doesNotContain`, ..) as a polarity.
//! C# reads FluentAssertions `.Should()` chains, xUnit / NUnit / MSTest `Assert` calls
//! and NUnit `Is.*` constraints the same way: comparison methods and constraints as
//! their operator (`Not` negating a constraint), the methods and constraints with a
//! negated twin as a polarity, and the comparisons of a `True` / `False` condition.
//! C and C++ read GoogleTest `EXPECT_*` / `ASSERT_*`, Catch2 and doctest `CHECK*` /
//! `REQUIRE*`, Unity `TEST_ASSERT*` and CMocka `assert_*` the same way: a comparison
//! macro (`EXPECT_GT`, `TEST_ASSERT_LESS_THAN`, `assert_int_equal`) or GoogleMock
//! matcher (`Gt`) as its operator, a macro with a negated twin as a polarity, and the
//! comparisons of a condition (`EXPECT_TRUE`, `REQUIRE`, `assert`).
//! PHP reads PHPUnit assertions and Pest `expect(..)` chains the same way: an equality
//! or ordering method (`assertSame`, `assertGreaterThan`) or a Pest comparison matcher as
//! its operator (`->not` negating the matcher), a method or matcher with a negated twin
//! as a polarity, and the comparisons of an `assertTrue` / `assertFalse` / `assert(..)`
//! condition.
//! Kotlin reads kotlin.test / JUnit assertions and AssertJ / Truth chains as Java does,
//! and Kotest matchers, infix (`x shouldBe 6`) or called: a comparison matcher
//! (`shouldBeGreaterThan`) as its operator and a matcher with a negated twin
//! (`shouldContain` / `shouldNotContain`) as a polarity.
//! Ruby reads Minitest `assert_*` / `refute_*` twins as a polarity (`assert_equal` /
//! `refute_equal` as `==` / `!=`, `assert_operator` as its symbol) and the comparisons of
//! an `assert` / `refute` condition, and RSpec expectations by their runner and matcher
//! (`to eq`, `not_to be >`, `to be_truthy`).
//! Swift reads XCTest assertions the same way: an equality or ordering
//! (`XCTAssertEqual`, `XCTAssertGreaterThan`) as its operator, an assertion with a
//! negated twin as a polarity, and the comparisons of an `XCTAssert` / `XCTAssertTrue` /
//! `XCTAssertFalse` or Swift Testing `#expect` / `#require` condition.
//! Scala reads ScalaTest infix matchers by their phrase (`shouldBe`, `should be >`,
//! `should not equal`, `should contain`), MUnit / JUnit `assertEquals` /
//! `assertNotEquals` and twins as Java's, and the comparisons of an `assert(..)`
//! condition.
//! Objective-C reads XCTest macros as Swift does (`XCTAssertEqualObjects` /
//! `XCTAssertNotEqualObjects` as `==` / `!=` too), and the comparisons of an `NSAssert` /
//! `NSCAssert` / `assert(..)` condition.

use super::ancestry::count;
use super::bounds::{numeric, text, walk, GoFailures, Record, Records};
use super::python::unparenthesized;
use super::TestFn;
use std::collections::HashMap;
use tree_sitter::Node;

/// One comparison of one assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub line: usize,
    /// The assertion's text with this operator replaced by `@` and the literal it is
    /// compared with, if any, by `#`; whitespace collapsed.
    pub skeleton: String,
    /// The operator as written (`>=`, `not in`).
    pub operator: String,
    /// The literal the comparison is made against, as written, when one side is one.
    pub literal: Option<String>,
    /// `true` when the comparison holding fails the test (a Go `if` guard), `false` when
    /// it must hold (an `assert`).
    pub fails_when_true: bool,
}

/// A comparison whose operator now accepts more results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weakened {
    pub line: usize,
    pub from: String,
    pub to: String,
    pub fails_when_true: bool,
    /// The assertion's polarity was inverted (#1049): it now checks the other outcome.
    pub inverted: bool,
}

impl Record for Comparison {
    fn line(&self) -> usize {
        self.line
    }
    /// The skeleton and the literal, or the operator when there is no literal: records
    /// that differ only in their literal (`toBe(1)`, `toBe(2)`, .. on one line) hash
    /// apart, so adding each is not a scan of the others.
    fn written(&self) -> (&str, &str) {
        (
            &self.skeleton,
            self.literal.as_deref().unwrap_or(&self.operator),
        )
    }
    fn of(test: &mut TestFn) -> &mut Vec<Self> {
        &mut test.comparisons
    }
}

/// The text of `whole` with each of `holes` replaced by its mark, whitespace collapsed.
fn skeleton(whole: Node, holes: &mut [(Node, char)], src: &str) -> String {
    holes.sort_by_key(|(n, _)| n.start_byte());
    let (s, e) = (whole.start_byte(), whole.end_byte());
    count(e.saturating_sub(s));
    let mut raw = String::with_capacity(e.saturating_sub(s));
    let mut at = s;
    for (n, mark) in holes.iter() {
        raw.push_str(src.get(at..n.start_byte()).unwrap_or(""));
        raw.push(*mark);
        at = n.end_byte();
    }
    raw.push_str(src.get(at..e).unwrap_or(""));
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn record(
    records: &mut Records<Comparison>,
    whole: Node,
    (left, op, right): (Node, Node, Node),
    is_literal: &dyn Fn(Node) -> bool,
    src: &str,
    fails_when_true: bool,
) {
    let literal = if is_literal(right) {
        Some(right)
    } else if is_literal(left) {
        Some(left)
    } else {
        None
    };
    let mut holes = vec![(op, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: op.start_position().row + 1,
        skeleton: skeleton(whole, &mut holes, src),
        operator: text(op, src)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true,
    });
}

const PY_OPERATORS: &[&str] = &["==", "!=", "<", "<=", ">", ">=", "in", "not in"];

fn py_literal(n: Node) -> bool {
    match n.kind() {
        "integer" | "float" | "string" | "true" | "false" | "none" => true,
        "unary_operator" => n
            .named_child(0)
            .is_some_and(|c| matches!(c.kind(), "integer" | "float")),
        _ => false,
    }
}

/// The family of a Python assertion's polarity and whether it is the negated one: `None`
/// for an operator that has no negated twin.
fn polarity(operator: &str) -> Option<(&'static str, bool)> {
    Some(match operator {
        "assert" => ("assert", false),
        "assert not" => ("assert", true),
        "assertTrue" => ("true", false),
        "assertFalse" => ("true", true),
        "assertIs" => ("is", false),
        "assertIsNot" => ("is", true),
        "assertIsNone" => ("none", false),
        "assertIsNotNone" => ("none", true),
        "assertEqual" | "assertEquals" => ("equal", false),
        "assertNotEqual" | "assertNotEquals" => ("equal", true),
        "assertAlmostEqual" | "assertAlmostEquals" => ("almost", false),
        "assertNotAlmostEqual" | "assertNotAlmostEquals" => ("almost", true),
        "assertIn" => ("in", false),
        "assertNotIn" => ("in", true),
        "assertIsInstance" => ("instance", false),
        "assertNotIsInstance" => ("instance", true),
        "assertRegex" | "assertRegexpMatches" => ("regex", false),
        "assertNotRegex" | "assertNotRegexpMatches" => ("regex", true),
        RS_ASSERT => ("rs-assert", false),
        RS_ASSERT_NOT => ("rs-assert", true),
        JS_ASSERT => ("js-assert", false),
        JS_ASSERT_NOT => ("js-assert", true),
        "assert.match" => ("js-match", false),
        "assert.doesNotMatch" => ("js-match", true),
        "assert.throws" => ("js-throws", false),
        "assert.doesNotThrow" => ("js-throws", true),
        "assert.rejects" => ("js-rejects", false),
        "assert.doesNotReject" => ("js-rejects", true),
        "assertNull" => ("null", false),
        "assertNotNull" => ("null", true),
        "assertSame" => ("same", false),
        "assertNotSame" => ("same", true),
        JAVA_ASSERT => ("java-assert", false),
        JAVA_ASSERT_NOT => ("java-assert", true),
        _ => {
            return js_matcher_polarity(operator)
                .or_else(|| java_chain_polarity(operator))
                .or_else(|| cs_polarity(operator))
                .or_else(|| c_check(operator).and_then(|c| c.polar))
                .or_else(|| php_polarity(operator))
                .or_else(|| kotest_polarity(operator))
                .or_else(|| rb_polarity(operator))
                .or_else(|| xct_check(operator).and_then(|c| c.polar))
                .or_else(|| sc_polarity(operator))
        }
    })
}

/// JS/TS: the polarity of an `expect(..)` matcher with a negated form, `.not` or its
/// twin (`toBeTruthy` / `toBeFalsy`, `toBeDefined` / `toBeUndefined`).
fn js_matcher_polarity(operator: &str) -> Option<(&'static str, bool)> {
    let (matcher, not) = match negated_matcher(operator) {
        Some(m) => (m, true),
        None => (operator, false),
    };
    let (family, negated) = match matcher {
        "toBeFalsy" => ("toBeTruthy", true),
        "toBeUndefined" => ("toBeDefined", true),
        "toBeFalse" => ("toBeTrue", true),
        _ => (
            *JS_POLAR_MATCHERS
                .iter()
                .chain(PEST_POLAR_MATCHERS)
                .find(|m| **m == matcher)?,
            false,
        ),
    };
    Some((family, negated != not))
}

/// The matcher of a JS/TS `not.matcher` or a Pest `not->matcher`.
fn negated_matcher(operator: &str) -> Option<&str> {
    operator
        .strip_prefix("not.")
        .or_else(|| operator.strip_prefix("not->"))
}

/// Python: the polarity of a `self.assert*(..)` call statement whose method has a
/// negated twin, the method name masked.
fn py_method_polarity(records: &mut Records<Comparison>, statement: Node, src: &str) {
    let Some(method) = statement
        .named_child(0)
        .filter(|c| c.kind() == "call")
        .and_then(|c| c.child_by_field_name("function"))
        .filter(|f| f.kind() == "attribute")
        .filter(|f| {
            f.child_by_field_name("object")
                .is_some_and(|o| text(o, src) == "self")
        })
        .and_then(|f| f.child_by_field_name("attribute"))
    else {
        return;
    };
    let name = text(method, src);
    if polarity(name).is_none() {
        return;
    }
    records.add(Comparison {
        line: method.start_position().row + 1,
        skeleton: skeleton(statement, &mut [(method, '@')], src),
        operator: name.to_string(),
        literal: None,
        fails_when_true: false,
    });
}

/// Python: the polarity of an `assert` statement, `assert x` or `assert not x`. The
/// skeleton is the condition without its `not` and parentheses, then the message.
fn py_assert_polarity(records: &mut Records<Comparison>, statement: Node, src: &str) {
    let Some(cond) = statement.named_child(0) else {
        return;
    };
    let bare = unparenthesized(cond);
    let (operator, checked) = match bare.kind() {
        "not_operator" => match bare.child_by_field_name("argument") {
            Some(a) => ("assert not", unparenthesized(a)),
            None => return,
        },
        _ => ("assert", bare),
    };
    let rest = src.get(cond.end_byte()..statement.end_byte()).unwrap_or("");
    count(rest.len() + checked.end_byte().saturating_sub(checked.start_byte()));
    let written = format!("assert @ {} {rest}", text(checked, src));
    records.add(Comparison {
        line: statement.start_position().row + 1,
        skeleton: written.split_whitespace().collect::<Vec<_>>().join(" "),
        operator: operator.to_string(),
        literal: None,
        fails_when_true: false,
    });
}

/// Python: each comparison of an `assert` statement, a chained one (`0 < x <= 5`)
/// read operator by operator, and the polarity of each assertion ([`polarity`]).
pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() == "expression_statement" {
            py_method_polarity(records, node, src);
            return true;
        }
        if node.kind() != "assert_statement" {
            return true;
        }
        py_assert_polarity(records, node, src);
        walk(node, &mut |n| {
            if n.kind() == "comparison_operator" {
                let mut cursor = n.walk();
                let kids: Vec<Node> = n.children(&mut cursor).map(unparenthesized).collect();
                for w in kids.windows(3) {
                    let op = text(w[1], src)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !w[1].is_named() && PY_OPERATORS.contains(&op.as_str()) {
                        record(records, node, (w[0], w[1], w[2]), &py_literal, src, false);
                    }
                }
            }
            true
        });
        false
    });
}

const GO_OPERATORS: &[&str] = &["==", "!=", "<", "<=", ">", ">="];

fn go_literal(n: Node) -> bool {
    match n.kind() {
        "int_literal"
        | "float_literal"
        | "interpreted_string_literal"
        | "raw_string_literal"
        | "rune_literal"
        | "true"
        | "false"
        | "nil" => true,
        "unary_expression" => n
            .child_by_field_name("operand")
            .is_some_and(|c| matches!(c.kind(), "int_literal" | "float_literal")),
        _ => false,
    }
}

/// Go: the comparison that is the whole condition of an `if` whose block fails the test
/// (`.Fatal`, `.Error`, `.FailNow`, `.Fail(`). The condition holding is the failure.
pub fn go(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    let failures = GoFailures::of(src);
    walk(root, &mut |node| {
        if node.kind() != "if_statement" {
            return true;
        }
        let fails = node
            .child_by_field_name("consequence")
            .is_some_and(|c| failures.inside(c));
        let Some(cond) = node
            .child_by_field_name("condition")
            .map(unparenthesized)
            .filter(|c| fails && c.kind() == "binary_expression")
        else {
            return true;
        };
        if let (Some(l), Some(op), Some(r)) = (
            cond.child_by_field_name("left").map(unparenthesized),
            cond.child_by_field_name("operator"),
            cond.child_by_field_name("right").map(unparenthesized),
        ) {
            if GO_OPERATORS.contains(&text(op, src)) {
                record(records, cond, (l, op, r), &go_literal, src, true);
            }
        }
        true
    });
}

const RS_OPERATORS: &[&str] = &["==", "!=", "<", "<=", ">", ">="];

/// The operators written for a Rust `assert!` condition and its negation.
const RS_ASSERT: &str = "assert!(x)";
const RS_ASSERT_NOT: &str = "assert!(!x)";

/// Tokens that join two operands: a condition holding one at its top level is more than
/// one value, and its `!` would not negate the whole of it.
const RS_BINARY: &[&str] = &[
    "&&", "||", "==", "!=", "<", "<=", ">", ">=", "&", "|", "^", "+", "-", "*", "/", "%", "<<",
    ">>", "..", "..=", "as",
];

/// The literal a Rust operand is, sign included, and the token to mask: `6`, `-1.5`,
/// `"a"`, `'c'`, `true`, in parentheses or not.
fn rs_literal<'t>(toks: &[Node<'t>], src: &str) -> Option<(Node<'t>, String)> {
    let num = |n: Node| matches!(n.kind(), "integer_literal" | "float_literal");
    match super::bounds::rs_unparenthesized(toks).as_slice() {
        [l] if num(*l)
            || matches!(
                l.kind(),
                "string_literal" | "raw_string_literal" | "char_literal" | "boolean_literal"
            )
            || matches!(text(*l, src), "true" | "false") =>
        {
            Some((*l, text(*l, src).to_string()))
        }
        [m, l] if text(*m, src) == "-" && num(*l) => Some((*l, format!("-{}", text(*l, src)))),
        _ => None,
    }
}

/// The text from the first to the last of `toks`, whitespace collapsed.
fn rs_span(toks: &[Node], src: &str) -> String {
    let (Some(first), Some(last)) = (toks.first(), toks.last()) else {
        return String::new();
    };
    count(last.end_byte().saturating_sub(first.start_byte()));
    src.get(first.start_byte()..last.end_byte())
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rust: the polarity of an `assert!(x)` / `assert!(!x)` condition, when the condition is
/// one value (no operator joins two at its top level), and each comparison that is a
/// whole operand of an `assert!` condition (`assert!(n > 6)`, `assert!(ok && n > 6)`,
/// `assert!((n > 6))`). A comparison inside a call's arguments (`f(n > 6)`) or after `!`
/// is not read, and neither is a run holding more than one comparison operator, which is
/// how a turbofish (`parse::<u32>()`) reads in a token tree.
fn rs_assert(records: &mut Records<Comparison>, node: Node, tt: Node, prefix: &str, src: &str) {
    let inside = super::bounds::rs_inside(tt);
    let comma = inside.iter().position(|t| text(*t, src) == ",");
    let cond = &inside[..comma.unwrap_or(inside.len())];
    let (negated, rest) = match cond.split_first() {
        Some((bang, rest)) if text(*bang, src) == "!" => (true, rest),
        _ => (false, cond),
    };
    let checked = super::bounds::rs_unparenthesized(rest);
    let one_value = !rest.is_empty()
        && (checked.len() < rest.len() || !rest.iter().any(|t| RS_BINARY.contains(&text(*t, src))));
    if one_value {
        let message = comma.map_or(String::new(), |c| rs_span(&inside[c..], src));
        records.add(Comparison {
            line: node.start_position().row + 1,
            skeleton: format!("{prefix}assert!(@ {} {message})", rs_span(&checked, src)),
            operator: (if negated { RS_ASSERT_NOT } else { RS_ASSERT }).to_string(),
            literal: None,
            fails_when_true: false,
        });
    }
    for (at, run) in super::bounds::rs_conditions(tt, src)
        .into_iter()
        .enumerate()
    {
        // The macro's own run ends at its message.
        let run = if at == 0 {
            &run[..cond.len().min(run.len())]
        } else {
            &run[..]
        };
        for operand in run.split(|t| matches!(text(*t, src), "&&" | "||" | ",")) {
            let ops: Vec<usize> = operand
                .iter()
                .enumerate()
                .filter(|(_, t)| RS_OPERATORS.contains(&text(**t, src)))
                .map(|(i, _)| i)
                .collect();
            let [i] = ops.as_slice() else {
                continue;
            };
            let (left, op, right) = (&operand[..*i], operand[*i], &operand[*i + 1..]);
            if left.is_empty() || right.is_empty() {
                continue;
            }
            let literal = rs_literal(right, src).or_else(|| rs_literal(left, src));
            let mut holes = vec![(op, '@')];
            holes.extend(literal.as_ref().map(|(l, _)| (*l, '#')));
            records.add(Comparison {
                line: op.start_position().row + 1,
                skeleton: format!("{prefix}{}", skeleton(node, &mut holes, src)),
                operator: text(op, src).to_string(),
                literal: literal.map(|(_, t)| t),
                fails_when_true: false,
            });
        }
    }
}

/// Rust: comparisons and polarity in `assert!`-family macros ([`rs_assert`]), and
/// `assert_eq!` / `assert_ne!` as `==` / `!=` ([`passing`]), the macro name and a literal
/// argument masked.
pub fn rust(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "macro_invocation" {
            return true;
        }
        let Some(name_node) = node.child_by_field_name("macro") else {
            return false;
        };
        let name = text(name_node, src);
        let name = name.rsplit("::").next().unwrap_or(name);
        let Some(tt) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "token_tree")
        else {
            return false;
        };
        if let Some(prefix) = name.strip_suffix("assert") {
            if prefix.is_empty() || prefix.ends_with('_') {
                rs_assert(records, node, tt, prefix, src);
            }
        } else if let Some((prefix, eq)) = name
            .strip_suffix("assert_eq")
            .map(|p| (p, "assert_eq!"))
            .or_else(|| name.strip_suffix("assert_ne").map(|p| (p, "assert_ne!")))
        {
            if prefix.is_empty() || prefix.ends_with('_') {
                let inside = super::bounds::rs_inside(tt);
                let mut args = inside.split(|t| text(*t, src) == ",");
                let (left, right) = (args.next().unwrap_or(&[]), args.next().unwrap_or(&[]));
                let literal = rs_literal(right, src).or_else(|| rs_literal(left, src));
                let mut holes = vec![(name_node, '@')];
                holes.extend(literal.as_ref().map(|(l, _)| (*l, '#')));
                records.add(Comparison {
                    line: node.start_position().row + 1,
                    skeleton: format!("{prefix}{}", skeleton(node, &mut holes, src)),
                    operator: eq.to_string(),
                    literal: literal.map(|(_, t)| t),
                    fails_when_true: false,
                });
            }
        }
        false
    });
}

const JS_OPERATORS: &[&str] = &["===", "!==", "==", "!=", "<", "<=", ">", ">="];

/// The operators written for a JS/TS `assert(..)` condition and its negation.
const JS_ASSERT: &str = "assert(x)";
const JS_ASSERT_NOT: &str = "assert(!x)";

/// `expect(..)` matchers read as a comparison: the literal they are given is masked.
const JS_COMPARISON_MATCHERS: &[&str] = &[
    "toBe",
    "toEqual",
    "toStrictEqual",
    "toBeGreaterThan",
    "toBeGreaterThanOrEqual",
    "toBeLessThan",
    "toBeLessThanOrEqual",
];

/// `expect(..)` matchers whose `.not` form is read as the polarity inverted. A matcher
/// outside both lists is not read: the operator a finding names is a closed set.
const JS_POLAR_MATCHERS: &[&str] = &[
    "toBeTruthy",
    "toBeFalsy",
    "toBeDefined",
    "toBeUndefined",
    "toBeNull",
    "toBeNaN",
    "toBeInstanceOf",
    "toBeCloseTo",
    "toContain",
    "toContainEqual",
    "toMatch",
    "toMatchObject",
    "toHaveProperty",
    "toHaveLength",
    "toThrow",
    "toThrowError",
    "toHaveBeenCalled",
    "toHaveBeenCalledTimes",
    "toHaveBeenCalledWith",
    "toHaveBeenLastCalledWith",
    "toHaveBeenNthCalledWith",
    "toHaveReturned",
    "toHaveReturnedWith",
];

/// `assert.*` methods read as `==` / `!=`, the literal they are given masked.
const JS_ASSERT_EQUALITY: &[&str] = &[
    "equal",
    "strictEqual",
    "deepEqual",
    "deepStrictEqual",
    "notEqual",
    "notStrictEqual",
    "notDeepEqual",
    "notDeepStrictEqual",
];

/// `assert.*` methods with a negated twin.
const JS_ASSERT_POLAR: &[&str] = &[
    "match",
    "doesNotMatch",
    "throws",
    "doesNotThrow",
    "rejects",
    "doesNotReject",
];

fn js_literal(n: Node, src: &str) -> bool {
    match n.kind() {
        "number" | "string" | "true" | "false" | "null" | "undefined" => true,
        "identifier" => text(n, src) == "undefined",
        "template_string" => {
            let mut cursor = n.walk();
            let plain = !n
                .named_children(&mut cursor)
                .any(|c| c.kind() == "template_substitution");
            plain
        }
        "unary_expression" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("argument")
                    .is_some_and(|a| a.kind() == "number")
        }
        _ => false,
    }
}

/// The text of `whole` with each byte range of `holes` replaced by its mark (an empty
/// range inserts it), whitespace collapsed.
fn spliced(whole: Node, holes: &mut [(usize, usize, &str)], src: &str) -> String {
    holes.sort_by_key(|(s, _, _)| *s);
    let (s, e) = (whole.start_byte(), whole.end_byte());
    count(e.saturating_sub(s));
    let mut raw = String::with_capacity(e.saturating_sub(s));
    let mut at = s;
    for (start, end, mark) in holes.iter() {
        raw.push_str(src.get(at..*start).unwrap_or(""));
        raw.push_str(mark);
        at = *end;
    }
    raw.push_str(src.get(at..e).unwrap_or(""));
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn js_args(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(|a| {
            let mut c = a.walk();
            a.named_children(&mut c).map(unparenthesized).collect()
        })
        .unwrap_or_default()
}

/// JS/TS: an `expect(..)` chain ending in a matcher of [`JS_COMPARISON_MATCHERS`] or
/// [`JS_POLAR_MATCHERS`], `.not`, `.resolves` and `.rejects` between them. The matcher
/// and its `.not` are masked, and for a comparison matcher its literal argument.
fn js_expect(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let Some(func) = call
        .child_by_field_name("function")
        .filter(|f| f.kind() == "member_expression")
    else {
        return false;
    };
    let Some(prop) = func.child_by_field_name("property") else {
        return false;
    };
    let mut holes = vec![(prop.start_byte(), prop.end_byte(), "@")];
    let mut not = false;
    let mut obj = func.child_by_field_name("object");
    loop {
        let Some(o) = obj else {
            return false;
        };
        match o.kind() {
            "member_expression" => {
                let (Some(p), Some(inner)) = (
                    o.child_by_field_name("property"),
                    o.child_by_field_name("object"),
                ) else {
                    return false;
                };
                match text(p, src) {
                    "not" => {
                        not = !not;
                        holes.push((inner.end_byte(), o.end_byte(), ""));
                    }
                    "resolves" | "rejects" => {}
                    _ => return false,
                }
                obj = Some(inner);
            }
            "call_expression" => {
                let callee = o
                    .child_by_field_name("function")
                    .map_or("", |f| text(f, src));
                if matches!(callee, "expect" | "expect.soft") {
                    break;
                }
                return false;
            }
            _ => return false,
        }
    }
    let name = text(prop, src);
    let (matcher, compared) = match JS_COMPARISON_MATCHERS.iter().find(|m| **m == name) {
        Some(m) => (*m, true),
        None => match JS_POLAR_MATCHERS.iter().find(|m| **m == name) {
            Some(m) => (*m, false),
            None => return true,
        },
    };
    let literal = js_args(call)
        .first()
        .copied()
        .filter(|a| compared && js_literal(*a, src));
    if let Some(l) = literal {
        holes.push((l.start_byte(), l.end_byte(), "#"));
    }
    records.add(Comparison {
        line: prop.start_position().row + 1,
        skeleton: spliced(stmt, &mut holes, src),
        operator: if not {
            format!("not.{matcher}")
        } else {
            matcher.to_string()
        },
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    true
}

/// JS/TS: each comparison that is a whole operand of an `assert(..)` condition, through
/// `&&`, `||` and parentheses. One inside a call's arguments or after `!` is not read.
fn js_operands(records: &mut Records<Comparison>, stmt: Node, cond: Node, src: &str) {
    let cond = unparenthesized(cond);
    if cond.kind() != "binary_expression" {
        return;
    }
    let (Some(l), Some(op), Some(r)) = (
        cond.child_by_field_name("left").map(unparenthesized),
        cond.child_by_field_name("operator"),
        cond.child_by_field_name("right").map(unparenthesized),
    ) else {
        return;
    };
    match text(op, src) {
        "&&" | "||" => {
            js_operands(records, stmt, l, src);
            js_operands(records, stmt, r, src);
        }
        o if JS_OPERATORS.contains(&o) => {
            record(
                records,
                stmt,
                (l, op, r),
                &|n| js_literal(n, src),
                src,
                false,
            );
        }
        _ => {}
    }
}

/// JS/TS: `assert(..)` / `assert.ok(..)`: the polarity of a condition that is one value
/// (`assert(x)` / `assert(!x)`) and its comparisons ([`js_operands`]); `assert.equal`
/// and its family as `==` / `!=`; `assert.match` / `throws` / `rejects` and their
/// negated twins as a polarity.
fn js_assert(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let Some(func) = call.child_by_field_name("function") else {
        return false;
    };
    let method = match func.kind() {
        "identifier" if text(func, src) == "assert" => None,
        "member_expression" => {
            let (Some(o), Some(p)) = (
                func.child_by_field_name("object"),
                func.child_by_field_name("property"),
            ) else {
                return false;
            };
            if o.kind() != "identifier" || text(o, src) != "assert" {
                return false;
            }
            Some(p)
        }
        _ => return false,
    };
    let args = js_args(call);
    let name = method.map(|p| text(p, src));
    if name.is_none() || name == Some("ok") {
        let (Some(cond), Some(list)) = (args.first(), call.child_by_field_name("arguments")) else {
            return true;
        };
        let (negated, checked) = match cond.kind() {
            "unary_expression"
                if cond
                    .child_by_field_name("operator")
                    .is_some_and(|o| text(o, src) == "!") =>
            {
                match cond.child_by_field_name("argument").map(unparenthesized) {
                    Some(a) => (true, a),
                    None => return true,
                }
            }
            _ => (false, *cond),
        };
        let double = checked.kind() == "unary_expression"
            && checked
                .child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "!");
        let joined = matches!(
            checked.kind(),
            "binary_expression" | "ternary_expression" | "sequence_expression"
        ) && !negated;
        if !double && !joined {
            let callee = text(func, src);
            let rest = src.get(cond.end_byte()..list.end_byte()).unwrap_or("");
            count(rest.len() + checked.end_byte().saturating_sub(checked.start_byte()));
            let written = format!("{callee}(@ {} {rest}", text(checked, src));
            records.add(Comparison {
                line: call.start_position().row + 1,
                skeleton: written.split_whitespace().collect::<Vec<_>>().join(" "),
                operator: (if negated { JS_ASSERT_NOT } else { JS_ASSERT }).to_string(),
                literal: None,
                fails_when_true: false,
            });
        }
        if !negated {
            js_operands(records, stmt, *cond, src);
        }
        return true;
    }
    let (Some(p), Some(name)) = (method, name) else {
        return true;
    };
    let equality = JS_ASSERT_EQUALITY.iter().find(|m| **m == name);
    let polar = JS_ASSERT_POLAR.iter().find(|m| **m == name);
    let Some(m) = equality.or(polar) else {
        return true;
    };
    let mut holes = vec![(p.start_byte(), p.end_byte(), "@")];
    let literal = if equality.is_some() {
        [args.get(1), args.first()]
            .into_iter()
            .flatten()
            .copied()
            .find(|a| js_literal(*a, src))
    } else {
        None
    };
    if let Some(l) = literal {
        holes.push((l.start_byte(), l.end_byte(), "#"));
    }
    records.add(Comparison {
        line: p.start_position().row + 1,
        skeleton: spliced(stmt, &mut holes, src),
        operator: format!("assert.{m}"),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    true
}

/// JS/TS: comparisons and polarity in `expect(..)` matchers ([`js_expect`]) and in
/// `assert(..)` calls ([`js_assert`]), each read from a statement that is the call,
/// awaited or in parentheses or not.
pub fn javascript(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "expression_statement" {
            return true;
        }
        let mut curr = node;
        while let Some(child) = match curr.kind() {
            "expression_statement" | "await_expression" | "parenthesized_expression" => {
                curr.named_child(0)
            }
            _ => None,
        } {
            curr = child;
        }
        if curr.kind() != "call_expression" {
            return true;
        }
        !(js_expect(records, node, curr, src) || js_assert(records, node, curr, src))
    });
}

/// The operators written for a Java `assert` statement's condition and its negation.
const JAVA_ASSERT: &str = "assert x";
const JAVA_ASSERT_NOT: &str = "assert !x";

/// JUnit / TestNG methods with a negated twin, read as a polarity, the method masked.
const JAVA_POLAR: &[&str] = &[
    "assertTrue",
    "assertFalse",
    "assertNull",
    "assertNotNull",
    "assertSame",
    "assertNotSame",
];

/// JUnit / TestNG methods read as `==` / `!=`, the literal they are given masked.
const JAVA_EQUALITY: &[&str] = &["assertEquals", "assertNotEquals"];

/// AssertJ / Truth methods of an `assertThat(..)` chain read as a comparison: the literal
/// they are given is masked.
const JAVA_CHAIN_COMPARISON: &[&str] = &[
    "isEqualTo",
    "isNotEqualTo",
    "isGreaterThan",
    "isGreaterThanOrEqualTo",
    "isLessThan",
    "isLessThanOrEqualTo",
];

/// AssertJ / Truth methods of an `assertThat(..)` chain with a negated twin, as
/// (method, twin). A method outside both lists is not read.
const JAVA_CHAIN_TWINS: &[(&str, &str)] = &[
    ("isTrue", "isFalse"),
    ("isNull", "isNotNull"),
    ("isEmpty", "isNotEmpty"),
    ("isBlank", "isNotBlank"),
    ("isZero", "isNotZero"),
    ("isNaN", "isNotNaN"),
    ("isPresent", "isNotPresent"),
    ("contains", "doesNotContain"),
    ("containsKey", "doesNotContainKey"),
    ("containsValue", "doesNotContainValue"),
    ("containsEntry", "doesNotContainEntry"),
    ("startsWith", "doesNotStartWith"),
    ("endsWith", "doesNotEndWith"),
    ("matches", "doesNotMatch"),
    ("isInstanceOf", "isNotInstanceOf"),
    ("isSameAs", "isNotSameAs"),
    ("isIn", "isNotIn"),
];

/// Java: the polarity of an `assertThat(..)` chain method with a negated twin.
fn java_chain_polarity(operator: &str) -> Option<(&'static str, bool)> {
    JAVA_CHAIN_TWINS.iter().find_map(|(m, twin)| {
        if *m == operator {
            Some((*m, false))
        } else if *twin == operator {
            Some((*m, true))
        } else {
            None
        }
    })
}

fn java_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| {
        matches!(
            n.kind(),
            "decimal_integer_literal"
                | "hex_integer_literal"
                | "octal_integer_literal"
                | "binary_integer_literal"
                | "decimal_floating_point_literal"
                | "hex_floating_point_literal"
        )
    };
    match n.kind() {
        "string_literal" | "character_literal" | "null_literal" | "true" | "false" => true,
        "unary_expression" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("operand").is_some_and(num)
        }
        _ => num(n),
    }
}

/// An expression without its parentheses, a Ruby `(..)` holding one statement included.
fn c_bare(mut n: Node) -> Node {
    loop {
        n = unparenthesized(n);
        if n.kind() != "parenthesized_statements" || n.named_child_count() != 1 {
            return n;
        }
        match n.named_child(0) {
            Some(inner) => n = inner,
            None => return n,
        }
    }
}

/// Java, C#, C/C++, PHP, Kotlin, Ruby and Scala: each comparison that is a whole operand
/// of a condition, through `&&`, `||` (`and`, `or`) and parentheses. One inside a call's
/// arguments or after `!` is not read.
fn c_operands(
    records: &mut Records<Comparison>,
    stmt: Node,
    cond: Node,
    src: &str,
    fails_when_true: bool,
    is_literal: &dyn Fn(Node) -> bool,
) {
    let cond = c_bare(cond);
    if !matches!(
        cond.kind(),
        "binary_expression" | "binary" | "infix_expression"
    ) {
        return;
    }
    let (Some(l), Some(op), Some(r)) = (
        cond.child_by_field_name("left").map(c_bare),
        cond.child_by_field_name("operator"),
        cond.child_by_field_name("right").map(c_bare),
    ) else {
        return;
    };
    match text(op, src) {
        "&&" | "||" | "and" | "or" => {
            c_operands(records, stmt, l, src, fails_when_true, is_literal);
            c_operands(records, stmt, r, src, fails_when_true, is_literal);
        }
        o if JS_OPERATORS.contains(&o) => {
            record(records, stmt, (l, op, r), is_literal, src, fails_when_true);
        }
        _ => {}
    }
}

fn java_args(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(|a| {
            let mut c = a.walk();
            a.named_children(&mut c)
                .filter(|n| !n.is_extra())
                .map(unparenthesized)
                .collect()
        })
        .unwrap_or_default()
}

/// Java: the `assert` statement: the polarity of a condition that is one value
/// (`assert x;` / `assert !x;`) and its comparisons ([`c_operands`]).
fn java_assert(records: &mut Records<Comparison>, stmt: Node, src: &str) {
    let Some(cond) = stmt.named_child(0) else {
        return;
    };
    let bare = unparenthesized(cond);
    let not = |n: Node| {
        n.kind() == "unary_expression"
            && n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "!")
    };
    let (negated, checked) = if not(bare) {
        match bare.child_by_field_name("operand").map(unparenthesized) {
            Some(a) => (true, a),
            None => return,
        }
    } else {
        (false, bare)
    };
    let joined = matches!(
        checked.kind(),
        "binary_expression" | "ternary_expression" | "instanceof_expression"
    ) && !negated;
    if !not(checked) && !joined {
        let rest = src.get(cond.end_byte()..stmt.end_byte()).unwrap_or("");
        count(rest.len() + checked.end_byte().saturating_sub(checked.start_byte()));
        let written = format!("assert @ {} {rest}", text(checked, src));
        records.add(Comparison {
            line: stmt.start_position().row + 1,
            skeleton: written.split_whitespace().collect::<Vec<_>>().join(" "),
            operator: (if negated {
                JAVA_ASSERT_NOT
            } else {
                JAVA_ASSERT
            })
            .to_string(),
            literal: None,
            fails_when_true: false,
        });
    }
    if !negated {
        c_operands(records, stmt, cond, src, false, &|n| java_literal(n, src));
    }
}

/// Java: a JUnit / TestNG assertion call, unqualified or on `Assertions`, `Assert` or
/// `AssertJUnit`: the method of [`JAVA_POLAR`] and [`JAVA_EQUALITY`] masked, the literal
/// of an equality masked, and the comparisons of an `assertTrue` / `assertFalse`
/// condition, which `assertFalse` reads as a failure condition. A JUnit 4 message comes
/// first (`assertTrue("m", ok)`), a JUnit 5 or TestNG one last.
fn java_junit(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let Some(name) = call.child_by_field_name("name") else {
        return false;
    };
    let method = text(name, src);
    let polar = JAVA_POLAR.contains(&method);
    let equality = JAVA_EQUALITY.contains(&method);
    if !polar && !equality {
        return false;
    }
    if let Some(o) = call.child_by_field_name("object") {
        let owner = text(o, src);
        let owner = owner.rsplit('.').next().unwrap_or(owner);
        if !matches!(owner, "Assertions" | "Assert" | "AssertJUnit") {
            return false;
        }
    }
    let args = java_args(call);
    // A JUnit 4 message is a string literal before the operands: `assertTrue("m", ok)`,
    // `assertEquals("m", 3, n)`; `assertEquals("a", s, "m")` compares the first.
    let message_first = args.first().is_some_and(|a| a.kind() == "string_literal")
        && if equality {
            args.len() >= 3 && args.last().is_some_and(|l| l.kind() != "string_literal")
        } else {
            args.len() >= 2
        };
    let operands = if message_first { &args[1..] } else { &args[..] };
    let mut holes = vec![(name, '@')];
    let literal = if equality {
        operands
            .iter()
            .take(2)
            .rev()
            .copied()
            .find(|a| java_literal(*a, src))
    } else {
        None
    };
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if matches!(method, "assertTrue" | "assertFalse") {
        if let Some(cond) = operands.first() {
            c_operands(records, stmt, *cond, src, method == "assertFalse", &|n| {
                java_literal(n, src)
            });
        }
    }
    true
}

/// Java: each method of an AssertJ / Truth `assertThat(..)` (or `then(..)`) chain that
/// is in [`JAVA_CHAIN_COMPARISON`] or [`JAVA_CHAIN_TWINS`], its name masked and, for a
/// comparison, its literal argument.
fn java_chain(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let mut links = Vec::new();
    let mut cur = call;
    loop {
        match cur.child_by_field_name("object") {
            Some(o) if o.kind() == "method_invocation" => {
                links.push(cur);
                cur = o;
            }
            _ => break,
        }
    }
    let root = cur.child_by_field_name("name").map_or("", |n| text(n, src));
    // A Mockito `then(mock).should()..` verifies a call, it asserts no value.
    let verifies = links.iter().any(|l| {
        l.child_by_field_name("name")
            .is_some_and(|n| text(n, src) == "should")
    });
    if links.is_empty() || !matches!(root, "assertThat" | "then") || verifies {
        return false;
    }
    for link in links.into_iter().rev() {
        let Some(name) = link.child_by_field_name("name") else {
            continue;
        };
        let method = text(name, src);
        let compared = JAVA_CHAIN_COMPARISON.contains(&method);
        if !compared && java_chain_polarity(method).is_none() {
            continue;
        }
        let literal = java_args(link)
            .first()
            .copied()
            .filter(|a| compared && java_literal(*a, src));
        let mut holes = vec![(name, '@')];
        holes.extend(literal.map(|l| (l, '#')));
        records.add(Comparison {
            line: name.start_position().row + 1,
            skeleton: skeleton(stmt, &mut holes, src),
            operator: method.to_string(),
            literal: literal.map(|l| text(l, src).to_string()),
            fails_when_true: false,
        });
    }
    true
}

/// Java: comparisons and polarity in `assert` statements ([`java_assert`]), JUnit /
/// TestNG assertion calls ([`java_junit`]) and AssertJ / Truth `assertThat(..)` chains
/// ([`java_chain`]), each read from a statement that is the call.
pub fn java(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| match node.kind() {
        "assert_statement" => {
            java_assert(records, node, src);
            false
        }
        "expression_statement" => {
            let Some(call) = node
                .named_child(0)
                .filter(|c| c.kind() == "method_invocation")
            else {
                return true;
            };
            !(java_junit(records, node, call, src) || java_chain(records, node, call, src))
        }
        _ => true,
    });
}

/// C# xUnit / NUnit / MSTest methods with a negated twin, read as a polarity, as (method,
/// twin): the method masked.
const CS_TWINS: &[(&str, &str)] = &[
    ("True", "False"),
    ("IsTrue", "IsFalse"),
    ("Null", "NotNull"),
    ("IsNull", "IsNotNull"),
    ("Same", "NotSame"),
    ("AreSame", "AreNotSame"),
    ("Equal", "NotEqual"),
    ("AreEqual", "AreNotEqual"),
    ("Contains", "DoesNotContain"),
    ("Empty", "NotEmpty"),
    ("IsEmpty", "IsNotEmpty"),
    ("InRange", "NotInRange"),
    ("Matches", "DoesNotMatch"),
    ("IsInstanceOfType", "IsNotInstanceOfType"),
    // FluentAssertions, after `.Should()`.
    ("BeTrue", "BeFalse"),
    ("BeNull", "NotBeNull"),
    ("BeEmpty", "NotBeEmpty"),
    ("BeNullOrEmpty", "NotBeNullOrEmpty"),
    ("Contain", "NotContain"),
    ("ContainKey", "NotContainKey"),
    ("ContainValue", "NotContainValue"),
    ("StartWith", "NotStartWith"),
    ("EndWith", "NotEndWith"),
    ("Match", "NotMatch"),
    ("MatchRegex", "NotMatchRegex"),
    ("BeSameAs", "NotBeSameAs"),
    ("BeOfType", "NotBeOfType"),
    ("BeAssignableTo", "NotBeAssignableTo"),
    ("BeEquivalentTo", "NotBeEquivalentTo"),
    ("HaveCount", "NotHaveCount"),
    ("Throw", "NotThrow"),
    ("ThrowAsync", "NotThrowAsync"),
];

/// C# xUnit / NUnit / MSTest methods read as `==` / `!=`, the literal they are given
/// masked.
const CS_EQUALITY: &[&str] = &["Equal", "NotEqual", "AreEqual", "AreNotEqual"];

/// C# NUnit classic methods read as a comparison of their first argument with their
/// second, the literal masked.
const CS_ORDERED: &[&str] = &["Greater", "GreaterOrEqual", "Less", "LessOrEqual"];

/// C# FluentAssertions methods read as a comparison, the literal masked.
const CS_FLUENT_COMPARISON: &[&str] = &[
    "Be",
    "NotBe",
    "BeGreaterThan",
    "BeGreaterThanOrEqualTo",
    "BeGreaterOrEqualTo",
    "BeLessThan",
    "BeLessThanOrEqualTo",
    "BeLessOrEqualTo",
];

/// C# NUnit constraints (`Is.GreaterThan(6)`, `Is.Not.EqualTo(3)`) read as a
/// comparison, the literal masked; `Not` negates.
const CS_CONSTRAINT_COMPARISON: &[&str] = &[
    "EqualTo",
    "GreaterThan",
    "GreaterThanOrEqualTo",
    "LessThan",
    "LessThanOrEqualTo",
];

/// C# NUnit constraints read as a polarity, `Not` negating, as (constraint, twin).
const CS_CONSTRAINT_TWINS: &[(&str, &str)] = &[
    ("True", "False"),
    ("Null", ""),
    ("Empty", ""),
    ("Zero", ""),
    ("NaN", ""),
    ("SameAs", ""),
    ("InstanceOf", ""),
    ("TypeOf", ""),
    ("Contain", ""),
    ("Contains", ""),
    ("StartWith", ""),
    ("EndWith", ""),
    ("Match", ""),
    ("SubsetOf", ""),
    ("SupersetOf", ""),
];

/// A constraint's root and name, with whether `Not` negates it: `Is.Not.Null` is
/// (`Is`, `Null`, true).
fn cs_constraint(operator: &str) -> Option<(&str, &str, bool)> {
    let mut parts = operator.split('.');
    let root = parts.next()?;
    if !matches!(root, "Is" | "Does") {
        return None;
    }
    let (name, not) = match (parts.next()?, parts.next()) {
        ("Not", Some(name)) => (name, true),
        (name, None) => (name, false),
        _ => return None,
    };
    parts.next().is_none().then_some((root, name, not))
}

/// C#: the polarity of a method of [`CS_TWINS`] or an NUnit constraint of
/// [`CS_CONSTRAINT_TWINS`].
fn cs_polarity(operator: &str) -> Option<(&'static str, bool)> {
    let (twins, name, not) = match cs_constraint(operator) {
        Some((_, name, not)) => (CS_CONSTRAINT_TWINS, name, not),
        None => (CS_TWINS, operator, false),
    };
    twins.iter().find_map(|(m, twin)| {
        if *m == name {
            Some((*m, not))
        } else if !twin.is_empty() && *twin == name {
            Some((*m, !not))
        } else {
            None
        }
    })
}

fn cs_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| matches!(n.kind(), "integer_literal" | "real_literal");
    match n.kind() {
        "string_literal"
        | "verbatim_string_literal"
        | "raw_string_literal"
        | "character_literal"
        | "null_literal"
        | "boolean_literal" => true,
        "prefix_unary_expression" => {
            let mut c = n.walk();
            let kids: Vec<Node> = n.children(&mut c).collect();
            matches!(kids.as_slice(), [op, operand] if text(*op, src) == "-" && num(*operand))
        }
        _ => num(n),
    }
}

/// The expressions of a C# call's arguments, a named argument's value included.
fn cs_args(call: Node) -> Vec<Node> {
    let Some(list) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut c = list.walk();
    list.named_children(&mut c)
        .filter(|a| a.kind() == "argument")
        .filter_map(|a| {
            let mut ac = a.walk();
            let value = a
                .named_children(&mut ac)
                .filter(|n| !n.is_extra())
                .last()
                .map(unparenthesized);
            value
        })
        .collect()
}

/// The identifier of a member access's name, without its type arguments: `Throw` of
/// `Throw<ArgumentException>`.
fn cs_name(access: Node) -> Option<Node> {
    let name = access.child_by_field_name("name")?;
    if name.kind() == "generic_name" {
        name.named_child(0).filter(|n| n.kind() == "identifier")
    } else {
        Some(name)
    }
}

/// C#: an NUnit constraint `Is[.Not].Name[(arg)]` / `Does[.Not].Name(arg)` given to
/// `Assert.That(actual, ..)`, its path masked as the operator and its literal argument
/// masked. A constraint chained further (`.Within(..)`, `.And`) is not read.
fn cs_constraint_record(records: &mut Records<Comparison>, stmt: Node, c: Node, src: &str) {
    let (path, arg) = match c.kind() {
        "invocation_expression" => {
            let Some(f) = c.child_by_field_name("function") else {
                return;
            };
            (f, cs_args(c).first().copied())
        }
        _ => (c, None),
    };
    if path.kind() != "member_access_expression" {
        return;
    }
    let operator: String = text(path, src).split_whitespace().collect();
    let Some((_, name, _)) = cs_constraint(&operator) else {
        return;
    };
    let compared = CS_CONSTRAINT_COMPARISON.contains(&name);
    if !compared && cs_polarity(&operator).is_none() {
        return;
    }
    let literal = arg.filter(|a| compared && cs_literal(*a, src));
    let mut holes = vec![(path, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: path.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator,
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
}

/// C#: an xUnit / NUnit / MSTest assertion on `Assert` or `ClassicAssert`: the method of
/// [`CS_TWINS`], [`CS_EQUALITY`] and [`CS_ORDERED`] masked with the literal of an
/// equality or an ordering, the comparisons of a `True` / `IsTrue` / `False` / `IsFalse`
/// condition (`False` read as a failure condition) and of an `Assert.That(condition)`,
/// and the constraint of an `Assert.That(actual, constraint)`.
fn cs_assert(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let Some(access) = call
        .child_by_field_name("function")
        .filter(|f| f.kind() == "member_access_expression")
    else {
        return false;
    };
    let owner = access
        .child_by_field_name("expression")
        .map_or("", |o| text(o, src));
    let owner = owner.rsplit('.').next().unwrap_or(owner);
    let Some(name) = cs_name(access) else {
        return false;
    };
    if !matches!(owner, "Assert" | "ClassicAssert") {
        return false;
    }
    let method = text(name, src);
    let args = cs_args(call);
    if method == "That" {
        match args.get(1) {
            Some(c)
                if !matches!(
                    c.kind(),
                    "string_literal" | "interpolated_string_expression"
                ) =>
            {
                cs_constraint_record(records, stmt, *c, src);
            }
            _ => {
                if let Some(cond) = args.first() {
                    c_operands(records, stmt, *cond, src, false, &|n| cs_literal(n, src));
                }
            }
        }
        return true;
    }
    let equality = CS_EQUALITY.contains(&method);
    let ordered = CS_ORDERED.contains(&method);
    if !ordered && cs_polarity(method).is_none() {
        return false;
    }
    let literal = if equality {
        args.iter()
            .take(2)
            .rev()
            .copied()
            .find(|a| cs_literal(*a, src))
    } else if ordered {
        args.get(1).copied().filter(|a| cs_literal(*a, src))
    } else {
        None
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if matches!(method, "True" | "IsTrue" | "False" | "IsFalse") {
        if let Some(cond) = args.first() {
            let fails = matches!(method, "False" | "IsFalse");
            c_operands(records, stmt, *cond, src, fails, &|n| cs_literal(n, src));
        }
    }
    true
}

/// C#: each method of a FluentAssertions `.Should()` chain (`.And` between links) that
/// is in [`CS_FLUENT_COMPARISON`] or [`CS_TWINS`], its name masked and, for a
/// comparison, its literal argument.
fn cs_fluent(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let mut links = Vec::new();
    let mut cur = call;
    let rooted = loop {
        let Some(access) = cur
            .child_by_field_name("function")
            .filter(|f| f.kind() == "member_access_expression")
        else {
            break false;
        };
        if cs_name(access).is_some_and(|n| text(n, src) == "Should") {
            break true;
        }
        links.push((cur, access));
        let mut inner = access.child_by_field_name("expression");
        // `.And` / `.Which` hold the next link's subject.
        if let Some(i) = inner.filter(|i| i.kind() == "member_access_expression") {
            if !matches!(cs_name(i).map(|n| text(n, src)), Some("And")) {
                break false;
            }
            inner = i.child_by_field_name("expression");
        }
        match inner {
            Some(i) if i.kind() == "invocation_expression" => cur = i,
            _ => break false,
        }
    };
    if !rooted || links.is_empty() {
        return false;
    }
    for (link, access) in links.into_iter().rev() {
        let Some(name) = cs_name(access) else {
            continue;
        };
        let method = text(name, src);
        let compared = CS_FLUENT_COMPARISON.contains(&method);
        if !compared && cs_polarity(method).is_none() {
            continue;
        }
        let literal = cs_args(link)
            .first()
            .copied()
            .filter(|a| compared && cs_literal(*a, src));
        let mut holes = vec![(name, '@')];
        holes.extend(literal.map(|l| (l, '#')));
        records.add(Comparison {
            line: name.start_position().row + 1,
            skeleton: skeleton(stmt, &mut holes, src),
            operator: method.to_string(),
            literal: literal.map(|l| text(l, src).to_string()),
            fails_when_true: false,
        });
    }
    true
}

/// C#: comparisons and polarity in xUnit / NUnit / MSTest assertions ([`cs_assert`]) and
/// FluentAssertions chains ([`cs_fluent`]), each read from a statement that is the call
/// (awaited or not).
pub fn csharp(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "expression_statement" {
            return true;
        }
        let mut call = node.named_child(0);
        if let Some(a) = call.filter(|c| c.kind() == "await_expression") {
            call = a.named_child(0);
        }
        let Some(call) = call.filter(|c| c.kind() == "invocation_expression") else {
            return true;
        };
        !(cs_assert(records, node, call, src) || cs_fluent(records, node, call, src))
    });
}

/// C/C++: what a test macro or assertion function checks ([`c_check`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CCheck {
    /// The operator its operands must satisfy (`EXPECT_GT` -> `>`).
    compare: Option<&'static str>,
    /// Its family and whether it is the negated twin (`EXPECT_TRUE` / `EXPECT_FALSE`).
    polar: Option<(&'static str, bool)>,
    /// Its first argument is a condition: `Some(true)` when the condition holding fails
    /// the test (`EXPECT_FALSE(cond)`).
    condition: Option<bool>,
    /// The literal of a comparison is its first argument only (Unity's threshold).
    threshold_first: bool,
}

const NO_CHECK: CCheck = CCheck {
    compare: None,
    polar: None,
    condition: None,
    threshold_first: false,
};

/// The operator of a two-letter GoogleTest / doctest comparison suffix.
fn c_suffix_operator(suffix: &str) -> Option<&'static str> {
    Some(match suffix {
        "EQ" => "==",
        "NE" => "!=",
        "LT" => "<",
        "LE" => "<=",
        "GT" => ">",
        "GE" => ">=",
        _ => return None,
    })
}

/// C/C++: what the test macro or assertion function `name` checks, for GoogleTest
/// (`EXPECT_*` / `ASSERT_*`), Catch2 and doctest (`CHECK*` / `REQUIRE*`), Unity
/// (`TEST_ASSERT*`), CMocka (`assert_*`) and the GoogleMock comparison matchers (`Gt`,
/// ..). A name outside these lists is not read.
fn c_check(name: &str) -> Option<CCheck> {
    let compare = |op| CCheck {
        compare: Some(op),
        ..NO_CHECK
    };
    let polar = |family, not| CCheck {
        polar: Some((family, not)),
        ..NO_CHECK
    };
    let condition = |family, not| CCheck {
        polar: Some((family, not)),
        condition: Some(not),
        ..NO_CHECK
    };
    let equality = |family, not| CCheck {
        compare: Some(if not { "!=" } else { "==" }),
        polar: Some((family, not)),
        ..NO_CHECK
    };
    if let Some(rest) = name
        .strip_prefix("EXPECT_")
        .or_else(|| name.strip_prefix("ASSERT_"))
    {
        return Some(match rest {
            "EQ" => equality("gtest-eq", false),
            "NE" => equality("gtest-eq", true),
            "TRUE" => condition("gtest-true", false),
            "FALSE" => condition("gtest-true", true),
            "STREQ" => polar("gtest-streq", false),
            "STRNE" => polar("gtest-streq", true),
            "STRCASEEQ" => polar("gtest-strcaseeq", false),
            "STRCASENE" => polar("gtest-strcaseeq", true),
            "ANY_THROW" => polar("gtest-throw", false),
            "NO_THROW" => polar("gtest-throw", true),
            _ => compare(c_suffix_operator(rest)?),
        });
    }
    if let Some(rest) = name
        .strip_prefix("CHECK_")
        .or_else(|| name.strip_prefix("REQUIRE_"))
    {
        return Some(match rest {
            "EQ" => equality("doctest-eq", false),
            "NE" => equality("doctest-eq", true),
            "FALSE" => condition("catch-check", true),
            "UNARY" => condition("doctest-unary", false),
            "UNARY_FALSE" => condition("doctest-unary", true),
            "THROWS" => polar("catch-throws", false),
            "NOTHROW" => polar("catch-throws", true),
            _ => compare(c_suffix_operator(rest)?),
        });
    }
    if let Some(rest) = name.strip_prefix("TEST_ASSERT") {
        let threshold = |op| CCheck {
            compare: Some(op),
            threshold_first: true,
            ..NO_CHECK
        };
        return Some(match rest {
            "" | "_TRUE" => condition("unity-true", false),
            "_FALSE" | "_UNLESS" => condition("unity-true", true),
            "_NULL" => polar("unity-null", false),
            "_NOT_NULL" => polar("unity-null", true),
            "_EMPTY" => polar("unity-empty", false),
            "_NOT_EMPTY" => polar("unity-empty", true),
            r if r.starts_with("_NOT_EQUAL") => equality("unity-equal", true),
            r if r.starts_with("_EQUAL") => equality("unity-equal", false),
            r if r.starts_with("_GREATER_THAN") => threshold(">"),
            r if r.starts_with("_GREATER_OR_EQUAL") => threshold(">="),
            r if r.starts_with("_LESS_THAN") => threshold("<"),
            r if r.starts_with("_LESS_OR_EQUAL") => threshold("<="),
            _ => return None,
        });
    }
    Some(match name {
        "CHECK" | "REQUIRE" => condition("catch-check", false),
        "assert_true" => condition("cmocka-true", false),
        "assert_false" => condition("cmocka-true", true),
        "assert_null" => polar("cmocka-null", false),
        "assert_non_null" => polar("cmocka-null", true),
        "assert_in_range" => polar("cmocka-range", false),
        "assert_not_in_range" => polar("cmocka-range", true),
        "assert_in_set" => polar("cmocka-set", false),
        "assert_not_in_set" => polar("cmocka-set", true),
        "Eq" => equality("gmock-eq", false),
        "Ne" => equality("gmock-eq", true),
        "Lt" => compare("<"),
        "Le" => compare("<="),
        "Gt" => compare(">"),
        "Ge" => compare(">="),
        _ => {
            let typed = name.strip_prefix("assert_")?;
            if let Some(t) = typed.strip_suffix("_not_equal") {
                return CMOCKA_TYPES
                    .contains(&t)
                    .then(|| equality("cmocka-equal", true));
            }
            let t = typed.strip_suffix("_equal")?;
            return CMOCKA_TYPES
                .contains(&t)
                .then(|| equality("cmocka-equal", false));
        }
    })
}

/// The value types of CMocka's `assert_<type>_equal` / `assert_<type>_not_equal`.
const CMOCKA_TYPES: &[&str] = &["int", "uint", "ptr", "string", "memory", "float", "double"];

fn c_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| n.kind() == "number_literal";
    match n.kind() {
        "string_literal"
        | "raw_string_literal"
        | "concatenated_string"
        | "char_literal"
        | "true"
        | "false"
        | "null"
        | "nullptr" => true,
        "unary_expression" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("argument").is_some_and(num)
        }
        _ => num(n),
    }
}

/// The expressions of a C/C++ call's arguments.
fn c_args(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(|a| {
            let mut c = a.walk();
            a.named_children(&mut c)
                .filter(|n| !n.is_extra())
                .map(unparenthesized)
                .collect()
        })
        .unwrap_or_default()
}

/// The identifier a C/C++ call names, a qualified name's last part (`testing::Gt`).
fn c_callee(call: Node) -> Option<Node> {
    let mut f = call.child_by_field_name("function")?;
    while f.kind() == "qualified_identifier" {
        f = f.child_by_field_name("name")?;
    }
    (f.kind() == "identifier").then_some(f)
}

/// C/C++: the call `name` names, read as [`c_check`] says: its name masked, the literal
/// of a comparison masked, and the comparisons of its condition ([`c_operands`]).
fn c_record(records: &mut Records<Comparison>, stmt: Node, call: Node, name: Node, src: &str) {
    let method = text(name, src);
    let Some(check) = c_check(method) else {
        return;
    };
    let args = c_args(call);
    let literal = match check.compare {
        Some(_) if check.threshold_first => args.first().copied().filter(|a| c_literal(*a, src)),
        Some(_) => args
            .iter()
            .take(2)
            .rev()
            .copied()
            .find(|a| c_literal(*a, src)),
        None => None,
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if let (Some(fails), Some(cond)) = (check.condition, args.first()) {
        c_operands(records, stmt, *cond, src, fails, &|n| c_literal(n, src));
    }
}

/// C/C++: comparisons and polarity in GoogleTest, Catch2, doctest, Unity and CMocka
/// assertions ([`c_check`]), the GoogleMock comparison matcher of an `EXPECT_THAT` /
/// `ASSERT_THAT`, and the comparisons of an `assert(..)` condition, each read from a
/// statement that is the call (a GoogleTest `<< "message"` after it kept).
pub fn c_cpp(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "expression_statement" {
            return true;
        }
        let mut expr = node.named_child(0);
        while let Some(b) = expr.filter(|e| {
            e.kind() == "binary_expression"
                && e.child_by_field_name("operator")
                    .is_some_and(|o| text(o, src) == "<<")
        }) {
            expr = b.child_by_field_name("left");
        }
        let Some(call) = expr.filter(|e| e.kind() == "call_expression") else {
            return true;
        };
        let Some(name) = c_callee(call) else {
            return true;
        };
        match text(name, src) {
            "assert" => {
                if let Some(cond) = c_args(call).first() {
                    c_operands(records, node, *cond, src, false, &|n| c_literal(n, src));
                }
            }
            "EXPECT_THAT" | "ASSERT_THAT" => {
                let matcher = c_args(call).get(1).copied();
                if let Some((m, n)) = matcher
                    .filter(|m| m.kind() == "call_expression")
                    .and_then(|m| Some((m, c_callee(m)?)))
                {
                    if c_check(text(n, src)).is_some_and(|c| c.compare.is_some()) {
                        c_record(records, node, m, n, src);
                    }
                }
            }
            "Eq" | "Ne" | "Lt" | "Le" | "Gt" | "Ge" => return true,
            _ if c_check(text(name, src)).is_some() => c_record(records, node, call, name, src),
            _ => return true,
        }
        false
    });
}

/// PHP PHPUnit methods with a negated twin, read as a polarity, as (method, twin): the
/// method masked.
const PHP_TWINS: &[(&str, &str)] = &[
    ("assertTrue", "assertFalse"),
    ("assertNull", "assertNotNull"),
    ("assertSame", "assertNotSame"),
    ("assertEquals", "assertNotEquals"),
    (
        "assertEqualsCanonicalizing",
        "assertNotEqualsCanonicalizing",
    ),
    ("assertEqualsIgnoringCase", "assertNotEqualsIgnoringCase"),
    ("assertEqualsWithDelta", "assertNotEqualsWithDelta"),
    ("assertContains", "assertNotContains"),
    ("assertContainsEquals", "assertNotContainsEquals"),
    ("assertContainsOnly", "assertNotContainsOnly"),
    ("assertCount", "assertNotCount"),
    ("assertEmpty", "assertNotEmpty"),
    ("assertArrayHasKey", "assertArrayNotHasKey"),
    ("assertInstanceOf", "assertNotInstanceOf"),
    ("assertSameSize", "assertNotSameSize"),
    (
        "assertStringContainsString",
        "assertStringNotContainsString",
    ),
    (
        "assertStringContainsStringIgnoringCase",
        "assertStringNotContainsStringIgnoringCase",
    ),
    ("assertStringStartsWith", "assertStringStartsNotWith"),
    ("assertStringEndsWith", "assertStringEndsNotWith"),
    (
        "assertMatchesRegularExpression",
        "assertDoesNotMatchRegularExpression",
    ),
    ("assertFileExists", "assertFileDoesNotExist"),
    ("assertDirectoryExists", "assertDirectoryDoesNotExist"),
    ("assertIsArray", "assertIsNotArray"),
    ("assertIsBool", "assertIsNotBool"),
    ("assertIsInt", "assertIsNotInt"),
    ("assertIsString", "assertIsNotString"),
    ("assertIsObject", "assertIsNotObject"),
    ("assertIsCallable", "assertIsNotCallable"),
    ("assertNan", ""),
];

/// PHP PHPUnit methods read as `==` / `!=`, the literal they are given masked.
const PHP_EQUALITY: &[&str] = &[
    "assertSame",
    "assertNotSame",
    "assertEquals",
    "assertNotEquals",
];

/// PHP PHPUnit methods read as a comparison of their second argument with their first,
/// the threshold, which is the literal masked.
const PHP_ORDERED: &[&str] = &[
    "assertGreaterThan",
    "assertGreaterThanOrEqual",
    "assertLessThan",
    "assertLessThanOrEqual",
];

/// Pest `expect(..)` matchers whose `->not` form is read as the polarity inverted; the
/// comparison matchers are JS's ([`JS_COMPARISON_MATCHERS`]).
const PEST_POLAR_MATCHERS: &[&str] = &[
    "toBeTrue",
    "toBeFalse",
    "toBeTruthy",
    "toBeFalsy",
    "toBeNull",
    "toBeEmpty",
    "toBeInstanceOf",
    "toBeArray",
    "toBeString",
    "toBeInt",
    "toBeBool",
    "toBeNumeric",
    "toBeIn",
    "toContain",
    "toHaveCount",
    "toHaveKey",
    "toHaveKeys",
    "toHaveLength",
    "toHaveProperty",
    "toMatch",
    "toMatchArray",
    "toStartWith",
    "toEndWith",
    "toThrow",
];

/// PHP: the polarity of a PHPUnit method of [`PHP_TWINS`].
fn php_polarity(operator: &str) -> Option<(&'static str, bool)> {
    PHP_TWINS.iter().find_map(|(m, twin)| {
        if *m == operator {
            Some((*m, false))
        } else if !twin.is_empty() && *twin == operator {
            Some((*m, true))
        } else {
            None
        }
    })
}

fn php_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| matches!(n.kind(), "integer" | "float");
    match n.kind() {
        "string" | "boolean" | "null" => true,
        // A double-quoted string without interpolation.
        "encapsed_string" => {
            let mut c = n.walk();
            let plain = n
                .named_children(&mut c)
                .all(|k| matches!(k.kind(), "string_content" | "escape_sequence"));
            plain
        }
        "unary_op_expression" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("argument").is_some_and(num)
        }
        _ => num(n),
    }
}

/// The expressions of a PHP call's arguments, a named argument's value included.
fn php_args(call: Node) -> Vec<Node> {
    let Some(list) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut c = list.walk();
    list.named_children(&mut c)
        .filter(|a| a.kind() == "argument")
        .filter_map(|a| {
            let mut ac = a.walk();
            let value = a
                .named_children(&mut ac)
                .filter(|n| !n.is_extra())
                .last()
                .map(unparenthesized);
            value
        })
        .collect()
}

/// PHP: a PHPUnit assertion on `$this`, on `self`, `static` or `parent`, on `Assert` or
/// `TestCase`, or called as a function: the method of [`PHP_TWINS`], [`PHP_EQUALITY`] and
/// [`PHP_ORDERED`] masked with the literal of an equality or an ordering, and the
/// comparisons of an `assertTrue` / `assertFalse` condition, `assertFalse` read as a
/// failure condition.
fn php_assert(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let owned = match call.kind() {
        "member_call_expression" => call
            .child_by_field_name("object")
            .is_some_and(|o| text(o, src) == "$this"),
        "scoped_call_expression" => call.child_by_field_name("scope").is_some_and(|s| {
            let scope = text(s, src);
            s.kind() == "relative_scope"
                || matches!(
                    scope.rsplit('\\').next().unwrap_or(scope),
                    "Assert" | "TestCase"
                )
        }),
        "function_call_expression" => true,
        _ => false,
    };
    let name = match call.kind() {
        "function_call_expression" => call
            .child_by_field_name("function")
            .filter(|f| f.kind() == "name"),
        _ => call.child_by_field_name("name"),
    };
    let Some(name) = name.filter(|_| owned) else {
        return false;
    };
    let method = text(name, src);
    let args = php_args(call);
    if method == "assert" && call.kind() == "function_call_expression" {
        if let Some(cond) = args.first() {
            c_operands(records, stmt, *cond, src, false, &|n| php_literal(n, src));
        }
        return true;
    }
    let equality = PHP_EQUALITY.contains(&method);
    let ordered = PHP_ORDERED.contains(&method);
    if !ordered && php_polarity(method).is_none() {
        return false;
    }
    let literal = if equality {
        args.iter().take(2).copied().find(|a| php_literal(*a, src))
    } else if ordered {
        args.first().copied().filter(|a| php_literal(*a, src))
    } else {
        None
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if matches!(method, "assertTrue" | "assertFalse") {
        if let Some(cond) = args.first() {
            let fails = method == "assertFalse";
            c_operands(records, stmt, *cond, src, fails, &|n| php_literal(n, src));
        }
    }
    true
}

/// PHP: each matcher of a Pest `expect(..)` chain that is in [`JS_COMPARISON_MATCHERS`]
/// or [`PEST_POLAR_MATCHERS`], its name and the `->not` before it masked and, for a
/// comparison, its literal argument. `->not` negates the matcher after it; `->and(..)`
/// starts the next expectation.
fn php_expect(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    // The chain's links from the outermost in, and the call it is rooted at.
    let mut links = Vec::new();
    let mut cur = call;
    loop {
        match cur.kind() {
            "member_call_expression" | "member_access_expression" => {
                links.push(cur);
                match cur.child_by_field_name("object") {
                    Some(o) => cur = o,
                    None => return false,
                }
            }
            "function_call_expression" => break,
            _ => return false,
        }
    }
    let rooted = cur
        .child_by_field_name("function")
        .is_some_and(|f| text(f, src) == "expect");
    if !rooted || links.is_empty() {
        return false;
    }
    let mut not = None;
    for link in links.into_iter().rev() {
        let Some(name) = link.child_by_field_name("name") else {
            continue;
        };
        let matcher = text(name, src);
        if link.kind() == "member_access_expression" {
            if matcher == "not" {
                not = Some(link);
            }
            continue;
        }
        let negation = not.take();
        let compared = JS_COMPARISON_MATCHERS.contains(&matcher);
        if !compared && !PEST_POLAR_MATCHERS.contains(&matcher) {
            continue;
        }
        let literal = php_args(link)
            .first()
            .copied()
            .filter(|a| compared && php_literal(*a, src));
        let mut holes = vec![(name.start_byte(), name.end_byte(), "@")];
        if let Some(n) = negation {
            let inner = n
                .child_by_field_name("object")
                .map_or(n.start_byte(), |o| o.end_byte());
            holes.push((inner, n.end_byte(), ""));
        }
        if let Some(l) = literal {
            holes.push((l.start_byte(), l.end_byte(), "#"));
        }
        records.add(Comparison {
            line: name.start_position().row + 1,
            skeleton: spliced(stmt, &mut holes, src),
            operator: if negation.is_some() {
                format!("not->{matcher}")
            } else {
                matcher.to_string()
            },
            literal: literal.map(|l| text(l, src).to_string()),
            fails_when_true: false,
        });
    }
    true
}

/// PHP: comparisons and polarity in PHPUnit assertions ([`php_assert`]), Pest
/// `expect(..)` chains ([`php_expect`]) and the condition of `assert(..)`, each read
/// from a statement that is the call.
pub fn php(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "expression_statement" {
            return true;
        }
        let Some(call) = node.named_child(0) else {
            return true;
        };
        !(php_expect(records, node, call, src) || php_assert(records, node, call, src))
    });
}

/// Kotest matchers read as a comparison, infix (`x shouldBe 6`) or called
/// (`x.shouldBe(6)`): the literal they are given is masked.
const KOTEST_COMPARISON: &[&str] = &[
    "shouldBe",
    "shouldNotBe",
    "shouldBeGreaterThan",
    "shouldBeGreaterThanOrEqual",
    "shouldBeLessThan",
    "shouldBeLessThanOrEqual",
];

/// Kotest matchers with a negated twin, as (matcher, twin): `shouldBe` -> `shouldNotBe`
/// with the same literal kept is an inverted check. A matcher outside both lists is not
/// read.
const KOTEST_TWINS: &[(&str, &str)] = &[
    ("shouldBe", "shouldNotBe"),
    ("shouldBeTrue", "shouldBeFalse"),
    ("shouldBeNull", "shouldNotBeNull"),
    ("shouldBeEmpty", "shouldNotBeEmpty"),
    ("shouldBeBlank", "shouldNotBeBlank"),
    ("shouldBeZero", "shouldNotBeZero"),
    ("shouldContain", "shouldNotContain"),
    ("shouldContainAll", "shouldNotContainAll"),
    ("shouldContainKey", "shouldNotContainKey"),
    ("shouldContainValue", "shouldNotContainValue"),
    ("shouldContainExactly", "shouldNotContainExactly"),
    ("shouldStartWith", "shouldNotStartWith"),
    ("shouldEndWith", "shouldNotEndWith"),
    ("shouldMatch", "shouldNotMatch"),
    ("shouldHaveSize", "shouldNotHaveSize"),
    ("shouldHaveLength", "shouldNotHaveLength"),
    ("shouldBeIn", "shouldNotBeIn"),
    ("shouldBeSameInstanceAs", "shouldNotBeSameInstanceAs"),
    ("shouldBeEqualIgnoringCase", "shouldNotBeEqualIgnoringCase"),
];

/// Kotlin: the polarity of a Kotest matcher of [`KOTEST_TWINS`].
fn kotest_polarity(operator: &str) -> Option<(&'static str, bool)> {
    KOTEST_TWINS.iter().find_map(|(m, twin)| {
        if *m == operator {
            Some((*m, false))
        } else if *twin == operator {
            Some((*m, true))
        } else {
            None
        }
    })
}

fn kt_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| matches!(n.kind(), "number_literal" | "float_literal");
    match n.kind() {
        "character_literal" | "boolean_literal" | "null_literal" => true,
        "identifier" => matches!(text(n, src), "true" | "false" | "null"),
        // A string without a template.
        "string_literal" => {
            let mut c = n.walk();
            let plain = n
                .named_children(&mut c)
                .all(|k| matches!(k.kind(), "string_content" | "escape_sequence"))
                && !text(n, src).contains('$');
            plain
        }
        "unary_expression" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("argument").is_some_and(num)
        }
        _ => num(n),
    }
}

/// The expressions of a Kotlin call's arguments, a named argument's value included.
fn kt_args(call: Node) -> Vec<Node> {
    let mut c = call.walk();
    let Some(list) = call
        .named_children(&mut c)
        .find(|n| n.kind() == "value_arguments")
    else {
        return Vec::new();
    };
    let mut lc = list.walk();
    list.named_children(&mut lc)
        .filter(|a| a.kind() == "value_argument")
        .filter_map(|a| {
            let mut ac = a.walk();
            let value = a
                .named_children(&mut ac)
                .filter(|n| !n.is_extra())
                .last()
                .map(unparenthesized);
            value
        })
        .collect()
}

/// A Kotlin call's callee and the identifier it names: `assertTrue` of `assertTrue(..)`,
/// (`x.shouldBe`, `shouldBe`) of `x.shouldBe(6)`.
fn kt_callee(call: Node) -> Option<(Node, Node)> {
    let callee = call.named_child(0)?;
    let name = match callee.kind() {
        "identifier" => callee,
        "navigation_expression" => callee
            .named_child(callee.named_child_count().checked_sub(1)?)
            .filter(|n| n.kind() == "identifier")?,
        _ => return None,
    };
    Some((callee, name))
}

/// Kotlin: a kotlin.test / JUnit assertion call, unqualified or on `Assertions`, `Assert`
/// or `AssertJUnit`: the method of [`JAVA_POLAR`] and [`JAVA_EQUALITY`] masked, the
/// literal of an equality masked, the comparisons of an `assertTrue` / `assertFalse`
/// condition (`assertFalse` read as a failure condition), and the comparisons of an
/// `assert(..)` condition.
fn kt_junit(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let Some((callee, name)) = kt_callee(call) else {
        return false;
    };
    let method = text(name, src);
    if callee.kind() == "navigation_expression" {
        let owner = callee.named_child(0).map_or("", |o| text(o, src));
        let owner = owner.rsplit('.').next().unwrap_or(owner);
        if !matches!(owner, "Assertions" | "Assert" | "AssertJUnit") {
            return false;
        }
    }
    let args = kt_args(call);
    if method == "assert" && callee.kind() == "identifier" {
        if let Some(cond) = args.first() {
            c_operands(records, stmt, *cond, src, false, &|n| kt_literal(n, src));
        }
        return true;
    }
    let polar = JAVA_POLAR.contains(&method);
    let equality = JAVA_EQUALITY.contains(&method);
    if !polar && !equality {
        return false;
    }
    // A JUnit 4 message is a string literal before the operands: `assertTrue("m", ok)`,
    // `assertEquals("m", 3, n)`; `assertEquals("a", s, "m")` compares the first.
    let message_first = args.first().is_some_and(|a| a.kind() == "string_literal")
        && if equality {
            args.len() >= 3 && args.last().is_some_and(|l| l.kind() != "string_literal")
        } else {
            args.len() >= 2
        };
    let operands = if message_first { &args[1..] } else { &args[..] };
    let literal = if equality {
        operands
            .iter()
            .take(2)
            .copied()
            .find(|a| kt_literal(*a, src))
    } else {
        None
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if matches!(method, "assertTrue" | "assertFalse") {
        if let Some(cond) = operands.first() {
            c_operands(records, stmt, *cond, src, method == "assertFalse", &|n| {
                kt_literal(n, src)
            });
        }
    }
    true
}

/// Kotlin: each method of an AssertJ / Truth `assertThat(..)` (or `then(..)`) chain that
/// is in [`JAVA_CHAIN_COMPARISON`] or [`JAVA_CHAIN_TWINS`], its name masked and, for a
/// comparison, its literal argument.
fn kt_chain(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let mut links = Vec::new();
    let mut cur = call;
    let rooted = loop {
        let Some((callee, name)) = kt_callee(cur) else {
            break false;
        };
        if callee.kind() == "identifier" {
            break matches!(text(name, src), "assertThat" | "then");
        }
        links.push((cur, name));
        match callee.named_child(0) {
            Some(r) if r.kind() == "call_expression" => cur = r,
            _ => break false,
        }
    };
    if !rooted || links.is_empty() {
        return false;
    }
    for (link, name) in links.into_iter().rev() {
        let method = text(name, src);
        let compared = JAVA_CHAIN_COMPARISON.contains(&method);
        if !compared && java_chain_polarity(method).is_none() {
            continue;
        }
        let literal = kt_args(link)
            .first()
            .copied()
            .filter(|a| compared && kt_literal(*a, src));
        let mut holes = vec![(name, '@')];
        holes.extend(literal.map(|l| (l, '#')));
        records.add(Comparison {
            line: name.start_position().row + 1,
            skeleton: skeleton(stmt, &mut holes, src),
            operator: method.to_string(),
            literal: literal.map(|l| text(l, src).to_string()),
            fails_when_true: false,
        });
    }
    true
}

/// Kotlin: a Kotest matcher of [`KOTEST_COMPARISON`] or [`KOTEST_TWINS`], infix
/// (`x shouldBe 6`) or called (`x.shouldBe(6)`): its name masked and, for a comparison,
/// its literal argument.
fn kt_kotest(records: &mut Records<Comparison>, stmt: Node, expr: Node, src: &str) -> bool {
    let (name, arg) = match expr.kind() {
        "infix_expression" => {
            let mut c = expr.walk();
            let kids: Vec<Node> = expr.named_children(&mut c).collect();
            match kids.as_slice() {
                [_, name, arg] if name.kind() == "identifier" => {
                    (*name, Some(unparenthesized(*arg)))
                }
                _ => return false,
            }
        }
        "call_expression" => match kt_callee(expr) {
            Some((callee, name)) if callee.kind() == "navigation_expression" => {
                (name, kt_args(expr).first().copied())
            }
            _ => return false,
        },
        _ => return false,
    };
    let matcher = text(name, src);
    let compared = KOTEST_COMPARISON.contains(&matcher);
    if !compared && kotest_polarity(matcher).is_none() {
        return false;
    }
    let literal = arg.filter(|a| compared && kt_literal(*a, src));
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: matcher.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    true
}

/// Kotlin: comparisons and polarity in kotlin.test / JUnit assertions ([`kt_junit`]),
/// AssertJ / Truth `assertThat(..)` chains ([`kt_chain`]) and Kotest matchers
/// ([`kt_kotest`]), each read from a statement that is the call or the infix expression.
pub fn kotlin(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        // The statements of a body: a call or an infix expression that is a child of
        // one, not an `if` condition.
        if !matches!(
            node.kind(),
            "block"
                | "lambda_literal"
                | "statements"
                | "if_expression"
                | "control_structure_body"
                | "function_body"
        ) {
            return true;
        }
        let condition = node.child_by_field_name("condition");
        let mut cursor = node.walk();
        let statements: Vec<Node> = node
            .named_children(&mut cursor)
            .filter(|c| matches!(c.kind(), "call_expression" | "infix_expression"))
            .filter(|c| Some(*c) != condition)
            .collect();
        for stmt in statements {
            // `assert(cond) { "message" }`: the call before the trailing lambda.
            let mut call = stmt;
            if let (Some(inner), Some(lambda)) = (stmt.named_child(0), stmt.named_child(1)) {
                if inner.kind() == "call_expression" && lambda.kind() == "annotated_lambda" {
                    call = inner;
                }
            }
            if stmt.kind() == "infix_expression" {
                kt_kotest(records, stmt, stmt, src);
            } else {
                let _ = kt_junit(records, stmt, call, src)
                    || kt_kotest(records, stmt, call, src)
                    || kt_chain(records, stmt, call, src);
            }
        }
        true
    });
}

/// Ruby Minitest assertions with a negated twin, as (assertion, twin): the method masked.
const RB_TWINS: &[(&str, &str)] = &[
    ("assert", "refute"),
    ("assert_equal", "refute_equal"),
    ("assert_nil", "refute_nil"),
    ("assert_empty", "refute_empty"),
    ("assert_includes", "refute_includes"),
    ("assert_match", "refute_match"),
    ("assert_same", "refute_same"),
    ("assert_kind_of", "refute_kind_of"),
    ("assert_instance_of", "refute_instance_of"),
    ("assert_predicate", "refute_predicate"),
    ("assert_respond_to", "refute_respond_to"),
    ("assert_in_delta", "refute_in_delta"),
    ("assert_in_epsilon", "refute_in_epsilon"),
    ("assert_operator", "refute_operator"),
    ("assert_path_exists", "refute_path_exists"),
];

/// RSpec matchers read as `==`, the literal they are given masked.
const RSPEC_EQUALITY: &[&str] = &["eq", "eql", "equal", "be"];

/// RSpec matchers whose `not_to` / `to_not` form is read as the polarity inverted, and
/// the twins `be_truthy` / `be_falsey`.
const RSPEC_POLAR: &[&str] = &[
    "be_truthy",
    "be_falsey",
    "be_falsy",
    "be_nil",
    "be_empty",
    "be_a",
    "be_an",
    "be_kind_of",
    "be_instance_of",
    "be_an_instance_of",
    "include",
    "match",
    "match_array",
    "contain_exactly",
    "start_with",
    "end_with",
    "have_key",
    "raise_error",
    "change",
    "exist",
    "satisfy",
    "respond_to",
];

/// RSpec: a runner (`to`, `not_to`, `to_not`) and the matcher after it, as recorded: `to
/// eq`, `not_to be >`.
fn rspec_operator(operator: &str) -> Option<(bool, &str)> {
    let (runner, matcher) = operator.split_once(' ')?;
    let not = match runner {
        "to" => false,
        "not_to" | "to_not" => true,
        _ => return None,
    };
    Some((not, matcher))
}

/// Ruby: the polarity of a Minitest assertion of [`RB_TWINS`] or an RSpec matcher of
/// [`RSPEC_POLAR`] after its runner.
fn rb_polarity(operator: &str) -> Option<(&'static str, bool)> {
    if let Some((not, matcher)) = rspec_operator(operator) {
        let (family, negated) = match matcher {
            "be_falsey" | "be_falsy" => ("be_truthy", true),
            _ => (*RSPEC_POLAR.iter().find(|m| **m == matcher)?, false),
        };
        return Some((family, negated != not));
    }
    RB_TWINS.iter().find_map(|(m, twin)| {
        if *m == operator {
            Some((*m, false))
        } else if *twin == operator {
            Some((*m, true))
        } else {
            None
        }
    })
}

/// Ruby: the operator an RSpec comparison (`to eq`, `not_to be >`) or a Minitest
/// `assert_operator` symbol (`:>`) must satisfy, a negation applied.
fn rb_passing(operator: &str, negate: bool) -> Option<&'static str> {
    let (not, op) = match rspec_operator(operator) {
        Some((not, m)) => (not, m.strip_prefix("be ").unwrap_or(m)),
        None => (false, operator.strip_prefix(':')?),
    };
    let op = if RSPEC_EQUALITY.contains(&op) {
        "=="
    } else {
        op
    };
    if !matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
        return None;
    }
    passing(op, negate != not)
}

fn rb_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| matches!(n.kind(), "integer" | "float");
    match n.kind() {
        "true" | "false" | "nil" | "simple_symbol" => true,
        // A string without interpolation.
        "string" => {
            let mut c = n.walk();
            let plain = n
                .named_children(&mut c)
                .all(|k| matches!(k.kind(), "string_content" | "escape_sequence"));
            plain
        }
        "unary" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("operand").is_some_and(num)
        }
        _ => num(n),
    }
}

/// The expressions of a Ruby call's arguments, a keyword argument left out.
fn rb_args(call: Node) -> Vec<Node> {
    let Some(list) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut c = list.walk();
    list.named_children(&mut c)
        .filter(|a| !a.is_extra() && a.kind() != "pair")
        .map(unparenthesized)
        .collect()
}

/// Ruby: a Minitest assertion of [`RB_TWINS`] called without a receiver: the method
/// masked, the literal of an `assert_equal` / `refute_equal` masked (read as `==` /
/// `!=`), an `assert_operator` read as its operator symbol with the literal masked, and
/// the comparisons of an `assert` / `refute` condition, `refute` read as a failure
/// condition.
fn rb_minitest(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    if call.child_by_field_name("receiver").is_some() {
        return false;
    }
    let Some(name) = call.child_by_field_name("method") else {
        return false;
    };
    let method = text(name, src);
    if rb_polarity(method).is_none() {
        return false;
    }
    let args = rb_args(call);
    if matches!(method, "assert_operator" | "refute_operator") {
        let (Some(op), Some(value)) = (args.get(1), args.get(2)) else {
            return true;
        };
        if op.kind() != "simple_symbol" || rb_passing(text(*op, src), false).is_none() {
            return true;
        }
        let literal = Some(*value).filter(|v| rb_literal(*v, src));
        let mut holes = vec![(*op, '@')];
        holes.extend(literal.map(|l| (l, '#')));
        records.add(Comparison {
            line: op.start_position().row + 1,
            skeleton: skeleton(stmt, &mut holes, src),
            operator: text(*op, src).to_string(),
            literal: literal.map(|l| text(l, src).to_string()),
            fails_when_true: method == "refute_operator",
        });
        return true;
    }
    let equality = matches!(method, "assert_equal" | "refute_equal");
    let literal = if equality {
        args.iter().take(2).copied().find(|a| rb_literal(*a, src))
    } else {
        None
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if matches!(method, "assert" | "refute") {
        if let Some(cond) = args.first() {
            c_operands(records, stmt, *cond, src, method == "refute", &|n| {
                rb_literal(n, src)
            });
        }
    }
    true
}

/// Ruby: an RSpec `expect(..)` (or `is_expected`) expectation: its runner (`to`,
/// `not_to`, `to_not`) and matcher masked, the matcher read as `==` ([`RSPEC_EQUALITY`]),
/// as the operator of `be > 6`, or as a polarity ([`RSPEC_POLAR`]), and the literal of
/// a comparison masked.
fn rb_rspec(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let (Some(runner), Some(receiver)) = (
        call.child_by_field_name("method"),
        call.child_by_field_name("receiver"),
    ) else {
        return false;
    };
    let expects = match receiver.kind() {
        "identifier" => text(receiver, src) == "is_expected",
        "call" => {
            receiver.child_by_field_name("receiver").is_none()
                && receiver
                    .child_by_field_name("method")
                    .is_some_and(|m| text(m, src) == "expect")
        }
        _ => false,
    };
    if !expects || !matches!(text(runner, src), "to" | "not_to" | "to_not") {
        return false;
    }
    let args = rb_args(call);
    let [matcher] = args.as_slice() else {
        return true;
    };
    // The matcher's start: `be` of `be > 6`, masked with its operator.
    let mut from = matcher.start_byte();
    let (name, literal) = match matcher.kind() {
        // `be > 6`
        "binary" => {
            let (Some(l), Some(op), Some(r)) = (
                matcher.child_by_field_name("left"),
                matcher.child_by_field_name("operator"),
                matcher.child_by_field_name("right").map(unparenthesized),
            ) else {
                return true;
            };
            if text(l, src) != "be" {
                return true;
            }
            (op, Some(r))
        }
        "call" if matcher.child_by_field_name("receiver").is_none() => {
            let Some(m) = matcher.child_by_field_name("method") else {
                return true;
            };
            from = m.start_byte();
            (m, rb_args(*matcher).first().copied())
        }
        "identifier" => (*matcher, None),
        _ => return true,
    };
    let written = if matcher.kind() == "binary" {
        format!("be {}", text(name, src))
    } else {
        text(name, src).to_string()
    };
    let operator = format!("{} {written}", text(runner, src));
    let compared = rb_passing(&operator, false).is_some();
    if !compared && rb_polarity(&operator).is_none() {
        return true;
    }
    let literal = literal.filter(|l| compared && rb_literal(*l, src));
    let mut holes = vec![
        (runner.start_byte(), runner.end_byte(), "@"),
        (from, name.end_byte(), "@"),
    ];
    holes.extend(literal.map(|l| (l.start_byte(), l.end_byte(), "#")));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: spliced(stmt, &mut holes, src),
        operator,
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    true
}

/// Ruby: comparisons and polarity in Minitest assertions ([`rb_minitest`]) and RSpec
/// expectations ([`rb_rspec`]), each read from a statement that is the call.
pub fn ruby(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if !matches!(
            node.kind(),
            "body_statement" | "block_body" | "then" | "else" | "begin" | "program"
        ) {
            return true;
        }
        let mut cursor = node.walk();
        let statements: Vec<Node> = node
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "call")
            .collect();
        for stmt in statements {
            let _ = rb_rspec(records, stmt, stmt, src) || rb_minitest(records, stmt, stmt, src);
        }
        true
    });
}

/// Swift and Objective-C: what an XCTest assertion or macro checks, as [`c_check`] says for C: the
/// operator its operands must satisfy, its polarity family, and whether its first
/// argument is a condition. A name outside these lists is not read.
fn xct_check(name: &str) -> Option<CCheck> {
    let compare = |op| CCheck {
        compare: Some(op),
        ..NO_CHECK
    };
    let polar = |family, not| CCheck {
        polar: Some((family, not)),
        ..NO_CHECK
    };
    let condition = |family, not| CCheck {
        polar: Some((family, not)),
        condition: Some(not),
        ..NO_CHECK
    };
    let equality = |family, not| CCheck {
        compare: Some(if not { "!=" } else { "==" }),
        polar: Some((family, not)),
        ..NO_CHECK
    };
    Some(match name.strip_prefix("XCTAssert")? {
        "" | "True" => condition("xct-true", false),
        "False" => condition("xct-true", true),
        "Equal" => equality("xct-equal", false),
        "NotEqual" => equality("xct-equal", true),
        "Identical" => polar("xct-identical", false),
        "NotIdentical" => polar("xct-identical", true),
        "Nil" => polar("xct-nil", false),
        "NotNil" => polar("xct-nil", true),
        "ThrowsError" | "Throws" => polar("xct-throws", false),
        "NoThrow" => polar("xct-throws", true),
        // Objective-C only.
        "EqualObjects" => equality("xct-equal-objects", false),
        "NotEqualObjects" => equality("xct-equal-objects", true),
        "EqualWithAccuracy" => polar("xct-accuracy", false),
        "NotEqualWithAccuracy" => polar("xct-accuracy", true),
        "GreaterThan" => compare(">"),
        "GreaterThanOrEqual" => compare(">="),
        "LessThan" => compare("<"),
        "LessThanOrEqual" => compare("<="),
        _ => return None,
    })
}

/// Swift comparison operators in a condition; `===` / `!==` are identity.
const SW_OPERATORS: &[&str] = &["==", "!=", "===", "!==", "<", "<=", ">", ">="];

fn sw_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| matches!(n.kind(), "integer_literal" | "real_literal");
    match n.kind() {
        "boolean_literal" | "nil_literal" => true,
        // A string without interpolation.
        "line_string_literal" => {
            let mut c = n.walk();
            let plain = n
                .named_children(&mut c)
                .all(|k| matches!(k.kind(), "line_str_text" | "str_escaped_char"));
            plain
        }
        "prefix_expression" => {
            n.child_by_field_name("operation")
                .is_some_and(|o| text(o, src) == "-")
                && n.child_by_field_name("target").is_some_and(num)
        }
        _ => num(n),
    }
}

/// A Swift expression without its parentheses: a one-element tuple.
fn sw_bare(mut n: Node) -> Node {
    while n.kind() == "tuple_expression" && n.named_child_count() == 1 {
        match n.named_child(0) {
            Some(inner) => n = inner,
            None => break,
        }
    }
    n
}

/// The value of each argument of a Swift call or macro.
fn sw_args(call: Node) -> Vec<Node> {
    let mut c = call.walk();
    let Some(list) = call
        .named_children(&mut c)
        .find(|n| n.kind() == "call_suffix")
        .and_then(|s| {
            let mut sc = s.walk();
            let list = s
                .named_children(&mut sc)
                .find(|n| n.kind() == "value_arguments");
            list
        })
    else {
        return Vec::new();
    };
    let mut lc = list.walk();
    list.named_children(&mut lc)
        .filter(|a| a.kind() == "value_argument")
        .filter_map(|a| a.child_by_field_name("value").map(sw_bare))
        .collect()
}

/// Swift: each comparison that is a whole operand of a condition, through `&&`, `||` and
/// parentheses. The grammar reads `a > 1 && b < 2` as `a > (1 && b < 2)`, so a comparison
/// with a joined operand is not read; neither is one inside a call's arguments or after
/// `!`.
fn sw_operands(records: &mut Records<Comparison>, stmt: Node, cond: Node, src: &str, fails: bool) {
    let cond = sw_bare(cond);
    let (Some(l), Some(op), Some(r)) = (
        cond.child_by_field_name("lhs").map(sw_bare),
        cond.child_by_field_name("op"),
        cond.child_by_field_name("rhs").map(sw_bare),
    ) else {
        return;
    };
    match cond.kind() {
        "conjunction_expression" | "disjunction_expression" => {
            sw_operands(records, stmt, l, src, fails);
            sw_operands(records, stmt, r, src, fails);
        }
        "comparison_expression" | "equality_expression" | "infix_expression"
            if SW_OPERATORS.contains(&text(op, src)) =>
        {
            let joined = |n: Node| {
                matches!(
                    n.kind(),
                    "conjunction_expression"
                        | "disjunction_expression"
                        | "comparison_expression"
                        | "equality_expression"
                        | "infix_expression"
                )
            };
            if !joined(l) && !joined(r) {
                record(
                    records,
                    stmt,
                    (l, op, r),
                    &|n| sw_literal(n, src),
                    src,
                    fails,
                );
            }
        }
        _ => {}
    }
}

/// Swift: an XCTest assertion ([`xct_check`]): its name masked, the literal of an
/// equality (either argument) or an ordering (the second) masked, and the comparisons of
/// its condition ([`sw_operands`]).
fn sw_xctest(records: &mut Records<Comparison>, stmt: Node, call: Node, src: &str) -> bool {
    let Some(name) = call
        .named_child(0)
        .filter(|n| n.kind() == "simple_identifier")
    else {
        return false;
    };
    let method = text(name, src);
    let Some(check) = xct_check(method) else {
        return false;
    };
    let args = sw_args(call);
    let literal = match (check.compare, check.polar) {
        (Some(_), Some(_)) => args.iter().take(2).copied().find(|a| sw_literal(*a, src)),
        (Some(_), None) => args.get(1).copied().filter(|a| sw_literal(*a, src)),
        _ => None,
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if let (Some(fails), Some(cond)) = (check.condition, args.first()) {
        sw_operands(records, stmt, *cond, src, fails);
    }
    true
}

/// Swift: comparisons and polarity in XCTest assertions ([`sw_xctest`]) and the
/// comparisons of a Swift Testing `#expect(..)` / `#require(..)` condition, each read
/// from a statement that is the call (after `try` or not).
pub fn swift(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "statements" {
            return true;
        }
        let mut cursor = node.walk();
        let statements: Vec<Node> = node.named_children(&mut cursor).collect();
        for stmt in statements {
            let mut expr = stmt;
            while expr.kind() == "try_expression" || expr.kind() == "await_expression" {
                match expr.child_by_field_name("expr") {
                    Some(e) => expr = e,
                    None => break,
                }
            }
            match expr.kind() {
                "call_expression" => {
                    sw_xctest(records, stmt, expr, src);
                }
                "macro_invocation" => {
                    let is_expectation = expr
                        .named_child(0)
                        .is_some_and(|n| matches!(text(n, src), "expect" | "require"));
                    if let Some(cond) = sw_args(expr).first().filter(|_| is_expectation) {
                        sw_operands(records, stmt, *cond, src, false);
                    }
                }
                _ => {}
            }
        }
        true
    });
}

/// ScalaTest matcher words a call expands into (`equal (6)`, `contain ("a")`).
const SC_WORDS: &[&str] = &[
    "be",
    "equal",
    "===",
    "contain",
    "startWith",
    "endWith",
    "include",
];

/// ScalaTest matchers read as a polarity, `not` / `shouldNot` negating.
const SC_POLAR: &[&str] = &["contain", "startWith", "endWith", "include"];

/// ScalaTest: a matcher phrase (`should not be >`, `shouldBe`) as whether it is negated
/// and the matcher after `should` / `shouldNot` / `should not`.
fn sc_phrase(operator: &str) -> Option<(bool, String)> {
    let words: Vec<&str> = operator.split(' ').collect();
    let (not, rest): (bool, &[&str]) = match words.as_slice() {
        ["shouldBe"] => return Some((false, "be".into())),
        ["shouldEqual"] => return Some((false, "equal".into())),
        ["shouldNot", rest @ ..] => (true, rest),
        ["should", "not", rest @ ..] => (true, rest),
        ["should", rest @ ..] => (false, rest),
        _ => return None,
    };
    let matcher = rest.join(" ");
    let known = matches!(
        rest,
        ["be"] | ["equal"] | ["==="] | ["be", "<" | "<=" | ">" | ">="]
    ) || matches!(rest, [m] if SC_POLAR.contains(m));
    known.then_some((not, matcher))
}

/// ScalaTest: the operator a matcher phrase must satisfy, a negation applied.
fn sc_passing(operator: &str, negate: bool) -> Option<&'static str> {
    let (not, matcher) = sc_phrase(operator)?;
    let op = match matcher.as_str() {
        "be" | "equal" | "===" => "==",
        "be <" => "<",
        "be <=" => "<=",
        "be >" => ">",
        "be >=" => ">=",
        _ => return None,
    };
    passing(op, negate != not)
}

/// ScalaTest: the polarity of a matcher phrase: an equality (`shouldBe` / `should not
/// be`) or a matcher of [`SC_POLAR`].
fn sc_polarity(operator: &str) -> Option<(&'static str, bool)> {
    let (not, matcher) = sc_phrase(operator)?;
    let family = match matcher.as_str() {
        // `shouldBe` is read first as Kotest's (`kotest_polarity`), under its own name.
        "be" | "equal" | "===" => "shouldBe",
        m => *SC_POLAR.iter().find(|p| **p == m)?,
    };
    Some((family, not))
}

fn sc_literal(n: Node, src: &str) -> bool {
    let num = |n: Node| matches!(n.kind(), "integer_literal" | "floating_point_literal");
    match n.kind() {
        "string" | "boolean_literal" | "character_literal" | "null_literal" => true,
        "prefix_expression" => {
            let mut c = n.walk();
            let kids: Vec<Node> = n.children(&mut c).collect();
            matches!(kids.as_slice(), [op, operand] if text(*op, src) == "-" && num(*operand))
        }
        _ => num(n),
    }
}

/// The expressions of a Scala call's arguments.
fn sc_args(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .filter(|a| a.kind() == "arguments")
        .map(|a| {
            let mut c = a.walk();
            a.named_children(&mut c)
                .filter(|n| !n.is_extra())
                .map(unparenthesized)
                .collect()
        })
        .unwrap_or_default()
}

/// ScalaTest: the words of an infix matcher statement, the subject first and the value
/// last: `f(rows) should not be > 6` is `f(rows)`, `should`, `not`, `be`, `>`, `6`.
fn sc_words<'t>(n: Node<'t>, src: &str, out: &mut Vec<Node<'t>>) {
    match n.kind() {
        "infix_expression" => {
            let (Some(l), Some(op), Some(r)) = (
                n.child_by_field_name("left"),
                n.child_by_field_name("operator"),
                n.child_by_field_name("right"),
            ) else {
                out.push(n);
                return;
            };
            sc_words(l, src, out);
            out.push(op);
            sc_words(r, src, out);
        }
        "call_expression" => {
            let function = n.child_by_field_name("function");
            let args = sc_args(n);
            match (function, args.as_slice()) {
                (Some(f), [arg]) if SC_WORDS.contains(&text(f, src)) => {
                    out.push(f);
                    out.push(*arg);
                }
                _ => out.push(n),
            }
        }
        _ => out.push(unparenthesized(n)),
    }
}

/// Scala: a ScalaTest infix matcher (`x shouldBe 6`, `x should be > 6`, `x should not
/// equal (6)`, `x should contain ("a")`): the phrase between the subject and the value
/// masked as the operator, and the value when it is a literal.
fn sc_matcher(records: &mut Records<Comparison>, stmt: Node, src: &str) -> bool {
    let mut words = Vec::new();
    sc_words(stmt, src, &mut words);
    let [_, phrase @ .., value] = words.as_slice() else {
        return false;
    };
    if phrase.is_empty()
        || !phrase
            .iter()
            .all(|w| matches!(w.kind(), "identifier" | "operator_identifier"))
    {
        return false;
    }
    let operator = phrase
        .iter()
        .map(|w| text(*w, src))
        .collect::<Vec<_>>()
        .join(" ");
    let compared = sc_passing(&operator, false).is_some();
    if !compared && sc_polarity(&operator).is_none() {
        return false;
    }
    let (first, last) = (phrase[0], phrase[phrase.len() - 1]);
    // Only spaces and a parenthesis between the phrase and the value: the grammar reads
    // `x should not be > 6` as `be` given `> 6`, which is not the phrase it looks like.
    let gap = src.get(last.end_byte()..value.start_byte()).unwrap_or("");
    if !gap.chars().all(|c| c.is_whitespace() || c == '(') {
        return false;
    }
    let literal = Some(*value).filter(|v| compared && sc_literal(*v, src));
    let mut holes = vec![(first.start_byte(), last.end_byte(), "@")];
    holes.extend(literal.map(|l| (l.start_byte(), l.end_byte(), "#")));
    records.add(Comparison {
        line: first.start_position().row + 1,
        skeleton: spliced(stmt, &mut holes, src),
        operator,
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    true
}

/// Scala: an `assert(..)` condition's comparisons, and a MUnit / JUnit assertion of
/// [`JAVA_POLAR`] and [`JAVA_EQUALITY`] called without a receiver: the method masked,
/// the literal of an equality masked, and the comparisons of an `assertTrue` /
/// `assertFalse` condition, `assertFalse` read as a failure condition.
fn sc_assert(records: &mut Records<Comparison>, stmt: Node, src: &str) -> bool {
    let Some(name) = stmt
        .child_by_field_name("function")
        .filter(|f| f.kind() == "identifier")
    else {
        return false;
    };
    let method = text(name, src);
    let args = sc_args(stmt);
    if method == "assert" {
        if let Some(cond) = args.first() {
            c_operands(records, stmt, *cond, src, false, &|n| sc_literal(n, src));
        }
        return true;
    }
    let equality = JAVA_EQUALITY.contains(&method);
    if !equality && !JAVA_POLAR.contains(&method) {
        return false;
    }
    let literal = if equality {
        args.iter().take(2).copied().find(|a| sc_literal(*a, src))
    } else {
        None
    };
    let mut holes = vec![(name, '@')];
    holes.extend(literal.map(|l| (l, '#')));
    records.add(Comparison {
        line: name.start_position().row + 1,
        skeleton: skeleton(stmt, &mut holes, src),
        operator: method.to_string(),
        literal: literal.map(|l| text(l, src).to_string()),
        fails_when_true: false,
    });
    if matches!(method, "assertTrue" | "assertFalse") {
        if let Some(cond) = args.first() {
            c_operands(records, stmt, *cond, src, method == "assertFalse", &|n| {
                sc_literal(n, src)
            });
        }
    }
    true
}

/// Scala: comparisons and polarity in `assert(..)` and MUnit / JUnit assertions
/// ([`sc_assert`]) and ScalaTest infix matchers ([`sc_matcher`]), each read from a
/// statement of a block.
pub fn scala(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if !matches!(node.kind(), "block" | "indented_block") {
            return true;
        }
        let mut cursor = node.walk();
        let statements: Vec<Node> = node.named_children(&mut cursor).collect();
        for stmt in statements {
            match stmt.kind() {
                "call_expression" => {
                    sc_assert(records, stmt, src);
                }
                "infix_expression" => {
                    sc_matcher(records, stmt, src);
                }
                _ => {}
            }
        }
        true
    });
}

fn objc_literal(n: Node, src: &str) -> bool {
    c_literal(n, src)
        || (n.kind() == "identifier" && matches!(text(n, src), "YES" | "NO" | "nil" | "Nil"))
}

/// Objective-C: an XCTest assertion macro ([`xct_check`]) or an `NSAssert` / `NSCAssert`
/// / `assert(..)` condition: the macro masked, the literal of an equality (either
/// argument) or an ordering (the second) masked, and the comparisons of its condition
/// ([`c_operands`]).
pub fn objc(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "expression_statement" {
            return true;
        }
        let Some(call) = node
            .named_child(0)
            .filter(|c| c.kind() == "call_expression")
        else {
            return true;
        };
        let Some(name) = call
            .child_by_field_name("function")
            .filter(|f| f.kind() == "identifier")
        else {
            return true;
        };
        let method = text(name, src);
        let args = c_args(call);
        if matches!(method, "NSAssert" | "NSCAssert" | "assert") {
            if let Some(cond) = args.first() {
                c_operands(records, node, *cond, src, false, &|n| objc_literal(n, src));
            }
            return false;
        }
        let Some(check) = xct_check(method) else {
            return true;
        };
        let literal = match (check.compare, check.polar) {
            (Some(_), Some(_)) => args.iter().take(2).copied().find(|a| objc_literal(*a, src)),
            (Some(_), None) => args.get(1).copied().filter(|a| objc_literal(*a, src)),
            _ => None,
        };
        let mut holes = vec![(name, '@')];
        holes.extend(literal.map(|l| (l, '#')));
        records.add(Comparison {
            line: name.start_position().row + 1,
            skeleton: skeleton(node, &mut holes, src),
            operator: method.to_string(),
            literal: literal.map(|l| text(l, src).to_string()),
            fails_when_true: false,
        });
        if let (Some(fails), Some(cond)) = (check.condition, args.first()) {
            c_operands(records, node, *cond, src, fails, &|n| objc_literal(n, src));
        }
        false
    });
}

/// The operator a comparison must satisfy for the test to pass: the operator itself in
/// an assertion, its negation in a failure condition.
fn passing(operator: &str, fails_when_true: bool) -> Option<&'static str> {
    // A JS/TS matcher after `.not`, or a Pest one after `->not`, checks the negation.
    let (operator, not) = match negated_matcher(operator) {
        Some(m) => (m, true),
        None => (operator, false),
    };
    let (op, negated) = match operator {
        "==" => ("==", "!="),
        "!=" => ("!=", "=="),
        "<" => ("<", ">="),
        "<=" => ("<=", ">"),
        ">" => (">", "<="),
        ">=" => (">=", "<"),
        "in" => ("in", "not in"),
        "not in" => ("not in", "in"),
        "assert_eq!" => ("==", "!="),
        "assert_ne!" => ("!=", "=="),
        "===" => ("==", "!="),
        "!==" => ("!=", "=="),
        "toBe" | "toEqual" | "toStrictEqual" => ("==", "!="),
        "toBeGreaterThan" => (">", "<="),
        "toBeGreaterThanOrEqual" => (">=", "<"),
        "toBeLessThan" => ("<", ">="),
        "toBeLessThanOrEqual" => ("<=", ">"),
        "assert.equal" | "assert.strictEqual" | "assert.deepEqual" | "assert.deepStrictEqual" => {
            ("==", "!=")
        }
        "assert.notEqual"
        | "assert.notStrictEqual"
        | "assert.notDeepEqual"
        | "assert.notDeepStrictEqual" => ("!=", "=="),
        "assertEquals" | "isEqualTo" | "assertSame" => ("==", "!="),
        "assertNotEquals" | "isNotEqualTo" | "assertNotSame" => ("!=", "=="),
        "assertGreaterThan" => (">", "<="),
        "assertGreaterThanOrEqual" => (">=", "<"),
        "assertLessThan" => ("<", ">="),
        "assertLessThanOrEqual" => ("<=", ">"),
        "assert_equal" => ("==", "!="),
        "refute_equal" => ("!=", "=="),
        "shouldBe" => ("==", "!="),
        "shouldNotBe" => ("!=", "=="),
        "shouldBeGreaterThan" => (">", "<="),
        "shouldBeGreaterThanOrEqual" => (">=", "<"),
        "shouldBeLessThan" => ("<", ">="),
        "shouldBeLessThanOrEqual" => ("<=", ">"),
        "isGreaterThan" => (">", "<="),
        "isGreaterThanOrEqualTo" => (">=", "<"),
        "isLessThan" => ("<", ">="),
        "isLessThanOrEqualTo" => ("<=", ">"),
        "Equal" | "AreEqual" | "Be" => ("==", "!="),
        "NotEqual" | "AreNotEqual" | "NotBe" => ("!=", "=="),
        "Greater" | "BeGreaterThan" => (">", "<="),
        "GreaterOrEqual" | "BeGreaterThanOrEqualTo" | "BeGreaterOrEqualTo" => (">=", "<"),
        "Less" | "BeLessThan" => ("<", ">="),
        "LessOrEqual" | "BeLessThanOrEqualTo" | "BeLessOrEqualTo" => ("<=", ">"),
        _ => match c_check(operator)
            .or_else(|| xct_check(operator))
            .and_then(|c| c.compare)
        {
            Some(op) => return passing(op, fails_when_true != not),
            None => {
                return cs_constraint_passing(operator, fails_when_true != not)
                    .or_else(|| rb_passing(operator, fails_when_true != not))
                    .or_else(|| sc_passing(operator, fails_when_true != not))
            }
        },
    };
    Some(if fails_when_true != not { negated } else { op })
}

/// C#: the operator an NUnit comparison constraint must satisfy, `Not` negating it.
fn cs_constraint_passing(operator: &str, negate: bool) -> Option<&'static str> {
    let (_, name, not) = cs_constraint(operator)?;
    let (op, negated) = match name {
        "EqualTo" => ("==", "!="),
        "GreaterThan" => (">", "<="),
        "GreaterThanOrEqualTo" => (">=", "<"),
        "LessThan" => ("<", ">="),
        "LessThanOrEqualTo" => ("<=", ">"),
        _ => return None,
    };
    Some(if negate != not { negated } else { op })
}

/// The smallest (for a lower bound) or largest (for an upper bound) value the bound
/// accepts, and whether that value is itself accepted. Integers are read as integers, so
/// `> 5` and `>= 6` are the same bound.
fn endpoint(op: &str, literal: &str, integers: bool) -> Option<(f64, bool)> {
    let v = numeric(literal)?;
    Some(match (op, integers) {
        (">", true) => (v + 1.0, true),
        ("<", true) => (v - 1.0, true),
        (">" | "<", false) => (v, false),
        (">=" | "<=", _) => (v, true),
        _ => return None,
    })
}

fn integer(literal: &str) -> bool {
    numeric(literal).is_some_and(|v| v.fract() == 0.0) && !literal.contains(['.', 'e', 'E'])
}

/// Whether a comparison that must satisfy `base` against `base_lit` now accepts more
/// when it must satisfy `head` against `head_lit`. Only an operator change is judged: a
/// literal moved under the same operator is `super::bounds`'s or `super::expectations`'s.
fn accepts_more(base: &str, base_lit: Option<&str>, head: &str, head_lit: Option<&str>) -> bool {
    if base == head {
        return false;
    }
    let same_literal = base_lit == head_lit;
    match (base, head) {
        // Membership polarity flipped: a check of the other outcome.
        ("in", "not in") | ("not in", "in") => same_literal,
        // One accepted value became a range or every value but one.
        ("==", "!=" | "<" | "<=" | ">" | ">=") => true,
        // A bound that now accepts its own literal, or a value past it.
        (">" | ">=", ">" | ">=") | ("<" | "<=", "<" | "<=") => {
            let (Some(b), Some(h)) = (base_lit, head_lit) else {
                return false;
            };
            let integers = integer(b) && integer(h);
            let (Some((bv, b_in)), Some((hv, h_in))) =
                (endpoint(base, b, integers), endpoint(head, h, integers))
            else {
                return false;
            };
            let lower = base.starts_with('>');
            let past = if lower { hv < bv } else { hv > bv };
            past || (hv == bv && h_in && !b_in)
        }
        // A bound that became "anything but one value", which the bound excluded.
        (">" | ">=" | "<" | "<=", "!=") => {
            let (Some(b), Some(h)) = (base_lit, head_lit) else {
                return false;
            };
            let (Some(bv), Some(hv)) = (numeric(b), numeric(h)) else {
                return false;
            };
            match base {
                ">" => hv <= bv,
                ">=" => hv < bv,
                "<" => hv >= bv,
                _ => hv > bv,
            }
        }
        _ => false,
    }
}

/// Comparisons whose operator now accepts more from `base` to `head`. A skeleton that is
/// not exactly once on each side is ambiguous and skipped.
pub fn weakened(base: &[Comparison], head: &[Comparison]) -> Vec<Weakened> {
    type Tally<'c> = HashMap<(&'c str, bool), (usize, usize)>;
    fn tally(set: &[Comparison]) -> Tally<'_> {
        let mut out = Tally::new();
        for (at, c) in set.iter().enumerate() {
            out.entry((c.skeleton.as_str(), c.fails_when_true))
                .or_insert((0, at))
                .0 += 1;
        }
        out
    }
    let (in_base, in_head) = (tally(base), tally(head));
    let mut out = Vec::new();
    for h in head {
        let key = (h.skeleton.as_str(), h.fails_when_true);
        if in_head.get(&key).map(|(n, _)| *n) != Some(1) {
            continue;
        }
        let Some((1, at)) = in_base.get(&key).copied() else {
            continue;
        };
        let b = &base[at];
        // A polarity is judged against the same literal; a Java `assertEquals(0, n)` ->
        // `assertNotEquals(1, n)` is the comparison `== 0` -> `!= 1` below.
        if let (Some((bf, bn)), Some((hf, hn)), true) = (
            polarity(&b.operator),
            polarity(&h.operator),
            b.literal == h.literal,
        ) {
            if bf == hf && bn != hn {
                out.push(Weakened {
                    line: h.line,
                    from: b.operator.clone(),
                    to: h.operator.clone(),
                    fails_when_true: false,
                    inverted: true,
                });
            }
            continue;
        }
        let (Some(bp), Some(hp)) = (
            passing(&b.operator, b.fails_when_true),
            passing(&h.operator, h.fails_when_true),
        ) else {
            continue;
        };
        if accepts_more(bp, b.literal.as_deref(), hp, h.literal.as_deref()) {
            out.push(Weakened {
                line: h.line,
                from: b.operator.clone(),
                to: h.operator.clone(),
                fails_when_true: h.fails_when_true,
                inverted: false,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn comparisons(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<Comparison> {
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        facts
            .tests
            .iter()
            .flat_map(|t| t.comparisons.iter().cloned())
            .collect()
    }

    fn py(check: &str) -> Vec<Comparison> {
        let src = format!("def test_x():\n    rows = [1]\n    {check}\n");
        comparisons(&crate::ast::python::PythonPack, "tests/test_x.py", &src)
    }

    fn go(cond: &str) -> Vec<Comparison> {
        let src = format!(
            "package p\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {{\n\tif {cond} {{\n\t\tt.Fatalf(\"bad\")\n\t}}\n}}\n"
        );
        comparisons(&crate::ast::r#go::GoPack, "p_test.go", &src)
    }

    fn ops(base: &[Comparison], head: &[Comparison]) -> Vec<(String, String)> {
        weakened(base, head)
            .into_iter()
            .map(|w| (w.from, w.to))
            .collect()
    }

    fn pair(from: &str, to: &str) -> Vec<(String, String)> {
        vec![(from.to_string(), to.to_string())]
    }

    #[test]
    fn python_records_each_operator_with_its_literal_masked() {
        let got = py("assert 0 < f(rows) <= 5");
        let read: Vec<(&str, Option<&str>, &str)> = got
            .iter()
            .map(|c| {
                (
                    c.operator.as_str(),
                    c.literal.as_deref(),
                    c.skeleton.as_str(),
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                ("assert", None, "assert @ 0 < f(rows) <= 5"),
                ("<", Some("0"), "assert # @ f(rows) <= 5"),
                ("<=", Some("5"), "assert 0 < f(rows) @ #"),
            ]
        );
        // The polarity record (#1049) is checked by its own test below.
        let member: Vec<_> = py("assert \"a\" not in rows")
            .into_iter()
            .filter(|c| !c.operator.starts_with("assert"))
            .collect();
        assert_eq!(member.len(), 1);
        assert_eq!(member[0].operator, "not in");
        assert_eq!(member[0].skeleton, "assert # @ rows");
        assert!(!member[0].fails_when_true);
    }

    /// #1049: an inverted `self.assert*` method or a `not` added to or taken from a bare
    /// `assert` is reported; a method with no negated twin, a different family, another
    /// object than `self`, and an assertion whose operands changed too are not.
    #[test]
    fn python_assertions_with_inverted_polarity_are_reported() {
        let inverted = |base: &str, head: &str| {
            weakened(&py(base), &py(head))
                .into_iter()
                .map(|w| (w.from, w.to, w.inverted))
                .collect::<Vec<_>>()
        };
        for (base, head, from, to) in [
            (
                "self.assertFalse(issubclass(int, str))",
                "self.assertTrue(issubclass(int, str))",
                "assertFalse",
                "assertTrue",
            ),
            (
                "self.assertIs(f(rows), rows)",
                "self.assertIsNot(f(rows), rows)",
                "assertIs",
                "assertIsNot",
            ),
            (
                "self.assertIsNotNone(f(rows))",
                "self.assertIsNone(f(rows))",
                "assertIsNotNone",
                "assertIsNone",
            ),
            (
                "self.assertEquals(f(rows), 1)",
                "self.assertNotEqual(f(rows), 1)",
                "assertEquals",
                "assertNotEqual",
            ),
            (
                "self.assertIn(1, rows)",
                "self.assertNotIn(1, rows)",
                "assertIn",
                "assertNotIn",
            ),
            (
                "self.assertIsInstance(rows, list)",
                "self.assertNotIsInstance(rows, list)",
                "assertIsInstance",
                "assertNotIsInstance",
            ),
            (
                "assert f(rows)",
                "assert not f(rows)",
                "assert",
                "assert not",
            ),
            (
                "assert not (f(rows)), 'msg'",
                "assert f(rows), 'msg'",
                "assert not",
                "assert",
            ),
        ] {
            assert_eq!(
                inverted(base, head),
                vec![(from.to_string(), to.to_string(), true)],
                "{base} -> {head}"
            );
            assert!(inverted(base, base).is_empty(), "{base}");
        }
        for (base, head) in [
            // Another family, or a method with no negated twin.
            ("self.assertTrue(f(rows))", "self.assertIsNotNone(f(rows))"),
            (
                "self.assertGreater(f(rows), 1)",
                "self.assertLess(f(rows), 1)",
            ),
            // Not `self`.
            ("other.assertTrue(f(rows))", "other.assertFalse(f(rows))"),
            // The checked value changed as well.
            ("self.assertTrue(f(rows))", "self.assertFalse(g(rows))"),
            ("assert f(rows)", "assert not g(rows)"),
            // `is` / `is not` is the operand's comparison, not the assertion's polarity.
            ("assert f(rows) is None", "assert f(rows) is not None"),
        ] {
            assert!(inverted(base, head).is_empty(), "{base} -> {head}");
        }
    }

    #[test]
    fn python_operators_moved_the_loose_way_are_weakened() {
        for (base, head, from, to) in [
            ("assert f([]) == 0", "assert f([]) != 1", "==", "!="),
            ("assert f(rows) > 6", "assert f(rows) >= 6", ">", ">="),
            ("assert f([]) < 1", "assert f([]) <= 1", "<", "<="),
            (
                "assert \"a\" in rows",
                "assert \"a\" not in rows",
                "in",
                "not in",
            ),
            (
                "assert \"a\" not in rows",
                "assert \"a\" in rows",
                "not in",
                "in",
            ),
            ("assert f(rows) > 6", "assert f(rows) >= 5", ">", ">="),
            ("assert f(rows) > 6", "assert f(rows) != 6", ">", "!="),
            ("assert f(rows) < 0.5", "assert f(rows) <= 0.5", "<", "<="),
        ] {
            assert_eq!(
                ops(&py(base), &py(head)),
                pair(from, to),
                "{base} -> {head}"
            );
        }
    }

    #[test]
    fn python_operators_kept_or_tightened_are_not_weakened() {
        for (base, head) in [
            // The negative control of #934: a new expected value, the operator kept.
            ("assert f(rows) == 7", "assert f(rows) == 8"),
            ("assert f(rows) > 6", "assert f(rows) > 6"),
            // Tightened.
            ("assert f(rows) >= 6", "assert f(rows) > 6"),
            ("assert f(rows) != 1", "assert f(rows) == 1"),
            // The same set of integers.
            ("assert f(rows) > 5", "assert f(rows) >= 6"),
            ("assert f(rows) <= 4", "assert f(rows) < 5"),
            // Not a superset: the head excludes what the base accepted.
            ("assert f(rows) >= 6", "assert f(rows) != 6"),
            ("assert f(rows) > 6", "assert f(rows) < 6"),
            // Another operand: not the same assertion.
            ("assert f(rows) > 6", "assert g(rows) >= 6"),
        ] {
            assert!(ops(&py(base), &py(head)).is_empty(), "{base} -> {head}");
        }
    }

    #[test]
    fn an_assertion_written_twice_is_ambiguous() {
        let base = py("assert f(rows) > 6\n    assert f(rows) > 6");
        let head = py("assert f(rows) >= 6\n    assert f(rows) > 6");
        assert!(ops(&base, &head).is_empty());
    }

    #[test]
    fn go_guards_are_read_as_the_failure_condition() {
        assert_eq!(
            ops(&go("Sum(1, 2) != 3"), &go("Sum(1, 2) < 3")),
            pair("!=", "<")
        );
        // `>= 10` fails on 10, `> 10` lets it pass.
        assert_eq!(ops(&go("got >= 10"), &go("got > 10")), pair(">=", ">"));
        // Tightened guards: they fail on more.
        assert!(ops(&go("got > 10"), &go("got >= 10")).is_empty());
        assert!(ops(&go("got < 3"), &go("got != 3")).is_empty());
        assert!(ops(&go("got != 3"), &go("got != 4")).is_empty());
        // A block that does not fail the test is not a guard.
        let src = "package p\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {\n\tif got != 3 {\n\t\tt.Log(\"x\")\n\t}\n}\n";
        assert!(comparisons(&crate::ast::r#go::GoPack, "p_test.go", src).is_empty());
    }

    fn rs(check: &str) -> Vec<Comparison> {
        let src = format!("#[test]\nfn x() {{\n    let rows = vec![1];\n    {check}\n}}\n");
        comparisons(&crate::ast::rust::RustPack, "tests/x.rs", &src)
    }

    fn rs_ops(check: &str) -> Vec<(String, Option<String>)> {
        rs(check)
            .into_iter()
            .filter(|c| !c.operator.starts_with("assert"))
            .map(|c| (c.operator, c.literal))
            .collect()
    }

    fn op(o: &str, lit: Option<&str>) -> (String, Option<String>) {
        (o.to_string(), lit.map(str::to_string))
    }

    #[test]
    fn rust_records_each_comparison_of_an_assert_condition_with_its_literal() {
        assert_eq!(
            rs_ops("assert!(count(&rows) > 6, \"seven rows\");"),
            vec![op(">", Some("6"))]
        );
        assert_eq!(
            rs_ops("debug_assert!(ok && rows.len() >= -2);"),
            vec![op(">=", Some("-2"))]
        );
        assert_eq!(
            rs_ops("assert!((rows.len() == 1));"),
            vec![op("==", Some("1"))]
        );
        assert_eq!(rs_ops("assert!(a != b);"), vec![op("!=", None)]);
        // Not a comparison of the condition: a call's arguments, a turbofish, a negated
        // group, the message after the condition, a macro outside the `assert!` family.
        for check in [
            "assert!(check(rows.len() > 6));",
            "assert!(s.parse::<u32>().unwrap() > 6);",
            "assert!(!(rows.len() > 6));",
            "assert!(ok, \"{}\", rows.len() > 6);",
            "reassert!(rows.len() > 6);",
        ] {
            assert_eq!(rs_ops(check), vec![], "{check}");
        }
    }

    #[test]
    fn rust_comparisons_report_an_operator_that_accepts_more() {
        let w = |a: &str, b: &str| ops(&rs(a), &rs(b));
        assert_eq!(
            w("assert!(n(&rows) > 6);", "assert!(n(&rows) >= 6);"),
            pair(">", ">=")
        );
        assert_eq!(
            w("assert!(n(&rows) == 0);", "assert!(n(&rows) != 1);"),
            pair("==", "!=")
        );
        assert_eq!(
            w(
                "prop_assert!(n(&rows) < 5);",
                "prop_assert!(n(&rows) <= 5);"
            ),
            pair("<", "<=")
        );
        // Tightened, the same integer bound written another way, and a pair the macro
        // name tells apart: none accepts more.
        assert_eq!(
            w("assert!(n(&rows) >= 6);", "assert!(n(&rows) > 6);"),
            vec![]
        );
        assert_eq!(
            w("assert!(n(&rows) > 5);", "assert!(n(&rows) >= 6);"),
            vec![]
        );
        assert_eq!(
            w("assert!(n(&rows) > 6);", "debug_assert!(n(&rows) >= 6);"),
            vec![]
        );
        // `assert_eq!` / `assert_ne!` are `==` / `!=`: one accepted value became every
        // value but one, the literal free to move. The other way, or a pair whose macro
        // prefix differs, is not reported.
        assert_eq!(
            w("assert_eq!(n(&rows), 0);", "assert_ne!(n(&rows), 1);"),
            pair("assert_eq!", "assert_ne!")
        );
        assert_eq!(
            w(
                "debug_assert_eq!(0, n(&rows));",
                "debug_assert_ne!(0, n(&rows));"
            ),
            pair("assert_eq!", "assert_ne!")
        );
        assert_eq!(
            w("assert_ne!(n(&rows), 1);", "assert_eq!(n(&rows), 1);"),
            vec![]
        );
        assert_eq!(
            w("debug_assert_eq!(n(&rows), 0);", "assert_ne!(n(&rows), 0);"),
            vec![]
        );
    }

    #[test]
    fn rust_polarity_inversions_are_reported() {
        let inverted = |a: &str, b: &str| {
            weakened(&rs(a), &rs(b))
                .into_iter()
                .filter(|w| w.inverted)
                .map(|w| (w.from, w.to))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            inverted("assert!(ok(1), \"one\");", "assert!(!ok(1), \"one\");"),
            pair(RS_ASSERT, RS_ASSERT_NOT)
        );
        assert_eq!(
            inverted("assert!(!(a && b));", "assert!((a && b));"),
            pair(RS_ASSERT_NOT, RS_ASSERT)
        );
        // `!a && b` does not negate `a && b`; a prefix tells two macros apart; a checked
        // value that changed is not paired.
        assert_eq!(inverted("assert!(a && b);", "assert!(!a && b);"), vec![]);
        assert_eq!(inverted("assert!(ok(1));", "assert!(!ok(2));"), vec![]);
    }

    fn js(check: &str) -> Vec<Comparison> {
        let src = format!(
            "import {{ f }} from '../src/f.js';\n\nit('x', async () => {{\n  const rows = [1];\n  {check}\n}});\n"
        );
        comparisons(
            &crate::ast::javascript::JavaScriptPack,
            "test/x.test.ts",
            &src,
        )
    }

    fn js_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&js(a), &js(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn js_one(from: &str, to: &str, inverted: bool) -> Vec<(String, String, bool)> {
        vec![(from.to_string(), to.to_string(), inverted)]
    }

    #[test]
    fn javascript_records_matchers_and_assert_comparisons_with_literals_masked() {
        let got = js("expect(f(rows)).toBeGreaterThan(6);");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].skeleton, "expect(f(rows)).@(#);");
        assert_eq!(got[0].operator, "toBeGreaterThan");
        assert_eq!(got[0].literal.as_deref(), Some("6"));
        assert_eq!(got[0].line, 5);
        let got = js("await expect(f(rows)).resolves.not.toBe(-1);");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].skeleton, "await expect(f(rows)).resolves.@(#);");
        assert_eq!(got[0].operator, "not.toBe");
        assert_eq!(got[0].literal.as_deref(), Some("-1"));
        let got = js("assert(f(rows) === 'a' && rows.length < 2, 'msg');");
        let ops: Vec<_> = got
            .iter()
            .map(|c| (c.operator.as_str(), c.literal.as_deref()))
            .collect();
        assert_eq!(ops, vec![("===", Some("'a'")), ("<", Some("2"))]);
        let got = js("assert.strictEqual(f(rows), 3);");
        assert_eq!(got[0].skeleton, "assert.@(f(rows), #);");
        assert_eq!(got[0].operator, "assert.strictEqual");
        // A matcher outside the closed lists, a comparison inside a call's arguments or
        // after `!`, and a call that is not an assertion are not read.
        assert_eq!(js("expect(f(rows)).toSatisfy(ok);"), vec![]);
        assert_eq!(js("assert.ok(check(f(rows) > 6));").len(), 1);
        assert_eq!(js("assert.ok(check(f(rows) > 6));")[0].operator, JS_ASSERT);
        assert!(js("assert(!(f(rows) > 6));")
            .iter()
            .all(|c| c.operator == JS_ASSERT_NOT));
        assert_eq!(js("console.log(f(rows) > 6);"), vec![]);
        assert_eq!(js("other(f(rows)).toBe(6);"), vec![]);
    }

    #[test]
    fn javascript_comparisons_report_an_operator_that_accepts_more() {
        assert_eq!(
            js_weakened(
                "expect(f(rows)).toBeGreaterThan(6);",
                "expect(f(rows)).toBeGreaterThanOrEqual(6);"
            ),
            js_one("toBeGreaterThan", "toBeGreaterThanOrEqual", false)
        );
        assert_eq!(
            js_weakened("expect(f(rows)).toBe(0);", "expect(f(rows)).not.toBe(1);"),
            js_one("toBe", "not.toBe", false)
        );
        assert_eq!(
            js_weakened(
                "expect(f(rows)).toEqual(7);",
                "expect(f(rows)).toBeLessThanOrEqual(7);"
            ),
            js_one("toEqual", "toBeLessThanOrEqual", false)
        );
        // `.not` of a bound is the opposite bound: `not.toBeLessThan(6)` is `>= 6`.
        assert_eq!(
            js_weakened(
                "expect(f(rows)).toBeGreaterThan(6);",
                "expect(f(rows)).not.toBeLessThan(6);"
            ),
            js_one("toBeGreaterThan", "not.toBeLessThan", false)
        );
        assert_eq!(
            js_weakened("assert(f(rows) > 6, 'm');", "assert(f(rows) >= 6, 'm');"),
            js_one(">", ">=", false)
        );
        assert_eq!(
            js_weakened("assert.ok(f(rows) === 3);", "assert.ok(f(rows) !== 4);"),
            js_one("===", "!==", false)
        );
        assert_eq!(
            js_weakened(
                "assert.deepStrictEqual(f(rows), 3);",
                "assert.notDeepStrictEqual(f(rows), 4);"
            ),
            js_one("assert.deepStrictEqual", "assert.notDeepStrictEqual", false)
        );
        // Kept, tightened, `==` / `===` swapped, or integers that accept the same values.
        for (a, b) in [
            (
                "expect(f(rows)).toBeGreaterThanOrEqual(6);",
                "expect(f(rows)).toBeGreaterThan(6);",
            ),
            ("expect(f(rows)).toBe(6);", "expect(f(rows)).toBe(6);"),
            ("expect(f(rows)).toBe(6);", "expect(f(rows)).toEqual(6);"),
            ("assert(f(rows) == 6);", "assert(f(rows) === 6);"),
            ("assert(f(rows) > 5);", "assert(f(rows) >= 6);"),
            ("expect(f(rows)).not.toBe(1);", "expect(f(rows)).toBe(0);"),
        ] {
            assert_eq!(js_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn javascript_polarity_inversions_are_reported() {
        assert_eq!(
            js_weakened(
                "expect(rows).toContain(1);",
                "expect(rows).not.toContain(1);"
            ),
            js_one("toContain", "not.toContain", true)
        );
        assert_eq!(
            js_weakened(
                "expect(f(rows)).toBeTruthy();",
                "expect(f(rows)).toBeFalsy();"
            ),
            js_one("toBeTruthy", "toBeFalsy", true)
        );
        assert_eq!(
            js_weakened(
                "expect(f(rows)).not.toBeUndefined();",
                "expect(f(rows)).toBeUndefined();"
            ),
            js_one("not.toBeUndefined", "toBeUndefined", true)
        );
        assert_eq!(
            js_weakened("assert(f(rows), 'm');", "assert(!f(rows), 'm');"),
            js_one(JS_ASSERT, JS_ASSERT_NOT, true)
        );
        assert_eq!(
            js_weakened(
                "assert.throws(() => f(rows));",
                "assert.doesNotThrow(() => f(rows));"
            ),
            js_one("assert.throws", "assert.doesNotThrow", true)
        );
        // `!a && b` does not negate `a && b`; `!!x` is not read; a checked value that
        // changed is not paired; the same polarity twice is kept.
        for (a, b) in [
            ("assert(a && b);", "assert(!a && b);"),
            ("assert(f(rows));", "assert(!!f(rows));"),
            (
                "expect(rows).toContain(1);",
                "expect(rows).not.toContain(2);",
            ),
            (
                "expect(f(rows)).toBeTruthy();",
                "expect(f(rows)).not.toBeFalsy();",
            ),
            (
                "expect(f(rows)).toBeTruthy();",
                "expect(f(rows)).toBeDefined();",
            ),
        ] {
            assert_eq!(js_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn java(check: &str) -> Vec<Comparison> {
        let src = format!(
            "import static org.junit.jupiter.api.Assertions.*;\nimport org.junit.jupiter.api.Test;\n\nclass XTest {{\n    @Test\n    void x() {{\n        int[] rows = {{1}};\n        {check}\n    }}\n}}\n"
        );
        comparisons(
            &crate::ast::java::JavaPack,
            "src/test/java/XTest.java",
            &src,
        )
    }

    fn java_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&java(a), &java(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn java_read(check: &str) -> Vec<(String, Option<String>, String)> {
        java(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    fn read(op: &str, lit: Option<&str>, skeleton: &str) -> (String, Option<String>, String) {
        (
            op.to_string(),
            lit.map(str::to_string),
            skeleton.to_string(),
        )
    }

    #[test]
    fn java_records_junit_assertj_and_assert_comparisons_with_literals_masked() {
        let got = java("assertThat(f(rows)).isGreaterThan(6);");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].line, 8);
        assert_eq!(
            java_read("assertThat(f(rows)).isGreaterThan(6);"),
            vec![read(
                "isGreaterThan",
                Some("6"),
                "assertThat(f(rows)).@(#);"
            )]
        );
        // Each method of a chain is read, its own name and literal masked.
        assert_eq!(
            java_read("assertThat(f(rows)).as(\"n\").isGreaterThan(-1).isLessThan(9);"),
            vec![
                read(
                    "isGreaterThan",
                    Some("-1"),
                    "assertThat(f(rows)).as(\"n\").@(#).isLessThan(9);"
                ),
                read(
                    "isLessThan",
                    Some("9"),
                    "assertThat(f(rows)).as(\"n\").isGreaterThan(-1).@(#);"
                ),
            ]
        );
        assert_eq!(
            java_read("assertTrue(f(rows) > 6 && rows.length < 2, \"m\");"),
            vec![
                read(
                    "assertTrue",
                    None,
                    "@(f(rows) > 6 && rows.length < 2, \"m\");"
                ),
                read(
                    ">",
                    Some("6"),
                    "assertTrue(f(rows) @ # && rows.length < 2, \"m\");"
                ),
                read(
                    "<",
                    Some("2"),
                    "assertTrue(f(rows) > 6 && rows.length @ #, \"m\");"
                ),
            ]
        );
        assert_eq!(
            java_read("Assertions.assertEquals(3, f(rows));"),
            vec![read("assertEquals", Some("3"), "Assertions.@(#, f(rows));")]
        );
        // A JUnit 4 message comes first and is not the literal compared.
        assert_eq!(
            java_read("assertEquals(\"m\", 3, f(rows));"),
            vec![read("assertEquals", Some("3"), "@(\"m\", #, f(rows));")]
        );
        assert_eq!(
            java_read("assertEquals(\"m\", want, f(rows));"),
            vec![read("assertEquals", None, "@(\"m\", want, f(rows));")]
        );
        assert_eq!(
            java_read("assertTrue(\"m\", f(rows) > 6);"),
            vec![
                read("assertTrue", None, "@(\"m\", f(rows) > 6);"),
                read(">", Some("6"), "assertTrue(\"m\", f(rows) @ #);"),
            ]
        );
        assert_eq!(
            java_read("assertEquals(\"a\", s(rows), \"m\");"),
            vec![read("assertEquals", Some("\"a\""), "@(#, s(rows), \"m\");")]
        );
        assert_eq!(
            java_read("assert f(rows) >= 0 : \"m\";"),
            vec![read(">=", Some("0"), "assert f(rows) @ # : \"m\";")]
        );
        // A condition that is one value carries the statement's polarity.
        assert_eq!(
            java_read("assert ok(rows) : \"m\";"),
            vec![read(JAVA_ASSERT, None, "assert @ ok(rows) : \"m\";")]
        );
        // A method outside the closed lists, Hamcrest, a comparison inside a call's
        // arguments, a call on another receiver and a Mockito `then(..).should()` are
        // not read.
        for check in [
            "assertThat(f(rows)).satisfies(ok);",
            "assertThat(f(rows), greaterThan(6));",
            "System.out.println(f(rows) > 6);",
            "other.assertTrue(f(rows) > 6);",
            "then(rows).should().contains(1);",
        ] {
            assert_eq!(java(check), vec![], "{check}");
        }
        assert_eq!(
            java_read("assertTrue(check(f(rows) > 6));"),
            vec![read("assertTrue", None, "@(check(f(rows) > 6));")]
        );
    }

    #[test]
    fn java_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "assertThat(f(rows)).isGreaterThan(6);",
                "assertThat(f(rows)).isGreaterThanOrEqualTo(6);",
                "isGreaterThan",
                "isGreaterThanOrEqualTo",
            ),
            (
                "assertThat(f(rows)).isEqualTo(0);",
                "assertThat(f(rows)).isNotEqualTo(1);",
                "isEqualTo",
                "isNotEqualTo",
            ),
            (
                "assertThat(f(rows)).isLessThan(7);",
                "assertThat(f(rows)).isLessThanOrEqualTo(7);",
                "isLessThan",
                "isLessThanOrEqualTo",
            ),
            (
                "assertEquals(0, f(rows));",
                "assertNotEquals(1, f(rows));",
                "assertEquals",
                "assertNotEquals",
            ),
            (
                "assertTrue(f(rows) > 6);",
                "assertTrue(f(rows) >= 6);",
                ">",
                ">=",
            ),
            // `assertFalse` reads its condition as a failure condition: failing on
            // `<= 6` became failing on `< 6`, so 6 now passes.
            (
                "assertFalse(f(rows) <= 6);",
                "assertFalse(f(rows) < 6);",
                "<=",
                "<",
            ),
            ("assert f(rows) == 3;", "assert f(rows) != 4;", "==", "!="),
        ] {
            assert_eq!(
                java_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Tightened, kept, integers that accept the same values, a failure condition
        // tightened, and `!=` back to `==`.
        for (a, b) in [
            (
                "assertThat(f(rows)).isGreaterThanOrEqualTo(6);",
                "assertThat(f(rows)).isGreaterThan(6);",
            ),
            ("assertEquals(6, f(rows));", "assertEquals(6, f(rows));"),
            (
                "assertThat(f(rows)).isGreaterThan(5);",
                "assertThat(f(rows)).isGreaterThanOrEqualTo(6);",
            ),
            ("assertFalse(f(rows) < 6);", "assertFalse(f(rows) <= 6);"),
            (
                "assertThat(f(rows)).isNotEqualTo(1);",
                "assertThat(f(rows)).isEqualTo(0);",
            ),
        ] {
            assert_eq!(java_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn java_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "assertTrue(f(rows));",
                "assertFalse(f(rows));",
                "assertTrue",
                "assertFalse",
            ),
            (
                "assertNull(\"m\", f(rows));",
                "assertNotNull(\"m\", f(rows));",
                "assertNull",
                "assertNotNull",
            ),
            (
                "assertEquals(3, f(rows));",
                "assertNotEquals(3, f(rows));",
                "assertEquals",
                "assertNotEquals",
            ),
            (
                "assertThat(rows).contains(1);",
                "assertThat(rows).doesNotContain(1);",
                "contains",
                "doesNotContain",
            ),
            (
                "assertThat(f(rows)).isFalse();",
                "assertThat(f(rows)).isTrue();",
                "isFalse",
                "isTrue",
            ),
            (
                "assert f(rows) : \"m\";",
                "assert !f(rows) : \"m\";",
                JAVA_ASSERT,
                JAVA_ASSERT_NOT,
            ),
        ] {
            assert_eq!(
                java_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // `!a && b` does not negate `a && b`; a checked value that changed is not
        // paired; two methods of different families are not twins.
        for (a, b) in [
            ("assert a && b;", "assert !a && b;"),
            (
                "assertThat(rows).contains(1);",
                "assertThat(rows).doesNotContain(2);",
            ),
            ("assertTrue(f(rows));", "assertFalse(g(rows));"),
            (
                "assertThat(f(rows)).isTrue();",
                "assertThat(f(rows)).isNotNull();",
            ),
        ] {
            assert_eq!(java_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn cs(check: &str) -> Vec<Comparison> {
        let src = format!(
            "using Xunit;\n\npublic class XTests\n{{\n    [Fact]\n    public async Task X()\n    {{\n        {check}\n    }}\n}}\n"
        );
        comparisons(&crate::ast::csharp::CSharpPack, "tests/XTests.cs", &src)
    }

    fn cs_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&cs(a), &cs(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn cs_read(check: &str) -> Vec<(String, Option<String>, String)> {
        cs(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn csharp_records_xunit_nunit_fluent_comparisons_with_literals_masked() {
        let got = cs("F(rows).Should().BeGreaterThan(6);");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].line, 8);
        // Each method of a chain is read, its own name and literal masked.
        assert_eq!(
            cs_read("F(rows).Should().BeGreaterThan(-1).And.BeLessThan(9, \"n\");"),
            vec![
                read(
                    "BeGreaterThan",
                    Some("-1"),
                    "F(rows).Should().@(#).And.BeLessThan(9, \"n\");"
                ),
                read(
                    "BeLessThan",
                    Some("9"),
                    "F(rows).Should().BeGreaterThan(-1).And.@(#, \"n\");"
                ),
            ]
        );
        assert_eq!(
            cs_read("await act.Should().ThrowAsync<ArgumentException>();"),
            vec![read(
                "ThrowAsync",
                None,
                "await act.Should().@<ArgumentException>();"
            )]
        );
        assert_eq!(
            cs_read("Assert.True(F(rows) > 6 && rows.Length < 2L, \"m\");"),
            vec![
                read(
                    "True",
                    None,
                    "Assert.@(F(rows) > 6 && rows.Length < 2L, \"m\");"
                ),
                read(
                    ">",
                    Some("6"),
                    "Assert.True(F(rows) @ # && rows.Length < 2L, \"m\");"
                ),
                read(
                    "<",
                    Some("2L"),
                    "Assert.True(F(rows) > 6 && rows.Length @ #, \"m\");"
                ),
            ]
        );
        assert_eq!(
            cs_read("Assert.Equal(3, F(rows));"),
            vec![read("Equal", Some("3"), "Assert.@(#, F(rows));")]
        );
        assert_eq!(
            cs_read("ClassicAssert.AreEqual(\"a\", S(rows), \"m\");"),
            vec![read(
                "AreEqual",
                Some("\"a\""),
                "ClassicAssert.@(#, S(rows), \"m\");"
            )]
        );
        // NUnit orders its first argument against its second.
        assert_eq!(
            cs_read("Assert.GreaterOrEqual(F(rows), 6.5);"),
            vec![read("GreaterOrEqual", Some("6.5"), "Assert.@(F(rows), #);")]
        );
        assert_eq!(
            cs_read("Assert.That(F(rows), Is.Not.EqualTo(-3));"),
            vec![read(
                "Is.Not.EqualTo",
                Some("-3"),
                "Assert.That(F(rows), @(#));"
            )]
        );
        assert_eq!(
            cs_read("Assert.That(F(rows), Is.Not.Null, \"m\");"),
            vec![read("Is.Not.Null", None, "Assert.That(F(rows), @, \"m\");")]
        );
        assert_eq!(
            cs_read("Assert.That(F(rows) >= 0, \"m\");"),
            vec![read(">=", Some("0"), "Assert.That(F(rows) @ #, \"m\");")]
        );
        // A method outside the closed lists, a constraint chained further, a
        // comparison inside a call's arguments, a call on another receiver, a chain
        // without `.Should()` and Shouldly are not read.
        for check in [
            "F(rows).Should().BeOneOf(1, 2);",
            "Assert.That(F(rows), Is.EqualTo(6).Within(1));",
            "Console.WriteLine(F(rows) > 6);",
            "other.True(F(rows) > 6);",
            "F(rows).Must().BeGreaterThan(6);",
            "F(rows).ShouldBeGreaterThan(6);",
        ] {
            assert_eq!(cs(check), vec![], "{check}");
        }
        assert_eq!(
            cs_read("Assert.True(Check(F(rows) > 6));"),
            vec![read("True", None, "Assert.@(Check(F(rows) > 6));")]
        );
    }

    #[test]
    fn csharp_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "F(rows).Should().BeGreaterThan(6);",
                "F(rows).Should().BeGreaterThanOrEqualTo(6);",
                "BeGreaterThan",
                "BeGreaterThanOrEqualTo",
            ),
            (
                "F(rows).Should().Be(0);",
                "F(rows).Should().NotBe(1);",
                "Be",
                "NotBe",
            ),
            (
                "Assert.Equal(0, F(rows));",
                "Assert.NotEqual(1, F(rows));",
                "Equal",
                "NotEqual",
            ),
            (
                "Assert.Less(F(rows), 7);",
                "Assert.LessOrEqual(F(rows), 7);",
                "Less",
                "LessOrEqual",
            ),
            (
                "Assert.That(F(rows), Is.GreaterThan(6));",
                "Assert.That(F(rows), Is.GreaterThanOrEqualTo(6));",
                "Is.GreaterThan",
                "Is.GreaterThanOrEqualTo",
            ),
            (
                "Assert.That(F(rows), Is.EqualTo(3));",
                "Assert.That(F(rows), Is.Not.EqualTo(4));",
                "Is.EqualTo",
                "Is.Not.EqualTo",
            ),
            (
                "Assert.True(F(rows) > 6);",
                "Assert.True(F(rows) >= 6);",
                ">",
                ">=",
            ),
            // `False` reads its condition as a failure condition: failing on `<= 6`
            // became failing on `< 6`, so 6 now passes.
            (
                "Assert.IsFalse(F(rows) <= 6);",
                "Assert.IsFalse(F(rows) < 6);",
                "<=",
                "<",
            ),
        ] {
            assert_eq!(
                cs_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Tightened, kept, integers that accept the same values, a failure condition
        // tightened, `!=` back to `==`, and a `Not` constraint tightened.
        for (a, b) in [
            (
                "F(rows).Should().BeGreaterThanOrEqualTo(6);",
                "F(rows).Should().BeGreaterThan(6);",
            ),
            ("Assert.Equal(6, F(rows));", "Assert.Equal(6, F(rows));"),
            (
                "F(rows).Should().BeGreaterThan(5);",
                "F(rows).Should().BeGreaterThanOrEqualTo(6);",
            ),
            ("Assert.False(F(rows) < 6);", "Assert.False(F(rows) <= 6);"),
            ("F(rows).Should().NotBe(1);", "F(rows).Should().Be(0);"),
            (
                "Assert.That(F(rows), Is.Not.LessThanOrEqualTo(6));",
                "Assert.That(F(rows), Is.Not.LessThan(7));",
            ),
        ] {
            assert_eq!(cs_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn csharp_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "Assert.True(F(rows));",
                "Assert.False(F(rows));",
                "True",
                "False",
            ),
            (
                "Assert.IsNull(F(rows), \"m\");",
                "Assert.IsNotNull(F(rows), \"m\");",
                "IsNull",
                "IsNotNull",
            ),
            (
                "Assert.AreEqual(3, F(rows));",
                "Assert.AreNotEqual(3, F(rows));",
                "AreEqual",
                "AreNotEqual",
            ),
            (
                "Assert.Contains(1, rows);",
                "Assert.DoesNotContain(1, rows);",
                "Contains",
                "DoesNotContain",
            ),
            (
                "rows.Should().Contain(1);",
                "rows.Should().NotContain(1);",
                "Contain",
                "NotContain",
            ),
            (
                "act.Should().Throw<ArgumentException>();",
                "act.Should().NotThrow<ArgumentException>();",
                "Throw",
                "NotThrow",
            ),
            (
                "Assert.That(F(rows), Is.Null);",
                "Assert.That(F(rows), Is.Not.Null);",
                "Is.Null",
                "Is.Not.Null",
            ),
            (
                "Assert.That(F(rows), Is.False);",
                "Assert.That(F(rows), Is.True);",
                "Is.False",
                "Is.True",
            ),
        ] {
            assert_eq!(
                cs_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two methods of different
        // families are not twins; `Is.Not.False` is `Is.True`.
        for (a, b) in [
            (
                "Assert.Contains(1, rows);",
                "Assert.DoesNotContain(2, rows);",
            ),
            ("Assert.True(F(rows));", "Assert.False(G(rows));"),
            (
                "F(rows).Should().BeTrue();",
                "F(rows).Should().NotBeNull();",
            ),
            (
                "Assert.That(F(rows), Is.True);",
                "Assert.That(F(rows), Is.Not.False);",
            ),
        ] {
            assert_eq!(cs_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn cc(check: &str) -> Vec<Comparison> {
        if check.contains("TEST_ASSERT") || check.contains("assert_") {
            let src = format!("void test_x(void) {{\n    int rows[] = {{1}};\n    {check}\n}}\n");
            return comparisons(&crate::ast::c_cpp::CPack, "tests/test_x.c", &src);
        }
        let src = format!("TEST(X, Y) {{\n    int rows[] = {{1}};\n    {check}\n}}\n");
        comparisons(&crate::ast::c_cpp::CppPack, "tests/XTest.cpp", &src)
    }

    fn cc_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&cc(a), &cc(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn cc_read(check: &str) -> Vec<(String, Option<String>, String)> {
        cc(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn c_cpp_records_gtest_catch2_unity_cmocka_comparisons_with_literals_masked() {
        let got = cc_read("EXPECT_GT(F(rows), 6) << \"seven rows\";");
        assert_eq!(
            got,
            vec![read(
                "EXPECT_GT",
                Some("6"),
                "@(F(rows), #) << \"seven rows\";"
            )]
        );
        assert_eq!(
            cc_read("ASSERT_EQ(3, F(rows));"),
            vec![read("ASSERT_EQ", Some("3"), "@(#, F(rows));")]
        );
        assert_eq!(
            cc_read("EXPECT_TRUE(F(rows) > 6 && G(rows) <= -1);"),
            vec![
                read("EXPECT_TRUE", None, "@(F(rows) > 6 && G(rows) <= -1);"),
                read(">", Some("6"), "EXPECT_TRUE(F(rows) @ # && G(rows) <= -1);"),
                read("<=", Some("-1"), "EXPECT_TRUE(F(rows) > 6 && G(rows) @ #);"),
            ]
        );
        assert_eq!(
            cc_read("REQUIRE(F(rows) != nullptr);"),
            vec![
                read("REQUIRE", None, "@(F(rows) != nullptr);"),
                read("!=", Some("nullptr"), "REQUIRE(F(rows) @ #);"),
            ]
        );
        assert_eq!(
            cc_read("CHECK_LT(F(rows), 2.5);"),
            vec![read("CHECK_LT", Some("2.5"), "@(F(rows), #);")]
        );
        assert_eq!(
            cc_read("EXPECT_THAT(F(rows), ::testing::Gt(6));"),
            vec![read(
                "Gt",
                Some("6"),
                "EXPECT_THAT(F(rows), ::testing::@(#));"
            )]
        );
        assert_eq!(
            cc_read("assert(F(rows) >= 6);"),
            vec![read(">=", Some("6"), "assert(F(rows) @ #);")]
        );
        // Unity's threshold comes first; `TEST_ASSERT_EQUAL_INT` reads the literal.
        assert_eq!(
            cc_read("TEST_ASSERT_GREATER_THAN(6, F(rows));"),
            vec![read(
                "TEST_ASSERT_GREATER_THAN",
                Some("6"),
                "@(#, F(rows));"
            )]
        );
        assert_eq!(
            cc_read("TEST_ASSERT_LESS_THAN(F(rows), 6);"),
            vec![read("TEST_ASSERT_LESS_THAN", None, "@(F(rows), 6);")]
        );
        assert_eq!(
            cc_read("TEST_ASSERT_EQUAL_INT(7, F(rows));"),
            vec![read("TEST_ASSERT_EQUAL_INT", Some("7"), "@(#, F(rows));")]
        );
        assert_eq!(
            cc_read("assert_int_equal(F(rows), 7);"),
            vec![read("assert_int_equal", Some("7"), "@(F(rows), #);")]
        );
        // Not read: a call outside the lists, a matcher outside `EXPECT_THAT`, a matcher
        // that is not a comparison, a comparison inside a call's arguments or in the
        // operand of a macro that takes no condition.
        for check in [
            "EXPECT_NEAR(F(rows), 6, 0.1);",
            "Gt(6);",
            "EXPECT_THAT(F(rows), IsEmpty());",
            "check(F(rows) > 6);",
            "assert_true_ish(F(rows) > 6);",
            "assert_bool_equal(F(rows), 6);",
        ] {
            assert_eq!(cc_read(check), vec![], "{check}");
        }
        assert_eq!(
            cc_read("EXPECT_TRUE(G(F(rows) > 6));"),
            vec![read("EXPECT_TRUE", None, "@(G(F(rows) > 6));")]
        );
        assert_eq!(
            cc_read("EXPECT_GT(F(rows) > 1, 0);"),
            vec![read("EXPECT_GT", Some("0"), "@(F(rows) > 1, #);")]
        );
    }

    #[test]
    fn c_cpp_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "EXPECT_GT(F(rows), 6);",
                "EXPECT_GE(F(rows), 6);",
                "EXPECT_GT",
                "EXPECT_GE",
            ),
            (
                "ASSERT_LT(F(rows), 6);",
                "ASSERT_LE(F(rows), 7);",
                "ASSERT_LT",
                "ASSERT_LE",
            ),
            (
                "EXPECT_EQ(F(rows), 0);",
                "EXPECT_NE(F(rows), 1);",
                "EXPECT_EQ",
                "EXPECT_NE",
            ),
            (
                "EXPECT_EQ(F(rows), 0);",
                "EXPECT_GE(F(rows), 0);",
                "EXPECT_EQ",
                "EXPECT_GE",
            ),
            ("REQUIRE(F(rows) > 6);", "REQUIRE(F(rows) >= 6);", ">", ">="),
            (
                "CHECK_GT(F(rows), 6);",
                "CHECK_GE(F(rows), 6);",
                "CHECK_GT",
                "CHECK_GE",
            ),
            (
                "EXPECT_TRUE(F(rows) == 7);",
                "EXPECT_TRUE(F(rows) >= 7);",
                "==",
                ">=",
            ),
            // A failure condition that now fails less often.
            (
                "EXPECT_FALSE(F(rows) <= 6);",
                "EXPECT_FALSE(F(rows) < 6);",
                "<=",
                "<",
            ),
            (
                "REQUIRE_FALSE(F(rows) <= 6);",
                "REQUIRE_FALSE(F(rows) < 6);",
                "<=",
                "<",
            ),
            (
                "EXPECT_THAT(F(rows), Gt(6));",
                "EXPECT_THAT(F(rows), Ge(6));",
                "Gt",
                "Ge",
            ),
            ("assert(F(rows) > 6);", "assert(F(rows) >= 6);", ">", ">="),
            (
                "TEST_ASSERT_GREATER_THAN(6, F(rows));",
                "TEST_ASSERT_GREATER_OR_EQUAL(6, F(rows));",
                "TEST_ASSERT_GREATER_THAN",
                "TEST_ASSERT_GREATER_OR_EQUAL",
            ),
            (
                "TEST_ASSERT_EQUAL_INT(0, F(rows));",
                "TEST_ASSERT_NOT_EQUAL_INT(1, F(rows));",
                "TEST_ASSERT_EQUAL_INT",
                "TEST_ASSERT_NOT_EQUAL_INT",
            ),
            (
                "TEST_ASSERT(F(rows) > 6);",
                "TEST_ASSERT(F(rows) >= 6);",
                ">",
                ">=",
            ),
            (
                "assert_int_equal(F(rows), 0);",
                "assert_int_not_equal(F(rows), 1);",
                "assert_int_equal",
                "assert_int_not_equal",
            ),
        ] {
            assert_eq!(
                cc_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept, moved to an integer that accepts the same
        // values, `!=` back to `==`, a failure condition tightened, a checked value that
        // changed.
        for (a, b) in [
            ("EXPECT_GE(F(rows), 6);", "EXPECT_GT(F(rows), 6);"),
            ("EXPECT_GT(F(rows), 6);", "ASSERT_GT(F(rows), 6);"),
            ("EXPECT_GT(F(rows), 6);", "EXPECT_GE(F(rows), 7);"),
            ("EXPECT_NE(F(rows), 1);", "EXPECT_EQ(F(rows), 2);"),
            ("EXPECT_FALSE(F(rows) < 6);", "EXPECT_FALSE(F(rows) <= 6);"),
            ("EXPECT_GT(F(rows), 6);", "EXPECT_GE(G(rows), 6);"),
            (
                "TEST_ASSERT_GREATER_OR_EQUAL(6, F(rows));",
                "TEST_ASSERT_GREATER_THAN(6, F(rows));",
            ),
            (
                "EXPECT_THAT(F(rows), Ge(6));",
                "EXPECT_THAT(F(rows), Gt(6));",
            ),
        ] {
            assert_eq!(cc_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn c_cpp_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "EXPECT_TRUE(F(rows));",
                "EXPECT_FALSE(F(rows));",
                "EXPECT_TRUE",
                "EXPECT_FALSE",
            ),
            (
                "ASSERT_EQ(F(rows), 3);",
                "ASSERT_NE(F(rows), 3);",
                "ASSERT_EQ",
                "ASSERT_NE",
            ),
            (
                "EXPECT_STREQ(F(rows), \"a\");",
                "EXPECT_STRNE(F(rows), \"a\");",
                "EXPECT_STREQ",
                "EXPECT_STRNE",
            ),
            (
                "EXPECT_ANY_THROW(F(rows));",
                "EXPECT_NO_THROW(F(rows));",
                "EXPECT_ANY_THROW",
                "EXPECT_NO_THROW",
            ),
            (
                "REQUIRE(F(rows));",
                "REQUIRE_FALSE(F(rows));",
                "REQUIRE",
                "REQUIRE_FALSE",
            ),
            (
                "CHECK_THROWS(F(rows));",
                "CHECK_NOTHROW(F(rows));",
                "CHECK_THROWS",
                "CHECK_NOTHROW",
            ),
            (
                "EXPECT_THAT(F(rows), Eq(3));",
                "EXPECT_THAT(F(rows), Ne(3));",
                "Eq",
                "Ne",
            ),
            (
                "TEST_ASSERT_TRUE(F(rows));",
                "TEST_ASSERT_FALSE(F(rows));",
                "TEST_ASSERT_TRUE",
                "TEST_ASSERT_FALSE",
            ),
            (
                "TEST_ASSERT_NULL(F(rows));",
                "TEST_ASSERT_NOT_NULL(F(rows));",
                "TEST_ASSERT_NULL",
                "TEST_ASSERT_NOT_NULL",
            ),
            (
                "assert_non_null(F(rows));",
                "assert_null(F(rows));",
                "assert_non_null",
                "assert_null",
            ),
        ] {
            assert_eq!(
                cc_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two macros of different families
        // are not twins; a test framework's own `TEST_ASSERT` -> `TEST_ASSERT_TRUE` keeps
        // the polarity.
        for (a, b) in [
            ("EXPECT_TRUE(F(rows));", "EXPECT_FALSE(G(rows));"),
            ("EXPECT_TRUE(F(rows));", "CHECK_FALSE(F(rows));"),
            (
                "EXPECT_STREQ(F(rows), \"a\");",
                "EXPECT_STRCASENE(F(rows), \"a\");",
            ),
            ("TEST_ASSERT(F(rows));", "TEST_ASSERT_TRUE(F(rows));"),
            ("assert_null(F(rows));", "assert_false(F(rows));"),
        ] {
            assert_eq!(cc_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn php(check: &str) -> Vec<Comparison> {
        let src = if check.starts_with("expect(") {
            format!("<?php\ntest('rows', function () {{\n    $rows = [1];\n    {check}\n}});\n")
        } else {
            format!(
                "<?php\nclass RowsTest extends TestCase {{\n    public function testX(): void {{\n        $rows = [1];\n        {check}\n    }}\n}}\n"
            )
        };
        comparisons(&crate::ast::php::PhpPack, "tests/RowsTest.php", &src)
    }

    fn php_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&php(a), &php(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn php_read(check: &str) -> Vec<(String, Option<String>, String)> {
        php(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn php_records_phpunit_pest_and_assert_comparisons_with_literals_masked() {
        assert_eq!(
            php_read("$this->assertGreaterThan(6, f($rows), 'seven rows');"),
            vec![read(
                "assertGreaterThan",
                Some("6"),
                "$this->@(#, f($rows), 'seven rows');"
            )]
        );
        assert_eq!(
            php_read("self::assertSame(3, f($rows));"),
            vec![read("assertSame", Some("3"), "self::@(#, f($rows));")]
        );
        assert_eq!(
            php_read("static::assertEquals(f($rows), -1.5);"),
            vec![read(
                "assertEquals",
                Some("-1.5"),
                "static::@(f($rows), #);"
            )]
        );
        assert_eq!(
            php_read("Assert::assertNotSame(\"a\", f($rows));"),
            vec![read(
                "assertNotSame",
                Some("\"a\""),
                "Assert::@(#, f($rows));"
            )]
        );
        assert_eq!(
            php_read("$this->assertTrue(f($rows) > 6 && g($rows) <= -1);"),
            vec![
                read(
                    "assertTrue",
                    None,
                    "$this->@(f($rows) > 6 && g($rows) <= -1);"
                ),
                read(
                    ">",
                    Some("6"),
                    "$this->assertTrue(f($rows) @ # && g($rows) <= -1);"
                ),
                read(
                    "<=",
                    Some("-1"),
                    "$this->assertTrue(f($rows) > 6 && g($rows) @ #);"
                ),
            ]
        );
        assert_eq!(
            php_read("$this->assertFalse((f($rows) === null) and $ok);"),
            vec![
                read(
                    "assertFalse",
                    None,
                    "$this->@((f($rows) === null) and $ok);"
                ),
                read(
                    "===",
                    Some("null"),
                    "$this->assertFalse((f($rows) @ #) and $ok);"
                ),
            ]
        );
        assert_eq!(
            php_read("parent::assertSame(3, f($rows));"),
            vec![read("assertSame", Some("3"), "parent::@(#, f($rows));")]
        );
        assert_eq!(
            php_read("assertSame(3, f($rows));"),
            vec![read("assertSame", Some("3"), "@(#, f($rows));")]
        );
        assert_eq!(
            php_read("$this->assertTrue($ok or f($rows) < 2);"),
            vec![
                read("assertTrue", None, "$this->@($ok or f($rows) < 2);"),
                read("<", Some("2"), "$this->assertTrue($ok or f($rows) @ #);"),
            ]
        );
        assert_eq!(
            php_read("assert(f($rows) >= 6);"),
            vec![read(">=", Some("6"), "assert(f($rows) @ #);")]
        );
        assert_eq!(
            php_read("$this->assertNull(f($rows));"),
            vec![read("assertNull", None, "$this->@(f($rows));")]
        );
        assert_eq!(
            php_read("expect(f($rows))->not->toBeGreaterThan(6);"),
            vec![read(
                "not->toBeGreaterThan",
                Some("6"),
                "expect(f($rows))->@(#);"
            )]
        );
        assert_eq!(
            php_read("expect(f($rows))->toBe(3)->and(g($rows))->toBeTrue();"),
            vec![
                read(
                    "toBe",
                    Some("3"),
                    "expect(f($rows))->@(#)->and(g($rows))->toBeTrue();"
                ),
                read(
                    "toBeTrue",
                    None,
                    "expect(f($rows))->toBe(3)->and(g($rows))->@();"
                ),
            ]
        );
        // Not read: a method outside the lists, a call on another object, an ordering
        // whose threshold is not a literal, a comparison inside a call's arguments or
        // after `!`, a matcher outside the lists, a chain not rooted at `expect(..)`.
        for check in [
            "$this->assertEqualsWithin(6, f($rows));",
            "$other->assertTrue(f($rows) > 6);",
            "check(f($rows) > 6);",
            "expect(f($rows))->toBeOne(6);",
            "want(f($rows))->toBe(6);",
        ] {
            assert_eq!(php_read(check), vec![], "{check}");
        }
        assert_eq!(
            php_read("$this->assertLessThan($max, f($rows));"),
            vec![read("assertLessThan", None, "$this->@($max, f($rows));")]
        );
        assert_eq!(
            php_read("$this->assertTrue(g(f($rows) > 6));"),
            vec![read("assertTrue", None, "$this->@(g(f($rows) > 6));")]
        );
        assert_eq!(
            php_read("$this->assertTrue(!(f($rows) > 6));"),
            vec![read("assertTrue", None, "$this->@(!(f($rows) > 6));")]
        );
    }

    #[test]
    fn php_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "$this->assertGreaterThan(6, f($rows));",
                "$this->assertGreaterThanOrEqual(6, f($rows));",
                "assertGreaterThan",
                "assertGreaterThanOrEqual",
            ),
            (
                "self::assertLessThan(6, f($rows));",
                "self::assertLessThanOrEqual(7, f($rows));",
                "assertLessThan",
                "assertLessThanOrEqual",
            ),
            (
                "$this->assertSame(0, f($rows));",
                "$this->assertNotSame(1, f($rows));",
                "assertSame",
                "assertNotSame",
            ),
            (
                "$this->assertEquals(0, f($rows));",
                "$this->assertNotEquals(1, f($rows));",
                "assertEquals",
                "assertNotEquals",
            ),
            (
                "$this->assertTrue(f($rows) > 6);",
                "$this->assertTrue(f($rows) >= 6);",
                ">",
                ">=",
            ),
            (
                "$this->assertTrue(f($rows) === 7);",
                "$this->assertTrue(f($rows) !== 8);",
                "===",
                "!==",
            ),
            // A failure condition that now fails less often.
            (
                "$this->assertFalse(f($rows) <= 6);",
                "$this->assertFalse(f($rows) < 6);",
                "<=",
                "<",
            ),
            ("assert(f($rows) > 6);", "assert(f($rows) >= 6);", ">", ">="),
            (
                "expect(f($rows))->toBeGreaterThan(6);",
                "expect(f($rows))->toBeGreaterThanOrEqual(6);",
                "toBeGreaterThan",
                "toBeGreaterThanOrEqual",
            ),
            (
                "expect(f($rows))->toBe(0);",
                "expect(f($rows))->not->toBe(1);",
                "toBe",
                "not->toBe",
            ),
            // `->not` reads the negation: `> 6` -> `>= 6`.
            (
                "expect(f($rows))->not->toBeLessThanOrEqual(6);",
                "expect(f($rows))->not->toBeLessThan(6);",
                "not->toBeLessThanOrEqual",
                "not->toBeLessThan",
            ),
        ] {
            assert_eq!(
                php_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept, moved to an integer that accepts the same
        // values, `!=` back to `==`, a failure condition tightened, a checked value that
        // changed, a threshold raised under the same operator.
        for (a, b) in [
            (
                "$this->assertGreaterThanOrEqual(6, f($rows));",
                "$this->assertGreaterThan(6, f($rows));",
            ),
            (
                "$this->assertGreaterThan(6, f($rows));",
                "self::assertGreaterThan(6, f($rows));",
            ),
            (
                "$this->assertGreaterThan(6, f($rows));",
                "$this->assertGreaterThanOrEqual(7, f($rows));",
            ),
            (
                "$this->assertNotSame(1, f($rows));",
                "$this->assertSame(2, f($rows));",
            ),
            (
                "$this->assertFalse(f($rows) < 6);",
                "$this->assertFalse(f($rows) <= 6);",
            ),
            (
                "$this->assertGreaterThan(6, f($rows));",
                "$this->assertGreaterThanOrEqual(6, g($rows));",
            ),
            (
                "$this->assertGreaterThan(6, f($rows));",
                "$this->assertGreaterThan(7, f($rows));",
            ),
            (
                "expect(f($rows))->not->toBeGreaterThan(6);",
                "expect(f($rows))->not->toBeGreaterThanOrEqual(6);",
            ),
        ] {
            assert_eq!(php_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn php_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "$this->assertTrue(f($rows));",
                "$this->assertFalse(f($rows));",
                "assertTrue",
                "assertFalse",
            ),
            (
                "self::assertSame(3, f($rows));",
                "self::assertNotSame(3, f($rows));",
                "assertSame",
                "assertNotSame",
            ),
            (
                "$this->assertNotNull(f($rows));",
                "$this->assertNull(f($rows));",
                "assertNotNull",
                "assertNull",
            ),
            (
                "$this->assertContains('a', f($rows));",
                "$this->assertNotContains('a', f($rows));",
                "assertContains",
                "assertNotContains",
            ),
            (
                "$this->assertEmpty(f($rows));",
                "$this->assertNotEmpty(f($rows));",
                "assertEmpty",
                "assertNotEmpty",
            ),
            (
                "$this->assertArrayHasKey('a', f($rows));",
                "$this->assertArrayNotHasKey('a', f($rows));",
                "assertArrayHasKey",
                "assertArrayNotHasKey",
            ),
            (
                "$this->assertStringStartsWith('a', f($rows));",
                "$this->assertStringStartsNotWith('a', f($rows));",
                "assertStringStartsWith",
                "assertStringStartsNotWith",
            ),
            (
                "expect(f($rows))->toBeTrue();",
                "expect(f($rows))->toBeFalse();",
                "toBeTrue",
                "toBeFalse",
            ),
            (
                "expect(f($rows))->toContain('a');",
                "expect(f($rows))->not->toContain('a');",
                "toContain",
                "not->toContain",
            ),
            (
                "expect(f($rows))->not->toBeNull();",
                "expect(f($rows))->toBeNull();",
                "not->toBeNull",
                "toBeNull",
            ),
        ] {
            assert_eq!(
                php_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two methods of different families
        // are not twins; a method swapped for its own spelling keeps the polarity.
        for (a, b) in [
            (
                "$this->assertTrue(f($rows));",
                "$this->assertFalse(g($rows));",
            ),
            (
                "$this->assertNull(f($rows));",
                "$this->assertFalse(f($rows));",
            ),
            (
                "$this->assertEmpty(f($rows));",
                "$this->assertNotCount(0, f($rows));",
            ),
            (
                "$this->assertTrue(f($rows));",
                "self::assertTrue(f($rows));",
            ),
            (
                "expect(f($rows))->toBeTrue();",
                "expect(f($rows))->toBeTruthy();",
            ),
        ] {
            assert_eq!(php_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn kt(check: &str) -> Vec<Comparison> {
        let src = if check.contains("should") {
            format!(
                "class RowsSpec : StringSpec({{\n    \"rows\" {{\n        val rows = listOf(1)\n        {check}\n    }}\n}})\n"
            )
        } else {
            format!(
                "class RowsTest {{\n    @Test\n    fun x() {{\n        val rows = listOf(1)\n        {check}\n    }}\n}}\n"
            )
        };
        comparisons(
            &crate::ast::kotlin::KotlinPack,
            "src/test/kotlin/RowsTest.kt",
            &src,
        )
    }

    fn kt_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&kt(a), &kt(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn kt_read(check: &str) -> Vec<(String, Option<String>, String)> {
        kt(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn kotlin_records_junit_assertj_kotest_comparisons_with_literals_masked() {
        assert_eq!(
            kt_read("assertEquals(3, f(rows), \"three rows\")"),
            vec![read(
                "assertEquals",
                Some("3"),
                "@(#, f(rows), \"three rows\")"
            )]
        );
        assert_eq!(
            kt_read("Assertions.assertNotEquals(f(rows), -1.5)"),
            vec![read(
                "assertNotEquals",
                Some("-1.5"),
                "Assertions.@(f(rows), #)"
            )]
        );
        assert_eq!(
            kt_read("assertTrue(f(rows) > 6 && g(rows) <= -1)"),
            vec![
                read("assertTrue", None, "@(f(rows) > 6 && g(rows) <= -1)"),
                read(">", Some("6"), "assertTrue(f(rows) @ # && g(rows) <= -1)"),
                read("<=", Some("-1"), "assertTrue(f(rows) > 6 && g(rows) @ #)"),
            ]
        );
        assert_eq!(
            kt_read("assertFalse(\"none\", (f(rows) == null) || ok)"),
            vec![
                read("assertFalse", None, "@(\"none\", (f(rows) == null) || ok)"),
                read(
                    "==",
                    Some("null"),
                    "assertFalse(\"none\", (f(rows) @ #) || ok)"
                ),
            ]
        );
        assert_eq!(
            kt_read("assert(f(rows) >= 6) { \"m\" }"),
            vec![read(">=", Some("6"), "assert(f(rows) @ #) { \"m\" }")]
        );
        assert_eq!(
            kt_read("assertThat(f(rows)).isGreaterThan(6).isNotNull()"),
            vec![
                read(
                    "isGreaterThan",
                    Some("6"),
                    "assertThat(f(rows)).@(#).isNotNull()"
                ),
                read(
                    "isNotNull",
                    None,
                    "assertThat(f(rows)).isGreaterThan(6).@()"
                ),
            ]
        );
        assert_eq!(
            kt_read("f(rows) shouldBe 6"),
            vec![read("shouldBe", Some("6"), "f(rows) @ #")]
        );
        assert_eq!(
            kt_read("f(rows).shouldBeLessThan(\"b\")"),
            vec![read("shouldBeLessThan", Some("\"b\""), "f(rows).@(#)")]
        );
        assert_eq!(
            kt_read("f(rows) shouldNotBe true"),
            vec![read("shouldNotBe", Some("true"), "f(rows) @ #")]
        );
        assert_eq!(
            kt_read("f(rows).shouldBeTrue()"),
            vec![read("shouldBeTrue", None, "f(rows).@()")]
        );
        // Not read: a call outside the lists, a call on another object, a matcher
        // outside the lists or through `should`, a template string as the literal, a
        // comparison inside a call's arguments or after `!`, or used as a value.
        for check in [
            "assertContentEquals(listOf(6), f(rows))",
            "other.assertTrue(f(rows) > 6)",
            "f(rows) shouldBeOneOf listOf(6)",
            "f(rows) should beGreaterThan(6)",
            "check(f(rows) > 6)",
            "val ok = assertTrue(f(rows) > 6)",
        ] {
            assert_eq!(kt_read(check), vec![], "{check}");
        }
        assert_eq!(
            kt_read("f(rows) shouldBe \"$x\""),
            vec![read("shouldBe", None, "f(rows) @ \"$x\"")]
        );
        assert_eq!(
            kt_read("assertTrue(g(f(rows) > 6))"),
            vec![read("assertTrue", None, "@(g(f(rows) > 6))")]
        );
        assert_eq!(
            kt_read("assertTrue(!(f(rows) > 6))"),
            vec![read("assertTrue", None, "@(!(f(rows) > 6))")]
        );
    }

    #[test]
    fn kotlin_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "f(rows) shouldBeGreaterThan 6",
                "f(rows) shouldBeGreaterThanOrEqual 6",
                "shouldBeGreaterThan",
                "shouldBeGreaterThanOrEqual",
            ),
            (
                "f(rows).shouldBeLessThan(6)",
                "f(rows).shouldBeLessThanOrEqual(7)",
                "shouldBeLessThan",
                "shouldBeLessThanOrEqual",
            ),
            (
                "f(rows) shouldBe 0",
                "f(rows) shouldNotBe 1",
                "shouldBe",
                "shouldNotBe",
            ),
            (
                "f(rows) shouldBe 0",
                "f(rows) shouldBeGreaterThanOrEqual 0",
                "shouldBe",
                "shouldBeGreaterThanOrEqual",
            ),
            (
                "assertEquals(0, f(rows))",
                "assertNotEquals(1, f(rows))",
                "assertEquals",
                "assertNotEquals",
            ),
            (
                "assertTrue(f(rows) > 6)",
                "assertTrue(f(rows) >= 6)",
                ">",
                ">=",
            ),
            (
                "assertTrue(f(rows) === 7)",
                "assertTrue(f(rows) !== 8)",
                "===",
                "!==",
            ),
            // A failure condition that now fails less often.
            (
                "assertFalse(f(rows) <= 6)",
                "assertFalse(f(rows) < 6)",
                "<=",
                "<",
            ),
            ("assert(f(rows) > 6)", "assert(f(rows) >= 6)", ">", ">="),
            (
                "assertThat(f(rows)).isGreaterThan(6)",
                "assertThat(f(rows)).isGreaterThanOrEqualTo(6)",
                "isGreaterThan",
                "isGreaterThanOrEqualTo",
            ),
        ] {
            assert_eq!(
                kt_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept (infix -> called), moved to an integer that
        // accepts the same values, `!=` back to `==`, a failure condition tightened, a
        // checked value that changed, a literal raised under the same operator.
        for (a, b) in [
            (
                "f(rows) shouldBeGreaterThanOrEqual 6",
                "f(rows) shouldBeGreaterThan 6",
            ),
            (
                "f(rows) shouldBeGreaterThan 6",
                "f(rows).shouldBeGreaterThan(6)",
            ),
            (
                "f(rows) shouldBeGreaterThan 6",
                "f(rows) shouldBeGreaterThanOrEqual 7",
            ),
            ("f(rows) shouldNotBe 1", "f(rows) shouldBe 2"),
            ("assertFalse(f(rows) < 6)", "assertFalse(f(rows) <= 6)"),
            (
                "f(rows) shouldBeGreaterThan 6",
                "g(rows) shouldBeGreaterThanOrEqual 6",
            ),
            (
                "f(rows) shouldBeGreaterThan 6",
                "f(rows) shouldBeGreaterThan 7",
            ),
        ] {
            assert_eq!(kt_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn kotlin_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "assertTrue(f(rows))",
                "assertFalse(f(rows))",
                "assertTrue",
                "assertFalse",
            ),
            (
                "assertEquals(3, f(rows))",
                "assertNotEquals(3, f(rows))",
                "assertEquals",
                "assertNotEquals",
            ),
            (
                "assertNotNull(f(rows))",
                "assertNull(f(rows))",
                "assertNotNull",
                "assertNull",
            ),
            (
                "f(rows) shouldBe 3",
                "f(rows) shouldNotBe 3",
                "shouldBe",
                "shouldNotBe",
            ),
            (
                "f(rows).shouldBeTrue()",
                "f(rows).shouldBeFalse()",
                "shouldBeTrue",
                "shouldBeFalse",
            ),
            (
                "f(rows) shouldContain 'a'",
                "f(rows) shouldNotContain 'a'",
                "shouldContain",
                "shouldNotContain",
            ),
            (
                "f(rows).shouldNotBeNull()",
                "f(rows).shouldBeNull()",
                "shouldNotBeNull",
                "shouldBeNull",
            ),
            (
                "assertThat(f(rows)).contains(\"a\")",
                "assertThat(f(rows)).doesNotContain(\"a\")",
                "contains",
                "doesNotContain",
            ),
        ] {
            assert_eq!(
                kt_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two matchers of different families
        // are not twins.
        for (a, b) in [
            ("assertTrue(f(rows))", "assertFalse(g(rows))"),
            ("f(rows).shouldBeTrue()", "f(rows).shouldBeNull()"),
            ("f(rows).shouldBeEmpty()", "f(rows).shouldNotBeBlank()"),
            ("assertNull(f(rows))", "assertFalse(f(rows))"),
        ] {
            assert_eq!(kt_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn rb(check: &str) -> Vec<Comparison> {
        let src = if check.contains("expect") {
            format!("RSpec.describe Rows do\n  it \"counts\" do\n    rows = [1]\n    {check}\n  end\nend\n")
        } else {
            format!(
                "class RowsTest < Minitest::Test\n  def test_counts\n    rows = [1]\n    {check}\n  end\nend\n"
            )
        };
        let path = if check.contains("expect") {
            "spec/rows_spec.rb"
        } else {
            "test/rows_test.rb"
        };
        comparisons(&crate::ast::ruby::RubyPack, path, &src)
    }

    fn rb_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&rb(a), &rb(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn rb_read(check: &str) -> Vec<(String, Option<String>, String)> {
        rb(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn ruby_records_minitest_and_rspec_comparisons_with_literals_masked() {
        assert_eq!(
            rb_read("assert_equal 3, f(rows), \"three rows\""),
            vec![read(
                "assert_equal",
                Some("3"),
                "@ #, f(rows), \"three rows\""
            )]
        );
        assert_eq!(
            rb_read("refute_equal(f(rows), -1.5)"),
            vec![read("refute_equal", Some("-1.5"), "@(f(rows), #)")]
        );
        assert_eq!(
            rb_read("assert f(rows) > 6 && g(rows) <= -1"),
            vec![
                read("assert", None, "@ f(rows) > 6 && g(rows) <= -1"),
                read(">", Some("6"), "assert f(rows) @ # && g(rows) <= -1"),
                read("<=", Some("-1"), "assert f(rows) > 6 && g(rows) @ #"),
            ]
        );
        assert_eq!(
            rb_read("refute((f(rows) == nil) or ok)"),
            vec![
                read("refute", None, "@((f(rows) == nil) or ok)"),
                read("==", Some("nil"), "refute((f(rows) @ #) or ok)"),
            ]
        );
        assert_eq!(
            rb_read("assert_operator f(rows), :>, 6"),
            vec![read(":>", Some("6"), "assert_operator f(rows), @, #")]
        );
        assert_eq!(
            rb_read("assert_nil f(rows)"),
            vec![read("assert_nil", None, "@ f(rows)")]
        );
        assert_eq!(
            rb_read("expect(f(rows)).to eq(6)"),
            vec![read("to eq", Some("6"), "expect(f(rows)).@ @(#)")]
        );
        assert_eq!(
            rb_read("expect(f(rows)).not_to be >= :a"),
            vec![read("not_to be >=", Some(":a"), "expect(f(rows)).@ @ #")]
        );
        assert_eq!(
            rb_read("is_expected.to eq(:a) # expect"),
            vec![read("to eq", Some(":a"), "is_expected.@ @(#)")]
        );
        assert_eq!(
            rb_read("expect(f(rows)).to be_truthy"),
            vec![read("to be_truthy", None, "expect(f(rows)).@ @")]
        );
        assert_eq!(
            rb_read("expect { f(rows) }.to raise_error(ArgumentError)"),
            vec![read(
                "to raise_error",
                None,
                "expect { f(rows) }.@ @(ArgumentError)"
            )]
        );
        // Not read: a method outside the lists, a call on a receiver, a matcher outside
        // the lists or chained, an operator symbol outside the comparisons, a comparison
        // inside a call's arguments, an expectation not rooted at `expect`.
        for check in [
            "assert_equal_ish 6, f(rows)",
            "other.assert f(rows) > 6",
            "expect(f(rows)).to be_within(1).of(6)",
            "expect(f(rows)).to have_attributes(size: 6)",
            "assert_operator f(rows), :eql?, 6",
            "want(f(rows)).to eq(6)",
        ] {
            assert_eq!(rb_read(check), vec![], "{check}");
        }
        assert_eq!(
            rb_read("assert g(f(rows) > 6)"),
            vec![read("assert", None, "@ g(f(rows) > 6)")]
        );
        assert_eq!(
            rb_read("expect(f(rows)).to eq(\"a#{x}\")"),
            vec![read("to eq", None, "expect(f(rows)).@ @(\"a#{x}\")")]
        );
    }

    #[test]
    fn ruby_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "expect(f(rows)).to be > 6",
                "expect(f(rows)).to be >= 6",
                "to be >",
                "to be >=",
            ),
            (
                "expect(f(rows)).to be < 6",
                "expect(f(rows)).to be <= 7",
                "to be <",
                "to be <=",
            ),
            (
                "expect(f(rows)).to eq(0)",
                "expect(f(rows)).not_to eq(1)",
                "to eq",
                "not_to eq",
            ),
            (
                "expect(f(rows)).to eq 0",
                "expect(f(rows)).to be >= 0",
                "to eq",
                "to be >=",
            ),
            // `not_to` reads the negation: `> 6` -> `>= 6`.
            (
                "expect(f(rows)).not_to be <= 6",
                "expect(f(rows)).to_not be < 6",
                "not_to be <=",
                "to_not be <",
            ),
            (
                "assert_equal 0, f(rows)",
                "refute_equal 1, f(rows)",
                "assert_equal",
                "refute_equal",
            ),
            ("assert f(rows) > 6", "assert f(rows) >= 6", ">", ">="),
            // A failure condition that now fails less often.
            ("refute f(rows) <= 6", "refute f(rows) < 6", "<=", "<"),
            (
                "assert_operator f(rows), :>, 6",
                "assert_operator f(rows), :>=, 6",
                ":>",
                ":>=",
            ),
            (
                "refute_operator f(rows), :<=, 6",
                "refute_operator f(rows), :<, 6",
                ":<=",
                ":<",
            ),
        ] {
            assert_eq!(
                rb_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept, moved to an integer that accepts the same
        // values, `!=` back to `==`, a failure condition tightened, a checked value that
        // changed, a literal raised under the same operator.
        for (a, b) in [
            ("expect(f(rows)).to be >= 6", "expect(f(rows)).to be > 6"),
            (
                "expect(f(rows)).not_to be <= 6",
                "expect(f(rows)).to_not be <= 6",
            ),
            ("expect(f(rows)).to be > 6", "expect(f(rows)).to be >= 7"),
            ("expect(f(rows)).not_to eq(1)", "expect(f(rows)).to eq(2)"),
            ("refute f(rows) < 6", "refute f(rows) <= 6"),
            ("expect(f(rows)).to be > 6", "expect(g(rows)).to be >= 6"),
            (
                "assert_operator f(rows), :>, 6",
                "assert_operator f(rows), :>, 7",
            ),
        ] {
            assert_eq!(rb_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn ruby_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            ("assert f(rows)", "refute f(rows)", "assert", "refute"),
            (
                "assert_equal 3, f(rows)",
                "refute_equal 3, f(rows)",
                "assert_equal",
                "refute_equal",
            ),
            (
                "refute_nil f(rows)",
                "assert_nil f(rows)",
                "refute_nil",
                "assert_nil",
            ),
            (
                "assert_includes f(rows), :a",
                "refute_includes f(rows), :a",
                "assert_includes",
                "refute_includes",
            ),
            (
                "expect(f(rows)).to be_truthy",
                "expect(f(rows)).to be_falsey",
                "to be_truthy",
                "to be_falsey",
            ),
            (
                "expect(f(rows)).to include(\"a\")",
                "expect(f(rows)).not_to include(\"a\")",
                "to include",
                "not_to include",
            ),
            (
                "expect(f(rows)).not_to be_nil",
                "expect(f(rows)).to be_nil",
                "not_to be_nil",
                "to be_nil",
            ),
            (
                "expect { f(rows) }.to raise_error(ArgumentError)",
                "expect { f(rows) }.not_to raise_error(ArgumentError)",
                "to raise_error",
                "not_to raise_error",
            ),
        ] {
            assert_eq!(
                rb_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two matchers of different families
        // are not twins; `not_to` -> `to_not` keeps the polarity.
        for (a, b) in [
            ("assert f(rows)", "refute g(rows)"),
            ("assert_nil f(rows)", "refute_empty f(rows)"),
            ("expect(f(rows)).to be_truthy", "expect(f(rows)).to be_nil"),
            (
                "expect(f(rows)).not_to be_nil",
                "expect(f(rows)).to_not be_nil",
            ),
        ] {
            assert_eq!(rb_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn sw(check: &str) -> Vec<Comparison> {
        let src = if check.contains('#') {
            format!("import Testing\n\n@Test func counts() throws {{\n    let rows = [1]\n    {check}\n}}\n")
        } else {
            format!(
                "import XCTest\n\nfinal class RowsTests: XCTestCase {{\n    func testCounts() throws {{\n        let rows = [1]\n        {check}\n    }}\n}}\n"
            )
        };
        comparisons(&crate::ast::swift::SwiftPack, "Tests/RowsTests.swift", &src)
    }

    fn sw_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&sw(a), &sw(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn sw_read(check: &str) -> Vec<(String, Option<String>, String)> {
        sw(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn swift_records_xctest_and_swift_testing_comparisons_with_literals_masked() {
        assert_eq!(
            sw_read("XCTAssertGreaterThan(f(rows), 6, \"seven rows\")"),
            vec![read(
                "XCTAssertGreaterThan",
                Some("6"),
                "@(f(rows), #, \"seven rows\")"
            )]
        );
        assert_eq!(
            sw_read("XCTAssertEqual(3, f(rows))"),
            vec![read("XCTAssertEqual", Some("3"), "@(#, f(rows))")]
        );
        assert_eq!(
            sw_read("XCTAssertNotEqual(f(rows), -1.5)"),
            vec![read("XCTAssertNotEqual", Some("-1.5"), "@(f(rows), #)")]
        );
        assert_eq!(
            sw_read("XCTAssertTrue(f(rows) > 6)"),
            vec![
                read("XCTAssertTrue", None, "@(f(rows) > 6)"),
                read(">", Some("6"), "XCTAssertTrue(f(rows) @ #)"),
            ]
        );
        assert_eq!(
            sw_read("XCTAssertFalse((f(rows) == nil) || ok)"),
            vec![
                read("XCTAssertFalse", None, "@((f(rows) == nil) || ok)"),
                read("==", Some("nil"), "XCTAssertFalse((f(rows) @ #) || ok)"),
            ]
        );
        assert_eq!(
            sw_read("try XCTAssert(f(rows) >= \"a\")"),
            vec![
                read("XCTAssert", None, "try @(f(rows) >= \"a\")"),
                read(">=", Some("\"a\""), "try XCTAssert(f(rows) @ #)"),
            ]
        );
        assert_eq!(
            sw_read("#expect(f(rows) <= 6)"),
            vec![read("<=", Some("6"), "#expect(f(rows) @ #)")]
        );
        assert_eq!(
            sw_read("try #require(f(rows) != nil)"),
            vec![read("!=", Some("nil"), "try #require(f(rows) @ #)")]
        );
        assert_eq!(
            sw_read("XCTAssertNil(f(rows))"),
            vec![read("XCTAssertNil", None, "@(f(rows))")]
        );
        // Not read: a call outside the lists, an ordering whose second argument is not a
        // literal, a comparison the grammar joins with `&&`, inside a call's arguments or
        // after `!`, a macro other than `#expect` / `#require`.
        for check in [
            "XCTAssertEqualish(f(rows), 6)",
            "XCTUnwrap(f(rows))",
            "#warning(f(rows) > 6)",
        ] {
            assert_eq!(sw_read(check), vec![], "{check}");
        }
        assert_eq!(
            sw_read("XCTAssertLessThan(6, f(rows))"),
            vec![read("XCTAssertLessThan", None, "@(6, f(rows))")]
        );
        for check in [
            "XCTAssertTrue(f(rows) > 6 && g(rows) <= -1)",
            "XCTAssertTrue(g(f(rows) > 6))",
            "XCTAssertTrue(!(f(rows) > 6))",
        ] {
            assert_eq!(
                sw_read(check).into_iter().map(|r| r.0).collect::<Vec<_>>(),
                vec!["XCTAssertTrue".to_string()],
                "{check}"
            );
        }
        assert_eq!(
            sw_read("XCTAssertTrue((f(rows) > 6) && (g(rows) < 2))"),
            vec![
                read("XCTAssertTrue", None, "@((f(rows) > 6) && (g(rows) < 2))"),
                read(
                    ">",
                    Some("6"),
                    "XCTAssertTrue((f(rows) @ #) && (g(rows) < 2))"
                ),
                read(
                    "<",
                    Some("2"),
                    "XCTAssertTrue((f(rows) > 6) && (g(rows) @ #))"
                ),
            ]
        );
        assert_eq!(
            sw_read("XCTAssertEqual(f(rows), \"a\\(x)\")"),
            vec![read("XCTAssertEqual", None, "@(f(rows), \"a\\(x)\")")]
        );
    }

    #[test]
    fn swift_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "XCTAssertGreaterThan(f(rows), 6)",
                "XCTAssertGreaterThanOrEqual(f(rows), 6)",
                "XCTAssertGreaterThan",
                "XCTAssertGreaterThanOrEqual",
            ),
            (
                "XCTAssertLessThan(f(rows), 6)",
                "XCTAssertLessThanOrEqual(f(rows), 7)",
                "XCTAssertLessThan",
                "XCTAssertLessThanOrEqual",
            ),
            (
                "XCTAssertEqual(f(rows), 0)",
                "XCTAssertNotEqual(f(rows), 1)",
                "XCTAssertEqual",
                "XCTAssertNotEqual",
            ),
            (
                "XCTAssertTrue(f(rows) > 6)",
                "XCTAssertTrue(f(rows) >= 6)",
                ">",
                ">=",
            ),
            (
                "XCTAssert(f(rows) == 7)",
                "XCTAssert(f(rows) != 8)",
                "==",
                "!=",
            ),
            // A failure condition that now fails less often.
            (
                "XCTAssertFalse(f(rows) <= 6)",
                "XCTAssertFalse(f(rows) < 6)",
                "<=",
                "<",
            ),
            ("#expect(f(rows) > 6)", "#expect(f(rows) >= 6)", ">", ">="),
            (
                "try #require(f(rows) < 6)",
                "try #require(f(rows) <= 6)",
                "<",
                "<=",
            ),
        ] {
            assert_eq!(
                sw_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept, moved to an integer that accepts the same
        // values, `!=` back to `==`, a failure condition tightened, a checked value that
        // changed, a literal raised under the same operator.
        for (a, b) in [
            (
                "XCTAssertGreaterThanOrEqual(f(rows), 6)",
                "XCTAssertGreaterThan(f(rows), 6)",
            ),
            ("XCTAssertTrue(f(rows) > 6)", "XCTAssert(f(rows) > 6)"),
            (
                "XCTAssertGreaterThan(f(rows), 6)",
                "XCTAssertGreaterThanOrEqual(f(rows), 7)",
            ),
            (
                "XCTAssertNotEqual(f(rows), 1)",
                "XCTAssertEqual(f(rows), 2)",
            ),
            (
                "XCTAssertFalse(f(rows) < 6)",
                "XCTAssertFalse(f(rows) <= 6)",
            ),
            (
                "XCTAssertGreaterThan(f(rows), 6)",
                "XCTAssertGreaterThanOrEqual(g(rows), 6)",
            ),
            ("#expect(f(rows) > 6)", "#expect(f(rows) > 7)"),
        ] {
            assert_eq!(sw_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn swift_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "XCTAssertTrue(f(rows))",
                "XCTAssertFalse(f(rows))",
                "XCTAssertTrue",
                "XCTAssertFalse",
            ),
            (
                "XCTAssert(f(rows))",
                "XCTAssertFalse(f(rows))",
                "XCTAssert",
                "XCTAssertFalse",
            ),
            (
                "XCTAssertEqual(f(rows), 3)",
                "XCTAssertNotEqual(f(rows), 3)",
                "XCTAssertEqual",
                "XCTAssertNotEqual",
            ),
            (
                "XCTAssertNotNil(f(rows))",
                "XCTAssertNil(f(rows))",
                "XCTAssertNotNil",
                "XCTAssertNil",
            ),
            (
                "XCTAssertIdentical(f(rows), x)",
                "XCTAssertNotIdentical(f(rows), x)",
                "XCTAssertIdentical",
                "XCTAssertNotIdentical",
            ),
            (
                "XCTAssertThrowsError(try f(rows))",
                "XCTAssertNoThrow(try f(rows))",
                "XCTAssertThrowsError",
                "XCTAssertNoThrow",
            ),
        ] {
            assert_eq!(
                sw_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two assertions of different
        // families are not twins; `XCTAssert` -> `XCTAssertTrue` keeps the polarity.
        for (a, b) in [
            ("XCTAssertTrue(f(rows))", "XCTAssertFalse(g(rows))"),
            ("XCTAssertNil(f(rows))", "XCTAssertFalse(f(rows))"),
            ("XCTAssert(f(rows))", "XCTAssertTrue(f(rows))"),
        ] {
            assert_eq!(sw_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn sc(check: &str) -> Vec<Comparison> {
        let src = if check.contains("should") {
            format!(
                "class RowsSpec extends AnyFlatSpec with Matchers {{\n  \"rows\" should \"count\" in {{\n    val rows = List(1)\n    {check}\n  }}\n}}\n"
            )
        } else {
            format!(
                "class RowsSuite extends munit.FunSuite {{\n  test(\"counts\") {{\n    val rows = List(1)\n    {check}\n  }}\n}}\n"
            )
        };
        comparisons(
            &crate::ast::scala::ScalaPack,
            "src/test/scala/RowsSpec.scala",
            &src,
        )
    }

    fn sc_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&sc(a), &sc(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn sc_read(check: &str) -> Vec<(String, Option<String>, String)> {
        sc(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn scala_records_scalatest_munit_and_assert_comparisons_with_literals_masked() {
        assert_eq!(
            sc_read("f(rows) shouldBe 6"),
            vec![read("shouldBe", Some("6"), "f(rows) @ #")]
        );
        assert_eq!(
            sc_read("f(rows) should be > -1"),
            vec![read("should be >", Some("-1"), "f(rows) @ #")]
        );
        assert_eq!(
            sc_read("f(rows) should not equal (\"a\")"),
            vec![read("should not equal", Some("\"a\""), "f(rows) @ (#)")]
        );
        assert_eq!(
            sc_read("f(rows) should === (6)"),
            vec![read("should ===", Some("6"), "f(rows) @ (#)")]
        );
        assert_eq!(
            sc_read("f(rows) should contain (\"a\")"),
            vec![read("should contain", None, "f(rows) @ (\"a\")")]
        );
        assert_eq!(
            sc_read("assert(f(rows) > 6 && g(rows) <= 1.5)"),
            vec![
                read(">", Some("6"), "assert(f(rows) @ # && g(rows) <= 1.5)"),
                read("<=", Some("1.5"), "assert(f(rows) > 6 && g(rows) @ #)"),
            ]
        );
        assert_eq!(
            sc_read("assert((f(rows) == null) || ok)"),
            vec![read("==", Some("null"), "assert((f(rows) @ #) || ok)")]
        );
        assert_eq!(
            sc_read("assertEquals(f(rows), 3, \"three rows\")"),
            vec![read(
                "assertEquals",
                Some("3"),
                "@(f(rows), #, \"three rows\")"
            )]
        );
        assert_eq!(
            sc_read("assertTrue(f(rows) >= 6)"),
            vec![
                read("assertTrue", None, "@(f(rows) >= 6)"),
                read(">=", Some("6"), "assertTrue(f(rows) @ #)"),
            ]
        );
        // Not read: a matcher outside the lists, a call outside the lists, a comparison
        // inside a call's arguments or after `!`, an interpolated string as the literal.
        for check in [
            "f(rows) should have length 6",
            "f(rows) should not be > 6",
            "f(rows) shouldBe empty should be",
            "assume(f(rows) > 6)",
            "assert(g(f(rows) > 6))",
            "assert(!(f(rows) > 6))",
        ] {
            assert_eq!(sc_read(check), vec![], "{check}");
        }
        assert_eq!(
            sc_read("f(rows) shouldBe s\"a$x\""),
            vec![read("shouldBe", None, "f(rows) @ s\"a$x\"")]
        );
    }

    #[test]
    fn scala_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "f(rows) should be > 6",
                "f(rows) should be >= 6",
                "should be >",
                "should be >=",
            ),
            (
                "f(rows) should be < 6",
                "f(rows) should be <= 7",
                "should be <",
                "should be <=",
            ),
            (
                "f(rows) shouldBe 0",
                "f(rows) should not be 1",
                "shouldBe",
                "should not be",
            ),
            (
                "f(rows) shouldBe 0",
                "f(rows) should be >= 0",
                "shouldBe",
                "should be >=",
            ),
            (
                "f(rows) should equal (0)",
                "f(rows) should not equal (1)",
                "should equal",
                "should not equal",
            ),
            (
                "assertEquals(f(rows), 0)",
                "assertNotEquals(f(rows), 1)",
                "assertEquals",
                "assertNotEquals",
            ),
            ("assert(f(rows) > 6)", "assert(f(rows) >= 6)", ">", ">="),
            // A failure condition that now fails less often.
            (
                "assertFalse(f(rows) <= 6)",
                "assertFalse(f(rows) < 6)",
                "<=",
                "<",
            ),
        ] {
            assert_eq!(
                sc_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept, moved to an integer that accepts the same
        // values, `!=` back to `==`, a failure condition tightened, a checked value that
        // changed, a literal raised under the same operator.
        for (a, b) in [
            ("f(rows) should be >= 6", "f(rows) should be > 6"),
            ("f(rows) shouldBe 6", "f(rows) should be 6"),
            ("f(rows) should be > 6", "f(rows) should be >= 7"),
            ("f(rows) should not be 1", "f(rows) shouldBe 2"),
            ("assertFalse(f(rows) < 6)", "assertFalse(f(rows) <= 6)"),
            ("f(rows) should be > 6", "g(rows) should be >= 6"),
            ("f(rows) should be > 6", "f(rows) should be > 7"),
        ] {
            assert_eq!(sc_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn scala_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "f(rows) shouldBe 3",
                "f(rows) should not be 3",
                "shouldBe",
                "should not be",
            ),
            (
                "f(rows) should contain (\"a\")",
                "f(rows) should not contain (\"a\")",
                "should contain",
                "should not contain",
            ),
            (
                "f(rows) should startWith (\"a\")",
                "f(rows) shouldNot startWith (\"a\")",
                "should startWith",
                "shouldNot startWith",
            ),
            (
                "assertEquals(f(rows), 3)",
                "assertNotEquals(f(rows), 3)",
                "assertEquals",
                "assertNotEquals",
            ),
            (
                "assertTrue(f(rows))",
                "assertFalse(f(rows))",
                "assertTrue",
                "assertFalse",
            ),
        ] {
            assert_eq!(
                sc_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two matchers of different families
        // are not twins; `should not` -> `shouldNot` keeps the polarity.
        for (a, b) in [
            ("assertTrue(f(rows))", "assertFalse(g(rows))"),
            (
                "f(rows) should contain (\"a\")",
                "f(rows) should not startWith (\"a\")",
            ),
            (
                "f(rows) should not contain (\"a\")",
                "f(rows) shouldNot contain (\"a\")",
            ),
        ] {
            assert_eq!(sc_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    fn oc(check: &str) -> Vec<Comparison> {
        let src = format!(
            "#import <XCTest/XCTest.h>\n\n@interface RowsTests : XCTestCase\n@end\n\n@implementation RowsTests\n\n- (void)testCounts {{\n    NSArray *rows = @[@1];\n    {check}\n}}\n\n@end\n"
        );
        comparisons(&crate::ast::objc::ObjcPack, "Tests/RowsTests.m", &src)
    }

    fn oc_weakened(a: &str, b: &str) -> Vec<(String, String, bool)> {
        weakened(&oc(a), &oc(b))
            .into_iter()
            .map(|w| (w.from, w.to, w.inverted))
            .collect()
    }

    fn oc_read(check: &str) -> Vec<(String, Option<String>, String)> {
        oc(check)
            .into_iter()
            .map(|c| (c.operator, c.literal, c.skeleton))
            .collect()
    }

    #[test]
    fn objc_records_xctest_and_nsassert_comparisons_with_literals_masked() {
        assert_eq!(
            oc_read("XCTAssertGreaterThan(F(rows), 6, @\"seven rows\");"),
            vec![read(
                "XCTAssertGreaterThan",
                Some("6"),
                "@(F(rows), #, @\"seven rows\");"
            )]
        );
        assert_eq!(
            oc_read("XCTAssertEqual(3, F(rows));"),
            vec![read("XCTAssertEqual", Some("3"), "@(#, F(rows));")]
        );
        assert_eq!(
            oc_read("XCTAssertEqualObjects(F(rows), @\"a\");"),
            vec![read(
                "XCTAssertEqualObjects",
                Some("@\"a\""),
                "@(F(rows), #);"
            )]
        );
        assert_eq!(
            oc_read("XCTAssertTrue(F(rows) > 6 && G(rows) <= -1);"),
            vec![
                read("XCTAssertTrue", None, "@(F(rows) > 6 && G(rows) <= -1);"),
                read(
                    ">",
                    Some("6"),
                    "XCTAssertTrue(F(rows) @ # && G(rows) <= -1);"
                ),
                read(
                    "<=",
                    Some("-1"),
                    "XCTAssertTrue(F(rows) > 6 && G(rows) @ #);"
                ),
            ]
        );
        assert_eq!(
            oc_read("XCTAssertFalse(F(rows) == YES);"),
            vec![
                read("XCTAssertFalse", None, "@(F(rows) == YES);"),
                read("==", Some("YES"), "XCTAssertFalse(F(rows) @ #);"),
            ]
        );
        assert_eq!(
            oc_read("NSAssert(F(rows) >= 6, @\"m\");"),
            vec![read(">=", Some("6"), "NSAssert(F(rows) @ #, @\"m\");")]
        );
        assert_eq!(
            oc_read("assert(F(rows) != nil);"),
            vec![read("!=", Some("nil"), "assert(F(rows) @ #);")]
        );
        assert_eq!(
            oc_read("XCTAssertNil(F(rows));"),
            vec![read("XCTAssertNil", None, "@(F(rows));")]
        );
        // Not read: a macro outside the lists, an ordering whose second argument is not a
        // literal, a comparison inside a call's arguments or after `!`, or in a call
        // that takes no condition.
        for check in [
            "XCTAssertEqualish(F(rows), 6);",
            "XCTFail(@\"x\");",
            "check(F(rows) > 6);",
        ] {
            assert_eq!(oc_read(check), vec![], "{check}");
        }
        assert_eq!(
            oc_read("XCTAssertLessThan(6, F(rows));"),
            vec![read("XCTAssertLessThan", None, "@(6, F(rows));")]
        );
        assert_eq!(
            oc_read("XCTAssertTrue(G(F(rows) > 6));"),
            vec![read("XCTAssertTrue", None, "@(G(F(rows) > 6));")]
        );
        assert_eq!(
            oc_read("XCTAssertTrue(!(F(rows) > 6));"),
            vec![read("XCTAssertTrue", None, "@(!(F(rows) > 6));")]
        );
    }

    #[test]
    fn objc_comparisons_report_an_operator_that_accepts_more() {
        for (a, b, from, to) in [
            (
                "XCTAssertGreaterThan(F(rows), 6);",
                "XCTAssertGreaterThanOrEqual(F(rows), 6);",
                "XCTAssertGreaterThan",
                "XCTAssertGreaterThanOrEqual",
            ),
            (
                "XCTAssertLessThan(F(rows), 6);",
                "XCTAssertLessThanOrEqual(F(rows), 7);",
                "XCTAssertLessThan",
                "XCTAssertLessThanOrEqual",
            ),
            (
                "XCTAssertEqual(F(rows), 0);",
                "XCTAssertNotEqual(F(rows), 1);",
                "XCTAssertEqual",
                "XCTAssertNotEqual",
            ),
            (
                "XCTAssertEqualObjects(F(rows), @\"a\");",
                "XCTAssertNotEqualObjects(F(rows), @\"b\");",
                "XCTAssertEqualObjects",
                "XCTAssertNotEqualObjects",
            ),
            (
                "XCTAssertTrue(F(rows) > 6);",
                "XCTAssertTrue(F(rows) >= 6);",
                ">",
                ">=",
            ),
            // A failure condition that now fails less often.
            (
                "XCTAssertFalse(F(rows) <= 6);",
                "XCTAssertFalse(F(rows) < 6);",
                "<=",
                "<",
            ),
            (
                "NSAssert(F(rows) > 6, @\"m\");",
                "NSAssert(F(rows) >= 6, @\"m\");",
                ">",
                ">=",
            ),
            ("assert(F(rows) < 6);", "assert(F(rows) <= 6);", "<", "<="),
        ] {
            assert_eq!(
                oc_weakened(a, b),
                vec![(from.to_string(), to.to_string(), false)],
                "{a} -> {b}"
            );
        }
        // Negative controls: tightened, kept, moved to an integer that accepts the same
        // values, `!=` back to `==`, a failure condition tightened, a checked value that
        // changed, a literal raised under the same operator.
        for (a, b) in [
            (
                "XCTAssertGreaterThanOrEqual(F(rows), 6);",
                "XCTAssertGreaterThan(F(rows), 6);",
            ),
            ("XCTAssertTrue(F(rows) > 6);", "XCTAssert(F(rows) > 6);"),
            (
                "XCTAssertGreaterThan(F(rows), 6);",
                "XCTAssertGreaterThanOrEqual(F(rows), 7);",
            ),
            (
                "XCTAssertNotEqual(F(rows), 1);",
                "XCTAssertEqual(F(rows), 2);",
            ),
            (
                "XCTAssertFalse(F(rows) < 6);",
                "XCTAssertFalse(F(rows) <= 6);",
            ),
            (
                "XCTAssertGreaterThan(F(rows), 6);",
                "XCTAssertGreaterThanOrEqual(G(rows), 6);",
            ),
            (
                "XCTAssertGreaterThan(F(rows), 6);",
                "XCTAssertGreaterThan(F(rows), 7);",
            ),
        ] {
            assert_eq!(oc_weakened(a, b), vec![], "{a} -> {b}");
        }
    }

    #[test]
    fn objc_polarity_inversions_are_reported() {
        for (a, b, from, to) in [
            (
                "XCTAssertTrue(F(rows));",
                "XCTAssertFalse(F(rows));",
                "XCTAssertTrue",
                "XCTAssertFalse",
            ),
            (
                "XCTAssertEqualObjects(F(rows), @\"a\");",
                "XCTAssertNotEqualObjects(F(rows), @\"a\");",
                "XCTAssertEqualObjects",
                "XCTAssertNotEqualObjects",
            ),
            (
                "XCTAssertNotNil(F(rows));",
                "XCTAssertNil(F(rows));",
                "XCTAssertNotNil",
                "XCTAssertNil",
            ),
            (
                "XCTAssertThrows(F(rows));",
                "XCTAssertNoThrow(F(rows));",
                "XCTAssertThrows",
                "XCTAssertNoThrow",
            ),
            (
                "XCTAssertEqualWithAccuracy(F(rows), 1.0, 0.1);",
                "XCTAssertNotEqualWithAccuracy(F(rows), 1.0, 0.1);",
                "XCTAssertEqualWithAccuracy",
                "XCTAssertNotEqualWithAccuracy",
            ),
        ] {
            assert_eq!(
                oc_weakened(a, b),
                vec![(from.to_string(), to.to_string(), true)],
                "{a} -> {b}"
            );
        }
        // A checked value that changed is not paired; two macros of different families
        // are not twins; `XCTAssert` -> `XCTAssertTrue` keeps the polarity.
        for (a, b) in [
            ("XCTAssertTrue(F(rows));", "XCTAssertFalse(G(rows));"),
            ("XCTAssertNil(F(rows));", "XCTAssertFalse(F(rows));"),
            (
                "XCTAssertEqual(F(rows), 3);",
                "XCTAssertNotEqualObjects(F(rows), 3);",
            ),
            ("XCTAssert(F(rows));", "XCTAssertTrue(F(rows));"),
        ] {
            assert_eq!(oc_weakened(a, b), vec![], "{a} -> {b}");
        }
    }
}
