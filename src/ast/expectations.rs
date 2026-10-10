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

use super::ancestry::count;
use super::bounds::{skeleton, text, walk, GoFailures, Record, Records};
use super::TestFn;
use std::collections::HashMap;
use std::sync::Arc;
use tree_sitter::Node;

/// One expected value of one assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectation {
    pub line: usize,
    /// The assertion's text with this literal replaced by `#`, whitespace collapsed.
    pub skeleton: String,
    /// The literal as written.
    pub literal: String,
    /// The calls the assertion makes, each as `expected_exceptions::call_text` writes it.
    /// `assertion-reduction` reads them when an expected exception is dropped: the call
    /// that had to fail may now be asserted to return a value.
    pub calls: Calls,
}

/// The calls one assertion makes, in the order of the source: a list of texts.
///
/// The expected values of one assertion all hold the same calls, and an assertion
/// inside another holds a run of the outer one's. The list is therefore kept once and
/// each holder reads its run of it; before, an assertion of many literals held as many
/// copies of the list, each the text of every call nested in it.
#[derive(Clone)]
pub struct Calls {
    all: Arc<[String]>,
    from: usize,
    to: usize,
}

impl std::ops::Deref for Calls {
    type Target = [String];
    fn deref(&self) -> &[String] {
        &self.all[self.from..self.to]
    }
}

impl Default for Calls {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl From<Vec<String>> for Calls {
    fn from(calls: Vec<String>) -> Self {
        let to = calls.len();
        Self {
            all: calls.into(),
            from: 0,
            to,
        }
    }
}

impl std::fmt::Debug for Calls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

impl PartialEq for Calls {
    fn eq(&self, other: &Self) -> bool {
        let same_run =
            Arc::ptr_eq(&self.all, &other.all) && (self.from, self.to) == (other.from, other.to);
        same_run || **self == **other
    }
}

impl Eq for Calls {}

impl IntoIterator for Calls {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter().cloned().collect::<Vec<_>>().into_iter()
    }
}

impl Record for Expectation {
    fn line(&self) -> usize {
        self.line
    }
    fn written(&self) -> (&str, &str) {
        (&self.skeleton, &self.literal)
    }
    fn of(test: &mut TestFn) -> &mut Vec<Self> {
        &mut test.expectations
    }
}

/// What one reader records, and the calls of the assertion it last listed them for.
struct Reader<'a> {
    records: Records<'a, Expectation>,
    /// The calls of the outermost assertion last read.
    calls: Arc<[String]>,
    /// For each node inside that assertion, by its id, its run of `calls`.
    runs: HashMap<usize, (usize, usize)>,
}

impl<'a> Reader<'a> {
    fn new(tests: &'a mut [TestFn], src: &str) -> Self {
        Self {
            records: Records::new(tests, src),
            calls: Vec::new().into(),
            runs: HashMap::new(),
        }
    }

    /// The calls inside `whole`. They are listed when `whole` is not inside the
    /// assertion last listed, and are a run of that list when it is.
    fn calls_of(&mut self, whole: Node, src: &str) -> Calls {
        count(1);
        if !self.runs.contains_key(&whole.id()) {
            let (calls, runs) = super::expected_exceptions::calls_in(whole, src);
            self.calls = calls.into();
            self.runs = runs;
        }
        let (from, to) = self.runs[&whole.id()];
        Calls {
            all: Arc::clone(&self.calls),
            from,
            to,
        }
    }

    fn record(&mut self, whole: Node, lit: Node, src: &str) {
        let calls = self.calls_of(whole, src);
        self.records.add(Expectation {
            line: lit.start_position().row + 1,
            skeleton: skeleton(whole, lit, src),
            literal: text(lit, src).to_string(),
            calls,
        });
    }
}

/// Lines of `head` expectations whose literal differs from the base expectation with the
/// same skeleton. A skeleton that is not exactly once on each side is ambiguous and skipped.
pub fn changed(base: &[Expectation], head: &[Expectation]) -> Vec<usize> {
    // For a skeleton: how many expectations have it, and the first.
    fn tally(set: &[Expectation]) -> HashMap<&str, (usize, usize)> {
        let mut out = HashMap::new();
        for (at, x) in set.iter().enumerate() {
            out.entry(x.skeleton.as_str()).or_insert((0, at)).0 += 1;
        }
        out
    }
    let (in_base, in_head) = (tally(base), tally(head));
    head.iter()
        .filter(|h| in_head.get(h.skeleton.as_str()).map(|(n, _)| *n) == Some(1))
        .filter_map(|h| match in_base.get(h.skeleton.as_str()) {
            Some((1, at)) => (base[*at].literal != h.literal).then_some(h.line),
            _ => None,
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
    let reader = &mut Reader::new(tests, src);
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
                    [l] if rs_lit(*l) => reader.record(node, *l, src),
                    [m, l] if text(*m, src) == "-" && rs_lit(*l) => reader.record(node, *l, src),
                    _ => {}
                }
            }
        } else if name.contains("snapshot") {
            for w in toks.windows(2) {
                if text(w[0], src) == "@" && rs_lit(w[1]) {
                    reader.record(node, w[1], src);
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
                    reader.record(node, *t, src);
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
    let reader = &mut Reader::new(tests, src);
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
                        reader.record(node, lit, src);
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
                        reader.record(node, a, src);
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

const JS_DENO_EQ_METHODS: &[&str] = &[
    "assertEquals",
    "assertStrictEquals",
    "assertNotEquals",
    "assertNotStrictEquals",
];

/// JS/TS: the argument of `toBe` / `toEqual`-family matchers, both arguments of
/// `assert.equal`-family calls, Deno standard equality assertions (`assertEquals`,
/// `assertStrictEquals`, `assertNotEquals`, `assertNotStrictEquals`), any string
/// argument of `toMatchInlineSnapshot`, and a literal operand of `===` / `==` / `!==` /
/// `!=` inside an `assert(...)` / `expect(...)`.
pub fn javascript(
    root: Node,
    src: &str,
    tests: &mut [TestFn],
    std_asserts: &[(String, &'static str)],
) {
    if tests.is_empty() {
        return;
    }
    let reader = &mut Reader::new(tests, src);
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
                    let Some(func) = n.child_by_field_name("function") else {
                        return true;
                    };
                    let args: Vec<Node> = n
                        .child_by_field_name("arguments")
                        .map(|a| {
                            let mut c = a.walk();
                            a.named_children(&mut c).collect()
                        })
                        .unwrap_or_default();
                    let picked: Vec<Node> = match func.kind() {
                        "member_expression" => {
                            let method = func
                                .child_by_field_name("property")
                                .map_or("", |p| text(p, src));
                            let object = func
                                .child_by_field_name("object")
                                .map_or("", |o| text(o, src));
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
                            }
                        }
                        "identifier" => {
                            let name = text(func, src);
                            if JS_DENO_EQ_METHODS.contains(&name)
                                && std_asserts.iter().any(|(bound, _)| bound == name)
                            {
                                args.into_iter().take(2).collect()
                            } else {
                                Vec::new()
                            }
                        }
                        _ => Vec::new(),
                    };
                    for a in picked.into_iter().filter(|a| js_lit(*a, src)) {
                        reader.record(node, a, src);
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
                                reader.record(node, s, src);
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
    let reader = &mut Reader::new(tests, src);
    let failures = GoFailures::of(src);
    walk(root, &mut |node| match node.kind() {
        "if_statement" => {
            let fails = node
                .child_by_field_name("consequence")
                .is_some_and(|c| failures.inside(c));
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
                            reader.record(cond, s, src);
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
                    reader.record(node, *a, src);
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
    fn javascript_reads_deno_standard_assertions() {
        let src = r#"
import { assertEquals, assertStrictEquals, assertNotEquals, assertNotStrictEquals, assertEquals as same } from "jsr:@std/assert";
import { assertEquals as localEq } from "./local.js";

test("deno asserts", () => {
  assertEquals(f(1), 2);
  assertStrictEquals(x, "expected");
  assertNotEquals(y, 10);
  assertNotStrictEquals(z, null);
  same(a, 99);
  localEq(b, 100);
});
"#;
        assert_eq!(
            literals(
                &crate::ast::javascript::JavaScriptPack,
                "src/x.test.js",
                src
            ),
            vec!["2", "\"expected\"", "10", "null"]
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

    /// `open` written `n` times, `core`, then `close` written `n` times.
    fn nest(n: usize, open: impl Fn(usize) -> String, core: &str, close: &str) -> String {
        let mut out: String = (0..n).map(open).collect();
        out.push_str(core);
        out.push_str(&close.repeat(n));
        out
    }

    /// `item` written for each of `0..n`, joined by `between`.
    fn row(n: usize, item: impl Fn(usize) -> String, between: &str) -> String {
        (0..n).map(item).collect::<Vec<_>>().join(between)
    }

    /// One statement that holds `n` expected values or bounds, for each reader of this
    /// file and of `bounds`: a name, the path it is read under, and the source.
    type Source = (&'static str, &'static str, fn(usize) -> String);

    /// Sources whose one assertion holds every literal, so each literal's skeleton is
    /// the text of the whole statement and the facts are the square of the source.
    fn statements_of_many_literals() -> Vec<Source> {
        vec![
            ("TypeScript suites inside an expect", "tests/m.ts", |n| {
                format!(
                    "it('t', () => {{ expect({}).toBe(0); }});\n",
                    nest(
                        n,
                        |i| {
                            format!(
                            "describe('d{i}', () => {{ it('t{i}', () => {{ expect(a()).toBe({i}); }}); "
                        )
                        },
                        "",
                        "})"
                    )
                )
            }),
            // What a fuzzing run found: nested suites with one matcher name cut short
            // and the text up to a later quote gone, so one `expect` statement holds
            // the suites after it.
            (
                "TypeScript suites cut inside a matcher",
                "tests/m.ts",
                |n| {
                    let suite = |i: usize| {
                        format!("describe('d{i}', () => {{ it('t{i}', () => {{ expect(a()).toBe(1); }}); ")
                    };
                    format!(
                    "{}describe('d', () => {{ it('t', () => {{ expect(a()).to9', () => {{ it('t9', () => {{ expect(a()).toBe(1); }}); {}{}",
                    row(n / 4, suite, ""),
                    row(n, suite, ""),
                    "});\n".repeat(n + n / 4)
                )
                },
            ),
            ("JavaScript chained matchers", "tests/m.test.js", |n| {
                format!(
                    "test('t', () => {{ expect(a){}; }});\n",
                    row(n, |i| format!(".toBe({i})"), "")
                )
            }),
            ("JavaScript matchers in one call", "tests/m.test.js", |n| {
                format!(
                    "test('t', () => {{ expect([{}]); }});\n",
                    row(n, |i| format!("a.toBe({i})"), ", ")
                )
            }),
            ("JavaScript bounds in one call", "tests/m.test.js", |n| {
                format!(
                    "test('t', () => {{ expect([{}]); }});\n",
                    row(n, |i| format!("a.toBeLessThan({i})"), ", ")
                )
            }),
            (
                "JavaScript comparisons in one assert",
                "tests/m.test.js",
                |n| {
                    format!(
                        "test('t', () => {{ assert({}); }});\n",
                        row(n, |i| format!("a === {i}"), " && ")
                    )
                },
            ),
            ("Rust comparisons in one assert", "tests/t.rs", |n| {
                format!(
                    "#[test]\nfn t() {{ assert!({}); }}\n",
                    row(n, |i| format!("a == {i}"), " && ")
                )
            }),
            ("Rust bounds in one assert", "tests/t.rs", |n| {
                format!(
                    "#[test]\nfn t() {{ assert!({}); }}\n",
                    row(n, |i| format!("a < {i}"), " && ")
                )
            }),
            ("Python bounds in one assert", "tests/test_m.py", |n| {
                format!(
                    "def test_t():\n    assert {}\n",
                    row(n, |i| format!("a < {i}"), " and ")
                )
            }),
            (
                "Python calls nested in one assert",
                "tests/test_m.py",
                |n| {
                    format!(
                        "def test_t():\n    assert {} == 1\n",
                        nest(n, |_| "f(".to_string(), "1", ")")
                    )
                },
            ),
            ("Go nested testify equalities", "m_test.go", |n| {
                format!(
                    "package p\n\nfunc TestT(t *testing.T) {{\n\t{}\n}}\n",
                    nest(n, |i| format!("assert.Equal(t, {i}, "), "x", ")")
                )
            }),
            ("Go nested testify bounds", "m_test.go", |n| {
                format!(
                    "package p\n\nfunc TestT(t *testing.T) {{\n\t{}x{}\n}}\n",
                    "assert.Less(t, ".repeat(n),
                    row(n, |i| format!(", {i})"), "")
                )
            }),
        ]
    }

    /// Sources whose assertions each hold one literal: the facts grow with the source.
    fn statements_of_one_literal() -> Vec<Source> {
        vec![
            ("TypeScript nested suites", "tests/m.ts", |n| {
                nest(
                    n,
                    |i| {
                        format!("describe('d{i}', () => {{ it('t{i}', () => {{ expect(a()).toBe(1); }});\n")
                    },
                    "",
                    "});\n",
                )
            }),
            ("JavaScript assertions in a row", "tests/m.test.js", |n| {
                format!(
                    "test('t', () => {{\n{}}});\n",
                    row(n, |i| format!("  expect(a({i})).toBe({i});\n"), "")
                )
            }),
            ("Rust assertions in a row", "tests/t.rs", |n| {
                format!(
                    "#[test]\nfn t() {{\n{}}}\n",
                    row(n, |i| format!("    assert_eq!(f({i}), {i});\n"), "")
                )
            }),
            ("Python assertions in a row", "tests/test_m.py", |n| {
                format!(
                    "def test_t():\n{}",
                    row(n, |i| format!("    assert f({i}) == {i}\n"), "")
                )
            }),
            ("Go assertions in a row", "m_test.go", |n| {
                format!(
                    "package p\n\nfunc TestT(t *testing.T) {{\n{}}}\n",
                    row(n, |i| format!("\tassert.Equal(t, {i}, f({i}))\n"), "")
                )
            }),
            ("Go nested failure conditions", "m_test.go", |n| {
                format!(
                    "package p\n\nfunc TestT(t *testing.T) {{\n{}}}\n",
                    nest(n, |i| format!("if x == {i} {{\nt.Fatal()\n"), "", "}\n")
                )
            }),
        ]
    }

    /// The steps one extraction of `src` under `path` counts, and how many expected
    /// values and bounds it recorded.
    fn steps_of(path: &str, src: &str) -> (u64, usize) {
        let registry = crate::ast::default_registry();
        let pack = registry.find_pack(path).expect("a pack for the path");
        let (facts, counted) =
            crate::ast::ancestry::steps(|| pack.extract(path, src, &AssertVocabulary::default()));
        let facts = facts.unwrap_or_else(|e| panic!("{path} is read: {e:#}"));
        let recorded = facts
            .tests
            .iter()
            .map(|t| t.expectations.len() + t.bounds.len())
            .sum();
        (counted, recorded)
    }

    /// A statement that holds four times the literals costs under twenty times the
    /// steps. Each literal is recorded with the text of its whole statement, and the
    /// calls of a statement are listed each as its whole text, so what is recorded is
    /// the square of the source, sixteen times as much; the work to record it is held to
    /// that. Before #672 the calls of the statement were listed again for each literal
    /// and each new record was compared with every one before it, and the same sources
    /// cost 26 to 59 times as much at four times the size.
    ///
    /// What is counted, with what `ancestry` counts: every node a walk of `bounds`
    /// visits, every byte of a skeleton and of a call's text, every byte hashed or
    /// compared to tell a record from the ones before it, and every byte and line read
    /// to place a record in its test.
    #[test]
    fn a_statement_of_many_literals_costs_steps_in_proportion_to_what_is_recorded() {
        crate::deep_stack::on_deep_stack(|| {
            for (name, path, source) in statements_of_many_literals() {
                let ((shallow, few), (deep, many)) =
                    (steps_of(path, &source(40)), steps_of(path, &source(160)));
                assert!(few > 0 && many >= few, "{name}: {few} and {many} recorded");
                assert!(
                    deep < 20 * shallow,
                    "{name}: {shallow} steps for 40, {deep} for 160"
                );
            }
        })
        .unwrap();
    }

    /// Where each assertion holds one literal, four times the assertions cost under
    /// five times the steps: what is recorded grows with the source, and so does the
    /// work. Before #672 nested suites and nested failure conditions cost 12 and 13
    /// times as much: the text of each statement was checked as UTF-8 for each statement
    /// around it, and the body of each Go `if` was searched for each `if` around it.
    #[test]
    fn assertions_of_one_literal_cost_steps_in_proportion_to_the_source() {
        crate::deep_stack::on_deep_stack(|| {
            for (name, path, source) in statements_of_one_literal() {
                let ((shallow, few), (deep, many)) =
                    (steps_of(path, &source(40)), steps_of(path, &source(160)));
                assert!(few >= 40, "{name}: {few} recorded for 40");
                assert!(many >= 160, "{name}: {many} recorded for 160");
                assert!(
                    deep < 5 * shallow,
                    "{name}: {shallow} steps for 40, {deep} for 160"
                );
            }
        })
        .unwrap();
    }

    /// The bytes of what `tests` hold as expected values and bounds: each skeleton and
    /// literal, and each list of calls once however many expected values read it.
    fn bytes_recorded(tests: &[TestFn]) -> usize {
        let mut lists = std::collections::HashSet::new();
        let mut bytes = 0;
        for t in tests {
            for b in &t.bounds {
                bytes += b.skeleton.len() + b.literal.len();
            }
            for e in &t.expectations {
                bytes += e.skeleton.len() + e.literal.len();
                if lists.insert(Arc::as_ptr(&e.calls.all).cast::<u8>()) {
                    bytes += e.calls.all.iter().map(String::len).sum::<usize>();
                }
            }
        }
        bytes
    }

    /// The source a fuzzing run found (`fuzz/corpus/language_packs/
    /// nested_ts_suites_cut_inside_a_matcher`): 333 nested suites with one matcher name
    /// cut short, so that one `expect` statement of 19 kB holds 262 expected values. It
    /// is read in at most [`STEPS_PER_BYTE`] steps for each byte of the source and of
    /// what is recorded for it, and the 262 expected values share one list of calls.
    #[test]
    fn the_source_a_fuzzing_run_found_is_read_in_steps_bounded_by_what_is_recorded() {
        /// Under four times what it takes: 1.6 for each byte.
        const STEPS_PER_BYTE: u64 = 6;
        crate::deep_stack::on_deep_stack(|| {
            let seed = std::fs::read(format!(
                "{}/fuzz/corpus/language_packs/nested_ts_suites_cut_inside_a_matcher",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
            // The selector of `tests/m.ts` (`fuzz/fuzz_targets/language_packs.rs`).
            assert_eq!(seed[0], 0x23);
            let src = String::from_utf8(seed[1..].to_vec()).unwrap();
            let registry = crate::ast::default_registry();
            let pack = registry.find_pack("tests/m.ts").unwrap();
            let (facts, counted) = crate::ast::ancestry::steps(|| {
                pack.extract("tests/m.ts", &src, &AssertVocabulary::default())
            });
            let facts = facts.unwrap();
            let held: Vec<&Expectation> = facts
                .tests
                .iter()
                .flat_map(|t| t.expectations.iter())
                .collect();
            assert_eq!(held.len(), 262);
            let longest = held.iter().map(|e| e.skeleton.len()).max().unwrap();
            assert!(longest > 18_000, "the longest skeleton is {longest} bytes");
            let lists: std::collections::HashSet<_> =
                held.iter().map(|e| Arc::as_ptr(&e.calls.all)).collect();
            // One list for the statement that holds the suites, one for the two that
            // stand before it.
            assert!(lists.len() <= 3, "{} lists of calls", lists.len());
            let bytes = (src.len() + bytes_recorded(&facts.tests)) as u64;
            assert!(
                counted <= STEPS_PER_BYTE * bytes,
                "{counted} steps for {bytes} bytes read and recorded"
            );
        })
        .unwrap();
    }

    fn expectations(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<Expectation> {
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        facts
            .tests
            .into_iter()
            .flat_map(|t| t.expectations)
            .collect()
    }

    /// An expected value written the same on the same line as another is recorded once;
    /// on another line, or with another literal, it is another record.
    #[test]
    fn an_expected_value_equal_to_one_recorded_is_not_recorded_again() {
        let js = |body: &str| {
            expectations(
                &crate::ast::javascript::JavaScriptPack,
                "src/x.test.js",
                &format!("test('t', () => {{\n{body}}});\n"),
            )
            .into_iter()
            .map(|e| (e.line, e.literal))
            .collect::<Vec<_>>()
        };
        let one = |n: &str| n.to_string();
        assert_eq!(
            js("  expect(a).toBe(1); expect(a).toBe(1);\n"),
            vec![(2, one("1"))]
        );
        assert_eq!(
            js("  expect(a).toBe(1); expect(a).toBe(2);\n"),
            vec![(2, one("1")), (2, one("2"))]
        );
        assert_eq!(
            js("  expect(a).toBe(1);\n  expect(a).toBe(1);\n"),
            vec![(2, one("1")), (3, one("1"))]
        );
        // Bounds are told apart the same way.
        let bounds = crate::ast::javascript::JavaScriptPack
            .extract(
                "src/x.test.js",
                "test('t', () => {\n  expect(a).toBeLessThan(1); expect(a).toBeLessThan(1); expect(a).toBeLessThan(2);\n});\n",
                &AssertVocabulary::default(),
            )
            .unwrap()
            .tests
            .remove(0)
            .bounds;
        assert_eq!(
            bounds
                .iter()
                .map(|b| b.literal.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );
    }

    /// The calls of an assertion are the calls inside it: an assertion inside another
    /// holds its own, and the outer one holds both.
    #[test]
    fn an_assertion_inside_another_holds_the_calls_inside_it() {
        let held = expectations(
            &crate::ast::go::GoPack,
            "p_test.go",
            "package p\n\nimport \"testing\"\n\nfunc TestT(t *testing.T) {\n\tassert.Equal(t, 1, assert.Equal(t, 2, g()), h())\n}\n",
        );
        let calls: Vec<(&str, Vec<&str>)> = held
            .iter()
            .map(|e| {
                (
                    e.literal.as_str(),
                    e.calls.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                (
                    "1",
                    vec![
                        "assert . Equal ( t , 1 , assert . Equal ( t , 2 , g ( ) ) , h ( ) )",
                        "assert . Equal ( t , 2 , g ( ) )",
                        "g ( )",
                        "h ( )"
                    ]
                ),
                ("2", vec!["assert . Equal ( t , 2 , g ( ) )", "g ( )"]),
            ]
        );
        // Equal lists are equal whether or not they are one list.
        assert_eq!(held[1].calls, Calls::from(held[1].calls.to_vec()));
        assert_ne!(held[0].calls, held[1].calls);
        // A call under a comment node is not a token of the call around it.
        let js = expectations(
            &crate::ast::javascript::JavaScriptPack,
            "src/x.test.js",
            "test('t', () => {\n  expect(f(/* g(1) */ 2)).toBe(3);\n});\n",
        );
        assert_eq!(
            js[0].calls.to_vec(),
            vec![
                "expect ( f ( 2 ) ) . toBe ( 3 )",
                "expect ( f ( 2 ) )",
                "f ( 2 )"
            ]
        );
    }

    fn e(skeleton: &str, literal: &str, line: usize) -> Expectation {
        Expectation {
            line,
            skeleton: skeleton.to_string(),
            literal: literal.to_string(),
            calls: Calls::default(),
        }
    }

    /// A skeleton twice at head is as ambiguous as one twice at base, for an expected
    /// value and for a bound; the records around it are still paired.
    #[test]
    fn a_skeleton_twice_at_head_is_not_paired() {
        let base = [
            e("assert_eq!(x, #);", "1", 3),
            e("assert_eq!(y, #);", "1", 4),
        ];
        let head = [
            e("assert_eq!(x, #);", "8", 3),
            e("assert_eq!(x, #);", "9", 4),
            e("assert_eq!(y, #);", "7", 5),
        ];
        assert_eq!(changed(&base, &head), vec![5]);
        let bound = |skeleton: &str, literal: &str, line: usize| crate::ast::bounds::Bound {
            line,
            skeleton: skeleton.to_string(),
            literal: literal.to_string(),
            looser_when_larger: true,
        };
        let loosened = crate::ast::bounds::loosened(
            &[bound("assert a < #", "1", 3), bound("assert b < #", "1", 4)],
            &[
                bound("assert a < #", "8", 3),
                bound("assert a < #", "9", 4),
                bound("assert b < #", "7", 5),
            ],
        );
        assert_eq!(loosened.iter().map(|l| l.line).collect::<Vec<_>>(), vec![5]);
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
