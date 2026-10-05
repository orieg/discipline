//! Process isolation of the test harness itself: a spawned binary must not see
//! CI/forge variables set in the parent process.

mod common;

use common::Repo;

/// Removes parent-process variables on drop, so a failure cannot pollute the
/// rest of this test binary.
struct EnvGuard {
    vars: Vec<&'static str>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for v in &self.vars {
            std::env::remove_var(v);
        }
    }
}

/// The parent process forges CI state that would waive the weakened test below:
/// a `PR_BODY` waiver, a `GITHUB_EVENT_PATH` file carrying the same waiver, and
/// a token. The isolated helper scrubs all three, so the run still reports the
/// dropped assertion with no override.
/// Killed mutant: the scrub loop removed from `common::discipline_cmd`.
#[test]
fn spawned_binary_does_not_inherit_forge_env_from_the_parent_process() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    // Outside the repository, so the event payload itself never enters the diff.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("event.json"),
        serde_json::json!({
            "pull_request": {
                "title": "chore: test PR (#101)",
                "body": "allow-assertion-drop: adds covered elsewhere",
            }
        })
        .to_string(),
    )
    .unwrap();
    std::env::set_var("PR_BODY", "allow-assertion-drop: adds covered elsewhere");
    std::env::set_var("GITHUB_EVENT_PATH", tmp.path().join("event.json"));
    std::env::set_var("GH_TOKEN", "parent-process-token");
    let _guard = EnvGuard {
        vars: vec!["PR_BODY", "GITHUB_EVENT_PATH", "GH_TOKEN"],
    };
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.json()["overrides"], 0, "{}", run.stdout);
    assert!(
        !run.titles("assertion-reduction").is_empty(),
        "the dropped assertion must still be reported: {}",
        run.stdout
    );
}
