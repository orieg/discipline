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
//! Rust `assert!` / `debug_assert!` / `prop_assert!` conditions, and JS/TS `expect(..)`
//! matchers and `assert(..)` calls (#1070).
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
//! `assert(x)` / `assert(!x)` and the `assert.*` twins the same way.

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
        _ => return js_matcher_polarity(operator),
    })
}

/// JS/TS: the polarity of an `expect(..)` matcher with a negated form, `.not` or its
/// twin (`toBeTruthy` / `toBeFalsy`, `toBeDefined` / `toBeUndefined`).
fn js_matcher_polarity(operator: &str) -> Option<(&'static str, bool)> {
    let (matcher, not) = match operator.strip_prefix("not.") {
        Some(m) => (m, true),
        None => (operator, false),
    };
    let (family, negated) = match matcher {
        "toBeFalsy" => ("toBeTruthy", true),
        "toBeUndefined" => ("toBeDefined", true),
        _ => (*JS_POLAR_MATCHERS.iter().find(|m| **m == matcher)?, false),
    };
    Some((family, negated != not))
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

/// The operator a comparison must satisfy for the test to pass: the operator itself in
/// an assertion, its negation in a failure condition.
fn passing(operator: &str, fails_when_true: bool) -> Option<&'static str> {
    // A JS/TS matcher after `.not` checks the negation.
    let (operator, not) = match operator.strip_prefix("not.") {
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
        _ => return None,
    };
    Some(if fails_when_true != not { negated } else { op })
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
        if let (Some((bf, bn)), Some((hf, hn))) = (polarity(&b.operator), polarity(&h.operator)) {
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
}
