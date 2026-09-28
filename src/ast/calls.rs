//! Calls inside test bodies that a test gate wants counted: sleeps, and assertions on
//! properties that are nearly always true.
//!
//! Same walk as `mocks.rs`: call nodes, judged by the callee and the call text up to the
//! first argument list, attributed to the innermost test whose span contains them.

use super::mocks::MockSpec;
use super::TestFn;
use tree_sitter::Node;

/// Call names a test gate counts, and how a bare entry (`sleep(`, no `.` or `::`) matches.
#[derive(Clone, Copy)]
pub struct Vocab {
    pub entries: &'static [&'static str],
    /// A bare entry names the whole callee or its last segment: `delay(` counts
    /// `delay(` and `this.delay(`, never `retry_delay(`. Qualified entries match anywhere.
    pub whole_name: bool,
}

/// A hard-coded delay in a test: the shape of a race "fixed" by waiting.
pub const SLEEP_VOCAB: Vocab = Vocab {
    entries: SLEEP_CALLS,
    whole_name: true,
};

const SLEEP_CALLS: &[&str] = &[
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
/// almost nothing. Entries match as substrings: `NotNil(` is meant to count
/// `XCTAssertNotNil(`.
pub const TRIVIAL_ASSERT_VOCAB: Vocab = Vocab {
    entries: TRIVIAL_ASSERTS,
    whole_name: false,
};

const TRIVIAL_ASSERTS: &[&str] = &[
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

/// Whether `entry` occurs in `hay` as a name of its own: not preceded by an identifier
/// character, so `delay(` is found in `this.delay(` and `delay(` but not `retry_delay(`.
fn at_name_start(hay: &str, entry: &str) -> bool {
    hay.match_indices(entry).any(|(i, _)| {
        !hay[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Whether a call with this callee and call prefix is one `vocab` names.
fn names(vocab: Vocab, callee: &str, head: &str) -> bool {
    let tail = format!("{callee}(");
    vocab.entries.iter().any(|v| {
        let bare = vocab.whole_name && !v.contains(['.', ':']);
        if bare {
            at_name_start(callee, v) || at_name_start(head, v) || at_name_start(&tail, v)
        } else {
            callee.contains(v) || head.contains(v) || tail.ends_with(v)
        }
    })
}

/// Count calls whose callee or call prefix contains a `vocab` entry, per test, into the
/// field `pick` selects. A matching chain counts once.
pub fn count(
    root: Node,
    src: &str,
    tests: &mut [TestFn],
    spec: &MockSpec,
    vocab: Vocab,
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
            // The callee's tail is judged too: a chain's head (`a.b(`) stops before the
            // `sleep(` it ends in.
            if names(vocab, callee, head) {
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

    #[test]
    fn a_bare_sleep_entry_names_the_whole_callee_or_its_last_segment() {
        use super::{names, SLEEP_VOCAB, TRIVIAL_ASSERT_VOCAB};
        // (callee, call prefix up to the first argument list)
        for (callee, head) in [
            ("retry_delay", "retry_delay("),
            ("compute_delay", "compute_delay("),
            ("nosleep", "nosleep("),
            ("self.no_usleep", "self.no_usleep("),
        ] {
            assert!(!names(SLEEP_VOCAB, callee, head), "{callee}");
        }
        for (callee, head) in [
            ("delay", "delay("),
            ("sleep", "sleep("),
            ("usleep", "usleep("),
            ("std::thread::sleep", "std::thread::sleep("),
            ("Task.Delay", "Task.Delay("),
            ("this.delay", "this.delay("),
            ("$this->sleep", "$this->sleep("),
            ("Kernel::sleep", "Kernel::sleep("),
            ("a.b(x).sleep", "a.b("),
        ] {
            assert!(names(SLEEP_VOCAB, callee, head), "{callee}");
        }
        // Trivial-assertion entries still match inside a longer name.
        assert!(names(
            TRIVIAL_ASSERT_VOCAB,
            "XCTAssertNotNil",
            "XCTAssertNotNil("
        ));
    }

    #[test]
    fn a_sleep_is_the_callee_itself_not_a_name_ending_in_it() {
        let rs = tests_of(
            "src/lib.rs",
            "#[test]\nfn a() { retry_delay(1); compute_delay(); nosleep(); no_sleep(2); }\n\
             #[test]\nfn b() { delay(10); sleep(1); std::thread::sleep(d); Self::sleep(2); }\n",
        );
        assert_eq!(rs[0].sleeps, 0, "{:?}", rs[0]);
        assert_eq!(rs[1].sleeps, 4, "{:?}", rs[1]);

        let ts = tests_of(
            "src/a.test.ts",
            "test('a', async () => {\n  retry_delay(1);\n  api.compute_delay();\n  expect(nosleep()).toEqual(3);\n});\n\
             test('b', async () => {\n  await this.delay(5);\n  await Task.Delay(5);\n  await sleep(1);\n  expect(run()).toEqual(3);\n});\n",
        );
        assert_eq!(ts[0].sleeps, 0, "{:?}", ts[0]);
        assert_eq!(ts[1].sleeps, 3, "{:?}", ts[1]);
    }
}
