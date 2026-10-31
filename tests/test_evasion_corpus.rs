//! Evasion cases with their verdicts, driven through the built binary (#837).
//!
//! Each case in `tests/fixtures/evasion_corpus.json` is a base tree, a head tree, an
//! optional PR body and the gates it enables beyond the built-in defaults. The head tree
//! is laid over the base: a path mapped to a string is written, a path mapped to `null`
//! is removed (a file, or a directory with everything under it), so a case can delete a
//! test file, a fixture directory or a workflow. The verdict is one of:
//! - `caught`: exactly these finding codes are reported, at any severity, counting a
//!   policy refusal by its `policy/<code>` rule id;
//! - `lifted`: nothing is reported, and a directive lifted exactly these finding codes;
//! - `known_miss`: nothing is reported, and the issue named tracks the gap;
//! - `clean`: a legitimate change, with nothing reported and nothing lifted, that the
//!   case's `gate` examined (a false-positive control);
//! - `exit`: the run could not check, with this exit code.
//!
//! A caught case that stops being caught fails here. So does a known miss that starts
//! being caught: update its verdict and the gate's "What it does NOT catch" list in
//! `docs/GATES.md` together, so the documented misses stay true. A known miss whose issue
//! that list does not name fails too.
//!
//! The corpus's `coverage` names every registered gate: on `required` when cases must
//! exercise it, or on `exempt` with the reason it has none. A required gate needs a catch
//! case (`caught` codes of the gate, or a `known_miss` / `exit` case whose `gate` names
//! it) and a false-positive control (a `clean` case whose `gate` names it, or a legitimate
//! change whose `lifted` codes are the gate's, for a gate that by design passes a
//! legitimate change only with its directive).

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
    /// The gate a `known_miss`, `clean` or `exit` case is about.
    #[serde(default)]
    gate: Option<String>,
    verdict: Verdict,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Verdict {
    Caught(Vec<String>),
    Lifted(Vec<String>),
    KnownMiss(String),
    Clean,
    Exit(i32),
}

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<Case>,
    coverage: Coverage,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    /// Gates that must have a catch case and a false-positive control.
    required: Vec<String>,
    /// Gates left out on purpose, each with the reason.
    exempt: BTreeMap<String, String>,
}

/// What the binary reported for one case.
struct Outcome {
    /// The exit code.
    code: i32,
    /// The code of every violation and the rule id of every policy refusal.
    reported: BTreeSet<String>,
    /// The code of every finding a directive lifted.
    lifted: BTreeSet<String>,
    /// Items examined, by enabled gate.
    examined: BTreeMap<String, u64>,
}

fn corpus() -> Corpus {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evasion_corpus.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// One list of the stability contract's v1 surface (`finding_codes`, `policy_refusals`, `gates`).
fn surface(key: &str) -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1_surface.json");
    let surface: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    surface["surface"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect()
}

/// The text of each gate's "What it does NOT catch" list in `docs/GATES.md`, by gate id.
fn documented_misses() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/GATES.md");
    let heading = regex::Regex::new(r"^#{1,4} `([a-z0-9-]+)`$").unwrap();
    let mut misses = BTreeMap::<String, String>::new();
    let (mut gate, mut inside) = (None::<String>, false);
    for line in std::fs::read_to_string(path).unwrap().lines() {
        if line.starts_with('#') && !line.starts_with("#####") {
            gate = heading.captures(line).map(|c| c[1].to_string());
            inside = false;
        } else if line.starts_with("- **What it does NOT catch:**") {
            inside = true;
        } else if line.starts_with("- **") || line.starts_with('#') {
            inside = false;
        }
        if let (true, Some(gate)) = (inside, &gate) {
            let text = misses.entry(gate.clone()).or_default();
            text.push_str(line);
            text.push('\n');
        }
    }
    misses
}

/// The gates a case is a catch case for, and the gates it is a false-positive control for.
fn gates_of(case: &Case) -> (BTreeSet<String>, BTreeSet<String>) {
    let of_codes = |codes: &[String]| -> BTreeSet<String> {
        codes
            .iter()
            .filter_map(|c| c.split_once('/').map(|(gate, _)| gate.to_string()))
            .collect()
    };
    let named: BTreeSet<String> = case.gate.iter().cloned().collect();
    match &case.verdict {
        Verdict::Caught(codes) => (&of_codes(codes) | &named, BTreeSet::new()),
        Verdict::KnownMiss(_) | Verdict::Exit(_) => (named, BTreeSet::new()),
        Verdict::Lifted(codes) => (BTreeSet::new(), of_codes(codes)),
        Verdict::Clean => (BTreeSet::new(), named),
    }
}

/// What each gate on the coverage list lacks: a catch case, a false-positive control.
fn uncovered(corpus: &Corpus) -> Vec<String> {
    let (mut caught, mut controlled) = (BTreeSet::new(), BTreeSet::new());
    for case in &corpus.cases {
        let (catch, control) = gates_of(case);
        caught.extend(catch);
        controlled.extend(control);
    }
    let mut missing = Vec::new();
    for gate in &corpus.coverage.required {
        if !caught.contains(gate) {
            missing.push(format!("{gate}: no catch case"));
        }
        if !controlled.contains(gate) {
            missing.push(format!("{gate}: no false-positive control"));
        }
    }
    missing
}

/// What the binary reports for one case.
fn run_case(case: &Case) -> Outcome {
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
    // A policy refusal is a line of text in the JSON report; the SARIF report carries its
    // rule id. Written under `.git` so it is never part of the tree.
    let sarif = repo.path().join(".git/corpus.sarif");
    let sarif_arg = sarif.to_str().unwrap();
    let mut args = vec![
        "check",
        "--format",
        "json",
        "--base",
        "main",
        "--report-sarif",
        sarif_arg,
    ];
    if !enable.is_empty() {
        args.extend(["--enable", &enable]);
    }
    let run = repo.run(&args, &[("PR_BODY", &case.body)]);
    if run.code == 2 {
        return Outcome {
            code: 2,
            reported: BTreeSet::new(),
            lifted: BTreeSet::new(),
            examined: BTreeMap::new(),
        };
    }
    let report = run.json();
    let outcomes = report["outcomes"].as_array().unwrap();
    let codes = |key: &str| -> Vec<String> {
        outcomes
            .iter()
            .flat_map(|o| o[key].as_array().cloned().unwrap_or_default())
            .map(|v| v["code"].as_str().unwrap().to_string())
            .collect()
    };
    let sarif: Value = serde_json::from_str(&std::fs::read_to_string(&sarif).unwrap()).unwrap();
    let refusals = sarif["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["results"].as_array().cloned().unwrap_or_default())
        .map(|r| r["ruleId"].as_str().unwrap().to_string())
        .filter(|id| id.starts_with("policy/"));
    Outcome {
        code: run.code,
        reported: codes("violations").into_iter().chain(refusals).collect(),
        lifted: codes("overrides").into_iter().collect(),
        examined: outcomes
            .iter()
            .filter(|o| o["enabled"] == true)
            .map(|o| {
                let gate = o["gate"].as_str().unwrap().to_string();
                (gate, o["examined"].as_u64().unwrap_or(0))
            })
            .collect(),
    }
}

/// The mismatch for one case, or `None` when the binary agrees with its verdict.
fn judge(case: &Case, outcome: &Outcome) -> Option<String> {
    let Outcome {
        code,
        reported: codes,
        lifted,
        examined,
    } = outcome;
    match &case.verdict {
        Verdict::Exit(want) if code != want => Some(format!(
            "{}: expected exit {want}, got exit {code} with {codes:?}",
            case.name
        )),
        Verdict::Exit(_) => None,
        _ if *code == 2 => Some(format!("{}: could not check (exit 2)", case.name)),
        Verdict::Caught(want) => {
            let want: BTreeSet<String> = want.iter().cloned().collect();
            (*codes != want).then(|| {
                format!(
                    "{}: expected exactly {want:?}, got {codes:?} (a detector stopped or started reporting this case)",
                    case.name
                )
            })
        }
        Verdict::Lifted(want) => {
            let want: BTreeSet<String> = want.iter().cloned().collect();
            (!codes.is_empty() || *lifted != want).then(|| {
                format!(
                    "{}: expected nothing reported and exactly {want:?} lifted, got {codes:?} reported and {lifted:?} lifted",
                    case.name
                )
            })
        }
        Verdict::Clean => {
            let gate = case.gate.as_deref().unwrap_or_default();
            let seen = examined.get(gate).copied().unwrap_or(0);
            (*code != 0 || !codes.is_empty() || !lifted.is_empty() || seen == 0).then(|| {
                format!(
                    "{}: expected a clean run that `{gate}` examined, got exit {code}, {codes:?} reported, {lifted:?} lifted, {seen} item(s) examined by `{gate}`",
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
                        .filter_map(|case| judge(case, &run_case(case)))
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
    let findings = surface("finding_codes");
    let refusals = surface("policy_refusals");
    let gates = surface("gates");
    let misses = documented_misses();
    let issue = regex::Regex::new(r"^#[1-9][0-9]*$").unwrap();
    let mut names = BTreeSet::new();
    for case in &corpus.cases {
        assert!(
            names.insert(&case.name),
            "duplicate case name {}",
            case.name
        );
        assert!(!case.head.is_empty(), "{}: the change is empty", case.name);
        for (path, content) in &case.head {
            let dir = format!("{path}/");
            assert!(
                content.is_some() || case.base.keys().any(|b| b == path || b.starts_with(&dir)),
                "{}: `{path}` is removed (`null`) but the base tree has no such file or directory",
                case.name
            );
        }
        if let Some(gate) = &case.gate {
            assert!(
                gates.contains(gate),
                "{}: `gate` names `{gate}`, which is not a registered gate",
                case.name
            );
        }
        match &case.verdict {
            Verdict::Caught(want) => {
                assert!(!want.is_empty(), "{}: `caught` lists no code", case.name);
                for code in want {
                    assert!(
                        findings.contains(code) || refusals.contains(code),
                        "{}: `{code}` is not a registered finding code or policy refusal",
                        case.name
                    );
                }
            }
            Verdict::Lifted(want) => {
                assert!(!want.is_empty(), "{}: `lifted` lists no code", case.name);
                for code in want {
                    assert!(
                        findings.contains(code),
                        "{}: `{code}` is not a registered finding code",
                        case.name
                    );
                }
            }
            Verdict::KnownMiss(id) => {
                assert!(
                    issue.is_match(id),
                    "{}: a known miss names its tracking issue as `#<number>`, not `{id}`",
                    case.name
                );
                let named = regex::Regex::new(&format!(r"{id}\b")).unwrap();
                let documented = match &case.gate {
                    Some(gate) => misses.get(gate).is_some_and(|t| named.is_match(t)),
                    None => misses.values().any(|t| named.is_match(t)),
                };
                assert!(
                    documented,
                    "{}: known miss {id} is not named in {} \"What it does NOT catch\" list in docs/GATES.md",
                    case.name,
                    case.gate
                        .as_ref()
                        .map_or("any gate's".to_string(), |g| format!("`{g}`'s"))
                );
            }
            Verdict::Clean => assert!(
                case.gate.is_some(),
                "{}: a `clean` case names the gate it controls in `gate`",
                case.name
            ),
            Verdict::Exit(code) => assert_eq!(*code, 2, "{}: only exit 2 is a verdict", case.name),
        }
    }
}

/// Every registered gate is on the coverage list or exempt with a reason, and every gate
/// on the list has a catch case and a false-positive control.
#[test]
fn every_listed_gate_has_a_case() {
    let corpus = corpus();
    let registered = surface("gates");
    let Coverage { required, exempt } = &corpus.coverage;
    for gate in required {
        assert!(
            registered.contains(gate),
            "coverage lists `{gate}`, which is not a registered gate"
        );
        assert!(
            !exempt.contains_key(gate),
            "`{gate}` is both required and exempt"
        );
    }
    for (gate, reason) in exempt {
        assert!(
            registered.contains(gate),
            "coverage exempts `{gate}`, which is not a registered gate"
        );
        assert!(
            !reason.trim().is_empty(),
            "`{gate}` is exempt without a reason"
        );
    }
    let listed: BTreeSet<&String> = required.iter().chain(exempt.keys()).collect();
    let unlisted: Vec<&String> = registered.iter().filter(|g| !listed.contains(g)).collect();
    assert!(
        unlisted.is_empty(),
        "gates neither required nor exempt in the corpus coverage: {unlisted:?}"
    );
    let missing = uncovered(&corpus);
    assert!(
        missing.is_empty(),
        "gates on the corpus coverage list without their controls: {missing:?}; add the case (`caught` / `known_miss` for a catch, `clean` / `lifted` for a false-positive control), or move the gate to `exempt` with a reason"
    );
}

/// The out-of-lane classes that docs/GATES.md states (and the README's generated
/// Non-Goals repeat) and the corpus's `out-of-lane-*` cases name each other: each class
/// has its `known_miss` case, on the gate whose list states it and under its issue, and
/// each such case has its class. A class whose case starts being caught fails
/// `every_evasion_case_gets_its_recorded_verdict`; one stated without a case fails here.
#[test]
fn every_out_of_lane_class_is_a_known_miss_case() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/GATES.md");
    let classes = discipline::docs::lane_classes(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(
        !classes.is_empty(),
        "docs/GATES.md states no out-of-lane class"
    );
    let corpus = corpus();
    for class in &classes {
        let name = class.corpus_case();
        let case = corpus.cases.iter().find(|c| c.name == name);
        let Some(case) = case else {
            panic!(
                "out-of-lane class `{}` has no corpus case `{name}`",
                class.class
            );
        };
        assert_eq!(
            case.gate.as_deref(),
            Some(class.gate.as_str()),
            "{name}: gate"
        );
        assert!(
            matches!(&case.verdict, Verdict::KnownMiss(issue) if *issue == class.issue),
            "{name}: expected `known_miss` {}, got {:?}",
            class.issue,
            case.verdict
        );
    }
    let stated: BTreeSet<String> = classes.iter().map(|c| c.corpus_case()).collect();
    for case in corpus
        .cases
        .iter()
        .filter(|c| c.name.starts_with("out-of-lane-"))
    {
        assert!(
            stated.contains(&case.name),
            "{}: no `**Out of lane: <class>**` entry in docs/GATES.md names this case",
            case.name
        );
    }
}

/// Gates that were on by default at `error` before the severity lifecycle's promotion rule
/// (docs/GATES.md "Severity Lifecycle", #941) and have no corpus controls yet. The list
/// only shrinks: a gate leaves it when its catch case and false-positive control land.
const ERROR_BEFORE_THE_RULE: &[&str] = &[
    "agent-scratch",
    "ci-skip-set",
    "citation-metadata",
    "pii",
    "test-budget",
];

/// The gates on by default at `error` that the corpus coverage does not require, other
/// than the grandfathered ones; and the grandfathered ones that no longer need to be.
fn promotion_rule_breaches(
    required: &[String],
    error_by_default: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let required: BTreeSet<&String> = required.iter().collect();
    let grandfathered: BTreeSet<String> = ERROR_BEFORE_THE_RULE
        .iter()
        .map(|g| g.to_string())
        .collect();
    let uncontrolled = error_by_default
        .iter()
        .filter(|g| !required.contains(g) && !grandfathered.contains(*g))
        .cloned()
        .collect();
    let stale = grandfathered
        .iter()
        .filter(|g| required.contains(g) || !error_by_default.contains(*g))
        .cloned()
        .collect();
    (uncontrolled, stale)
}

/// The promotion rule of the severity lifecycle: a gate on by default at `error` has
/// corpus controls (it is on the coverage's `required` list).
#[test]
fn an_error_default_has_corpus_controls() {
    use discipline::config::{GateSettings, Gates, Severity, GATES};
    let defaults = Gates::default();
    let error_by_default: BTreeSet<String> = GATES
        .iter()
        .filter(|g| g.available)
        .filter(|g| {
            defaults
                .settings(g.id)
                .is_some_and(|s: &dyn GateSettings| s.enabled() && s.severity() == Severity::Error)
        })
        .map(|g| g.id.to_string())
        .collect();
    assert!(
        error_by_default.contains("assertion-reduction"),
        "the default severities could not be read"
    );
    let corpus = corpus();
    let (uncontrolled, stale) =
        promotion_rule_breaches(&corpus.coverage.required, &error_by_default);
    assert!(
        uncontrolled.is_empty(),
        "gates on by default at `error` without corpus controls: {uncontrolled:?}; add their catch case and false-positive control and list them under `coverage.required`, or default them below `error` (docs/GATES.md \"Severity Lifecycle\")"
    );
    assert!(
        stale.is_empty(),
        "gates on ERROR_BEFORE_THE_RULE that are now required or no longer `error` by default: {stale:?}; remove them from the list"
    );
    // The check discriminates: a required gate dropped from the coverage is reported.
    let without: Vec<String> = corpus
        .coverage
        .required
        .iter()
        .filter(|g| *g != "assertion-reduction")
        .cloned()
        .collect();
    assert_eq!(
        promotion_rule_breaches(&without, &error_by_default).0,
        vec!["assertion-reduction".to_string()]
    );
}

/// The coverage check discriminates: removing a listed gate's catch cases, or its
/// false-positive controls, reports that gate.
#[test]
fn coverage_reports_a_listed_gate_whose_cases_are_removed() {
    let gate = "shell-secrets".to_string();
    let without = |control: bool| {
        let mut corpus = corpus();
        assert!(uncovered(&corpus).is_empty());
        assert!(corpus.coverage.required.contains(&gate));
        corpus.cases.retain(|c| {
            let (catch, controls) = gates_of(c);
            !if control { controls } else { catch }.contains(&gate)
        });
        uncovered(&corpus)
    };
    assert_eq!(without(false), vec!["shell-secrets: no catch case"]);
    assert_eq!(
        without(true),
        vec!["shell-secrets: no false-positive control"]
    );
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
        gate: None,
        verdict,
    };
    let set = |codes: &[&str]| codes.iter().map(|c| c.to_string()).collect::<BTreeSet<_>>();
    let ran = |code: i32, reported: &[&str], lifted: &[&str]| Outcome {
        code,
        reported: set(reported),
        lifted: set(lifted),
        examined: BTreeMap::from([("a".to_string(), 1)]),
    };
    let caught = case(Verdict::Caught(vec!["a/b".into()]));
    assert!(judge(&caught, &ran(1, &["a/b"], &[])).is_none());
    assert!(
        judge(&caught, &ran(0, &[], &[])).is_some(),
        "a caught case stopped being caught"
    );
    assert!(
        judge(&caught, &ran(1, &["a/b", "c/d"], &[])).is_some(),
        "an extra code"
    );
    assert!(
        judge(&caught, &ran(2, &[], &[])).is_some(),
        "could not check"
    );
    let lifted = case(Verdict::Lifted(vec!["a/b".into()]));
    assert!(judge(&lifted, &ran(0, &[], &["a/b"])).is_none());
    assert!(
        judge(&lifted, &ran(1, &["a/b"], &[])).is_some(),
        "the directive no longer lifts it"
    );
    assert!(
        judge(&lifted, &ran(0, &[], &[])).is_some(),
        "nothing to lift: the detector stopped reporting it"
    );
    assert!(
        judge(&lifted, &ran(1, &["c/d"], &["a/b"])).is_some(),
        "lifted, but something else is reported"
    );
    let miss = case(Verdict::KnownMiss("#1".into()));
    assert!(judge(&miss, &ran(0, &[], &[])).is_none());
    assert!(
        judge(&miss, &ran(1, &["a/b"], &[])).is_some(),
        "a known miss is now caught"
    );
    assert!(
        judge(&miss, &ran(0, &["w/x"], &[])).is_some(),
        "even at warning severity"
    );
    let clean = Case {
        gate: Some("a".into()),
        ..case(Verdict::Clean)
    };
    assert!(judge(&clean, &ran(0, &[], &[])).is_none());
    assert!(
        judge(&clean, &ran(1, &["a/b"], &[])).is_some(),
        "a false positive"
    );
    assert!(
        judge(&clean, &ran(0, &["a/w"], &[])).is_some(),
        "even at warning severity"
    );
    assert!(
        judge(&clean, &ran(0, &[], &["a/b"])).is_some(),
        "clean only because a directive lifted it"
    );
    let unexamined = Outcome {
        examined: BTreeMap::from([("a".to_string(), 0)]),
        ..ran(0, &[], &[])
    };
    assert!(
        judge(&clean, &unexamined).is_some(),
        "clean only because the gate looked at nothing"
    );
    let exit = case(Verdict::Exit(2));
    assert!(judge(&exit, &ran(2, &[], &[])).is_none());
    assert!(judge(&exit, &ran(0, &[], &[])).is_some());
}
