//! Mock usage inside test bodies, attributed to the test that contains the call.
//!
//! Two counts per test: **mock setups** (a double constructed or a return value
//! programmed: `Mock()`, `jest.fn()`, `sinon.stub`, `mock!`, `when(...)`, `.Setup(`) and
//! **mock assertions** (the test checks that the double was called, not what the code
//! produced: `assert_called_with`, `toHaveBeenCalled`, `verify(`, `.Received(`).
//!
//! One walk over the call nodes of a file; each call is attributed to the test whose line
//! span contains it. Packs pass their call-node kinds and callee field; the vocabulary is
//! shared and extended from configuration.

use super::TestFn;
use tree_sitter::Node;

pub struct MockSpec {
    /// Node kinds that are calls (`call_expression`, `call`, `macro_invocation`).
    pub call_kinds: &'static [&'static str],
    /// Field holding the callee, tried in order.
    pub callee_fields: &'static [&'static str],
}

/// Callee text fragments that construct or program a test double.
pub const SETUP_VOCAB: &[&str] = &[
    // Python
    "Mock(",
    "MagicMock(",
    "AsyncMock(",
    "mock.patch",
    "patch(",
    "patch.object",
    "mocker.patch",
    "monkeypatch.setattr",
    "create_autospec",
    // JS/TS
    "jest.fn",
    "jest.mock",
    "jest.spyOn",
    "vi.fn",
    "vi.mock",
    "vi.spyOn",
    "sinon.stub",
    "sinon.spy",
    "sinon.mock",
    "sinon.fake",
    "td.replace",
    "td.when",
    "mockResolvedValue",
    "mockReturnValue",
    "mockImplementation",
    // Rust
    "mock!",
    "MockAll",
    ".expect_",
    "mockito::mock",
    "mockito::Server",
    "wiremock",
    // Go
    "gomock.NewController",
    "NewMock",
    ".EXPECT(",
    "mock.On(",
    "httptest.NewServer",
    // Java / Kotlin
    "Mockito.mock",
    "mock(",
    "spy(",
    "when(",
    "doReturn(",
    "doThrow(",
    "doNothing(",
    "given(",
    "mockStatic(",
    "mockk(",
    "mockk<",
    "spyk(",
    "every {",
    // C#
    "new Mock<",
    ".Setup(",
    ".SetupGet(",
    ".Returns(",
    "Substitute.For",
    "A.Fake<",
    "A.CallTo(",
    // PHP / Ruby
    "createMock(",
    "getMockBuilder(",
    "prophesize(",
    "->method(",
    "->willReturn(",
    "double(",
    "instance_double(",
    "allow(",
    "stub(",
];

/// Callee text fragments that assert on the double's interactions.
pub const VERIFY_VOCAB: &[&str] = &[
    // Python
    "assert_called",
    "assert_awaited",
    "assert_not_called",
    "assert_any_call",
    "assert_has_calls",
    "call_count",
    // JS/TS
    "toHaveBeenCalled",
    "toHaveBeenCalledWith",
    "toHaveBeenCalledTimes",
    "toHaveBeenLastCalledWith",
    "toHaveBeenNthCalledWith",
    "toBeCalled",
    "toBeCalledWith",
    "calledOnce",
    "calledWith",
    "calledOnceWith",
    "td.verify",
    // Rust
    ".checkpoint(",
    // Go
    "AssertExpectations",
    "AssertCalled",
    "AssertNumberOfCalls",
    ".Finish(",
    // Java / Kotlin
    "verify(",
    "verifyNoMoreInteractions",
    "verifyNoInteractions",
    "inOrder(",
    "verify {",
    // C#
    ".Verify(",
    ".VerifyAll(",
    ".Received(",
    ".DidNotReceive(",
    "MustHaveHappened",
    // PHP / Ruby
    "->expects(",
    "shouldHaveBeenCalled",
    "have_received",
];

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Count mock setups and mock assertions per test in place.
pub fn count(
    root: Node,
    src: &str,
    tests: &mut [TestFn],
    spec: &MockSpec,
    extra_setup: &[String],
    extra_verify: &[String],
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
            // Judge the callee (for a chained matcher, `expect(f).toHaveBeenCalled`, it
            // carries the matcher) and the call text up to its first argument list
            // (`verify(`, `new Mock<T>(`, `every {`). Never the arguments: a test's own
            // body would otherwise make the enclosing `test(...)` call a match.
            let whole = text(node, src);
            let cut = whole.find(['(', '{']).map_or(whole.len(), |i| i + 1);
            let head = &whole[..cut.min(whole.len())];
            let hit = |v: &str| callee.contains(v) || head.contains(v);
            let is_verify =
                VERIFY_VOCAB.iter().any(|v| hit(v)) || extra_verify.iter().any(|v| hit(v.as_str()));
            let is_setup = !is_verify
                && (SETUP_VOCAB.iter().any(|v| hit(v))
                    || extra_setup.iter().any(|v| hit(v.as_str())));
            if is_verify || is_setup {
                let line = node.start_position().row + 1;
                if let Some(t) = tests
                    .iter_mut()
                    .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
                    .min_by_key(|t| t.end_line.saturating_sub(t.line))
                {
                    if is_verify {
                        t.mock_asserts += 1;
                    } else {
                        t.mock_setups += 1;
                    }
                }
                // `when(x).thenReturn(y)` nests a matching call; count the chain once.
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

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-java",
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
    fn python_setups_and_interaction_asserts_are_counted_per_test() {
        let t = tests_of(
            "tests/test_svc.py",
            "from unittest.mock import Mock, patch\n\n\
             def test_calls_repo():\n    repo = Mock()\n    svc(repo).run()\n    repo.save.assert_called_once_with(1)\n\n\
             def test_result():\n    assert svc(FakeRepo()).run() == 3\n",
        );
        assert_eq!((t[0].mock_setups, t[0].mock_asserts), (1, 1));
        assert_eq!((t[1].mock_setups, t[1].mock_asserts), (0, 0));
    }

    #[test]
    fn javascript_chained_matchers_and_nested_calls_count_once() {
        let t = tests_of(
            "src/a.test.ts",
            "test('calls', () => {\n  const fn = jest.fn();\n  jest.spyOn(api, 'get').mockResolvedValue(1);\n  run(fn);\n  expect(fn).toHaveBeenCalledWith(1);\n  expect(fn).toHaveBeenCalledTimes(1);\n});\n\
             test('value', () => {\n  expect(run()).toBe(2);\n});\n",
        );
        assert_eq!((t[0].mock_setups, t[0].mock_asserts), (2, 2), "{:?}", t[0]);
        assert_eq!((t[1].mock_setups, t[1].mock_asserts), (0, 0));
    }

    #[test]
    fn java_when_verify_and_rust_mockall_are_counted() {
        let j = tests_of(
            "src/test/java/ATest.java",
            "class ATest {\n  @Test void t() {\n    Repo r = mock(Repo.class);\n    when(r.find(1)).thenReturn(x);\n    svc.run(r);\n    verify(r).save(x);\n    assertEquals(1, svc.count());\n  }\n}\n",
        );
        assert_eq!((j[0].mock_setups, j[0].mock_asserts), (2, 1), "{:?}", j[0]);
        let r = tests_of(
            "src/lib.rs",
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        let mut m = MockRepo::new();\n        m.expect_find().returning(|_| 1);\n        assert_eq!(run(&m), 1);\n        m.checkpoint();\n    }\n}\n",
        );
        assert_eq!((r[0].mock_setups, r[0].mock_asserts), (1, 1), "{:?}", r[0]);
    }

    #[test]
    fn configured_vocabulary_extends_the_built_in_one() {
        let reg = default_registry();
        let vocab = AssertVocabulary {
            mock_setup_fns: vec!["fake_clock".into()],
            mock_assert_fns: vec!["assert_logged".into()],
            ..Default::default()
        };
        let t = reg
            .find_pack("tests/test_a.py")
            .unwrap()
            .extract(
                "tests/test_a.py",
                "def test_a():\n    c = fake_clock()\n    run(c)\n    assert_logged(c, 'x')\n",
                &vocab,
            )
            .unwrap()
            .tests;
        assert_eq!((t[0].mock_setups, t[0].mock_asserts), (1, 1));
    }
}
