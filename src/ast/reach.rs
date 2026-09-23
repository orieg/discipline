//! Code the test runner never reaches: the body of an `if` whose condition is a constant
//! false, and the statements after an unconditional terminator (`return`, `panic!`,
//! `pytest.fail()`, `throw`) at the same block level. A skip call (`t.Skip`,
//! `pytest.skip`) or a thrown skip exception (`throw XCTSkip(...)`, `raise
//! unittest.SkipTest`) is not a terminator: the test is reported as ignored, and its
//! body stays what it is.
//!
//! An assertion there counts as an assertion to a line count and fails nothing. Each
//! pack's assertion walk skips nodes that start inside a dead range.

use tree_sitter::Node;

pub struct ReachSpec {
    /// `if` node kinds; the condition is the `condition` field or the first named child,
    /// the consequence the `consequence` / `body` field or the next named child.
    pub if_kinds: &'static [&'static str],
    /// Block node kinds whose named children are the statements of one level.
    pub block_kinds: &'static [&'static str],
    /// Node kinds that are not statements inside a block (comments).
    pub ignored_kinds: &'static [&'static str],
    /// Heads of a statement after which nothing at the same level runs.
    pub terminators: &'static [&'static str],
}

/// Byte ranges no execution reaches.
pub type DeadRanges = Vec<(usize, usize)>;

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// The Rust constant-expression check, shared: a condition that is the literal `false`
/// (or `0` where the language reads it as false), with any parentheses stripped.
pub fn is_constant_false(condition: &str) -> bool {
    let t = condition
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    matches!(t, "false" | "False" | "FALSE" | "0" | "0 == 1" | "1 == 0")
}

/// Whether a statement's text starts with a terminator head as a whole word: `return`
/// and `return x`, not `returned = 1`.
/// Exceptions a test framework raises to skip a test (`throw XCTSkip(...)`, `raise
/// unittest.SkipTest`, TestNG's `SkipException`, JUnit's `TestAbortedException`). A
/// statement throwing one is a skip, not a terminator.
const SKIP_EXCEPTIONS: &[&str] = &[
    "XCTSkip",
    "SkipTest",
    "SkipException",
    "skip.Exception",
    "TestAbortedException",
];

fn terminates(t: &str, spec: &ReachSpec) -> bool {
    let t = t.trim();
    if SKIP_EXCEPTIONS.iter().any(|s| t.contains(s)) {
        return false;
    }
    spec.terminators.iter().any(|h| {
        t.starts_with(h)
            && t[h.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

pub fn dead_ranges(root: Node, src: &str, spec: &ReachSpec) -> DeadRanges {
    let mut out: DeadRanges = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if spec.if_kinds.contains(&node.kind()) {
            let condition = node
                .child_by_field_name("condition")
                .or_else(|| node.named_child(0));
            let consequence = node
                .child_by_field_name("consequence")
                .or_else(|| node.child_by_field_name("body"))
                .or_else(|| node.named_child(1));
            if let (Some(c), Some(body)) = (condition, consequence) {
                if is_constant_false(text(c, src)) {
                    out.push((body.start_byte(), body.end_byte()));
                }
            }
        }
        if spec.block_kinds.contains(&node.kind()) {
            let mut cursor = node.walk();
            let stmts: Vec<Node> = node
                .named_children(&mut cursor)
                .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
                .collect();
            if let Some(i) = stmts.iter().position(|s| terminates(text(*s, src), spec)) {
                if let Some(next) = stmts.get(i + 1) {
                    out.push((next.start_byte(), node.end_byte()));
                }
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    out
}

/// Whether a node that starts at `at` is inside a dead range.
pub fn is_dead(ranges: &DeadRanges, at: usize) -> bool {
    ranges.iter().any(|(s, e)| *s <= at && at < *e)
}

#[cfg(test)]
mod tests {
    use super::is_constant_false;

    #[test]
    fn constant_false_conditions() {
        assert!(is_constant_false("false"));
        assert!(is_constant_false("(false)"));
        assert!(is_constant_false("0"));
        assert!(is_constant_false("False"));
        assert!(!is_constant_false("flag"));
        assert!(!is_constant_false("x == 0"));
        assert!(!is_constant_false("true"));
    }
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-go",
    feature = "lang-java",
    feature = "lang-kotlin"
))]
mod pack_tests {
    use crate::ast::{default_registry, AssertVocabulary};

    /// `(total, strong)` of each test in `src`, in order.
    fn counts(path: &str, src: &str) -> Vec<(usize, usize)> {
        let reg = default_registry();
        reg.find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
            .iter()
            .map(|t| (t.total_asserts, t.strong_asserts))
            .collect()
    }

    #[test]
    fn assertions_under_a_constant_false_branch_or_after_a_terminator_do_not_count() {
        // Rust: dead under `if false`, dead after `return`, live under a real condition and
        // in the `else` branch of a constant-false `if`.
        let rs = counts(
            "src/lib.rs",
            "#[test]\nfn dead_branch() { if false { assert_eq!(1, 2); } }\n\
             #[test]\nfn after_return() { return; assert_eq!(1, 2); }\n\
             #[test]\nfn after_panic() { panic!(\"not yet\"); assert_eq!(1, 2); }\n\
             #[test]\nfn live() { let flag = f(); if flag { assert_eq!(1, 2); } if false { } else { assert_eq!(3, 4); } }\n",
        );
        assert_eq!(rs, vec![(0, 0), (0, 0), (0, 0), (2, 2)]);
        let py = counts(
            "tests/test_a.py",
            "def test_dead_branch():\n    if False:\n        assert 1 == 2\n\n\
             def test_after_fail():\n    pytest.fail('later')\n    assert 1 == 2\n\n\
             def test_after_return():\n    return\n    assert 1 == 2\n\n\
             def test_live():\n    if flag:\n        assert 1 == 2\n    if 0:\n        pass\n    else:\n        assert 3 == 4\n",
        );
        assert_eq!(py, vec![(0, 0), (0, 0), (0, 0), (2, 2)]);
        let js = counts(
            "src/a.test.ts",
            "test('dead', () => { if (false) { expect(1).toBe(2); } return; expect(3).toBe(4); });\n\
             test('live', () => { if (flag) { expect(1).toBe(2); } });\n",
        );
        assert_eq!(js, vec![(0, 0), (1, 1)]);
        let go = counts(
            "pkg/a_test.go",
            "package a\nfunc TestDead(t *testing.T) {\n\tif false {\n\t\tt.Fatal(\"x\")\n\t}\n\treturn\n\tt.Fatal(\"y\")\n}\nfunc TestLive(t *testing.T) {\n\tif flag {\n\t\tt.Fatal(\"x\")\n\t}\n}\n",
        );
        assert_eq!(go, vec![(0, 0), (1, 1)]);
        let java = counts(
            "src/test/java/ATest.java",
            "class ATest {\n  @Test void dead() { if (false) { assertEquals(1, 2); } return; }\n  @Test void live() { if (flag) { assertEquals(1, 2); } }\n}\n",
        );
        assert_eq!(java, vec![(0, 0), (1, 1)]);
        let kt = counts(
            "src/test/kotlin/ATest.kt",
            "class ATest {\n    @Test\n    fun dead() {\n        if (false) {\n            assertEquals(1, 2)\n        }\n        return\n        assertEquals(3, 4)\n    }\n    @Test\n    fun live() {\n        if (flag) {\n            assertEquals(1, 2)\n        }\n    }\n}\n",
        );
        assert_eq!(kt, vec![(0, 0), (1, 1)]);
    }
}
