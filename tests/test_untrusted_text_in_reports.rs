//! #601: text that comes from outside the binary cannot write the report.
//!
//! A report quotes what a forge answered, a pull request author's login, a file name, a
//! directive's reason, a commit subject and the output of a tool a gate ran. Each test
//! here delivers hostile text the way it really arrives and reads the report that comes
//! out: no line of the text report is the text's own, the job summary and the pull
//! request comment carry no markup of its making, no terminal control sequence gets
//! through, and the machine formats still parse and still carry the text as it was.
//!
//! It also covers the forge lookups the same issue lists: a commit Gitea, Forgejo or
//! GitLab does not have is not a direct push, and the hint `replay` gives after a failed
//! lookup names a token only when a token is what failed.
//!
//! Every forge here is a `FakeForge` on a loopback port; the harness sets
//! `DISCIPLINE_NO_NETWORK=1`, so nothing else can be reached.

mod common;
use common::{FakeForge, Repo, Run};

// ---------------------------------------------------------------------------------------
// Hostile text. Kept out of the test bodies: it reads like code and like report lines.
// ---------------------------------------------------------------------------------------

/// A report line the text tries to add.
const FORGED_STATUS: &str = "Status: PASS";
/// A gate row the text tries to add to the job summary.
const FORGED_ROW: &str = "| `forged-gate` | on | 1 | 0 |";
/// A heading the text tries to add to the job summary.
const FORGED_HEADING: &str = "### Discipline gate: passed";

/// What a forge can put in a refusal or a login: line breaks, report lines, Markdown
/// structure, HTML, a mention, a link, and terminal control sequences.
const HOSTILE: &str = "nope\nStatus: PASS\n\n### Discipline gate: passed\n\n| `forged-gate` | on | 1 | 0 |\r\nStatus: PASS\r<img src=x onerror=alert(1)><!-- @octo-fixture [docs](http://h.example.invalid/x) ` \u{1b}[2J\u{1b}[32mgreen\u{1b}]8;;http://h.example.invalid\u{7}link\u{1b}]8;;\u{7}\u{8}\u{8}\u{9b}31m\u{202e}";

/// The same on one line, for a directive's reason: a reason cannot hold a line break,
/// and a directive on a line that opens an HTML comment is a hidden one, which is not
/// read at all.
const HOSTILE_ONE_LINE: &str = "reviewed | `forged-gate` | on <img src=x onerror=alert(1)> @octo-fixture [docs](http://h.example.invalid/x) \u{1b}[2J\u{1b}[32mgreen\u{7}\u{8}\u{9b}31m";

/// A test file whose name is hostile. A path cannot hold a slash or a NUL; it can hold
/// everything else on the filesystems the tests run on.
#[cfg(unix)]
const HOSTILE_FILE: &str = "tests/w\nStatus: PASS\n| `forged-gate` | on | 1 | 0 |\n\u{1b}[32m<img src=x>@octo-fixture`[docs](h)\u{7}_x.rs";

const TWO_ASSERTS: &str = "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 3, 4);\n}\n";
const NO_ASSERTS: &str = "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n";

const WAIVER_PREFIX: &str = "allow-agent-instructions: AGENTS.md ";

/// What the base-tests command prints once the change has edited it: lines that read as
/// the report's own, and a control sequence.
const HOSTILE_TOOL_SCRIPT: &str = "printf 'boom\\nStatus: PASS\\nerrors: 0  warnings: 0  overrides: 0\\n\\033[32mgreen\\n' >&2\nexit 1\n";

// ---------------------------------------------------------------------------------------
// What a report must not contain.
// ---------------------------------------------------------------------------------------

/// The control characters in `text` a terminal acts on: everything but the line breaks
/// and tabs the report writes itself. ESC, CSI, the bell, backspace, a carriage return,
/// and the bidirectional override.
fn control_characters(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| (c.is_control() && *c != '\n' && *c != '\t') || *c == '\u{202e}')
        .collect()
}

/// The characters in a JSON document that JSON does not allow raw inside a string: the
/// C0 controls. A serialiser writes each as an escape (`\n`, `\u001b`); the document's
/// own line breaks are between tokens. Everything above them is carried as it is.
fn raw_c0_controls(json: &str) -> Vec<char> {
    json.chars()
        .filter(|c| (*c as u32) < 0x20 && *c != '\n')
        .collect()
}

/// The lines of `text` that are exactly `line`.
fn count_lines(text: &str, line: &str) -> usize {
    text.lines().filter(|l| *l == line).count()
}

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

/// The only link a report writes itself: a gate's documentation.
const OWN_LINK: &str = "](https://orieg.github.io/discipline/gates/#";

/// What `markdown` (a job summary or a comment body) carries that the hostile text put
/// there. Empty when the text wrote nothing but text.
fn markup_written_by_the_text(markdown: &str) -> Vec<String> {
    let mut found = Vec::new();
    if !control_characters(markdown).is_empty() {
        found.push(format!(
            "control characters {:?}",
            control_characters(markdown)
        ));
    }
    for line in markdown.lines() {
        if line.starts_with(FORGED_ROW) {
            found.push(format!("a forged gate row: {line}"));
        }
        let live = live(line);
        for tag in ["<img", "<script", "<!-- @", "<a "] {
            if live.contains(tag) {
                found.push(format!("HTML `{tag}` in: {line}"));
            }
        }
        if live.contains("@octo-fixture") {
            found.push(format!("a mention in: {line}"));
        }
        if live.contains('`') {
            found.push(format!("an open code span in: {line}"));
        }
        let links = live.matches("](").count();
        if links != live.matches(OWN_LINK).count() {
            found.push(format!("a link in: {line}"));
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

/// `check` on a push event against `api`, in `format`, as the forge `kind`.
fn push_run(repo: &Repo, api: &FakeForge, kind: &str, format: &str, extra: &[(&str, &str)]) -> Run {
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
    env.extend_from_slice(extra);
    repo.run(&["check", "--base", "main", "--format", format], &env)
}

/// The same run writing a job summary; returns the run and the summary.
fn summary_run(repo: &Repo, api: &FakeForge, kind: &str) -> (Run, String) {
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("summary.md");
    let run = push_run(
        repo,
        api,
        kind,
        "github-summary",
        &[("GITHUB_STEP_SUMMARY", path.to_str().unwrap())],
    );
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    (run, written)
}

fn directive_notes(run: &Run) -> Vec<String> {
    run.json()["directive_notes"]
        .as_array()
        .map(|a| a.iter().map(|n| n.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

/// The merged pull request lookup of `sha` on the forge `kind`.
fn pulls_path(kind: &str, sha: &str) -> String {
    match kind {
        "github" => format!("repos/o/r/commits/{sha}/pulls"),
        "gitlab" => format!("projects/o%2Fr/repository/commits/{sha}/merge_requests"),
        _ => format!("repos/o/r/commits/{sha}/pull"),
    }
}

/// The endpoint that answers for commit `sha` itself, on the forges that are asked.
fn commit_path(kind: &str, sha: &str) -> String {
    match kind {
        "gitlab" => format!("projects/o%2Fr/repository/commits/{sha}"),
        _ => format!("repos/o/r/git/commits/{sha}"),
    }
}

fn refusal(key: &str, text: &str) -> String {
    serde_json::json!({ key: text }).to_string()
}

// ---------------------------------------------------------------------------------------
// A forge's refusal message.
// ---------------------------------------------------------------------------------------

#[test]
fn a_forge_refusal_cannot_add_a_line_to_the_text_report() {
    let (repo, sha) = plain_change();
    // Each forge's field for its reason, on the forge that uses it.
    for (kind, key) in [
        ("github", "message"),
        ("gitea", "message"),
        ("gitlab", "error_description"),
        ("forgejo", "error"),
    ] {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 403, &[], &refusal(key, HOSTILE));
        let run = push_run(&repo, &api, kind, "terminal", &[]);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        // The run passes, so the report says so once; the refusal adds no second line.
        assert_eq!(
            count_lines(&run.stdout, FORGED_STATUS),
            1,
            "{kind}: a forged status line:\n{}",
            run.stdout
        );
        let notes: Vec<&str> = run
            .stdout
            .lines()
            .filter(|l| l.starts_with("directives: "))
            .collect();
        assert_eq!(notes.len(), 1, "{kind}:\n{}", run.stdout);
        // The whole refusal is on that line, to its end.
        assert!(
            notes[0].contains("nope")
                && notes[0].ends_with("continuing without it (`directives.degrade_offline`)"),
            "{kind}: {}",
            notes[0]
        );
        assert_eq!(
            control_characters(&run.stdout),
            Vec::<char>::new(),
            "{kind}:\n{:?}",
            run.stdout
        );
    }
}

#[test]
fn a_forge_refusal_cannot_write_markup_into_the_job_summary() {
    let (repo, sha) = plain_change();
    for (kind, key) in [("github", "message"), ("gitlab", "error_description")] {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 403, &[], &refusal(key, HOSTILE));
        let (run, summary) = summary_run(&repo, &api, kind);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(
            markup_written_by_the_text(&summary),
            Vec::<String>::new(),
            "{kind}:\n{summary}"
        );
        assert_eq!(
            summary
                .lines()
                .filter(|l| l.starts_with("**Directives:** "))
                .count(),
            1,
            "{kind}:\n{summary}"
        );
        assert_eq!(
            count_lines(&summary, FORGED_HEADING),
            1,
            "{kind}:\n{summary}"
        );
        // What the forge said is still there to read, as text.
        let note = line_starting(&summary, "**Directives:** ");
        assert!(
            note.contains("&lt;img src=x onerror=alert(1)&gt;")
                && note.contains("&#64;octo-fixture"),
            "{kind}: {note}"
        );
    }
}

#[test]
fn a_forge_refusal_cannot_drive_the_terminal_from_an_error_message() {
    let (repo, _) = plain_change();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\ndegrade_offline = false\n",
    );
    repo.commit("chore: require the record");
    let api = FakeForge::start();
    for c in [head(&repo), repo.git_output(&["rev-parse", "HEAD~1"])] {
        api.serve_raw(
            &pulls_path("github", c.trim()),
            403,
            &[],
            &refusal("message", HOSTILE),
        );
    }
    let run = push_run(&repo, &api, "github", "terminal", &[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("merged-pr-body: cannot resolve") && run.stderr.contains("nope"),
        "{}",
        run.stderr
    );
    assert_eq!(
        control_characters(&run.stderr),
        Vec::<char>::new(),
        "{:?}",
        run.stderr
    );
    assert_eq!(count_lines(&run.stderr, FORGED_STATUS), 0, "{}", run.stderr);
}

/// A configuration the change wrote, which is not valid TOML: a raw escape sequence and
/// a bell inside a string. The parse error quotes the line.
const CONFIG_WITH_AN_ESCAPE: &str = "[meta]\nversion = 1\nname = \"t\u{1b}[2J\u{7}\"\n";

#[test]
fn a_configuration_error_cannot_drive_the_terminal() {
    let repo = Repo::new();
    repo.write("discipline.toml", CONFIG_WITH_AN_ESCAPE);
    repo.commit("chore: configure");
    let run = repo.run(&["check", "--base", "main", "--format", "terminal"], &[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    // The excerpt is still under the error, on its own lines, with the line it quotes.
    assert!(
        run.stderr.contains("is not valid TOML at line 3")
            && run
                .stderr
                .contains("\n3 | name = \"t\u{fffd}[2J\u{fffd}\"\n"),
        "{:?}",
        run.stderr
    );
    assert_eq!(
        control_characters(&run.stderr),
        Vec::<char>::new(),
        "{:?}",
        run.stderr
    );
}

// ---------------------------------------------------------------------------------------
// A pull request author's login.
// ---------------------------------------------------------------------------------------

fn merged_pull(author: &str) -> serde_json::Value {
    serde_json::json!([{
        "number": 12,
        "merged_at": "2026-09-21T00:00:00Z",
        "user": {"login": author},
        "body": "Reviewed.",
        "head": {"sha": "feedbeef"}
    }])
}

#[test]
fn an_author_login_cannot_add_a_line_to_the_text_report() {
    let (repo, sha) = plain_change();
    let api = FakeForge::start();
    api.serve(&pulls_path("github", &sha), merged_pull(HOSTILE));
    let run = push_run(&repo, &api, "github", "terminal", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(count_lines(&run.stdout, FORGED_STATUS), 1, "{}", run.stdout);
    assert_eq!(
        run.stdout
            .lines()
            .filter(|l| l.starts_with("directives: "))
            .count(),
        2,
        "{}",
        run.stdout
    );
    assert_eq!(
        control_characters(&run.stdout),
        Vec::<char>::new(),
        "{:?}",
        run.stdout
    );
    // Something was there, and the reader can see it.
    assert!(
        line_starting(&run.stdout, "directives: merged-pr-body: ")
            .contains("nope\u{fffd}Status: PASS"),
        "{}",
        run.stdout
    );
}

#[test]
fn an_author_login_cannot_write_markup_into_the_job_summary() {
    let (repo, sha) = plain_change();
    let api = FakeForge::start();
    api.serve(&pulls_path("github", &sha), merged_pull(HOSTILE));
    let (run, summary) = summary_run(&repo, &api, "github");
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        markup_written_by_the_text(&summary),
        Vec::<String>::new(),
        "{summary}"
    );
    assert_eq!(count_lines(&summary, FORGED_HEADING), 1, "{summary}");
    assert_eq!(
        summary
            .lines()
            .filter(|l| l.starts_with("**Directives:** "))
            .count(),
        2,
        "{summary}"
    );
}

#[test]
fn the_json_report_carries_an_author_login_as_the_forge_sent_it() {
    let (repo, sha) = plain_change();
    let api = FakeForge::start();
    api.serve(&pulls_path("github", &sha), merged_pull(HOSTILE));
    let run = push_run(&repo, &api, "github", "json", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    // The serialiser encodes; it does not rewrite. A consumer gets the login back.
    let notes = directive_notes(&run);
    assert_eq!(
        notes,
        vec![
            format!("0 directive(s) read from merged pull request #12 (author {HOSTILE})"),
            format!(
                "merged-pr-body: commit {} arrived through merged pull request #12 (author {HOSTILE})",
                &sha[..10]
            ),
        ]
    );
    // And the document itself has no raw control character in it: each is an escape.
    assert_eq!(
        raw_c0_controls(&run.stdout),
        Vec::<char>::new(),
        "{:?}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------------------
// A file name from the change.
// ---------------------------------------------------------------------------------------

/// A test that loses its assertions, in a file whose name is hostile, and the event of
/// the pull request that carries it.
#[cfg(unix)]
fn weakened_test_in_a_hostile_file() -> (Repo, std::path::PathBuf) {
    let repo = Repo::new();
    repo.commit_base(HOSTILE_FILE, TWO_ASSERTS, "test: add");
    repo.write(HOSTILE_FILE, NO_ASSERTS);
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

#[cfg(unix)]
fn pull_request_run(
    repo: &Repo,
    event: &std::path::Path,
    api: &FakeForge,
    args: &[&str],
    env: &[(&str, &str)],
) -> Run {
    let url = api.url();
    let mut all = vec![
        ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
        ("GITHUB_REPOSITORY", "o/r"),
        ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ("GITHUB_TOKEN", "t"),
        ("PR_BODY", ""),
    ];
    all.extend_from_slice(env);
    let mut argv = vec!["check", "--base", "main"];
    argv.extend_from_slice(args);
    repo.run(&argv, &all)
}

#[cfg(unix)]
#[test]
fn a_file_name_cannot_add_a_line_to_the_text_report() {
    let (repo, event) = weakened_test_in_a_hostile_file();
    let api = FakeForge::start();
    let run = pull_request_run(&repo, &event, &api, &["--format", "terminal"], &[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    // The run failed: no line of its report says it passed.
    assert_eq!(count_lines(&run.stdout, FORGED_STATUS), 0, "{}", run.stdout);
    assert_eq!(
        count_lines(&run.stdout, "Status: FAILED"),
        1,
        "{}",
        run.stdout
    );
    assert_eq!(
        control_characters(&run.stdout),
        Vec::<char>::new(),
        "{:?}",
        run.stdout
    );
}

#[cfg(unix)]
#[test]
fn a_file_name_cannot_write_markup_into_the_job_summary() {
    let (repo, event) = weakened_test_in_a_hostile_file();
    let api = FakeForge::start();
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("summary.md");
    let run = pull_request_run(
        &repo,
        &event,
        &api,
        &["--format", "github-summary"],
        &[("GITHUB_STEP_SUMMARY", path.to_str().unwrap())],
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let summary = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        markup_written_by_the_text(&summary),
        Vec::<String>::new(),
        "{summary}"
    );
    assert_eq!(count_lines(&summary, FORGED_STATUS), 0, "{summary}");
    // One row for the finding, and the heading says the run failed.
    assert_eq!(
        summary
            .lines()
            .filter(|l| l.starts_with("| Error |"))
            .count(),
        1,
        "{summary}"
    );
    assert!(
        summary.starts_with("### Discipline gate: FAILED"),
        "{summary}"
    );
    // The workflow commands printed with the summary each stay one line too.
    assert_eq!(
        control_characters(&run.stdout),
        Vec::<char>::new(),
        "{:?}",
        run.stdout
    );
    assert_eq!(count_lines(&run.stdout, FORGED_STATUS), 0, "{}", run.stdout);
}

#[cfg(unix)]
#[test]
fn a_file_name_cannot_write_markup_into_the_pull_request_comment() {
    let (repo, event) = weakened_test_in_a_hostile_file();
    let api = FakeForge::start();
    api.serve(
        "repos/o/r/issues/7/comments?per_page=100&page=1",
        serde_json::json!([]),
    );
    api.serve_raw("repos/o/r/issues/7/comments", 201, &[], r#"{"id": 900}"#);
    let run = pull_request_run(&repo, &event, &api, &["--format", "json", "--comment"], &[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let writes = api.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    let body: serde_json::Value = serde_json::from_str(&writes[0].2).unwrap();
    let body = body["body"].as_str().unwrap();
    assert_eq!(
        markup_written_by_the_text(body),
        Vec::<String>::new(),
        "{body}"
    );
    assert_eq!(
        body.matches("<!-- discipline:report -->").count(),
        1,
        "{body}"
    );
    assert_eq!(count_lines(body, FORGED_STATUS), 0, "{body}");
    assert_eq!(
        body.lines().filter(|l| l.starts_with("| error |")).count(),
        1,
        "{body}"
    );
}

/// The hostile file's findings in each machine format: `(json, sarif, junit, gitlab)`.
#[cfg(unix)]
fn machine_reports() -> (String, String, String, String) {
    let (repo, event) = weakened_test_in_a_hostile_file();
    let api = FakeForge::start();
    let out = tempfile::tempdir().unwrap();
    let file = |name: &str| out.path().join(name).to_str().unwrap().to_string();
    let (sarif, junit, gitlab) = (file("r.sarif"), file("r.xml"), file("r.json"));
    let run = pull_request_run(
        &repo,
        &event,
        &api,
        &[
            "--format",
            "json",
            "--report-sarif",
            &sarif,
            "--report-junit",
            &junit,
            "--report-gitlab",
            &gitlab,
        ],
        &[],
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let read = |p: &str| std::fs::read_to_string(p).unwrap();
    (run.stdout, read(&sarif), read(&junit), read(&gitlab))
}

/// Every string in `v`, at any depth.
fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

#[cfg(unix)]
#[test]
fn the_json_report_parses_and_carries_a_hostile_file_name_as_it_is() {
    let (json, _, _, _) = machine_reports();
    let report: serde_json::Value = serde_json::from_str(&json).expect("the report is JSON");
    let mut all = Vec::new();
    strings(&report, &mut all);
    assert!(all.iter().any(|s| s == HOSTILE_FILE), "{json}");
    assert_eq!(raw_c0_controls(&json), Vec::<char>::new(), "{json:?}");
}

#[cfg(unix)]
#[test]
fn the_sarif_report_parses_and_carries_a_hostile_file_name() {
    let (_, sarif, _, _) = machine_reports();
    let report: serde_json::Value = serde_json::from_str(&sarif).expect("the report is JSON");
    assert_eq!(report["version"], "2.1.0", "{sarif}");
    let results = report["runs"][0]["results"].as_array().unwrap();
    assert!(!results.is_empty(), "{sarif}");
    let mut all = Vec::new();
    strings(&report, &mut all);
    // The message names the file as it is; the location is a URI and encodes it.
    assert!(
        all.iter().any(|s| s.contains("@octo-fixture`[docs](h)")),
        "{sarif}"
    );
    assert_eq!(raw_c0_controls(&sarif), Vec::<char>::new(), "{sarif:?}");
}

#[cfg(unix)]
#[test]
fn the_gitlab_report_parses_and_carries_a_hostile_file_name_as_it_is() {
    let (_, _, _, gitlab) = machine_reports();
    let report: serde_json::Value = serde_json::from_str(&gitlab).expect("the report is JSON");
    let issues = report.as_array().unwrap();
    assert!(!issues.is_empty(), "{gitlab}");
    assert!(
        issues.iter().any(|i| i["location"]["path"] == HOSTILE_FILE),
        "{gitlab}"
    );
    assert_eq!(raw_c0_controls(&gitlab), Vec::<char>::new(), "{gitlab:?}");
}

/// The element names of `xml` in document order, `/name` for an end tag: a walk that
/// fails on markup the document's own structure does not account for.
fn xml_tags(xml: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find('<') {
        let after = &rest[at + 1..];
        let end = after.find('>').unwrap_or(after.len());
        let tag = &after[..end];
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '?' | '!'))
            .collect();
        if !tag.ends_with('/') || name.starts_with('/') {
            tags.push(name.clone());
        } else {
            tags.push(name.clone());
            tags.push(format!("/{name}"));
        }
        rest = &after[end.min(after.len())..];
    }
    tags
}

/// Whether `tags` open and close in order.
fn balanced(tags: &[String]) -> bool {
    let mut open: Vec<&str> = Vec::new();
    for t in tags.iter().filter(|t| !t.starts_with('?')) {
        match t.strip_prefix('/') {
            Some(name) => {
                if open.pop() != Some(name) {
                    return false;
                }
            }
            None => open.push(t),
        }
    }
    open.is_empty()
}

#[cfg(unix)]
#[test]
fn the_junit_report_is_well_formed_with_a_hostile_file_name() {
    let (_, _, junit, _) = machine_reports();
    let tags = xml_tags(&junit);
    assert!(balanced(&tags), "{tags:?}\n{junit}");
    // Only the elements the report writes: the file name added none.
    let mut names: Vec<&str> = tags.iter().map(|t| t.trim_start_matches('/')).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names,
        [
            "?xml",
            "failure",
            "properties",
            "property",
            "skipped",
            "system-out",
            "testcase",
            "testsuite",
            "testsuites"
        ],
        "{junit}"
    );
    // Every character is one XML 1.0 allows (ESC and the bell are not).
    assert!(
        junit.chars().all(|c| matches!(c as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)),
        "{junit:?}"
    );
    assert!(junit.contains("&lt;img src=x&gt;@octo-fixture"), "{junit}");
}

// ---------------------------------------------------------------------------------------
// A directive's reason, written in the pull request body.
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

#[test]
fn a_directive_reason_cannot_drive_the_terminal_or_write_markup() {
    let repo = instruction_change();
    let body = format!("Reviewed.\n\n{WAIVER_PREFIX}{HOSTILE_ONE_LINE}\n");
    let text = repo.run(
        &["check", "--base", "main", "--format", "terminal"],
        &[("PR_BODY", body.as_str())],
    );
    assert_eq!(text.code, 0, "{}{}", text.stdout, text.stderr);
    let applied = line_starting(&text.stdout, "  · override applied: ");
    assert!(
        applied.contains("reviewed | `forged-gate`"),
        "{}",
        text.stdout
    );
    assert_eq!(
        control_characters(&text.stdout),
        Vec::<char>::new(),
        "{:?}",
        text.stdout
    );

    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("summary.md");
    let run = repo.run(
        &["check", "--base", "main", "--format", "github-summary"],
        &[
            ("PR_BODY", body.as_str()),
            ("GITHUB_STEP_SUMMARY", path.to_str().unwrap()),
        ],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let summary = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        markup_written_by_the_text(&summary),
        Vec::<String>::new(),
        "{summary}"
    );
    // The overrides table has its header, its rule and one row of five cells.
    let row = line_starting(
        &summary,
        "| `instruction-smuggling` | `allow-agent-instructions` | ",
    );
    assert!(
        row.contains("&lt;img src=x onerror=alert(1)&gt;"),
        "{summary}"
    );
    let cells = row.replace("\\|", "").matches('|').count();
    assert_eq!(cells, 6, "{row}");
}

// ---------------------------------------------------------------------------------------
// The output of a tool a gate ran.
// ---------------------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn tool_output_quoted_in_a_finding_cannot_add_a_line_to_the_text_report() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("run.sh", "exit 0\n");
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[gates.command]\npreset = \"base-tests\"\ncommand = \"sh run.sh\"\n",
    );
    repo.commit("ci: run the base tests");
    repo.git(&["checkout", "-q", "-B", "work"]);
    // The change edits what the command runs; the command line itself is the base's.
    repo.write("run.sh", HOSTILE_TOOL_SCRIPT);
    repo.commit("chore: edit the runner");

    let json = repo.run(&["check", "--base", "main", "--format", "json"], &[]);
    assert_eq!(json.code, 1, "{}{}", json.stdout, json.stderr);
    let quoted: Vec<String> = json
        .violations("command")
        .iter()
        .map(|v| v["message"].as_str().unwrap().to_string())
        .collect();
    // The JSON report carries the output as the tool wrote it.
    assert!(
        quoted.iter().any(|m| m
            .contains("boom\nStatus: PASS\nerrors: 0  warnings: 0  overrides: 0\n\u{1b}[32mgreen")),
        "{quoted:?}"
    );

    let text = repo.run(&["check", "--base", "main", "--format", "terminal"], &[]);
    assert_eq!(text.code, 1, "{}{}", text.stdout, text.stderr);
    assert_eq!(
        count_lines(&text.stdout, FORGED_STATUS),
        0,
        "{}",
        text.stdout
    );
    assert_eq!(
        count_lines(&text.stdout, "errors: 0  warnings: 0  overrides: 0"),
        0,
        "{}",
        text.stdout
    );
    // The output is still there, under its finding.
    assert_eq!(
        count_lines(&text.stdout, "   Status: PASS"),
        1,
        "{}",
        text.stdout
    );
    assert_eq!(
        control_characters(&text.stdout),
        Vec::<char>::new(),
        "{:?}",
        text.stdout
    );
}

// ---------------------------------------------------------------------------------------
// A commit subject and a file name in `replay` and `audit`.
// ---------------------------------------------------------------------------------------

const HOSTILE_SUBJECT: &str = "docs: notes \u{1b}[2J\u{1b}[32mpassed\u{7}\u{8} (#3)";

#[test]
fn a_commit_subject_cannot_drive_the_terminal_from_the_replay_summary() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit(HOSTILE_SUBJECT);
    let run = repo.run(&["replay", "--last", "1", "--ref", "main"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("docs: notes "), "{}", run.stdout);
    assert_eq!(
        control_characters(&run.stdout),
        Vec::<char>::new(),
        "{:?}",
        run.stdout
    );
}

/// A source file whose name holds a line break, a report line and a tag.
#[cfg(unix)]
const HOSTILE_SOURCE: &str = "src/m\nStatus: PASS\n\u{1b}[32m<img src=x>'\"x.rs";

#[cfg(unix)]
const MARKED: &str = "fn a() {} // discipline:allow(time-estimates) a quoted release plan\n";

/// A history whose last change adds an inline marker in a hostile-named file.
#[cfg(unix)]
fn history_with_a_marker_in_a_hostile_file() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://forge.example.invalid/o/r.git",
    ]);
    repo.write(HOSTILE_SOURCE, MARKED);
    repo.commit("feat: a (#2)");
    repo
}

#[cfg(unix)]
#[test]
fn a_file_name_cannot_add_a_line_to_the_audit_summary() {
    let repo = history_with_a_marker_in_a_hostile_file();
    let run = repo.run(&["audit", "--last", "1", "--ref", "main"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("inline-marker"), "{}", run.stdout);
    assert_eq!(count_lines(&run.stdout, FORGED_STATUS), 0, "{}", run.stdout);
    assert_eq!(
        control_characters(&run.stdout),
        Vec::<char>::new(),
        "{:?}",
        run.stdout
    );
}

#[cfg(unix)]
#[test]
fn a_file_name_cannot_leave_an_element_or_an_attribute_of_the_audit_page() {
    let repo = history_with_a_marker_in_a_hostile_file();
    let out = repo.file("audit.html");
    let run = repo.run(
        &[
            "audit",
            "--last",
            "1",
            "--ref",
            "main",
            "--format",
            "html",
            "--output",
            out.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let page = std::fs::read_to_string(&out).unwrap();
    // The name is on the page, as text.
    assert!(page.contains("&lt;img src=x&gt;&#39;&quot;x.rs"), "{page}");
    assert!(!page.contains("<img"), "{page}");
    // No attribute holds a raw quote from the name: every `x.rs` follows an escaped one.
    assert_eq!(
        page.matches("x.rs").count(),
        page.matches("&quot;x.rs").count() + page.matches("%22x.rs").count(),
        "{page}"
    );
}

// ---------------------------------------------------------------------------------------
// A commit Gitea, Forgejo or GitLab does not have is not a direct push.
// ---------------------------------------------------------------------------------------

const NOT_FOUND: &str = r#"{"message":"The target couldn't be found."}"#;

#[test]
fn a_404_for_the_pull_request_is_read_by_asking_for_the_commit() {
    let (repo, sha) = plain_change();
    let short = &sha[..10];
    for kind in ["gitea", "forgejo", "gitlab"] {
        // The forge has the commit: a direct push.
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
        api.serve(&commit_path(kind, &sha), serde_json::json!({"sha": sha}));
        let run = push_run(&repo, &api, kind, "json", &[]);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(
            directive_notes(&run),
            vec![format!(
                "merged-pr-body: commit {short} arrived through no merged pull request (direct push); its message is the only directive source"
            )],
            "{kind}"
        );
        let asked: Vec<String> = api.requests().into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            asked,
            vec![pulls_path(kind, &sha), commit_path(kind, &sha)],
            "{kind}"
        );

        // The forge does not have the commit: said so, as on GitHub.
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
        api.serve_raw(&commit_path(kind, &sha), 404, &[], NOT_FOUND);
        let run = push_run(&repo, &api, kind, "json", &[]);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(
            directive_notes(&run),
            vec![format!(
                "merged-pr-body: commit {short} is not on {kind} (a local commit), so no merged pull request carries it; its message is the only directive source"
            )],
            "{kind}"
        );

        // The forge refuses to say: a lookup that failed.
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &sha), 404, &[], NOT_FOUND);
        api.serve_raw(
            &commit_path(kind, &sha),
            403,
            &[],
            r#"{"message":"token lacks scope"}"#,
        );
        let run = push_run(&repo, &api, kind, "json", &[]);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        let notes = directive_notes(&run);
        assert_eq!(notes.len(), 1, "{kind}: {notes:?}");
        assert!(
            notes[0].starts_with(&format!(
                "merged-pr-body: cannot resolve the merged pull request of commit {short} on {kind} ("
            )) && notes[0].contains("HTTP 403 token lacks scope")
                && notes[0].ends_with("continuing without it (`directives.degrade_offline`)"),
            "{kind}: {notes:?}"
        );
    }
}

#[test]
fn a_commit_lookup_that_fails_stops_a_run_that_requires_the_record() {
    let (repo, _) = plain_change();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\ndegrade_offline = false\n",
    );
    repo.commit("chore: require the record");
    let commits = [head(&repo), repo.git_output(&["rev-parse", "HEAD~1"])];
    for kind in ["gitea", "gitlab"] {
        let api = FakeForge::start();
        for c in &commits {
            api.serve_raw(&pulls_path(kind, c.trim()), 404, &[], NOT_FOUND);
            api.serve_raw(&commit_path(kind, c.trim()), 403, &[], "{}");
        }
        let run = push_run(&repo, &api, kind, "json", &[]);
        assert_eq!(run.code, 2, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(run.could_not_check(), ("forge".to_string(), None), "{kind}");

        // A commit the forge says it does not have is an answer, not a failure.
        let api = FakeForge::start();
        for c in &commits {
            api.serve_raw(&pulls_path(kind, c.trim()), 404, &[], NOT_FOUND);
            api.serve_raw(&commit_path(kind, c.trim()), 404, &[], NOT_FOUND);
        }
        let run = push_run(&repo, &api, kind, "json", &[]);
        assert_eq!(run.code, 0, "{kind}: {}{}", run.stdout, run.stderr);
        assert_eq!(directive_notes(&run).len(), 2, "{kind}");
    }
}

#[test]
fn a_pull_request_that_is_found_asks_for_nothing_more() {
    let (repo, sha) = plain_change();
    let api = FakeForge::start();
    api.serve(
        &pulls_path("gitea", &sha),
        serde_json::json!({"number": 5, "merged": true, "user": {"login": "agent"}, "body": "", "head": {"sha": "h5"}}),
    );
    let run = push_run(&repo, &api, "gitea", "json", &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let asked: Vec<String> = api.requests().into_iter().map(|(p, _)| p).collect();
    assert_eq!(asked, vec![pulls_path("gitea", &sha)]);
}

// ---------------------------------------------------------------------------------------
// `replay`: a commit the forge does not have, and the hint after a failed lookup.
// ---------------------------------------------------------------------------------------

/// `main` gains two squash-merged changes: #2 weakens a test, #3 adds a doc. Returns the
/// repository and the commit of #2.
fn history() -> (Repo, String) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("tests/a.rs", NO_ASSERTS);
    repo.commit("test: simplify adds (#2)");
    let weakening = head(&repo);
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes (#3)");
    (repo, weakening)
}

/// The detail `replay` gives for #2, the change that would be blocked.
fn replay_detail(repo: &Repo, env: &[(&str, &str)]) -> (String, String) {
    let run = repo.run(&["replay", "--last", "2", "--ref", "main", "--json"], env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let s: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let case = &s["cases_detail"][1];
    (
        case["verdict"].as_str().unwrap().to_string(),
        case["detail"].as_str().unwrap_or("").to_string(),
    )
}

/// A token no forge issued, to find in output it must not reach.
const FIXTURE_TOKEN: &str = "fixture-token-value-601";

#[test]
fn replay_does_not_read_a_commit_gitea_does_not_have_as_a_direct_push() {
    let (repo, weakening) = history();
    let tip = head(&repo);
    let api = FakeForge::start();
    for c in [&weakening, &tip] {
        api.serve_raw(&pulls_path("gitea", c), 404, &[], NOT_FOUND);
        api.serve_raw(&commit_path("gitea", c), 404, &[], NOT_FOUND);
    }
    let url = api.url();
    let (verdict, detail) = replay_detail(
        &repo,
        &[
            ("DISCIPLINE_FORGE", "gitea"),
            ("DISCIPLINE_FORGE_URL", "http://127.0.0.1"),
            ("DISCIPLINE_FORGE_REPO", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ],
    );
    assert_eq!(verdict, "could_not_check", "{detail}");
    assert_eq!(
        detail,
        format!(
            "its merged pull request could not be read, and its body may carry directives: the forge does not have commit {} (not pushed, or another repository)",
            &weakening[..10]
        )
    );
}

#[test]
fn the_replay_hint_names_a_token_only_when_a_token_is_what_failed() {
    let (repo, weakening) = history();
    let pulls = pulls_path("github", &weakening);
    let base = |url: &str| -> Vec<(String, String)> {
        vec![
            ("GITHUB_REPOSITORY".to_string(), "o/r".to_string()),
            ("DISCIPLINE_FORGE_API_URL".to_string(), url.to_string()),
        ]
    };
    let detail_of = |api: &FakeForge, token: Option<(&str, &str)>| -> String {
        let mut env = base(&api.url());
        if let Some((k, v)) = token {
            env.push((k.to_string(), v.to_string()));
        }
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (verdict, detail) = replay_detail(&repo, &env);
        assert_eq!(verdict, "could_not_check", "{detail}");
        detail
    };

    // Refused, and no token was sent: the variables GitHub's token is read from.
    let api = FakeForge::start();
    api.serve_raw(&pulls, 403, &[], r#"{"message":"Resource not accessible"}"#);
    let refused = detail_of(&api, None);
    assert!(
        refused.ends_with(
            "(no token was sent: set a token that can read pull requests in DISCIPLINE_FORGE_TOKEN, GH_TOKEN or GITHUB_TOKEN)"
        ),
        "{refused}"
    );

    // Refused with a token: the token is the problem, and it is not printed.
    let with_token = detail_of(&api, Some(("GH_TOKEN", FIXTURE_TOKEN)));
    assert!(
        with_token.contains("the token in use cannot read this repository's pull requests"),
        "{with_token}"
    );
    assert!(!with_token.contains(FIXTURE_TOKEN), "{with_token}");
    assert!(!with_token.contains(&FIXTURE_TOKEN[..12]), "{with_token}");

    // Rate limited without a token.
    let api = FakeForge::start();
    api.serve_raw(
        &pulls,
        403,
        &[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "99999999999"),
        ],
        r#"{"message":"API rate limit exceeded"}"#,
    );
    let limited = detail_of(&api, None);
    assert!(
        limited.contains("the forge limits requests without one"),
        "{limited}"
    );
    let limited_with_token = detail_of(&api, Some(("DISCIPLINE_FORGE_TOKEN", FIXTURE_TOKEN)));
    assert!(
        limited_with_token.contains("the request limit of the token in use is spent"),
        "{limited_with_token}"
    );
    assert!(
        !limited_with_token.contains(FIXTURE_TOKEN),
        "{limited_with_token}"
    );

    // Not about a token: an answer the client cannot use, and a commit that is missing.
    let api = FakeForge::start();
    api.serve_raw(&pulls, 422, &[], r#"{"message":"Validation Failed"}"#);
    let unusable = detail_of(&api, None);
    assert!(
        unusable.ends_with("without saying the commit is missing"),
        "{unusable}"
    );
    assert!(!unusable.to_lowercase().contains("token"), "{unusable}");

    let api = FakeForge::start();
    api.serve_raw(
        &pulls,
        422,
        &[],
        r#"{"message":"No commit found for SHA: 0"}"#,
    );
    let missing = detail_of(&api, None);
    assert!(
        missing.ends_with("(not pushed, or another repository)"),
        "{missing}"
    );
    assert!(!missing.to_lowercase().contains("token"), "{missing}");
}

#[test]
fn the_replay_hint_names_the_variables_of_the_forge_that_was_asked() {
    let (repo, weakening) = history();
    for (kind, variables) in [
        ("gitea", "DISCIPLINE_FORGE_TOKEN or GITEA_TOKEN"),
        (
            "forgejo",
            "DISCIPLINE_FORGE_TOKEN, FORGEJO_TOKEN or GITEA_TOKEN",
        ),
        ("gitlab", "DISCIPLINE_FORGE_TOKEN or GITLAB_TOKEN"),
    ] {
        let api = FakeForge::start();
        api.serve_raw(&pulls_path(kind, &weakening), 401, &[], "{}");
        let url = api.url();
        let (verdict, detail) = replay_detail(
            &repo,
            &[
                ("DISCIPLINE_FORGE", kind),
                ("DISCIPLINE_FORGE_URL", "http://127.0.0.1"),
                ("DISCIPLINE_FORGE_REPO", "o/r"),
                ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ],
        );
        assert_eq!(verdict, "could_not_check", "{kind}: {detail}");
        assert!(
            detail.ends_with(&format!(
                "(no token was sent: set a token that can read pull requests in {variables})"
            )),
            "{kind}: {detail}"
        );
    }
}
