//! Settings that silently did nothing, patterns that silently changed meaning, and a base
//! policy that stopped its own repair (#600). Each defect has a test that failed before
//! its fix and a control that passed before and after.

mod common;
use common::{Repo, Run};

const HEAD: &str = "[meta]\nversion = 1\nname = \"t\"\n";

fn detail(run: &Run) -> String {
    run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn notes(run: &Run, gate: &str) -> String {
    run.outcome(gate)["notes"].to_string()
}

fn codes(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

// ---- command: a pattern that does not compile ---------------------------------------

/// A repository whose base and head carry the same `[gates.command]` table, so the command
/// is not one the change supplies; the change itself touches a document.
fn command_repo(table: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.command]\nenabled = true\n{table}"),
    );
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    repo.commit("docs: plan");
    repo
}

fn assert_output_pattern_rejected(run: &Run, key: &str) {
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("configuration".to_string(), Some("command".to_string()))
    );
    let d = detail(run);
    assert!(
        d.contains(key) && d.contains("not a valid regular expression"),
        "{d}"
    );
}

const FORBID_UNCLOSED_GROUP: &str = "command = \"echo all good\"\nforbid_output = ['(FAILED']\n";
const FORBID_UNCLOSED_GROUP_PRINTED: &str =
    "command = \"echo '(FAILED'\"\nforbid_output = ['(FAILED']\n";
const ZERO_UNCLOSED_CLASS: &str =
    "command = \"echo ran 0 tests\"\nzero_items_pattern = 'ran 0 tests[' \n";
const CANARY_UNCLOSED_GROUP: &str = "command = \"echo ok\"\ncanary_command = \"sh -c 'echo boom; exit 1'\"\ncanary_expected_diagnostic = 'boom('\n";
const ENTRY_FORBID_UNCLOSED: &str = "[[gates.command.commands]]\nname = \"unit\"\ncommand = \"echo ok\"\nforbid_output = ['(FAILED']\n";
const ENTRY_ZERO_UNCLOSED: &str = "[[gates.command.commands]]\nname = \"unit\"\ncommand = \"echo ok\"\nzero_items_pattern = '0 tests ('\n";
const ENTRY_CANARY_UNCLOSED: &str = "[[gates.command.commands]]\nname = \"unit\"\ncommand = \"echo ok\"\ncanary_command = \"sh -c 'echo boom; exit 1'\"\ncanary_expected_diagnostic = 'boom('\n";

/// A `forbid_output` value that does not compile was matched as text. The author's
/// pattern `(FAILED` was meant as a regular expression; as text it guards nothing the
/// expression would have.
#[test]
fn a_forbid_output_pattern_that_does_not_compile_is_a_configuration_error() {
    let run = command_repo(FORBID_UNCLOSED_GROUP).check(&[]);
    assert_output_pattern_rejected(&run, "gates.command.forbid_output");
}

/// The issue's reproduction: the text itself is printed, and the run used to report
/// `forbidden-output` through the literal match.
#[test]
fn a_forbid_output_value_is_never_matched_as_literal_text() {
    let run = command_repo(FORBID_UNCLOSED_GROUP_PRINTED).check(&[]);
    assert_output_pattern_rejected(&run, "gates.command.forbid_output");
}

#[test]
fn a_zero_items_pattern_that_does_not_compile_is_a_configuration_error() {
    let run = command_repo(ZERO_UNCLOSED_CLASS).check(&[]);
    assert_output_pattern_rejected(&run, "gates.command.zero_items_pattern");
}

#[test]
fn a_canary_diagnostic_that_does_not_compile_is_a_configuration_error() {
    let run = command_repo(CANARY_UNCLOSED_GROUP).check(&[]);
    assert_output_pattern_rejected(&run, "gates.command.canary_expected_diagnostic");
}

#[test]
fn the_output_patterns_of_a_commands_entry_are_checked_too() {
    let run = command_repo(ENTRY_FORBID_UNCLOSED).check(&[]);
    assert_output_pattern_rejected(&run, "gates.command.commands[unit].forbid_output");
    let run = command_repo(ENTRY_ZERO_UNCLOSED).check(&[]);
    assert_output_pattern_rejected(&run, "gates.command.commands[unit].zero_items_pattern");
    let run = command_repo(ENTRY_CANARY_UNCLOSED).check(&[]);
    assert_output_pattern_rejected(
        &run,
        "gates.command.commands[unit].canary_expected_diagnostic",
    );
}

/// Under base policy the change's copy is not in force; a pattern that does not compile in
/// it would stop every run once merged.
#[test]
fn base_policy_refuses_a_change_that_adds_an_output_pattern_that_does_not_compile() {
    let repo = command_repo("command = \"echo all good\"\n");
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.command]\nenabled = true\n{FORBID_UNCLOSED_GROUP}"),
    );
    repo.commit("chore: forbid failures");
    let run = repo.check(&["--policy-from", "base"]);
    assert_output_pattern_rejected(&run, "gates.command.forbid_output");
}

/// Control: a value that compiles is matched as a regular expression.
#[test]
fn a_forbid_output_pattern_that_compiles_still_matches() {
    let run =
        command_repo("command = \"echo FAILED7 here\"\nforbid_output = ['FAILED\\d']\n").check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "command"),
        vec!["command/forbidden-output".to_string()]
    );
}

/// Control, and the migration: the escaped form of the text matches the text.
#[test]
fn an_escaped_forbid_output_pattern_matches_the_literal_text() {
    let run =
        command_repo("command = \"echo '(FAILED'\"\nforbid_output = ['\\(FAILED']\n").check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "command"),
        vec!["command/forbidden-output".to_string()]
    );
}

/// Control: output the expression does not match is not forbidden.
#[test]
fn a_forbid_output_pattern_that_does_not_match_passes() {
    let run =
        command_repo("command = \"echo all good\"\nforbid_output = ['FAILED\\d']\n").check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "command").is_empty(), "{}", run.stdout);
}

/// Control: a zero-items pattern that compiles still reports a run of nothing.
#[test]
fn a_zero_items_pattern_that_compiles_still_reports_zero_items() {
    let run = command_repo("command = \"echo ran 0 tests\"\nzero_items_pattern = 'ran 0 \\w+'\n")
        .check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "command"),
        vec!["command/zero-items-executed".to_string()]
    );
}

/// Control: a canary diagnostic that compiles is found in the canary's output, and one
/// that is absent is still reported.
#[test]
fn a_canary_diagnostic_that_compiles_is_still_judged() {
    let found = command_repo(
        "command = \"echo ok\"\ncanary_command = \"sh -c 'echo boom7; exit 1'\"\ncanary_expected_diagnostic = 'boom\\d'\n",
    )
    .check(&[]);
    assert_eq!(found.code, 0, "{}{}", found.stdout, found.stderr);
    let missing = command_repo(
        "command = \"echo ok\"\ncanary_command = \"sh -c 'echo quiet; exit 1'\"\ncanary_expected_diagnostic = 'boom\\d'\n",
    )
    .check(&[]);
    assert_eq!(
        codes(&missing, "command"),
        vec!["command/canary-diagnostic-missing".to_string()],
        "{}",
        missing.stdout
    );
}

/// Control: a gate that is off is not validated.
#[test]
fn a_disabled_command_gate_with_a_bad_output_pattern_does_not_stop_the_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.command]\nenabled = false\n{FORBID_UNCLOSED_GROUP}"),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

// ---- base policy: a base value that does not compile ----------------------------------

const SCOPE_BAD: &str =
    "[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/[a-z\"]\n";
const SCOPE_GOOD: &str =
    "[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/[a-z]*\"]\n";
const SCOPE_OTHER_BAD: &str =
    "[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/{a\"]\n";
const SCOPE_OFF_STILL_BAD: &str =
    "[gates.scope-confinement]\nenabled = false\nforbidden_paths = [\"secrets/**\", \"keys/[a-z\"]\n";
const ESTIMATES_OFF: &str = "[gates.time-estimates]\nenabled = false\n";
const ESTIMATE_DOC: &str = "# Plan\n\nPhase 2 (1 week).\n";
const SCOPE_OTHER_GOOD: &str =
    "[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/[0-9]*\"]\n";
const REPAIR_DIRECTIVE: &str =
    "allow-gate-weakening: scope-confinement the entry replaced never compiled\n";

/// A repository whose base `discipline.toml` is `HEAD` followed by `base`, on `work`.
fn policy_repo(base: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", &format!("{HEAD}{base}"));
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

fn set_policy(repo: &Repo, table: &str, message: &str) {
    repo.write("discipline.toml", &format!("{HEAD}{table}"));
    repo.commit(message);
}

/// The base copy is the one in force and its glob does not compile, so the change that
/// repairs the glob was stopped like any other.
#[test]
fn base_policy_lets_the_repair_of_a_bad_base_glob_through() {
    let repo = policy_repo(SCOPE_BAD);
    set_policy(&repo, SCOPE_GOOD, "fix: close the character class");
    let run = repo.check(&["--policy-from", "base"]);
    assert_ne!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let n = notes(&run, "scope-confinement");
    assert!(
        n.contains("gates.scope-confinement.forbidden_paths")
            && n.contains("does not compile")
            && n.contains("change's own value"),
        "{n}"
    );
}

/// The repair is reported as any edit of that list is: replacing an entry of
/// `forbidden_paths` loses one, which `config-integrity` reports whether or not the entry
/// it lost compiled. The findings are those of the same edit over a base that compiles.
#[test]
fn config_integrity_reports_the_repair_as_it_reports_any_edit_of_the_list() {
    let repaired = policy_repo(SCOPE_BAD);
    set_policy(&repaired, SCOPE_GOOD, "fix: close the character class");
    let repair = repaired.check(&["--policy-from", "base"]);
    let ordinary = policy_repo(SCOPE_OTHER_GOOD);
    set_policy(&ordinary, SCOPE_GOOD, "chore: forbid keys");
    let edit = ordinary.check(&["--policy-from", "base"]);
    assert_eq!(
        codes(&edit, "config-integrity"),
        vec!["config-integrity/gate-weakened".to_string()],
        "{}{}",
        edit.stdout,
        edit.stderr
    );
    assert_eq!(repair.code, edit.code, "{}{}", repair.stdout, repair.stderr);
    assert_eq!(
        repair.violations("config-integrity"),
        edit.violations("config-integrity")
    );
    assert!(
        notes(&repair, "config-integrity").contains("gates.scope-confinement.forbidden_paths"),
        "{}",
        repair.stdout
    );
    // Control: the ordinary edit took nothing from the change and says nothing of the kind.
    assert!(
        !notes(&edit, "config-integrity").contains("change's own value"),
        "{}",
        edit.stdout
    );
}

/// The repair lands the way any reviewed edit of that list does: with the directive.
#[test]
fn the_repair_passes_with_the_directive_any_such_edit_needs() {
    let repo = policy_repo(SCOPE_BAD);
    set_policy(&repo, SCOPE_GOOD, "fix: close the character class");
    let run = repo.check_with_pr(&["--policy-from", "base"], REPAIR_DIRECTIVE);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(codes(&run, "config-integrity").is_empty(), "{}", run.stdout);
}

/// The change's value is in force for the repaired key: a path only its glob forbids is
/// reported.
#[test]
fn the_repaired_glob_is_the_one_the_gate_matches_with() {
    let repo = policy_repo(SCOPE_BAD);
    repo.write("keys/abc.txt", "k\n");
    set_policy(&repo, SCOPE_GOOD, "fix: close the character class");
    let run = repo.check(&["--policy-from", "base"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let found = run.violations("scope-confinement");
    assert_eq!(found.len(), 1, "{}", run.stdout);
    assert_eq!(found[0]["file"], "keys/abc.txt");
    assert_eq!(
        found[0]["code"],
        "scope-confinement/file-in-forbidden-scope"
    );
}

/// Only the key that does not compile is taken from the change: a gate the same change
/// switches off is still on, as the base copy says.
#[test]
fn the_repair_takes_nothing_else_from_the_change() {
    let repo = policy_repo(SCOPE_BAD);
    repo.write("docs/plan.md", ESTIMATE_DOC);
    set_policy(
        &repo,
        &format!("{SCOPE_GOOD}{ESTIMATES_OFF}"),
        "fix: close the character class",
    );
    let run = repo.check(&["--policy-from", "base"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let estimates = run.outcome("time-estimates");
    assert_eq!(estimates["enabled"], true, "{estimates}");
    assert_eq!(
        estimates["violations"].as_array().unwrap().len(),
        1,
        "{estimates}"
    );
}

/// A base pattern that does not compile is repaired the same way.
#[test]
fn base_policy_lets_the_repair_of_a_bad_base_pattern_through() {
    let repo = policy_repo(
        "[gates.command]\nenabled = true\ncommand = \"echo 3 passed\"\ncount_pattern = '(\\d+ passed'\n",
    );
    set_policy(
        &repo,
        "[gates.command]\nenabled = true\ncommand = \"echo 3 passed\"\ncount_pattern = '(\\d+) passed'\n",
        "fix: close the group",
    );
    let run = repo.check(&["--policy-from", "base"]);
    assert_ne!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let n = notes(&run, "command");
    assert!(
        n.contains("gates.command.count_pattern") && n.contains("change's own value"),
        "{n}"
    );
}

fn assert_stopped_on_the_base_glob(run: &Run) {
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("gate".to_string(), Some("scope-confinement".to_string()))
    );
    let d = detail(run);
    assert!(
        d.contains("invalid glob") && d.contains("gates.scope-confinement.forbidden_paths"),
        "{d}"
    );
}

/// Control: a change that leaves the base glob as it is still stops.
#[test]
fn base_policy_still_stops_a_change_that_leaves_the_bad_glob() {
    let repo = policy_repo(SCOPE_BAD);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    repo.commit("docs: plan");
    let run = repo.check(&["--policy-from", "base"]);
    assert_stopped_on_the_base_glob(&run);
    assert!(detail(&run).contains("keys/[a-z"), "{}", detail(&run));
}

/// Control: a change that replaces the bad glob with another bad one still stops.
#[test]
fn base_policy_still_stops_a_change_that_swaps_one_bad_glob_for_another() {
    let repo = policy_repo(SCOPE_BAD);
    set_policy(&repo, SCOPE_OTHER_BAD, "fix: try braces");
    let run = repo.check(&["--policy-from", "base"]);
    assert_stopped_on_the_base_glob(&run);
}

/// Control: switching the gate off at the head leaves its glob unchecked there, and is no
/// repair: the base copy still has the gate on and the value still does not compile.
#[test]
fn base_policy_still_stops_a_change_that_only_switches_the_gate_off() {
    let repo = policy_repo(SCOPE_BAD);
    set_policy(&repo, SCOPE_OFF_STILL_BAD, "chore: gate off");
    let run = repo.check(&["--policy-from", "base"]);
    assert_stopped_on_the_base_glob(&run);
}

/// Control: a change that introduces a bad glob over a base that compiles still stops.
#[test]
fn base_policy_still_stops_a_change_that_introduces_a_bad_glob() {
    let repo = policy_repo(SCOPE_GOOD);
    set_policy(&repo, SCOPE_BAD, "chore: forbid more");
    let run = repo.check(&["--policy-from", "base"]);
    assert_stopped_on_the_base_glob(&run);
}

/// Control: under the default policy side the change's own copy is in force, and a
/// repaired copy needs no substitution and leaves no note.
#[test]
fn head_policy_has_nothing_to_substitute() {
    let repo = policy_repo(SCOPE_BAD);
    set_policy(&repo, SCOPE_GOOD, "fix: close the character class");
    let run = repo.check(&[]);
    assert_ne!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(
        !notes(&run, "scope-confinement").contains("change's own value"),
        "{}",
        run.stdout
    );
}

// ---- ci-integrity: the documented job count -------------------------------------------

/// A workflow with a verification job and a rollup; `extra` is appended to the test command.
fn workflow(extra: &str) -> String {
    format!(
        "name: ci\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test{extra}\n  gate:\n    needs: [test]\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n"
    )
}

const WF: &str = ".github/workflows/ci.yml";
const COUNT_PATH_ONLY: &str =
    "[gates.ci-integrity]\nrollup_job = \"gate\"\ndocumented_job_count_path = \"docs/ci.md\"\n";
const COUNT_PATTERN_ONLY: &str = "[gates.ci-integrity]\nrollup_job = \"gate\"\ndocumented_job_count_pattern = 'There are (\\d+) jobs'\n";
const COUNT_BOTH: &str = "[gates.ci-integrity]\nrollup_job = \"gate\"\ndocumented_job_count_path = \"docs/ci.md\"\ndocumented_job_count_pattern = 'There are (\\d+) jobs'\n";
const COUNT_BOTH_NO_SUCH_ROLLUP: &str = "[gates.ci-integrity]\nrollup_job = \"all-green\"\ndocumented_job_count_path = \"docs/ci.md\"\ndocumented_job_count_pattern = 'There are (\\d+) jobs'\n";
const COUNT_PATH_ONLY_GATE_OFF: &str =
    "[gates.ci-integrity]\nenabled = false\ndocumented_job_count_path = \"docs/ci.md\"\n";

/// A repository whose base has a two-job workflow and a document stating `doc`, and whose
/// change edits the workflow under the configuration `table`.
fn job_count_repo(doc: &str, table: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(WF, &workflow(""));
    repo.write("docs/ci.md", doc);
    repo.commit("ci: add");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(WF, &workflow(" --all"));
    repo.write("discipline.toml", &format!("{HEAD}{table}"));
    repo.commit("ci: test everything");
    repo
}

fn assert_half_configured_job_count(run: &Run) {
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        (
            "configuration".to_string(),
            Some("ci-integrity".to_string())
        )
    );
    let d = detail(run);
    assert!(
        d.contains("gates.ci-integrity.documented_job_count_path")
            && d.contains("gates.ci-integrity.documented_job_count_pattern"),
        "{d}"
    );
}

/// The count was compared only when both keys were set; one alone did nothing, silently.
#[test]
fn a_job_count_path_without_its_pattern_is_a_configuration_error() {
    let run = job_count_repo("There are 9 jobs.\n", COUNT_PATH_ONLY).check(&[]);
    assert_half_configured_job_count(&run);
}

#[test]
fn a_job_count_pattern_without_its_path_is_a_configuration_error() {
    let run = job_count_repo("There are 9 jobs.\n", COUNT_PATTERN_ONLY).check(&[]);
    assert_half_configured_job_count(&run);
}

/// Checked with the configuration, not when a workflow happens to change.
#[test]
fn a_half_configured_job_count_is_found_when_no_workflow_changed() {
    let repo = Repo::new();
    repo.write("discipline.toml", &format!("{HEAD}{COUNT_PATH_ONLY}"));
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_half_configured_job_count(&run);
}

/// Both keys set, and no examined workflow has the rollup job: the count was not compared
/// and nothing said so.
#[test]
fn a_job_count_with_no_rollup_job_to_count_under_leaves_a_note() {
    let run = job_count_repo("There are 9 jobs.\n", COUNT_BOTH_NO_SUCH_ROLLUP).check(&[]);
    assert!(
        !codes(&run, "ci-integrity").contains(&"ci-integrity/job-count-mismatch".to_string()),
        "{}",
        run.stdout
    );
    let n = notes(&run, "ci-integrity");
    assert!(
        n.contains("all-green") && n.contains("job count was not compared"),
        "{n}"
    );
}

/// Control: with both keys and the rollup job, the count is compared, and nothing claims
/// it was not.
#[test]
fn a_fully_configured_job_count_is_still_compared() {
    let wrong = job_count_repo("There are 9 jobs.\n", COUNT_BOTH).check(&[]);
    assert_eq!(wrong.code, 1, "{}{}", wrong.stdout, wrong.stderr);
    assert_eq!(
        codes(&wrong, "ci-integrity"),
        vec!["ci-integrity/job-count-mismatch".to_string()]
    );
    let right = job_count_repo("There are 2 jobs.\n", COUNT_BOTH).check(&[]);
    assert_eq!(right.code, 0, "{}{}", right.stdout, right.stderr);
    assert!(
        !notes(&right, "ci-integrity").contains("not compared"),
        "{}",
        right.stdout
    );
}

/// Control: a gate that is off is not validated.
#[test]
fn a_disabled_gate_with_a_half_configured_job_count_does_not_stop_the_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}{COUNT_PATH_ONLY_GATE_OFF}"),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// `--staged` judges the index. The document was read from the working tree, so an
/// unstaged edit decided the result.
#[test]
fn a_staged_run_reads_the_staged_job_count_document() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(WF, &workflow(""));
    repo.write("docs/ci.md", "There are 2 jobs.\n");
    repo.write("discipline.toml", &format!("{HEAD}{COUNT_BOTH}"));
    repo.commit("ci: add");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(WF, &workflow(" --all"));
    repo.git(&["add", "-A"]);
    // Staged: 2 (right). Working tree, not staged: 9 (wrong).
    repo.write("docs/ci.md", "There are 9 jobs.\n");
    let run = repo.check(&["--staged"]);
    assert!(
        codes(&run, "ci-integrity").is_empty(),
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.outcome("ci-integrity")["examined"], 1);
}

/// The reverse: the staged document is wrong and the working tree hides it.
#[test]
fn a_staged_run_is_not_satisfied_by_an_unstaged_job_count_document() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(WF, &workflow(""));
    repo.write("docs/ci.md", "There are 2 jobs.\n");
    repo.write("discipline.toml", &format!("{HEAD}{COUNT_BOTH}"));
    repo.commit("ci: add");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(WF, &workflow(" --all"));
    repo.write("docs/ci.md", "There are 9 jobs.\n");
    repo.git(&["add", "-A"]);
    repo.write("docs/ci.md", "There are 2 jobs.\n");
    let run = repo.check(&["--staged"]);
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/job-count-mismatch".to_string()],
        "{}{}",
        run.stdout,
        run.stderr
    );
}

/// Control: a staged document removed from the index is a missing document.
#[test]
fn a_staged_run_reports_a_job_count_document_removed_from_the_index() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(WF, &workflow(""));
    repo.write("docs/ci.md", "There are 2 jobs.\n");
    repo.write("discipline.toml", &format!("{HEAD}{COUNT_BOTH}"));
    repo.commit("ci: add");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(WF, &workflow(" --all"));
    repo.git(&["add", "-A"]);
    repo.git(&["rm", "-q", "--cached", "docs/ci.md"]);
    let run = repo.check(&["--staged"]);
    assert!(
        codes(&run, "ci-integrity").contains(&"ci-integrity/job-count-file-missing".to_string()),
        "{}{}",
        run.stdout,
        run.stderr
    );
}

// ---- ci-integrity: an action file that does not parse ----------------------------------

const ACTION: &str = "action.yml";
const BASE_ACTION: &str = "name: build\ndescription: build it\nruns:\n  using: composite\n  steps:\n    - run: cargo build\n      shell: bash\n";
const BROKEN_ACTION: &str =
    "name: build\ndescription: build it\nruns:\n  using: composite\n  steps: [\n";
const ACTION_WITH_TAG_REF: &str = "name: build\ndescription: build it\nruns:\n  using: composite\n  steps:\n    - uses: someone/tool@v1\n    - run: cargo build\n      shell: bash\n";
const WORKFLOWS_WITH_ACTION: &str =
    "[gates.ci-integrity]\nworkflows = [\".github/workflows/**\", \"action.yml\"]\n[gates.sandbox-config]\nenabled = false\n";

fn action_repo() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", &format!("{HEAD}{WORKFLOWS_WITH_ACTION}"));
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

/// An existing action file whose head side no longer parses checks nothing of what its
/// base side had; only a note said so, while the same condition in a workflow is a finding.
#[test]
fn an_action_file_whose_head_side_does_not_parse_is_a_finding() {
    let repo = action_repo();
    repo.commit_base(ACTION, BASE_ACTION, "ci: add action");
    repo.write(ACTION, BROKEN_ACTION);
    repo.commit("ci: rework action");
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/pipeline-file-unreadable".to_string()],
        "{}",
        run.stdout
    );
    let out = run.outcome("ci-integrity");
    assert_eq!(out["violations"][0]["file"], ACTION, "{out}");
    // Nothing in the file was examined.
    assert_eq!(out["examined"], 0, "{out}");
    assert!(
        out["notes"].to_string().contains("does not parse as YAML"),
        "{out}"
    );
}

/// Control: a new action file has no base side to be weakened against; it keeps its note.
#[test]
fn a_new_action_file_that_does_not_parse_is_a_note_only() {
    let repo = action_repo();
    repo.write(ACTION, BROKEN_ACTION);
    repo.commit("ci: add action");
    let run = repo.check(&[]);
    assert!(codes(&run, "ci-integrity").is_empty(), "{}", run.stdout);
    assert!(
        notes(&run, "ci-integrity").contains("does not parse as YAML"),
        "{}",
        run.stdout
    );
}

/// Control: an action file that parses is still judged by its steps.
#[test]
fn an_action_file_that_parses_is_still_judged_by_its_steps() {
    let repo = action_repo();
    repo.commit_base(ACTION, BASE_ACTION, "ci: add action");
    repo.write(ACTION, ACTION_WITH_TAG_REF);
    repo.commit("ci: use a tool");
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/unpinned-action".to_string()],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome("ci-integrity")["examined"], 1);
}

// ---- baseline: the path in the message --------------------------------------------------

const NOT_TOML: &str = "this is = = not toml [\n";

/// The message named the file by its absolute path, which carries the runner's directory
/// layout into the report.
#[test]
fn a_baseline_inside_the_repository_is_named_by_its_relative_path() {
    let repo = Repo::new();
    repo.write("discipline-baseline.toml", NOT_TOML);
    repo.commit("chore: baseline");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("baseline".to_string(), None));
    let d = detail(&run);
    let dir = repo.path().file_name().unwrap().to_str().unwrap();
    assert!(
        d.contains("`discipline-baseline.toml`") && !d.contains(dir),
        "{d}"
    );
    assert!(!run.stderr.contains(dir), "{}", run.stderr);
}

/// The same for a baseline named on the command line that does not exist.
#[test]
fn a_missing_baseline_inside_the_repository_is_named_by_its_relative_path() {
    let repo = Repo::new();
    let run = repo.check(&["--baseline-file", "policy/known.toml"]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let d = detail(&run);
    let dir = repo.path().file_name().unwrap().to_str().unwrap();
    assert!(d.contains("`policy/known.toml`") && !d.contains(dir), "{d}");
}

/// A baseline outside the repository has no relative name; only its file name is printed,
/// since its directory is the runner's.
#[test]
fn a_baseline_outside_the_repository_is_named_by_its_file_name() {
    let repo = Repo::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("known.toml");
    std::fs::write(&outside, NOT_TOML).unwrap();
    let outside = outside.to_str().unwrap();
    let run = repo.check(&["--baseline-file", outside]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("baseline".to_string(), None));
    let d = detail(&run);
    let dir = elsewhere.path().file_name().unwrap().to_str().unwrap();
    assert!(d.contains("`known.toml`") && !d.contains(dir), "{d}");
    assert!(!run.stderr.contains(dir), "{}", run.stderr);
}

// ---- test-budget: fuzz_targets -----------------------------------------------------------

const FUZZ_MANIFEST: &str = "[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n\n[[bin]]\nname = \"parse\"\npath = \"fuzz_targets/parse.rs\"\n\n[[bin]]\nname = \"encode\"\npath = \"fuzz_targets/encode.rs\"\n";
const FUZZ_MANIFEST_ONE_LEFT: &str = "[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n\n[[bin]]\nname = \"parse\"\npath = \"fuzz_targets/parse.rs\"\n";
const FUZZ_HARNESS: &str = "pub fn run(data: &[u8]) -> usize {\n    data.len()\n}\n";
const NESTED_FUZZ: &str = "[gates.test-budget]\nfuzz_targets = [\"crates/*/fuzz/Cargo.toml\", \"crates/*/fuzz/fuzz_targets/**\"]\n";
const BAD_FUZZ_GLOB: &str = "[gates.test-budget]\nfuzz_targets = [\"crates/[a-z/fuzz/**\"]\n";

/// A repository whose base has a fuzz crate under `dir` with two targets, under `table`.
fn fuzz_repo(dir: &str, table: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", &format!("{HEAD}{table}"));
    repo.write(&format!("{dir}/Cargo.toml"), FUZZ_MANIFEST);
    repo.write(&format!("{dir}/fuzz_targets/parse.rs"), FUZZ_HARNESS);
    repo.write(&format!("{dir}/fuzz_targets/encode.rs"), FUZZ_HARNESS);
    repo.commit("test: fuzz crate");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

fn drop_encode_target(repo: &Repo, dir: &str) -> Run {
    repo.write(&format!("{dir}/Cargo.toml"), FUZZ_MANIFEST_ONE_LEFT);
    repo.remove(&format!("{dir}/fuzz_targets/encode.rs"));
    repo.commit("test: drop the encode target");
    repo.check(&[])
}

fn sorted(mut codes: Vec<String>) -> Vec<String> {
    codes.sort();
    codes
}

/// `fuzz_targets` was never read: a fuzz crate anywhere but `fuzz/` was not watched,
/// whatever the setting said.
#[test]
fn fuzz_targets_globs_name_the_fuzz_crate_to_watch() {
    let repo = fuzz_repo("crates/core/fuzz", NESTED_FUZZ);
    let run = drop_encode_target(&repo, "crates/core/fuzz");
    assert_eq!(
        sorted(codes(&run, "test-budget")),
        vec![
            "test-budget/fuzz-target-deleted".to_string(),
            "test-budget/fuzz-target-removed".to_string()
        ],
        "{}",
        run.stdout
    );
}

/// Control: a nested fuzz crate the setting does not name is not watched.
#[test]
fn a_fuzz_crate_no_glob_names_is_not_watched() {
    let repo = fuzz_repo("crates/core/fuzz", "");
    let run = drop_encode_target(&repo, "crates/core/fuzz");
    assert!(codes(&run, "test-budget").is_empty(), "{}", run.stdout);
}

/// Control: the default configuration reports what it reported: the `fuzz/` crate's
/// manifest and any deleted Rust file under `fuzz/`, outside `fuzz_targets/` too.
#[test]
fn the_default_fuzz_targets_report_what_they_reported() {
    let repo = fuzz_repo("fuzz", "");
    repo.commit_base("fuzz/src/support.rs", FUZZ_HARNESS, "test: fuzz support");
    repo.remove("fuzz/src/support.rs");
    let run = drop_encode_target(&repo, "fuzz");
    assert_eq!(
        sorted(codes(&run, "test-budget")),
        vec![
            "test-budget/fuzz-target-deleted".to_string(),
            "test-budget/fuzz-target-deleted".to_string(),
            "test-budget/fuzz-target-removed".to_string()
        ],
        "{}",
        run.stdout
    );
}

/// A glob that does not compile in `fuzz_targets` was never compiled, so never reported.
#[test]
fn an_invalid_fuzz_targets_glob_is_a_configuration_error() {
    let repo = Repo::new();
    repo.write("discipline.toml", &format!("{HEAD}{BAD_FUZZ_GLOB}"));
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("gate".to_string(), Some("test-budget".to_string()))
    );
    let d = detail(&run);
    assert!(
        d.contains("invalid glob") && d.contains("gates.test-budget.fuzz_targets"),
        "{d}"
    );
}
