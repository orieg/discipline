//! A pull request number is read only from the end of a subject (#651).
//!
//! `replay` and `audit` took the last `(#N)` anywhere in a commit subject as the pull
//! request, so `fix (#12) typo` was reported against pull request 12. They now read what
//! `commit-provenance` reads for a squash: a subject that ends with `(#N)`.
//!
//! Each case drives the real binary over a history built by this file.

mod common;
use common::Repo;
use serde_json::Value;

const SUBJECTS: &[(&str, Option<u64>)] = &[
    ("fix: a (#12) typo", None),
    ("fix: b (#12) and (#34)", Some(34)),
    ("fix: c (#56)", Some(56)),
    ("fix: d (#56) ", Some(56)),
    ("fix: e (#7a)", None),
    ("fix: f (#0)", None),
    ("fix: g #9", None),
];

/// One change per subject, each with a directive in its body so `audit` records it.
fn history() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    for (i, (subject, _)) in SUBJECTS.iter().enumerate() {
        repo.write(&format!("docs/n{i}.md"), "# Notes\n");
        repo.commit(&format!("{subject}\n\nno-issue: bookkeeping {i}\n"));
    }
    repo
}

/// The `pr` of each change, oldest first.
fn numbers(repo: &Repo, command: &str, list: &str) -> Vec<Option<u64>> {
    let last = SUBJECTS.len().to_string();
    let run = repo.run(
        &[command, "--last", &last, "--ref", "main", "--json"],
        &[("DISCIPLINE_NO_NETWORK", "1")],
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let json: Value = serde_json::from_str(&run.stdout).unwrap();
    let mut seen: Vec<Option<u64>> = json[list]
        .as_array()
        .unwrap_or_else(|| panic!("{json:#}"))
        .iter()
        .map(|c| c["pr"].as_u64())
        .collect();
    seen.reverse();
    seen
}

fn wanted() -> Vec<Option<u64>> {
    SUBJECTS.iter().map(|(_, n)| *n).collect()
}

#[test]
fn replay_reads_a_pull_number_only_at_the_end_of_the_subject() {
    let repo = history();
    assert_eq!(numbers(&repo, "replay", "cases_detail"), wanted());
}

#[test]
fn audit_reads_a_pull_number_only_at_the_end_of_the_subject() {
    let repo = history();
    assert_eq!(numbers(&repo, "audit", "records"), wanted());
}
