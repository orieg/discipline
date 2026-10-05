//! `assertion-reduction/test-cases-reduced` over the forms #532 lists: a Kotlin
//! annotation that was never counted, and a counted base whose head side has no literal
//! count (the case source became an expression, or the parametrization was removed and
//! one case hard-coded). Every test drives the real binary.

mod common;
use common::{Repo, Run};

const TITLE: &str = "Test Cases Reduced In Parametrized Test";
const CODE: &str = "assertion-reduction/test-cases-reduced";

/// Commits `base` at `path` on the base side, writes `head` over it, and checks.
fn check_change(path: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(&[(path, base)], "test: base cases");
    repo.write(path, head);
    repo.commit("test: change cases");
    repo.check(&[])
}

/// The one case finding of a run, with exit code 1; its message.
fn case_finding(run: &Run) -> String {
    let titles = run.titles("assertion-reduction");
    assert_eq!(
        titles,
        vec![TITLE.to_string()],
        "expected the case finding alone: {}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let v = &run.violations("assertion-reduction")[0];
    assert_eq!(v["code"], CODE);
    assert!(
        v["remediation"]
            .as_str()
            .unwrap()
            .contains("allow-case-drop: "),
        "{v}"
    );
    v["message"].as_str().unwrap().to_string()
}

fn notes(run: &Run) -> String {
    run.outcome("assertion-reduction")["notes"].to_string()
}

const PY_THREE: &str =
    "import pytest\n\n@pytest.mark.parametrize(\"x\", [1, 2, 3])\ndef test_x(x):\n    assert x > 0\n";
const PY_CALL: &str =
    "import pytest\n\n@pytest.mark.parametrize(\"x\", cases())\ndef test_x(x):\n    assert x > 0\n";

fn kotlin_value_source(values: &str) -> String {
    format!(
        "import org.junit.jupiter.params.ParameterizedTest\nimport org.junit.jupiter.params.provider.ValueSource\n\nclass ATest {{\n    @ParameterizedTest\n    @ValueSource(strings = [{values}])\n    fun t(s: String) {{\n        assertEquals(1, s.length)\n    }}\n}}\n"
    )
}

#[test]
fn kotlin_value_source_rows_removed_is_a_case_reduction() {
    let run = check_change(
        "src/test/kotlin/ATest.kt",
        &kotlin_value_source("\"a\", \"b\", \"c\""),
        &kotlin_value_source("\"a\""),
    );
    let message = case_finding(&run);
    assert!(message.contains("dropped from 3 to 1"), "{message}");
}

#[test]
fn a_literal_list_replaced_by_a_call_is_a_case_reduction() {
    let run = check_change("tests/test_calc.py", PY_THREE, PY_CALL);
    let message = case_finding(&run);
    assert!(
        message.contains("no longer a literal list") && message.contains("3 literal cases"),
        "{message}"
    );
    // The call's text is the change's own and is not echoed.
    assert!(!message.contains("cases()"), "{message}");
}

#[test]
fn a_removed_python_parametrization_with_one_case_hard_coded_is_a_case_reduction() {
    let run = check_change(
        "tests/test_calc.py",
        PY_THREE,
        "import pytest\n\ndef test_x():\n    x = 1\n    assert x > 0\n",
    );
    let message = case_finding(&run);
    assert!(
        message.contains("ran 3 cases") && message.contains("no longer read as parametrized"),
        "{message}"
    );
}

#[test]
fn a_removed_csharp_inline_data_set_is_a_case_reduction() {
    let run = check_change(
        "tests/CalcTests.cs",
        "using Xunit;\n\npublic class CalcTests\n{\n    [Theory]\n    [InlineData(1)]\n    [InlineData(2)]\n    [InlineData(3)]\n    public void Positive(int x)\n    {\n        Assert.True(x > 0);\n    }\n}\n",
        "using Xunit;\n\npublic class CalcTests\n{\n    [Fact]\n    public void Positive()\n    {\n        var x = 1;\n        Assert.True(x > 0);\n    }\n}\n",
    );
    let message = case_finding(&run);
    assert!(
        message.contains("ran 3 cases") && message.contains("no longer read as parametrized"),
        "{message}"
    );
}

#[test]
fn a_removed_javascript_each_table_is_a_case_reduction() {
    let run = check_change(
        "tests/calc.test.js",
        "test.each([1, 2, 3])(\"t\", (x) => {\n  expect(x > 0).toBe(true);\n});\n",
        "test(\"t\", () => {\n  const x = 1;\n  expect(x > 0).toBe(true);\n});\n",
    );
    let message = case_finding(&run);
    assert!(
        message.contains("ran 3 cases") && message.contains("no longer read as parametrized"),
        "{message}"
    );
}

/// Control: cases added to a literal list.
#[test]
fn control_cases_added_to_a_literal_list_pass() {
    let run = check_change(
        "tests/test_calc.py",
        PY_THREE,
        "import pytest\n\n@pytest.mark.parametrize(\"x\", [1, 2, 3, 4])\ndef test_x(x):\n    assert x > 0\n",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.titles("assertion-reduction").is_empty());
}

/// Control: nothing was counted on the base side, so nothing is known to have dropped.
#[test]
fn control_a_source_that_was_never_a_literal_leaves_the_note_only() {
    let run = check_change(
        "tests/test_calc.py",
        PY_CALL,
        "import pytest\n\n@pytest.mark.parametrize(\"x\", other_cases())\ndef test_x(x):\n    assert x > 0\n",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.titles("assertion-reduction").is_empty());
    assert!(
        notes(&run).contains("non-literal test case source in `test_x`"),
        "{}",
        run.stdout
    );
}

/// Control: a call replaced by a literal list gains a count; nothing dropped.
#[test]
fn control_a_call_replaced_by_a_literal_list_passes() {
    let run = check_change("tests/test_calc.py", PY_CALL, PY_THREE);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.titles("assertion-reduction").is_empty());
}

/// Control: one literal case is one run with or without the parametrization.
#[test]
fn control_a_single_case_base_losing_its_parametrization_passes() {
    let run = check_change(
        "tests/test_calc.py",
        "import pytest\n\n@pytest.mark.parametrize(\"x\", [1])\ndef test_x(x):\n    assert x > 0\n",
        "import pytest\n\ndef test_x():\n    x = 1\n    assert x > 0\n",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.titles("assertion-reduction").is_empty());
}

#[test]
fn the_uncounted_head_finding_is_lifted_by_allow_case_drop() {
    let repo = Repo::new();
    repo.commit_base_files(&[("tests/test_calc.py", PY_THREE)], "test: base cases");
    repo.write("tests/test_calc.py", PY_CALL);
    repo.commit("test: change cases");

    let unlifted = repo.check_with_pr(&[], "Fixes #450\n");
    assert_eq!(unlifted.titles("assertion-reduction"), vec![TITLE]);

    let lifted = repo.check_with_pr(
        &[],
        "Fixes #450\n\nallow-case-drop: test_x the cases moved to a shared provider\n",
    );
    assert_eq!(lifted.code, 0, "{}{}", lifted.stdout, lifted.stderr);
    assert!(lifted.titles("assertion-reduction").is_empty());
    let overrides = lifted.outcome("assertion-reduction")["overrides"].clone();
    assert_eq!(overrides.as_array().unwrap().len(), 1, "{overrides}");
}
