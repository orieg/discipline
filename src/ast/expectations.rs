//! Expected values inside equality assertions: `assert_eq!(add(2, 2), 4)`,
//! `assert parse("1") == 1`, `expect(f(1)).toBe(2)`, `assert.Equal(t, 4, got)`, and inline
//! snapshots (`assert_snapshot!(x, @"...")`, `toMatchInlineSnapshot(`...`)`).
//!
//! Editing the expected value of an existing assertion keeps its count and its strength,
//! so a count cannot see a test made to agree with changed code. Each literal that is a
//! whole operand of an equality assertion is recorded with its *skeleton* (the assertion's
//! text with that literal replaced by `#`, whitespace collapsed); `assertion-reduction`
//! pairs a test's base and head expectations by skeleton and reports a literal that changed.
//!
//! Only whole operands are read: `add(2, 2)` is the call under test, not an expectation,
//! and a value built from a variable or a constant is not read. The assertion message
//! (`assert_eq!(a, 4, "msg")`) is not an operand.

use super::bounds::{skeleton, text, walk};
use super::TestFn;
use tree_sitter::Node;

/// One expected value of one assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectation {
    pub line: usize,
    /// The assertion's text with this literal replaced by `#`, whitespace collapsed.
    pub skeleton: String,
    /// The literal as written.
    pub literal: String,
}

fn attribute(tests: &mut [TestFn], e: Expectation) {
    let line = e.line;
    if let Some(t) = tests
        .iter_mut()
        .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
        .min_by_key(|t| t.end_line.saturating_sub(t.line))
    {
        if !t.expectations.contains(&e) {
            t.expectations.push(e);
        }
    }
}

fn record(tests: &mut [TestFn], whole: Node, lit: Node, src: &str) {
    attribute(
        tests,
        Expectation {
            line: lit.start_position().row + 1,
            skeleton: skeleton(whole, lit, src),
            literal: text(lit, src).to_string(),
        },
    );
}

/// Lines of `head` expectations whose literal differs from the base expectation with the
/// same skeleton. A skeleton that is not exactly once on each side is ambiguous and skipped.
pub fn changed(base: &[Expectation], head: &[Expectation]) -> Vec<usize> {
    let once = |set: &[Expectation], s: &str| set.iter().filter(|x| x.skeleton == s).count() == 1;
    head.iter()
        .filter(|h| once(head, &h.skeleton) && once(base, &h.skeleton))
        .filter_map(|h| {
            let b = base.iter().find(|b| b.skeleton == h.skeleton)?;
            (b.literal != h.literal).then_some(h.line)
        })
        .collect()
}

fn rs_lit(n: Node) -> bool {
    matches!(
        n.kind(),
        "integer_literal"
            | "float_literal"
            | "string_literal"
            | "raw_string_literal"
            | "char_literal"
            | "boolean_literal"
    )
}

/// Equality macros whose first two arguments are the compared pair.
const RS_EQ: &[&str] = &[
    "assert_eq",
    "assert_ne",
    "debug_assert_eq",
    "debug_assert_ne",
    "prop_assert_eq",
    "prop_assert_ne",
    "assert_str_eq",
];

/// Rust: the first two arguments of `assert_eq!`-family macros when an argument is one
/// literal (`-` sign included), a literal next to `==` / `!=` at the top level of another
/// `assert!`-like macro, and the `@"..."` of insta's inline snapshots.
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
        if !name.contains("assert") {
            return false;
        }
        let Some(tt) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "token_tree")
        else {
            return false;
        };
        let mut cursor = tt.walk();
        let all: Vec<Node> = tt.children(&mut cursor).collect();
        // The delimiters of the token tree are its first and last children.
        let toks: &[Node] = if all.len() >= 2 {
            &all[1..all.len() - 1]
        } else {
            &[]
        };
        if RS_EQ.contains(&name) {
            for arg in toks.split(|t| text(*t, src) == ",").take(2) {
                match arg {
                    [l] if rs_lit(*l) => record(tests, node, *l, src),
                    [m, l] if text(*m, src) == "-" && rs_lit(*l) => record(tests, node, *l, src),
                    _ => {}
                }
            }
        } else if name.contains("snapshot") {
            for w in toks.windows(2) {
                if text(w[0], src) == "@" && rs_lit(w[1]) {
                    record(tests, node, w[1], src);
                }
            }
        } else {
            let opens = |t: &str| matches!(t, "" | "(" | "," | "&&" | "||" | "!");
            let closes = |t: &str| matches!(t, "" | ")" | "," | "&&" | "||" | ";");
            for (i, t) in toks.iter().enumerate() {
                if !rs_lit(*t) {
                    continue;
                }
                let before = i.checked_sub(1).map(|j| text(toks[j], src)).unwrap_or("");
                let after = toks.get(i + 1).map_or("", |n| text(*n, src));
                let eq = |o: &str| matches!(o, "==" | "!=");
                if (eq(before) && closes(after)) || (eq(after) && opens(before)) {
                    record(tests, node, *t, src);
                }
            }
        }
        false
    });
}

fn py_lit(n: Node) -> bool {
    match n.kind() {
        "integer" | "float" | "true" | "false" | "none" => true,
        // An f-string is built from values, not written.
        "string" => !n
            .children(&mut n.walk())
            .any(|c| c.kind() == "interpolation"),
        "unary_operator" => n
            .named_child(0)
            .is_some_and(|c| matches!(c.kind(), "integer" | "float")),
        _ => false,
    }
}

/// The literal of an operand: the operand itself, or the one argument of an
/// `inline-snapshot` call (`snapshot("...")`).
fn py_operand<'t>(n: Node<'t>, src: &str) -> Option<Node<'t>> {
    if py_lit(n) {
        return Some(n);
    }
    if n.kind() == "call"
        && n.child_by_field_name("function")
            .is_some_and(|f| text(f, src) == "snapshot")
    {
        let args = n.child_by_field_name("arguments")?;
        let mut c = args.walk();
        let pos: Vec<Node> = args.named_children(&mut c).collect();
        if let [one] = pos.as_slice() {
            return py_lit(*one).then_some(*one);
        }
    }
    None
}

const PY_EQ_CALLS: &[&str] = &[
    "assertEqual",
    "assertEquals",
    "assertNotEqual",
    "assertNotEquals",
    "assertIs",
    "assertIsNot",
    "assertMultiLineEqual",
];

/// Python: `assert a == <literal>` / `!=` / `is` / `is not` (one comparison), the first two
/// positional arguments of `assertEqual`-family calls, and `snapshot("...")` operands.
pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| match node.kind() {
        "assert_statement" => {
            let Some(cmp) = node
                .named_child(0)
                .filter(|c| c.kind() == "comparison_operator")
            else {
                return false;
            };
            let mut cursor = cmp.walk();
            let named: Vec<Node> = cmp.named_children(&mut cursor).collect();
            let op: String = cmp
                .children(&mut cmp.walk())
                .filter(|c| !c.is_named())
                .map(|c| text(c, src))
                .collect::<Vec<_>>()
                .join(" ");
            if named.len() == 2 && matches!(op.as_str(), "==" | "!=" | "is" | "is not") {
                for side in named {
                    if let Some(lit) = py_operand(side, src) {
                        record(tests, node, lit, src);
                    }
                }
            }
            false
        }
        "expression_statement" => {
            let Some(call) = node.named_child(0).filter(|c| c.kind() == "call") else {
                return true;
            };
            let leaf = call
                .child_by_field_name("function")
                .map_or("", |f| text(f, src))
                .rsplit('.')
                .next()
                .unwrap_or("");
            if !PY_EQ_CALLS.contains(&leaf) {
                return true;
            }
            if let Some(args) = call.child_by_field_name("arguments") {
                let mut c = args.walk();
                let pos: Vec<Node> = args
                    .named_children(&mut c)
                    .filter(|x| x.kind() != "keyword_argument")
                    .collect();
                for a in pos.into_iter().take(2) {
                    if py_lit(a) {
                        record(tests, node, a, src);
                    }
                }
            }
            false
        }
        _ => true,
    });
}

fn js_lit(n: Node, src: &str) -> bool {
    match n.kind() {
        "number" | "string" | "true" | "false" | "null" => true,
        "template_string" => !n
            .children(&mut n.walk())
            .any(|c| c.kind() == "template_substitution"),
        "undefined" => true,
        "identifier" => text(n, src) == "undefined",
        "unary_expression" => n
            .child_by_field_name("argument")
            .is_some_and(|a| a.kind() == "number"),
        _ => false,
    }
}

/// Matchers whose first argument is the expected value.
const JS_MATCHERS: &[&str] = &[
    "toBe",
    "toEqual",
    "toStrictEqual",
    "toHaveLength",
    "toThrow",
    "toThrowError",
    "equal",
    "equals",
    "eql",
];
/// `assert.<method>(actual, expected)`: both arguments are the compared pair.
const JS_ASSERT_METHODS: &[&str] = &[
    "equal",
    "strictEqual",
    "deepEqual",
    "deepStrictEqual",
    "notEqual",
    "notStrictEqual",
    "notDeepEqual",
    "notDeepStrictEqual",
];

/// JS/TS: the argument of `toBe` / `toEqual`-family matchers, both arguments of
/// `assert.equal`-family calls, any string argument of `toMatchInlineSnapshot`, and a
/// literal operand of `===` / `==` / `!==` / `!=` inside an `assert(...)` / `expect(...)`.
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
                    let Some(member) = n
                        .child_by_field_name("function")
                        .filter(|f| f.kind() == "member_expression")
                    else {
                        return true;
                    };
                    let method = member
                        .child_by_field_name("property")
                        .map_or("", |p| text(p, src));
                    let object = member
                        .child_by_field_name("object")
                        .map_or("", |o| text(o, src));
                    let args: Vec<Node> = n
                        .child_by_field_name("arguments")
                        .map(|a| {
                            let mut c = a.walk();
                            a.named_children(&mut c).collect()
                        })
                        .unwrap_or_default();
                    let picked: Vec<Node> =
                        if object == "assert" && JS_ASSERT_METHODS.contains(&method) {
                            args.into_iter().take(2).collect()
                        } else if JS_MATCHERS.contains(&method) {
                            args.into_iter().take(1).collect()
                        } else if method == "toMatchInlineSnapshot" {
                            args.into_iter()
                                .filter(|a| matches!(a.kind(), "string" | "template_string"))
                                .collect()
                        } else {
                            Vec::new()
                        };
                    for a in picked.into_iter().filter(|a| js_lit(*a, src)) {
                        record(tests, node, a, src);
                    }
                }
                "binary_expression" => {
                    let op = n
                        .child_by_field_name("operator")
                        .map_or("", |o| text(o, src));
                    if matches!(op, "===" | "==" | "!==" | "!=") {
                        for side in ["left", "right"] {
                            if let Some(s) = n.child_by_field_name(side).filter(|s| js_lit(*s, src))
                            {
                                record(tests, node, s, src);
                            }
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

fn go_lit(n: Node) -> bool {
    matches!(
        n.kind(),
        "int_literal"
            | "float_literal"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "rune_literal"
            | "true"
            | "false"
            | "nil"
    )
}

/// testify calls and the argument positions of the compared values, counted after the
/// `t` argument (`assert.Equal(t, 4, got)`); `assert.New(t)` forms have no `t`.
const GO_CALLS: &[(&str, &[usize])] = &[
    ("Equal", &[0, 1]),
    ("Equalf", &[0, 1]),
    ("NotEqual", &[0, 1]),
    ("NotEqualf", &[0, 1]),
    ("EqualValues", &[0, 1]),
    ("Exactly", &[0, 1]),
    ("EqualError", &[1]),
    ("Len", &[1]),
];

/// Go: testify `Equal`-family literals, and `if <x == / != N> { ... t.Fatal / t.Error ... }`.
pub fn go(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| match node.kind() {
        "if_statement" => {
            let fails = node.child_by_field_name("consequence").is_some_and(|c| {
                let t = text(c, src);
                [".Fatal", ".Error", ".FailNow", ".Fail("]
                    .iter()
                    .any(|m| t.contains(m))
            });
            if let Some(cond) = node
                .child_by_field_name("condition")
                .filter(|c| fails && c.kind() == "binary_expression")
            {
                let op = cond
                    .child_by_field_name("operator")
                    .map_or("", |o| text(o, src));
                if matches!(op, "==" | "!=") {
                    for side in ["left", "right"] {
                        if let Some(s) = cond.child_by_field_name(side).filter(|s| go_lit(*s)) {
                            record(tests, cond, s, src);
                        }
                    }
                }
            }
            true
        }
        "call_expression" => {
            let leaf = node
                .child_by_field_name("function")
                .map_or("", |f| text(f, src))
                .rsplit('.')
                .next()
                .unwrap_or("");
            let Some((_, positions)) = GO_CALLS.iter().find(|(n, _)| *n == leaf) else {
                return true;
            };
            let args: Vec<Node> = node
                .child_by_field_name("arguments")
                .map(|a| {
                    let mut c = a.walk();
                    a.named_children(&mut c).collect()
                })
                .unwrap_or_default();
            // `assert.Equal(t, ...)`: the first argument is the test handle, not a value.
            let skip = usize::from(args.first().is_some_and(|a| a.kind() == "identifier"));
            for p in positions.iter() {
                if let Some(a) = args.get(p + skip).filter(|a| go_lit(**a)) {
                    record(tests, node, *a, src);
                }
            }
            true
        }
        _ => true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn literals(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<String> {
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        facts
            .tests
            .iter()
            .flat_map(|t| t.expectations.iter())
            .map(|e| e.literal.clone())
            .collect()
    }

    #[test]
    fn rust_reads_whole_operands_of_equality_macros_and_inline_snapshots() {
        let src = "#[test]\nfn t() {\n    assert_eq!(add(2, 2), 4);\n    assert_ne!(name, \"x\", \"message {}\", y);\n    assert_eq!(-1, f());\n    assert!(n == 3 && ok);\n    assert!(n - 1 == m);\n    assert_eq!(a, b);\n    insta::assert_snapshot!(out, @\"hello\");\n    assert!(x < 5);\n}\n";
        assert_eq!(
            literals(&crate::ast::rust::RustPack, "tests/t.rs", src),
            vec!["4", "\"x\"", "1", "3", "\"hello\""]
        );
    }

    #[test]
    fn python_reads_equality_asserts_unittest_and_inline_snapshots() {
        let src = "def test_p(self):\n    assert parse(\"1\") == 1\n    assert name != 'x', 'msg'\n    assert v is None\n    assert f\"{a}\" == b\n    assert total - 1 == m\n    self.assertEqual(f(2), 4)\n    assert out == snapshot(\"text\")\n    assert n < 5\n    assert a == b\n";
        assert_eq!(
            literals(&crate::ast::python::PythonPack, "tests/test_p.py", src),
            vec!["1", "'x'", "None", "4", "\"text\""]
        );
    }

    #[test]
    fn javascript_reads_matchers_assert_methods_and_inline_snapshots() {
        let src = "test('t', () => {\n  expect(f(1)).toBe(2);\n  expect(x).not.toEqual('y');\n  assert.strictEqual(g(), true);\n  expect(out).toMatchInlineSnapshot(`\"ok\"`);\n  expect(a).toBe(b);\n  expect(ms).toBeLessThan(200);\n  assert(n === 3);\n});\n";
        assert_eq!(
            literals(
                &crate::ast::javascript::JavaScriptPack,
                "src/x.test.js",
                src
            ),
            vec!["2", "'y'", "true", "`\"ok\"`", "3"]
        );
    }

    #[test]
    fn go_reads_testify_equals_and_failure_conditions() {
        let src = "package p\n\nimport \"testing\"\n\nfunc TestT(t *testing.T) {\n\tassert.Equal(t, 4, add(2, 2), \"four\")\n\tif got != \"ok\" {\n\t\tt.Errorf(\"bad\")\n\t}\n\tif n == 3 {\n\t\tlog.Print(\"n\")\n\t}\n\trequire.Len(t, items, 2)\n\tassert.Equal(t, want, got)\n}\n";
        assert_eq!(
            literals(&crate::ast::go::GoPack, "p_test.go", src),
            vec!["4", "\"ok\"", "2"]
        );
    }

    fn e(skeleton: &str, literal: &str, line: usize) -> Expectation {
        Expectation {
            line,
            skeleton: skeleton.to_string(),
            literal: literal.to_string(),
        }
    }

    #[test]
    fn changed_pairs_by_skeleton_and_skips_ambiguous_ones() {
        let base = [e("assert_eq!(add(2, 2), #);", "4", 3)];
        // The expected value moved: reported at the head line.
        assert_eq!(
            changed(&base, &[e("assert_eq!(add(2, 2), #);", "5", 7)]),
            vec![7]
        );
        // Same literal (the line moved): quiet.
        assert!(changed(&base, &[e("assert_eq!(add(2, 2), #);", "4", 9)]).is_empty());
        // The call under test changed too: a different skeleton, not paired.
        assert!(changed(&base, &[e("assert_eq!(add(3, 2), #);", "5", 3)]).is_empty());
        // An added assertion has no base partner.
        assert!(changed(&[], &[e("assert_eq!(x, #);", "1", 3)]).is_empty());
        // A skeleton twice on a side is ambiguous.
        let twice = [
            e("assert_eq!(x, #);", "1", 3),
            e("assert_eq!(x, #);", "2", 4),
        ];
        assert!(changed(&twice, &[e("assert_eq!(x, #);", "9", 3)]).is_empty());
    }
}
