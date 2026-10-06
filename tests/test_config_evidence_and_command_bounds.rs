//! Evidence keys judged by their effective value, and the bounds of what the `command`
//! gate and `test-floor`'s `test_command` execute and read (#592).
//!
//! Each case builds a throwaway repository, commits a configuration on `main`, changes it
//! (or something else) on `work`, and runs the real binary against `main`. A command that
//! must not run creates a marker file, so a case fails when it ran, whatever the report
//! says. No preset's own tool is started: every command that runs is `sh`, `echo`, `true`
//! or `printf`.

mod common;
use common::{Repo, Run};

const WEAKENED: &str = "Gate Weakened By This Change";
const MARKER: &str = "RAN_BY_CHANGE";
const COMMAND_FINDING: &str = "command/untrusted-command-modification";
const TEST_COMMAND_FINDING: &str = "test-floor/untrusted-test-command";
const ALLOW: (&str, &str) = ("DISCIPLINE_ALLOW_COMMAND_CHANGE", "1");

/// The built-in `issue-link` pattern, as a TOML literal string.
const BUILTIN_ISSUE_PATTERN: &str = r"(?i)(?:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)?#\d+\b|https?://[^\s/]+/[^\s/]+/[^\s/]+/(?:issues|pull)/\d+\b";
const ISSUE_LINK: &str = "[gates.issue-link]\nenabled = true\n";
const ISSUE_LINK_ANYTHING: &str = "[gates.issue-link]\nenabled = true\npattern = \".\"\n";
const NO_REFERENCE_TITLE: &str = "chore: tidy the plan";
const NO_REFERENCE_BODY: &str = "Reword one sentence.";

/// `cargo-mutants` named and nothing else: the preset's own command is the one in force.
const MUTANTS_PRESET_ONLY: &str = "[gates.command]\npreset = \"cargo-mutants\"\n";
const MUTANTS_OWN_COMMAND: &str =
    "[gates.command]\npreset = \"cargo-mutants\"\ncommand = \"echo 5 mutants tested\"\n";
/// `cargo-mutants` with a base-side command that prints the preset's "nothing ran" line.
const MUTANTS_NOTHING_RAN: &str =
    "[gates.command]\npreset = \"cargo-mutants\"\ncommand = \"echo 0 mutants tested\"\n";
const ZERO_GUARD_WRITTEN_DOWN: &str = "zero_items_pattern = \"0 mutants tested\"\n";
const ZERO_GUARD_REPLACED: &str = "zero_items_pattern = \"never printed\"\n";
const MUTANTS_ENTRY: &str = "[gates.command]\n\n[[gates.command.commands]]\nname = \"mut\"\n\
     preset = \"cargo-mutants\"\ncommand = \"echo 5 mutants tested\"\n";
/// A command with no preset, printing the line the `cargo-mutants` preset guards against.
const NO_PRESET_NOTHING_RAN: &str = "[gates.command]\ncommand = \"echo 0 mutants tested\"\n";

/// Prints one byte more than the capture limit holds, then the line a check looks for.
const FLOOD_THEN_FORBIDDEN: &str = "head -c 26214401 /dev/zero\necho\necho FORBIDDEN_LINE\n";
const FLOOD_THEN_NOTHING_RAN: &str = "head -c 26214401 /dev/zero\necho\necho running 0 tests\n";
const FLOOD_THEN_ZERO_COUNT: &str = "head -c 26214401 /dev/zero\necho\necho 0 passed\n";
const FLOOD_ONLY: &str = "head -c 26214401 /dev/zero\necho\n";
const SHORT_THEN_FORBIDDEN: &str = "echo short\necho FORBIDDEN_LINE\n";
const FORBID_TABLE: &str =
    "[gates.command]\ncommand = \"sh out.sh\"\nforbid_output = [\"FORBIDDEN_LINE\"]\n";
const ZERO_TABLE: &str =
    "[gates.command]\ncommand = \"sh out.sh\"\nzero_items_pattern = \"running 0 tests\"\n";
const COUNT_TABLE: &str =
    "[gates.command]\ncommand = \"sh out.sh\"\ncount_pattern = '(\\d+) passed'\n";
const STATUS_ONLY_TABLE: &str = "[gates.command]\ncommand = \"sh out.sh\"\n";
/// A flood, then a JUnit report with one failed case, and exit status 0.
const FLOOD_THEN_FAILED_CASE: &str = "head -c 26214401 /dev/zero\necho\n\
     echo '<testsuite><testcase name=\"adds\" classname=\"a\"><failure message=\"boom\"/></testcase></testsuite>'\n";
const BASE_TESTS_TABLE: &str =
    "[gates.command]\npreset = \"base-tests\"\ncommand = \"sh out.sh\"\n";

const API_ONE_LINE: &str = "pub fn a()\n";
const SNAPSHOT_TABLE_ONLY_ENTRIES: &str = "[gates.command]\nsnapshot = \"api.txt\"\n\n\
     [[gates.command.commands]]\nname = \"api\"\ncommand = \"echo pub fn other()\"\n";
const SNAPSHOT_ON_THE_ENTRY: &str = "[gates.command]\n\n\
     [[gates.command.commands]]\nname = \"api\"\ncommand = \"echo pub fn other()\"\n\
     snapshot = \"api.txt\"\n";
const SNAPSHOT_TABLE: &str =
    "[gates.command]\ncommand = \"sh render.sh\"\nsnapshot = \"api.txt\"\n";
/// Prints a surface the snapshot does not hold, after writing it into the snapshot.
const RENDER_REWRITES_SNAPSHOT: &str =
    "printf 'pub fn changed()\\n' > api.txt\nprintf 'pub fn changed()\\n'\n";
const RENDER_API_ONE_LINE: &str = "printf 'pub fn a()\\n'\n";

const FLOOD_THEN_COUNT: &str = "head -c 26214401 /dev/zero\necho\necho 99999\n";
const TEST_FLOOR_LISTING: &str =
    "[gates.test-floor]\nmin_tests = 1\ntest_command = \"sh list.sh\"\n";
const SHORT_LISTING: &str = "echo 7\n";

const DISABLED_WITH_COMMAND: &str =
    "[gates.command]\nenabled = false\ncommand = \"sh -c 'touch RAN_BASE_COMMAND'\"\n";
const ENABLED_WITH_COMMAND: &str =
    "[gates.command]\nenabled = true\ncommand = \"sh -c 'touch RAN_BASE_COMMAND'\"\n";

const GRADLE_BUILD: &str = "// gradle\n";
const NO_RUNNER_DETECTED: &str = "# No test runner was detected here.";

fn config(gates: &str) -> String {
    format!("[meta]\nversion = 1\nname = \"t\"\n\n{gates}")
}

/// `base` committed on `main` with `base_files`, then `head` and `head_files` on `work`.
fn repo_with_files(
    base: &str,
    base_files: &[(&str, &str)],
    head: &str,
    head_files: &[(&str, &str)],
) -> Repo {
    let repo = Repo::new();
    let cfg = config(base);
    let mut files = vec![("discipline.toml", cfg.as_str())];
    files.extend_from_slice(base_files);
    repo.commit_base_files(&files, "ci: base configuration");
    repo.write("discipline.toml", &config(head));
    for (rel, content) in head_files {
        repo.write(rel, content);
    }
    repo.commit("chore: change the configuration");
    repo
}

fn repo_with(base: &str, head: &str) -> Repo {
    repo_with_files(base, &[], head, &[])
}

/// The same configuration on both sides; the change touches a document.
fn repo_unchanged_policy(gates: &str, files: &[(&str, &str)]) -> Repo {
    let repo = Repo::new();
    let cfg = config(gates);
    let mut all = vec![("discipline.toml", cfg.as_str())];
    all.extend_from_slice(files);
    repo.commit_base_files(&all, "ci: base configuration");
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1, Phase 2, then Phase 3.\n",
    );
    repo.commit("docs: extend the plan");
    repo
}

fn check_env(repo: &Repo, env: &[(&str, &str)]) -> Run {
    repo.run(&["check", "--format", "json", "--base", "main"], env)
}

/// The messages of the weakenings `config-integrity` reported.
fn weakenings(run: &Run) -> Vec<String> {
    run.violations("config-integrity")
        .iter()
        .map(|v| {
            assert_eq!(v["title"], WEAKENED, "{v}");
            v["message"].as_str().unwrap().to_string()
        })
        .collect()
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

fn detail(run: &Run) -> String {
    run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn all(run: &Run) -> String {
    format!("exit {}\n{}{}", run.code, run.stdout, run.stderr)
}

/// The run stopped (exit 2) in `gate` for `reason`, and says `needle`.
fn assert_could_not_check(run: &Run, reason: &str, gate: &str, needle: &str) {
    assert_eq!(run.code, 2, "{}", all(run));
    assert_eq!(
        run.could_not_check(),
        (reason.to_string(), Some(gate.to_string())),
        "{}",
        all(run)
    );
    let d = detail(run);
    assert!(d.contains(needle), "`{needle}` not in: {d}");
}

// ---- item 1: an evidence key added over a built-in default -----------------------

/// `pattern = "."` accepts a pull request that references nothing; the built-in pattern
/// it replaces would not.
#[test]
fn an_issue_pattern_added_over_the_builtin_default_is_reported() {
    let repo = repo_with(ISSUE_LINK, ISSUE_LINK_ANYTHING);
    let run = repo.check_with_pr_metadata(&[], Some(NO_REFERENCE_TITLE), Some(NO_REFERENCE_BODY));
    // The loosening is real: the head-side gate accepts the pull request.
    assert_eq!(
        codes(&run, "issue-link"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    let found = weakenings(&run);
    assert!(
        found.len() == 1
            && found[0].starts_with("[issue-link] `pattern` changed from ")
            && found[0].contains("to \".\"")
            && found[0].contains("built-in default"),
        "{found:?}\n{}",
        all(&run)
    );
    assert_eq!(run.code, 1, "{}", all(&run));
}

/// Control: under the built-in pattern the same pull request is reported by `issue-link`.
#[test]
fn the_builtin_issue_pattern_rejects_a_pull_request_without_a_reference() {
    let repo = repo_with(ISSUE_LINK, ISSUE_LINK);
    let run = repo.check_with_pr_metadata(&[], Some(NO_REFERENCE_TITLE), Some(NO_REFERENCE_BODY));
    assert_eq!(codes(&run, "issue-link").len(), 1, "{}", all(&run));
    assert_eq!(weakenings(&run), Vec::<String>::new());
}

/// Control: the built-in pattern written down changes nothing.
#[test]
fn the_builtin_issue_pattern_written_down_is_not_a_weakening() {
    let head = format!("{ISSUE_LINK}pattern = '{BUILTIN_ISSUE_PATTERN}'\n");
    let repo = repo_with(ISSUE_LINK, &head);
    let run = repo.check_with_pr_metadata(&[], Some("chore: tidy (#12)"), Some(NO_REFERENCE_BODY));
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
}

/// The existing directive lifts the report, as it lifts every other weakening of the gate.
#[test]
fn the_directive_lifts_an_issue_pattern_added_over_the_default() {
    let head = "[gates.issue-link]\nenabled = true\npattern = 'TRACK-\\d+'\n";
    let repo = repo_with(ISSUE_LINK, head);
    let title = Some("chore: adopt the tracker pattern TRACK-7");
    let run = repo.check_with_pr_metadata(&[], title, Some(NO_REFERENCE_BODY));
    assert_eq!(weakenings(&run).len(), 1, "{}", all(&run));

    let lifted = "allow-gate-weakening: issue-link issues live in the TRACK tracker";
    let run = repo.check_with_pr_metadata(&[], title, Some(lifted));
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
    assert_eq!(
        run.outcome("config-integrity")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// A `command` written over the command a preset supplies replaces what the gate runs.
/// The runner authorised the execution; the replaced evidence is still reported.
#[test]
fn a_command_added_over_a_preset_default_is_reported() {
    let repo = repo_with(MUTANTS_PRESET_ONLY, MUTANTS_OWN_COMMAND);
    let run = check_env(&repo, &[ALLOW]);
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    let found = weakenings(&run);
    assert!(
        found.len() == 1
            && found[0]
                .starts_with("[command] `command` changed from \"cargo mutants --in-diff\" to "),
        "{found:?}\n{}",
        all(&run)
    );
    assert_eq!(run.code, 1, "{}", all(&run));
}

/// Control: with no preset, unset means no command, and adding one adds a check.
#[test]
fn a_command_added_where_there_was_none_is_not_a_weakening() {
    let repo = repo_with("[gates.command]\n", "[gates.command]\ncommand = \"true\"\n");
    let run = check_env(&repo, &[ALLOW]);
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
}

// ---- item 2: removing a key whose value is the effective default -----------------

#[test]
fn removing_a_key_equal_to_its_preset_default_is_not_a_weakening() {
    let repo = repo_with(
        &format!("{MUTANTS_NOTHING_RAN}{ZERO_GUARD_WRITTEN_DOWN}"),
        MUTANTS_NOTHING_RAN,
    );
    let run = repo.check(&[]);
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    // Nothing changed in force: the preset's guard still fires.
    assert_eq!(codes(&run, "command"), ["command/zero-items-executed"]);
}

#[test]
fn removing_the_builtin_issue_pattern_written_down_is_not_a_weakening() {
    let base = format!("{ISSUE_LINK}pattern = '{BUILTIN_ISSUE_PATTERN}'\n");
    let repo = repo_with(&base, ISSUE_LINK);
    let run = repo.check_with_pr_metadata(&[], Some("chore: tidy (#12)"), Some(NO_REFERENCE_BODY));
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
}

/// The same on a `commands` entry, which is judged as a whole.
#[test]
fn removing_an_entry_key_equal_to_its_preset_default_is_not_a_lost_entry() {
    let repo = repo_with(
        &format!("{MUTANTS_ENTRY}{ZERO_GUARD_WRITTEN_DOWN}"),
        MUTANTS_ENTRY,
    );
    let run = repo.check(&[]);
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(run.code, 0, "{}", all(&run));
}

/// Control: removing a value that is not the default still changes what is in force.
#[test]
fn removing_a_key_that_differs_from_its_preset_default_is_reported() {
    let repo = repo_with(
        &format!("{MUTANTS_NOTHING_RAN}{ZERO_GUARD_REPLACED}"),
        MUTANTS_NOTHING_RAN,
    );
    let found = weakenings(&repo.check(&[]));
    assert!(
        found.len() == 1 && found[0].starts_with("[command] `zero_items_pattern` removed"),
        "{found:?}"
    );

    let repo = repo_with(ISSUE_LINK_ANYTHING, ISSUE_LINK);
    let run = repo.check_with_pr_metadata(&[], Some("chore: tidy (#12)"), Some(NO_REFERENCE_BODY));
    let found = weakenings(&run);
    assert!(
        found.len() == 1 && found[0].starts_with("[issue-link] `pattern` removed"),
        "{found:?}"
    );
}

// ---- item 3: a preset first named by the head, under runner authorisation --------

/// The runner accepted the new preset's command. The guard that preset would apply is
/// replaced in the same change, so a run that tested nothing passes.
#[test]
fn a_key_added_beside_a_preset_the_head_first_names_is_judged_against_that_preset() {
    let head = format!("{MUTANTS_NOTHING_RAN}{ZERO_GUARD_REPLACED}");
    let repo = repo_with(NO_PRESET_NOTHING_RAN, &head);
    let run = check_env(&repo, &[ALLOW]);
    // The loosening is real: the command gate ran and saw nothing wrong.
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert_eq!(run.outcome("command")["examined"], 1);
    let found = weakenings(&run);
    assert!(
        found.len() == 1
            && found[0].starts_with(
                "[command] `zero_items_pattern` changed from \"0 mutants tested\" to \"never printed\""
            ),
        "{found:?}\n{}",
        all(&run)
    );
    assert_eq!(run.code, 1, "{}", all(&run));
}

/// The same on an entry the head adds.
#[test]
fn a_key_on_a_new_entry_is_judged_against_the_preset_the_entry_names() {
    let head = format!("{MUTANTS_ENTRY}{ZERO_GUARD_REPLACED}");
    let repo = repo_with("[gates.command]\n", &head);
    let run = check_env(&repo, &[ALLOW]);
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    let found = weakenings(&run);
    assert!(
        found.len() == 1
            && found[0].contains("zero_items_pattern")
            && found[0].contains("\"0 mutants tested\""),
        "{found:?}\n{}",
        all(&run)
    );
    assert_eq!(run.code, 1, "{}", all(&run));
}

/// Control: the new preset with its guards left alone, or written down, is not reported.
#[test]
fn a_preset_the_head_first_names_with_its_own_guards_is_not_a_weakening() {
    for rest in ["", ZERO_GUARD_WRITTEN_DOWN] {
        let head = format!("{MUTANTS_OWN_COMMAND}{rest}");
        let repo = repo_with(
            "[gates.command]\ncommand = \"echo 5 mutants tested\"\n",
            &head,
        );
        let run = check_env(&repo, &[ALLOW]);
        assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
        assert_eq!(run.code, 0, "{}", all(&run));
    }
}

/// Control: without the runner's authorisation the new preset is refused, as before.
#[test]
fn a_preset_the_head_first_names_is_refused_without_authorisation() {
    let head = format!("{MUTANTS_NOTHING_RAN}{ZERO_GUARD_REPLACED}");
    let repo = repo_with(NO_PRESET_NOTHING_RAN, &head);
    let run = repo.check(&[]);
    assert_eq!(codes(&run, "command"), [COMMAND_FINDING], "{}", all(&run));
}

// ---- item 4: the table-level snapshot with only entries --------------------------

/// An entry does not inherit the table's snapshot, so a table that declares no command of
/// its own compares the file with nothing.
#[test]
fn a_table_snapshot_with_no_table_command_is_a_configuration_error() {
    let repo = repo_unchanged_policy(SNAPSHOT_TABLE_ONLY_ENTRIES, &[("api.txt", API_ONE_LINE)]);
    let run = repo.check(&[]);
    assert_could_not_check(&run, "configuration", "command", "snapshot");
    assert!(
        detail(&run).contains("entries do not inherit"),
        "{}",
        detail(&run)
    );
}

/// Control: the same snapshot on the entry is compared.
#[test]
fn a_snapshot_on_the_entry_is_compared() {
    let repo = repo_unchanged_policy(SNAPSHOT_ON_THE_ENTRY, &[("api.txt", API_ONE_LINE)]);
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "command"),
        ["command/snapshot-mismatch"],
        "{}",
        all(&run)
    );
}

// ---- item 4: output past the capture limit ---------------------------------------

/// A command that prints more than the capture holds pushes the line a check looks for
/// past the limit. Each check that reads the output then cannot answer.
#[test]
fn output_past_the_capture_limit_stops_every_check_that_reads_it() {
    for (table, script) in [
        (FORBID_TABLE, FLOOD_THEN_FORBIDDEN),
        (ZERO_TABLE, FLOOD_THEN_NOTHING_RAN),
        (COUNT_TABLE, FLOOD_THEN_ZERO_COUNT),
    ] {
        let repo = repo_unchanged_policy(table, &[("out.sh", script)]);
        let run = repo.check(&[]);
        assert_could_not_check(&run, "gate", "command", "capture limit");
        assert!(detail(&run).contains("26214400"), "{}", detail(&run));
    }
}

/// Control: under the limit the same line is found.
#[test]
fn a_forbidden_line_within_the_capture_limit_is_reported() {
    let repo = repo_unchanged_policy(FORBID_TABLE, &[("out.sh", SHORT_THEN_FORBIDDEN)]);
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "command"),
        ["command/forbidden-output"],
        "{}",
        all(&run)
    );
    assert_eq!(run.code, 1);
}

/// Control: a command judged by its exit status alone has no check that reads the output.
#[test]
fn output_past_the_capture_limit_is_a_note_when_nothing_reads_it() {
    let repo = repo_unchanged_policy(STATUS_ONLY_TABLE, &[("out.sh", FLOOD_ONLY)]);
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}", all(&run));
    assert!(
        notes(&run, "command").contains("capture limit"),
        "{}",
        all(&run)
    );
}

/// `base-tests` reads the failed cases from the output; a report cut off is not a pass.
#[test]
fn base_tests_output_past_the_capture_limit_is_not_a_pass() {
    let repo = repo_unchanged_policy(BASE_TESTS_TABLE, &[("out.sh", FLOOD_THEN_FAILED_CASE)]);
    let run = repo.check(&[]);
    assert_could_not_check(&run, "gate", "command", "capture limit");
}

// ---- item 4: where and when the snapshot is read ---------------------------------

/// The command writes its own output into the snapshot before printing it. The file is
/// read before anything runs, so the comparison is with what was committed.
#[test]
fn a_command_cannot_rewrite_the_snapshot_it_is_compared_with() {
    let repo = repo_unchanged_policy(
        SNAPSHOT_TABLE,
        &[
            ("api.txt", API_ONE_LINE),
            ("render.sh", RENDER_REWRITES_SNAPSHOT),
        ],
    );
    let run = repo.check(&[]);
    assert_eq!(
        codes(&run, "command"),
        ["command/snapshot-mismatch"],
        "{}",
        all(&run)
    );
    assert_eq!(run.code, 1);
}

/// Control: a command that leaves the snapshot alone and prints it matches.
#[test]
fn a_command_that_prints_the_committed_snapshot_matches() {
    let repo = repo_unchanged_policy(
        SNAPSHOT_TABLE,
        &[
            ("api.txt", API_ONE_LINE),
            ("render.sh", RENDER_API_ONE_LINE),
        ],
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}", all(&run));
    assert_eq!(run.outcome("command")["examined"], 1);
}

/// A snapshot that is a symbolic link compares the output with whatever the link names.
#[cfg(unix)]
#[test]
fn a_snapshot_that_is_a_symbolic_link_is_refused() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", &config(SNAPSHOT_TABLE));
    repo.write("render.sh", RENDER_API_ONE_LINE);
    repo.write("elsewhere.txt", API_ONE_LINE);
    std::os::unix::fs::symlink("elsewhere.txt", repo.file("api.txt")).unwrap();
    repo.commit("ci: base configuration");
    repo.git(&["checkout", "-q", "-B", "work", "main"]);
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1, Phase 2, then Phase 3.\n",
    );
    repo.commit("docs: extend the plan");

    let run = repo.check(&[]);
    assert_could_not_check(&run, "gate", "command", "symbolic link");
}

/// A snapshot larger than the capture limit can never equal a captured output, and is not
/// read into memory to find that out.
#[test]
fn a_snapshot_past_the_capture_limit_is_refused() {
    let big = "a".repeat(26_214_401);
    let repo = repo_unchanged_policy(
        SNAPSHOT_TABLE,
        &[
            ("api.txt", big.as_str()),
            ("render.sh", RENDER_API_ONE_LINE),
        ],
    );
    let run = repo.check(&[]);
    assert_could_not_check(&run, "gate", "command", "26214400");
}

// ---- item 4: the presets `init` writes -------------------------------------------

/// `pit`'s own command is Maven's; a Gradle project gets the Gradle task beside it.
#[test]
fn init_gives_a_gradle_project_a_gradle_mutation_command() {
    let repo = Repo::new();
    repo.write("build.gradle", GRADLE_BUILD);
    assert_eq!(repo.run(&["init", "--name", "demo"], &[]).code, 0);
    let starter = std::fs::read_to_string(repo.file("discipline.toml")).unwrap();
    assert!(starter.contains("# preset = \"pit\""), "{starter}");
    assert!(
        starter.contains("# command = \"gradle pitest\""),
        "{starter}"
    );
    assert!(!starter.contains("mvn "), "{starter}");
}

/// With no test runner detected the starter's commented lines name Cargo's tools; they
/// are written as an example to replace, not as what the repository uses.
#[test]
fn init_says_its_lines_are_an_example_when_no_runner_is_detected() {
    let repo = Repo::new();
    assert_eq!(repo.run(&["init", "--name", "demo"], &[]).code, 0);
    let starter = std::fs::read_to_string(repo.file("discipline.toml")).unwrap();
    assert!(starter.contains(NO_RUNNER_DETECTED), "{starter}");
    assert_eq!(repo.run(&["gates"], &[]).code, 0, "the starter must load");
}

/// Control: a Cargo project still gets the diff-scoped preset.
#[test]
fn init_gives_a_cargo_project_the_diff_scoped_preset() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[package]\nname = \"demo\"\n");
    assert_eq!(repo.run(&["init", "--name", "demo"], &[]).code, 0);
    let starter = std::fs::read_to_string(repo.file("discipline.toml")).unwrap();
    assert!(
        starter.contains("# Diff-scoped mutation testing"),
        "{starter}"
    );
    assert!(
        starter.contains("# preset = \"cargo-mutants\""),
        "{starter}"
    );
}

// ---- item 5: `test_command` runs under the same bounds ---------------------------

/// A listing cut off at the capture limit is not a count.
#[test]
fn a_test_listing_past_the_capture_limit_is_not_counted() {
    let repo = repo_unchanged_policy(TEST_FLOOR_LISTING, &[("list.sh", FLOOD_THEN_COUNT)]);
    let run = repo.check(&[]);
    assert_could_not_check(&run, "gate", "test-floor", "capture limit");
}

/// Control: a listing within the limit is counted.
#[test]
fn a_test_listing_within_the_capture_limit_is_counted() {
    let repo = repo_unchanged_policy(TEST_FLOOR_LISTING, &[("list.sh", SHORT_LISTING)]);
    let run = repo.check(&[]);
    assert_eq!(run.outcome("test-floor")["examined"], 7, "{}", all(&run));
    assert_eq!(codes(&run, "test-floor"), Vec::<String>::new());
}

// ---- item 6: what `DISCIPLINE_COMMAND` authorises --------------------------------

fn marking_canary() -> String {
    format!("canary_command = \"sh -c 'touch {MARKER}; exit 1'\"\n")
}

fn marking_command() -> String {
    format!("command = \"sh -c 'touch {MARKER}'\"\n")
}

fn marking_test_command() -> String {
    format!("test_command = \"sh -c 'touch {MARKER}; echo 99999'\"\n")
}

/// The command gate refused the change under `env`: its finding, nothing run.
fn assert_command_refused(repo: &Repo, env: &[(&str, &str)]) {
    let run = check_env(repo, env);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        codes(&run, "command"),
        [COMMAND_FINDING],
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert_eq!(run.outcome("command")["examined"], 0);
    assert!(!ran, "the command the change supplied was executed");
}

const RUNNER_COMMAND: (&str, &str) = ("DISCIPLINE_COMMAND", "true");

#[test]
fn discipline_command_does_not_authorise_a_canary_the_change_adds() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(base, &format!("{base}{}", marking_canary()));
    assert_command_refused(&repo, &[RUNNER_COMMAND]);
}

#[test]
fn discipline_command_does_not_authorise_an_entry_the_change_adds() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let entry = format!(
        "\n[[gates.command.commands]]\nname = \"extra\"\n{}",
        marking_command()
    );
    let repo = repo_with(base, &format!("{base}{entry}"));
    assert_command_refused(&repo, &[RUNNER_COMMAND]);
}

/// A preset names a default canary, which the runner's command does not replace.
#[test]
fn discipline_command_does_not_authorise_a_preset_the_change_names() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(base, &format!("{base}preset = \"cargo-deny\"\n"));
    assert_command_refused(&repo, &[RUNNER_COMMAND]);
}

#[test]
fn discipline_command_does_not_authorise_a_test_command_the_change_adds() {
    let base = "[gates.test-floor]\nmin_tests = 1\n";
    let repo = repo_with(base, &format!("{base}{}", marking_test_command()));
    let run = check_env(&repo, &[RUNNER_COMMAND]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        codes(&run, "test-floor"),
        [TEST_COMMAND_FINDING],
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the test command the change supplied was executed");
}

/// Control: what the variable supplies replaces the table's command, so the change's own
/// `command` is never run and the difference is moot.
#[test]
fn discipline_command_replaces_a_command_the_change_alters() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(base, &format!("[gates.command]\n{}", marking_command()));
    let run = check_env(
        &repo,
        &[("DISCIPLINE_COMMAND", "sh -c 'touch RAN_BY_RUNNER'")],
    );
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert_eq!(run.outcome("command")["examined"], 1);
    assert!(repo.file("RAN_BY_RUNNER").exists(), "{}", all(&run));
    assert!(!repo.file(MARKER).exists(), "the change's command ran");
}

/// Control: `DISCIPLINE_ALLOW_COMMAND_CHANGE` still accepts every change to what runs.
#[test]
fn allow_command_change_still_authorises_an_added_canary() {
    let base = "[gates.command]\ncommand = \"true\"\n";
    let repo = repo_with(base, &format!("{base}{}", marking_canary()));
    let run = check_env(&repo, &[ALLOW]);
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert!(repo.file(MARKER).exists(), "{}", all(&run));
}

const UNIT_ENTRY: &str = "[gates.command]\n\n[[gates.command.commands]]\nname = \"unit\"\n";
const RUNNER_UNIT_COMMAND: (&str, &str) =
    ("DISCIPLINE_COMMAND_UNIT", "sh -c 'touch RAN_BY_RUNNER'");

/// The per-entry variable supplies that entry's command, so a change to it is moot.
#[test]
fn an_entry_override_replaces_that_entry_command() {
    let repo = repo_with(
        &format!("{UNIT_ENTRY}command = \"true\"\n"),
        &format!("{UNIT_ENTRY}{}", marking_command()),
    );
    let run = check_env(&repo, &[RUNNER_UNIT_COMMAND]);
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert!(repo.file("RAN_BY_RUNNER").exists(), "{}", all(&run));
    assert!(!repo.file(MARKER).exists(), "the change's command ran");
}

/// It authorises nothing else: not that entry's canary, and not another entry's command.
#[test]
fn an_entry_override_authorises_only_that_entry_command() {
    let base = format!("{UNIT_ENTRY}command = \"true\"\n");
    let repo = repo_with(&base, &format!("{base}{}", marking_canary()));
    assert_command_refused(&repo, &[RUNNER_UNIT_COMMAND]);

    let other = "\n[[gates.command.commands]]\nname = \"lint\"\n";
    let repo = repo_with(
        &format!("{base}{other}command = \"true\"\n"),
        &format!("{base}{other}{}", marking_command()),
    );
    assert_command_refused(&repo, &[RUNNER_UNIT_COMMAND]);
}

// ---- item 7: a gate the base had disabled ----------------------------------------

/// The command is the base's own text; the change decided that it runs. Enabling a gate
/// is no weakening, and the run says what it executed.
#[test]
fn a_command_table_the_change_enables_runs_with_a_note() {
    let repo = repo_with(DISABLED_WITH_COMMAND, ENABLED_WITH_COMMAND);
    let run = repo.check(&[]);
    assert!(repo.file("RAN_BASE_COMMAND").exists(), "{}", all(&run));
    assert_eq!(weakenings(&run), Vec::<String>::new(), "{}", all(&run));
    assert_eq!(codes(&run, "command"), Vec::<String>::new());
    assert_eq!(run.code, 0, "{}", all(&run));
    let said = notes(&run, "command");
    assert!(
        said.contains("disabled on the base side") && said.contains("this change enables"),
        "{said}"
    );
}

/// Control: a table both sides enable carries no such note.
#[test]
fn a_command_table_both_sides_enable_has_no_enabling_note() {
    let repo = repo_with(ENABLED_WITH_COMMAND, ENABLED_WITH_COMMAND);
    let run = repo.check(&[]);
    assert!(repo.file("RAN_BASE_COMMAND").exists(), "{}", all(&run));
    assert!(!notes(&run, "command").contains("disabled on the base side"));
}

// ---- item 8: a run with no base --------------------------------------------------

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

fn staged(repo: &Repo, env: &[(&str, &str)]) -> Run {
    repo.run(&["check", "--format", "json", "--staged"], env)
}

/// In CI there is no base to vouch for the command, so nothing the change wrote runs.
#[test]
fn a_ci_run_with_no_base_runs_no_configured_command() {
    for ci in [
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "GITEA_ACTIONS",
        "FORGEJO_ACTIONS",
    ] {
        let repo = unborn_repo(&format!("[gates.command]\n{}", marking_command()));
        let run = staged(&repo, &[(ci, "true")]);
        let ran = repo.file(MARKER).exists();
        assert_eq!(
            codes(&run, "command"),
            [COMMAND_FINDING],
            "{ci}: marker exists: {ran}; {}",
            all(&run)
        );
        assert!(!ran, "{ci}: the configured command ran with no base");
    }
}

#[test]
fn a_ci_run_with_no_base_runs_no_configured_test_command() {
    let repo = unborn_repo(&format!(
        "[gates.test-floor]\nmin_tests = 1\n{}",
        marking_test_command()
    ));
    let run = staged(&repo, &[("CI", "true")]);
    let ran = repo.file(MARKER).exists();
    assert_eq!(
        codes(&run, "test-floor"),
        [TEST_COMMAND_FINDING],
        "marker exists: {ran}; {}",
        all(&run)
    );
    assert!(!ran, "the configured test command ran with no base");
}

/// The runner can still vouch for it.
#[test]
fn a_ci_run_with_no_base_runs_a_command_the_runner_authorises() {
    let repo = unborn_repo(&format!("[gates.command]\n{}", marking_command()));
    let run = staged(&repo, &[("CI", "true"), ALLOW]);
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert!(repo.file(MARKER).exists(), "{}", all(&run));
}

/// Control: on a developer's machine the first commit's own configuration runs.
#[test]
fn a_local_run_with_no_base_runs_the_configured_command() {
    let repo = unborn_repo(&format!("[gates.command]\n{}", marking_command()));
    let run = staged(&repo, &[]);
    assert_eq!(
        codes(&run, "command"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert!(repo.file(MARKER).exists(), "{}", all(&run));
}

/// Control: a run whose base does not resolve never reaches a gate.
#[test]
fn a_run_whose_base_does_not_resolve_stops_before_any_command() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &config(&format!("[gates.command]\n{}", marking_command())),
    );
    repo.commit("ci: configure a command");
    let run = repo.run(
        &["check", "--format", "json", "--base", "no-such-ref"],
        &[("CI", "true")],
    );
    assert_eq!(run.code, 2, "{}", all(&run));
    assert_eq!(run.could_not_check().0, "repository", "{}", all(&run));
    assert!(!repo.file(MARKER).exists(), "a command ran without a base");
}
