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

// ---- ratified-paths ----------------------------------------------------------------

const RATIFY: &str = "[gates.ratified-paths]\nenabled = true\nprotected_paths = [\"scripts/check_*.py\", \"discipline.toml\"]\nratifiers = [\"owner\"]\nagent_logins = [\"agent\"]\n";

/// A comment newer than any commit the test makes.
const LATER: &str = "2099-01-01T00:00:00Z";

/// A repository whose base carries the policy and a protected script, and whose change
/// edits the files in `edits`.
fn protected_change(policy: &str, edits: &[(&str, &str)]) -> (Repo, std::path::PathBuf) {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", policy),
            ("scripts/check_x.py", "print('check')\n"),
        ],
        "chore: policy",
    );
    for (path, content) in edits {
        repo.write(path, content);
    }
    repo.commit("chore: edit");
    let event = repo.path().join("..").join(format!(
        "event-{}.json",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        &event,
        r#"{"pull_request":{"number":7,"user":{"login":"agent"},"head":{"sha":"abc"}}}"#,
    )
    .unwrap();
    (repo, event)
}

fn ratify_check(
    repo: &Repo,
    event: &std::path::Path,
    api: &FakeForge,
    body: &str,
    policy_from: &str,
) -> common::Run {
    let url = api.url();
    repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "json",
            "--policy-from",
            policy_from,
        ],
        &[
            ("GITEA_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://git.example.com"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GITEA_TOKEN", "t"),
            ("PR_BODY", body),
            ("PR_TITLE", "chore: edit"),
        ],
    )
}

/// A loopback Gitea: issue 12 open with `comments`; issue 1162 missing, and its comments
/// answer 500 as Gitea 1.24 does.
fn gitea_with(comments: serde_json::Value) -> FakeForge {
    let api = gitea();
    api.serve_raw(
        "repos/o/r/issues/1162",
        404,
        &[],
        r#"{"message":"issue does not exist"}"#,
    );
    api.serve_raw(
        "repos/o/r/issues/1162/comments?limit=50&page=1",
        500,
        &[],
        r#"{"message":"issue does not exist [id: 0, repo_id: 1, index: 1162]"}"#,
    );
    api.serve(
        "repos/o/r/issues/12",
        serde_json::json!({"number": 12, "state": "open"}),
    );
    let n = comments
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
        .to_string();
    api.serve_raw(
        "repos/o/r/issues/12/comments?limit=50&page=1",
        200,
        &[("X-Total-Count", n.as_str())],
        &comments.to_string(),
    );
    api
}

fn comment(login: &str, body: &str, created: &str, updated: &str) -> serde_json::Value {
    serde_json::json!({"id": 5, "user": {"login": login}, "body": body,
        "created_at": created, "updated_at": updated, "original_author": ""})
}

const BLOCK: &str = "Owner-ratified-paths:\n- scripts/check_x.py\n";

fn codes(run: &common::Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_protected_edit_without_a_ratification_fails() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('weaker')\n")]);
    let api = gitea_with(serde_json::json!([comment("owner", "LGTM", LATER, LATER)]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "ratified-paths"),
        vec!["ratified-paths/protected-path-unratified"]
    );
}

/// The motivating defect, through the binary: the prose number resolves to nothing and is
/// never asked for its comments (which would answer 500); the real closing issue ratifies.
#[test]
fn a_missing_issue_in_the_body_does_not_crash_the_ratification() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let api = gitea_with(serde_json::json!([comment("owner", BLOCK, LATER, LATER)]));
    let run = ratify_check(
        &repo,
        &event,
        &api,
        "Closes #12\n\nThis is the change which fixes #1162 upstream.",
        "base",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let notes = notes(&run, "ratified-paths");
    assert!(notes.contains("#1162 → ref-not-found"), "{notes}");
    assert!(notes.contains("ratified in"), "{notes}");
    assert!(!api
        .requests()
        .iter()
        .any(|(p, _)| p.contains("1162/comments")));
}

#[test]
fn a_ratification_by_an_agent_login_does_not_count() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let api = gitea_with(serde_json::json!([comment("agent", BLOCK, LATER, LATER)]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let c = codes(&run, "ratified-paths");
    assert!(
        c.contains(&"ratified-paths/protected-path-unratified".to_string())
            && c.contains(&"ratified-paths/ratification-author-not-accepted".to_string()),
        "{c:?}"
    );
}

#[test]
fn a_glob_in_owner_ratified_paths_is_refused() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let body = "Owner-ratified-paths:\n- scripts/*.py\n- scripts/check_x.py\n";
    let api = gitea_with(serde_json::json!([comment("owner", body, LATER, LATER)]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let c = codes(&run, "ratified-paths");
    assert!(
        c.contains(&"ratified-paths/ratification-entry-malformed".to_string())
            && c.contains(&"ratified-paths/protected-path-unratified".to_string()),
        "the glob voids the whole block: {c:?}"
    );
}

#[test]
fn an_edited_owner_comment_does_not_ratify() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let api = gitea_with(serde_json::json!([comment(
        "owner",
        BLOCK,
        LATER,
        "2099-01-01T00:05:00Z"
    )]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "ratified-paths")
        .contains(&"ratified-paths/ratification-comment-edited".to_string()));
}

#[test]
fn a_ratification_older_than_the_paths_last_change_lapses() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let old = "2000-01-01T00:00:00Z";
    let api = gitea_with(serde_json::json!([comment("owner", BLOCK, old, old)]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "ratified-paths")
        .contains(&"ratified-paths/ratification-outside-window".to_string()));
}

#[test]
fn a_workflow_edit_fails_even_when_ratified() {
    let (repo, event) = protected_change(
        RATIFY,
        &[(".gitea/workflows/ci.yml", "on: push\njobs: {}\n")],
    );
    let body = "Owner-ratified-paths:\n- .gitea/workflows/ci.yml\n";
    let api = gitea_with(serde_json::json!([comment("owner", body, LATER, LATER)]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "ratified-paths")
        .contains(&"ratified-paths/never-ratifiable-path-changed".to_string()));
}

#[test]
fn the_policy_must_come_from_the_base() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let api = gitea_with(serde_json::json!([comment("owner", BLOCK, LATER, LATER)]));
    let run = ratify_check(&repo, &event, &api, "Closes #12", "head");
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check().0, "configuration");
}

#[test]
fn an_unreachable_forge_is_a_labelled_failure_never_a_pass() {
    let (repo, event) = protected_change(RATIFY, &[("scripts/check_x.py", "print('new')\n")]);
    let api = gitea();
    api.serve(
        "repos/o/r/issues/12",
        serde_json::json!({"number": 12, "state": "open"}),
    );
    api.serve_raw(
        "repos/o/r/issues/12/comments?limit=50&page=1",
        502,
        &[],
        "{}",
    );
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check().0, "forge");
    let detail = run.json()["could_not_check"]["detail"].to_string();
    assert!(detail.contains("forge-unavailable"), "{detail}");
}

#[test]
fn an_unprotected_change_asks_the_forge_nothing() {
    let (repo, event) = protected_change(RATIFY, &[("docs/notes.md", "# Notes\n")]);
    let api = FakeForge::start();
    // The body closes an issue: a gate that looked anything up would ask for it.
    let run = ratify_check(&repo, &event, &api, "Closes #12", "base");
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(api.requests().is_empty(), "{:?}", api.requests());
}
