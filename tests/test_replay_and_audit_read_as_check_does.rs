//! `audit` and `replay` read a commit as `check` does, and `replay --json` is the same on
//! every run (#651).
//!
//! - A commit message that is not UTF-8 is read lossily by `check`: each invalid byte is
//!   U+FFFD and the directives in it are read. `audit` read such a message as empty, and
//!   `replay` rebuilt the commit with an empty message.
//! - A `--ref` that is not a commit is named by `replay` as `audit` names it.
//! - An override read from the replayed commit's message names that commit as its source,
//!   not the scratch commit `replay` builds, whose id differs on every run: two runs of
//!   the same replay print the same bytes.
//!
//! Each case drives the real binary over a history built by this file.

mod common;
use common::{Repo, Run};
use serde_json::Value;

const TWO_ASSERTS: &str = "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 3, 4);\n}\n";
const ONE_ASSERT: &str = "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n}\n";

/// Commits everything with `message` as raw bytes. git rewrites a message that is not
/// UTF-8 as if it were Latin-1 unless the commit declares another encoding, so the
/// commit declares one: the bytes are then stored as they are.
fn commit_bytes(repo: &Repo, message: &[u8]) {
    let file = repo.path().join(".git").join("RAW_MESSAGE");
    std::fs::write(&file, message).unwrap();
    repo.git(&["add", "-A"]);
    repo.git(&[
        "-c",
        "i18n.commitEncoding=ISO-8859-1",
        "commit",
        "-q",
        "-F",
        file.to_str().unwrap(),
    ]);
}

/// `main` gains one change that drops an assertion, waived in its commit message. The
/// message has one byte that is not UTF-8 when `latin1` is set.
fn history(latin1: bool) -> (Repo, String) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("tests/a.rs", TWO_ASSERTS);
    repo.commit("test: adds (#1)");
    repo.write("tests/a.rs", ONE_ASSERT);
    let mut message = b"test: simplify adds (#2)\n\nSimplifi".to_vec();
    message.extend_from_slice(if latin1 { b"\xe9" } else { b"e" });
    message
        .extend_from_slice(b"d.\n\nallow-assertion-drop: adds the second check moved elsewhere\n");
    commit_bytes(&repo, &message);
    let sha = repo.git_output(&["rev-parse", "HEAD"]).trim().to_string();
    // The fixture is what it says: the stored message is UTF-8 exactly when asked.
    let stored = repo.git_output(&["cat-file", "commit", "HEAD"]);
    assert_eq!(stored.contains('\u{fffd}'), latin1, "{stored}");
    (repo, sha)
}

fn run(repo: &Repo, args: &[&str]) -> Run {
    repo.run(args, &[("DISCIPLINE_NO_NETWORK", "1")])
}

fn json(run: &Run) -> Value {
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    serde_json::from_str(&run.stdout).unwrap()
}

#[test]
fn audit_reads_the_directives_of_a_message_that_is_not_utf8() {
    for latin1 in [false, true] {
        let (repo, sha) = history(latin1);
        let s = json(&run(
            &repo,
            &["audit", "--last", "1", "--ref", "main", "--json"],
        ));
        let records = s["records"].as_array().unwrap();
        let directive = records
            .iter()
            .find(|r| r["directive"] == "allow-assertion-drop")
            .unwrap_or_else(|| panic!("latin1={latin1}: no directive record in {s:#}"));
        assert_eq!(directive["sha"], sha.as_str(), "{s:#}");
        assert_eq!(directive["pr"], 2, "{s:#}");
    }
}

#[test]
fn replay_reads_the_directives_of_a_message_that_is_not_utf8() {
    for latin1 in [false, true] {
        let (repo, _) = history(latin1);
        let s = json(&run(
            &repo,
            &["replay", "--last", "1", "--ref", "main", "--json"],
        ));
        let case = &s["cases_detail"][0];
        assert_eq!(case["verdict"], "passed", "latin1={latin1}: {s:#}");
        assert_eq!(case["pr"], 2, "latin1={latin1}: {s:#}");
        assert_eq!(
            case["overrides"][0]["directive"], "allow-assertion-drop",
            "latin1={latin1}: {s:#}"
        );
    }
}

#[test]
fn an_override_of_a_replayed_commit_names_that_commit_and_two_runs_are_identical() {
    let (repo, sha) = history(false);
    let first = run(&repo, &["replay", "--last", "2", "--ref", "main", "--json"]);
    let s = json(&first);
    // The replayed commit's id, cut as `check` cuts a commit id.
    let source = s["cases_detail"][0]["overrides"][0]["source"]
        .as_str()
        .unwrap_or_else(|| panic!("{s:#}"));
    let id = source
        .strip_prefix("commit ")
        .unwrap_or_else(|| panic!("{source}"));
    assert!(
        id.len() >= 7 && sha.starts_with(id),
        "{source} is not {sha}"
    );
    // A second run writes the same bytes.
    let second = run(&repo, &["replay", "--last", "2", "--ref", "main", "--json"]);
    assert_eq!(first.stdout, second.stdout);
}

/// Control: what `check` does with the same change, which the two commands now match.
#[test]
fn check_reads_the_directives_of_a_message_that_is_not_utf8() {
    let (repo, _) = history(true);
    let out = run(&repo, &["check", "--format", "json", "--base", "main~1"]);
    let s = json(&out);
    let applied: Vec<&Value> = s["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|o| o["overrides"].as_array().unwrap())
        .collect();
    assert_eq!(applied.len(), 1, "{s:#}");
    assert_eq!(applied[0]["directive"], "allow-assertion-drop");
}

#[test]
fn replay_names_a_ref_that_is_not_a_commit_as_audit_does() {
    let (repo, _) = history(false);
    for command in ["audit", "replay"] {
        let out = run(
            &repo,
            &[command, "--last", "1", "--ref", "main^{tree}", "--json"],
        );
        assert_eq!(out.code, 2, "{command}: {}{}", out.stdout, out.stderr);
        assert!(
            out.stderr.contains("`main^{tree}` is not a commit"),
            "{command}: {}",
            out.stderr
        );
    }
}
