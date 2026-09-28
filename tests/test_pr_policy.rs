//! Pull-request policy checks that read the forge: `issue-link` with `verify_references`,
//! driven through the real binary against a loopback Gitea.

mod common;

use common::{FakeForge, Repo};

const ISSUE_LINK: &str = "[gates.issue-link]\nenabled = true\nverify_references = true\n";

fn pr(config: &str) -> Repo {
    let repo = Repo::new();
    repo.write("discipline.toml", config);
    repo.write("docs/notes.md", "# Notes\n\nA change.\n");
    repo.commit("docs: notes");
    repo
}

fn check(repo: &Repo, api: &FakeForge, body: &str) -> common::Run {
    let url = api.url();
    repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[
            ("GITEA_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://git.example.com"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GITEA_TOKEN", "t"),
            ("PR_BODY", body),
            ("PR_TITLE", "docs: notes"),
        ],
    )
}

fn gitea() -> FakeForge {
    let api = FakeForge::start();
    api.serve("repos/o/r", serde_json::json!({"full_name": "o/r"}));
    api
}

fn notes(run: &common::Run, gate: &str) -> String {
    run.outcome(gate)["notes"].to_string()
}

/// The motivating defect: a body mentioning an upstream bug by bare number ("which fixes
/// #1162") on a Gitea that answers 500 for the comments of a missing issue. The number is
/// resolved through `issues/{n}` (404), so it is a named verdict, and the real reference
/// decides.
#[test]
fn a_bare_number_that_does_not_exist_is_a_verdict_not_a_crash() {
    let repo = pr(ISSUE_LINK);
    let api = gitea();
    api.serve_raw(
        "repos/o/r/issues/1162",
        404,
        &[],
        r#"{"message":"issue does not exist"}"#,
    );
    api.serve_raw(
        "repos/o/r/issues/1162/comments",
        500,
        &[],
        r#"{"message":"issue does not exist [id: 0, repo_id: 1, index: 1162]"}"#,
    );
    api.serve(
        "repos/o/r/issues/12",
        serde_json::json!({"number": 12, "state": "open"}),
    );

    let ok = check(
        &repo,
        &api,
        "Closes #12\n\nThis is the change which fixes #1162 upstream.",
    );
    assert_eq!(ok.code, 0, "{}{}", ok.stdout, ok.stderr);
    let n = notes(&ok, "issue-link");
    assert!(n.contains("#12 → ref-issue"), "{n}");
    assert!(n.contains("#1162 → ref-not-found"), "{n}");
    assert!(
        !api.requests().iter().any(|(p, _)| p.contains("/comments")),
        "comments are never read for issue-link"
    );

    // The same missing number as the only reference: a finding (exit 1), not exit 2.
    let missing = check(
        &repo,
        &api,
        "This is the change which fixes #1162 upstream.",
    );
    assert_eq!(missing.code, 1, "{}{}", missing.stdout, missing.stderr);
    let v = missing.violations("issue-link");
    assert_eq!(
        v[0]["code"], "issue-link/issue-reference-not-found",
        "{v:?}"
    );
}

/// A squash-style title carries the pull request's own number: it resolves to a pull
/// request, which does not track the change.
#[test]
fn the_pull_requests_own_number_in_the_title_is_not_an_issue() {
    let repo = pr(ISSUE_LINK);
    let api = gitea();
    api.serve(
        "repos/o/r/issues/101",
        serde_json::json!({"number": 101, "state": "open", "pull_request": {"merged": false}}),
    );
    let url = api.url();
    let run = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[
            ("GITEA_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://git.example.com"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("PR_BODY", "A change."),
            ("PR_TITLE", "docs: notes (#101)"),
        ],
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(notes(&run, "issue-link").contains("#101 → ref-is-pull"));
}

#[test]
fn a_closed_issue_does_not_track_a_change_unless_configured() {
    let api = gitea();
    api.serve(
        "repos/o/r/issues/8",
        serde_json::json!({"number": 8, "state": "closed"}),
    );
    let repo = pr(ISSUE_LINK);
    let closed = check(&repo, &api, "Refs #8");
    assert_eq!(closed.code, 1, "{}{}", closed.stdout, closed.stderr);
    assert_eq!(
        closed.violations("issue-link")[0]["code"],
        "issue-link/issue-reference-closed"
    );

    let repo = pr(&format!("{ISSUE_LINK}require_open_issue = false\n"));
    let ok = check(&repo, &api, "Refs #8");
    assert_eq!(ok.code, 0, "{}{}", ok.stdout, ok.stderr);
}

#[test]
fn a_forge_outage_is_retried_then_a_labelled_failure_never_a_pass() {
    let repo = pr(ISSUE_LINK);
    let api = gitea();
    api.serve_raw(
        "repos/o/r/issues/12",
        502,
        &[],
        r#"{"message":"bad gateway"}"#,
    );
    let down = check(&repo, &api, "Closes #12");
    assert_eq!(down.code, 2, "{}{}", down.stdout, down.stderr);
    let (reason, _) = down.could_not_check();
    assert_eq!(reason, "forge");
    let detail = down.json()["could_not_check"]["detail"].to_string();
    assert!(
        detail.contains("forge-unavailable") && detail.contains("after 3 attempts"),
        "{detail}"
    );
    let tries = api
        .requests()
        .iter()
        .filter(|(p, _)| p == "repos/o/r/issues/12")
        .count();
    assert_eq!(tries, 3);

    // A transient failure that recovers within the retries gives the normal verdict.
    let api = gitea();
    api.queue("repos/o/r/issues/12", 503, &[], "{}");
    api.serve(
        "repos/o/r/issues/12",
        serde_json::json!({"number": 12, "state": "open"}),
    );
    let ok = check(&repo, &api, "Closes #12");
    assert_eq!(ok.code, 0, "{}{}", ok.stdout, ok.stderr);
}

#[test]
fn a_repository_the_token_cannot_see_is_denied_not_a_run_of_missing_issues() {
    let repo = pr(ISSUE_LINK);
    let api = FakeForge::start();
    api.serve_raw("repos/o/r", 404, &[], r#"{"message":"Not Found"}"#);
    let run = check(&repo, &api, "Closes #12");
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let detail = run.json()["could_not_check"]["detail"].to_string();
    assert!(detail.contains("forge-denied"), "{detail}");
}

#[test]
fn cross_repository_references_are_not_looked_up_and_do_not_track_the_change() {
    let repo = pr(ISSUE_LINK);
    let api = gitea();
    let run = check(&repo, &api, "Fixes upstream/project#1162");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(notes(&run, "issue-link").contains("ref-cross-repo"));
    assert!(!api.requests().iter().any(|(p, _)| p.contains("upstream")));

    // A listed repository is looked up.
    let repo = pr(&format!(
        "{ISSUE_LINK}reference_repos = [\"upstream/project\"]\n"
    ));
    api.serve(
        "repos/upstream/project/issues/1162",
        serde_json::json!({"number": 1162, "state": "open"}),
    );
    let ok = check(&repo, &api, "Fixes upstream/project#1162");
    assert_eq!(ok.code, 0, "{}{}", ok.stdout, ok.stderr);
}

#[test]
fn the_waiver_can_be_switched_off() {
    let api = gitea();
    let body = "no-issue: typo in a comment";
    let repo = pr(ISSUE_LINK);
    let waived = check(&repo, &api, body);
    assert_eq!(waived.code, 0, "{}{}", waived.stdout, waived.stderr);

    let repo = pr(&format!("{ISSUE_LINK}waiver = \"none\"\n"));
    let refused = check(&repo, &api, body);
    assert_eq!(refused.code, 1, "{}{}", refused.stdout, refused.stderr);
}

#[test]
fn verify_references_with_a_custom_pattern_is_a_configuration_error() {
    let repo = pr(&format!("{ISSUE_LINK}pattern = \"JIRA-[0-9]+\"\n"));
    let api = gitea();
    let run = check(&repo, &api, "JIRA-12");
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check().0, "configuration");
}
