//! End-to-end tests for the adoption path: what a consumer meets when it first
//! records a baseline, ports an existing test floor, or writes its first
//! `version-lockstep` groups from the documentation.

mod common;
use common::*;

use std::collections::BTreeMap;

// ---- baseline: only blocking severities by default ---------------------------

/// One finding per severity, each from a gate whose severity is pinned in the
/// config so the fixture does not depend on built-in defaults:
/// - `time-estimates` at `error`      (docs/legacy.md)
/// - `suppression-delta` at `warning` (src/legacy.rs)
/// - `pii` at `note`                  (docs/contact.md, a LAN address)
const SEVERITY_CONFIG: &str = r#"
[meta]
version = 1
name = "adoption"

[gates.time-estimates]
severity = "error"

[gates.suppression-delta]
severity = "warning"

[gates.pii]
severity = "note"
"#;

/// The debt on `main` with a clean working branch (the brownfield case, reached
/// by `--whole-tree`), or on the working branch itself (reached by the diff).
fn severity_fixture_with_debt_on(branch: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", branch]);
    repo.write("discipline.toml", SEVERITY_CONFIG);
    repo.write("docs/legacy.md", "Ships in 3 weeks.\n");
    repo.write(
        "src/legacy.rs",
        "#[allow(dead_code)]\nfn unused() -> u8 {\n    1\n}\n",
    );
    // Assembled at runtime so this source file does not itself carry the address.
    repo.write(
        "docs/contact.md",
        &format!("host = {}\n", ["192", "168", "4", "7"].join(".")),
    );
    repo.commit("chore: pre-existing debt");
    if branch == "main" {
        repo.git(&["checkout", "-q", "-B", "work"]);
    }
    repo
}

fn severity_fixture() -> Repo {
    severity_fixture_with_debt_on("main")
}

/// Recorded baseline entries, counted per gate.
fn recorded_by_gate(repo: &Repo) -> BTreeMap<String, usize> {
    let text = std::fs::read_to_string(repo.file("discipline-baseline.toml"))
        .expect("baseline file written");
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    let mut by_gate = BTreeMap::new();
    for f in parsed
        .get("findings")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default()
    {
        *by_gate
            .entry(f["gate"].as_str().unwrap().to_string())
            .or_insert(0) += 1;
    }
    by_gate
}

#[test]
fn baseline_fixture_carries_one_finding_per_severity() {
    // Guards the fixture itself: if a gate stops firing, the severity tests
    // below would pass vacuously.
    let repo = severity_fixture_with_debt_on("work");
    let run = repo.check(&[]);
    let json = run.json();
    let mut sev: BTreeMap<(String, String), usize> = BTreeMap::new();
    for o in json["outcomes"].as_array().unwrap() {
        for v in o["violations"].as_array().unwrap() {
            *sev.entry((
                o["gate"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
            ))
            .or_insert(0) += 1;
        }
    }
    assert_eq!(
        sev.get(&("time-estimates".into(), "error".into())),
        Some(&1),
        "{sev:?}"
    );
    assert_eq!(
        sev.get(&("suppression-delta".into(), "warning".into())),
        Some(&1),
        "{sev:?}"
    );
    assert_eq!(sev.get(&("pii".into(), "note".into())), Some(&1), "{sev:?}");
}

#[test]
fn baseline_whole_tree_records_only_blocking_findings_by_default() {
    let repo = severity_fixture();
    let run = repo.run(&["baseline", "--write", "--whole-tree"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);

    let recorded = recorded_by_gate(&repo);
    assert_eq!(
        recorded,
        BTreeMap::from([("time-estimates".to_string(), 1)]),
        "a non-blocking warning or note buys nothing in a baseline:\n{}",
        run.stdout
    );

    // Nothing disappears silently: the skipped findings are counted by severity
    // and the flag that records them is named.
    assert!(
        run.stdout.contains("recorded: 1 error"),
        "missing recorded breakdown:\n{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("skipped: 1 warning (suppression-delta: 1), 1 note (pii: 1)"),
        "missing skipped breakdown:\n{}",
        run.stdout
    );
    assert!(run.stdout.contains("--all-severities"), "{}", run.stdout);

    // The dry run reports the same split without writing.
    std::fs::remove_file(repo.file("discipline-baseline.toml")).unwrap();
    let dry = repo.run(&["baseline", "--whole-tree"], &[]);
    assert_eq!(dry.code, 0, "{}{}", dry.stdout, dry.stderr);
    assert!(!repo.file("discipline-baseline.toml").exists());
    assert!(
        dry.stdout
            .contains("Found 1 finding eligible for grandfathering"),
        "{}",
        dry.stdout
    );
    assert!(
        dry.stdout
            .contains("skipped: 1 warning (suppression-delta: 1), 1 note (pii: 1)"),
        "{}",
        dry.stdout
    );
}

#[test]
fn baseline_all_severities_records_warnings_and_notes() {
    let repo = severity_fixture();
    let run = repo.run(
        &["baseline", "--write", "--whole-tree", "--all-severities"],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        recorded_by_gate(&repo),
        BTreeMap::from([
            ("pii".to_string(), 1),
            ("suppression-delta".to_string(), 1),
            ("time-estimates".to_string(), 1),
        ]),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("recorded: 1 error, 1 warning, 1 note"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("skipped: none"), "{}", run.stdout);
}

#[test]
fn baseline_under_fail_on_warnings_records_warnings_but_not_notes() {
    // Under fail-on-warnings a warning blocks, so it is worth grandfathering;
    // a note never blocks. Both ways of turning it on are honoured: the flag,
    // and the environment variable the CI integrations set.
    for (args, env) in [
        (
            vec!["baseline", "--write", "--whole-tree", "--fail-on-warnings"],
            vec![],
        ),
        (
            vec!["baseline", "--write", "--whole-tree"],
            vec![("DISCIPLINE_FAIL_ON_WARNINGS", "true")],
        ),
    ] {
        let repo = severity_fixture();
        let run = repo.run(&args, &env);
        assert_eq!(
            run.code, 0,
            "{args:?} {env:?}\n{}{}",
            run.stdout, run.stderr
        );
        assert_eq!(
            recorded_by_gate(&repo),
            BTreeMap::from([
                ("suppression-delta".to_string(), 1),
                ("time-estimates".to_string(), 1),
            ]),
            "{args:?} {env:?}\n{}",
            run.stdout
        );
        assert!(
            run.stdout.contains("skipped: 1 note (pii: 1)"),
            "{args:?} {env:?}\n{}",
            run.stdout
        );
    }
}

#[test]
fn baseline_default_is_sufficient_for_check_to_pass_and_keeps_warnings_visible() {
    // The default baseline must still let `check` pass on the unchanged debt:
    // every skipped finding is non-blocking, and it stays reported rather than
    // hidden. Diff mode, so every gate in the fixture is in scope for `check`.
    let repo = severity_fixture_with_debt_on("work");
    let run = repo.run(&["baseline", "--write", "--base", "main"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        recorded_by_gate(&repo),
        BTreeMap::from([("time-estimates".to_string(), 1)]),
        "{}",
        run.stdout
    );
    let check = repo.check_with_pr(
        &[],
        "allow-gate-weakening: baseline grandfathering pre-existing findings for adoption",
    );
    assert_eq!(check.code, 0, "{}{}", check.stdout, check.stderr);
    let json = check.json();
    assert_eq!(json["errors"], 0, "{json}");
    assert_eq!(json["baselined"], 1, "{json}");
    assert_eq!(
        check.violations("suppression-delta").len(),
        1,
        "the skipped warning stays visible"
    );
    assert_eq!(
        check.violations("pii").len(),
        1,
        "the skipped note stays visible"
    );
}

// ---- test-floor: runtime counting basis via test_command ---------------------

/// Stands in for `cargo test -- --list`: five `: test` lines across two
/// binaries, plus the lines a real listing interleaves that must not count.
const LISTING_SCRIPT: &str = "\
#!/bin/sh
echo 'unit::adds: test'
echo 'unit::orders: test'
echo 'unit::bench_insert: benchmark'
echo '3 tests, 1 benchmark'
echo 'integration::round_trip: test'
echo 'integration::rejects_empty: test'
echo 'src/lib.rs - read (line 3): test'
echo '3 tests, 0 benchmarks'
";

fn floor_config(min_tests: Option<usize>) -> String {
    let floor = min_tests
        .map(|n| format!("min_tests = {n}\n"))
        .unwrap_or_default();
    format!(
        r#"
[meta]
version = 1
name = "adoption"

[gates.test-floor]
enabled = true
test_command = "sh scripts/list-tests.sh"
{floor}"#
    )
}

#[test]
fn test_floor_ratchet_compares_the_test_command_count_not_the_static_count() {
    // The static count of this repository is 2 (tests/a.rs holds two #[test]
    // functions); the listing reports 5. Each floor below separates the bases.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("scripts/list-tests.sh", LISTING_SCRIPT),
            ("discipline.toml", &floor_config(Some(5))),
        ],
        "chore: floor on the runtime listing",
    );

    // Floor 5 passes only on the runtime basis (static 2 < 5 would fire).
    let at_floor = repo.check(&[]);
    assert_eq!(at_floor.code, 0, "{}{}", at_floor.stdout, at_floor.stderr);
    let outcome = at_floor.outcome("test-floor");
    assert_eq!(outcome["examined"], 5, "{outcome}");
    assert!(at_floor.violations("test-floor").is_empty(), "{outcome}");

    // Floor 6 fires, and the message quotes the runtime count.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("scripts/list-tests.sh", LISTING_SCRIPT),
            ("discipline.toml", &floor_config(Some(6))),
        ],
        "chore: floor above the runtime listing",
    );
    let below = repo.check(&[]);
    assert_eq!(below.code, 1, "{}{}", below.stdout, below.stderr);
    let v = below.violations("test-floor");
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0]["title"], "Test Count Below Floor");
    assert!(
        v[0]["message"]
            .as_str()
            .unwrap()
            .contains("Workspace test count (5) is below the required floor of 6"),
        "{v:?}"
    );
}

#[test]
fn test_floor_test_command_without_an_explicit_floor_fails_closed() {
    // With no floor configured the ratchet would compare the runtime count at
    // HEAD against a STATIC count of the base ref: two different bases. That
    // comparison is refused rather than silently passing or failing.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("scripts/list-tests.sh", LISTING_SCRIPT),
            ("discipline.toml", &floor_config(None)),
        ],
        "chore: runtime listing without a floor",
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("test_command") && run.stderr.contains("min_tests"),
        "the error must name the missing key:\n{}",
        run.stderr
    );
}
