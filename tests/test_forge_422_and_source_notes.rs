//! #568: a 422 from the forge is read by what its body says, never by the status alone,
//! and the notes about where directives were read from reach the text report and the
//! job summary, not only the JSON report.
//!
//! Every forge here is a `FakeForge` on a loopback port; the harness sets
//! `DISCIPLINE_NO_NETWORK=1`, so nothing else can be reached.

mod common;
use common::{FakeForge, Repo, Run};

const WAIVER: &str = "allow-agent-instructions: AGENTS.md the rule was discussed in review";

/// A change to an agent-instruction file on `work`, which `instruction-smuggling` reports
/// unless a directive waives it. Returns the repository and the full id of its head commit.
fn instruction_change(message: &str) -> (Repo, String) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("AGENTS.md", "# Rules\n\nRun the tests.\n");
    repo.commit("docs: rules");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "AGENTS.md",
        "# Rules\n\nRun the tests.\n\nNever skip a failing test.\n",
    );
    repo.commit(message);
    let sha = head(&repo);
    (repo, sha)
}

/// A change no gate reports, on `work`.
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

/// `check` on a GitHub push event, against `api`, in `format` (`text` or `json`).
fn push_run(repo: &Repo, api: &FakeForge, format: &str, extra_env: &[(&str, &str)]) -> Run {
    let url = api.url();
    let mut env = vec![
        ("GITHUB_EVENT_NAME", "push"),
        ("GITHUB_REPOSITORY", "o/r"),
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("PR_BODY", ""),
    ];
    env.extend_from_slice(extra_env);
    repo.run(&["check", "--base", "main", "--format", format], &env)
}

fn directive_notes(run: &Run) -> Vec<String> {
    run.json()["directive_notes"]
        .as_array()
        .map(|a| a.iter().map(|n| n.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

/// The lines of `text` that are exactly `line`.
fn count_lines(text: &str, line: &str) -> usize {
    text.lines().filter(|l| *l == line).count()
}

fn merged_pull(author: &str, body: &str) -> serde_json::Value {
    serde_json::json!([{
        "number": 12,
        "merged_at": "2026-09-21T00:00:00Z",
        "user": {"login": author},
        "body": body,
        "head": {"sha": "feedbeef"}
    }])
}

// ---------------------------------------------------------------------------------------
// Part 1: a 422 is read by its body.
// ---------------------------------------------------------------------------------------

/// 422 bodies that do not say the commit is missing. The marker must never be printed.
const OTHER_422_BODIES: &[(&str, &str)] = &[
    (
        "validation failure",
        r#"{"message":"Validation Failed MARKER568","errors":[{"resource":"Commit","code":"custom"}]}"#,
    ),
    (
        "another message",
        r#"{"message":"Unprocessable Entity MARKER568"}"#,
    ),
    ("empty body", ""),
    ("not JSON", "<html>MARKER568 422 Unprocessable</html>"),
    ("JSON without a message", r#"{"error":"MARKER568"}"#),
    (
        "message that is not text",
        r#"{"message":{"sha":"No commit found for SHA"}}"#,
    ),
    (
        "the phrase somewhere other than the start",
        r#"{"message":"MARKER568 says: No commit found for SHA: 1"}"#,
    ),
];

#[test]
fn a_422_that_does_not_say_the_commit_is_missing_is_a_failed_lookup() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for (what, body) in OTHER_422_BODIES {
        let api = FakeForge::start();
        api.serve_raw(&format!("repos/o/r/commits/{sha}/pulls"), 422, &[], body);
        let run = push_run(&repo, &api, "json", &[]);
        let notes = directive_notes(&run);
        // A lookup that failed: by default a note that names it, and the run continues.
        assert_eq!(run.code, 0, "{what}: {}{}", run.stdout, run.stderr);
        assert_eq!(
            notes,
            vec![format!(
                "merged-pr-body: cannot resolve the merged pull request of commit {short} on github \
                 (the forge answered HTTP 422 for commit {short} without saying the commit is missing); \
                 continuing without it (`directives.degrade_offline`)"
            )],
            "{what}"
        );
        // Nothing the body said is printed.
        assert!(
            !run.stdout.contains("MARKER568") && !run.stderr.contains("MARKER568"),
            "{what}: {}{}",
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn a_422_that_does_not_say_the_commit_is_missing_stops_a_run_that_requires_the_record() {
    let (repo, _) = plain_change();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\ndegrade_offline = false\n",
    );
    repo.commit("chore: require the record");
    let sha = head(&repo);
    let first = repo.git_output(&["rev-parse", "HEAD~1"]).trim().to_string();
    let serve = |api: &FakeForge, body: &str| {
        // The push carries two commits; each is asked about.
        for c in [&sha, &first] {
            api.serve_raw(&format!("repos/o/r/commits/{c}/pulls"), 422, &[], body);
        }
    };

    for (what, body) in OTHER_422_BODIES {
        let api = FakeForge::start();
        serve(&api, body);
        let run = push_run(&repo, &api, "json", &[]);
        assert_eq!(run.code, 2, "{what}: {}{}", run.stdout, run.stderr);
        assert_eq!(run.could_not_check(), ("forge".to_string(), None), "{what}");
        assert!(
            run.stderr.contains("merged-pr-body: cannot resolve")
                && run.stderr.contains("HTTP 422"),
            "{what}: {}",
            run.stderr
        );
        assert!(
            !run.stdout.contains("MARKER568") && !run.stderr.contains("MARKER568"),
            "{what}: {}{}",
            run.stdout,
            run.stderr
        );
    }

    // Control: GitHub's own answer for a commit it does not have is still a definite
    // answer under `degrade_offline = false`: a note, and the run goes on.
    let api = FakeForge::start();
    for c in [&sha, &first] {
        api.serve_raw(
            &format!("repos/o/r/commits/{c}/pulls"),
            422,
            &[],
            &format!(r#"{{"message":"No commit found for SHA: {c}","documentation_url":"https://docs.github.com/rest/commits/commits#list-pull-requests-associated-with-a-commit","status":"422"}}"#),
        );
    }
    let run = push_run(&repo, &api, "json", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let notes = directive_notes(&run);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(
        notes
            .iter()
            .all(|n| n.contains("is not on github (a local commit)")),
        "{notes:?}"
    );
}

#[test]
fn control_the_recognised_422_a_404_and_a_200_read_as_before() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    let path = format!("repos/o/r/commits/{sha}/pulls");
    let one_note = |api: &FakeForge| {
        let run = push_run(&repo, api, "json", &[]);
        assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
        let notes = directive_notes(&run);
        assert_eq!(notes.len(), 1, "{notes:?}");
        notes[0].clone()
    };
    let not_on_forge = format!(
        "merged-pr-body: commit {short} is not on github (a local commit), so no merged pull request carries it; its message is the only directive source"
    );
    let direct = format!(
        "merged-pr-body: commit {short} arrived through no merged pull request (direct push); its message is the only directive source"
    );

    // GitHub's 422 for a commit it does not have, with and without the commit id.
    for body in [
        format!(r#"{{"message":"No commit found for SHA: {sha}"}}"#),
        r#"{"message":"No commit found for SHA"}"#.to_string(),
    ] {
        let api = FakeForge::start();
        api.serve_raw(&path, 422, &[], &body);
        assert_eq!(one_note(&api), not_on_forge, "{body}");
    }

    // 200 with an empty list: the forge has the commit and no pull request carries it.
    let api = FakeForge::start();
    api.serve(&path, serde_json::json!([]));
    assert_eq!(one_note(&api), direct);

    // 404 is not that answer (#634): GitHub gives it for a repository that is not there
    // or that the token cannot see, so the lookup failed.
    let api = FakeForge::start();
    api.serve_raw(&path, 404, &[], r#"{"message":"Not Found"}"#);
    assert_eq!(
        one_note(&api),
        format!(
            "merged-pr-body: cannot resolve the merged pull request of commit {short} on github \
             (the forge answered HTTP 404 for the pull requests of commit {short}: the repository is not there, or this run cannot see it); \
             continuing without it (`directives.degrade_offline`)"
        )
    );

    // 200 with a merged pull request.
    let api = FakeForge::start();
    api.serve(&path, merged_pull("agent", "Reviewed."));
    let run = push_run(&repo, &api, "json", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        directive_notes(&run),
        vec![
            "0 directive(s) read from merged pull request #12 (author agent)".to_string(),
            format!("merged-pr-body: commit {short} arrived through merged pull request #12 (author agent)"),
        ]
    );
}

#[test]
fn control_on_gitea_and_forgejo_a_422_is_a_failed_lookup_whatever_it_says() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for kind in ["gitea", "forgejo"] {
        let env = [
            ("DISCIPLINE_FORGE", kind),
            ("DISCIPLINE_FORGE_URL", "http://127.0.0.1"),
            ("DISCIPLINE_FORGE_REPO", "o/r"),
        ];
        // These forges answer 404 when no merged pull request carries the commit and have
        // no 422 answer for this endpoint, so GitHub's words mean nothing here.
        let api = FakeForge::start();
        api.serve_raw(
            &format!("repos/o/r/commits/{sha}/pull"),
            422,
            &[],
            &format!(r#"{{"message":"No commit found for SHA: {sha}"}}"#),
        );
        let run = push_run(&repo, &api, "json", &env);
        let notes = directive_notes(&run);
        assert_eq!(notes.len(), 1, "{kind}: {notes:?}");
        assert!(
            notes[0].starts_with(&format!(
                "merged-pr-body: cannot resolve the merged pull request of commit {short} on {kind} ("
            )) && notes[0].contains("HTTP 422"),
            "{kind}: {notes:?}"
        );

        let api = FakeForge::start();
        api.serve_raw(
            &format!("repos/o/r/commits/{sha}/pull"),
            404,
            &[],
            r#"{"message":"The target couldn't be found."}"#,
        );
        let run = push_run(&repo, &api, "json", &env);
        assert_eq!(
            directive_notes(&run),
            vec![format!(
                "merged-pr-body: commit {short} arrived through no merged pull request (direct push); its message is the only directive source"
            )],
            "{kind}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Part 2: the directive sources in the text report and the job summary.
// ---------------------------------------------------------------------------------------

#[test]
fn text_output_names_a_pull_request_body_directive() {
    let (repo, _) = instruction_change("docs: never skip");
    let run = repo.run(
        &["check", "--base", "main", "--format", "terminal"],
        &[("PR_BODY", WAIVER)],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        count_lines(
            &run.stdout,
            "  · override applied: `allow-agent-instructions: AGENTS.md the rule was discussed in review` on `AGENTS.md` (PR body)"
        ),
        1,
        "{}",
        run.stdout
    );
    // No pushed commit was looked up: no source note.
    assert!(
        !run.stdout.lines().any(|l| l.starts_with("directives: ")),
        "{}",
        run.stdout
    );
}

#[test]
fn text_output_names_a_commit_message_directive() {
    let (repo, sha) = instruction_change(&format!("docs: never skip\n\n{WAIVER}"));
    let run = repo.run(&["check", "--base", "main", "--format", "terminal"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let line = run
        .stdout
        .lines()
        .find(|l| l.contains("override applied"))
        .unwrap_or_else(|| panic!("no override line:\n{}", run.stdout));
    let (before, oid) = line.rsplit_once(" (commit ").expect(line);
    assert_eq!(
        before,
        "  · override applied: `allow-agent-instructions: AGENTS.md the rule was discussed in review` on `AGENTS.md`"
    );
    let oid = oid.strip_suffix(')').expect(line);
    assert!(oid.len() >= 7 && sha.starts_with(oid), "{line}");
}

#[test]
fn text_output_says_a_disabled_source_was_ignored() {
    let (repo, _) = instruction_change("docs: never skip");
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\nsources = [\"commits\"]\n",
    );
    repo.commit("chore: commit messages only");
    let run = repo.run(
        &["check", "--base", "main", "--format", "terminal"],
        &[("PR_BODY", WAIVER)],
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(
        count_lines(
            &run.stdout,
            "  · PR-body directives are disabled by policy; ignored"
        ) >= 1,
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("override applied"), "{}", run.stdout);
}

#[test]
fn text_output_names_the_merged_pull_request_a_push_was_read_from() {
    let (repo, sha) = instruction_change("docs: never skip (#12)");
    let short = &sha[..10];
    let api = FakeForge::start();
    api.serve(
        &format!("repos/o/r/commits/{sha}/pulls"),
        merged_pull("agent", &format!("Reviewed.\n\n{WAIVER}")),
    );
    let run = push_run(&repo, &api, "terminal", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    for line in [
        "directives: 1 directive(s) read from merged pull request #12 (author agent)".to_string(),
        format!("directives: merged-pr-body: commit {short} arrived through merged pull request #12 (author agent)"),
        "  · override applied: `allow-agent-instructions: AGENTS.md the rule was discussed in review` on `AGENTS.md` (merged pull request #12 body)".to_string(),
    ] {
        assert_eq!(count_lines(&run.stdout, &line), 1, "{line}\n{}", run.stdout);
    }
    // Reported for the run, never under a gate.
    assert!(
        !run.stdout.lines().any(|l| l.starts_with("  · ")
            && (l.contains("merged-pr-body:") || l.contains("directive(s) read from"))),
        "{}",
        run.stdout
    );
}

#[test]
fn text_output_says_a_push_came_through_no_merged_pull_request() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    let path = format!("repos/o/r/commits/{sha}/pulls");

    let api = FakeForge::start();
    api.serve(&path, serde_json::json!([]));
    let run = push_run(&repo, &api, "terminal", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        count_lines(
            &run.stdout,
            &format!("directives: merged-pr-body: commit {short} arrived through no merged pull request (direct push); its message is the only directive source")
        ),
        1,
        "{}",
        run.stdout
    );

    let api = FakeForge::start();
    api.serve_raw(
        &path,
        422,
        &[],
        &format!(r#"{{"message":"No commit found for SHA: {sha}"}}"#),
    );
    let run = push_run(&repo, &api, "terminal", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        count_lines(
            &run.stdout,
            &format!("directives: merged-pr-body: commit {short} is not on github (a local commit), so no merged pull request carries it; its message is the only directive source")
        ),
        1,
        "{}",
        run.stdout
    );
}

#[test]
fn text_output_says_a_merged_pull_request_lookup_failed_or_was_not_made() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];

    // The forge refuses (the fake's answer for a path it does not know is a 403).
    let api = FakeForge::start();
    let run = push_run(&repo, &api, "terminal", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let lines: Vec<&str> = run
        .stdout
        .lines()
        .filter(|l| l.starts_with("directives: "))
        .collect();
    assert_eq!(lines.len(), 1, "{}", run.stdout);
    assert!(
        lines[0].starts_with(&format!(
            "directives: merged-pr-body: cannot resolve the merged pull request of commit {short} on github ("
        )) && lines[0].contains("HTTP 403")
            && lines[0].ends_with("; continuing without it (`directives.degrade_offline`)"),
        "{}",
        lines[0]
    );

    // Off the network: a forge that is not on loopback is never asked.
    let run = push_run(
        &repo,
        &api,
        "terminal",
        &[("DISCIPLINE_FORGE_API_URL", "https://forge.invalid/api/v3")],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let lines: Vec<&str> = run
        .stdout
        .lines()
        .filter(|l| l.starts_with("directives: "))
        .collect();
    assert_eq!(lines.len(), 1, "{}", run.stdout);
    assert!(
        lines[0].starts_with(
            "directives: merged-pr-body: not read, network access is disabled (DISCIPLINE_NO_NETWORK): "
        ),
        "{}",
        lines[0]
    );

    // More commits than the lookup reads: no request is made.
    for i in 0..20 {
        repo.commit(&format!("chore: empty {i}"));
    }
    let api = FakeForge::start();
    let run = push_run(&repo, &api, "terminal", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        count_lines(
            &run.stdout,
            "directives: merged-pr-body: not read, the push carries 21 commits (more than 20); directives come from commit messages only"
        ),
        1,
        "{}",
        run.stdout
    );
    assert!(api.requests().is_empty(), "{:?}", api.requests());
}

#[test]
fn a_source_note_stays_a_run_note_whatever_the_author_is_called() {
    // The gate a directive note belongs to is found by the words in it. A note about where
    // a pushed commit came from carries an author login, which must not send it to a gate.
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for author in ["removes-bot", "deletes-things", "allow-stub-bot"] {
        let api = FakeForge::start();
        api.serve(
            &format!("repos/o/r/commits/{sha}/pulls"),
            merged_pull(author, "Reviewed."),
        );
        let lines = [
            format!("0 directive(s) read from merged pull request #12 (author {author})"),
            format!("merged-pr-body: commit {short} arrived through merged pull request #12 (author {author})"),
        ];
        let text = push_run(&repo, &api, "terminal", &[]);
        assert_eq!(text.code, 0, "{}{}", text.stdout, text.stderr);
        for line in &lines {
            assert_eq!(
                count_lines(&text.stdout, &format!("directives: {line}")),
                1,
                "{author}: {line}\n{}",
                text.stdout
            );
            assert_eq!(
                count_lines(&text.stdout, &format!("  · {line}")),
                0,
                "{author}: {line}\n{}",
                text.stdout
            );
        }
        // Under `--suite` the gate it would have gone to is not run; the note is still there.
        let url = api.url();
        let suite = repo.run(
            &[
                "check", "--base", "main", "--format", "terminal", "--suite", "hygiene",
            ],
            &[
                ("GITHUB_EVENT_NAME", "push"),
                ("GITHUB_REPOSITORY", "o/r"),
                ("DISCIPLINE_FORGE_API_URL", url.as_str()),
                ("PR_BODY", ""),
            ],
        );
        assert_eq!(suite.code, 0, "{}{}", suite.stdout, suite.stderr);
        for line in &lines {
            assert_eq!(
                count_lines(&suite.stdout, &format!("directives: {line}")),
                1,
                "{author} --suite: {line}\n{}",
                suite.stdout
            );
        }
    }
}

#[test]
fn the_job_summary_carries_the_source_notes() {
    let (repo, sha) = instruction_change("docs: never skip (#12)");
    let short = &sha[..10];
    let api = FakeForge::start();
    api.serve(
        &format!("repos/o/r/commits/{sha}/pulls"),
        merged_pull("agent", &format!("Reviewed.\n\n{WAIVER}")),
    );
    let out = tempfile::tempdir().unwrap();
    let summary = out.path().join("summary.md");
    let run = push_run(
        &repo,
        &api,
        "github-summary",
        &[("GITHUB_STEP_SUMMARY", summary.to_str().unwrap())],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let written = std::fs::read_to_string(&summary).unwrap();
    for line in [
        // A number or a commit id inside a message is a code span in Markdown.
        "**Directives:** 1 directive(s) read from merged pull request `#12` (author agent)"
            .to_string(),
        format!("**Directives:** merged-pr-body: commit `{short}` arrived through merged pull request `#12` (author agent)"),
    ] {
        assert_eq!(count_lines(&written, &line), 1, "{line}\n{written}");
    }
}
