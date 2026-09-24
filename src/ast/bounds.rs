//! Numeric bounds inside assertions: `assert elapsed < 1.5`, `pytest.approx(x, rel=1e-6)`,
//! `expect(ms).toBeLessThan(200)`, `if d > 1500*time.Millisecond { t.Fatal() }`.
//!
//! An assertion whose bound moves the loose way still counts as one assertion, so a
//! count cannot see `< 1.5` become `< 5.0`. Each bound is recorded with its *skeleton*
//! (the assertion's text with that literal replaced by `#`, whitespace collapsed), the
//! literal, and which way loosens it; `assertion-reduction` pairs a test's base and head
//! bounds by skeleton and reports a value that moved the loose way.
//!
//! Only literals in an assertion's own comparison or tolerance are read: a bound built
//! from a variable or a constant is not, and neither is a literal nested inside a call
//! (`Duration::from_millis(1500)`), except Go's `N*time.Unit`.

use super::TestFn;
use tree_sitter::Node;

/// One numeric bound of one assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub line: usize,
    /// The assertion's text with this literal replaced by `#`, whitespace collapsed.
    pub skeleton: String,
    /// The literal as written.
    pub literal: String,
    /// `true` when a larger value accepts more (an upper bound, a tolerance); `false`
    /// when a smaller one does (a lower bound, `places=`, `toBeCloseTo` digits).
    pub looser_when_larger: bool,
}

/// A bound whose value moved the loose way between base and head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loosened {
    pub line: usize,
    pub skeleton: String,
    pub from: String,
    pub to: String,
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// The value of a numeric literal as written (`1_500`, `1e-6`, `0.5f64`, `-2`), or `None`.
pub fn numeric(literal: &str) -> Option<f64> {
    let t = literal.trim().replace('_', "");
    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E');
    // A Rust suffix such as `f64` / `u32` ends in digits after its letter.
    let t = [
        "f32", "f64", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
        "i128", "isize",
    ]
    .iter()
    .find_map(|s| t.strip_suffix(s))
    .unwrap_or(t);
    if t.starts_with("0x") || t.starts_with("0b") || t.starts_with("0o") {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn skeleton(whole: Node, lit: Node, src: &str) -> String {
    let (s, e) = (whole.start_byte(), whole.end_byte());
    let (ls, le) = (lit.start_byte(), lit.end_byte());
    let raw = format!("{}#{}", &src[s..ls], &src[le..e]);
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attribute(tests: &mut [TestFn], bound: Bound) {
    let line = bound.line;
    if let Some(t) = tests
        .iter_mut()
        .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
        .min_by_key(|t| t.end_line.saturating_sub(t.line))
    {
        if !t.bounds.contains(&bound) {
            t.bounds.push(bound);
        }
    }
}

fn record(tests: &mut [TestFn], whole: Node, lit: Node, src: &str, looser_when_larger: bool) {
    if numeric(text(lit, src)).is_none() {
        return;
    }
    attribute(
        tests,
        Bound {
            line: lit.start_position().row + 1,
            skeleton: skeleton(whole, lit, src),
            literal: text(lit, src).to_string(),
            looser_when_larger,
        },
    );
}

/// For `left op right` where one side is a numeric literal, the literal and whether a
/// larger value loosens the check the comparison makes when it must hold.
fn comparison<'a>(
    left: Node<'a>,
    op: &str,
    right: Node<'a>,
    is_num: &dyn Fn(Node) -> bool,
) -> Option<(Node<'a>, bool)> {
    let upper_when_right = match op {
        "<" | "<=" => true,
        ">" | ">=" => false,
        _ => return None,
    };
    if is_num(right) {
        Some((right, upper_when_right))
    } else if is_num(left) {
        Some((left, !upper_when_right))
    } else {
        None
    }
}

fn walk(root: Node, f: &mut dyn FnMut(Node) -> bool) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if !f(node) {
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
}

fn py_num(n: Node) -> bool {
    matches!(n.kind(), "integer" | "float")
}

/// Tolerance keywords: a larger value accepts more.
const WIDER_WHEN_LARGER: &[&str] = &[
    "rel",
    "abs",
    "rel_tol",
    "abs_tol",
    "delta",
    "atol",
    "rtol",
    "epsilon",
    "max_relative",
    "max_ulps",
    "ulps",
];
/// Precision keywords: a smaller value accepts more.
const WIDER_WHEN_SMALLER: &[&str] = &["places", "decimal", "digits", "significant"];

/// Python: comparisons inside `assert`, tolerance keywords of calls inside `assert` or of
/// an `assert*` call, and `assertLess` / `assertGreater` / `assertAlmostEqual` literals.
pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        let stmt_is_assert_call = node.kind() == "expression_statement"
            && node.named_child(0).is_some_and(|c| {
                c.kind() == "call"
                    && c.child_by_field_name("function").is_some_and(|f| {
                        text(f, src)
                            .rsplit('.')
                            .next()
                            .unwrap_or("")
                            .starts_with("assert")
                    })
            });
        if node.kind() != "assert_statement" && !stmt_is_assert_call {
            return true;
        }
        walk(node, &mut |n| {
            match n.kind() {
                "comparison_operator" => {
                    let mut cursor = n.walk();
                    let kids: Vec<Node> = n.children(&mut cursor).collect();
                    for w in kids.windows(3) {
                        if let Some((lit, up)) = comparison(w[0], text(w[1], src), w[2], &|x| {
                            py_num(x)
                                || (x.kind() == "unary_operator"
                                    && x.named_child(0).is_some_and(py_num))
                        }) {
                            record(tests, node, lit, src, up);
                        }
                    }
                }
                "keyword_argument" => {
                    let name = n.child_by_field_name("name").map_or("", |x| text(x, src));
                    if let Some(v) = n.child_by_field_name("value").filter(|v| py_num(*v)) {
                        if WIDER_WHEN_LARGER.contains(&name) {
                            record(tests, node, v, src, true);
                        } else if WIDER_WHEN_SMALLER.contains(&name) {
                            record(tests, node, v, src, false);
                        }
                    }
                }
                "call" => {
                    let callee = n
                        .child_by_field_name("function")
                        .map_or("", |f| text(f, src));
                    let leaf = callee.rsplit('.').next().unwrap_or("");
                    let pos: Vec<Node> = n
                        .child_by_field_name("arguments")
                        .map(|a| {
                            let mut c = a.walk();
                            a.named_children(&mut c)
                                .filter(|x| x.kind() != "keyword_argument")
                                .collect()
                        })
                        .unwrap_or_default();
                    let (idx, up) = match leaf {
                        "assertLess" | "assertLessEqual" => (1, true),
                        "assertGreater" | "assertGreaterEqual" => (1, false),
                        "assertAlmostEqual" | "assertNotAlmostEqual" => (2, false),
                        _ => return true,
                    };
                    if let Some(v) = pos.get(idx).filter(|v| py_num(**v)) {
                        record(tests, node, *v, src, up);
                    }
                }
                _ => {}
            }
            true
        });
        false
    });
}

fn js_num(n: Node) -> bool {
    n.kind() == "number"
}

/// JS/TS: `expect(x).toBeLessThan(n)` and the other matchers with a numeric argument,
/// chai `below` / `above` / `most` / `least`, `toBeCloseTo(x, digits)`, and comparisons
/// inside an `assert(...)` / `expect(...)` call.
pub fn javascript(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "expression_statement" {
            return true;
        }
        let head = text(node, src).trim_start();
        if !(head.starts_with("expect") || head.starts_with("assert")) {
            return true;
        }
        walk(node, &mut |n| {
            match n.kind() {
                "call_expression" => {
                    let method = n
                        .child_by_field_name("function")
                        .filter(|f| f.kind() == "member_expression")
                        .and_then(|f| f.child_by_field_name("property"))
                        .map_or("", |p| text(p, src));
                    let args: Vec<Node> = n
                        .child_by_field_name("arguments")
                        .map(|a| {
                            let mut c = a.walk();
                            a.named_children(&mut c).collect()
                        })
                        .unwrap_or_default();
                    let (idx, up) = match method {
                        "toBeLessThan"
                        | "toBeLessThanOrEqual"
                        | "lessThan"
                        | "below"
                        | "most"
                        | "lte"
                        | "lt" => (0, true),
                        "toBeGreaterThan"
                        | "toBeGreaterThanOrEqual"
                        | "greaterThan"
                        | "above"
                        | "least"
                        | "gte"
                        | "gt" => (0, false),
                        "toBeCloseTo" => (1, false),
                        "closeTo" | "approximately" => (1, true),
                        _ => return true,
                    };
                    if let Some(v) = args.get(idx).filter(|v| js_num(**v)) {
                        record(tests, node, *v, src, up);
                    }
                }
                "binary_expression" => {
                    let op = n
                        .child_by_field_name("operator")
                        .map_or("", |o| text(o, src));
                    if let (Some(l), Some(r)) = (
                        n.child_by_field_name("left"),
                        n.child_by_field_name("right"),
                    ) {
                        if let Some((lit, up)) = comparison(l, op, r, &js_num) {
                            record(tests, node, lit, src, up);
                        }
                    }
                }
                _ => {}
            }
            true
        });
        false
    });
}

fn rs_num(n: Node) -> bool {
    matches!(n.kind(), "integer_literal" | "float_literal")
}

/// Rust: in `assert!` / `debug_assert!` / `prop_assert!`, a literal next to a comparison
/// at the macro's top level (`assert!((a - b).abs() < 1e-9)`), and `epsilon = 1e-6` style
/// tolerances in approx macros.
pub fn rust(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "macro_invocation" {
            return true;
        }
        let name = node
            .child_by_field_name("macro")
            .map_or("", |m| text(m, src));
        let name = name.rsplit("::").next().unwrap_or(name);
        if !(name.contains("assert")) {
            return false;
        }
        let Some(tt) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "token_tree")
        else {
            return false;
        };
        let mut cursor = tt.walk();
        let toks: Vec<Node> = tt.children(&mut cursor).collect();
        for (i, t) in toks.iter().enumerate() {
            if !rs_num(*t) {
                continue;
            }
            let before = i.checked_sub(1).map(|j| text(toks[j], src)).unwrap_or("");
            let after = toks.get(i + 1).map_or("", |n| text(*n, src));
            // `- 1e-9` after a comparison: the literal is the operand, sign included.
            let before = if before == "-" {
                i.checked_sub(2).map(|j| text(toks[j], src)).unwrap_or("")
            } else {
                before
            };
            // The literal is a whole operand only between an operand boundary and the
            // comparison: `x - 1 < y` compares an arithmetic expression, not `1`.
            let opens = |t: &str| matches!(t, "" | "(" | "," | "&&" | "||" | "!");
            let closes = |t: &str| matches!(t, "" | ")" | "," | "&&" | "||" | ";");
            let up = match (before, after) {
                ("<" | "<=", a) if closes(a) => Some(true),
                (">" | ">=", a) if closes(a) => Some(false),
                (b, "<" | "<=") if opens(b) => Some(false),
                (b, ">" | ">=") if opens(b) => Some(true),
                ("=", _) => {
                    let key = i.checked_sub(2).map(|j| text(toks[j], src)).unwrap_or("");
                    if WIDER_WHEN_LARGER.contains(&key) {
                        Some(true)
                    } else if WIDER_WHEN_SMALLER.contains(&key) {
                        Some(false)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(up) = up {
                record(tests, node, *t, src, up);
            }
        }
        false
    });
}

fn go_num(n: Node) -> bool {
    matches!(n.kind(), "int_literal" | "float_literal")
}

/// Go: `if <x op N> { ... t.Fatal / t.Error ... }` (the condition is the failure, so
/// `x > N` makes `N` an upper bound), `N*time.Unit` included; testify `Less` / `Greater`
/// / `InDelta` / `InEpsilon` literals.
pub fn go(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    // `1500*time.Millisecond` bounds by its literal.
    fn operand<'t>(n: Node<'t>, src: &str) -> Option<Node<'t>> {
        if go_num(n) {
            return Some(n);
        }
        if n.kind() == "binary_expression" {
            let (l, r) = (
                n.child_by_field_name("left")?,
                n.child_by_field_name("right")?,
            );
            let op = n
                .child_by_field_name("operator")
                .map_or("", |o| text(o, src));
            if op == "*" && go_num(l) && r.kind() == "selector_expression" {
                return Some(l);
            }
        }
        None
    }
    walk(root, &mut |node| {
        match node.kind() {
            "if_statement" => {
                let fails = node.child_by_field_name("consequence").is_some_and(|c| {
                    let t = text(c, src);
                    [".Fatal", ".Error", ".FailNow", ".Fail("]
                        .iter()
                        .any(|m| t.contains(m))
                });
                let Some(cond) = node.child_by_field_name("condition").filter(|_| fails) else {
                    return true;
                };
                if cond.kind() == "binary_expression" {
                    let op = cond
                        .child_by_field_name("operator")
                        .map_or("", |o| text(o, src));
                    if let (Some(l), Some(r)) = (
                        cond.child_by_field_name("left"),
                        cond.child_by_field_name("right"),
                    ) {
                        // The condition fails the test: `x > N` holds `x <= N`.
                        let lit = match (operand(l, src), operand(r, src)) {
                            (_, Some(n)) => Some((n, matches!(op, ">" | ">="))),
                            (Some(n), None) => Some((n, matches!(op, "<" | "<="))),
                            _ => None,
                        };
                        if let Some((lit, up)) =
                            lit.filter(|_| matches!(op, "<" | "<=" | ">" | ">="))
                        {
                            record(tests, cond, lit, src, up);
                        }
                    }
                }
                true
            }
            "call_expression" => {
                let callee = node
                    .child_by_field_name("function")
                    .map_or("", |f| text(f, src));
                let leaf = callee.rsplit('.').next().unwrap_or("");
                let (idx, up) = match leaf {
                    "Less" | "LessOrEqual" => (2, true),
                    "Greater" | "GreaterOrEqual" => (2, false),
                    "InDelta" | "InEpsilon" => (3, true),
                    _ => return true,
                };
                let args: Vec<Node> = node
                    .child_by_field_name("arguments")
                    .map(|a| {
                        let mut c = a.walk();
                        a.named_children(&mut c).collect()
                    })
                    .unwrap_or_default();
                if let Some(v) = args.get(idx).and_then(|a| operand(*a, src)) {
                    record(tests, node, v, src, up);
                }
                true
            }
            _ => true,
        }
    });
}

/// Bounds whose value moved the loose way from `base` to `head`. A skeleton that is not
/// exactly once on each side is ambiguous and skipped.
pub fn loosened(base: &[Bound], head: &[Bound]) -> Vec<Loosened> {
    let once = |set: &[Bound], b: &Bound| {
        set.iter()
            .filter(|x| x.skeleton == b.skeleton && x.looser_when_larger == b.looser_when_larger)
            .count()
            == 1
    };
    let mut out = Vec::new();
    for h in head.iter().filter(|h| once(head, h)) {
        let Some(b) = base
            .iter()
            .find(|b| b.skeleton == h.skeleton && b.looser_when_larger == h.looser_when_larger)
        else {
            continue;
        };
        if !once(base, b) {
            continue;
        }
        let (Some(bv), Some(hv)) = (numeric(&b.literal), numeric(&h.literal)) else {
            continue;
        };
        let looser = if h.looser_when_larger {
            hv > bv
        } else {
            hv < bv
        };
        if looser {
            out.push(Loosened {
                line: h.line,
                skeleton: h.skeleton.clone(),
                from: b.literal.clone(),
                to: h.literal.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn bounds(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<(String, bool)> {
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        facts
            .tests
            .iter()
            .flat_map(|t| t.bounds.iter())
            .map(|b| (b.literal.clone(), b.looser_when_larger))
            .collect()
    }

    fn pairs(v: &[(&str, bool)]) -> Vec<(String, bool)> {
        v.iter().map(|(l, u)| (l.to_string(), *u)).collect()
    }

    #[test]
    fn python_comparisons_tolerances_and_unittest_bounds() {
        let src = "import pytest\n\ndef test_x(self):\n    assert end - start < 1.5  # generous\n    assert 0.9 <= ratio\n    assert x == pytest.approx(y, rel=1e-6)\n    self.assertLess(ms, 200)\n    self.assertAlmostEqual(a, b, places=7)\n    assert n < limit\n    assert total - 1 == 3\n";
        assert_eq!(
            bounds(&crate::ast::python::PythonPack, "tests/test_x.py", src),
            pairs(&[
                ("1.5", true),
                ("0.9", false),
                ("1e-6", true),
                ("200", true),
                ("7", false)
            ])
        );
    }

    #[test]
    fn javascript_matchers_and_comparisons() {
        let src = "test('t', () => {\n  expect(ms).toBeLessThan(200);\n  expect(r).toBeGreaterThanOrEqual(0.9);\n  expect(v).toBeCloseTo(1.23, 2);\n  expect(ms < 50).toBe(true);\n  expect(a).toBe(3);\n});\n";
        assert_eq!(
            bounds(
                &crate::ast::javascript::JavaScriptPack,
                "src/x.test.js",
                src
            ),
            pairs(&[("200", true), ("0.9", false), ("2", false), ("50", true)])
        );
    }

    #[test]
    fn rust_assert_macros_read_whole_operands_only() {
        let src = "#[test]\nfn t() {\n    assert!((a - b).abs() < 1e-9);\n    assert!(0.5 <= r && r < 2.0);\n    assert!(x - 1 < y);\n    assert!(x < 2 * y);\n    assert_relative_eq!(a, b, epsilon = 1e-6);\n    assert_eq!(n, 3);\n}\n";
        assert_eq!(
            bounds(&crate::ast::rust::RustPack, "tests/t.rs", src),
            pairs(&[
                ("1e-9", true),
                ("0.5", false),
                ("2.0", true),
                ("1e-6", true)
            ])
        );
    }

    #[test]
    fn go_failure_conditions_durations_and_testify() {
        let src = "package p\n\nimport \"testing\"\n\nfunc TestT(t *testing.T) {\n\tif elapsed > 1500*time.Millisecond {\n\t\tt.Fatalf(\"slow\")\n\t}\n\tif ratio < 0.9 {\n\t\tt.Errorf(\"low\")\n\t}\n\tif n > 3 {\n\t\tlog.Print(\"n\")\n\t}\n\tassert.InDelta(t, 1.0, got, 0.01)\n}\n";
        assert_eq!(
            bounds(&crate::ast::go::GoPack, "p_test.go", src),
            pairs(&[("1500", true), ("0.9", false), ("0.01", true)])
        );
    }

    fn b(skeleton: &str, literal: &str, up: bool) -> Bound {
        Bound {
            line: 4,
            skeleton: skeleton.to_string(),
            literal: literal.to_string(),
            looser_when_larger: up,
        }
    }

    #[test]
    fn a_bound_is_loosened_only_when_it_moves_the_loose_way_once() {
        let s = "assert d < #";
        let got = loosened(&[b(s, "1.5", true)], &[b(s, "5.0", true)]);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].from.as_str(), got[0].to.as_str()), ("1.5", "5.0"));
        // Tightened, unchanged, or the other direction: nothing.
        assert!(loosened(&[b(s, "5.0", true)], &[b(s, "1.5", true)]).is_empty());
        assert!(loosened(&[b(s, "1.5", true)], &[b(s, "1.5", true)]).is_empty());
        assert_eq!(
            loosened(&[b(s, "0.9", false)], &[b(s, "0.5", false)]).len(),
            1
        );
        assert!(loosened(&[b(s, "0.5", false)], &[b(s, "0.9", false)]).is_empty());
        // A skeleton twice on one side is ambiguous.
        assert!(loosened(&[b(s, "1", true), b(s, "2", true)], &[b(s, "9", true)]).is_empty());
        // A different skeleton is a different assertion.
        assert!(loosened(&[b(s, "1", true)], &[b("assert e < #", "9", true)]).is_empty());
    }

    #[test]
    fn literals_parse_with_separators_suffixes_and_exponents() {
        assert_eq!(numeric("1_500"), Some(1500.0));
        assert_eq!(numeric("0.5f64"), Some(0.5));
        assert_eq!(numeric("10u32"), Some(10.0));
        assert_eq!(numeric("1e-6"), Some(1e-6));
        assert_eq!(numeric("-2"), Some(-2.0));
        assert_eq!(numeric("0x10"), None);
    }
}
