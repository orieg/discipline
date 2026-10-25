//! Evasion cases with their verdicts, driven through the built binary (#837).
//!
//! Each case in `tests/fixtures/evasion_corpus.json` is a base tree, a head tree, an
//! optional PR body and the gates it enables beyond the built-in defaults. The verdict is
//! one of:
//! - `caught`: exactly these finding codes are reported, at any severity;
//! - `known_miss`: nothing is reported, and the issue named tracks the gap;
//! - `exit`: the run could not check, with this exit code.
//!
//! A caught case that stops being caught fails here. So does a known miss that starts
//! being caught: update its verdict and the gate's "What it does NOT catch" list in
//! `docs/GATES.md` together, so the documented misses stay true.

mod common;

use common::Repo;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    base: BTreeMap<String, String>,
    head: BTreeMap<String, Option<String>>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    enable: Vec<String>,
    verdict: Verdict,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Verdict {
    Caught(Vec<String>),
    KnownMiss(String),
    Exit(i32),
}

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

fn corpus() -> Corpus {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evasion_corpus.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The finding codes the stability contract records (`finding_codes` of the v1 surface).
fn registered_codes() -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_surface.json");
    let surface: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    surface["surface"]["finding_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect()
}

/// What the binary reports for one case: its exit code and the codes of every violation.
fn run_case(case: &Case) -> (i32, BTreeSet<String>) {
    let repo = Repo {
        dir: tempfile::tempdir().unwrap(),
    };
    repo.git(&["init", "-q", "-b", "main"]);
    repo.write("AGENTS.md", "# agents\n");
    for (path, content) in &case.base {
        repo.write(path, content);
    }
    repo.commit("base");
    repo.git(&["checkout", "-q", "-b", "work"]);
    for (path, content) in &case.head {
        match content {
            Some(content) => repo.write(path, content),
            None => repo.remove(path),
        }
    }
    repo.commit("change");

    let enable = case.enable.join(",");
    let mut args = vec!["check", "--format", "json", "--base", "main"];
    if !enable.is_empty() {
        args.extend(["--enable", &enable]);
    }
    let run = repo.run(&args, &[("PR_BODY", &case.body)]);
    if run.code == 2 {
        return (2, BTreeSet::new());
    }
    let codes = run.json()["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|o| o["violations"].as_array().cloned().unwrap_or_default())
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect();
    (run.code, codes)
}

/// The mismatch for one case, or `None` when the binary agrees with its verdict.
fn judge(case: &Case, code: i32, codes: &BTreeSet<String>) -> Option<String> {
    match &case.verdict {
        Verdict::Exit(want) if code != *want => Some(format!(
            "{}: expected exit {want}, got exit {code} with {codes:?}",
            case.name
        )),
        Verdict::Exit(_) => None,
        _ if code == 2 => Some(format!("{}: could not check (exit 2)", case.name)),
        Verdict::Caught(want) => {
            let want: BTreeSet<String> = want.iter().cloned().collect();
            (*codes != want).then(|| {
                format!(
                    "{}: expected exactly {want:?}, got {codes:?} (a detector stopped or started reporting this case)",
                    case.name
                )
            })
        }
        Verdict::KnownMiss(issue) => (!codes.is_empty()).then(|| {
            format!(
                "{}: known miss ({issue}) is now reported as {codes:?}; set its verdict to `caught` and update the gate's \"What it does NOT catch\" list in docs/GATES.md",
                case.name
            )
        }),
    }
}

#[test]
fn every_evasion_case_gets_its_recorded_verdict() {
    let corpus = corpus();
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let chunk = corpus.cases.len().div_ceil(workers).max(1);
    let mut failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = corpus
            .cases
            .chunks(chunk)
            .map(|cases| {
                scope.spawn(move || {
                    cases
                        .iter()
                        .filter_map(|case| {
                            let (code, codes) = run_case(case);
                            judge(case, code, &codes)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    failures.sort();
    assert!(
        failures.is_empty(),
        "{} of {} evasion cases disagree with tests/fixtures/evasion_corpus.json:\n{}",
        failures.len(),
        corpus.cases.len(),
        failures.join("\n")
    );
}

#[test]
fn the_corpus_is_well_formed() {
    let corpus = corpus();
    let codes = registered_codes();
    let issue = regex::Regex::new(r"^#[1-9][0-9]*$").unwrap();
    let mut names = BTreeSet::new();
    for case in &corpus.cases {
        assert!(
            names.insert(&case.name),
            "duplicate case name {}",
            case.name
        );
        assert!(!case.head.is_empty(), "{}: the change is empty", case.name);
        match &case.verdict {
            Verdict::Caught(want) => {
                assert!(!want.is_empty(), "{}: `caught` lists no code", case.name);
                for code in want {
                    assert!(
                        codes.contains(code),
                        "{}: `{code}` is not a registered finding code",
                        case.name
                    );
                }
            }
            Verdict::KnownMiss(id) => assert!(
                issue.is_match(id),
                "{}: a known miss names its tracking issue as `#<number>`, not `{id}`",
                case.name
            ),
            Verdict::Exit(code) => assert_eq!(*code, 2, "{}: only exit 2 is a verdict", case.name),
        }
    }
}

/// The judge itself discriminates: each verdict kind fails on the outcome it rules out.
#[test]
fn the_judge_rejects_each_kind_of_disagreement() {
    let case = |verdict| Case {
        name: "c".into(),
        base: BTreeMap::new(),
        head: BTreeMap::new(),
        body: String::new(),
        enable: vec![],
        verdict,
    };
    let set = |codes: &[&str]| codes.iter().map(|c| c.to_string()).collect::<BTreeSet<_>>();
    let caught = case(Verdict::Caught(vec!["a/b".into()]));
    assert!(judge(&caught, 1, &set(&["a/b"])).is_none());
    assert!(
        judge(&caught, 0, &set(&[])).is_some(),
        "a caught case stopped being caught"
    );
    assert!(
        judge(&caught, 1, &set(&["a/b", "c/d"])).is_some(),
        "an extra code"
    );
    assert!(judge(&caught, 2, &set(&[])).is_some(), "could not check");
    let miss = case(Verdict::KnownMiss("#1".into()));
    assert!(judge(&miss, 0, &set(&[])).is_none());
    assert!(
        judge(&miss, 1, &set(&["a/b"])).is_some(),
        "a known miss is now caught"
    );
    assert!(
        judge(&miss, 0, &set(&["w/x"])).is_some(),
        "even at warning severity"
    );
    let exit = case(Verdict::Exit(2));
    assert!(judge(&exit, 2, &set(&[])).is_none());
    assert!(judge(&exit, 0, &set(&[])).is_some());
}
