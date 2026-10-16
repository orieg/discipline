//! `ci-integrity/exit-code-masked` reads a CI script's bash syntax tree (audit M7): each
//! form of masking a command's failure is reported on a GitHub workflow step and on a
//! GitLab job, whatever its spacing or spelling, and text that only mentions a mask is
//! not. Each case drives the real binary over a base and a head side.

mod common;
use common::{Repo, Run};

const WF: &str = ".github/workflows/ci.yml";
const GITLAB: &str = ".gitlab-ci.yml";

/// Forms that make `cargo test` unable to fail the step or job.
const MASKED: &[&str] = &[
    "cargo test || true",
    "cargo test ||true",
    "cargo test || :",
    "cargo test || exit 0",
    "cargo test || exit 00",
    "cargo test || echo failed",
    "cargo test || /bin/true",
    "cargo test || { true; }",
    "(cargo test || true)",
];

/// Forms that keep the failure, or only mention a mask in text.
const NOT_MASKED: &[&str] = &[
    "cargo test",
    "cargo test || exit 1",
    "cargo test || { echo failed; exit 1; }",
    "echo 'use || true to ignore it'; cargo test",
    // errexit is on in a CI script: the failure ends it before `true` runs.
    "cargo test; true",
];

fn github(run: &str) -> String {
    format!(
        "name: CI\npermissions: read-all\non: [pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Run tests\n        run: {run}\n"
    )
}

fn gitlab(line: &str) -> String {
    format!("stages: [test]\nunit-tests:\n  stage: test\n  script:\n    - {line}\n")
}

/// The run of a change that moves `path` from `base` to `head`.
fn change(path: &str, base: &str, head: &str) -> (Repo, Run) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(path, base);
    repo.commit("ci: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(path, head);
    repo.commit("ci: change");
    let run = repo.check(&[]);
    (repo, run)
}

fn codes(run: &Run) -> Vec<String> {
    run.violations("ci-integrity")
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

/// YAML quoting for a run line holding `:`, `{` or `|`.
fn quoted(cmd: &str) -> String {
    format!("\"{}\"", cmd.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn github_step_masking_forms_are_reported() {
    for cmd in MASKED {
        let (_repo, run) = change(WF, &github("cargo test"), &github(&quoted(cmd)));
        assert_eq!(
            codes(&run),
            vec!["ci-integrity/exit-code-masked"],
            "`{cmd}`: {}{}",
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn github_step_forms_that_keep_the_failure_are_not_reported() {
    for cmd in NOT_MASKED {
        let (_repo, run) = change(WF, &github("cargo test"), &github(&quoted(cmd)));
        assert!(
            codes(&run).is_empty(),
            "`{cmd}`: {:?} {}",
            codes(&run),
            run.stdout
        );
    }
}

#[test]
fn gitlab_job_masking_forms_are_reported() {
    for cmd in MASKED {
        let (_repo, run) = change(GITLAB, &gitlab("cargo test"), &gitlab(&quoted(cmd)));
        assert_eq!(
            codes(&run),
            vec!["ci-integrity/exit-code-masked"],
            "`{cmd}`: {}{}",
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn gitlab_job_forms_that_keep_the_failure_are_not_reported() {
    for cmd in NOT_MASKED {
        let (_repo, run) = change(GITLAB, &gitlab("cargo test"), &gitlab(&quoted(cmd)));
        assert!(
            codes(&run).is_empty(),
            "`{cmd}`: {:?} {}",
            codes(&run),
            run.stdout
        );
    }
}

#[test]
fn a_set_plus_e_that_is_only_mentioned_is_not_reported() {
    let block = |body: &str| {
        format!(
            "name: CI\npermissions: read-all\non: [pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Run tests\n        run: |\n{body}"
        )
    };
    let mentions = block("          # never use set +e here\n          echo 'set +e is banned'\n          cargo test\n");
    let (_repo, run) = change(WF, &github("cargo test"), &mentions);
    assert!(codes(&run).is_empty(), "{:?}", codes(&run));
    // Control: a real `set +e` with the status dropped.
    let real = block("          set +e\n          cargo test\n          echo done\n");
    let (_repo, run) = change(WF, &github("cargo test"), &real);
    assert_eq!(codes(&run), vec!["ci-integrity/exit-code-masked"]);
}

#[test]
fn the_finding_is_reported_at_the_masking_line() {
    let head = "name: CI\npermissions: read-all\non: [pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Run tests\n        run: |\n          cargo build\n          cargo test || echo failed\n";
    let (_repo, run) = change(WF, &github("cargo test"), head);
    let v = run.violations("ci-integrity");
    assert_eq!(v.len(), 1, "{}", run.stdout);
    assert_eq!(v[0]["line"], 11, "{}", run.stdout);
}

#[test]
fn a_template_expression_is_read_as_a_word() {
    let head = github(&quoted("${{ matrix.cmd }} || true"));
    let (_repo, run) = change(WF, &github("cargo test"), &head);
    assert_eq!(codes(&run), vec!["ci-integrity/exit-code-masked"]);
    assert!(
        !run.outcome("ci-integrity")["notes"]
            .to_string()
            .contains("does not parse"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_script_that_does_not_parse_is_noted() {
    let head = github(&quoted("if cargo test; then"));
    let (_repo, run) = change(WF, &github("cargo test"), &head);
    assert!(
        run.outcome("ci-integrity")["notes"]
            .to_string()
            .contains("does not parse as shell"),
        "{}",
        run.stdout
    );
    let (_repo, run) = change(
        GITLAB,
        &gitlab("cargo test"),
        &gitlab(&quoted("if cargo test; then")),
    );
    assert!(
        run.outcome("ci-integrity")["notes"]
            .to_string()
            .contains("does not parse as shell"),
        "{}",
        run.stdout
    );
}
