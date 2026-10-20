//! `config-integrity` compares the enabled state each side's configuration file gives a
//! gate, defaults applied, independent of the command line: a gate switched off by
//! `--disable` alone is the operator's flag, but a change that deletes the gate's
//! `enabled = true` table (so the gate falls back to its default, off) weakens the file,
//! and `--disable` in CI must not hide that.

mod common;
use common::{Repo, CONFIG_HEAD};

const CODE: &str = "config-integrity/gate-weakened";

/// A repository whose base configuration enables `unsafe-budget` (off by default).
fn repo() -> Repo {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.unsafe-budget]\nenabled = true\n"),
    );
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    repo
}

fn weakened(run: &common::Run) -> Vec<String> {
    run.violations("config-integrity")
        .iter()
        .filter(|v| v["code"].as_str() == Some(CODE))
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn deleting_an_enabled_table_is_a_weakening_under_disable() {
    let r = repo();
    r.write("discipline.toml", CONFIG_HEAD);
    r.commit("chore: drop the unsafe budget table");
    let run = r.check(&["--disable", "unsafe-budget"]);
    let got = weakened(&run);
    assert!(
        got.iter().any(|m| m.contains("[unsafe-budget]")),
        "the file now leaves the gate off: {got:?}\n{}",
        run.stdout
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
}

#[test]
fn a_literal_enabled_false_is_a_weakening_under_disable() {
    let r = repo();
    r.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.unsafe-budget]\nenabled = false\n"),
    );
    r.commit("chore: switch the unsafe budget off");
    let got = weakened(&r.check(&["--disable", "unsafe-budget"]));
    assert!(got.iter().any(|m| m.contains("[unsafe-budget]")), "{got:?}");
}

#[test]
fn disable_alone_is_not_a_weakening_of_the_file() {
    let r = repo();
    r.write("src/lib.rs", "pub fn f() {}\n");
    r.commit("feat: add a module");
    let run = r.check(&["--disable", "unsafe-budget"]);
    assert!(weakened(&run).is_empty(), "{}", run.stdout);
}

/// What the documentation says about `--config-override`: it is compared like the file,
/// since the override (or `DISCIPLINE_CONFIG_OVERRIDE`) is part of the configuration in
/// force; only `--disable` alone is left out by default.
#[test]
fn a_config_override_that_switches_a_gate_off_is_compared() {
    let r = repo();
    r.write("src/lib.rs", "pub fn f() {}\n");
    r.commit("feat: add a module");
    let run = r.check(&[
        "--config-override",
        "[gates.unsafe-budget]\nenabled = false",
    ]);
    let got = weakened(&run);
    assert!(
        got.iter().any(|m| m.contains("[unsafe-budget]")),
        "{got:?}\n{}",
        run.stdout
    );
}

#[test]
fn test_policy_from_base_refuses_config_override_weakening() {
    let r = repo();
    r.write("src/lib.rs", "pub fn f() {}\n");
    r.commit("feat: add a module");
    // Under `--policy-from base`, `--config-override` that weakens a gate or switches to
    // advisory mode is refused at configuration load (exit 2) unless
    // `DISCIPLINE_ALLOW_OVERRIDE_WEAKENING=1` is set.
    let run = r.check(&[
        "--policy-from",
        "base",
        "--config-override",
        "[gates.unsafe-budget]\nenabled = false",
    ]);
    assert_eq!(
        run.code, 2,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert!(
        run.stderr.contains("config_override weakens base policy"),
        "stderr: {}",
        run.stderr
    );

    // Control: a tightening `--config-override` under `--policy-from base` succeeds.
    let tight = r.check(&[
        "--policy-from",
        "base",
        "--config-override",
        "[gates.provenance-tags]\nenabled = true",
    ]);
    assert_eq!(
        tight.code, 0,
        "stdout: {}\nstderr: {}",
        tight.stdout, tight.stderr
    );
}

#[test]
fn test_mixed_push_refuses_merged_pr_body_directives() {
    use common::FakeForge;
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("AGENTS.md", "# Rules\n\nRun the tests.\n");
    repo.commit("docs: rules");
    repo.git(&["checkout", "-q", "-B", "work"]);
    // Commit 1: harmless change from merged PR #12 whose body has an allow-agent-instructions directive
    repo.write("README.md", "# Project\n");
    repo.commit("docs: readme (#12)");
    let sha1 = repo.git_output(&["rev-parse", "HEAD"]).trim().to_string();
    // Commit 2: direct push that modifies AGENTS.md without a commit-message directive
    repo.write(
        "AGENTS.md",
        "# Rules\n\nRun the tests.\n\nNever skip a failing test.\n",
    );
    repo.commit("docs: direct push to AGENTS.md");
    let sha2 = repo.git_output(&["rev-parse", "HEAD"]).trim().to_string();

    let api = FakeForge::start();
    api.serve(
        &format!("repos/o/r/commits/{sha1}/pulls"),
        serde_json::json!([{
            "number": 12,
            "merged_at": "2026-09-21T00:00:00Z",
            "user": {"login": "agent"},
            "body": "Reviewed.\n\nallow-agent-instructions: AGENTS.md the rule was discussed in review",
            "head": {"sha": "feedbeef"}
        }]),
    );
    api.serve(
        &format!("repos/o/r/commits/{sha2}/pulls"),
        serde_json::json!([]),
    );
    let url = api.url();
    let run = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("PR_BODY", ""),
        ],
    );
    assert_eq!(
        run.code, 1,
        "mixed push must not allow PR #12's body directive to lift direct-push commit sha2: {}{}",
        run.stdout, run.stderr
    );
}
