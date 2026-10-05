//! A CI event payload variable that is set but names an unusable file stops the run
//! (exit 2). Unset, empty, or naming a readable JSON file: the run proceeds as before.

mod common;
use common::Repo;

const CHECK: [&str; 5] = ["check", "--base", "main", "--format", "json"];

/// A repository whose branch carries one clean change, and a directory outside it for
/// the payload files.
fn fixture() -> (Repo, tempfile::TempDir) {
    let repo = Repo::new();
    repo.write("docs/notes.md", "# Notes\n\nPhase 1 then Phase 2.\n");
    repo.commit("docs: add notes");
    (repo, tempfile::tempdir().unwrap())
}

fn payload(dir: &tempfile::TempDir, name: &str, content: &str) -> String {
    let p = dir.path().join(name);
    std::fs::write(&p, content).unwrap();
    p.to_str().unwrap().to_string()
}

fn detail(run: &common::Run) -> String {
    run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap()
        .to_string()
}

const PULL_WITH_ESTIMATE: &str = r#"{"pull_request": {"number": 7, "title": "docs: add notes (#7)", "body": "Plan: ship in 3 weeks."}}"#;

#[test]
fn a_missing_first_payload_file_stops_the_run_and_names_its_variable() {
    let (repo, dir) = fixture();
    let github = payload(&dir, "github.json", PULL_WITH_ESTIMATE);
    let missing = dir.path().join("absent").join("event.json");
    let run = repo.run(
        &CHECK,
        &[
            ("FORGEJO_EVENT_PATH", missing.to_str().unwrap()),
            ("GITHUB_EVENT_PATH", github.as_str()),
        ],
    );
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("configuration".to_string(), None));
    let detail = detail(&run);
    assert!(detail.contains("FORGEJO_EVENT_PATH"), "{detail}");
    assert!(detail.contains("cannot be read"), "{detail}");
    assert!(!detail.contains("GITHUB_EVENT_PATH"), "{detail}");
    assert!(run.stderr.contains("FORGEJO_EVENT_PATH"), "{}", run.stderr);
}

#[test]
fn a_payload_file_that_is_not_json_stops_the_run_without_echoing_it() {
    let (repo, dir) = fixture();
    let sentinel = "SENTINEL-7f3a-not-for-output";
    let bad = payload(&dir, "bad.json", &format!("{{\"token\": {sentinel}"));
    let run = repo.run(&CHECK, &[("GITEA_EVENT_PATH", bad.as_str())]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("configuration".to_string(), None));
    let detail = detail(&run);
    assert!(detail.contains("GITEA_EVENT_PATH"), "{detail}");
    assert!(detail.contains("not valid JSON"), "{detail}");
    assert!(!run.stdout.contains(sentinel), "{}", run.stdout);
    assert!(!run.stderr.contains(sentinel), "{}", run.stderr);
}

/// The push base is read from the payload too: a run with no `--base` stops the same way
/// instead of falling back to `HEAD~1`.
#[test]
fn an_unusable_payload_stops_a_run_that_would_detect_its_base() {
    let (repo, dir) = fixture();
    let missing = dir.path().join("event.json");
    let run = repo.run(
        &["check", "--format", "json"],
        &[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_EVENT_PATH", missing.to_str().unwrap()),
        ],
    );
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(detail(&run).contains("GITHUB_EVENT_PATH"), "{}", run.stdout);
}

#[test]
fn baseline_stops_on_an_unusable_payload() {
    let (repo, dir) = fixture();
    let missing = dir.path().join("event.json");
    let run = repo.run(
        &["baseline", "--base", "main"],
        &[("GITHUB_EVENT_PATH", missing.to_str().unwrap())],
    );
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("GITHUB_EVENT_PATH"), "{}", run.stderr);
    assert!(!repo.file("discipline-baseline.toml").exists());
}

// Controls: the run proceeds.

#[test]
fn control_no_payload_variable_proceeds() {
    let (repo, _dir) = fixture();
    let run = repo.run(&CHECK, &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.json()["could_not_check"].is_null());
}

#[test]
fn control_an_empty_payload_variable_proceeds() {
    let (repo, _dir) = fixture();
    let run = repo.run(
        &CHECK,
        &[
            ("FORGEJO_EVENT_PATH", ""),
            ("GITEA_EVENT_PATH", "  "),
            ("GITHUB_EVENT_PATH", ""),
        ],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.json()["could_not_check"].is_null());
}

#[test]
fn control_a_valid_payload_without_pull_request_or_before_proceeds() {
    let (repo, dir) = fixture();
    let event = payload(
        &dir,
        "event.json",
        r#"{"action": "created", "ref": "main"}"#,
    );
    let run = repo.run(&CHECK, &[("GITHUB_EVENT_PATH", event.as_str())]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.json()["could_not_check"].is_null());
}

#[test]
fn control_a_valid_pull_request_payload_has_its_body_scanned() {
    let (repo, dir) = fixture();
    let event = payload(&dir, "event.json", PULL_WITH_ESTIMATE);
    let run = repo.run(&CHECK, &[("GITHUB_EVENT_PATH", event.as_str())]);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stdout);
    let found = run.violations("time-estimates");
    assert!(
        found.iter().any(|v| v.to_string().contains("PR body")),
        "{}{}",
        run.stdout,
        run.stderr
    );
}
