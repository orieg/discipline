//! A pull request event whose payload carries `before` (GitHub's `synchronize`: a new push
//! to an open pull request) is a pull request run, not a push: the base is the pull
//! request's base branch, so the whole change is examined and not only the latest push.

mod common;
use common::{Repo, Run};

/// `adds` with one of its two assertions removed: a finding relative to `main`.
const ERODED_TEST: &str = "\
#[test]
fn adds() {
    let x = 1;
    assert_eq!(x + 1, 2);
}

#[test]
fn orders() {
    let x = 1;
    assert!(x < 2);
}
";

/// `work` holds two commits over `main`: the first removes an assertion, the second only
/// edits a document. Returns the repository and the two commit ids, oldest first.
fn two_push_repo() -> (Repo, String, String) {
    let repo = Repo::new();
    // The harness repository has no remote; `GITHUB_BASE_REF=main` resolves `origin/main`.
    repo.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    repo.write("tests/a.rs", ERODED_TEST);
    repo.commit("test: simplify adds");
    let first = repo.git_output(&["rev-parse", "HEAD"]);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2.\n\nMore.\n");
    repo.commit("docs: extend the plan");
    let second = repo.git_output(&["rev-parse", "HEAD"]);
    (repo, first, second)
}

fn write_payload(repo: &Repo, json: &str) -> String {
    // Outside the working tree, so the payload is not part of the change.
    let path = repo.path().join(".git").join("event.json");
    std::fs::write(&path, json).unwrap();
    path.to_string_lossy().into_owned()
}

fn synchronize_payload(before: Option<&str>, head: &str) -> String {
    let before = before.map_or(String::new(), |b| {
        format!(r#""before":"{b}","after":"{head}","#)
    });
    format!(
        r#"{{"action":"synchronize","number":7,{before}"pull_request":{{"number":7,"title":"test: simplify adds (#7)","body":"Refs #1","user":{{"login":"u"}},"head":{{"sha":"{head}"}},"base":{{"ref":"main"}}}}}}"#
    )
}

fn check(repo: &Repo, env: &[(&str, &str)]) -> Run {
    repo.run(&["check", "--format", "json"], env)
}

/// The assertion removed in the first push is reported, against the pull request's base.
fn assert_first_push_is_reported(run: &Run) {
    let report = run.json();
    let base = report["base"].as_str().unwrap_or_default();
    assert!(
        base.starts_with("origin/main"),
        "the base is not the pull request's base branch: {base:?}"
    );
    let found: Vec<(String, String, String)> = run
        .violations("assertion-reduction")
        .iter()
        .map(|v| {
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            (s("code"), s("title"), s("file"))
        })
        .collect();
    assert_eq!(
        found,
        [(
            "assertion-reduction/assertions-reduced".to_string(),
            "Assertion Count Decreased In Existing Test".to_string(),
            "tests/a.rs".to_string()
        )],
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

/// The reported case: a GitHub pull request run whose payload carries the previous head as
/// `before`. The assertion removed in the first push is part of the pull request.
#[test]
fn a_synchronize_run_examines_the_whole_pull_request() {
    let (repo, first, second) = two_push_repo();
    let event = write_payload(&repo, &synchronize_payload(Some(&first), &second));
    let run = check(
        &repo,
        &[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_EVENT_NAME", "pull_request"),
            ("GITHUB_BASE_REF", "main"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    );
    assert_first_push_is_reported(&run);
}

/// The same payload with no event name at all (a local run handed a payload file): the
/// `pull_request` object alone says this is not a push.
#[test]
fn a_synchronize_payload_with_no_event_name_is_not_a_push() {
    let (repo, first, second) = two_push_repo();
    let event = write_payload(&repo, &synchronize_payload(Some(&first), &second));
    let run = check(
        &repo,
        &[("GITHUB_BASE_REF", "main"), ("GITHUB_EVENT_PATH", &event)],
    );
    assert_first_push_is_reported(&run);
}

/// The same on Gitea and Forgejo, which set their own variables beside the
/// GitHub-compatible ones.
#[test]
fn a_synchronize_run_on_gitea_and_forgejo_examines_the_whole_pull_request() {
    let (repo, first, second) = two_push_repo();
    let event = write_payload(&repo, &synchronize_payload(Some(&first), &second));
    for (actions, name, base, path) in [
        (
            "GITEA_ACTIONS",
            "GITEA_EVENT_NAME",
            "GITEA_BASE_REF",
            "GITEA_EVENT_PATH",
        ),
        (
            "FORGEJO_ACTIONS",
            "FORGEJO_EVENT_NAME",
            "FORGEJO_BASE_REF",
            "FORGEJO_EVENT_PATH",
        ),
    ] {
        let run = check(
            &repo,
            &[
                (actions, "true"),
                (name, "pull_request"),
                (base, "main"),
                (path, &event),
            ],
        );
        assert_first_push_is_reported(&run);
    }
}

/// Control: with no `before` in the payload the run was already a pull request run.
#[test]
fn control_a_pull_request_payload_without_before_examines_the_whole_pull_request() {
    let (repo, _first, second) = two_push_repo();
    let event = write_payload(&repo, &synchronize_payload(None, &second));
    let run = check(
        &repo,
        &[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_EVENT_NAME", "pull_request"),
            ("GITHUB_BASE_REF", "main"),
            ("GITHUB_EVENT_PATH", &event),
        ],
    );
    assert_first_push_is_reported(&run);
}

/// Control: a real push with the same `before` still examines only the pushed range, both
/// when the event name says so and when a bare payload file is all there is.
#[test]
fn control_a_push_still_examines_only_the_pushed_range() {
    let (repo, first, second) = two_push_repo();
    let event = write_payload(
        &repo,
        &format!(r#"{{"ref":"refs/heads/work","before":"{first}","after":"{second}"}}"#),
    );
    for env in [
        &[
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_EVENT_PATH", event.as_str()),
        ][..],
        &[("GITHUB_EVENT_PATH", event.as_str())][..],
    ] {
        let run = check(&repo, env);
        let report = run.json();
        let base = report["base"].as_str().unwrap_or_default();
        assert!(base.starts_with(first.as_str()), "{env:?}: base {base:?}");
        assert!(
            run.violations("assertion-reduction").is_empty(),
            "{env:?}: a push run reported a commit outside the pushed range:\n{}",
            run.stdout
        );
    }
}
