//! Calls inside test bodies that a test gate wants counted: sleeps, and assertions on
//! properties that are nearly always true.
//!
//! Same walk as `mocks.rs`: call nodes, judged by the callee and the call text up to the
//! first argument list, attributed to the innermost test whose span contains them.

use super::mocks::MockSpec;
use super::TestFn;
use tree_sitter::Node;

/// A hard-coded delay in a test: the shape of a race "fixed" by waiting.
pub const SLEEP_VOCAB: &[&str] = &[
    "thread::sleep(",
    "std::thread::sleep(",
    "tokio::time::sleep(",
    "async_std::task::sleep(",
    "time.sleep(",
    "asyncio.sleep(",
    "time.Sleep(",
    "Thread.sleep(",
    "TimeUnit.",
    "Task.Delay(",
    "Thread.Sleep(",
    "setTimeout(",
    "sleep(",
    "usleep(",
    "delay(",
];

/// An assertion that holds for nearly any value: it counts as an assertion and fails
/// almost nothing.
pub const TRIVIAL_ASSERT_VOCAB: &[&str] = &[
    // Python
    "assertIsNotNone(",
    "is not None",
    "assertIsInstance(",
    // JS/TS
    ".toBeDefined(",
    ".toBeTruthy(",
    ".not.toBeNull(",
    ".not.toBeUndefined(",
    ".toBeInstanceOf(",
    ".to.exist",
    ".to.be.ok",
    // Rust
    ".is_some()",
    ".is_ok()",
    ".is_empty() == false",
    // Go
    "NotNil(",
    "NoError(",
    // Java / C#
    "assertNotNull(",
    "IsNotNull(",
    "NotNull(",
    "Assert.NotNull(",
    "Should().NotBeNull(",
];

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Count calls whose callee or call prefix contains a `vocab` entry, per test, into the
/// field `pick` selects. A matching chain counts once.
pub fn count(
    root: Node,
    src: &str,
    tests: &mut [TestFn],
    spec: &MockSpec,
    vocab: &[&str],
    pick: fn(&mut TestFn) -> &mut usize,
) {
    if tests.is_empty() {
        return;
    }
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if spec.call_kinds.contains(&node.kind()) {
            let callee = spec
                .callee_fields
                .iter()
                .find_map(|f| node.child_by_field_name(f))
                .map(|n| text(n, src))
                .unwrap_or_else(|| text(node, src));
            let whole = text(node, src);
            // A macro's arguments are a token tree, not call nodes: `assert!(x.is_ok())`
            // is judged by its whole text.
            let cut = if node.kind() == "macro_invocation" {
                whole.len()
            } else {
                whole.find(['(', '{']).map_or(whole.len(), |i| i + 1)
            };
            let head = &whole[..cut.min(whole.len())];
            // `sleep(` alone would match `Thread.sleep(`; judge the callee's tail so a
            // bare `sleep(` needs the callee to end there.
            let hit = vocab.iter().any(|v| {
                callee.contains(v) || head.contains(v) || format!("{callee}(").ends_with(v)
            });
            if hit {
                let line = node.start_position().row + 1;
                if let Some(t) = tests
                    .iter_mut()
                    .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
                    .min_by_key(|t| t.end_line.saturating_sub(t.line))
                {
                    *pick(t) += 1;
                }
                continue;
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

/// Python's `assert x is not None` is a statement, not a call. Counted by text over
/// assert statements the walker cannot see as calls.
pub fn count_python_assert_statements(root: Node, src: &str, tests: &mut [TestFn]) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "assert_statement" {
            let t = text(node, src);
            if t.contains(" is not None") || t.contains("isinstance(") {
                let line = node.start_position().row + 1;
                if let Some(test) = tests
                    .iter_mut()
                    .filter(|x| x.line <= line && line <= x.end_line.max(x.line))
                    .min_by_key(|x| x.end_line.saturating_sub(x.line))
                {
                    test.trivial_asserts += 1;
                }
            }
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

pub fn sleeps(t: &mut TestFn) -> &mut usize {
    &mut t.sleeps
}

pub fn trivial_asserts(t: &mut TestFn) -> &mut usize {
    &mut t.trivial_asserts
}

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-rust"
))]
mod tests {
    use crate::ast::{default_registry, AssertVocabulary, TestFn};

    fn tests_of(path: &str, src: &str) -> Vec<TestFn> {
        let reg = default_registry();
        reg.find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
    }

    #[test]
    fn sleeps_and_trivial_assertions_are_counted_per_test() {
        let py = tests_of(
            "tests/test_a.py",
            "import time\n\ndef test_a():\n    time.sleep(0.5)\n    r = run()\n    assert r is not None\n\n\
             def test_b():\n    assert run() == 3\n",
        );
        assert_eq!((py[0].sleeps, py[0].trivial_asserts), (1, 1), "{:?}", py[0]);
        assert_eq!((py[1].sleeps, py[1].trivial_asserts), (0, 0));

        let ts = tests_of(
            "src/a.test.ts",
            "test('a', async () => {\n  await new Promise((r) => setTimeout(r, 200));\n  expect(run()).toBeDefined();\n});\n\
             test('b', () => {\n  expect(run()).toEqual(3);\n});\n",
        );
        assert_eq!((ts[0].sleeps, ts[0].trivial_asserts), (1, 1), "{:?}", ts[0]);
        assert_eq!((ts[1].sleeps, ts[1].trivial_asserts), (0, 0));

        let rs = tests_of(
            "src/lib.rs",
            "#[test]\nfn a() { std::thread::sleep(std::time::Duration::from_millis(50)); assert!(run().is_ok()); }\n\
             #[test]\nfn b() { assert_eq!(run().unwrap(), 3); }\n",
        );
        assert_eq!((rs[0].sleeps, rs[0].trivial_asserts), (1, 1), "{:?}", rs[0]);
        assert_eq!((rs[1].sleeps, rs[1].trivial_asserts), (0, 0));
    }
}
