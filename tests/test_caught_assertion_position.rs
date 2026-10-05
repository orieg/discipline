//! `assertion-reduction/assertion-failure-caught` reports an assertion that a change put
//! behind a swallowing handler. Whether a swallowed assertion is new is decided by its
//! position inside the paired test, never by its line in the file (#526): a file line
//! moves whenever anything above the test is edited.

mod common;
use common::{Repo, Run};

const GATE: &str = "assertion-reduction";
const CAUGHT: &str = "Assertion Failure Caught Inside Test";
const PATH: &str = "tests/test_t.py";

/// A test whose only assertion was already swallowed on the base side.
const SWALLOWED: &str =
    "def test_a():\n    try:\n        assert f() == 1\n    except AssertionError:\n        pass\n";

fn check(base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base(PATH, base, "test: base");
    repo.write(PATH, head);
    repo.commit("test: change");
    repo.check(&[])
}

fn assert_silent(run: &Run) {
    assert_eq!(
        run.titles(GATE),
        Vec::<String>::new(),
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// The lines of the swallowed assertions reported, in report order.
fn caught_lines(run: &Run) -> Vec<u64> {
    let violations = run.violations(GATE);
    assert!(
        violations.iter().all(|v| v["title"] == CAUGHT),
        "{violations:?}"
    );
    violations
        .iter()
        .map(|v| v["line"].as_u64().expect("a caught assertion has a line"))
        .collect()
}

#[test]
fn lines_added_above_an_untouched_swallowed_assertion_report_nothing() {
    let run = check(SWALLOWED, &format!("import os\n\n\n{SWALLOWED}"));
    assert_silent(&run);
}

#[test]
fn a_test_moved_lower_in_its_file_unchanged_reports_nothing() {
    let other = "def test_b():\n    assert g() == 2\n";
    let run = check(
        &format!("{SWALLOWED}\n\n{other}"),
        &format!("{other}\n\n{SWALLOWED}"),
    );
    assert_silent(&run);
}

#[test]
fn a_handler_wrapped_around_an_existing_assertion_is_reported_on_its_head_line() {
    // The head also gains lines above the test, so the reported line is the head file line
    // (6), which is neither the base line (2) nor an offset inside the test (2).
    let run = check(
        "def test_a():\n    assert f() == 1\n",
        &format!("import os\n\n\n{SWALLOWED}"),
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(caught_lines(&run), vec![6], "{}", run.stdout);
}

#[test]
fn a_second_swallowed_assertion_is_reported_once_and_the_first_is_not() {
    // The base assertion on line 6 is effective; the head swallows it. The swallowed
    // assertion that was already there moves from file line 3 to 6 and is not reported.
    let base = format!("{SWALLOWED}    assert g() == 2\n");
    let head = format!(
        "import os\n\n\n{SWALLOWED}    try:\n        assert g() == 2\n    except AssertionError:\n        pass\n"
    );
    let run = check(&base, &head);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(caught_lines(&run), vec![10], "{}", run.stdout);
}

#[test]
fn a_line_added_inside_the_test_above_its_swallowed_assertion_reports_nothing() {
    // The swallowed assertion moves within the test, but the test swallows no more
    // assertions than it did, so none of them is new.
    let head = "def test_a():\n    x = f()\n    try:\n        assert x == 1\n    except AssertionError:\n        pass\n";
    let run = check(SWALLOWED, head);
    assert_silent(&run);
}
