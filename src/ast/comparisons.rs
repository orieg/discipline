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
//! Read for Python `assert` statements and Go `if` conditions whose block fails the test.

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
}

impl Record for Comparison {
    fn line(&self) -> usize {
        self.line
    }
    fn written(&self) -> (&str, &str) {
        (&self.skeleton, &self.operator)
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

/// Python: each comparison of an `assert` statement, a chained one (`0 < x <= 5`)
/// read operator by operator.
pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let records = &mut Records::new(tests, src);
    walk(root, &mut |node| {
        if node.kind() != "assert_statement" {
            return true;
        }
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

/// The operator a comparison must satisfy for the test to pass: the operator itself in
/// an assertion, its negation in a failure condition.
fn passing(operator: &str, fails_when_true: bool) -> Option<&'static str> {
    let (op, negated) = match operator {
        "==" => ("==", "!="),
        "!=" => ("!=", "=="),
        "<" => ("<", ">="),
        "<=" => ("<=", ">"),
        ">" => (">", "<="),
        ">=" => (">=", "<"),
        "in" => ("in", "not in"),
        "not in" => ("not in", "in"),
        _ => return None,
    };
    Some(if fails_when_true { negated } else { op })
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
                ("<", Some("0"), "assert # @ f(rows) <= 5"),
                ("<=", Some("5"), "assert 0 < f(rows) @ #"),
            ]
        );
        let member = py("assert \"a\" not in rows");
        assert_eq!(member.len(), 1);
        assert_eq!(member[0].operator, "not in");
        assert_eq!(member[0].skeleton, "assert # @ rows");
        assert!(!member[0].fails_when_true);
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
}
