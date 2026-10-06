//! #634: what a forge's answer to "which merged pull request carried this commit" means.
//!
//! Three readings, each from a shape that is positively recognised:
//!
//! * no merged pull request carries the commit (a direct push): GitHub answers 200 with a
//!   list that holds no merged pull request; Gitea, Forgejo and GitLab answer 404 for the
//!   lookup and then 200 for the commit itself, with a body that names that commit;
//! * the forge does not have the commit: GitHub answers 422 and says so in its body;
//!   Gitea, Forgejo and GitLab answer 404 for the lookup and 404 for the commit;
//! * anything else is a lookup that failed. On GitHub that includes every 404: it is the
//!   answer for a repository that is not there or not visible, never for a direct push.
//!
//! Every forge here is a `FakeForge` on a loopback port; the harness sets
//! `DISCIPLINE_NO_NETWORK=1`, so nothing else can be reached.

mod common;
use common::{FakeForge, Repo, Run};

const PULLS_DOC: &str =
    "https://docs.github.com/rest/commits/commits#list-pull-requests-associated-with-a-commit";

/// A change no gate reports, on `work`. Returns the repository and its head commit.
fn plain_change() -> (Repo, String) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("a.txt", "a\n");
    repo.commit("chore: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("b.txt", "b\n");
    repo.commit("chore: local only");
    let sha = head(&repo);
    (repo, sha)
}

fn head(repo: &Repo) -> String {
    repo.git_output(&["rev-parse", "HEAD"]).trim().to_string()
}

/// `check` on a push event against `api`, as the forge `kind`, with the JSON report.
fn push_run(repo: &Repo, api: &FakeForge, kind: &str) -> Run {
    let url = api.url();
    let mut env = vec![
        ("GITHUB_EVENT_NAME", "push"),
        ("GITHUB_REPOSITORY", "o/r"),
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("PR_BODY", ""),
    ];
    if kind != "github" {
        env.extend_from_slice(&[
            ("DISCIPLINE_FORGE", kind),
            ("DISCIPLINE_FORGE_URL", "http://127.0.0.1"),
            ("DISCIPLINE_FORGE_REPO", "o/r"),
        ]);
    }
    repo.run(&["check", "--base", "main", "--format", "json"], &env)
}

fn directive_notes(run: &Run) -> Vec<String> {
    run.json()["directive_notes"]
        .as_array()
        .map(|a| a.iter().map(|n| n.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn pulls_path(kind: &str, sha: &str) -> String {
    match kind {
        "github" => format!("repos/o/r/commits/{sha}/pulls"),
        "gitlab" => format!("projects/o%2Fr/repository/commits/{sha}/merge_requests"),
        _ => format!("repos/o/r/commits/{sha}/pull"),
    }
}

fn commit_path(kind: &str, sha: &str) -> String {
    match kind {
        "gitlab" => format!("projects/o%2Fr/repository/commits/{sha}"),
        _ => format!("repos/o/r/git/commits/{sha}"),
    }
}

fn direct_push(short: &str) -> String {
    format!(
        "merged-pr-body: commit {short} arrived through no merged pull request (direct push); its message is the only directive source"
    )
}

fn not_on(kind: &str, short: &str) -> String {
    format!(
        "merged-pr-body: commit {short} is not on {kind} (a local commit), so no merged pull request carries it; its message is the only directive source"
    )
}

/// The one directive note of a run that went on, which must say the lookup failed.
fn failed_lookup(run: &Run, kind: &str, short: &str, what: &str) -> String {
    assert_eq!(run.code, 0, "{what}: {}{}", run.stdout, run.stderr);
    let notes = directive_notes(run);
    assert_eq!(notes.len(), 1, "{what}: {notes:?}");
    assert!(
        notes[0].starts_with(&format!(
            "merged-pr-body: cannot resolve the merged pull request of commit {short} on {kind} ("
        )) && notes[0].ends_with("; continuing without it (`directives.degrade_offline`)"),
        "{what}: {notes:?}"
    );
    notes[0].clone()
}

// ---------------------------------------------------------------------------------------
// GitHub: a 404 is never a direct push.
// ---------------------------------------------------------------------------------------

/// 404 bodies GitHub, a proxy or a wrong base path can send. The marker is never printed.
const GITHUB_404_BODIES: &[(&str, &str)] = &[
    (
        "GitHub's own",
        r#"{"message":"Not Found","documentation_url":"https://docs.github.com/rest/commits/commits#list-pull-requests-associated-with-a-commit","status":"404"}"#,
    ),
    ("a bare message", r#"{"message":"Not Found"}"#),
    ("empty", ""),
    ("not JSON", "<html>MARKER634 404</html>"),
    ("an empty list", "[]"),
];

#[test]
fn on_github_a_404_for_the_pull_requests_of_a_commit_is_a_failed_lookup() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for (what, body) in GITHUB_404_BODIES {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path("github", &sha), 404, &[], body);
        let run = push_run(&repo, &api, "github");
        let note = failed_lookup(&run, "github", short, what);
        assert!(note.contains("HTTP 404"), "{what}: {note}");
        assert!(
            !run.stdout.contains("MARKER634") && !run.stderr.contains("MARKER634"),
            "{what}: {}{}",
            run.stdout,
            run.stderr
        );
        // GitHub is asked once: no second request settles a 404 there.
        assert_eq!(api.requests().len(), 1, "{what}");
    }

    // Control: the recognised answers still read as they did.
    let api = FakeForge::start();
    api.serve(&pulls_path("github", &sha), serde_json::json!([]));
    let run = push_run(&repo, &api, "github");
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(directive_notes(&run), vec![direct_push(short)]);

    let api = FakeForge::start();
    api.serve_raw(
        &pulls_path("github", &sha),
        422,
        &[],
        &format!(r#"{{"message":"No commit found for SHA: {sha}"}}"#),
    );
    let run = push_run(&repo, &api, "github");
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(directive_notes(&run), vec![not_on("github", short)]);
}

#[test]
fn on_github_a_404_stops_a_run_that_requires_the_record() {
    let (repo, _) = plain_change();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\ndegrade_offline = false\n",
    );
    repo.commit("chore: require the record");
    let commits = [
        head(&repo),
        repo.git_output(&["rev-parse", "HEAD~1"]).trim().to_string(),
    ];
    let api = FakeForge::start();
    for c in &commits {
        api.serve_raw(
            &pulls_path("github", c),
            404,
            &[],
            r#"{"message":"Not Found"}"#,
        );
    }
    let run = push_run(&repo, &api, "github");
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("forge".to_string(), None));
    assert!(
        run.stderr.contains("merged-pr-body: cannot resolve") && run.stderr.contains("HTTP 404"),
        "{}",
        run.stderr
    );

    // Control: an empty list is an answer, and the run goes on.
    let api = FakeForge::start();
    for c in &commits {
        api.serve(&pulls_path("github", c), serde_json::json!([]));
    }
    let run = push_run(&repo, &api, "github");
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(directive_notes(&run).len(), 2);
}

// ---------------------------------------------------------------------------------------
// GitHub: the 422 for a commit it does not have, by its words or by its fields.
// ---------------------------------------------------------------------------------------

#[test]
fn githubs_422_for_a_missing_commit_is_recognised_by_its_fields_when_the_words_differ() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    let body = |v: serde_json::Value| v.to_string();

    // The words GitHub uses today are not there; its fields are: its own status, the
    // documentation of this endpoint, no validation errors, and the commit asked for.
    let recognised = [
        body(serde_json::json!({
            "message": format!("Commit {sha} does not exist"),
            "documentation_url": PULLS_DOC,
            "status": "422",
        })),
        body(serde_json::json!({
            "message": format!("No object for {sha}"),
            "documentation_url": format!("https://docs.github.com/enterprise-server@3.14/{}", &PULLS_DOC["https://docs.github.com/".len()..]),
            "status": "422",
        })),
    ];
    for b in &recognised {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path("github", &sha), 422, &[], b);
        let run = push_run(&repo, &api, "github");
        assert_eq!(run.code, 0, "{b}: {}{}", run.stdout, run.stderr);
        assert_eq!(directive_notes(&run), vec![not_on("github", short)], "{b}");
    }

    // One field short of that shape, and it is a 422 that does not say the commit is
    // missing: a lookup that failed.
    let other = [
        (
            "a validation failure",
            body(serde_json::json!({
                "message": format!("Validation Failed for {sha}"),
                "errors": [{"resource": "Commit", "code": "custom"}],
                "documentation_url": PULLS_DOC,
                "status": "422",
            })),
        ),
        (
            "the commit is not named",
            body(serde_json::json!({
                "message": "Commit does not exist",
                "documentation_url": PULLS_DOC,
                "status": "422",
            })),
        ),
        (
            "another commit is named",
            body(serde_json::json!({
                "message": format!("Commit {} does not exist", "0".repeat(sha.len())),
                "documentation_url": PULLS_DOC,
                "status": "422",
            })),
        ),
        (
            "another endpoint's documentation",
            body(serde_json::json!({
                "message": format!("Commit {sha} does not exist"),
                "documentation_url": "https://docs.github.com/rest/commits/commits#get-a-commit",
                "status": "422",
            })),
        ),
        (
            "no documentation",
            body(serde_json::json!({
                "message": format!("Commit {sha} does not exist"),
                "status": "422",
            })),
        ),
        (
            "another status in the body",
            body(serde_json::json!({
                "message": format!("Commit {sha} does not exist"),
                "documentation_url": PULLS_DOC,
                "status": "409",
            })),
        ),
        (
            "no status in the body",
            body(serde_json::json!({
                "message": format!("Commit {sha} does not exist"),
                "documentation_url": PULLS_DOC,
            })),
        ),
    ];
    for (what, b) in &other {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path("github", &sha), 422, &[], b);
        let run = push_run(&repo, &api, "github");
        let note = failed_lookup(&run, "github", short, what);
        assert!(
            note.contains(&format!(
                "the forge answered HTTP 422 for commit {short} without saying the commit is missing"
            )),
            "{what}: {note}"
        );
    }

    // The fields mean nothing with another status: the same body under a 400.
    let api = FakeForge::start();
    api.serve_raw(&pulls_path("github", &sha), 400, &[], &recognised[0]);
    let run = push_run(&repo, &api, "github");
    let note = failed_lookup(&run, "github", short, "400");
    assert!(note.contains("HTTP 400"), "{note}");
}

// ---------------------------------------------------------------------------------------
// Gitea, Forgejo, GitLab: the commit's own answer must name the commit.
// ---------------------------------------------------------------------------------------

const NOT_FOUND: &str = r#"{"message":"The target couldn't be found."}"#;

#[test]
fn a_direct_push_needs_the_commit_endpoint_to_name_the_commit() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for kind in ["gitea", "forgejo", "gitlab"] {
        // The commit answered for, under the field each forge uses.
        for named in [
            serde_json::json!({"sha": sha}),
            serde_json::json!({"id": sha, "short_id": &sha[..8]}),
            serde_json::json!({"sha": sha.to_uppercase()}),
        ] {
            let api = FakeForge::start();
            api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
            api.serve(&commit_path(kind, &sha), named.clone());
            let run = push_run(&repo, &api, kind);
            assert_eq!(run.code, 0, "{kind} {named}: {}{}", run.stdout, run.stderr);
            assert_eq!(
                directive_notes(&run),
                vec![direct_push(short)],
                "{kind} {named}"
            );
        }

        // A 200 that does not name the commit says nothing about it.
        for (what, unnamed) in [
            ("an empty object", "{}".to_string()),
            (
                "another commit",
                serde_json::json!({"sha": "0".repeat(sha.len())}).to_string(),
            ),
            (
                "a short id only",
                serde_json::json!({"sha": &sha[..8]}).to_string(),
            ),
            ("a list", serde_json::json!([{"sha": sha}]).to_string()),
            (
                "a sha that is not text",
                r#"{"sha":{"MARKER634":1}}"#.to_string(),
            ),
            (
                "a page that is not about a commit",
                r#"{"ok":true,"note":"MARKER634"}"#.to_string(),
            ),
        ] {
            let api = FakeForge::start();
            api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
            api.serve_raw(&commit_path(kind, &sha), 200, &[], &unnamed);
            let run = push_run(&repo, &api, kind);
            let note = failed_lookup(&run, kind, short, &format!("{kind}: {what}"));
            assert!(
                note.contains(&format!(
                    "the forge answered for commit {short} without naming it"
                )),
                "{kind} {what}: {note}"
            );
            assert!(
                !run.stdout.contains("MARKER634") && !run.stderr.contains("MARKER634"),
                "{kind} {what}: {}{}",
                run.stdout,
                run.stderr
            );
        }

        // Control: a 404 for the commit is still "not on the forge".
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
        api.serve_raw(&commit_path(kind, &sha), 404, &[], NOT_FOUND);
        let run = push_run(&repo, &api, kind);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(directive_notes(&run), vec![not_on(kind, short)], "{kind}");
    }
}

#[test]
fn a_fake_forge_that_serves_only_the_lookup_answers_for_a_commit_it_has() {
    // The harness answers the commit endpoint itself when a test serves only the lookup:
    // that default names the commit, so such a test still reads a direct push.
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for kind in ["gitea", "forgejo", "gitlab"] {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
        let run = push_run(&repo, &api, kind);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(directive_notes(&run), vec![direct_push(short)], "{kind}");
    }
}

// ---------------------------------------------------------------------------------------
// A 200 says "no merged pull request" only when it says whether each one merged.
// ---------------------------------------------------------------------------------------

/// A forge, the 200 bodies that read as a direct push, and the named 200 bodies that do
/// not say whether a pull request merged.
type AnswersOfAForge = (
    &'static str,
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
);

#[test]
fn a_pull_request_answer_that_does_not_say_whether_it_merged_is_a_failed_lookup() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    // Per forge: answers that name a pull request and say it did not merge (a direct
    // push), and answers that do not say.
    let cases: &[AnswersOfAForge] = &[
        (
            "github",
            &["[]", r#"[{"number":3,"merged_at":null}]"#],
            &[
                ("no merged_at", r#"[{"number":3,"note":"MARKER634"}]"#),
                ("an empty entry", "[{}]"),
                ("an entry that is text", r#"["MARKER634"]"#),
                ("a null entry", "[null]"),
                (
                    "merged_at that is a number",
                    r#"[{"number":3,"merged_at":7}]"#,
                ),
            ],
        ),
        (
            "gitea",
            &[r#"{"number":6,"merged":false}"#],
            &[
                ("an empty object", "{}"),
                ("no merged", r#"{"number":5,"note":"MARKER634"}"#),
                ("merged that is text", r#"{"number":5,"merged":"yes"}"#),
                ("a list", "[]"),
            ],
        ),
        (
            "forgejo",
            &[r#"{"number":6,"merged":false}"#],
            &[
                ("an empty object", "{}"),
                ("merged that is null", r#"{"merged":null}"#),
            ],
        ),
        (
            "gitlab",
            &[
                "[]",
                r#"[{"iid":9,"state":"opened"}]"#,
                r#"[{"iid":9,"state":"closed"}]"#,
            ],
            &[
                ("no state", r#"[{"iid":9,"note":"MARKER634"}]"#),
                ("an empty entry", "[{}]"),
                ("state that is a number", r#"[{"iid":9,"state":3}]"#),
                ("a null entry", "[null]"),
            ],
        ),
    ];
    for (kind, direct, unsaid) in cases {
        for body in *direct {
            let api = FakeForge::start();
            api.serve_raw(&pulls_path(kind, &sha), 200, &[], body);
            let run = push_run(&repo, &api, kind);
            assert_eq!(run.code, 0, "{kind} {body}: {}{}", run.stdout, run.stderr);
            assert_eq!(
                directive_notes(&run),
                vec![direct_push(short)],
                "{kind} {body}"
            );
            // The answer is complete: the commit itself is not asked for.
            assert_eq!(api.requests().len(), 1, "{kind} {body}");
        }
        for (what, body) in *unsaid {
            let api = FakeForge::start();
            api.serve_raw(&pulls_path(kind, &sha), 200, &[], body);
            let run = push_run(&repo, &api, kind);
            let note = failed_lookup(&run, kind, short, &format!("{kind}: {what}"));
            assert!(
                note.contains(&format!(
                    "the forge answered for the pull requests of commit {short} without saying whether one merged"
                )),
                "{kind} {what}: {note}"
            );
            assert!(
                !run.stdout.contains("MARKER634") && !run.stderr.contains("MARKER634"),
                "{kind} {what}: {}{}",
                run.stdout,
                run.stderr
            );
        }
    }
}
