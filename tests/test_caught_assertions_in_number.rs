//! Many caught assertions and many expected exceptions in one test are read in work
//! that grows with their number (#672).
//!
//! A Rust test that binds the result of `catch_unwind` is judged by the uses of the
//! binding after it, and a later `let` of the same name is such a use, judged in turn by
//! the uses after it. With nothing looking at any of them, each binding asked every
//! later one again: 18 bindings of one name, in a file of 1.2 kB, cost 1.0e12
//! instructions in a build without optimisation, and each binding more doubled it, so a
//! test of 40 kept a run from returning. A Kotlin test walked its function once for each
//! expected exception, and a Python `try` read its handlers once for each assertion in
//! its body.
//!
//! The unit tests of `src/ast/caught_assertions.rs` and `src/ast/expected_exceptions.rs`
//! hold the work to the number of them, counted in steps and not in time. Here the
//! binary is given such sources in a change: it returns with its report, and the report
//! names each caught assertion at its line.
//!
//! The limit on each run is there to end a run that hangs. It measures nothing: it is
//! far above what a run takes on a loaded machine.

mod common;
use common::{discipline_cmd, Repo, Run};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Far above what a run takes; see the module text.
const TIME_LIMIT: Duration = Duration::from_secs(900);

/// `discipline check --format json --base main`, killed at [`TIME_LIMIT`]. Its output
/// goes to files in the git directory, which no gate reads. `None` for the exit code of
/// a process a signal ended.
fn check_within_limit(repo: &Repo) -> (Option<i32>, Run) {
    let out = repo.file(".git/check.out");
    let err = repo.file(".git/check.err");
    let mut cmd = discipline_cmd(repo.path());
    cmd.args(["check", "--format", "json", "--base", "main"])
        .env("PR_TITLE", "chore: test PR (#101)")
        .env("PR_BODY", "no-issue: a test change")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap());
    let mut child = cmd.spawn().unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > TIME_LIMIT {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("`discipline check` did not return");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    (
        status.code(),
        Run {
            code: status.code().unwrap_or(-1),
            stdout: std::fs::read_to_string(&out).unwrap(),
            stderr: std::fs::read_to_string(&err).unwrap(),
        },
    )
}

/// The lines of `path` at which `assertion-reduction` reported `code`.
fn lines_reported(run: &Run, code: &str, path: &str) -> Vec<u64> {
    run.violations("assertion-reduction")
        .iter()
        .filter(|v| v["code"].as_str() == Some(code) && v["file"].as_str() == Some(path))
        .map(|v| v["line"].as_u64().unwrap_or(0))
        .collect()
}

const CAUGHT: &str = "assertion-reduction/assertion-failure-caught";

/// How many of each construct a test below holds.
const NUMBER: usize = 40;

/// `line` written for each of `0..NUMBER`.
fn lines(line: impl Fn(usize) -> String) -> String {
    (0..NUMBER).map(line).collect()
}

/// A Rust test of [`NUMBER`] unwind results bound to one name with nothing looking at
/// any is checked by a run that returns, and each of the assertions is reported as
/// caught, at its line.
#[test]
fn unwind_results_bound_to_one_name_are_each_reported() {
    let repo = Repo::new();
    repo.write(
        "tests/unwind.rs",
        &format!(
            "#[test]\nfn t() {{\n{}}}\n",
            lines(|i| format!(
                "    let r = std::panic::catch_unwind(|| assert_eq!(f({i}), {i}));\n"
            ))
        ),
    );
    repo.commit("test: add a test of unwind results");
    let (exit, run) = check_within_limit(&repo);
    assert_eq!(exit, Some(1), "the run returns\n{}", run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
    assert_eq!(
        lines_reported(&run, CAUGHT, "tests/unwind.rs"),
        (3..3 + NUMBER as u64).collect::<Vec<_>>()
    );
}

/// The same test with the last result looked at: the earlier bindings are read as
/// handed on to the last, and nothing is reported as caught.
#[test]
fn unwind_results_bound_to_one_name_and_looked_at_last_are_no_finding() {
    let repo = Repo::new();
    repo.write(
        "tests/unwind.rs",
        &format!(
            "#[test]\nfn t() {{\n{}    assert!(r.is_ok());\n}}\n",
            lines(|i| format!(
                "    let r = std::panic::catch_unwind(|| assert_eq!(f({i}), {i}));\n"
            ))
        ),
    );
    repo.commit("test: add a test of unwind results");
    let (exit, run) = check_within_limit(&repo);
    assert!(exit.is_some(), "the run returns\n{}", run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
    assert_eq!(
        lines_reported(&run, CAUGHT, "tests/unwind.rs"),
        Vec::<u64>::new()
    );
}

/// A Python `try` of [`NUMBER`] assertions whose handler keeps the error in a list that
/// nothing reads: each assertion is reported as caught. With the list asserted empty
/// after the `try`, none is.
#[test]
fn assertions_of_a_python_try_that_keeps_its_error_are_reported_unless_it_is_checked() {
    let source = |checked: bool| {
        format!(
            "def test_x():\n    errs = []\n    try:\n{}    except AssertionError as e:\n{}{}{}",
            lines(|i| format!("        assert a == {i}\n")),
            lines(|_| "        errs.append(e)\n".to_string()),
            lines(|i| format!("    x{i} = {i}\n")),
            if checked { "    assert not errs\n" } else { "" }
        )
    };
    for (checked, want) in [
        (false, (4..4 + NUMBER as u64).collect::<Vec<_>>()),
        (true, Vec::new()),
    ] {
        let repo = Repo::new();
        repo.write("tests/test_kept.py", &source(checked));
        repo.commit("test: add a test that keeps its errors");
        let (exit, run) = check_within_limit(&repo);
        assert!(exit.is_some(), "the run returns\n{}", run.stderr);
        assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
        assert_eq!(
            lines_reported(&run, CAUGHT, "tests/test_kept.py"),
            want,
            "checked: {checked}"
        );
    }
}
