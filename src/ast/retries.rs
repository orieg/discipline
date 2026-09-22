//! Retry and flaky annotations: a test made green by running it again.
//!
//! `@pytest.mark.flaky(reruns=3)`, `jest.retryTimes(3)`, `this.retries(2)`, `@RetryingTest`,
//! `[Retry(3)]`, `flaky_test::flaky_test`, RSpec `retry: 3`. The marker is read from
//! decorator, attribute, annotation and call nodes and attributed to the test it decorates
//! (the next test definition) or contains it; a file-level marker applies to every test in
//! the file.

use super::TestFn;
use tree_sitter::Node;

pub struct RetrySpec {
    /// Node kinds a marker may be: decorators, attributes, annotations, calls.
    pub marker_kinds: &'static [&'static str],
}

/// Text fragments of a per-test marker.
const TEST_VOCAB: &[&str] = &[
    "pytest.mark.flaky",
    "mark.flaky",
    "@flaky",
    "flaky(",
    "@retry(",
    "@retry\n",
    "pytest.mark.rerun",
    "this.retries(",
    "retry:",
    "{ retry:",
    "retries:",
    "RetryingTest",
    "@Retry",
    "@Flaky",
    "RepeatedIfExceptionsTest",
    "[Retry",
    "[Flaky",
    "[RetryOnFailure",
    "flaky_test",
    "#[retry",
    ":retry =>",
];

/// Text fragments of a file- or suite-level marker.
const FILE_VOCAB: &[&str] = &[
    "jest.retryTimes(",
    "retryTimes(",
    "vi.retry(",
    "test.retry(",
];

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Fill `TestFn::retries` in place.
pub fn mark(root: Node, src: &str, tests: &mut [TestFn], spec: &RetrySpec) {
    if tests.is_empty() {
        return;
    }
    let mut file_level: Option<String> = None;
    let mut per_test: Vec<(usize, usize, String)> = Vec::new(); // (start line, end line, marker)
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if spec.marker_kinds.contains(&node.kind()) {
            let t = text(node, src);
            let head: String = t.lines().next().unwrap_or("").chars().take(120).collect();
            if FILE_VOCAB.iter().any(|v| head.contains(v)) {
                file_level.get_or_insert(head.trim().to_string());
            } else if TEST_VOCAB.iter().any(|v| head.contains(v)) {
                per_test.push((
                    node.start_position().row + 1,
                    node.end_position().row + 1,
                    head.trim().to_string(),
                ));
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    // A marker inside a test's span (an options object, `this.retries`) belongs to that
    // test; any other marker decorates the test whose definition starts within four
    // lines below it.
    let contained = |start: usize| {
        tests
            .iter()
            .any(|t| t.line <= start && start <= t.end_line.max(t.line))
    };
    let (inside, above): (Vec<_>, Vec<_>) =
        per_test.iter().partition(|(start, _, _)| contained(*start));
    for t in tests.iter_mut() {
        if let Some(m) = &file_level {
            t.retries = Some(m.clone());
            continue;
        }
        let found = inside
            .iter()
            .find(|(start, _, _)| t.line <= *start && *start <= t.end_line.max(t.line))
            .or_else(|| {
                above
                    .iter()
                    .find(|(_, end, _)| *end < t.line && t.line - *end <= 4)
            });
        if let Some((_, _, m)) = found {
            t.retries = Some(m.clone());
        }
    }
}

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-java"
))]
mod tests {
    use crate::ast::{default_registry, AssertVocabulary};

    fn retries(path: &str, src: &str) -> Vec<(String, Option<String>)> {
        let reg = default_registry();
        reg.find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
            .into_iter()
            .map(|t| (t.name, t.retries))
            .collect()
    }

    #[test]
    fn python_decorators_attach_to_the_test_below_only() {
        let got = retries(
            "tests/test_a.py",
            "import pytest\n\n@pytest.mark.flaky(reruns=3)\ndef test_a():\n    assert 1 == 1\n\n\ndef test_b():\n    assert 2 == 2\n\n@flaky\n@pytest.mark.slow\ndef test_c():\n    assert 3 == 3\n",
        );
        assert_eq!(
            got[0],
            ("test_a".into(), Some("@pytest.mark.flaky(reruns=3)".into()))
        );
        assert_eq!(got[1].1, None);
        assert_eq!(got[2].1.as_deref(), Some("@flaky"));
    }

    #[test]
    fn javascript_file_level_and_in_body_retries() {
        let file = retries(
            "src/a.test.ts",
            "jest.retryTimes(3);\ntest('a', () => { expect(1).toBe(1); });\ntest('b', () => { expect(2).toBe(2); });\n",
        );
        assert!(
            file.iter()
                .all(|(_, r)| r.as_deref() == Some("jest.retryTimes(3)")),
            "{file:?}"
        );
        let body = retries(
            "src/b.test.ts",
            "it('a', function () { this.retries(2); expect(1).toBe(1); });\nit('b', () => { expect(2).toBe(2); });\n",
        );
        assert!(body[0].1.is_some());
        assert_eq!(body[1].1, None);
    }

    #[test]
    fn java_retrying_test_annotation() {
        let got = retries(
            "src/test/java/ATest.java",
            "class ATest {\n  @Test\n  @RetryingTest(3)\n  void a() { assertEquals(1, 1); }\n  @Test\n  void b() { assertEquals(2, 2); }\n}\n",
        );
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].1.as_deref(), Some("@RetryingTest(3)"));
        assert_eq!(got[1].1, None);
    }
}
