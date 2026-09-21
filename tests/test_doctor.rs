//! End-to-end tests for `discipline doctor`, driving the real binary.

mod common;
use common::Repo;

const WORKFLOW: &str = "name: CI
on:
  pull_request:
    types: [opened, synchronize, reopened, edited]
permissions:
  contents: read
jobs:
  discipline:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: orieg/discipline@v0
  ci-gate:
    needs: [discipline]
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
";

const CODEOWNERS: &str = "/discipline.toml @o\n/.github/workflows/ @o\n";

/// A stand-in `gh` answering the rules, ruleset, branch and repository endpoints.
fn fake_gh(repo: &Repo, rules: &str) -> String {
    let path = repo.file("fake-gh.sh");
    let script = format!(
        "#!/bin/sh\ncase \"$2\" in\n  repos/o/r/rules/branches/main) echo '{rules}' ;;\n  repos/o/r/rulesets/1) echo '{{\"bypass_actors\": []}}' ;;\n  repos/o/r/branches/main) echo '{{\"protected\": true, \"protection\": {{\"enabled\": false}}}}' ;;\n  repos/o/r) echo '{{\"default_branch\": \"main\"}}' ;;\n  *) echo \"unexpected $2\" >&2; exit 1 ;;\nesac\n"
    );
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_str().unwrap().to_string()
}

const GOOD_RULES: &str = r#"[{"type":"required_status_checks","ruleset_id":1,"parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"ci-gate"}]}},{"type":"non_fast_forward","ruleset_id":1},{"type":"deletion","ruleset_id":1},{"type":"pull_request","ruleset_id":1,"parameters":{}}]"#;

fn protected_repo() -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".github/workflows/ci.yml", WORKFLOW),
            (".github/CODEOWNERS", CODEOWNERS),
            ("discipline.toml", "[meta]\nversion = 1\nname = \"t\"\n"),
        ],
        "base",
    );
    repo
}

fn statuses(stdout: &str) -> Vec<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap();
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["id"].as_str().unwrap().to_string(),
                f["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn doctor_healthy_repository_passes() {
    let repo = protected_repo();
    let gh = fake_gh(&repo, GOOD_RULES);
    let run = repo.run(
        &["doctor", "--repo", "o/r", "--format", "json"],
        &[("DISCIPLINE_GH", gh.as_str())],
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("required-check".into(), "pass".into())),
        "{st:?}"
    );
    assert!(st.iter().all(|(_, s)| s == "pass" || s == "info"), "{st:?}");
}

#[test]
fn doctor_required_check_without_discipline_fails() {
    let repo = protected_repo();
    let rules = GOOD_RULES.replace("\"ci-gate\"", "\"lint\"");
    let gh = fake_gh(&repo, &rules);
    let run = repo.run(
        &["doctor", "--repo", "o/r", "--format", "json"],
        &[("DISCIPLINE_GH", gh.as_str())],
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(statuses(&run.stdout).contains(&("required-check".into(), "fail".into())));
}

#[test]
fn doctor_without_platform_access_is_could_not_check() {
    let repo = protected_repo();
    // The test harness points DISCIPLINE_GH at a path that does not exist.
    let run = repo.run(&["doctor", "--repo", "o/r"], &[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("unknown"), "{}", run.stdout);

    let local = repo.run(&["doctor", "--local-only"], &[]);
    assert_eq!(local.code, 0, "{}\n{}", local.stdout, local.stderr);
}

#[test]
fn doctor_local_findings_warn_and_strict_fails_on_them() {
    let repo = Repo::new();
    repo.commit_base(
        ".github/workflows/ci.yml",
        &WORKFLOW
            .replace("    types: [opened, synchronize, reopened, edited]\n", "")
            .replace("permissions:\n  contents: read\n", ""),
        "base",
    );
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let st = statuses(&run.stdout);
    for id in ["trigger", "token", "codeowners"] {
        assert!(st.contains(&(id.into(), "warn".into())), "{id}: {st:?}");
    }
    let strict = repo.run(&["doctor", "--local-only", "--strict"], &[]);
    assert_eq!(strict.code, 1, "{}", strict.stdout);

    let empty = Repo::new();
    empty.commit_base("README.md", "x\n", "base");
    let none = empty.run(&["doctor", "--local-only"], &[]);
    assert_eq!(none.code, 1, "{}", none.stdout);
    assert!(none.stdout.contains("no workflow job runs discipline"));
}
