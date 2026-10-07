//! A configured pattern that cannot do its job, and a read or parse that fails, stop the
//! run or leave a note; neither reads as "nothing found" (#567). Each defect has a test
//! that failed before its fix and a control that passed before and after.

mod common;
use common::{Repo, Run};

const HEAD: &str = common::CONFIG_HEAD;

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

/// A workflow with a verification job and a rollup; `extra` is appended to the test command.
fn workflow(extra: &str) -> String {
    format!(
        "name: ci\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test{extra}\n  gate:\n    needs: [test]\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n"
    )
}

const WF: &str = ".github/workflows/ci.yml";

/// A repository whose base has a two-job workflow and a document stating `doc`, and whose
/// change edits the workflow under a configuration with `pattern`.
fn job_count_repo(doc: &[u8], pattern: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(WF, &workflow(""));
    std::fs::write(repo.file("docs/ci.md"), doc).unwrap();
    repo.commit("ci: add");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(WF, &workflow(" --all"));
    repo.write("discipline.toml", &job_count_config(pattern));
    repo.commit("ci: test everything");
    repo
}

fn job_count_config(pattern: &str) -> String {
    format!(
        "{HEAD}[gates.ci-integrity]\nrollup_job = \"gate\"\ndocumented_job_count_path = \"docs/ci.md\"\ndocumented_job_count_pattern = '{pattern}'\n"
    )
}

fn assert_job_count_pattern_rejected(run: &Run, needle: &str) {
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
        d.contains("gates.ci-integrity.documented_job_count_pattern") && d.contains(needle),
        "{d}"
    );
}

/// The capture was indexed without a check, so a pattern with no group aborted the run.
#[test]
fn a_job_count_pattern_without_a_capture_group_is_a_configuration_error() {
    let repo = job_count_repo(b"There are 3 jobs.\n", r"There are \d+ jobs");
    let run = repo.check(&[]);
    assert!(!run.stderr.contains("panicked"), "{}", run.stderr);
    assert_job_count_pattern_rejected(&run, "capture group");
}

/// A pattern that does not compile used to switch the documented-count check off.
#[test]
fn a_job_count_pattern_that_does_not_compile_is_a_configuration_error() {
    let repo = job_count_repo(b"There are 3 jobs.\n", r"There are (\d+ jobs");
    let run = repo.check(&[]);
    assert_job_count_pattern_rejected(&run, "not a valid regular expression");
}

/// The pattern is checked with the configuration, not when a workflow happens to change.
#[test]
fn a_job_count_pattern_is_checked_when_no_workflow_changed() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(WF, &workflow(""));
    repo.write("docs/ci.md", "There are 2 jobs.\n");
    repo.write("discipline.toml", &job_count_config(r"There are \d+ jobs"));
    repo.commit("ci: add");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    repo.commit("docs: plan");
    let run = repo.check(&[]);
    assert_job_count_pattern_rejected(&run, "capture group");
}

/// Under base policy the change's copy is not in force; a pattern it adds is still checked,
/// or it would stop every run once merged.
#[test]
fn base_policy_refuses_a_change_that_adds_a_job_count_pattern_without_a_group() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", HEAD);
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("discipline.toml", &job_count_config(r"There are \d+ jobs"));
    repo.commit("chore: document the job count");
    let run = repo.check(&["--policy-from", "base"]);
    assert_job_count_pattern_rejected(&run, "capture group");
}

/// Control: a pattern with a group still finds a count that disagrees with the workflow.
#[test]
fn a_valid_job_count_pattern_still_reports_a_mismatch() {
    let repo = job_count_repo(b"There are 3 jobs.\n", r"There are (\d+) jobs");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/job-count-mismatch".to_string()]
    );
}

/// Control: a count that agrees is not a finding and leaves no note about the document.
#[test]
fn a_valid_job_count_pattern_accepts_a_matching_count() {
    let repo = job_count_repo(b"There are 2 jobs.\n", r"There are (\d+) jobs");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(!notes(&run, "ci-integrity").contains("docs/ci.md"));
}

/// Control: a gate that is off is not validated.
#[test]
fn a_disabled_gate_with_a_bad_job_count_pattern_does_not_stop_the_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.ci-integrity]\nenabled = false\ndocumented_job_count_pattern = '(a'\n"
        ),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// A document the pattern no longer matches used to end the comparison without a word.
#[test]
fn a_job_count_document_the_pattern_does_not_match_leaves_a_note() {
    let repo = job_count_repo(b"The jobs are listed below.\n", r"There are (\d+) jobs");
    let run = repo.check(&[]);
    let n = notes(&run, "ci-integrity");
    assert!(
        n.contains("docs/ci.md") && n.contains("does not match") && n.contains("not compared"),
        "{n}"
    );
}

#[test]
fn a_job_count_capture_that_is_not_a_number_leaves_a_note() {
    let repo = job_count_repo(b"There are many jobs.\n", r"There are (\w+) jobs");
    let run = repo.check(&[]);
    let n = notes(&run, "ci-integrity");
    assert!(
        n.contains("docs/ci.md") && n.contains("not a number") && n.contains("not compared"),
        "{n}"
    );
}

/// A document that is not UTF-8 failed its read, which was dropped.
#[test]
fn a_job_count_document_that_cannot_be_read_as_text_leaves_a_note() {
    let repo = job_count_repo(b"There are 3 jobs.\n\xff\xfe\n", r"There are (\d+) jobs");
    let run = repo.check(&[]);
    let n = notes(&run, "ci-integrity");
    assert!(
        n.contains("docs/ci.md") && n.contains("could not be read") && n.contains("not compared"),
        "{n}"
    );
}

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

fn assert_count_pattern_rejected(run: &Run, key: &str, needle: &str) {
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("configuration".to_string(), Some("command".to_string()))
    );
    let d = detail(run);
    assert!(d.contains(key) && d.contains(needle), "{d}");
}

/// A `count_pattern` that does not compile extracted no count, so a run of zero items passed.
#[test]
fn a_count_pattern_that_does_not_compile_is_a_configuration_error() {
    let repo = command_repo("command = \"echo 0 passed\"\ncount_pattern = '(\\d+ passed'\n");
    let run = repo.check(&[]);
    assert_count_pattern_rejected(
        &run,
        "gates.command.count_pattern",
        "not a valid regular expression",
    );
}

/// Without a group there is no count to read: the same pass over zero items.
#[test]
fn a_count_pattern_without_a_capture_group_is_a_configuration_error() {
    let repo = command_repo("command = \"echo 0 passed\"\ncount_pattern = '\\d+ passed'\n");
    let run = repo.check(&[]);
    assert_count_pattern_rejected(&run, "gates.command.count_pattern", "capture group");
}

#[test]
fn a_count_pattern_of_a_commands_entry_is_checked_too() {
    let repo = command_repo(
        "[[gates.command.commands]]\nname = \"unit\"\ncommand = \"echo 0 passed\"\ncount_pattern = '\\d+ passed'\n",
    );
    let run = repo.check(&[]);
    assert_count_pattern_rejected(
        &run,
        "gates.command.commands[unit].count_pattern",
        "capture group",
    );
}

/// Control: a pattern with a group still reads the count and reports a run of zero items.
#[test]
fn a_valid_count_pattern_still_reports_zero_items() {
    let repo = command_repo("command = \"echo 0 passed\"\ncount_pattern = '(\\d+) passed'\n");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "command"),
        vec!["command/zero-items-executed".to_string()]
    );
}

/// Control: and passes a run that executed something.
#[test]
fn a_valid_count_pattern_accepts_a_run_with_items() {
    let repo = command_repo("command = \"echo 4 passed\"\ncount_pattern = '(\\d+) passed'\n");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// `exempt_arms` was compiled only once a benchmark artifact changed, so a malformed glob
/// in it passed every other change.
#[test]
fn a_malformed_exempt_arms_glob_stops_a_change_with_no_benchmark_artifact() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.bench-regression]\nenabled = true\nexempt_arms = [\"heap[\"]\n"),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        (
            "configuration".to_string(),
            Some("bench-regression".to_string())
        )
    );
    let d = detail(&run);
    assert!(
        d.contains("invalid glob") && d.contains("gates.bench-regression.exempt_arms"),
        "{d}"
    );
}

/// Control: an entry that compiles, and a plain arm name, do not stop the run.
#[test]
fn a_valid_exempt_arms_list_does_not_stop_the_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.bench-regression]\nenabled = true\nexempt_arms = [\"*.heap.*\", \"map_get/random\"]\n"
        ),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// Pinned (already fixed by the up-front glob check): an invalid `required_paths` glob is
/// not reported as a missing path.
#[test]
fn an_invalid_required_paths_glob_stops_the_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.archive-contents]\nenabled = true\narchive_path = \"a.tar\"\nrequired_paths = [\"src/[a-z/*\"]\n"
        ),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("gate".to_string(), Some("archive-contents".to_string()))
    );
    let d = detail(&run);
    assert!(
        d.contains("invalid glob") && d.contains("gates.archive-contents.required_paths"),
        "{d}"
    );
}

/// Pinned (already fixed): a head-side baseline file that does not parse stops the run.
#[test]
fn a_head_baseline_that_does_not_parse_stops_the_run() {
    let repo = Repo::new();
    repo.write("discipline-baseline.toml", "this is = = not toml [\n");
    repo.commit("chore: baseline");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("baseline".to_string(), None));
}

const MANIFEST: &str =
    "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";

/// A base with a manifest and a `deny.toml` that bans `leftpad`; the change adds `leftpad`
/// and writes `deny` as the policy file.
fn banned_dependency_repo(deny_path: &str, head_deny: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("Cargo.toml", MANIFEST);
    repo.write(deny_path, "[bans]\ndeny = [\"leftpad\"]\n");
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.dependency-delta]\ndeny_file = \"{deny_path}\"\n"),
    );
    repo.commit("chore: manifest and policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("Cargo.toml", &format!("{MANIFEST}leftpad = \"1.0.0\"\n"));
    repo.write(deny_path, head_deny);
    repo.commit("feat: pad");
    repo
}

/// A `deny.toml` that does not parse became an empty policy: the change that breaks it
/// lifts every ban in it.
#[test]
fn a_deny_file_that_does_not_parse_stops_the_run() {
    let repo = banned_dependency_repo("deny.toml", "[bans\ndeny = [\"SENTINEL_VALUE\"\n");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let (_, gate) = run.could_not_check();
    assert_eq!(gate.as_deref(), Some("dependency-delta"), "{}", run.stdout);
    let d = detail(&run);
    assert!(
        d.contains("deny.toml") && d.contains("does not parse"),
        "{d}"
    );
    // Location only: the parser's message can quote the file.
    assert!(!run.stdout.contains("SENTINEL_VALUE"), "{}", run.stdout);
    assert!(!run.stderr.contains("SENTINEL_VALUE"), "{}", run.stderr);
}

/// Control: a `deny.toml` that parses still bans what it names.
#[test]
fn a_deny_file_that_parses_still_bans_a_dependency() {
    let repo = banned_dependency_repo("deny.toml", "[bans]\ndeny = [\"leftpad\"]\n");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(
        codes(&run, "dependency-delta").contains(&"dependency-delta/banned-dependency".to_string()),
        "{}",
        run.stdout
    );
}

/// A directory that cannot be searched made every file under it read as absent, so the
/// policy file in it was an empty policy.
#[cfg(unix)]
#[test]
fn a_file_under_an_unsearchable_directory_is_a_failed_read_not_an_absent_file() {
    use std::os::unix::fs::PermissionsExt;
    let repo = banned_dependency_repo("policy/deny.toml", "[bans]\ndeny = [\"leftpad\"]\n");
    let dir = repo.file("policy");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A process that can search the directory anyway (root) cannot show the defect.
    let enforced = std::fs::metadata(dir.join("deny.toml")).is_err();
    let run = repo.check(&[]);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !enforced {
        return;
    }
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(detail(&run).contains("policy/deny.toml"), "{}", run.stdout);
}

const BASE_WORKFLOW: &str = "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n";

/// A workflow whose head side does not parse runs nothing, its verification jobs included;
/// only a note said so, while the same condition in a GitLab pipeline is a finding.
#[test]
fn a_workflow_whose_head_side_does_not_parse_is_a_finding() {
    let repo = Repo::new();
    repo.commit_base(WF, BASE_WORKFLOW, "ci: add");
    repo.write(WF, "name: ci\non: push\njobs:\n  build: [\n");
    repo.commit("ci: rework");
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/pipeline-file-unreadable".to_string()],
        "{}",
        run.stdout
    );
    let out = run.outcome("ci-integrity");
    // Nothing in the file was examined.
    assert_eq!(out["examined"], 0, "{out}");
    assert!(
        out["notes"]
            .to_string()
            .contains("the head side does not parse as YAML"),
        "{out}"
    );
}

/// Control: the same edit as valid YAML that drops the job is judged for what it removes.
#[test]
fn a_workflow_that_parses_is_still_judged_by_its_jobs() {
    let repo = Repo::new();
    repo.commit_base(WF, BASE_WORKFLOW, "ci: add");
    repo.write(
        WF,
        "name: ci\non: push\njobs:\n  other:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n",
    );
    repo.commit("ci: rework");
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/verification-job-removed".to_string()],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome("ci-integrity")["examined"], 1);
}

/// Control: a new workflow has no base side to be weakened against; it keeps its note.
#[test]
fn a_new_workflow_that_does_not_parse_is_a_note_only() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.sandbox-config]\nenabled = false\n"),
    );
    repo.write(WF, "name: ci\non: push\njobs:\n  build: [\n");
    repo.commit("ci: add");
    let run = repo.check(&[]);
    assert!(codes(&run, "ci-integrity").is_empty(), "{}", run.stdout);
    assert!(
        notes(&run, "ci-integrity").contains("the head side does not parse as YAML"),
        "{}",
        run.stdout
    );
}

/// The GitLab finding carried the YAML parser's message, which can quote the file.
#[test]
fn an_unreadable_gitlab_pipeline_finding_does_not_quote_the_file() {
    let repo = Repo::new();
    repo.commit_base(
        ".gitlab-ci.yml",
        "stages: [test]\nunit:\n  stage: test\n  script:\n    - cargo test\n",
        "ci: add",
    );
    repo.write(
        ".gitlab-ci.yml",
        "stages: [test]\nunit:\n  stage: test\n  script:\n    - cargo test\nSENTINEL_VALUE: 1\nSENTINEL_VALUE: 2\n",
    );
    repo.commit("ci: rework");
    let run = repo.check(&[]);
    let found = run.violations("ci-integrity");
    assert_eq!(
        codes(&run, "ci-integrity"),
        vec!["ci-integrity/pipeline-file-unreadable".to_string()],
        "{}",
        run.stdout
    );
    let message = found[0]["message"].as_str().unwrap();
    assert!(message.contains("head side"), "{message}");
    assert!(!run.stdout.contains("SENTINEL_VALUE"), "{}", run.stdout);
}

/// Removes the loose object holding `path` as committed on `main`.
#[cfg(unix)]
fn lose_base_blob(repo: &Repo, path: &str) {
    use std::os::unix::fs::PermissionsExt;
    let sha = repo.git_output(&["rev-parse", &format!("main:{path}")]);
    let obj = repo.file(&format!(".git/objects/{}/{}", &sha[..2], &sha[2..]));
    assert!(obj.is_file(), "blob {sha} is not loose");
    std::fs::set_permissions(&obj, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(obj).unwrap();
}

/// Pinned (the error was already propagated, with no test): with `diff_only = false` an
/// unchanged workflow is compared with a base side that cannot be read.
#[cfg(unix)]
#[test]
fn ci_integrity_does_not_read_an_unreadable_base_workflow_as_absent() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (WF, BASE_WORKFLOW),
            (
                "discipline.toml",
                &format!("{HEAD}[gates.ci-integrity]\ndiff_only = false\n"),
            ),
        ],
        "ci: add",
    );
    // Not committed: `git add` may write the lost object back for an unchanged file.
    lose_base_blob(&repo, WF);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let (_, gate) = run.could_not_check();
    assert_eq!(gate.as_deref(), Some("ci-integrity"), "{}", run.stdout);
    assert!(detail(&run).contains(WF), "{}", run.stdout);
}

/// Pinned: a changed shell script whose head side cannot be read stops the run. The diff
/// reads the file before `shell-secrets` does, so the gate named is whichever asks first.
#[cfg(unix)]
#[test]
fn an_unreadable_changed_script_stops_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.shell-secrets]\nenabled = true\n"),
    );
    repo.write("run.sh", "#!/bin/sh\necho hi\n");
    repo.commit("chore: script");
    repo.write("run.sh", "#!/bin/sh\necho hello\n");
    let script = repo.file("run.sh");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o000)).unwrap();
    let enforced = std::fs::read(&script).is_err();
    let run = repo.check(&[]);
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
    if !enforced {
        return;
    }
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(run.json()["could_not_check"].is_object(), "{}", run.stdout);
}

/// A base with a floor constant of 1 in `floor.rs`; the change rewrites the file as `head`.
fn floor_constant_repo(head: Option<&str>) -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("floor.rs", "pub const MIN_TESTS: usize = 1;\n"),
            (
                "discipline.toml",
                &format!(
                    "{HEAD}[gates.test-floor]\nenabled = true\nconstant_file = \"floor.rs\"\nconstant_name = \"MIN_TESTS\"\n"
                ),
            ),
        ],
        "chore: floor",
    );
    match head {
        Some(content) => repo.write("floor.rs", content),
        None => repo.remove("floor.rs"),
    }
    repo.commit("chore: floor file");
    repo
}

/// Lowering the floor constant is a finding; removing it, which lifts the floor altogether,
/// passed without a word because a constant that could not be read was skipped.
#[test]
fn a_floor_constant_removed_at_head_is_a_finding() {
    let repo = floor_constant_repo(Some("// the floor is gone\n"));
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "test-floor"),
        vec!["test-floor/floor-constant-decreased".to_string()]
    );
    let message = run.violations("test-floor")[0]["message"].to_string();
    assert!(message.contains("no longer defined"), "{message}");
}

#[test]
fn a_floor_constant_file_removed_at_head_is_a_finding() {
    let repo = floor_constant_repo(None);
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        codes(&run, "test-floor"),
        vec!["test-floor/floor-constant-decreased".to_string()]
    );
}

/// The removal is lifted by the directive that lifts a lowered constant, naming it.
#[test]
fn a_removed_floor_constant_is_lifted_by_allow_test_shrink() {
    let repo = floor_constant_repo(Some("// the floor is gone\n"));
    let run = repo.check_with_pr(
        &[],
        "allow-test-shrink: MIN_TESTS retired with the suite it counted\n",
    );
    assert!(codes(&run, "test-floor").is_empty(), "{}", run.stdout);
}

/// Control: a constant that is kept or raised is not a finding.
#[test]
fn a_floor_constant_kept_or_raised_is_not_a_finding() {
    for head in [
        "pub const MIN_TESTS: usize = 1;\n",
        "// moved down the file\n\npub const MIN_TESTS: usize = 2;\n",
    ] {
        let repo = floor_constant_repo(Some(head));
        let run = repo.check(&[]);
        assert!(codes(&run, "test-floor").is_empty(), "{}", run.stdout);
    }
}
