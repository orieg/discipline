//! Quoted text cannot make a reference to an issue, a commit or a mail address in the job
//! summary and the pull request comment (#651).
//!
//! A forge reads rendered Markdown for references: `#123`, `owner/repo#123`, `GH-123`,
//! `!123`, a commit id, `name@host`. One written by a directive's reason would notify the
//! people it names and write a line in the timeline of the issue it points at. In the two
//! Markdown formats each such word is a code span, which no forge reads. The terminal and
//! JSON reports carry the text as it is.
//!
//! The reason is written between two markers, so the test reads exactly the span the
//! report quoted. The forge here is a `FakeForge` on a loopback port.

mod common;
use common::{FakeForge, Repo, Run};

const START: &str = "QSTART651";
const END: &str = "QEND651";
const REFERENCES: &str =
    "fixes #123 and GH-45 for owner/repo#6 or !7 at deadbeef1 by a.b@h.example.invalid";
const WAIVER_PREFIX: &str = "allow-agent-instructions: AGENTS.md ";

fn instruction_change() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("AGENTS.md", "# Rules\n\nRun the tests.\n");
    repo.commit("docs: rules");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "AGENTS.md",
        "# Rules\n\nRun the tests.\n\nNever skip a failing test.\n",
    );
    repo.commit("docs: never skip");
    repo
}

fn waiver_body() -> String {
    format!("Reviewed.\n\n{WAIVER_PREFIX}{START} {REFERENCES} {END}\n")
}

/// The quoted span of `markdown`, between the markers.
fn quoted(markdown: &str) -> String {
    let line = markdown
        .lines()
        .find(|l| l.contains(START) && l.contains(END))
        .unwrap_or_else(|| panic!("no quoted span in:\n{markdown}"));
    let from = line.find(START).unwrap() + START.len();
    line[from..line.find(END).unwrap()].to_string()
}

/// The part of `text` outside its code spans (each written with single backticks here).
fn outside_code_spans(text: &str) -> String {
    text.split('`').step_by(2).collect::<Vec<_>>().join("\u{3}")
}

fn assert_no_reference(markdown: &str) {
    let span = quoted(markdown);
    for word in [
        "`#123`",
        "`GH-45`",
        "`owner/repo#6`",
        "`!7`",
        "`deadbeef1`",
        "`a.b@h.example.invalid`",
    ] {
        assert!(span.contains(word), "{word} is not a code span in: {span}");
    }
    let live = outside_code_spans(&span);
    assert_eq!(
        live.replace('\u{3}', "").trim(),
        "fixes  and  for  or  at  by",
        "{span}"
    );
}

#[test]
fn a_directive_reason_makes_no_reference_in_the_job_summary() {
    let repo = instruction_change();
    let body = waiver_body();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("summary.md");
    let run = repo.run(
        &["check", "--base", "main", "--format", "github-summary"],
        &[
            ("GITHUB_STEP_SUMMARY", path.to_str().unwrap()),
            ("PR_BODY", body.as_str()),
        ],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let summary = std::fs::read_to_string(&path).unwrap();
    assert_no_reference(&summary);
    // The report's own links are as they were.
    assert!(
        summary.starts_with("### Discipline gate: passed"),
        "{summary}"
    );
}

fn comment_run(repo: &Repo, pr_body: &str) -> (Run, String) {
    let event = repo.path().join("..").join(format!(
        "event-{}.json",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        &event,
        r#"{"pull_request":{"number":7,"user":{"login":"dev"},"head":{"sha":"abc"},"title":"t (#7)"}}"#,
    )
    .unwrap();
    let api = FakeForge::start();
    api.serve(
        "repos/o/r/issues/7/comments?per_page=100&page=1",
        serde_json::json!([]),
    );
    api.serve_raw("repos/o/r/issues/7/comments", 201, &[], r#"{"id": 900}"#);
    let url = api.url();
    let run = repo.run(
        &["check", "--base", "main", "--format", "json", "--comment"],
        &[
            ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GITHUB_TOKEN", "t"),
            ("PR_BODY", pr_body),
        ],
    );
    let writes = api.writes();
    assert_eq!(writes.len(), 1, "{writes:?}\n{}{}", run.stdout, run.stderr);
    let body: serde_json::Value = serde_json::from_str(&writes[0].2).unwrap();
    (run, body["body"].as_str().unwrap().to_string())
}

#[test]
fn a_directive_reason_makes_no_reference_in_the_pull_request_comment() {
    let repo = instruction_change();
    let (run, comment) = comment_run(&repo, &waiver_body());
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_no_reference(&comment);
}

#[test]
fn the_terminal_and_json_reports_write_the_reason_as_it_is() {
    let repo = instruction_change();
    let body = waiver_body();
    for format in ["terminal", "json"] {
        let run = repo.run(
            &["check", "--base", "main", "--format", format],
            &[("PR_BODY", body.as_str())],
        );
        assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
        assert!(
            run.stdout.contains(&format!("{START} {REFERENCES} {END}")),
            "{format}: {}",
            run.stdout
        );
    }
}
