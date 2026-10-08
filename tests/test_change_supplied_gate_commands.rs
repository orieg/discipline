//! A change under review cannot make `msrv`, `miri` or `sanitizers` execute text the
//! change supplies.
//!
//! Each case commits a configuration on `main`, changes it on `work`, and runs the real
//! binary against `main`. `msrv` runs a whole configured command, which here creates a
//! marker file. `miri` and `sanitizers` run `cargo` with configured values among its
//! arguments: a stand-in `cargo`, first on `PATH`, writes each argument it receives on a
//! line of its own, so a case sees whether anything ran and with which arguments. No
//! real toolchain is started.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const MARKER: &str = "RAN_BY_CHANGE";
const BASE_MARKER: &str = "RAN_BASE_COMMAND";
const ARGV: &str = "CARGO_ARGV";
const MSRV_FINDING: &str = "msrv/untrusted-command-modification";
const MIRI_FINDING: &str = "miri/untrusted-command-modification";
const SANITIZERS_FINDING: &str = "sanitizers/untrusted-command-modification";
const ALLOW: (&str, &str) = ("DISCIPLINE_ALLOW_COMMAND_CHANGE", "1");
const RUNNER_COMMAND: (&str, &str) = ("DISCIPLINE_COMMAND", "true");
const CI: (&str, &str) = ("CI", "true");
const BASE_POLICY: &[&str] = &["--policy-from", "base"];
const ENABLING_NOTE: &str = "this change enables it";

const NO_GATES: &str = "";

const MSRV_ON: &str = "[gates.msrv]\nenabled = true\npinned_version = \"1.90\"\n";
const MSRV_OFF: &str = "[gates.msrv]\nenabled = false\npinned_version = \"1.90\"\n";
const MSRV_MARKING: &str = "command = \"sh -c 'touch RAN_BY_CHANGE; exit 0'\"\n";
const MSRV_BASE_COMMAND: &str = "command = \"sh -c 'touch RAN_BASE_COMMAND; exit 0'\"\n";

const MIRI_ON: &str = "[gates.miri]\nenabled = true\n";
const MIRI_OFF: &str = "[gates.miri]\nenabled = false\n";
const MIRI_LIB: &str = "args = [\"--lib\"]\n";
const MIRI_TESTS: &str = "args = [\"--tests\"]\n";
const MIRI_TWO_ARGS: &str = "args = [\"--package\", \"core_crate\"]\n";
const MIRI_ARG_WITH_SPACE: &str = "args = [\"--lib --config build.rustc-wrapper=wrapper\"]\n";
const MIRI_ARG_WITH_METACHARACTERS: &str = "args = [\"--lib;touch${IFS}RAN_BY_CHANGE\"]\n";
const MIRI_ARG_EMPTY: &str = "args = [\"\"]\n";
const MIRI_ARGS_KEY: &str = "gates.miri.args";

const SANITIZERS_ON: &str = "[gates.sanitizers]\nenabled = true\n";
const SANITIZERS_OFF: &str = "[gates.sanitizers]\nenabled = false\n";
const SANITIZER_THREAD: &str = "sanitizer = \"thread\"\n";
const SANITIZER_MEMORY: &str = "sanitizer = \"memory\"\n";
const SANITIZER_WITH_SPACE: &str = "sanitizer = \"address --config build.rustc-wrapper=wrapper\"\n";
const SANITIZER_WITH_LEADING_DASH: &str = "sanitizer = \"-Zunstable-options\"\n";
const SANITIZER_WITH_METACHARACTERS: &str = "sanitizer = \"address;touch${IFS}RAN_BY_CHANGE\"\n";
const SANITIZER_KEY: &str = "gates.sanitizers.sanitizer";
const CANARY_ON: &str = "canary = true\n";

/// Stands in for `cargo`: one argument per line, a `--` line after each invocation, the
/// diagnostic the canary is expected to print, and a test summary that is not empty.
const FAKE_CARGO: &str = "#!/bin/sh\n\
for a in \"$@\"; do printf '%s\\n' \"$a\" >> CARGO_ARGV; done\n\
printf -- '--\\n' >> CARGO_ARGV\n\
case \" $* \" in *race_canary*) echo 'ThreadSanitizer: data race'; exit 1;; esac\n\
echo 'test result: ok. 1 passed'\n";

const MIRI_BUILT_IN: &[&str] = &["miri", "test", "--"];
const MIRI_WITH_LIB: &[&str] = &["miri", "test", "--lib", "--"];
const MIRI_WITH_TWO_ARGS: &[&str] = &["miri", "test", "--package", "core_crate", "--"];
const SANITIZERS_BUILT_IN: &[&str] = &["test", "-Zsanitizer=address", "--"];
const SANITIZERS_THREAD: &[&str] = &["test", "-Zsanitizer=thread", "--"];
const SANITIZERS_THREAD_WITH_CANARY: &[&str] = &[
    "test",
    "--test",
    "race_canary",
    "--",
    "test",
    "-Zsanitizer=thread",
    "--",
];

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

/// No configuration on `main`; `head` committed on `work`.
fn repo_adding_config(head: &str) -> Repo {
    let repo = Repo::new();
    repo.write("discipline.toml", &config(head));
    repo.commit("chore: add a configuration");
    repo
}

/// The same configuration on both sides; the change touches a document.
fn repo_unchanged(gates: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base("discipline.toml", &config(gates), "ci: base configuration");
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1, Phase 2, then Phase 3.\n",
    );
    repo.commit("docs: extend the plan");
    repo
}

/// A repository with no commit: `--staged` then compares with the empty tree.
fn unborn_repo(gates: &str) -> Repo {
    let repo = Repo {
        dir: tempfile::tempdir().unwrap(),
    };
    repo.git(&["init", "-q", "-b", "main"]);
    repo.write("discipline.toml", &config(gates));
    repo.write("src/lib.rs", common::GOOD_LIB);
    repo.git(&["add", "-A"]);
    repo
}

/// A directory holding the stand-in `cargo`, and the `PATH` that puts it first.
struct FakeCargo {
    _dir: tempfile::TempDir,
    path: String,
}

fn fake_cargo() -> FakeCargo {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("cargo");
    std::fs::write(&tool, FAKE_CARGO).unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", dir.path().display());
    FakeCargo { _dir: dir, path }
}

fn check(repo: &Repo, extra: &[&str], env: &[(&str, &str)]) -> Run {
    let mut args = vec!["check", "--format", "json", "--base", "main"];
    args.extend(extra);
    repo.run(&args, env)
}

/// `check` with the stand-in `cargo` first on `PATH`.
fn check_with_cargo(repo: &Repo, extra: &[&str], env: &[(&str, &str)]) -> Run {
    let tool = fake_cargo();
    let mut env = env.to_vec();
    env.push(("PATH", &tool.path));
    check(repo, extra, &env)
}

fn staged(repo: &Repo, env: &[(&str, &str)]) -> Run {
    repo.run(&["check", "--format", "json", "--staged"], env)
}

fn staged_with_cargo(repo: &Repo, env: &[(&str, &str)]) -> Run {
    let tool = fake_cargo();
    let mut env = env.to_vec();
    env.push(("PATH", &tool.path));
    staged(repo, &env)
}

fn codes(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

fn notes(run: &Run, gate: &str) -> String {
    run.outcome(gate)["notes"].to_string()
}

fn all(run: &Run) -> String {
    format!("exit {}\n{}{}", run.code, run.stdout, run.stderr)
}

/// Every argument the stand-in `cargo` received, in order; empty when it never ran.
fn argv(repo: &Repo) -> Vec<String> {
    std::fs::read_to_string(repo.file(ARGV))
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// What a refused run looks like: `(codes, examined, exit code)`.
fn refusal(run: &Run, gate: &str) -> (Vec<String>, u64, i32) {
    (
        codes(run, gate),
        run.outcome(gate)["examined"].as_u64().unwrap(),
        run.code,
    )
}

fn refused(finding: &str) -> (Vec<String>, u64, i32) {
    (vec![finding.to_string()], 0, 1)
}

/// The run stopped before any gate on the shape of `key` in `gate`'s table.
fn stopped_on(run: &Run, gate: &str, key: &str) -> bool {
    let detail = run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    run.code == 2
        && run.could_not_check() == ("configuration".to_string(), Some(gate.to_string()))
        && detail.contains(key)
}

// ---- msrv: `command` -------------------------------------------------------------

#[test]
fn an_msrv_command_added_by_the_change_is_not_run() {
    let repo = repo_with(MSRV_ON, &format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = check(&repo, &[], &[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        refusal(&run, "msrv"),
        refused(MSRV_FINDING),
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the command the change supplied was executed");
}

#[test]
fn an_msrv_command_altered_by_the_change_is_not_run() {
    let repo = repo_with(
        &format!("{MSRV_ON}{MSRV_BASE_COMMAND}"),
        &format!("{MSRV_ON}{MSRV_MARKING}"),
    );
    let run = check(&repo, &[], &[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        refusal(&run, "msrv"),
        refused(MSRV_FINDING),
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the command the change supplied was executed");
    assert!(
        !repo.file(BASE_MARKER).exists(),
        "a refused gate ran the base's command"
    );
}

#[test]
fn an_msrv_gate_the_change_enables_with_a_command_is_not_run() {
    let repo = repo_with(NO_GATES, &format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = check(&repo, &[], &[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        refusal(&run, "msrv"),
        refused(MSRV_FINDING),
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the command the change supplied was executed");
}

/// The base had the gate off with its own command; the change swaps the command and
/// turns the gate on.
#[test]
fn an_msrv_gate_the_change_enables_with_another_command_is_not_run() {
    let repo = repo_with(
        &format!("{MSRV_OFF}{MSRV_BASE_COMMAND}"),
        &format!("{MSRV_ON}{MSRV_MARKING}"),
    );
    let run = check(&repo, &[], &[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        refusal(&run, "msrv"),
        refused(MSRV_FINDING),
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the command the change supplied was executed");
}

/// No base configuration at all: a `command` on head is supplied by the change.
#[test]
fn an_msrv_command_in_a_configuration_the_change_adds_is_not_run() {
    let repo = repo_adding_config(&format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = check(&repo, &[], &[]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        refusal(&run, "msrv"),
        refused(MSRV_FINDING),
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the command the change supplied was executed");
}

/// `DISCIPLINE_COMMAND` supplies the `command` gate's command and authorises nothing.
#[test]
fn discipline_command_does_not_authorise_an_msrv_command_the_change_adds() {
    let repo = repo_with(MSRV_ON, &format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = check(&repo, &[], &[RUNNER_COMMAND]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        refusal(&run, "msrv"),
        refused(MSRV_FINDING),
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the command the change supplied was executed");
}

/// In CI there is no base to vouch for the command, so nothing the change wrote runs.
#[test]
fn a_ci_run_with_no_base_runs_no_msrv_command() {
    let repo = unborn_repo(&format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = staged(&repo, &[CI]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        codes(&run, "msrv"),
        [MSRV_FINDING],
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the configured command ran with no base");
}

/// Control: a command both sides declare still runs.
#[test]
fn an_unchanged_msrv_command_still_runs() {
    let repo = repo_unchanged(&format!("{MSRV_ON}{MSRV_BASE_COMMAND}"));
    let run = check(&repo, &[], &[]);
    assert!(repo.file(BASE_MARKER).exists(), "{}", all(&run));
    assert_eq!(codes(&run, "msrv"), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
    assert!(!notes(&run, "msrv").contains(ENABLING_NOTE));
}

/// Control: the runner's environment authorises a changed command.
#[test]
fn the_runner_environment_authorises_a_changed_msrv_command() {
    let repo = repo_with(MSRV_ON, &format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = check(&repo, &[], &[ALLOW]);
    assert!(repo.file(MARKER).exists(), "{}", all(&run));
    assert_eq!(codes(&run, "msrv"), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
}

/// Control: under `--policy-from base` the base copy is in force, so the head's command
/// is not what runs and there is no modification to report.
#[test]
fn under_base_policy_the_base_msrv_command_runs_and_nothing_is_reported() {
    let repo = repo_with(
        &format!("{MSRV_ON}{MSRV_BASE_COMMAND}"),
        &format!("{MSRV_ON}{MSRV_MARKING}"),
    );
    let run = check(&repo, BASE_POLICY, &[]);
    assert!(
        !repo.file(MARKER).exists(),
        "the head's command ran under the base policy"
    );
    assert!(repo.file(BASE_MARKER).exists(), "{}", all(&run));
    assert_eq!(codes(&run, "msrv"), Vec::<String>::new(), "{}", all(&run));
}

/// Control: the command is the base's own text; the change decided that it runs, and
/// the run says so.
#[test]
fn an_msrv_gate_the_change_enables_runs_the_base_command_with_a_note() {
    let repo = repo_with(
        &format!("{MSRV_OFF}{MSRV_BASE_COMMAND}"),
        &format!("{MSRV_ON}{MSRV_BASE_COMMAND}"),
    );
    let run = check(&repo, &[], &[]);
    assert!(repo.file(BASE_MARKER).exists(), "{}", all(&run));
    assert_eq!(codes(&run, "msrv"), Vec::<String>::new(), "{}", all(&run));
    let said = notes(&run, "msrv");
    assert!(
        said.contains("disabled on the base side") && said.contains(ENABLING_NOTE),
        "{said}"
    );
}

/// Control: on a developer's machine the first commit's own configuration runs.
#[test]
fn a_local_run_with_no_base_runs_the_msrv_command() {
    let repo = unborn_repo(&format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = staged(&repo, &[]);
    assert_eq!(codes(&run, "msrv"), Vec::<String>::new(), "{}", all(&run));
    assert!(repo.file(MARKER).exists(), "{}", all(&run));
}

/// Control: the runner can still vouch for a command in a CI run with no base.
#[test]
fn a_ci_run_with_no_base_runs_an_msrv_command_the_runner_authorises() {
    let repo = unborn_repo(&format!("{MSRV_ON}{MSRV_MARKING}"));
    let run = staged(&repo, &[CI, ALLOW]);
    assert_eq!(codes(&run, "msrv"), Vec::<String>::new(), "{}", all(&run));
    assert!(repo.file(MARKER).exists(), "{}", all(&run));
}

// ---- miri: `args` ----------------------------------------------------------------

#[test]
fn miri_args_added_by_the_change_are_not_run() {
    let repo = repo_with(MIRI_ON, &format!("{MIRI_ON}{MIRI_LIB}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "miri"),
        refused(MIRI_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's arguments"
    );
}

#[test]
fn miri_args_altered_by_the_change_are_not_run() {
    let repo = repo_with(
        &format!("{MIRI_ON}{MIRI_LIB}"),
        &format!("{MIRI_ON}{MIRI_TESTS}"),
    );
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "miri"),
        refused(MIRI_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's arguments"
    );
}

#[test]
fn a_miri_gate_the_change_enables_with_args_is_not_run() {
    let repo = repo_with(NO_GATES, &format!("{MIRI_ON}{MIRI_LIB}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "miri"),
        refused(MIRI_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's arguments"
    );
}

#[test]
fn miri_args_in_a_configuration_the_change_adds_are_not_run() {
    let repo = repo_adding_config(&format!("{MIRI_ON}{MIRI_LIB}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "miri"),
        refused(MIRI_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's arguments"
    );
}

#[test]
fn discipline_command_does_not_authorise_miri_args_the_change_adds() {
    let repo = repo_with(MIRI_ON, &format!("{MIRI_ON}{MIRI_LIB}"));
    let run = check_with_cargo(&repo, &[], &[RUNNER_COMMAND]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "miri"),
        refused(MIRI_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's arguments"
    );
}

#[test]
fn a_ci_run_with_no_base_runs_no_miri_args() {
    let repo = unborn_repo(&format!("{MIRI_ON}{MIRI_LIB}"));
    let run = staged_with_cargo(&repo, &[CI]);
    let ran = argv(&repo);
    assert_eq!(
        codes(&run, "miri"),
        [MIRI_FINDING],
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with no base");
}

/// An argument both sides declare is trusted, and still must be one argument: text with
/// a space in it would otherwise reach `cargo` as several, here as `--config <value>`.
#[test]
fn a_miri_arg_with_a_space_is_a_configuration_error() {
    let repo = repo_unchanged(&format!("{MIRI_ON}{MIRI_ARG_WITH_SPACE}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "miri", MIRI_ARGS_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with a re-split argument"
    );
}

#[test]
fn a_miri_arg_with_shell_metacharacters_is_a_configuration_error() {
    let repo = repo_unchanged(&format!("{MIRI_ON}{MIRI_ARG_WITH_METACHARACTERS}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "miri", MIRI_ARGS_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with the argument");
}

#[test]
fn an_empty_miri_arg_is_a_configuration_error() {
    let repo = repo_unchanged(&format!("{MIRI_ON}{MIRI_ARG_EMPTY}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "miri", MIRI_ARGS_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with the argument");
}

/// The shape is checked before any gate runs, so a value of another shape that the
/// change itself writes stops the run as a configuration error, not as a refusal.
#[test]
fn a_miri_arg_of_another_shape_written_by_the_change_stops_the_run_up_front() {
    let repo = repo_with(MIRI_ON, &format!("{MIRI_ON}{MIRI_ARG_WITH_SPACE}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "miri", MIRI_ARGS_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with the argument");
}

/// Control: arguments both sides declare still run, each as one argument. A leading `-`
/// is what an argument of `cargo miri test` looks like, so it is accepted here.
#[test]
fn unchanged_miri_args_still_run_one_argument_each() {
    let repo = repo_unchanged(&format!("{MIRI_ON}{MIRI_LIB}"));
    let run = check_with_cargo(&repo, &[], &[]);
    assert_eq!(argv(&repo), MIRI_WITH_LIB, "{}", all(&run));
    assert_eq!(codes(&run, "miri"), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
}

/// Control: two list items are two arguments.
#[test]
fn two_unchanged_miri_args_are_two_arguments() {
    let repo = repo_unchanged(&format!("{MIRI_ON}{MIRI_TWO_ARGS}"));
    let run = check_with_cargo(&repo, &[], &[]);
    assert_eq!(argv(&repo), MIRI_WITH_TWO_ARGS, "{}", all(&run));
}

#[test]
fn the_runner_environment_authorises_changed_miri_args() {
    let repo = repo_with(MIRI_ON, &format!("{MIRI_ON}{MIRI_LIB}"));
    let run = check_with_cargo(&repo, &[], &[ALLOW]);
    assert_eq!(argv(&repo), MIRI_WITH_LIB, "{}", all(&run));
    assert_eq!(codes(&run, "miri"), Vec::<String>::new(), "{}", all(&run));
}

#[test]
fn under_base_policy_the_base_miri_args_run_and_nothing_is_reported() {
    let repo = repo_with(
        &format!("{MIRI_ON}{MIRI_LIB}"),
        &format!("{MIRI_ON}{MIRI_TESTS}"),
    );
    let run = check_with_cargo(&repo, BASE_POLICY, &[]);
    assert_eq!(argv(&repo), MIRI_WITH_LIB, "{}", all(&run));
    assert_eq!(codes(&run, "miri"), Vec::<String>::new(), "{}", all(&run));
}

/// Control: enabling the gate with nothing configured runs the built-in command, which
/// is not the change's text, and the run says the change turned it on.
#[test]
fn a_miri_gate_the_change_enables_without_args_runs_the_built_in_command_with_a_note() {
    let repo = repo_with(MIRI_OFF, MIRI_ON);
    let run = check_with_cargo(&repo, &[], &[]);
    assert_eq!(argv(&repo), MIRI_BUILT_IN, "{}", all(&run));
    assert_eq!(codes(&run, "miri"), Vec::<String>::new(), "{}", all(&run));
    let said = notes(&run, "miri");
    assert!(
        said.contains("disabled on the base side") && said.contains(ENABLING_NOTE),
        "{said}"
    );
}

#[test]
fn a_local_run_with_no_base_runs_the_miri_args() {
    let repo = unborn_repo(&format!("{MIRI_ON}{MIRI_LIB}"));
    let run = staged_with_cargo(&repo, &[]);
    assert_eq!(argv(&repo), MIRI_WITH_LIB, "{}", all(&run));
    assert_eq!(codes(&run, "miri"), Vec::<String>::new(), "{}", all(&run));
}

// ---- sanitizers: `sanitizer` and `canary` -----------------------------------------

#[test]
fn a_sanitizer_name_added_by_the_change_is_not_run() {
    let repo = repo_with(SANITIZERS_ON, &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's value"
    );
}

#[test]
fn a_sanitizer_name_altered_by_the_change_is_not_run() {
    let repo = repo_with(
        &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"),
        &format!("{SANITIZERS_ON}{SANITIZER_MEMORY}"),
    );
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's value"
    );
}

#[test]
fn a_sanitizers_gate_the_change_enables_with_a_name_is_not_run() {
    let repo = repo_with(NO_GATES, &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's value"
    );
}

#[test]
fn a_sanitizer_name_in_a_configuration_the_change_adds_is_not_run() {
    let repo = repo_adding_config(&format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's value"
    );
}

/// `canary` selects a second built-in command; a change that turns it on adds a run.
#[test]
fn a_sanitizer_canary_added_by_the_change_is_not_run() {
    // The canary belongs to `thread`, which both sides name.
    let repo = repo_with(
        &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"),
        &format!("{SANITIZERS_ON}{SANITIZER_THREAD}{CANARY_ON}"),
    );
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "the canary or the main command ran"
    );
}

/// The other direction changes what runs as well: the canary the base declares is gone.
#[test]
fn a_sanitizer_canary_removed_by_the_change_is_not_run() {
    let repo = repo_with(&format!("{SANITIZERS_ON}{CANARY_ON}"), SANITIZERS_ON);
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "the main command ran without its canary"
    );
}

#[test]
fn a_sanitizers_gate_the_change_enables_with_a_canary_is_not_run() {
    // The canary belongs to `thread`; the change supplies both keys.
    let repo = repo_with(
        NO_GATES,
        &format!("{SANITIZERS_ON}{SANITIZER_THREAD}{CANARY_ON}"),
    );
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "the canary or the main command ran"
    );
}

#[test]
fn discipline_command_does_not_authorise_a_sanitizer_name_the_change_adds() {
    let repo = repo_with(SANITIZERS_ON, &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = check_with_cargo(&repo, &[], &[RUNNER_COMMAND]);
    let ran = argv(&repo);
    assert_eq!(
        refusal(&run, "sanitizers"),
        refused(SANITIZERS_FINDING),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with the change's value"
    );
}

#[test]
fn a_ci_run_with_no_base_runs_no_sanitizer_name() {
    let repo = unborn_repo(&format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = staged_with_cargo(&repo, &[CI]);
    let ran = argv(&repo);
    assert_eq!(
        codes(&run, "sanitizers"),
        [SANITIZERS_FINDING],
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with no base");
}

/// A name both sides declare is trusted, and still must be one name: with a space in it
/// the text after the space would reach `cargo` as arguments of its own.
#[test]
fn a_sanitizer_name_with_a_space_is_a_configuration_error() {
    let repo = repo_unchanged(&format!("{SANITIZERS_ON}{SANITIZER_WITH_SPACE}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "sanitizers", SANITIZER_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with injected arguments"
    );
}

#[test]
fn a_sanitizer_name_with_a_leading_dash_is_a_configuration_error() {
    let repo = repo_unchanged(&format!("{SANITIZERS_ON}{SANITIZER_WITH_LEADING_DASH}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "sanitizers", SANITIZER_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with the value");
}

#[test]
fn a_sanitizer_name_with_shell_metacharacters_is_a_configuration_error() {
    let repo = repo_unchanged(&format!("{SANITIZERS_ON}{SANITIZER_WITH_METACHARACTERS}"));
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "sanitizers", SANITIZER_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(ran, Vec::<String>::new(), "cargo ran with the value");
}

/// The shape is checked before any gate runs, so a name of another shape that the
/// change itself writes stops the run as a configuration error, not as a refusal.
#[test]
fn a_sanitizer_name_of_another_shape_written_by_the_change_stops_the_run_up_front() {
    let repo = repo_with(
        SANITIZERS_ON,
        &format!("{SANITIZERS_ON}{SANITIZER_WITH_SPACE}"),
    );
    let run = check_with_cargo(&repo, &[], &[]);
    let ran = argv(&repo);
    assert!(
        stopped_on(&run, "sanitizers", SANITIZER_KEY),
        "cargo received {ran:?}; {}",
        all(&run)
    );
    assert_eq!(
        ran,
        Vec::<String>::new(),
        "cargo ran with injected arguments"
    );
}

/// Control: a name and a canary both sides declare still run, the canary first.
#[test]
fn an_unchanged_sanitizer_name_and_canary_still_run() {
    let repo = repo_unchanged(&format!("{SANITIZERS_ON}{SANITIZER_THREAD}{CANARY_ON}"));
    let run = check_with_cargo(&repo, &[], &[]);
    assert_eq!(argv(&repo), SANITIZERS_THREAD_WITH_CANARY, "{}", all(&run));
    assert_eq!(
        codes(&run, "sanitizers"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert_eq!(run.code, 0, "{}", all(&run));
}

#[test]
fn the_runner_environment_authorises_a_changed_sanitizer_name() {
    let repo = repo_with(SANITIZERS_ON, &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = check_with_cargo(&repo, &[], &[ALLOW]);
    assert_eq!(argv(&repo), SANITIZERS_THREAD, "{}", all(&run));
    assert_eq!(
        codes(&run, "sanitizers"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
}

#[test]
fn under_base_policy_the_base_sanitizer_name_runs_and_nothing_is_reported() {
    let repo = repo_with(
        &format!("{SANITIZERS_ON}{SANITIZER_THREAD}"),
        &format!("{SANITIZERS_ON}{SANITIZER_MEMORY}"),
    );
    let run = check_with_cargo(&repo, BASE_POLICY, &[]);
    assert_eq!(argv(&repo), SANITIZERS_THREAD, "{}", all(&run));
    assert_eq!(
        codes(&run, "sanitizers"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
}

/// Control: enabling the gate with nothing configured runs the built-in command, and
/// the run says the change turned it on.
#[test]
fn a_sanitizers_gate_the_change_enables_with_defaults_runs_the_built_in_command_with_a_note() {
    let repo = repo_with(SANITIZERS_OFF, SANITIZERS_ON);
    let run = check_with_cargo(&repo, &[], &[]);
    assert_eq!(argv(&repo), SANITIZERS_BUILT_IN, "{}", all(&run));
    assert_eq!(
        codes(&run, "sanitizers"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    let said = notes(&run, "sanitizers");
    assert!(
        said.contains("disabled on the base side") && said.contains(ENABLING_NOTE),
        "{said}"
    );
}

#[test]
fn a_local_run_with_no_base_runs_the_sanitizer_name() {
    let repo = unborn_repo(&format!("{SANITIZERS_ON}{SANITIZER_THREAD}"));
    let run = staged_with_cargo(&repo, &[]);
    assert_eq!(argv(&repo), SANITIZERS_THREAD, "{}", all(&run));
    assert_eq!(
        codes(&run, "sanitizers"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
}
