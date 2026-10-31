//! The review-routing section (#944) through the real binary: what the JSON report
//! carries, where each human surface puts it (after the verdict, zero rows left out,
//! the deterministic and heuristic halves apart, pointers capped), and that it is the
//! same with the network switched off.

mod common;

use common::Repo;
use serde_json::Value;

/// A base with a Python tool that already spawns a process, then a change that adds a
/// workflow, a Rust process spawn, a Python network call beside the old spawn, a test,
/// an inline marker, and carries a directive in its commit body.
fn routed_change() -> Repo {
    let repo = Repo::new();
    repo.commit_base(
        "tools/old.py",
        "import os\nos.system('make')\n",
        "chore: a tool that already spawns",
    );
    repo.write(
        ".github/workflows/extra.yml",
        "on: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n",
    );
    repo.write(
        "src/spawn.rs",
        "use std::process::Command;\n\npub fn run() {\n    let _ = Command::new(\"ls\").status();\n}\n",
    );
    repo.write(
        "tools/old.py",
        "import os\nos.system('make')\nimport requests\nrequests.get('https://example.invalid')\n",
    );
    repo.write(
        "tests/b.rs",
        "#[test]\nfn added() {\n    assert_eq!(1 + 1, 2);\n}\n",
    );
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1 then Phase 2.\n\nSee host-a. <!-- discipline:allow(pii) -->\n",
    );
    repo.commit("feat: routed change\n\nno-issue: a fixture for the routing section");
    repo
}

fn routing(report: &Value) -> &Value {
    let r = &report["review_routing"];
    assert!(r.is_object(), "no review_routing:\n{report:#}");
    r
}

#[test]
fn the_json_report_carries_both_halves_and_the_coverage() {
    let repo = routed_change();
    let run = repo.check(&[]);
    assert_ne!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let report = run.json();
    let r = routing(&report);
    assert!(r["stability"]
        .as_str()
        .unwrap()
        .starts_with("unstable until 1.0"));

    let d = &r["deterministic"];
    assert_eq!(d["directives"], 1, "{r:#}");
    assert_eq!(d["directive_kinds"], serde_json::json!(["no-issue"]));
    assert_eq!(d["inline_markers_added"], 1, "{r:#}");
    assert_eq!(d["inline_markers_removed"], 0);
    assert_eq!(
        d["authority_files"],
        serde_json::json!([{
            "path": ".github/workflows/extra.yml",
            "surface": "ci",
            "change": "added"
        }])
    );

    let h = &r["heuristic"];
    // The base's `os.system` is on a line the change did not add: not listed.
    assert_eq!(
        h["capability_sites"],
        serde_json::json!([
            { "file": "src/spawn.rs", "line": 4, "surface": "process-spawn" },
            { "file": "tools/old.py", "line": 4, "surface": "network" }
        ]),
        "{r:#}"
    );
    assert_eq!(h["tests_added"], 1, "{r:#}");
    assert_eq!(h["tests_removed"], 0);

    let c = &r["coverage"];
    // Five files changed; the Rust, Python and Rust test files are in an inventoried
    // language, the workflow and the Markdown are not.
    assert_eq!(c["changed_files"], 5, "{r:#}");
    assert_eq!(c["inventoried_files"], 3, "{r:#}");
    assert_eq!(c["unparsed_files"], 0);
    let languages: Vec<&str> = c["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(languages.contains(&"Rust") && languages.contains(&"Python"));

    // The routing section is no gate: it adds no outcome and no finding code.
    for o in report["outcomes"].as_array().unwrap() {
        assert!(!o["gate"].as_str().unwrap().contains("routing"));
    }

    // The same section with the network switched off.
    let offline = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("DISCIPLINE_NO_NETWORK", "1")],
    );
    assert_eq!(routing(&offline.json()), r);
}

#[test]
fn the_terminal_report_prints_it_after_the_verdict() {
    let repo = routed_change();
    let run = repo.run(&["check", "--base", "main"], &[]);
    let out = &run.stdout;
    let status = out.find("Status: ").expect("a verdict");
    let section = out.find("review routing: ").expect("the section");
    assert!(section > status, "{out}");
    let tail = &out[section..];
    let det = tail
        .find("deterministic (exact")
        .expect("deterministic half");
    let heu = tail
        .find("heuristic (non-exhaustive")
        .expect("heuristic half");
    assert!(det < heu, "{tail}");
    assert!(tail.contains("src/spawn.rs:4 (process-spawn)"), "{tail}");
    assert!(tail.contains("runtime semantics not inspected"), "{tail}");
    // Zero rows are left out.
    assert!(
        !tail.contains("removed\n    tests") && !tail.contains(": 0 "),
        "{tail}"
    );
}

#[test]
fn the_job_summary_opens_with_the_counts_and_the_step_output_carries_them() {
    let repo = routed_change();
    let dir = tempfile::tempdir().unwrap();
    let summary = dir.path().join("summary.md");
    let output = dir.path().join("output");
    repo.run(
        &["check", "--format", "github-summary", "--base", "main"],
        &[
            ("GITHUB_STEP_SUMMARY", summary.to_str().unwrap()),
            ("GITHUB_OUTPUT", output.to_str().unwrap()),
        ],
    );
    let md = std::fs::read_to_string(&summary).unwrap();
    let mut lines = md.lines().filter(|l| !l.trim().is_empty());
    assert!(lines.next().unwrap().starts_with("### Discipline gate: "));
    let counts = lines.next().unwrap();
    assert!(
        counts.starts_with("**review routing: deterministic: directives 1"),
        "{md}"
    );
    assert!(md.contains("#### Review routing"), "{md}");
    assert!(md.contains("**Deterministic**") && md.contains("**Heuristic**"));
    let outputs = std::fs::read_to_string(&output).unwrap();
    let line = outputs
        .lines()
        .find(|l| l.starts_with("review_routing="))
        .expect("the step output");
    assert!(line.contains("capability sites 2"), "{line}");
}

#[test]
fn the_agent_prompt_quotes_pointers_as_data_and_names_no_directive() {
    let repo = routed_change();
    let run = repo.run(
        &["check", "--format", "agent-prompt", "--base", "main"],
        &[],
    );
    let out = &run.stdout;
    assert!(out.contains("Review routing (for the reviewer"), "{out}");
    assert!(out.contains("`src/spawn.rs:4` (process-spawn)"), "{out}");
    assert!(!out.contains("no-issue"), "{out}");
}

/// The hook relays the section with the findings it blocks on, its pointers quoted as
/// data and no directive named, and logs the counts on its line (#937's record).
#[test]
fn the_hook_relays_the_section_and_logs_its_counts() {
    use std::io::Write as _;
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    repo.write(
        "src/spawn.rs",
        "use std::process::Command;\n\npub fn run() {\n    let _ = Command::new(\"ls\").status();\n}\n",
    );
    // An untracked file is not part of the change until git knows of it.
    repo.git(&["add", "-N", "src/spawn.rs"]);
    let mut cmd = common::discipline_cmd(repo.path());
    cmd.args(["hook", "run", "--agent", "claude-code"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"hook_event_name":"PostToolUse","tool_name":"Edit"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    let findings = stderr.find("### Issue 1").expect("the finding");
    let section = stderr
        .find("Review routing (for the reviewer")
        .expect("the section");
    assert!(findings < section, "{stderr}");
    assert!(
        stderr.contains("`src/spawn.rs:4` (process-spawn)"),
        "{stderr}"
    );

    let log = std::fs::read_to_string(repo.path().join(".git/discipline/hook-observe.log"))
        .expect("the hook log");
    let line: Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(
        line["routing"]["heuristic"]["capability_sites"], 1,
        "{line}"
    );
    assert_eq!(
        line["routing"]["deterministic"]["authority_files"], 0,
        "{line}"
    );
}

#[test]
fn a_change_with_no_file_prints_no_section() {
    let repo = Repo::new();
    let run = repo.run(&["check", "--base", "main"], &[]);
    assert!(!run.stdout.contains("review routing"), "{}", run.stdout);
    let json = repo.check(&[]).json();
    assert_eq!(json["review_routing"]["coverage"]["changed_files"], 0);
}
