//! `check --max-report-lines N` cuts only the terminal report on stdout (#891).
mod common;

use common::Repo;

fn repo_with_a_finding() -> Repo {
    let repo = Repo::new();
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1 takes 2 weeks. Phase 2 takes 3 days.\n",
    );
    repo.commit("docs: plan");
    repo
}

#[test]
fn max_report_lines_cuts_the_terminal_report_and_keeps_the_json_report_whole() {
    let repo = repo_with_a_finding();
    let full = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "terminal",
            "--fail-on-warnings",
        ],
        &[("NO_COLOR", "1")],
    );
    assert_eq!(full.code, 1, "{}\n{}", full.stdout, full.stderr);
    let full_lines = full.stdout.lines().count();
    assert!(full_lines > 4, "report too short to cut:\n{}", full.stdout);

    let json_path = repo.file("report.json");
    let cut = repo.run(
        &[
            "check",
            "--fail-on-warnings",
            "--base",
            "main",
            "--format",
            "terminal",
            "--max-report-lines",
            "3",
            "--json-out",
            json_path.to_str().unwrap(),
        ],
        &[("NO_COLOR", "1")],
    );
    assert_eq!(cut.code, 1, "the exit code does not change");
    let lines: Vec<&str> = cut.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "3 report lines and one note:\n{}",
        cut.stdout
    );
    assert_eq!(
        lines[3],
        format!(
            "... {} more report lines not shown (--max-report-lines 3); \
             --json-out and --output-file keep the full report",
            full_lines - 3
        )
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert!(
        json.to_string().contains("time-estimates"),
        "JSON report must stay whole: {json}"
    );
}

#[test]
fn max_report_lines_leaves_a_machine_format_on_stdout_whole() {
    let repo = repo_with_a_finding();
    let run = repo.run(
        &[
            "check",
            "--fail-on-warnings",
            "--base",
            "main",
            "--format",
            "json",
            "--max-report-lines",
            "1",
        ],
        &[],
    );
    assert_eq!(run.code, 1);
    assert!(run.json()["summary"].is_object() || run.json().is_object());
    assert!(!run.stdout.contains("more report lines not shown"));
}
