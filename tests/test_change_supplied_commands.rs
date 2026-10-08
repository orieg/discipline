//! A change under review cannot make discipline execute a command the change supplies.
//!
//! Each case commits a configuration on `main`, changes it on `work`, and runs the real
//! binary against `main`. The command the change supplies creates a marker file, so a
//! case fails when the command ran, whatever the report says.

mod common;
use common::{Repo, CONFIG_HEAD};

const MARKER: &str = "RAN_BY_CHANGE";
const COMMAND_FINDING: &str = "command/untrusted-command-modification";
const TEST_COMMAND_FINDING: &str = "test-floor/untrusted-test-command";

/// A command that proves it ran and then fails, as a negative-control canary should.
fn marking_canary() -> String {
    format!("canary_command = \"sh -c 'touch {MARKER}; exit 1'\"\n")
}

/// A test listing that proves it ran and reports a count no floor is above.
fn marking_test_command() -> String {
    format!("test_command = \"sh -c 'touch {MARKER}; echo 99999'\"\n")
}

fn config(gates: &str) -> String {
    format!("{CONFIG_HEAD}\n{gates}")
}

/// `base` committed on `main`, then `head` committed on `work`.
fn repo_with(base: &str, head: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base("discipline.toml", &config(base), "ci: base configuration");
    repo.write("discipline.toml", &config(head));
    repo.commit("chore: change the configuration");
    repo
}

fn codes(run: &common::Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

/// The command gate refused the change: its finding, nothing examined, nothing run.
fn assert_command_refused(repo: &Repo) {
    let run = repo.check(&[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        codes(&run, "command"),
        vec![COMMAND_FINDING.to_string()],
        "marker exists: {ran}; exit {}\n{}{}",
        run.code,
        run.stdout,
        run.stderr
    );
    assert_eq!(run.outcome("command")["examined"], 0);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(!ran, "the command the change supplied was executed");
}

/// The test-floor gate refused the change: its finding, no count taken, nothing run.
fn assert_test_command_refused(repo: &Repo) {
    let run = repo.check(&[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        codes(&run, "test-floor"),
        vec![TEST_COMMAND_FINDING.to_string()],
        "marker exists: {ran}; exit {}\n{}{}",
        run.code,
        run.stdout,
        run.stderr
    );
    assert_eq!(run.outcome("test-floor")["examined"], 0);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(!ran, "the test command the change supplied was executed");
}

// ---- command: the table-level canary ------------------------------------------

#[test]
fn a_table_canary_added_by_the_change_is_not_run() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(base, &format!("{base}{}", marking_canary()));
    assert_command_refused(&repo);
}

/// Entries inherit the table's canary, so a table-level key reaches every entry.
#[test]
fn a_table_canary_added_over_an_entry_is_not_run() {
    let entry = "[[gates.command.commands]]\nname = \"unit\"\ncommand = \"true\"\n";
    let repo = repo_with(
        &format!("[gates.command]\n\n{entry}"),
        &format!("[gates.command]\n{}\n{entry}", marking_canary()),
    );
    assert_command_refused(&repo);
}

/// The `sanitizers` preset has its own canary; the table-level key replaces it.
#[test]
fn a_table_canary_added_over_a_preset_default_is_not_run() {
    let base = "[gates.command]\npreset = \"sanitizers\"\n";
    let repo = repo_with(base, &format!("{base}{}", marking_canary()));
    assert_command_refused(&repo);
}

#[test]
fn a_table_canary_repointed_by_the_change_is_not_run() {
    let table = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(
        &format!("{table}canary_command = \"false\"\n"),
        &format!("{table}{}", marking_canary()),
    );
    assert_command_refused(&repo);
}

/// Control: a canary both sides declare still runs, and its verdict is still reported.
#[test]
fn an_unchanged_table_canary_still_runs() {
    let gates = "[gates.command]\ncommand = \"true\"\n\
                 canary_command = \"sh -c 'touch CANARY_RAN; echo harmless'\"\n\
                 canary_expected_diagnostic = \"EXPECTED_DIAGNOSTIC\"\n";
    let repo = Repo::new();
    repo.commit_base("discipline.toml", &config(gates), "ci: base configuration");
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1, Phase 2, then Phase 3.\n",
    );
    repo.commit("docs: extend the plan");

    let run = repo.check(&[]);
    assert!(
        repo.file("CANARY_RAN").exists(),
        "{}{}",
        run.stdout,
        run.stderr
    );
    let found = codes(&run, "command");
    assert!(
        found.contains(&"command/canary-diagnostic-missing".to_string())
            && found.contains(&"command/canary-command-succeeded".to_string()),
        "{found:?}"
    );
    assert!(!found.contains(&COMMAND_FINDING.to_string()), "{found:?}");
    assert_eq!(run.outcome("command")["examined"], 1);
}

/// Control: the runner's environment authorises a changed command.
#[test]
fn the_runner_environment_authorises_a_changed_canary() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(base, &format!("{base}{}", marking_canary()));
    let run = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("DISCIPLINE_ALLOW_COMMAND_CHANGE", "1")],
    );
    assert!(repo.file(MARKER).exists(), "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "command").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("command")["examined"], 1);
}

// ---- test-floor: test_command ---------------------------------------------------

#[test]
fn a_test_command_added_by_the_change_is_not_run() {
    let base = "[gates.test-floor]\nmin_tests = 1\n";
    let repo = repo_with(base, &format!("{base}{}", marking_test_command()));
    assert_test_command_refused(&repo);
}

/// With no floor configured the gate used to stop at a configuration error; the change
/// supplying the command is the finding either way, and nothing runs.
#[test]
fn a_test_command_added_without_a_floor_is_not_run() {
    let repo = repo_with(
        "[gates.test-floor]\nenabled = true\n",
        &format!("[gates.test-floor]\n{}", marking_test_command()),
    );
    assert_test_command_refused(&repo);
}

/// No base configuration at all: a `test_command` on head is supplied by the change.
#[test]
fn a_test_command_in_a_configuration_the_change_adds_is_not_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &config(&format!(
            "[gates.test-floor]\nmin_tests = 1\n{}",
            marking_test_command()
        )),
    );
    repo.commit("chore: add a configuration");
    assert_test_command_refused(&repo);
}

#[test]
fn a_test_command_repointed_by_the_change_is_not_run() {
    let table = "[gates.test-floor]\nmin_tests = 1\n";
    let repo = repo_with(
        &format!("{table}test_command = \"echo 7\"\n"),
        &format!("{table}{}", marking_test_command()),
    );
    assert_test_command_refused(&repo);
}

/// Control: a `test_command` both sides declare still runs and its count is the one used.
#[test]
fn an_unchanged_test_command_still_runs_and_its_count_is_used() {
    let gates = "[gates.test-floor]\nmin_tests = 1\n\
                 test_command = \"sh -c 'touch LISTING_RAN; echo 7'\"\n";
    let repo = Repo::new();
    repo.commit_base("discipline.toml", &config(gates), "ci: base configuration");
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1, Phase 2, then Phase 3.\n",
    );
    repo.commit("docs: extend the plan");

    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(repo.file("LISTING_RAN").exists());
    assert_eq!(run.outcome("test-floor")["examined"], 7);
    assert!(codes(&run, "test-floor").is_empty());
}

/// Control: the same switch as the command gate authorises a changed `test_command`.
#[test]
fn the_runner_environment_authorises_a_changed_test_command() {
    let base = "[gates.test-floor]\nmin_tests = 1\n";
    let repo = repo_with(base, &format!("{base}{}", marking_test_command()));
    let run = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("DISCIPLINE_ALLOW_COMMAND_CHANGE", "1")],
    );
    assert!(repo.file(MARKER).exists(), "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["examined"], 99999);
}

/// Control: under `--policy-from base` the base copy is in force, so the head's command
/// is not what runs and there is no modification to report.
#[test]
fn under_base_policy_the_base_test_command_runs_and_nothing_is_reported() {
    let table = "[gates.test-floor]\nmin_tests = 1\n";
    let repo = repo_with(
        &format!("{table}test_command = \"sh -c 'touch BASE_LISTING_RAN; echo 7'\"\n"),
        &format!("{table}{}", marking_test_command()),
    );
    let run = repo.check(&["--policy-from", "base"]);
    assert!(
        !repo.file(MARKER).exists(),
        "the head's test command ran under the base policy"
    );
    assert!(
        repo.file("BASE_LISTING_RAN").exists(),
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert!(codes(&run, "test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["examined"], 7);
}
