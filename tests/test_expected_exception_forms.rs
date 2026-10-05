//! Expected-exception forms read through the real binary (#527): the `as` form of
//! `pytest.raises` is read like the plain form, and a C# call named `Throws` counts as
//! an expected exception only when it is an assertion.

mod common;
use common::{Repo, Run};

const WIDENED: &str = "Expected Exception Or Panic Widened";
const VACUOUS: &str = "Vacuous Test Added";

fn py_test(with_line: &str) -> String {
    format!("import pytest\n\n\ndef test_neg():\n    {with_line}\n        f(-1)\n")
}

/// Commits `base` on the base side and `head` on the work branch, then checks.
fn check_python_change(base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base("tests/test_r.py", &py_test(base), "test: base");
    repo.write("tests/test_r.py", &py_test(head));
    repo.commit("test: change the expected exception");
    repo.check(&[])
}

fn cs_test(body: &str) -> String {
    format!(
        "using Xunit;\npublic class SutTests {{\n    [Fact]\n    public void T() {{\n        {body}\n    }}\n}}\n"
    )
}

/// Adds one new xUnit test with `body` and checks.
fn check_new_csharp_test(body: &str) -> Run {
    let repo = Repo::new();
    repo.write("tests/SutTests.cs", &cs_test(body));
    repo.commit("test: add a csharp test");
    repo.check(&[])
}

#[test]
fn pytest_raises_as_form_widened_is_reported() {
    let run = check_python_change(
        "with pytest.raises(ValueError, match=\"neg\") as e:",
        "with pytest.raises(Exception) as e:",
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.titles("assertion-reduction"),
        vec![WIDENED],
        "{}",
        run.stdout
    );
    let violations = run.violations("assertion-reduction");
    assert_eq!(
        violations[0]["code"],
        "assertion-reduction/expected-exception-widened"
    );
    assert!(violations[0]["message"]
        .as_str()
        .unwrap_or("")
        .contains("test_neg"));
}

/// Control: the plain form was already read.
#[test]
fn pytest_raises_plain_form_widened_is_reported() {
    let run = check_python_change(
        "with pytest.raises(ValueError, match=\"neg\"):",
        "with pytest.raises(Exception):",
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.titles("assertion-reduction"),
        vec![WIDENED],
        "{}",
        run.stdout
    );
}

/// Control: an `as` form whose expectation is unchanged is silent when the body changes.
#[test]
fn pytest_raises_as_form_unchanged_is_silent() {
    let repo = Repo::new();
    let with_line = "with pytest.raises(ValueError, match=\"neg\") as e:";
    repo.commit_base("tests/test_r.py", &py_test(with_line), "test: base");
    repo.write(
        "tests/test_r.py",
        &format!("{}\n\ndef helper():\n    return 1\n", py_test(with_line)),
    );
    repo.commit("test: add a helper beside the test");
    let run = repo.check(&[]);
    assert!(
        run.titles("assertion-reduction").is_empty(),
        "{}",
        run.stdout
    );
}

#[test]
fn unittest_assert_raises_as_form_widened_is_reported_once() {
    let src = |call: &str| {
        format!(
            "import unittest\n\n\nclass TestNeg(unittest.TestCase):\n    def test_neg(self):\n        with {call} as cm:\n            f(-1)\n"
        )
    };
    let repo = Repo::new();
    repo.commit_base(
        "tests/test_u.py",
        &src("self.assertRaisesRegex(ValueError, \"neg\")"),
        "test: base",
    );
    repo.write("tests/test_u.py", &src("self.assertRaises(Exception)"));
    repo.commit("test: widen");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.titles("assertion-reduction"),
        vec![WIDENED],
        "{}",
        run.stdout
    );
}

#[test]
fn csharp_mock_only_throws_test_is_vacuous() {
    let run = check_new_csharp_test("mock.Setup(m => m.Get()).Throws(new Exception()); sut.Run();");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.titles("vacuous-tests"), vec![VACUOUS], "{}", run.stdout);
}

/// Control: the same test with `.Returns(1)` was already vacuous.
#[test]
fn csharp_mock_only_returns_test_is_vacuous() {
    let run = check_new_csharp_test("mock.Setup(m => m.Get()).Returns(1); sut.Run();");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.titles("vacuous-tests"), vec![VACUOUS], "{}", run.stdout);
}

/// Control: a real expected-exception assertion still counts.
#[test]
fn csharp_assert_throws_test_is_not_vacuous() {
    let run = check_new_csharp_test("Assert.Throws<InvalidOperationException>(() => sut.Run());");
    assert!(run.titles("vacuous-tests").is_empty(), "{}", run.stdout);
}

#[test]
fn csharp_assert_throws_type_argument_widened_is_reported() {
    let repo = Repo::new();
    repo.commit_base(
        "tests/SutTests.cs",
        &cs_test("Assert.Throws<ArgumentNullException>(() => sut.Run());"),
        "test: base",
    );
    repo.write(
        "tests/SutTests.cs",
        &cs_test("Assert.Throws<Exception>(() => sut.Run());"),
    );
    repo.commit("test: widen");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.titles("assertion-reduction"),
        vec![WIDENED],
        "{}",
        run.stdout
    );
}
