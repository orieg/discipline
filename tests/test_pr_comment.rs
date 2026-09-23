//! `discipline check --comment` against a loopback forge: one comment per pull
//! request, edited on later runs, never a failure on a fork's read-only token.

mod common;

use common::{FakeForge, Repo};

const LIST: &str = "repos/o/r/issues/7/comments?per_page=100&page=1";
const MARKER: &str = "<!-- discipline:report -->";

fn weakened_pr() -> (Repo, std::path::PathBuf) {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    repo.commit("test: simplify");
    let event = repo.path().join("..").join(format!(
        "event-{}.json",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        &event,
        r#"{"pull_request":{"number":7,"user":{"login":"dev"},"head":{"sha":"abc"},"title":"t (#7)"}}"#,
    )
    .unwrap();
    (repo, event)
}

fn run(repo: &Repo, event: &std::path::Path, api: &str, extra: &[&str]) -> common::Run {
    let mut args = vec!["check", "--base", "main", "--format", "json"];
    args.extend_from_slice(extra);
    repo.run(
        &args,
        &[
            ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", api),
            ("GITHUB_TOKEN", "t"),
            ("PR_BODY", ""),
        ],
    )
}

#[test]
fn the_first_run_posts_one_comment_and_the_next_edits_it() {
    let (repo, event) = weakened_pr();
    let api = FakeForge::start();
    api.serve(LIST, serde_json::json!([{"id": 1, "body": "LGTM"}]));
    api.serve_raw("repos/o/r/issues/7/comments", 201, &[], r#"{"id": 42}"#);
    let first = run(&repo, &event, &api.url(), &["--comment"]);
    assert_eq!(
        first.code, 1,
        "the verdict is the gates', not the comment's: {}",
        first.stderr
    );
    let writes = api.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    let (method, path, body) = &writes[0];
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("POST", "repos/o/r/issues/7/comments")
    );
    let text: serde_json::Value = serde_json::from_str(body).unwrap();
    let text = text["body"].as_str().unwrap();
    assert!(
        text.starts_with(MARKER) && text.contains("`assertion-reduction`"),
        "{text}"
    );
    assert!(
        !text.contains("allow-assertion-drop"),
        "waiver syntax in the comment: {text}"
    );
    assert!(
        first.stderr.contains("comment: posted on #7"),
        "{}",
        first.stderr
    );

    let api = FakeForge::start();
    api.serve(LIST, serde_json::json!([{"id": 1, "body": "LGTM"}, {"id": 42, "body": format!("{MARKER}\nold")}]));
    api.serve_raw("repos/o/r/issues/comments/42", 200, &[], r#"{"id": 42}"#);
    let second = run(&repo, &event, &api.url(), &["--comment"]);
    assert_eq!(second.code, 1);
    let writes = api.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(
        (writes[0].0.as_str(), writes[0].1.as_str()),
        ("PATCH", "repos/o/r/issues/comments/42")
    );
}

#[test]
fn a_fork_token_is_a_note_an_unreachable_forge_stops_and_no_flag_posts_nothing() {
    let (repo, event) = weakened_pr();
    // The token cannot write (a fork): named, and the verdict stands.
    let api = FakeForge::start();
    api.serve(LIST, serde_json::json!([]));
    api.serve_raw(
        "repos/o/r/issues/7/comments",
        403,
        &[],
        r#"{"message":"Resource not accessible by integration"}"#,
    );
    let fork = run(&repo, &event, &api.url(), &["--comment"]);
    assert_eq!(fork.code, 1, "{}", fork.stderr);
    assert!(fork.stderr.contains("not posted on #7"), "{}", fork.stderr);

    // Without the flag nothing is written.
    let api = FakeForge::start();
    api.serve(LIST, serde_json::json!([]));
    let quiet = run(&repo, &event, &api.url(), &[]);
    assert_eq!(quiet.code, 1);
    assert!(api.writes().is_empty());

    // A forge that cannot be reached stops the run: the comment was asked for.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let down = run(&repo, &event, &dead, &["--comment"]);
    assert_eq!(down.code, 2, "{}", down.stderr);
    assert!(down.stderr.contains("--comment"), "{}", down.stderr);
}

#[test]
fn a_run_without_a_pull_request_posts_nothing() {
    let (repo, _) = weakened_pr();
    let api = FakeForge::start();
    let run = repo.run(
        &["check", "--base", "main", "--comment"],
        &[
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", api.url().as_str()),
            ("GITHUB_TOKEN", "t"),
        ],
    );
    assert_eq!(run.code, 1);
    assert!(run.stderr.contains("nothing posted"), "{}", run.stderr);
    assert!(api.writes().is_empty());
}
