//! A job's `if:` is read to a fixed nesting and length by `ci-skip-set` (#667). The
//! rule's own reader of the expression calls itself once for each `!`, parenthesis and
//! call argument, and its evaluator once for each `&&` and `||`, and the text is the
//! workflow's: ten thousand `!` in a changed workflow ended the process with a stack
//! overflow and no report. Past 64 levels or 256 operators the expression is one the
//! rule cannot read, and is reported as any other such expression is.

mod common;
use common::*;

const CONFIG: &str = r#"[meta]
version = 1
name = "skip-set-bounds"

[gates.ci-skip-set]
unconditional_jobs = ["docs-lint"]
"#;

const CONTEXT: &str = r#"{
  "docs-lint": {"result": "success", "outputs": {}},
  "lint": {"result": "skipped", "outputs": {}}
}"#;

const ON_A_PULL_REQUEST: &str = "github.event_name == 'pull_request'";

fn workflow(condition: &str) -> String {
    format!(
        r#"name: CI
on: pull_request
permissions: read-all
jobs:
  docs-lint:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
  lint:
    if: {condition}
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
  ci-gate:
    if: always()
    needs: [docs-lint, lint]
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
"#
    )
}

/// The titles `ci-skip-set` reports for a change that sets `lint`'s condition, in a
/// rollup where `lint` was skipped on a pull request, and how the process ended.
fn titles_for(condition: &str) -> (Option<i32>, Vec<String>, String) {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (
                ".github/workflows/ci.yml",
                &workflow("github.event_name != 'pull_request'"),
            ),
            ("discipline.toml", CONFIG),
        ],
        "ci: add workflow",
    );
    repo.write(".github/workflows/ci.yml", &workflow(condition));
    repo.commit("ci: change a condition");
    let mut cmd = discipline_cmd(repo.path());
    let out = cmd
        .args([
            "check",
            "--format",
            "json",
            "--base",
            "main",
            "--suite",
            "integrity",
        ])
        .env("GITHUB_EVENT_NAME", "pull_request")
        .env("DISCIPLINE_CI_CONTEXT", CONTEXT)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let Some(code) = out.status.code() else {
        return (None, Vec::new(), stderr);
    };
    let run = Run {
        code,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: stderr.clone(),
    };
    (Some(code), run.titles("ci-skip-set"), stderr)
}

const UNRELATED: &str = "Change-Detection Job Missing From Needs";
const SKIPPED_WHILE_TRUE: &str = "Job Skipped While Condition True";
const UNVERIFIABLE: &str = "Skip Decision Unverifiable";

/// The condition is read, and the skipped job named, at 64 levels of `!` and
/// parentheses; one level further the rule says it cannot verify the skip.
#[test]
fn a_condition_at_the_nesting_bound_is_read_and_one_level_deeper_is_unverifiable() {
    // An even number of `!` leaves the condition true on a pull request: the job
    // should have run and was skipped.
    let nots = |n: usize| format!("${{{{ {}({ON_A_PULL_REQUEST}) }}}}", "!".repeat(n));
    let (exit, titles, stderr) = titles_for(&nots(62));
    assert_eq!(exit, Some(1), "{stderr}");
    assert_eq!(titles, [UNRELATED, SKIPPED_WHILE_TRUE]);
    // 63 `!` and the parentheses are 64 levels: read, and false on a pull request.
    let (exit, titles, stderr) = titles_for(&nots(63));
    assert_eq!(exit, Some(1), "{stderr}");
    assert_eq!(titles, [UNRELATED]);
    // 64 `!` and the parentheses are 65: true on a pull request, and not read.
    let (exit, titles, stderr) = titles_for(&nots(64));
    assert_eq!(exit, Some(1), "{stderr}");
    assert_eq!(titles, [UNRELATED, UNVERIFIABLE]);
}

/// Nesting that ended the process: the run exits with its report, and the condition is
/// one the rule cannot verify.
#[test]
fn a_condition_nested_ten_thousand_deep_is_unverifiable_and_the_process_exits() {
    let nots = format!("${{{{ {}({ON_A_PULL_REQUEST}) }}}}", "!".repeat(10_000));
    let parens = format!(
        "{}{ON_A_PULL_REQUEST}{}",
        "(".repeat(10_000),
        ")".repeat(10_000)
    );
    for condition in [nots, parens] {
        let (exit, titles, stderr) = titles_for(&condition);
        assert_eq!(exit, Some(1), "the process must exit: {stderr}");
        assert!(!stderr.contains("overflowed its stack"), "{stderr}");
        assert_eq!(titles, [UNRELATED, UNVERIFIABLE]);
    }
}

/// A chain is read to 256 `||`; one more, or a hundred thousand, is unverifiable.
#[test]
fn a_chain_at_the_operator_bound_is_read_and_a_longer_one_is_unverifiable() {
    let ors = |n: usize| {
        format!(
            "{ON_A_PULL_REQUEST}{}",
            format!(" || {ON_A_PULL_REQUEST}").repeat(n)
        )
    };
    let (exit, titles, stderr) = titles_for(&ors(256));
    assert_eq!(exit, Some(1), "{stderr}");
    assert_eq!(titles, [UNRELATED, SKIPPED_WHILE_TRUE]);
    for n in [257, 100_000] {
        let (exit, titles, stderr) = titles_for(&ors(n));
        assert_eq!(exit, Some(1), "{n}: the process must exit: {stderr}");
        assert_eq!(titles, [UNRELATED, UNVERIFIABLE], "{n}");
    }
}
