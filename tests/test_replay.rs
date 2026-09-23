//! `discipline replay` through the real binary: a history of merged changes replayed
//! under a configuration, with and without the pull request body.

mod common;

use common::{FakeForge, Repo};
use serde_json::Value;

const WEAKENED: &str = "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n";

/// `main` gains two squash-merged changes: #2 weakens a test, #3 adds a doc.
fn history() -> (Repo, String) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("tests/a.rs", WEAKENED);
    repo.commit("test: simplify adds (#2)");
    let weakening = repo.git_output(&["rev-parse", "HEAD"]).trim().to_string();
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes (#3)");
    (repo, weakening)
}

fn replay(repo: &Repo, extra: &[&str], env: &[(&str, &str)]) -> Value {
    let mut args = vec!["replay", "--last", "2", "--ref", "main", "--json"];
    args.extend_from_slice(extra);
    let run = repo.run(&args, env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    serde_json::from_str(&run.stdout).unwrap()
}

fn verdicts(s: &Value) -> Vec<(u64, String)> {
    s["cases_detail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["pr"].as_u64().unwrap(),
                c["verdict"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn replay_reports_what_the_configuration_would_have_blocked() {
    let (repo, _) = history();
    let status_before = repo.git_output(&["status", "--porcelain"]);
    let head_before = repo.git_output(&["rev-parse", "HEAD"]);
    let objects_before = repo.git_output(&["count-objects"]);

    let s = replay(&repo, &[], &[]);
    assert_eq!(
        verdicts(&s),
        [(3, "passed".to_string()), (2, "blocked".to_string())]
    );
    assert_eq!(
        s["errors_by_gate"]["assertion-reduction"],
        serde_json::json!(["#2"])
    );
    assert!(s["cases_detail"][1]["directives_from"]
        .as_str()
        .unwrap()
        .starts_with("commit message only"));

    // A candidate configuration without the gate would have let #2 through.
    let cfg = repo.file("candidate.toml");
    std::fs::write(
        &cfg,
        "[meta]\nversion = 1\nname = \"t\"\n[gates.assertion-reduction]\nenabled = false\n",
    )
    .unwrap();
    let lenient = replay(&repo, &["--config", cfg.to_str().unwrap()], &[]);
    assert_eq!(lenient["blocked"], 0, "{lenient}");
    std::fs::remove_file(&cfg).unwrap();

    // Neither replay touched the source repository: no worktree change, no moved HEAD,
    // no new objects (the candidate configuration's blob lives in the throwaway store).
    assert_eq!(repo.git_output(&["status", "--porcelain"]), status_before);
    assert_eq!(repo.git_output(&["rev-parse", "HEAD"]), head_before);
    assert_eq!(repo.git_output(&["count-objects"]), objects_before);

    let text = repo.run(&["replay", "--last", "2", "--ref", "main"], &[]);
    assert!(
        text.stdout
            .contains("2 changes: 1 passed, 1 blocked, 0 could not be checked"),
        "{}",
        text.stdout
    );
}

#[test]
fn replay_reads_the_waiver_in_the_merged_pull_request_body() {
    let (repo, weakening) = history();
    let api = FakeForge::start();
    api.serve(
        &format!("repos/o/r/commits/{weakening}/pulls"),
        serde_json::json!([{
            "number": 12,
            "merged_at": "2026-09-21T00:00:00Z",
            "user": {"login": "dev"},
            "body": "Simplified.\n\nallow-assertion-drop: adds the second check moved to an integration test",
            "head": {"sha": "feedbeef"}
        }]),
    );
    let url = api.url();
    let s = replay(
        &repo,
        &[],
        &[
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            // A replay inside a CI job that comments must not comment on anything.
            ("DISCIPLINE_COMMENT", "1"),
            ("GITHUB_TOKEN", "t"),
        ],
    );
    assert!(api.writes().is_empty(), "{:?}", api.writes());
    let case = &s["cases_detail"][1];
    assert_eq!(case["pr"], 12, "{s}");
    assert_eq!(case["directives_from"], "pull request body");
    assert_eq!(case["verdict"], "passed", "{s}");
}
