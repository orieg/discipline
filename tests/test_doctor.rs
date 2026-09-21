//! End-to-end tests for `discipline doctor`, driving the real binary.

mod common;
use common::{FakeForge, Repo};

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
    if: always()
    needs: [discipline]
    runs-on: ubuntu-latest
    steps:
      - env:
          NEEDS: ${{ toJson(needs) }}
        run: echo \"$NEEDS\" | jq -e 'all(.[]; .result == \"success\")'
";

const CODEOWNERS: &str = "/discipline.toml @o\n/.github/workflows/ @o\n";

/// A loopback GitHub API answering the rules, ruleset, branch and repository endpoints.
fn github_api(rules: &str) -> FakeForge {
    let api = FakeForge::start();
    api.serve_raw("repos/o/r/rules/branches/main", 200, &[], rules);
    api.serve(
        "repos/o/r/rulesets/1",
        serde_json::json!({"bypass_actors": []}),
    );
    api.serve(
        "repos/o/r/branches/main",
        serde_json::json!({"protected": true, "protection": {"enabled": false}}),
    );
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    api
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
    let api = github_api(GOOD_RULES);
    let url = api.url();
    let run = repo.run(
        &["doctor", "--repo", "o/r", "--format", "json"],
        &[
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GH_TOKEN", "gh-tok-1"),
        ],
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("required-check".into(), "pass".into())),
        "{st:?}"
    );
    assert!(st.iter().all(|(_, s)| s == "pass" || s == "info"), "{st:?}");
    // The token is sent as a bearer credential, with GitHub's API headers.
    let reqs = api.requests();
    assert!(!reqs.is_empty());
    for (_, headers) in &reqs {
        assert!(
            headers.contains(&("authorization".into(), "Bearer gh-tok-1".into())),
            "{headers:?}"
        );
        assert!(headers
            .iter()
            .any(|(k, v)| k == "user-agent" && v.starts_with("discipline/")));
    }
}

#[test]
fn doctor_required_check_without_discipline_fails() {
    let repo = protected_repo();
    let rules = GOOD_RULES.replace("\"ci-gate\"", "\"lint\"");
    let api = github_api(&rules);
    let url = api.url();
    let run = repo.run(
        &["doctor", "--repo", "o/r", "--format", "json"],
        &[("DISCIPLINE_FORGE_API_URL", url.as_str())],
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(statuses(&run.stdout).contains(&("required-check".into(), "fail".into())));
}

#[test]
fn doctor_without_platform_access_is_could_not_check() {
    let repo = protected_repo();
    // The harness sets DISCIPLINE_NO_NETWORK=1 and no API URL: github.com is unreachable.
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

#[test]
fn doctor_reads_gitea_protection_with_the_gitea_token_header() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".gitea/workflows/ci.yml", WORKFLOW),
            (
                ".gitea/CODEOWNERS",
                CODEOWNERS.replace(".github", ".gitea").as_str(),
            ),
            ("discipline.toml", "[meta]\nversion = 1\nname = \"t\"\n"),
        ],
        "base",
    );
    // Field names as a Gitea 1.24 instance returns them.
    let rule = r#"[{"rule_name":"main","enable_push":false,"enable_force_push":false,"enable_status_check":true,"status_check_contexts":["CI / ci-gate (pull_request)"],"block_on_outdated_branch":true,"block_admin_merge_override":true,"enable_merge_whitelist":false,"require_signed_commits":false}]"#;
    let api = FakeForge::start();
    api.serve_raw("repos/o/r/branch_protections", 200, &[], rule);
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    let url = api.url();
    let env = [
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("DISCIPLINE_FORGE", "gitea"),
        ("DISCIPLINE_FORGE_URL", "https://git.example.com"),
        ("DISCIPLINE_FORGE_REPO", "o/r"),
        ("GITEA_TOKEN", "tok-secret-123"),
    ];
    let run = repo.run(&["doctor", "--format", "json"], &env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("required-check".into(), "pass".into())),
        "{st:?}"
    );
    assert!(st.contains(&("codeowners".into(), "pass".into())), "{st:?}");
    let reqs = api.requests();
    assert!(
        reqs.iter()
            .all(|(_, h)| h.contains(&("authorization".into(), "token tok-secret-123".into()))),
        "{reqs:?}"
    );
    assert!(!run.stdout.contains("tok-secret-123") && !run.stderr.contains("tok-secret-123"));
}

#[test]
fn redirects_to_another_host_are_not_followed_with_the_token() {
    let repo = protected_repo();
    let api = FakeForge::start();
    api.serve_raw(
        "repos/o/r",
        302,
        &[("Location", "http://127.0.0.2:9/steal")],
        "",
    );
    let url = api.url();
    let run = repo.run(
        &["doctor", "--repo", "o/r"],
        &[
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GH_TOKEN", "t0k"),
        ],
    );
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("another host"), "{}", run.stdout);

    // Same host: followed.
    api.serve_raw("repos/o/r", 301, &[("Location", "/repos/o/renamed")], "");
    api.serve(
        "repos/o/renamed",
        serde_json::json!({"default_branch": "main"}),
    );
    let run = repo.run(
        &["doctor", "--repo", "o/r", "--format", "json"],
        &[("DISCIPLINE_FORGE_API_URL", url.as_str())],
    );
    assert!(
        run.stdout.contains("\"branch\": \"main\""),
        "{}",
        run.stdout
    );
}

#[test]
fn plain_http_forge_and_url_credentials_are_refused_or_stripped() {
    let repo = protected_repo();
    let run = repo.run(
        &["doctor"],
        &[
            ("DISCIPLINE_FORGE", "gitea"),
            (
                "DISCIPLINE_FORGE_URL",
                "http://oauth2:hunter2@git.example.com",
            ),
            ("DISCIPLINE_FORGE_REPO", "o/r"),
        ],
    );
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("plain HTTP"), "{}", run.stdout);
    assert!(!run.stdout.contains("hunter2") && !run.stderr.contains("hunter2"));
}

#[test]
fn pending_issue_state_is_read_from_gitea() {
    let repo = Repo::new();
    repo.commit_base("docs/perf.md", "# Perf\n", "base");
    repo.write("docs/perf.md", "# Perf\n\nArm B is pending re-run (#7).\n");
    repo.commit("docs: pending");
    let cfg = "[gates.provenance-tags]\nenabled = true\nrequire_open_pending_issues = true\n";
    let args = [
        "check",
        "--format",
        "json",
        "--base",
        "main",
        "--config-override",
        cfg,
    ];
    let api = FakeForge::start();
    let url = api.url();
    let env = vec![
        ("DISCIPLINE_FORGE_API_URL", url.clone()),
        ("DISCIPLINE_FORGE", "gitea".to_string()),
        (
            "DISCIPLINE_FORGE_URL",
            "https://git.example.com".to_string(),
        ),
        ("DISCIPLINE_FORGE_REPO", "o/r".to_string()),
    ];

    api.serve(
        "repos/o/r/issues/7",
        serde_json::json!({"number": 7, "state": "closed"}),
    );
    let run = repo.run(&args, &as_refs(&env));
    assert_eq!(run.code, 1, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("closed issue(s): #7"), "{}", run.stdout);

    api.serve(
        "repos/o/r/issues/7",
        serde_json::json!({"number": 7, "state": "open"}),
    );
    let run = repo.run(&args, &as_refs(&env));
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);

    // Unknown forge: could not check, never a pass.
    let run = repo.run(&args, &[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("DISCIPLINE_FORGE"), "{}", run.stderr);
}

fn as_refs<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(k, val)| (*k, val.as_str())).collect()
}

#[test]
fn an_invalid_forge_setting_is_not_replaced_by_a_github_guess() {
    let repo = protected_repo();
    let run = repo.run(
        &["doctor", "--repo", "o/r"],
        &[("DISCIPLINE_FORGE", "gitlub")],
    );
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("gitlub"), "{}", run.stdout);
}

#[test]
fn a_gate_without_its_input_is_reported_as_not_evaluated() {
    let repo = protected_repo();
    repo.write("README.md", "change\n");
    repo.commit("docs");
    let run = repo.run(&["check", "--base", "main"], &[]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    // ci-skip-set needs DISCIPLINE_CI_CONTEXT; without it the gate is neither passed nor failed.
    assert!(run.stdout.contains("1 not evaluated"), "{}", run.stdout);
}

#[test]
fn an_ssh_host_alias_in_the_remote_resolves_to_its_hostname() {
    // `git@forge:o/r.git` names a `Host forge` alias in ~/.ssh/config; the forge's
    // API is at its HostName, not at `https://forge`.
    let repo = protected_repo();
    repo.git(&["remote", "add", "origin", "git@forge:o/r.git"]);
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(
        home.path().join(".ssh/config"),
        "Host forge\n    HostName gitea.example.com\n    Port 2222\n    User git\n",
    )
    .unwrap();
    let api = FakeForge::start();
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    api.serve_raw("repos/o/r/branch_protections", 200, &[], "[]");
    let url = api.url();
    let home_str = home.path().to_str().unwrap();
    let run = repo.run(
        &["doctor", "--format", "json"],
        &[
            ("HOME", home_str),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ],
    );
    let v: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", run.stdout, run.stderr));
    assert_eq!(v["platform"], "gitea", "{}", run.stdout);
    assert_eq!(
        v["forge_url"], "https://gitea.example.com",
        "{}",
        run.stdout
    );
    assert_eq!(v["repository"], "o/r");

    // Without the alias the address is the bare alias, and the hint says so.
    let run = repo.run(
        &["doctor"],
        &[("HOME", "/nonexistent-home"), ("DISCIPLINE_FORGE", "gitea")],
    );
    assert_eq!(run.code, 2, "{}", run.stdout);
    assert!(run.stdout.contains("(https://forge)"), "{}", run.stdout);
}

#[test]
fn a_gitea_below_1_26_turns_the_token_warning_into_information() {
    let repo = Repo::new();
    // No `permissions:` in the workflow: a warning on GitHub, inert on Gitea 1.24.
    repo.commit_base_files(
        &[(
            ".gitea/workflows/ci.yml",
            &WORKFLOW.replace("permissions:\n  contents: read\n", ""),
        )],
        "base",
    );
    let api = FakeForge::start();
    api.serve("version", serde_json::json!({"version": "1.24.6"}));
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    api.serve_raw("repos/o/r/branch_protections", 200, &[], "[]");
    let url = api.url();
    let env = [
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("DISCIPLINE_FORGE", "gitea"),
        ("DISCIPLINE_FORGE_URL", "https://git.example.com"),
        ("DISCIPLINE_FORGE_REPO", "o/r"),
    ];
    let run = repo.run(&["doctor", "--format", "json"], &env);
    let st = statuses(&run.stdout);
    assert!(st.contains(&("token".into(), "info".into())), "{st:?}");
    assert!(
        run.stdout.contains("Gitea 1.24.6 ignores"),
        "{}",
        run.stdout
    );

    api.serve("version", serde_json::json!({"version": "1.26.0"}));
    let run = repo.run(&["doctor", "--format", "json"], &env);
    assert!(
        statuses(&run.stdout).contains(&("token".into(), "warn".into())),
        "{}",
        run.stdout
    );
}
