//! The findings a gate reports through `GateOutcome::lift_or_push` (#493): each is reported
//! without a directive, and with the directive that names its subject it is recorded as an
//! override carrying the finding's code instead. Each is driven through the real binary,
//! the reported run beside the lifted one.

mod common;
use common::{Repo, Run};

/// The codes of the findings `gate` reports in `run`.
fn codes(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

/// The `(code, directive)` of each override `gate` recorded in `run`.
fn lifted(run: &Run, gate: &str) -> Vec<(String, String)> {
    run.outcome(gate)["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["code"].as_str().unwrap().to_string(),
                o["directive"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// `code` is reported by `gate` in `reported` and nothing is lifted; in `excused` it is
/// lifted by `directive` and no longer reported.
fn assert_reported_then_lifted(
    reported: &Run,
    excused: &Run,
    gate: &str,
    code: &str,
    directive: &str,
) {
    assert!(
        codes(reported, gate).iter().any(|c| c == code),
        "not reported: {:?}",
        reported.violations(gate)
    );
    assert!(
        lifted(reported, gate).is_empty(),
        "lifted with no directive: {:?}",
        reported.outcome(gate)["overrides"]
    );
    assert!(
        !codes(excused, gate).iter().any(|c| c == code),
        "still reported: {:?}",
        excused.violations(gate)
    );
    assert_eq!(
        lifted(excused, gate),
        vec![(code.to_string(), directive.to_string())],
        "{:?}",
        excused.outcome(gate)["overrides"]
    );
}

#[test]
fn a_delay_added_to_a_test_is_reported_and_lifted_by_the_test_name() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("tests/test_a.py", "def test_a():\n    assert run() == 3\n");
    repo.commit("test: a");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "tests/test_a.py",
        "import time\n\ndef test_a():\n    time.sleep(0.2)\n    assert run() == 3\n",
    );
    repo.commit("test: wait for it");
    let reported = repo.check(&[]);
    let excused = repo.check_with_pr(
        &[],
        "allow-ignore: test_a the fixture warms a cache, tracked in #12\n",
    );
    assert_reported_then_lifted(
        &reported,
        &excused,
        "ignored-tests",
        "ignored-tests/test-sleep-added",
        "allow-ignore",
    );
}

#[test]
fn a_snapshot_added_for_an_existing_test_is_reported_and_lifted_by_its_path() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "crates/x/src/parser.rs",
        "#[test]\nfn parses_empty() {\n    insta::assert_snapshot!(parse(\"\"));\n}\n",
    );
    repo.commit("test: existing test");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "crates/x/src/snapshots/x__parser__parses_empty.snap",
        "---\nsource: parser.rs\n---\n\n",
    );
    repo.commit("test: record");
    let reported = repo.check(&[]);
    let excused = repo.check_with_pr(
        &[],
        "allow-golden-update: crates/x/src/snapshots/x__parser__parses_empty.snap the empty parse is the intended output\n",
    );
    assert_reported_then_lifted(
        &reported,
        &excused,
        "golden-output",
        "golden-output/snapshot-added-for-existing-test",
        "allow-golden-update",
    );
}

#[test]
fn a_deleted_toolchain_configuration_is_reported_and_lifted_by_its_path() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "tsconfig.json",
        "{\"compilerOptions\": {\"strict\": true, \"target\": \"es2022\"}}\n",
    );
    repo.commit("chore: type-check strictly");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.remove("tsconfig.json");
    repo.commit("chore: drop the type-checker configuration");
    let reported = repo.check(&[]);
    let excused = repo.check_with_pr(
        &[],
        "allow-toolchain-weakening: tsconfig.json the package moved to the workspace configuration\n",
    );
    assert_reported_then_lifted(
        &reported,
        &excused,
        "toolchain-config",
        "toolchain-config/toolchain-config-deleted",
        "allow-toolchain-weakening",
    );
}

#[test]
fn a_baseline_migration_mixed_with_another_change_is_reported_and_lifted_by_baseline() {
    let entry = |version: u32, rule: &str| {
        format!(
            "version = {version}\n\n[[findings]]\ngate = \"pii\"\nrule = \"{rule}\"\npath = \"docs/old.md\"\nfingerprint = \"0123456789abcdef\"\n"
        )
    };
    let repo = Repo::new();
    repo.commit_base(
        "discipline-baseline.toml",
        &entry(1, "Host / PII Leak"),
        "chore: version-1 baseline",
    );
    repo.write("discipline-baseline.toml", &entry(2, "pii/host-pii-leak"));
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("chore: migrate the baseline, and notes");
    let reported = repo.check(&[]);
    let excused = repo.check_with_pr(
        &[],
        "allow-gate-weakening: baseline migrated together with the notes it documents\n",
    );
    assert_reported_then_lifted(
        &reported,
        &excused,
        "config-integrity",
        "config-integrity/baseline-migration-not-alone",
        "allow-gate-weakening",
    );
}
