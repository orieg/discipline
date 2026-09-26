//! End-to-end: the `ci-skip-set` rule driven through the real binary with a
//! rollup's runtime `needs` context in `DISCIPLINE_CI_CONTEXT`.

mod common;
use common::*;

const WORKFLOW: &str = r#"name: CI
on: pull_request
permissions: read-all
jobs:
  detect-changes:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    outputs:
      tooling: ${{ steps.filter.outputs.tooling }}
      rust-src: ${{ steps.filter.outputs.rust-src }}
    steps:
      - run: "true"
  docs-lint:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
  lint:
    needs: detect-changes
    if: needs.detect-changes.outputs.tooling == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
  miri:
    needs: detect-changes
    if: needs.detect-changes.outputs.rust-src == 'true' || github.event_name != 'pull_request'
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
  ci-gate:
    if: always()
    needs: [detect-changes, docs-lint, lint, miri]
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - run: "true"
"#;

const CONFIG: &str = r#"[meta]
version = 1
name = "skip-set-e2e"

[gates.ci-skip-set]
unconditional_jobs = ["docs-lint"]
"#;

fn repo() -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".github/workflows/ci.yml", WORKFLOW),
            ("discipline.toml", CONFIG),
        ],
        "ci: add workflow",
    );
    repo
}

fn needs(lint: &str, miri: &str, tooling: &str, rust_src: &str) -> String {
    format!(
        r#"{{
  "detect-changes": {{"result": "success", "outputs": {{"tooling": "{tooling}", "rust-src": "{rust_src}"}}}},
  "docs-lint": {{"result": "success", "outputs": {{}}}},
  "lint": {{"result": "{lint}", "outputs": {{}}}},
  "miri": {{"result": "{miri}", "outputs": {{}}}}
}}"#
    )
}

fn check(repo: &Repo, context: Option<&str>, event: &str) -> Run {
    let mut env = vec![("GITHUB_EVENT_NAME", event)];
    if let Some(c) = context {
        env.push(("DISCIPLINE_CI_CONTEXT", c));
    }
    repo.run(
        &[
            "check",
            "--format",
            "json",
            "--base",
            "main",
            "--suite",
            "integrity",
        ],
        &env,
    )
}

#[test]
fn without_context_the_rule_is_named_not_evaluated_and_never_fails() {
    let repo = repo();
    let run = check(&repo, None, "pull_request");
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let o = run.outcome("ci-skip-set");
    assert!(o["enabled"].as_bool().unwrap());
    assert_eq!(o["examined"], 0);
    assert!(run.violations("ci-skip-set").is_empty());
    let notes = o["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .unwrap()
            .starts_with("not evaluated: DISCIPLINE_CI_CONTEXT")),
        "{notes:?}"
    );
}

#[test]
fn consistent_skip_set_passes_from_inline_json() {
    let repo = repo();
    let ctx = needs("success", "skipped", "true", "false");
    let run = check(&repo, Some(&ctx), "pull_request");
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.outcome("ci-skip-set")["examined"], 4);
    assert!(run.violations("ci-skip-set").is_empty());
}

#[test]
fn job_skipped_under_a_true_gate_fails_from_context_file() {
    let repo = repo();
    // Written outside the tree so the diff under check stays empty.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("needs.json");
    std::fs::write(&path, needs("skipped", "skipped", "true", "false")).unwrap();
    let run = check(&repo, Some(path.to_str().unwrap()), "pull_request");
    assert_eq!(run.code, 1, "{}\n{}", run.stdout, run.stderr);
    let v = run.violations("ci-skip-set");
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0]["title"], "Job Skipped While Condition True");
    assert_eq!(v[0]["code"], "ci-skip-set/job-skipped-while-condition-true");
    assert!(
        v[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("`lint` was skipped"),
        "the job is named in the message: {}",
        v[0]["message"]
    );
    assert_eq!(v[0]["file"], ".github/workflows/ci.yml");
    assert_eq!(v[0]["line"], 18, "the `lint:` key");
}

#[test]
fn push_event_makes_the_fallback_term_true() {
    let repo = repo();
    let ctx = needs("success", "skipped", "true", "false");
    let pr = check(&repo, Some(&ctx), "pull_request");
    assert_eq!(pr.code, 0, "{}", pr.stdout);
    let push = check(&repo, Some(&ctx), "push");
    assert_eq!(push.code, 1, "{}", push.stdout);
    assert_eq!(
        push.titles("ci-skip-set"),
        vec!["Job Skipped While Condition True".to_string()]
    );
}

#[test]
fn failed_change_detection_and_skipped_unconditional_job_fail() {
    let repo = repo();
    let ctx = needs("skipped", "skipped", "false", "false")
        .replacen(r#""result": "success""#, r#""result": "failure""#, 1)
        .replace(
            r#""docs-lint": {"result": "success""#,
            r#""docs-lint": {"result": "skipped""#,
        );
    let run = check(&repo, Some(&ctx), "pull_request");
    assert_eq!(run.code, 1, "{}", run.stdout);
    let titles = run.titles("ci-skip-set");
    assert!(
        titles.contains(&"Change-Detection Job Did Not Succeed".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"Unconditional Job Skipped".to_string()),
        "{titles:?}"
    );
}

#[test]
fn unreadable_context_fails_closed_with_exit_2() {
    let repo = repo();
    let bad_json = check(&repo, Some("{not json"), "pull_request");
    assert_eq!(bad_json.code, 2, "{}\n{}", bad_json.stdout, bad_json.stderr);
    assert!(
        bad_json.stderr.contains("ci-skip-set"),
        "{}",
        bad_json.stderr
    );
    assert_eq!(
        bad_json.could_not_check(),
        ("gate".to_string(), Some("ci-skip-set".to_string()))
    );
    let missing = check(&repo, Some("does/not/exist.json"), "pull_request");
    assert_eq!(missing.code, 2, "{}\n{}", missing.stdout, missing.stderr);
    assert!(
        missing.stderr.contains("could not be read"),
        "{}",
        missing.stderr
    );
    // A context file that is not there is the run's input, not the gate's fault.
    assert_eq!(
        missing.could_not_check(),
        ("configuration".to_string(), Some("ci-skip-set".to_string()))
    );
}
