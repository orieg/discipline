//! #634: quoted text cannot render as emphasis or as a link in the job summary and the
//! pull request comment.
//!
//! A report quotes a directive's reason, a file name and what a forge answered. In the
//! two Markdown formats that text is escaped so that `*`, `_`, `~` and `[` are literal,
//! and a word a renderer would link by itself (`http://...`, `www....`) is a code span.
//! The report's own markup (its bold labels, its gate links) still renders, and the
//! terminal report is as it was.
//!
//! Each hostile text is written between two markers, so the test reads exactly the span
//! the report quoted. Every forge here is a `FakeForge` on a loopback port; the harness
//! sets `DISCIPLINE_NO_NETWORK=1`, so nothing else can be reached.

mod common;
use common::{FakeForge, Repo, Run};

const START: &str = "QSTART634";
const END: &str = "QEND634";

/// Emphasis, strikethrough, a link, an image, and words a renderer links by itself.
const MARKUP: &str = "**bold** *it* _it_ __strong__ ~~gone~~ [text](h) ![i](h) see http://h.example.invalid/a_b and https://h.example.invalid/*c* or www.h.example.invalid ftp://h.example.invalid/x mailto:x.y a*b*c";

/// The same in a file name: no slash, and no colon (a path a checkout can hold).
#[cfg(unix)]
const MARKUP_FILE: &str =
    "tests/QSTART634 **bold** _it_ ~~gone~~ [text] www.h.example.invalid QEND634_x.rs";

const WAIVER_PREFIX: &str = "allow-agent-instructions: AGENTS.md ";

const TWO_ASSERTS: &str = "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 3, 4);\n}\n";
const NO_ASSERTS: &str = "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n";

/// The gate links a report writes itself.
const OWN_LINK: &str = "](https://orieg.github.io/discipline/gates/#";

// ---------------------------------------------------------------------------------------
// Reading a Markdown report.
// ---------------------------------------------------------------------------------------

/// The part of a Markdown line outside every code span, read as CommonMark reads it:
/// what a renderer treats as live. An escaped character is dropped, since it is literal.
fn live(line: &str) -> String {
    let c: Vec<char> = line.chars().collect();
    let run_at = |i: usize| c[i..].iter().take_while(|x| **x == '`').count();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && i + 1 < c.len() && c[i + 1].is_ascii_punctuation() {
            i += 2;
        } else if c[i] == '`' {
            let run = run_at(i);
            let mut j = i + run;
            let mut close = None;
            while j < c.len() {
                if c[j] == '`' {
                    let r = run_at(j);
                    if r == run {
                        close = Some(j);
                        break;
                    }
                    j += r;
                } else {
                    j += 1;
                }
            }
            match close {
                Some(j) => i = j + run,
                None => {
                    out.push_str(&"`".repeat(run));
                    i += run;
                }
            }
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// Every span of `markdown` between the two markers, with the line it is on.
fn quoted_spans(markdown: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in markdown.lines() {
        let mut rest = line;
        while let Some(at) = rest.find(START) {
            let after = &rest[at + START.len()..];
            let Some(end) = after.find(END) else { break };
            found.push((after[..end].to_string(), line.to_string()));
            rest = &after[end + END.len()..];
        }
    }
    found
}

/// What is live in the quoted spans of `markdown`: emphasis, a link, an image, a code
/// span left open, HTML, or a word a renderer links by itself. Empty when the quoted
/// text is text and nothing else. `spans` is how many spans must have been read.
fn live_markup_in_quoted_spans(markdown: &str, spans: usize) -> Vec<String> {
    let quoted = quoted_spans(markdown);
    let mut found = Vec::new();
    if quoted.len() < spans {
        found.push(format!(
            "{} quoted span(s) found, {spans} expected",
            quoted.len()
        ));
    }
    for (span, line) in quoted {
        // The line is read whole, so a code span that starts before the marker or ends
        // after it is seen as one; the quoted part of what is live is then cut out.
        let live_line = live(&line);
        let live_span = match (live_line.find(START), live_line.find(END)) {
            (Some(a), Some(b)) if a < b => live_line[a + START.len()..b].to_string(),
            // The markers are inside a code span: nothing of the span is live.
            _ => String::new(),
        };
        for c in ['*', '_', '~', '`', '<', '['] {
            if live_span.contains(c) {
                found.push(format!("a live `{c}` in `{span}` of: {line}"));
            }
        }
        let lower = live_span.to_ascii_lowercase();
        for trigger in ["://", "www.", "mailto:"] {
            if lower.contains(trigger) {
                found.push(format!("a live `{trigger}` in `{span}` of: {line}"));
            }
        }
    }
    found
}

/// The first line of `text` that starts with `prefix`.
fn line_starting<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines().find(|l| l.starts_with(prefix)).unwrap_or("")
}

// ---------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------

/// A change to an agent-instruction file, which `instruction-smuggling` reports unless a
/// directive waives it.
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
    format!("Reviewed.\n\n{WAIVER_PREFIX}{START} {MARKUP} {END}\n")
}

/// A pull request event for `repo`, written beside it.
fn pull_request_event(repo: &Repo) -> std::path::PathBuf {
    let event = repo.path().join("..").join(format!(
        "event-{}.json",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        &event,
        r#"{"pull_request":{"number":7,"user":{"login":"dev"},"head":{"sha":"abc"},"title":"t (#7)"}}"#,
    )
    .unwrap();
    event
}

/// `check --comment` on a pull request event; returns the run and the comment written.
fn comment_run(repo: &Repo, pr_body: &str) -> (Run, String) {
    let event = pull_request_event(repo);
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

/// `check` writing a job summary; returns the run and the summary.
fn summary_run(repo: &Repo, env: &[(&str, &str)]) -> (Run, String) {
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("summary.md");
    let mut all = vec![("GITHUB_STEP_SUMMARY", path.to_str().unwrap())];
    all.extend_from_slice(env);
    let run = repo.run(
        &["check", "--base", "main", "--format", "github-summary"],
        &all,
    );
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    (run, written)
}

// ---------------------------------------------------------------------------------------
// A directive's reason.
// ---------------------------------------------------------------------------------------

#[test]
fn a_directive_reason_renders_no_emphasis_or_link_in_the_job_summary() {
    let repo = instruction_change();
    let body = waiver_body();
    let (run, summary) = summary_run(&repo, &[("PR_BODY", body.as_str())]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        live_markup_in_quoted_spans(&summary, 1),
        Vec::<String>::new(),
        "{summary}"
    );
    // The row is still one row of five cells, and it still says what the reason said.
    let row = line_starting(
        &summary,
        "| `instruction-smuggling` | `allow-agent-instructions` | ",
    );
    assert_eq!(row.replace("\\|", "").matches('|').count(), 6, "{row}");
    assert!(row.contains("\\*\\*bold\\*\\*"), "{row}");
    assert!(row.contains("`http://h.example.invalid/a_b`"), "{row}");
    assert!(row.contains("`www.h.example.invalid`"), "{row}");
    // The report's own markup is as it was.
    assert!(summary.contains("**Summary:**"), "{summary}");
    assert!(
        summary.starts_with("### Discipline gate: passed"),
        "{summary}"
    );
}

#[test]
fn a_directive_reason_renders_no_emphasis_or_link_in_the_pull_request_comment() {
    let repo = instruction_change();
    let (run, comment) = comment_run(&repo, &waiver_body());
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        live_markup_in_quoted_spans(&comment, 1),
        Vec::<String>::new(),
        "{comment}"
    );
    let item = line_starting(&comment, "- `instruction-smuggling` on ");
    assert!(item.contains("\\*\\*bold\\*\\*"), "{comment}");
    assert!(item.contains("`www.h.example.invalid`"), "{comment}");
    assert!(
        comment.contains("**Findings lifted by an override:**"),
        "{comment}"
    );
}

#[test]
fn the_terminal_report_writes_a_directive_reason_as_it_is() {
    let repo = instruction_change();
    let body = waiver_body();
    let run = repo.run(
        &["check", "--base", "main", "--format", "terminal"],
        &[("PR_BODY", body.as_str())],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let applied = line_starting(&run.stdout, "  · override applied: ");
    assert!(
        applied.contains(&format!("{START} {MARKUP} {END}")),
        "{}",
        run.stdout
    );
    // The JSON report carries it as it is too.
    let json = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("PR_BODY", body.as_str())],
    );
    assert!(
        json.stdout.contains(&format!("{START} {MARKUP} {END}")),
        "{}",
        json.stdout
    );
}

// ---------------------------------------------------------------------------------------
// A file name from the change.
// ---------------------------------------------------------------------------------------

#[cfg(unix)]
fn weakened_test_in_a_file_named_with_markup() -> Repo {
    let repo = Repo::new();
    repo.commit_base(MARKUP_FILE, TWO_ASSERTS, "test: add");
    repo.write(MARKUP_FILE, NO_ASSERTS);
    repo.commit("test: simplify");
    repo
}

#[cfg(unix)]
#[test]
fn a_file_name_renders_no_emphasis_or_link_in_the_job_summary() {
    let repo = weakened_test_in_a_file_named_with_markup();
    let (run, summary) = summary_run(&repo, &[("PR_BODY", "")]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        live_markup_in_quoted_spans(&summary, 1),
        Vec::<String>::new(),
        "{summary}"
    );
    // The finding's row keeps the report's own link to the gate and its own emphasis.
    let rows: Vec<&str> = summary
        .lines()
        .filter(|l| l.starts_with("| Error |"))
        .collect();
    assert!(!rows.is_empty(), "{summary}");
    for row in &rows {
        assert_eq!(row.matches(OWN_LINK).count(), 1, "{row}");
        assert!(row.contains("| **"), "{row}");
    }
}

#[cfg(unix)]
#[test]
fn a_file_name_renders_no_emphasis_or_link_in_the_pull_request_comment() {
    let repo = weakened_test_in_a_file_named_with_markup();
    let (run, comment) = comment_run(&repo, "");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        live_markup_in_quoted_spans(&comment, 1),
        Vec::<String>::new(),
        "{comment}"
    );
    assert!(
        comment.lines().any(|l| l.starts_with("| error |")),
        "{comment}"
    );
}

// ---------------------------------------------------------------------------------------
// What a forge answered.
// ---------------------------------------------------------------------------------------

#[test]
fn a_forge_refusal_renders_no_emphasis_or_link_in_the_job_summary() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("a.txt", "a\n");
    repo.commit("chore: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("b.txt", "b\n");
    repo.commit("chore: local only");
    let sha = repo.git_output(&["rev-parse", "HEAD"]).trim().to_string();
    let api = FakeForge::start();
    api.serve_raw(
        &format!("repos/o/r/commits/{sha}/pulls"),
        403,
        &[],
        &serde_json::json!({ "message": format!("{START} {MARKUP} {END}") }).to_string(),
    );
    let url = api.url();
    let (run, summary) = summary_run(
        &repo,
        &[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("PR_BODY", ""),
        ],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        live_markup_in_quoted_spans(&summary, 1),
        Vec::<String>::new(),
        "{summary}"
    );
    let line = line_starting(&summary, "**Directives:** ");
    assert!(line.contains("\\*\\*bold\\*\\*"), "{summary}");
    // The report's own code span in that line still renders as one.
    assert!(line.contains("(`directives.degrade_offline`)"), "{summary}");
}

// ---------------------------------------------------------------------------------------
// A ref name the comment writes in a code span of its own.
// ---------------------------------------------------------------------------------------

#[test]
fn a_base_ref_that_reads_as_an_address_is_one_code_span_in_the_pull_request_comment() {
    // A branch name cannot hold `://`; it can be `www.` and a host.
    let repo = instruction_change();
    repo.git(&["branch", "www.h.example.invalid", "main"]);
    let event = pull_request_event(&repo);
    let api = FakeForge::start();
    api.serve(
        "repos/o/r/issues/7/comments?per_page=100&page=1",
        serde_json::json!([]),
    );
    api.serve_raw("repos/o/r/issues/7/comments", 201, &[], r#"{"id": 900}"#);
    let url = api.url();
    let run = repo.run(
        &[
            "check",
            "--base",
            "www.h.example.invalid",
            "--format",
            "json",
            "--comment",
        ],
        &[
            ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GITHUB_TOKEN", "t"),
            ("PR_BODY", ""),
        ],
    );
    let writes = api.writes();
    assert_eq!(writes.len(), 1, "{writes:?}\n{}{}", run.stdout, run.stderr);
    let body: serde_json::Value = serde_json::from_str(&writes[0].2).unwrap();
    let comment = body["body"].as_str().unwrap();
    let line = line_starting(comment, "Base ");
    assert!(line.contains("www.h.example.invalid"), "{comment}");
    let live_line = live(line);
    assert!(!live_line.contains("www."), "{line}");
    assert!(!live_line.contains('`'), "{line}");
    assert!(live_line.starts_with("Base  · "), "{line}");
}
