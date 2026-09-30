//! `discipline audit` through the real binary: a history of merged changes, each
//! carrying one kind of escape hatch.

mod common;

use common::Repo;
use serde_json::Value;

const CONFIG: &str = "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nexempt_paths = [\"a/**\"]\n";

/// `main` gains five squash-merged changes after a base that adopts a configuration:
/// #2 carries directives, #3 grows `exempt_paths`, #4 adds an inline marker, #5 grows the
/// baseline, #6 carries nothing.
fn history() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG);
    repo.commit("chore: adopt discipline (#1)");
    repo.write("src/a.rs", "pub fn a() {}\n");
    repo.commit(
        "feat: a (#2)\n\nno-issue: release bookkeeping\nallow-dependency: serde AKIA-SECRET parser\n",
    );
    repo.write(
        "discipline.toml",
        &CONFIG.replace("[\"a/**\"]", "[\"a/**\", \"b/**\"]"),
    );
    repo.commit("chore: exempt b (#3)");
    repo.write(
        "src/b.rs",
        "pub const HOST: &str = \"h\"; // discipline:allow(pii): fixture host\n// \"discipline:allow(pii)\" in prose is not a marker\n",
    );
    repo.commit("feat: b (#4)");
    repo.write(
        "discipline-baseline.toml",
        "version = 2\n[[findings]]\ngate = \"stub-bodies\"\nrule = \"r\"\npath = \"src/a.rs\"\nfingerprint = \"\"\n",
    );
    repo.commit("chore: grandfather (#5)");
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes (#6)");
    repo
}

fn audit(repo: &Repo, extra: &[&str]) -> Value {
    let mut args = vec!["audit", "--last", "5", "--ref", "main", "--json"];
    args.extend_from_slice(extra);
    let run = repo.run(&args, &[("DISCIPLINE_NO_NETWORK", "1")]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    serde_json::from_str(&run.stdout).unwrap()
}

fn rows(s: &Value) -> Vec<(u64, String, String)> {
    s["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["pr"].as_u64().unwrap(),
                r["kind"].as_str().unwrap().to_string(),
                r["gate"].as_str().unwrap_or("-").to_string(),
            )
        })
        .collect()
}

#[test]
fn each_kind_of_escape_hatch_is_recorded_against_its_change() {
    let repo = history();
    let s = audit(&repo, &[]);
    assert_eq!(s["changes"], 5, "{s:#}");
    assert_eq!(s["changes_with_records"], 4, "{s:#}");
    let row = |pr: u64, kind: &str, gate: &str| (pr, kind.to_string(), gate.to_string());
    assert_eq!(
        rows(&s),
        vec![
            row(5, "baseline", "stub-bodies"),
            row(4, "inline-marker", "pii"),
            row(3, "config", "pii"),
            row(2, "directive", "issue-link"),
            row(2, "directive", "dependency-delta"),
        ],
        "{s:#}"
    );
    let records = s["records"].as_array().unwrap();
    let config = &records[2];
    assert_eq!(config["key"], "exempt_paths");
    assert_eq!(config["change"], "gained");
    assert_eq!(config["count"], 1);
    assert_eq!(
        (config["evidence"].as_str(), config["tier"].as_str()),
        (Some("applied"), Some("A"))
    );
    let marker = &records[1];
    assert_eq!(
        (marker["file"].as_str(), marker["line"].as_u64()),
        (Some("src/b.rs"), Some(1))
    );
    let waiver = &records[3];
    assert_eq!(waiver["class"], "process");
    assert_eq!(
        (waiver["evidence"].as_str(), waiver["tier"].as_str()),
        (Some("claimed"), Some("C"))
    );
    assert_eq!(records[4]["class"], "detector");
    assert_eq!(s["by_class"]["process"], 1);
    assert_eq!(s["by_gate"]["pii"], 2);
    // The reason is a hash and a length unless asked for.
    assert!(!s.to_string().contains("AKIA"), "{s:#}");
    assert_eq!(records[4]["reason_sha256"].as_str().unwrap().len(), 64);
    let with_reasons = audit(&repo, &["--reasons"]);
    assert_eq!(
        with_reasons["records"][4]["reason"],
        "serde AKIA-SECRET parser"
    );
}

#[test]
fn an_unreadable_historical_configuration_is_a_record_not_a_failure() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nretired_option = true\n",
    );
    repo.commit("chore: adopt (#1)");
    repo.write("discipline.toml", "[meta]\nversion = 1\nname = \"t\"\n");
    repo.commit("chore: drop the retired option (#2)");
    let run = repo.run(&["audit", "--last", "1", "--ref", "main", "--json"], &[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let s: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(s["records"][0]["kind"], "config-unreadable", "{s:#}");
    assert!(s["records"][0]["detail"]
        .as_str()
        .unwrap()
        .starts_with("parent: "));
}

#[test]
fn the_text_output_lists_each_record_and_the_totals() {
    let repo = history();
    let run = repo.run(&["audit", "--last", "5", "--ref", "main"], &[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout
            .contains("5 changes audited, 4 carried an exception (5 records)"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("#3        config"), "{}", run.stdout);
    assert!(run.stdout.contains("exempt_paths gained"), "{}", run.stdout);
    assert!(!run.stdout.contains("AKIA"), "{}", run.stdout);
}

#[test]
fn a_ref_that_does_not_resolve_is_exit_2() {
    let repo = history();
    let run = repo.run(&["audit", "--last", "1", "--ref", "no-such-branch"], &[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("no-such-branch"), "{}", run.stderr);
    let zero = repo.run(&["audit", "--last", "0"], &[]);
    assert_eq!(zero.code, 2, "{}", zero.stderr);
}

fn signal_ids(s: &Value) -> Vec<String> {
    s["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn signals_rank_what_needs_a_decision_and_a_tightening_restores_a_loosening() {
    let repo = history();
    let s = audit(&repo, &[]);
    assert_eq!(
        signal_ids(&s),
        vec![
            "loosening-without-waiver",
            "loosened-not-restored",
            "baseline-grew"
        ],
        "{s:#}"
    );
    let first = &s["signals"][0];
    assert_eq!(first["changes"], serde_json::json!(["#3"]));
    assert_eq!(first["rank"], "review");
    assert!(first["next"]
        .as_str()
        .unwrap()
        .contains("pull request body"));
    // Every signal has a state, and what git cannot tell is named, not left out.
    let state = |id: &str| {
        s["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .map(|c| c["state"].as_str().unwrap().to_string())
    };
    assert_eq!(state("guard-gate-loosened").as_deref(), Some("clean"));
    assert_eq!(state("baseline-grew").as_deref(), Some("found"));
    assert_eq!(state("owner-ratification").as_deref(), Some("not-checked"));

    // #7 restores `exempt_paths`: the loosening is paid back.
    repo.write("discipline.toml", CONFIG);
    repo.commit("chore: drop the b exemption (#7)");
    let run = repo.run(&["audit", "--last", "6", "--ref", "main", "--json"], &[]);
    let s: Value = serde_json::from_str(&run.stdout).unwrap();
    assert!(
        !signal_ids(&s).contains(&"loosened-not-restored".to_string()),
        "{s:#}"
    );
    assert_eq!(s["tightenings"][0]["pr"], 7, "{s:#}");
    assert_eq!(s["tightenings"][0]["key"], "exempt_paths");

    let text = repo.run(&["audit", "--last", "6", "--ref", "main"], &[]);
    assert!(
        text.stdout.starts_with("Needs a decision:\n"),
        "{}",
        text.stdout
    );
    assert!(
        text.stdout.contains("not-checked independent-review"),
        "{}",
        text.stdout
    );
}
