//! End-to-end tests for `discipline doctor`, driving the real binary.

mod common;
use common::{FakeForge, Repo, Run, CONFIG_HEAD};

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
    api.serve(
        "repos/o/r",
        serde_json::json!({"default_branch": "main", "permissions": {"admin": true},
            "private": false, "visibility": "public", "allow_auto_merge": false, "allow_forking": true,
            "security_and_analysis": {"secret_scanning": {"status": "enabled"},
                "secret_scanning_push_protection": {"status": "enabled"}}}),
    );
    serve_safe_settings(&api);
    api
}

/// The repository settings `doctor` reads (Actions policy, immutable releases, tag
/// rulesets, secrets), each at its safe value.
fn serve_safe_settings(api: &FakeForge) {
    api.serve(
        "repos/o/r/actions/permissions",
        serde_json::json!({"enabled": true, "allowed_actions": "selected", "sha_pinning_required": true}),
    );
    api.serve(
        "repos/o/r/actions/permissions/workflow",
        serde_json::json!({"default_workflow_permissions": "read", "can_approve_pull_request_reviews": false}),
    );
    api.serve(
        "repos/o/r/immutable-releases",
        serde_json::json!({"enabled": true, "enforced_by_owner": false}),
    );
    api.serve(
        "repos/o/r/rulesets?targets=tag&includes_parents=true&per_page=100&page=1",
        serde_json::json!([{"id": 9, "target": "tag", "enforcement": "active"}]),
    );
    api.serve(
        "repos/o/r/rulesets/9",
        serde_json::json!({"name": "release tags", "target": "tag", "enforcement": "active",
            "conditions": {"ref_name": {"include": ["refs/tags/v*.*.*"], "exclude": []}},
            "rules": [{"type": "update"}, {"type": "deletion"}]}),
    );
    api.serve(
        "repos/o/r/releases/latest",
        serde_json::json!({"tag_name": "v1.0.0"}),
    );
    api.serve(
        "repos/o/r/actions/secrets?per_page=100&page=1",
        serde_json::json!({"total_count": 0, "secrets": []}),
    );
    api.serve_raw("repos/o/r/vulnerability-alerts", 204, &[], "");
    api.serve(
        "repos/o/r/keys?per_page=100&page=1",
        serde_json::json!([{"id": 1, "read_only": true}]),
    );
    api.serve(
        "repos/o/r/collaborators?affiliation=outside&per_page=100&page=1",
        serde_json::json!([]),
    );
    api.serve(
        "repos/o/r/hooks?per_page=100&page=1",
        serde_json::json!([{"id": 4, "config": {"url": "https://ci.example.com/hook", "secret": "********", "insecure_ssl": "0"}}]),
    );
    api.serve(
        "repos/o/r/environments?per_page=100",
        serde_json::json!({"total_count": 0, "environments": []}),
    );
    api.serve(
        "repos/o/r/automated-security-fixes",
        serde_json::json!({"enabled": true, "paused": false}),
    );
}

const GOOD_RULES: &str = r#"[{"type":"required_status_checks","ruleset_id":1,"parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"ci-gate"}]}},{"type":"non_fast_forward","ruleset_id":1},{"type":"deletion","ruleset_id":1},{"type":"pull_request","ruleset_id":1,"parameters":{"required_approving_review_count":1,"require_code_owner_review":true,"dismiss_stale_reviews_on_push":true}}]"#;

fn protected_repo() -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".github/workflows/ci.yml", WORKFLOW),
            (".github/CODEOWNERS", CODEOWNERS),
            ("discipline.toml", CONFIG_HEAD),
        ],
        "base",
    );
    repo
}

/// `doctor --repo o/r --format json` against the loopback `api` as GitHub with
/// the harness token: the env/run pair most doctor tests share.
fn doctor_json(repo: &Repo, api: &FakeForge) -> Run {
    let url = api.url();
    let env = [
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("GH_TOKEN", "gh-tok-1"),
    ];
    repo.run(&["doctor", "--repo", "o/r", "--format", "json"], &env)
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

/// The repository settings the OWASP CI/CD Security Cheat Sheet names that `doctor` reads
/// from the repository object (#366): auto-merge, forking of a private repository, secret
/// scanning and push protection, dependency alerts. Information or a warning, never a
/// failure; a setting the token cannot see is "could not check", never a pass.
#[test]
fn doctor_reports_auto_merge_forking_secret_scanning_and_dependency_alerts() {
    let status = |st: &[(String, String)], id: &str| {
        st.iter()
            .find(|(i, _)| i == id)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| panic!("no {id}: {st:?}"))
    };
    let repo = protected_repo();
    let no_review = GOOD_RULES.replace(
        "\"required_approving_review_count\":1",
        "\"required_approving_review_count\":0",
    );
    let api = github_api(&no_review);
    api.serve(
        "repos/o/r",
        serde_json::json!({"default_branch": "main", "permissions": {"admin": true},
            "private": true, "visibility": "private", "allow_auto_merge": true, "allow_forking": true,
            "security_and_analysis": {"secret_scanning": {"status": "disabled"},
                "secret_scanning_push_protection": {"status": "disabled"}}}),
    );
    api.serve_raw(
        "repos/o/r/vulnerability-alerts",
        404,
        &[],
        "{\"message\": \"Not Found\"}",
    );
    api.serve(
        "repos/o/r/automated-security-fixes",
        serde_json::json!({"enabled": false, "paused": false}),
    );
    let run = doctor_json(&repo, &api);
    let st = statuses(&run.stdout);
    // Auto-merge with no required review merges on the required check alone.
    assert_eq!(status(&st, "auto-merge"), "warn", "{}", run.stdout);
    assert_eq!(status(&st, "forking"), "warn", "{}", run.stdout);
    assert_eq!(status(&st, "secret-scanning"), "info", "{}", run.stdout);
    assert_eq!(status(&st, "dependency-alerts"), "info", "{}", run.stdout);
    // Warnings, not failures: without `--strict` the run still passes.
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);

    // A token without admin rights sees no auto-merge flag, no security settings and no
    // alert state: each is "could not check", never a pass.
    api.serve(
        "repos/o/r",
        serde_json::json!({"default_branch": "main", "permissions": {"admin": false, "push": true},
            "private": true, "visibility": "private", "allow_forking": false}),
    );
    let run = doctor_json(&repo, &api);
    let st = statuses(&run.stdout);
    for id in ["auto-merge", "secret-scanning", "dependency-alerts"] {
        assert_eq!(status(&st, id), "warn", "{id}: {}", run.stdout);
    }
    assert_eq!(status(&st, "forking"), "pass", "{}", run.stdout);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let auto = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "auto-merge")
        .unwrap();
    assert!(
        auto["summary"]
            .as_str()
            .unwrap()
            .contains("could not check"),
        "{auto}"
    );
}

/// Access and identity (#366): deploy keys that can push and outside collaborators,
/// counted and never named, and on GitHub the owning organisation's base permission,
/// two-factor requirement and visibility-change setting.
#[test]
fn doctor_counts_deploy_keys_and_collaborators_and_reads_the_organisation() {
    let status = |st: &[(String, String)], id: &str| {
        st.iter()
            .find(|(i, _)| i == id)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| panic!("no {id}: {st:?}"))
    };
    let repo = protected_repo();
    let api = github_api(GOOD_RULES);
    api.serve(
        "repos/o/r",
        serde_json::json!({"default_branch": "main", "permissions": {"admin": true},
            "owner": {"type": "Organization"}, "private": true, "visibility": "private",
            "allow_auto_merge": false, "allow_forking": false,
            "security_and_analysis": {"secret_scanning": {"status": "enabled"},
                "secret_scanning_push_protection": {"status": "enabled"}}}),
    );
    api.serve(
        "repos/o/r/keys?per_page=100&page=1",
        serde_json::json!([{"id": 1, "read_only": false, "title": "release-bot"}, {"id": 2, "read_only": true}]),
    );
    api.serve(
        "repos/o/r/collaborators?affiliation=outside&per_page=100&page=1",
        serde_json::json!([{"id": 7, "login": "contractor-x", "permissions": {"push": true}}]),
    );
    api.serve(
        "orgs/o",
        serde_json::json!({"default_repository_permission": "write", "two_factor_requirement_enabled": false,
            "members_can_change_repo_visibility": true}),
    );
    let run = doctor_json(&repo, &api);
    let st = statuses(&run.stdout);
    for (id, want) in [
        ("deploy-keys", "warn"),
        ("outside-collaborators", "info"),
        ("org-base-permission", "warn"),
        ("two-factor", "warn"),
        ("visibility-change", "warn"),
    ] {
        assert_eq!(status(&st, id), want, "{id}: {}", run.stdout);
    }
    // Counted, never named.
    assert!(
        !run.stdout.contains("release-bot") && !run.stdout.contains("contractor-x"),
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);

    // A member's token: the organisation hides its base permission and two-factor setting.
    api.serve(
        "orgs/o",
        serde_json::json!({"default_repository_permission": null, "two_factor_requirement_enabled": null,
            "members_can_change_repo_visibility": false}),
    );
    let run = doctor_json(&repo, &api);
    let st = statuses(&run.stdout);
    assert_eq!(status(&st, "org-base-permission"), "warn", "{}", run.stdout);
    assert_eq!(status(&st, "two-factor"), "warn", "{}", run.stdout);
    assert_eq!(status(&st, "visibility-change"), "pass", "{}", run.stdout);
    assert!(run.stdout.contains("organisation owner"), "{}", run.stdout);
}

/// Webhooks and deployment environments (#366): a GitHub webhook with no secret or with TLS
/// verification off, by host only, and an environment that holds secrets but needs no
/// reviewer.
#[test]
fn doctor_reports_weak_webhooks_and_unreviewed_environments_with_secrets() {
    let status = |st: &[(String, String)], id: &str| {
        st.iter()
            .find(|(i, _)| i == id)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| panic!("no {id}: {st:?}"))
    };
    let repo = protected_repo();
    let api = github_api(GOOD_RULES);
    api.serve(
        "repos/o/r/hooks?per_page=100&page=1",
        serde_json::json!([
            {"id": 1, "config": {"url": "https://chat.example.com/hook/AbC123", "insecure_ssl": "0"}},
            {"id": 2, "config": {"url": "https://ci.example.com/hook", "secret": "********", "insecure_ssl": "0"}}
        ]),
    );
    api.serve(
        "repos/o/r/environments?per_page=100",
        serde_json::json!({"total_count": 3, "environments": [
            {"name": "copilot", "protection_rules": []},
            {"name": "package signing", "protection_rules": [{"type": "branch_policy"}]},
            {"name": "release", "protection_rules": [{"type": "required_reviewers"}]}
        ]}),
    );
    api.serve(
        "repos/o/r/environments/copilot/secrets",
        serde_json::json!({"total_count": 0, "secrets": []}),
    );
    api.serve(
        "repos/o/r/environments/package%20signing/secrets",
        serde_json::json!({"total_count": 2, "secrets": [{"name": "SIGNING_KEY"}, {"name": "SIGNING_PASS"}]}),
    );
    let run = doctor_json(&repo, &api);
    let st = statuses(&run.stdout);
    assert_eq!(status(&st, "webhooks"), "warn", "{}", run.stdout);
    assert_eq!(
        status(&st, "environment-reviewers"),
        "info",
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("chat.example.com"), "{}", run.stdout);
    assert!(
        run.stdout.contains("`package signing` (2 secret(s))"),
        "{}",
        run.stdout
    );
    // An environment with no secret needs no reviewer for this check.
    assert!(!run.stdout.contains("`copilot`"), "{}", run.stdout);
    // The webhook's path (often a token) and the secrets' names never appear.
    for leak in ["AbC123", "SIGNING_KEY", "SIGNING_PASS"] {
        assert!(!run.stdout.contains(leak), "{leak}: {}", run.stdout);
    }
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
}

/// `security-policy` (#366, OpenSSF Scorecard Security-Policy): a local file check,
/// information when the repository has no SECURITY.md.
#[test]
fn doctor_says_whether_the_repository_has_a_security_policy() {
    let status = |stdout: &str| {
        statuses(stdout)
            .into_iter()
            .find(|(i, _)| i == "security-policy")
            .map(|(_, s)| s)
            .unwrap_or_else(|| panic!("no security-policy: {stdout}"))
    };
    let repo = protected_repo();
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(status(&run.stdout), "info", "{}", run.stdout);
    repo.write(".github/SECURITY.md", "# Security\n\nReport privately.\n");
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(status(&run.stdout), "pass", "{}", run.stdout);
    assert!(
        run.stdout.contains(".github/SECURITY.md is present"),
        "{}",
        run.stdout
    );
}

#[test]
fn doctor_healthy_repository_passes() {
    let repo = protected_repo();
    let api = github_api(GOOD_RULES);
    let run = doctor_json(&repo, &api);
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
fn doctor_strict_ignores_the_copilot_cloud_agent_install_step() {
    // `hook install --agent copilot --cloud-agent` writes this workflow: it only puts the
    // binary on PATH, so its triggers and token are not a check's.
    let repo = protected_repo();
    repo.commit_base(
        ".github/workflows/copilot-setup-steps.yml",
        "on:\n  workflow_dispatch:\n  push:\n    paths: [.github/workflows/copilot-setup-steps.yml]\n  pull_request:\n    paths: [.github/workflows/copilot-setup-steps.yml]\npermissions:\n  contents: read\njobs:\n  copilot-setup-steps:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: orieg/discipline@v0\n        with:\n          install_only: 'true'\n",
        "setup steps",
    );
    let run = repo.run(&["doctor", "--local-only", "--strict"], &[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        !run.stdout.contains("copilot-setup-steps"),
        "{}",
        run.stdout
    );
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
            ("discipline.toml", CONFIG_HEAD),
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

/// `ratified-paths` trusts comment authorship, so an agent login with administrator
/// rights fails `doctor`, and an unprotected workflow that runs discipline is named.
#[test]
fn doctor_checks_what_owner_ratification_relies_on_gitea() {
    let config = "[meta]\nversion = 1\nname = \"t\"\n\n[gates.ratified-paths]\nenabled = true\nprotected_paths = [\"scripts/**\"]\nratifiers = [\"owner\"]\nagent_logins = [\"agent\"]\n";
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".gitea/workflows/ci.yml", WORKFLOW),
            (
                ".gitea/CODEOWNERS",
                CODEOWNERS.replace(".github", ".gitea").as_str(),
            ),
            ("discipline.toml", config),
        ],
        "base",
    );
    let run = |patterns: &str, agent: &str| {
        let rule = format!(
            r#"[{{"rule_name":"main","enable_push":false,"enable_status_check":true,"status_check_contexts":["CI / ci-gate (pull_request)"],"block_on_outdated_branch":true,"block_admin_merge_override":true,"protected_file_patterns":"{patterns}"}}]"#
        );
        let api = FakeForge::start();
        api.serve_raw("repos/o/r/branch_protections", 200, &[], &rule);
        api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
        api.serve_raw("repos/o/r/collaborators/agent/permission", 200, &[], agent);
        let url = api.url();
        let env = [
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("DISCIPLINE_FORGE", "gitea"),
            ("DISCIPLINE_FORGE_URL", "https://git.example.com"),
            ("DISCIPLINE_FORGE_REPO", "o/r"),
            ("GITEA_TOKEN", "t"),
        ];
        repo.run(&["doctor", "--format", "json"], &env)
    };
    let writer =
        r#"{"permission":"write","role_name":"write","user":{"login":"agent","is_admin":false}}"#;
    let ok = run(".gitea/workflows/**", writer);
    assert_eq!(ok.code, 0, "{}\n{}", ok.stdout, ok.stderr);
    let st = statuses(&ok.stdout);
    assert!(
        st.contains(&("workflow-protection".into(), "pass".into())),
        "{st:?}"
    );
    assert!(
        st.contains(&("agent-permission".into(), "pass".into())),
        "{st:?}"
    );

    let open = run("docs/*", writer);
    assert!(
        statuses(&open.stdout).contains(&("workflow-protection".into(), "warn".into())),
        "{}",
        open.stdout
    );
    // #428: Gitea's `*` stops at a `.`, so this pattern leaves `ci.yml` open.
    let star = run(".gitea/workflows/*", writer);
    assert!(
        statuses(&star.stdout).contains(&("workflow-protection".into(), "warn".into())),
        "{}",
        star.stdout
    );
    assert!(
        star.stdout.contains(".gitea/workflows/**"),
        "{}",
        star.stdout
    );

    let admin = run(
        ".gitea/workflows/**",
        r#"{"permission":"admin","role_name":"admin","user":{"login":"agent"}}"#,
    );
    assert_eq!(admin.code, 1, "{}\n{}", admin.stdout, admin.stderr);
    assert!(statuses(&admin.stdout).contains(&("agent-permission".into(), "fail".into())));
}

/// Bot logins carry brackets (`renovate[bot]`): the login is percent-encoded into the
/// API path, which the client otherwise refuses, leaving the check undecided.
#[test]
fn a_bot_agent_login_is_looked_up_not_refused() {
    let config = "[meta]\nversion = 1\nname = \"t\"\n\n[gates.ratified-paths]\nenabled = true\nprotected_paths = [\"scripts/**\"]\nratifiers = [\"owner\"]\nagent_logins = [\"renovate[bot]\"]\n";
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".gitea/workflows/ci.yml", WORKFLOW),
            (
                ".gitea/CODEOWNERS",
                CODEOWNERS.replace(".github", ".gitea").as_str(),
            ),
            ("discipline.toml", config),
        ],
        "base",
    );
    let rule = r#"[{"rule_name":"main","enable_push":false,"enable_status_check":true,"status_check_contexts":["CI / ci-gate (pull_request)"],"block_on_outdated_branch":true,"block_admin_merge_override":true,"protected_file_patterns":".gitea/workflows/**"}]"#;
    let api = FakeForge::start();
    api.serve_raw("repos/o/r/branch_protections", 200, &[], rule);
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    // An app account is not a collaborator: 404.
    api.serve_raw(
        "repos/o/r/collaborators/renovate%5Bbot%5D/permission",
        404,
        &[],
        r#"{"message":"Not Found"}"#,
    );
    let url = api.url();
    let env = [
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("DISCIPLINE_FORGE", "gitea"),
        ("DISCIPLINE_FORGE_URL", "https://git.example.com"),
        ("DISCIPLINE_FORGE_REPO", "o/r"),
        ("GITEA_TOKEN", "t"),
    ];
    let run = repo.run(&["doctor", "--format", "json"], &env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert!(
        statuses(&run.stdout).contains(&("agent-permission".into(), "pass".into())),
        "{}",
        run.stdout
    );
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

#[test]
fn doctor_reports_a_push_trigger_when_squash_or_rebase_merges_drop_the_pr_body() {
    let repo = Repo::new();
    let wf = WORKFLOW.replace(
        "on:\n  pull_request:\n",
        "on:\n  push:\n    branches: [main]\n  pull_request:\n",
    );
    repo.commit_base_files(
        &[
            (".github/workflows/ci.yml", wf.as_str()),
            (".github/CODEOWNERS", CODEOWNERS),
            ("discipline.toml", CONFIG_HEAD),
        ],
        "base",
    );
    let run_with = |repo_json: serde_json::Value| {
        let api = github_api(GOOD_RULES);
        api.serve("repos/o/r", repo_json);
        doctor_json(&repo, &api)
    };
    // Squash merges allowed, `merged-pr-body` on by default: information, naming the token
    // the push run needs.
    let run = run_with(
        serde_json::json!({"default_branch": "main", "allow_squash_merge": true, "allow_rebase_merge": false}),
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "info".into())),
        "{st:?}"
    );
    assert!(
        run.stdout.contains("token that can read pull requests"),
        "{}",
        run.stdout
    );
    // Merge commits only: the body reaches the push run; pass.
    let run = run_with(
        serde_json::json!({"default_branch": "main", "allow_squash_merge": false, "allow_rebase_merge": false}),
    );
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "pass".into())),
        "{st:?}"
    );
    // Local only with the source on: information.
    let local = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    let st = statuses(&local.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "info".into())),
        "{st:?}"
    );
    // With the source disabled the review record is lost on a squash merge: warn, on the
    // forge and locally.
    repo.commit_base_files(
        &[("discipline.toml", "[meta]\nversion = 1\nname = \"t\"\n[directives]\nsources = [\"pr-body\", \"commits\"]\n")],
        "chore: no merged-pr-body",
    );
    let run = run_with(
        serde_json::json!({"default_branch": "main", "allow_squash_merge": true, "allow_rebase_merge": false}),
    );
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "warn".into())),
        "{st:?}"
    );
    assert!(run.stdout.contains("merged-pr-body"), "{}", run.stdout);
    let local = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    let st = statuses(&local.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "warn".into())),
        "{st:?}"
    );
    // With the source off, a token not shown the merge methods cannot decide.
    let run = run_with(serde_json::json!({"default_branch": "main"}));
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "unknown".into())),
        "{st:?}"
    );
    // With the source on again, the same hidden merge methods are information: the token,
    // not the method, decides whether the review record reaches the push run.
    repo.commit_base_files(&[("discipline.toml", CONFIG_HEAD)], "chore: defaults again");
    let run = run_with(serde_json::json!({"default_branch": "main"}));
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("push-trigger".into(), "info".into())),
        "{st:?}"
    );
    // The healthy fixture (pull_request only) carries no such finding.
    let quiet = protected_repo();
    let api = github_api(GOOD_RULES);
    let url = api.url();
    let run = quiet.run(
        &["doctor", "--repo", "o/r", "--format", "json"],
        &[
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GH_TOKEN", "t"),
        ],
    );
    assert!(!statuses(&run.stdout)
        .iter()
        .any(|(id, _)| id == "push-trigger"));
}

/// A repository Copilot hook in a folder Copilot CLI does not trust never runs: `doctor`
/// warns and names the fixes; a user-level hook covering the folder makes it information,
/// a trusted folder a pass, and a machine without Copilot CLI configuration no finding.
#[test]
fn doctor_warns_when_copilot_does_not_trust_the_repository_hook() {
    let repo = protected_repo();
    let installed = repo.run(&["hook", "install", "--agent", "copilot"], &[]);
    assert_eq!(installed.code, 0, "{}", installed.stderr);
    let home = tempfile::tempdir().unwrap();
    let env = [("COPILOT_HOME", home.path().to_str().unwrap())];
    let trust = |folders: &[String]| {
        std::fs::write(
            home.path().join("config.json"),
            format!(
                "// managed\n{{\"trustedFolders\": {}}}\n",
                serde_json::json!(folders)
            ),
        )
        .unwrap()
    };
    let finding = || {
        let run = repo.run(&["doctor", "--local-only", "--format", "json"], &env);
        let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "copilot-trust")
            .cloned()
    };
    assert_eq!(finding(), None, "no Copilot CLI configuration");

    trust(&[]);
    let warn = finding().expect("untrusted");
    assert_eq!(warn["status"], "warn", "{warn}");
    let fix = warn["remediation"].as_str().unwrap_or_default();
    assert!(
        fix.contains("trustedFolders") && fix.contains("--agent copilot --user"),
        "{warn}"
    );

    let user = repo.run(&["hook", "install", "--agent", "copilot", "--user"], &env);
    assert_eq!(user.code, 0, "{}", user.stderr);
    assert_eq!(finding().unwrap()["status"], "info");

    trust(&[repo
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()]);
    assert_eq!(finding().unwrap()["status"], "pass");
}

fn local(repo: &Repo) -> (i32, Vec<(String, String)>) {
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    (run.code, statuses(&run.stdout))
}

fn status_of(st: &[(String, String)], id: &str) -> Vec<String> {
    st.iter()
        .filter(|(i, _)| i == id)
        .map(|(_, s)| s.clone())
        .collect()
}

#[test]
fn doctor_reports_what_several_agents_in_one_repository_rely_on() {
    let repo = protected_repo();
    // One worktree, no guard: information only.
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "ref-guard"), vec!["info"], "{st:?}");

    // A second worktree without the guard: a warning; installed: a pass.
    std::fs::write(repo.path().join(".git/info/exclude"), "wt2/\n").unwrap();
    repo.git(&["worktree", "add", "-q", "-b", "feat/b", "wt2"]);
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "ref-guard"), vec!["warn"], "{st:?}");
    assert_eq!(repo.run(&["lease", "install-guard"], &[]).code, 0);
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "ref-guard"), vec!["pass"], "{st:?}");

    // A stale lease is listed; a lease that does not parse fails.
    let dir = repo.path().join(".git/discipline/leases");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("wt2.json"),
        r#"{"agent":"copilot","session":"s","worktree":"/w","branches":["feat/b"],"taken_at":1,"heartbeat":1,"ttl_secs":60}"#,
    )
    .unwrap();
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "leases"), vec!["info"], "{st:?}");
    std::fs::write(dir.join("wt2.json"), "{").unwrap();
    let (code, st) = local(&repo);
    assert_eq!(status_of(&st, "leases"), vec!["fail"], "{st:?}");
    assert_eq!(code, 1);
    std::fs::remove_file(dir.join("wt2.json")).unwrap();

    // A hook file written before the pre-tool entry existed: information; regenerated: a pass.
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"discipline hook run --agent claude-code"}]}]}}"#,
    )
    .unwrap();
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "pretool-hook"), vec!["info"], "{st:?}");
    std::fs::remove_file(repo.path().join(".claude/settings.json")).unwrap();
    let install = repo.run(&["hook", "install", "--agent", "claude-code"], &[]);
    assert_eq!(install.code, 0, "{}", install.stderr);
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "pretool-hook"), vec!["pass"], "{st:?}");

    // Qwen Code: a project's own settings file is not discipline's; an earlier generated
    // one is information; regenerated, a pass.
    std::fs::create_dir_all(repo.path().join(".qwen")).unwrap();
    std::fs::write(
        repo.path().join(".qwen/settings.json"),
        r#"{"model":{"name":"m"}}"#,
    )
    .unwrap();
    let (_, st) = local(&repo);
    assert_eq!(status_of(&st, "pretool-hook"), vec!["pass"], "{st:?}");
    std::fs::write(
        repo.path().join(".qwen/settings.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"discipline hook run --agent qwen"}]}]}}"#,
    )
    .unwrap();
    let (_, st) = local(&repo);
    assert_eq!(
        status_of(&st, "pretool-hook"),
        vec!["pass", "info"],
        "{st:?}"
    );
    std::fs::remove_file(repo.path().join(".qwen/settings.json")).unwrap();
    let install = repo.run(&["hook", "install", "--agent", "qwen"], &[]);
    assert_eq!(install.code, 0, "{}", install.stderr);
    let (_, st) = local(&repo);
    assert_eq!(
        status_of(&st, "pretool-hook"),
        vec!["pass", "pass"],
        "{st:?}"
    );

    // Codex: an earlier generated file is information; regenerated, a pass.
    std::fs::create_dir_all(repo.path().join(".codex")).unwrap();
    std::fs::write(
        repo.path().join(".codex/hooks.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"discipline hook run --agent codex"}]}]}}"#,
    )
    .unwrap();
    let (_, st) = local(&repo);
    assert_eq!(
        status_of(&st, "pretool-hook"),
        vec!["pass", "pass", "info"],
        "{st:?}"
    );
    std::fs::remove_file(repo.path().join(".codex/hooks.json")).unwrap();
    let install = repo.run(&["hook", "install", "--agent", "codex"], &[]);
    assert_eq!(install.code, 0, "{}", install.stderr);
    let (_, st) = local(&repo);
    assert_eq!(
        status_of(&st, "pretool-hook"),
        vec!["pass", "pass", "pass"],
        "{st:?}"
    );
}

#[test]
fn doctor_reports_the_repository_settings_that_let_mutable_code_run() {
    let repo = protected_repo();
    let api = github_api(GOOD_RULES);
    let url = api.url();
    let env = [("DISCIPLINE_FORGE_API_URL", url.as_str())];
    let run = repo.run(&["doctor", "--repo", "o/r", "--format", "json"], &env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    for id in [
        "actions-sha-pinning",
        "allowed-actions",
        "default-token",
        "actions-approve-prs",
        "immutable-releases",
        "tag-protection",
        "secret-scoping",
    ] {
        assert!(st.contains(&(id.into(), "pass".into())), "{id}: {st:?}");
    }

    // The settings this repository had before they were changed, and a secret that only
    // an environment-bound job reads.
    api.serve(
        "repos/o/r/actions/permissions",
        serde_json::json!({"enabled": true, "allowed_actions": "all", "sha_pinning_required": false}),
    );
    api.serve(
        "repos/o/r/actions/permissions/workflow",
        serde_json::json!({"default_workflow_permissions": "write", "can_approve_pull_request_reviews": true}),
    );
    api.serve_raw(
        "repos/o/r/immutable-releases",
        404,
        &[],
        r#"{"message":"Not Found"}"#,
    );
    api.serve(
        "repos/o/r/rulesets?targets=tag&includes_parents=true&per_page=100&page=1",
        serde_json::json!([]),
    );
    api.serve(
        "repos/o/r/actions/secrets?per_page=100&page=1",
        serde_json::json!({"total_count": 1, "secrets": [{"name": "DEPLOY_KEY"}]}),
    );
    repo.commit_base(
        ".github/workflows/deploy.yml",
        "on: push\njobs:\n  deploy:\n    runs-on: ubuntu-latest\n    environment: production\n    steps:\n      - run: deploy\n        env:\n          KEY: ${{ secrets.DEPLOY_KEY }}\n",
        "deploy",
    );
    let run = repo.run(&["doctor", "--repo", "o/r", "--format", "json"], &env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    for id in [
        "actions-sha-pinning",
        "allowed-actions",
        "default-token",
        "actions-approve-prs",
        "immutable-releases",
        "tag-protection",
        "secret-scoping",
    ] {
        assert!(st.contains(&(id.into(), "warn".into())), "{id}: {st:?}");
    }
    assert!(
        run.stdout.contains("`DEPLOY_KEY` (environment production)"),
        "{}",
        run.stdout
    );
    let strict = repo.run(&["doctor", "--repo", "o/r", "--strict"], &env);
    assert_eq!(strict.code, 1, "{}\n{}", strict.stdout, strict.stderr);
}

#[test]
fn doctor_fails_an_imposter_pin_and_warns_on_a_tag_pinned_nested_action() {
    use base64::Engine as _;
    const GOOD: &str = "1111111111111111111111111111111111111111";
    const BAD: &str = "2222222222222222222222222222222222222222";
    let repo = protected_repo();
    let api = github_api(GOOD_RULES);
    let url = api.url();
    let env = [("DISCIPLINE_FORGE_API_URL", url.as_str())];
    api.serve("repos/a/act", serde_json::json!({"default_branch": "main"}));
    api.serve(
        &format!("repos/a/act/compare/main...{GOOD}"),
        serde_json::json!({"status": "behind"}),
    );
    api.serve(
        &format!("repos/a/act/compare/main...{BAD}"),
        serde_json::json!({"status": "diverged"}),
    );
    api.serve(
        "repos/a/act/branches?per_page=100&page=1",
        serde_json::json!([{"name": "main", "commit": {"sha": "a".repeat(40)}}]),
    );
    api.serve(
        "repos/a/act/tags?per_page=100&page=1",
        serde_json::json!([]),
    );
    let meta = |yml: &str| {
        serde_json::json!({"encoding": "base64",
            "content": base64::engine::general_purpose::STANDARD.encode(yml)})
    };
    let composite =
        |uses: &str| format!("runs:\n  using: composite\n  steps:\n    - uses: {uses}\n");
    api.serve(
        &format!("repos/a/act/contents/action.yml?ref={GOOD}"),
        meta(&composite("x/y@v1")),
    );
    api.serve(
        &format!("repos/a/act/contents/action.yml?ref={BAD}"),
        meta(&composite(&format!("x/y@{GOOD}"))),
    );
    let pinned = |sha: &str| {
        format!("on: push\njobs:\n  j:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: a/act@{sha}\n")
    };

    // A commit on `a/act`'s default branch, whose action uses a tag.
    repo.commit_base(".github/workflows/pins.yml", &pinned(GOOD), "pin");
    let run = repo.run(&["doctor", "--repo", "o/r", "--format", "json"], &env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("imposter-commit".into(), "pass".into())),
        "{st:?}"
    );
    assert!(
        st.contains(&("nested-action-pins".into(), "warn".into())),
        "{st:?}"
    );
    assert!(run.stdout.contains("`x/y@v1`"), "{}", run.stdout);

    // A commit on no branch or tag of `a/act`: it resolved through a fork.
    repo.commit_base(".github/workflows/pins.yml", &pinned(BAD), "imposter");
    let run = repo.run(&["doctor", "--repo", "o/r", "--format", "json"], &env);
    assert_eq!(run.code, 1, "{}\n{}", run.stdout, run.stderr);
    let st = statuses(&run.stdout);
    assert!(
        st.contains(&("imposter-commit".into(), "fail".into())),
        "{st:?}"
    );
    assert!(
        st.contains(&("nested-action-pins".into(), "pass".into())),
        "{st:?}"
    );
    assert!(run.stdout.contains(BAD), "{}", run.stdout);
}

/// GitLab settings through the binary against a fake forge serving the responses
/// recorded from gitlab.com (Phase 14): the job token, the CI/CD variables, the protected
/// tags and the token `doctor` runs with; then a fine-grained token refused the variables
/// read, whose warning names the permission it lacks.
#[test]
fn doctor_reads_gitlab_settings_and_audits_the_token() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/forge_settings/gitlab.json")).unwrap();
    let repo = Repo::new();
    let api = FakeForge::start();
    // The recording kept the CI settings; `doctor` also reads the default branch.
    let mut project = recorded["project_ci_settings"].clone();
    project["default_branch"] = serde_json::json!("main");
    project["visibility"] = serde_json::json!("private");
    project["forking_access_level"] = serde_json::json!("enabled");
    project["secret_push_protection_enabled"] = serde_json::json!(false);
    project["namespace"] = serde_json::json!({"kind": "group", "full_path": "o"});
    api.serve("projects/o%2Fr", project);
    // The top-level group does not require two-factor authentication.
    api.serve(
        "groups/o",
        serde_json::json!({"id": 5, "full_path": "o", "require_two_factor_authentication": false}),
    );
    // The recorded protected environment needs no approval.
    api.serve(
        "projects/o%2Fr/protected_environments?per_page=100&page=1",
        recorded["protected_environments"].clone(),
    );
    api.serve(
        "projects/o%2Fr/deploy_keys?per_page=100&page=1",
        serde_json::json!([{"id": 3, "title": "deploy", "can_push": true}]),
    );
    api.serve(
        "projects/o%2Fr/hooks?per_page=100&page=1",
        serde_json::json!([{"id": 8, "url": "https://plain.example.com/h", "enable_ssl_verification": false}]),
    );
    api.serve(
        "projects/o%2Fr/job_token_scope",
        recorded["job_token_scope"].clone(),
    );
    let mut vars = recorded["variables"].clone();
    for v in vars.as_array_mut().unwrap() {
        if v["key"] == "DEPLOY_TOKEN" {
            v["environment_scope"] = serde_json::json!("*");
        }
    }
    // A masked variable scoped to the recorded `production` environment.
    vars.as_array_mut().unwrap().push(serde_json::json!({
        "key": "PROD_DEPLOY_KEY", "masked": true, "hidden": false, "protected": true,
        "environment_scope": "production", "variable_type": "env_var", "raw": false, "description": null
    }));
    api.serve("projects/o%2Fr/variables?per_page=100&page=1", vars);
    api.serve(
        "projects/o%2Fr/protected_tags?per_page=100&page=1",
        recorded["protected_tags"].clone(),
    );
    api.serve(
        "projects/o%2Fr/releases?per_page=1",
        serde_json::json!([{"tag_name": "v1.0.0"}]),
    );
    // The probing token's scopes: it can write.
    api.serve(
        "personal_access_tokens/self",
        serde_json::json!({"scopes": recorded["token_self"]["scopes"], "expires_at": "2099-01-01"}),
    );
    let url = api.url();
    let env = [
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("DISCIPLINE_FORGE", "gitlab"),
        ("DISCIPLINE_FORGE_URL", "https://gitlab.example.com"),
        ("DISCIPLINE_FORGE_REPO", "o/r"),
        ("GITLAB_TOKEN", "glpat-secret-123"),
    ];
    let run = repo.run(&["doctor", "--format", "json"], &env);
    let st = statuses(&run.stdout);
    for (id, want) in [
        ("default-token", "pass"),
        ("tag-protection", "pass"),
        ("secret-scoping", "warn"),
        ("forge-token", "warn"),
        ("actions-sha-pinning", "info"),
        ("auto-merge", "info"),
        ("forking", "warn"),
        ("secret-scanning", "info"),
        ("dependency-alerts", "info"),
        ("deploy-keys", "warn"),
        ("outside-collaborators", "info"),
        ("two-factor", "warn"),
        ("webhooks", "warn"),
        ("environment-reviewers", "info"),
    ] {
        assert!(
            st.contains(&(id.into(), want.into())),
            "{id} {want}: {st:?}"
        );
    }
    assert!(run.stdout.contains("`DEPLOY_TOKEN`"), "{}", run.stdout);
    assert!(
        run.stdout.contains("`production` (1 secret(s))"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("`api`"), "{}", run.stdout);
    assert!(!run.stdout.contains("glpat-secret-123") && !run.stderr.contains("glpat-secret-123"));

    // A fine-grained token without `Variable: Read`.
    api.serve_raw(
        "projects/o%2Fr/variables?per_page=100&page=1",
        403,
        &[],
        r#"{"error":"insufficient_granular_scope","error_description":"Access denied: This operation requires a fine-grained personal access token with the following project permissions: [Variable: Read]."}"#,
    );
    let run = repo.run(&["doctor", "--format", "json"], &env);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let scoping = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "secret-scoping")
        .unwrap()
        .clone();
    assert_eq!(scoping["status"], "warn", "{scoping}");
    assert!(
        scoping["remediation"]
            .as_str()
            .unwrap_or("")
            .contains("`Variable: Read`"),
        "{scoping}"
    );
}

/// Gitea secrets through the binary against a fake forge serving the lists recorded from
/// Gitea 1.24 (Phase 14 Step 2): the exposure is stated, a pull-request workflow reading
/// a secret is named, and an organisation secret no workflow reads is a warning.
#[test]
fn doctor_states_gitea_secrets_and_reports_an_unread_one() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/forge_settings/gitea.json")).unwrap();
    let repo = Repo::new();
    repo.write(
        ".gitea/workflows/deploy.yml",
        "on:\n  pull_request:\njobs:\n  deploy:\n    runs-on: ubuntu-latest\n    steps:\n      - run: deploy\n        env:\n          KEY: ${{ secrets.DEPLOY_KEY }}\n",
    );
    repo.commit("ci: deploy");
    let api = FakeForge::start();
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    api.serve(
        "repos/o/r/actions/secrets?limit=50&page=1",
        recorded["repo_secrets"].clone(),
    );
    api.serve(
        "repos/o/r/actions/secrets?limit=50&page=2",
        serde_json::json!([]),
    );
    api.serve(
        "orgs/o/actions/secrets?limit=50&page=1",
        recorded["org_secrets"].clone(),
    );
    api.serve(
        "orgs/o/actions/secrets?limit=50&page=2",
        serde_json::json!([]),
    );
    let url = api.url();
    let env = [
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("DISCIPLINE_FORGE", "gitea"),
        ("DISCIPLINE_FORGE_URL", "https://git.example.com"),
        ("DISCIPLINE_FORGE_REPO", "o/r"),
        ("GITEA_TOKEN", "tok-secret-123"),
    ];
    let run = repo.run(&["doctor", "--format", "json"], &env);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let f = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "secret-scoping")
        .unwrap()
        .clone();
    assert_eq!(f["status"], "warn", "{f}");
    let summary = f["summary"].as_str().unwrap();
    assert!(summary.contains("no environment to scope"), "{summary}");
    assert!(
        summary.contains("`DEPLOY_KEY` (.gitea/workflows/deploy.yml job `deploy`)"),
        "{summary}"
    );
    assert!(
        summary.contains("not read by any workflow here: `ORG_TOKEN`"),
        "{summary}"
    );
    assert!(!run.stdout.contains("tok-secret-123") && !run.stderr.contains("tok-secret-123"));
}

#[test]
fn doctor_reports_test_report_finding_for_runtime_identity_ratcheting() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[package]\nname = \"r\"\n");
    repo.write(".github/workflows/ci.yml", WORKFLOW);
    repo.write(".github/CODEOWNERS", CODEOWNERS);
    repo.write("discipline.toml", "[meta]\nversion = 1\nname = \"r\"\n");
    repo.commit("ci: init with cargo and default test-floor");

    // Case 1: Cargo detected, no test_report in discipline.toml -> Status: Info
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let f = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "test-report")
        .expect("must emit test-report finding");
    assert_eq!(f["status"], "info", "{f}");
    let summary = f["summary"].as_str().unwrap();
    assert!(summary.contains("recognized test runner"));
    assert!(summary.contains("runtime-only test erosion"));
    assert!(f["remediation"]
        .as_str()
        .unwrap()
        .contains("gates.test-floor"));

    // Case 2: build-output test_report without base_report -> Status: Warn
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"r\"\n\n[gates.test-floor]\ntest_report = \"target/nextest/ci/junit.xml\"\n",
    );
    repo.commit("ci: configure build-output test_report");
    let run2 = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run2.code, 0, "{}", run2.stdout);
    let v2: serde_json::Value = serde_json::from_str(&run2.stdout).unwrap();
    let f2 = v2["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "test-report")
        .expect("must emit test-report finding");
    assert_eq!(f2["status"], "warn", "{f2}");
    assert!(f2["summary"]
        .as_str()
        .unwrap()
        .contains("target/nextest/ci/junit.xml"));
    assert!(f2["remediation"].as_str().unwrap().contains("base_report"));

    // Case 3: test_report configured with tracked path -> Status: Pass
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"r\"\n\n[gates.test-floor]\ntest_report = \"reports/junit.xml\"\n",
    );
    repo.commit("ci: configure tracked test_report");
    let run3 = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run3.code, 0, "{}", run3.stdout);
    let v3: serde_json::Value = serde_json::from_str(&run3.stdout).unwrap();
    let f3 = v3["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "test-report")
        .expect("must emit test-report finding");
    assert_eq!(f3["status"], "pass", "{f3}");
    assert!(f3["summary"]
        .as_str()
        .unwrap()
        .contains("reports/junit.xml"));
}

#[test]
fn doctor_reports_mutation_preset_finding() {
    let repo = Repo::new();
    repo.commit_base(".github/workflows/ci.yml", WORKFLOW, "base");
    repo.write("Cargo.toml", "[package]\nname = \"r\"\n");
    repo.commit("feat: rust package");

    // Case 1: runner detected, no mutation preset in config -> Status: Info
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let f = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "mutation-testing")
        .expect("must emit mutation-testing finding");
    assert_eq!(f["status"], "info", "{f}");
    let summary = f["summary"].as_str().unwrap();
    assert!(summary.contains("recognized test runner"));
    assert!(summary.contains("special-cased test inputs"));
    assert!(f["remediation"].as_str().unwrap().contains("cargo-mutants"));

    // Case 2: command.commands preset configured -> Status: Pass
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"r\"\n\n[[gates.command.commands]]\nname = \"mutation\"\npreset = \"cargo-mutants\"\n",
    );
    repo.commit("ci: configure mutation preset");
    let run2 = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run2.code, 0, "{}", run2.stdout);
    let v2: serde_json::Value = serde_json::from_str(&run2.stdout).unwrap();
    let f2 = v2["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "mutation-testing")
        .expect("must emit mutation-testing finding");
    assert_eq!(f2["status"], "pass", "{f2}");
    assert!(f2["summary"].as_str().unwrap().contains("cargo-mutants"));

    // Case 3: command.preset configured directly -> Status: Pass
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"r\"\n\n[gates.command]\npreset = \"cargo-mutants\"\n",
    );
    repo.commit("ci: configure command.preset");
    let run3 = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run3.code, 0, "{}", run3.stdout);
    let v3: serde_json::Value = serde_json::from_str(&run3.stdout).unwrap();
    let f3 = v3["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "mutation-testing")
        .expect("must emit mutation-testing finding");
    assert_eq!(f3["status"], "pass", "{f3}");

    // Case 4: Go runner detected with no mutation preset available -> no mutation-testing finding
    let go_repo = Repo::new();
    go_repo.commit_base(".github/workflows/ci.yml", WORKFLOW, "base");
    go_repo.write("go.mod", "module demo\n");
    go_repo.commit("feat: go module");
    let run_go = go_repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    assert_eq!(run_go.code, 0, "{}", run_go.stdout);
    let v_go: serde_json::Value = serde_json::from_str(&run_go.stdout).unwrap();
    assert!(
        v_go["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["id"] != "mutation-testing"),
        "Go runner without mutation preset must not emit mutation-testing finding"
    );
}

#[test]
fn doctor_reports_hook_mode_truthfully() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (".github/workflows/ci.yml", WORKFLOW),
            (".github/CODEOWNERS", CODEOWNERS),
        ],
        "base",
    );

    let agents = [
        ("claude-code", ".claude/settings.json"),
        ("copilot", ".github/hooks/discipline.json"),
        ("agy", ".agents/hooks.json"),
        ("opencode", ".opencode/plugins/discipline.js"),
        ("qwen", ".qwen/settings.json"),
        ("codex", ".codex/hooks.json"),
    ];

    for (agent, rel) in agents {
        // 1. Observe mode: Warn, pretool does not say "refuses"
        let install_obs = repo.run(&["hook", "install", "--agent", agent, "--observe"], &[]);
        assert_eq!(install_obs.code, 0, "{agent}: {}", install_obs.stderr);

        let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
        let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let findings = v["findings"].as_array().unwrap();

        let hook_mode = findings
            .iter()
            .find(|f| f["id"] == "hook-mode" && f["summary"].as_str().unwrap().contains(rel))
            .unwrap_or_else(|| panic!("must emit hook-mode finding for {agent} in observe mode"));
        assert_eq!(
            hook_mode["status"], "warn",
            "{agent} observe mode must be warn"
        );
        assert!(
            hook_mode["summary"].as_str().unwrap().contains("observe"),
            "{agent}: {hook_mode}"
        );
        let rem = hook_mode["remediation"]
            .as_str()
            .expect("must have remediation");
        assert!(rem.contains("discipline hook install"), "{agent}: {rem}");
        assert!(rem.contains("--enforce"), "{agent}: {rem}");

        let pretool = findings
            .iter()
            .find(|f| f["id"] == "pretool-hook" && f["summary"].as_str().unwrap().contains(rel))
            .unwrap_or_else(|| panic!("must emit pretool-hook finding for {agent}"));
        assert_eq!(pretool["status"], "pass");
        assert!(
            !pretool["summary"].as_str().unwrap().contains("refuses"),
            "{agent} observe pretool text must not contain 'refuses': {}",
            pretool["summary"]
        );
        assert!(
            pretool["summary"].as_str().unwrap().contains("logs"),
            "{agent} observe pretool text must say logs: {}",
            pretool["summary"]
        );

        let strict = repo.run(&["doctor", "--local-only", "--strict"], &[]);
        assert_eq!(
            strict.code, 0,
            "doctor --strict must not fail on observe mode hook: {}",
            strict.stdout
        );

        // 2. Enforcing mode: switch with --enforce, Pass, pretool says "refuses"
        let install_enf = repo.run(&["hook", "install", "--agent", agent, "--enforce"], &[]);
        assert_eq!(install_enf.code, 0, "{agent}: {}", install_enf.stderr);
        assert!(
            install_enf.stdout.contains("enforcing mode"),
            "{agent}: {}",
            install_enf.stdout
        );

        let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
        let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let findings = v["findings"].as_array().unwrap();

        let hook_mode = findings
            .iter()
            .find(|f| f["id"] == "hook-mode" && f["summary"].as_str().unwrap().contains(rel))
            .unwrap_or_else(|| panic!("must emit hook-mode finding for {agent} in enforcing mode"));
        assert_eq!(
            hook_mode["status"], "pass",
            "{agent} enforcing mode must be pass"
        );

        let pretool = findings
            .iter()
            .find(|f| f["id"] == "pretool-hook" && f["summary"].as_str().unwrap().contains(rel))
            .unwrap_or_else(|| panic!("must emit pretool-hook finding for {agent}"));
        assert_eq!(pretool["status"], "pass");
        assert!(
            pretool["summary"].as_str().unwrap().contains("refuses"),
            "{agent} enforcing pretool text must contain 'refuses': {}",
            pretool["summary"]
        );

        // 3. Edited file: Unknown
        let existing = std::fs::read_to_string(repo.path().join(rel)).unwrap();
        let edited = format!("{existing}\n// edited custom line");
        std::fs::write(repo.path().join(rel), edited).unwrap();

        let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
        let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
        let findings = v["findings"].as_array().unwrap();

        let hook_mode = findings
            .iter()
            .find(|f| f["id"] == "hook-mode" && f["summary"].as_str().unwrap().contains(rel))
            .unwrap_or_else(|| panic!("must emit hook-mode finding for {agent} when edited"));
        assert_eq!(
            hook_mode["status"], "unknown",
            "{agent} edited file must be unknown"
        );

        std::fs::remove_file(repo.path().join(rel)).unwrap();
    }

    // 4. Hook file with no check or stop entry: Warn
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"discipline hook run --agent claude-code --event pre-tool"}]}]}}"#,
    ).unwrap();
    let run = repo.run(&["doctor", "--local-only", "--format", "json"], &[]);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let findings = v["findings"].as_array().unwrap();
    let hook_mode = findings
        .iter()
        .find(|f| {
            f["id"] == "hook-mode"
                && f["summary"]
                    .as_str()
                    .unwrap()
                    .contains(".claude/settings.json")
        })
        .expect("must emit hook-mode finding for hook file with no check or stop entry");
    assert_eq!(hook_mode["status"], "warn");
    assert!(hook_mode["summary"]
        .as_str()
        .unwrap()
        .contains("no check or stop entry"));
    std::fs::remove_file(repo.path().join(".claude/settings.json")).unwrap();
}
