//! Equality assertions that compare an expression with itself (`assert_eq!(y, y)`,
//! `expect(x).toBe(x)`, `assert x == x`).
//!
//! A pack calls [`note`] at each place it reads the two operands of an equality
//! assertion. The answer is what the pack counts as a tautology; the record left on the
//! test is what `vacuous-tests` and `assertion-reduction` report by line.
//!
//! **Identity is exact form.** Two operands are the same when their leaf tokens are the
//! same in the same order. Tokens are read from the syntax tree: comments are left out,
//! white space between tokens does not count, and parentheses that wrap a whole operand
//! are removed (`(x)` is `x`). Nothing follows a value: `a` and `b` bound to the same
//! value are two operands, and so are `x.clone()` and `x`.
//!
//! **What is counted and not recorded.** An identical pair is a tautology to the pack
//! whatever it holds. It is recorded, and so reported under its own finding, unless:
//! - an operand contains a call, an object creation, a message send, a macro invocation,
//!   an assignment, an increment or decrement, or an `await` / `yield` ([`EFFECT_KINDS`];
//!   in a Rust macro argument, a parenthesised group that follows a name, and any `name!`
//!   group). Two evaluations of `it.next()` or `rand()` are not one value, and no list of
//!   such names is complete, so every call is left out;
//! - the assertion is inside a macro definition ([`MACRO_DEFINITION_KINDS`]: a Rust
//!   `macro_rules!` body, a C / C++ / Objective-C `#define`), or an operand holds a Rust
//!   macro metavariable (`$x`). What a macro expands to at its call sites is never read:
//!   the source is parsed, not its expansion.
//!
//! **Reflexivity tests.** [`EqualityOperands::reportable`] leaves out a self-comparison
//! whose operand is also compared with a different operand by another equality assertion
//! of the same test (`assert_eq!(a, a); assert_eq!(a, b);`): the test exercises the
//! equality of that value, and it holds a comparison of it that can fail.

use std::hash::{Hash, Hasher};
use tree_sitter::Node;

/// Node kinds that make an operand's value depend on when it is evaluated.
pub const EFFECT_KINDS: &[&str] = &[
    "call_expression",
    "call",
    "method_invocation",
    "invocation_expression",
    "object_creation_expression",
    "function_call_expression",
    "member_call_expression",
    "nullsafe_member_call_expression",
    "scoped_call_expression",
    "new_expression",
    "instance_expression",
    "message_expression",
    "macro_invocation",
    "update_expression",
    "assignment_expression",
    "augmented_assignment_expression",
    "await_expression",
    "await",
    "yield_expression",
    "yield",
];

/// Node kinds of a macro definition: an assertion inside one is not recorded.
pub const MACRO_DEFINITION_KINDS: &[&str] = &[
    "macro_definition",
    "macro_rule",
    "preproc_def",
    "preproc_function_def",
];

/// One equality assertion whose two operands are the same tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfComparison {
    /// 1-based line of the assertion's first operand.
    pub line: usize,
    /// The operand's tokens, hashed: what pairs the assertion across a change and with
    /// the other comparisons of its test. The text is not kept.
    pub operand: u64,
}

/// What a pack read about the operands of a test's equality assertions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EqualityOperands {
    /// Assertions that compare an operand with itself, in source order.
    pub same: Vec<SelfComparison>,
    /// Operands of the assertions that compare two different operands.
    pub different: Vec<u64>,
}

impl EqualityOperands {
    /// The self-comparisons a gate reports: those whose operand no other equality
    /// assertion of the test compares with a different operand.
    pub fn reportable(&self) -> Vec<&SelfComparison> {
        self.same
            .iter()
            .filter(|s| !self.different.contains(&s.operand))
            .collect()
    }

    /// The reportable self-comparisons of `self` (the head side of a changed test) that
    /// `base` did not already hold: one more than the base side had for that operand.
    pub fn introduced_since(&self, base: &EqualityOperands) -> Vec<&SelfComparison> {
        let mut held: Vec<u64> = base.same.iter().map(|s| s.operand).collect();
        self.reportable()
            .into_iter()
            .filter(|s| match held.iter().position(|o| *o == s.operand) {
                Some(at) => {
                    held.swap_remove(at);
                    false
                }
                None => true,
            })
            .collect()
    }
}

/// An operand as the nodes that make it up: one expression node, or the run of tokens
/// between two commas of a Rust macro's token tree.
#[derive(Clone, Copy)]
pub struct Operand<'a, 't> {
    pub nodes: &'a [Node<'t>],
}

fn push_gap<'s>(src: &'s [u8], from: usize, to: usize, out: &mut Vec<&'s str>) {
    if from >= to || to > src.len() {
        return;
    }
    if let Ok(text) = std::str::from_utf8(&src[from..to]) {
        out.extend(text.split_whitespace());
    }
}

/// The leaf tokens under `node`, comments left out. Text of a node that no child covers
/// (a grammar that keeps a literal's content on the parent) is kept as tokens too, so
/// two operands that differ anywhere differ here.
fn push_tokens<'s>(node: Node, src: &'s [u8], out: &mut Vec<&'s str>) {
    if node.kind().contains("comment") {
        return;
    }
    if node.child_count() == 0 {
        if let Ok(text) = node.utf8_text(src) {
            let text = text.trim();
            if !text.is_empty() {
                out.push(text);
            }
        }
        return;
    }
    let mut at = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        push_gap(src, at, child.start_byte(), out);
        push_tokens(child, src, out);
        at = child.end_byte();
    }
    push_gap(src, at, node.end_byte(), out);
}

/// Removes the parentheses that wrap the whole token run, as often as they do.
fn strip_wrapping_parentheses<'a, 's>(mut tokens: &'a [&'s str]) -> &'a [&'s str] {
    loop {
        if tokens.len() < 3 || tokens[0] != "(" || tokens[tokens.len() - 1] != ")" {
            return tokens;
        }
        // The opening parenthesis must close at the last token, not before it:
        // `(a) + (b)` is not wrapped.
        let mut depth = 0usize;
        for (i, t) in tokens.iter().enumerate() {
            match *t {
                "(" => depth += 1,
                ")" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && i + 1 < tokens.len() {
                        return tokens;
                    }
                }
                _ => {}
            }
        }
        tokens = &tokens[1..tokens.len() - 1];
    }
}

fn operand_tokens<'s>(operand: Operand, src: &'s [u8]) -> Vec<&'s str> {
    let mut all = Vec::new();
    for node in operand.nodes {
        push_tokens(*node, src, &mut all);
    }
    strip_wrapping_parentheses(&all).to_vec()
}

fn hash_tokens(tokens: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tokens.hash(&mut hasher);
    hasher.finish()
}

/// Whether two operands are the same tokens. Two empty operands are not.
pub fn same(a: Node, b: Node, src: &[u8]) -> bool {
    let (a, b) = (
        operand_tokens(Operand { nodes: &[a] }, src),
        operand_tokens(Operand { nodes: &[b] }, src),
    );
    !a.is_empty() && a == b
}

fn inside_macro_definition(node: Node) -> bool {
    let mut at = Some(node);
    while let Some(n) = at {
        if MACRO_DEFINITION_KINDS.contains(&n.kind()) {
            return true;
        }
        at = n.parent();
    }
    false
}

fn holds_kind(node: Node, kinds: &[&str]) -> bool {
    if kinds.contains(&node.kind()) {
        return true;
    }
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| holds_kind(c, kinds));
    found
}

/// A call written in the tokens of a Rust macro argument, where the grammar builds no
/// call node: a parenthesised group that follows a name, a closing group or `>`
/// (`f(x)`, `Some(x)`, `v.get(0)`, `f::<T>(x)`), and any group that follows `!`.
fn token_run_calls(nodes: &[Node], src: &[u8]) -> bool {
    for (i, node) in nodes.iter().enumerate() {
        if node.kind() != "token_tree" {
            continue;
        }
        let mut cursor = node.walk();
        let inner: Vec<Node> = node.children(&mut cursor).collect();
        if let Some(before) = i.checked_sub(1).map(|p| nodes[p]) {
            let parenthesised = inner.first().is_some_and(|d| d.kind() == "(");
            let follows_value =
                before.is_named() || matches!(before.utf8_text(src).unwrap_or(""), ">" | "self");
            if before.kind() == "!" || (parenthesised && follows_value) {
                return true;
            }
        }
        if token_run_calls(&inner, src) {
            return true;
        }
    }
    false
}

/// Reads the two operands of an equality assertion. Returns whether they are the same
/// tokens, which is what a pack counts as a tautology; when they are, and the form is
/// one this module records (see the module text), the assertion is recorded on `eq`.
/// Two different operands are recorded as compared with one another.
pub fn note_operands(eq: &mut EqualityOperands, a: Operand, b: Operand, src: &[u8]) -> bool {
    let (ta, tb) = (operand_tokens(a, src), operand_tokens(b, src));
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    if ta != tb {
        eq.different.push(hash_tokens(&ta));
        eq.different.push(hash_tokens(&tb));
        return false;
    }
    let Some(first) = a.nodes.first() else {
        return true;
    };
    let in_macro = inside_macro_definition(*first)
        || a.nodes.iter().any(|n| holds_kind(*n, &["metavariable"]));
    let evaluates =
        a.nodes.iter().any(|n| holds_kind(*n, EFFECT_KINDS)) || token_run_calls(a.nodes, src);
    if !in_macro && !evaluates {
        eq.same.push(SelfComparison {
            line: first.start_position().row + 1,
            operand: hash_tokens(&ta),
        });
    }
    true
}

/// [`note_operands`] for two expression nodes.
pub fn note(eq: &mut EqualityOperands, a: Node, b: Node, src: &[u8]) -> bool {
    note_operands(eq, Operand { nodes: &[a] }, Operand { nodes: &[b] }, src)
}

/// The two sides of an `==` (or `===`, `is`, `eq`) comparison that is the whole
/// condition of a boolean assertion, parentheses around it removed. `None` for any other
/// condition: an inequality, a conjunction, a comparison inside a larger expression.
pub fn equality_sides<'t>(cond: Node<'t>, src: &[u8]) -> Option<(Node<'t>, Node<'t>)> {
    let mut cond = cond;
    while matches!(
        cond.kind(),
        "parenthesized_expression" | "parenthesized_statements"
    ) && cond.named_child_count() == 1
    {
        cond = cond.named_child(0)?;
    }
    let mut cursor = cond.walk();
    let parts: Vec<Node> = cond
        .children(&mut cursor)
        .filter(|c| !c.kind().contains("comment"))
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let operator = parts[1].utf8_text(src).ok()?.trim();
    matches!(operator, "==" | "===" | "is" | "eq").then_some((parts[0], parts[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn rust_tree(code: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        crate::ast::source_text::parse(&mut parser, code).unwrap()
    }

    fn find<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        children.into_iter().find_map(|c| find(c, kind))
    }

    /// The two initialisers of `let a = <A>; let b = <B>;`.
    fn sides(tree: &tree_sitter::Tree) -> (Node<'_>, Node<'_>) {
        let body = find(tree.root_node(), "block").unwrap();
        let mut cursor = body.walk();
        let values: Vec<Node> = body
            .children(&mut cursor)
            .filter(|c| c.kind() == "let_declaration")
            .filter_map(|l| l.child_by_field_name("value"))
            .collect();
        (values[0], values[1])
    }

    fn compare(a: &str, b: &str) -> (bool, EqualityOperands) {
        let code = format!("fn f() {{ let a = {a}; let b = {b}; }}");
        let tree = rust_tree(&code);
        let (l, r) = sides(&tree);
        let mut eq = EqualityOperands::default();
        let same = note(&mut eq, l, r, code.as_bytes());
        (same, eq)
    }

    #[test]
    fn identity_is_by_token_and_ignores_space_comments_and_wrapping_parentheses() {
        for (a, b) in [
            ("y", "y"),
            ("a.b", "a . b"),
            ("a.b", "(a.b)"),
            ("((a + b))", "a + b"),
            ("a /* left */ + b", "a + b"),
            ("x[0]", "x[ 0 ]"),
        ] {
            let (same, eq) = compare(a, b);
            assert!(same, "{a} / {b}");
            assert_eq!(eq.same.len(), 1, "{a} / {b}");
            assert!(eq.different.is_empty(), "{a} / {b}");
        }
        // Near-misses: a different token anywhere, and parentheses that do not wrap the
        // whole operand.
        for (a, b) in [
            ("y", "z"),
            ("a.b", "a.c"),
            ("(a) + (b)", "a) + (b"),
            ("(a + b) * c", "a + b * c"),
            ("\"a b\"", "\"a  b\""),
            ("x[0]", "x[1]"),
        ] {
            let code = format!("fn f() {{ let a = {a}; let b = {b}; }}");
            let tree = rust_tree(&code);
            if tree.root_node().has_error() {
                continue;
            }
            let (l, r) = sides(&tree);
            let mut eq = EqualityOperands::default();
            assert!(!note(&mut eq, l, r, code.as_bytes()), "{a} / {b}");
            assert!(eq.same.is_empty(), "{a} / {b}");
            assert_eq!(eq.different.len(), 2, "{a} / {b}");
        }
    }

    #[test]
    fn an_operand_that_evaluates_something_is_counted_and_not_recorded() {
        for operand in ["it.next()", "f()", "v[i].len()", "Some(x)", "vec![1]"] {
            let (same, eq) = compare(operand, operand);
            assert!(same, "{operand}");
            assert!(eq.same.is_empty(), "{operand}");
        }
        // Control: the same shapes without a call are recorded.
        for operand in ["it.next", "v[i].len", "(x, y)"] {
            let (same, eq) = compare(operand, operand);
            assert!(same, "{operand}");
            assert_eq!(eq.same.len(), 1, "{operand}");
        }
    }

    /// The operands of the first `name!( a, b )` written as tokens under `node`.
    fn token_operands<'t>(tree: Node<'t>) -> (Vec<Node<'t>>, Vec<Node<'t>>) {
        let mut cursor = tree.walk();
        let inner: Vec<Node> = tree.children(&mut cursor).collect();
        let inner = &inner[1..inner.len() - 1];
        let comma = inner.iter().position(|n| n.kind() == ",").unwrap();
        (inner[..comma].to_vec(), inner[comma + 1..].to_vec())
    }

    #[test]
    fn an_assertion_in_a_macro_definition_is_not_recorded() {
        // The transcriber of a `macro_rules!` rule: `assert_eq ! ( $a , $a )` as tokens.
        let code = "macro_rules! same { ($a:expr) => { assert_eq!($a, $a); }; }";
        let tree = rust_tree(code);
        let rule = find(tree.root_node(), "macro_rule").expect("a macro rule");
        let body = rule.child_by_field_name("right").expect("a transcriber");
        let mut cursor = body.walk();
        let args = body
            .children(&mut cursor)
            .find(|c| c.kind() == "token_tree")
            .expect("the assertion's arguments");
        let (a, b) = token_operands(args);
        let mut eq = EqualityOperands::default();
        assert!(note_operands(
            &mut eq,
            Operand { nodes: &a },
            Operand { nodes: &b },
            code.as_bytes()
        ));
        assert!(eq.same.is_empty(), "{eq:?}");

        // A body with no metavariable is skipped for where it is.
        let code = "macro_rules! same { () => { assert_eq!(y, y); }; }";
        let tree = rust_tree(code);
        let rule = find(tree.root_node(), "macro_rule").unwrap();
        let body = rule.child_by_field_name("right").unwrap();
        let mut cursor = body.walk();
        let args = body
            .children(&mut cursor)
            .find(|c| c.kind() == "token_tree")
            .unwrap();
        let (a, b) = token_operands(args);
        let mut eq = EqualityOperands::default();
        assert!(note_operands(
            &mut eq,
            Operand { nodes: &a },
            Operand { nodes: &b },
            code.as_bytes()
        ));
        assert!(eq.same.is_empty(), "{eq:?}");

        // Control: the same invocation in a function is recorded.
        let code = "fn t() { assert_eq!(y, y); }";
        let tree = rust_tree(code);
        let call = find(tree.root_node(), "macro_invocation").unwrap();
        let mut cursor = call.walk();
        let args = call
            .children(&mut cursor)
            .find(|c| c.kind() == "token_tree")
            .unwrap();
        let (a, b) = token_operands(args);
        let mut eq = EqualityOperands::default();
        assert!(note_operands(
            &mut eq,
            Operand { nodes: &a },
            Operand { nodes: &b },
            code.as_bytes()
        ));
        assert_eq!(eq.same.len(), 1, "{eq:?}");
        assert_eq!(eq.same[0].line, 1);
    }

    fn operands(same: &[u64], different: &[u64]) -> EqualityOperands {
        EqualityOperands {
            same: same
                .iter()
                .enumerate()
                .map(|(i, o)| SelfComparison {
                    line: i + 1,
                    operand: *o,
                })
                .collect(),
            different: different.to_vec(),
        }
    }

    #[test]
    fn an_operand_also_compared_with_another_is_not_reportable() {
        // `assert_eq!(a, a); assert_eq!(a, b);`: the test compares `a` for real.
        assert!(operands(&[1], &[1, 2]).reportable().is_empty());
        // Control: the other comparison is of other operands.
        assert_eq!(operands(&[1], &[2, 3]).reportable().len(), 1);
        assert_eq!(operands(&[1, 4], &[1, 2]).reportable().len(), 1);
    }

    #[test]
    fn only_a_self_comparison_the_base_side_did_not_hold_is_introduced() {
        let base = operands(&[1], &[]);
        // Untouched.
        assert!(operands(&[1], &[]).introduced_since(&base).is_empty());
        // A second one of the same operand, and one of another operand.
        let lines = |head: &EqualityOperands| -> Vec<usize> {
            head.introduced_since(&base)
                .iter()
                .map(|s| s.line)
                .collect()
        };
        assert_eq!(lines(&operands(&[1, 1], &[])), vec![2]);
        assert_eq!(lines(&operands(&[1, 7], &[])), vec![2]);
        assert_eq!(lines(&operands(&[7], &[])), vec![1]);
        // A base side with none: every one is introduced.
        assert_eq!(
            operands(&[1, 2], &[])
                .introduced_since(&EqualityOperands::default())
                .len(),
            2
        );
    }

    #[test]
    fn the_sides_of_a_whole_equality_condition_are_read() {
        let read = |cond: &str| -> Option<(String, String)> {
            let code = format!("fn f() {{ let a = {cond}; let b = 0; }}");
            let tree = rust_tree(&code);
            let (c, _) = sides(&tree);
            equality_sides(c, code.as_bytes()).map(|(l, r)| {
                (
                    l.utf8_text(code.as_bytes()).unwrap().to_string(),
                    r.utf8_text(code.as_bytes()).unwrap().to_string(),
                )
            })
        };
        assert_eq!(read("x == y"), Some(("x".into(), "y".into())));
        assert_eq!(read("(x == x)"), Some(("x".into(), "x".into())));
        for other in ["x != x", "x == x && y", "!(x == x)", "x <= x", "f(x == x)"] {
            assert_eq!(read(other), None, "{other}");
        }
    }

    fn extracted(path: &str, src: &str) -> Vec<crate::ast::TestFn> {
        crate::ast::default_registry()
            .find_pack(path)
            .unwrap()
            .extract(path, src, &crate::ast::AssertVocabulary::default())
            .unwrap()
            .tests
    }

    /// One entry a pack: the path, a file with one test named `NAME` whose statements
    /// are `BODY`, the equality assertions of the pack with the same tokens on both
    /// sides, and near-misses (other operands, an inequality, the same call twice).
    /// Every exact form is also a tautology to its pack, but the Pest matcher of the PHP
    /// pack, which is recorded and still counted.
    const PACKS: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "tests/t.rs",
            "#[test]\nfn NAME() {\n    let r = make();\nBODY}\n",
            &[
                "    assert_eq!(y, y);",
                "    assert_eq!(a.b, a.b);",
                "    assert_eq!( a.b , (a.b) );",
                "    assert_eq!(a /* one */, a, \"why\");",
                "    debug_assert_eq!(v[0], v[0]);",
                "    prop_assert_eq!(&x, &x);",
            ],
            &[
                "    assert_eq!(x.clone(), x);",
                "    assert_eq!(a.b, a.c);",
                "    assert_ne!(z, z);",
                "    assert_eq!(it.next(), it.next());",
                "    assert_eq!(Some(q), Some(q));",
                "    assert!(w == w);",
            ],
        ),
        (
            "tests/test_t.py",
            "class T(unittest.TestCase):\n    def test_NAME(self):\n        r = make()\nBODY",
            &[
                "        assert x == x",
                "        assert (a.b == a.b)",
                "        assert y is y",
                "        self.assertEqual(a.b, a.b)",
                "        self.assertIs(z, (z))",
            ],
            &[
                "        assert x == w",
                "        assert x != x",
                "        assert next(it) == next(it)",
                "        self.assertEqual(a.b, a.c)",
                "        self.assertNotEqual(z, z)",
                "        assert x == x and y",
            ],
        ),
        (
            "t.test.ts",
            "test('NAME', () => {\n  const r = make();\nBODY});\n",
            &[
                "  expect(x).toBe(x);",
                "  expect(a.b).toEqual(a.b);",
                "  expect(a.b).toStrictEqual((a.b));",
                "  assert.strictEqual(y, y);",
                "  assert.deepEqual(z, z);",
            ],
            &[
                "  expect(x).toBe(w);",
                "  expect(x).not.toBe(x);",
                "  expect(next()).toBe(next());",
                "  assert.notStrictEqual(y, y);",
                "  expect(a.b).toBe(a.c);",
            ],
        ),
        (
            "t_test.go",
            "package x\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/assert\"\n\t\"github.com/stretchr/testify/require\"\n)\n\nfunc Test_NAME(t *testing.T) {\nBODY}\n",
            &[
                "\tassert.Equal(t, v, v)",
                "\trequire.Equal(t, a.B, a.B)",
                "\tassert.Same(t, p, (p))",
            ],
            &[
                "\tassert.Equal(t, v, w)",
                "\tassert.NotEqual(t, v, v)",
                "\tassert.Equal(t, next(), next())",
            ],
        ),
        (
            "src/test/java/TTest.java",
            "class TTest {\n    @Test\n    void NAME() {\nBODY    }\n}\n",
            &[
                "        assertEquals(x, x);",
                "        assertSame(a.b, a.b);",
                "        assertThat(y).isEqualTo(y);",
                "        assertThat(a.b).isSameAs((a.b));",
            ],
            &[
                "        assertEquals(x, w);",
                "        assertNotEquals(x, x);",
                "        assertEquals(it.next(), it.next());",
                "        assertThat(y).isEqualTo(w);",
                "        assertThat(y).isNotEqualTo(y);",
            ],
        ),
        (
            "src/test/kotlin/TTest.kt",
            "class TTest {\n    @Test\n    fun NAME() {\nBODY    }\n}\n",
            &[
                "        assertEquals(x, x)",
                "        assertSame(a.b, a.b)",
                "        y shouldBe y",
            ],
            &[
                "        assertEquals(x, w)",
                "        assertNotEquals(x, x)",
                "        assertEquals(it.next(), it.next())",
                "        y shouldBe w",
            ],
        ),
        (
            "tests/TTests.cs",
            "public class TTests\n{\n    [Fact] public void NAME() {\nBODY    }\n}\n",
            &[
                "        Assert.Equal(x, x);",
                "        Assert.Same(a.B, a.B);",
                "        Assert.StrictEqual(y, (y));",
                "        Assert.AreEqual(z, z);",
                "        Assert.AreSame(q.R, q.R);",
                "        Assert.That(w, Is.EqualTo(w));",
                "        Assert.That(u.V, Is.SameAs(u.V));",
            ],
            &[
                "        Assert.Equal(x, w);",
                "        Assert.NotEqual(x, x);",
                "        Assert.Equal(Next(), Next());",
                "        Assert.AreEqual(m, n);",
                "        Assert.AreNotEqual(m, m);",
                "        Assert.That(k, Is.Not.EqualTo(k));",
                "        Assert.That(k, Is.GreaterThan(k));",
            ],
        ),
        (
            "tests/t_test.cc",
            "#include <gtest/gtest.h>\n\nTEST(T, NAME) {\nBODY}\n",
            &[
                "  EXPECT_EQ(a, a);",
                "  ASSERT_EQ(r.f, r.f);",
                "  EXPECT_EQ(v[0], (v[0]));",
            ],
            &[
                "  EXPECT_EQ(a, b);",
                "  EXPECT_NE(a, a);",
                "  EXPECT_EQ(Next(), Next());",
                "  EXPECT_EQ(i++, i++);",
                "  EXPECT_DOUBLE_EQ(d, d);",
            ],
        ),
        (
            "test/t_test.rb",
            "class TTest < Minitest::Test\n  def test_NAME\n    r = make\nBODY  end\nend\n",
            &[
                "    assert_equal r, r",
                "    assert_same @x, @x",
                "    assert_equal(r, (r))",
            ],
            &[
                "    assert_equal r, w",
                "    refute_equal r, r",
                "    assert_equal e.next, e.next",
            ],
        ),
        (
            "tests/TTest.php",
            "<?php\nclass TTest extends TestCase {\n    public function testNAME(): void {\nBODY    }\n}\n",
            &[
                "        $this->assertSame($x, $x);",
                "        $this->assertEquals($a->b, $a->b);",
                "        expect($y)->toBe($y);",
            ],
            &[
                "        $this->assertSame($x, $w);",
                "        $this->assertNotSame($x, $x);",
                "        $this->assertSame(next($it), next($it));",
                "        expect($y)->toBe($w);",
            ],
        ),
        (
            "tests/AppTests/TTests.swift",
            "final class TTests: XCTestCase {\n    func testNAME() {\n        let r = make()\nBODY    }\n}\n",
            &[
                "        XCTAssertEqual(a, a)",
                "        XCTAssertEqual(r.f, r.f)",
                "        #expect(x == x)",
            ],
            &[
                "        XCTAssertEqual(a, b)",
                "        XCTAssertNotEqual(a, a)",
                "        XCTAssertEqual(next(), next())",
                "        #expect(x != x)",
            ],
        ),
        (
            "src/test/scala/TSpec.scala",
            "class TSpec extends AnyFunSuite {\n  test(\"NAME\") {\n    val r = make()\nBODY  }\n}\n",
            &[
                "    assert(x == x)",
                "    assertEquals(a.b, a.b)",
                "    y shouldBe y",
            ],
            &[
                "    assert(x == w)",
                "    assert(x != x)",
                "    assert(it.next() == it.next())",
                "    assertNotEquals(q, q)",
            ],
        ),
        (
            "tests/TTests.m",
            "@implementation TTests\n\n- (void)testNAME {\n    R *r = make();\nBODY}\n\n@end\n",
            &[
                "    XCTAssertEqual(a, a);",
                "    XCTAssertEqualObjects(r.f, r.f);",
            ],
            &[
                "    XCTAssertEqual(a, b);",
                "    XCTAssertNotEqual(a, a);",
                "    XCTAssertEqual([it next], [it next]);",
            ],
        ),
    ];

    fn pack_file(frame: &str, name: &str, lines: &[&str]) -> String {
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        frame.replace("NAME", name).replace("BODY", &body)
    }

    /// The 1-based line of each of `lines` in `src`.
    fn lines_in(src: &str, lines: &[&str]) -> Vec<usize> {
        lines
            .iter()
            .map(|l| src.lines().position(|s| s == *l).expect("the line") + 1)
            .collect()
    }

    #[test]
    fn every_pack_records_its_equality_assertions_with_identical_operands() {
        let mut wrong = Vec::new();
        for (path, frame, exact, near) in PACKS {
            // Positive: every exact form is recorded, at its line, and counted.
            let src = pack_file(frame, "exact", exact);
            let tests = extracted(path, &src);
            let recorded: Vec<usize> = tests
                .first()
                .map(|t| t.equality_operands.same.iter().map(|s| s.line).collect())
                .unwrap_or_default();
            if tests.len() != 1 || recorded != lines_in(&src, exact) {
                wrong.push(format!(
                    "{path}: exact forms on {:?}, recorded {recorded:?}",
                    lines_in(&src, exact)
                ));
            } else if tests[0].tautologies < exact.len() - usize::from(path.ends_with(".php")) {
                wrong.push(format!(
                    "{path}: {} exact forms, {} tautologies",
                    exact.len(),
                    tests[0].tautologies
                ));
            }
            // Negative: no near-miss is.
            let src = pack_file(frame, "near", near);
            let tests = extracted(path, &src);
            let recorded: Vec<usize> = tests
                .first()
                .map(|t| t.equality_operands.same.iter().map(|s| s.line).collect())
                .unwrap_or_default();
            if tests.len() != 1 || !recorded.is_empty() {
                wrong.push(format!("{path}: near-misses recorded on {recorded:?}"));
            }
        }
        assert_eq!(wrong, Vec::<String>::new());
    }

    /// Token identity sees more than the text comparison it replaces, and only that:
    /// the counts of a pack do not fall for any form.
    #[test]
    fn white_space_and_wrapping_parentheses_do_not_hide_a_tautology() {
        for (path, src) in [
            (
                "tests/t.rs",
                "#[test]\nfn a() {\n    assert_eq!(a.b, (a . b));\n}\n",
            ),
            (
                "tests/test_t.py",
                "def test_a():\n    assert (a.b) == a . b\n",
            ),
            (
                "t.test.js",
                "test('a', () => {\n  expect((a.b)).toBe(a . b);\n});\n",
            ),
        ] {
            let t = &extracted(path, src)[0];
            assert_eq!((t.total_asserts, t.tautologies), (1, 1), "{path}: {t:?}");
            assert!(t.is_vacuous(), "{path}");
        }
    }
}
