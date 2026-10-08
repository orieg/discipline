//! Executed keys supplied through the runner's inline override, evidence keys that were
//! not judged, configured patterns compiled before any gate runs, and reads that were
//! skipped without a note.
//!
//! Each case builds a temporary repository and runs the real binary against `main`. A
//! configured command only ever creates a marker file there; `cargo` is a stand-in
//! script, first on `PATH`, and no real toolchain is started.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const HEAD: &str = CONFIG_HEAD;
const MARKER: &str = "RAN_FROM_OVERRIDE";
const ALLOW: (&str, &str) = ("DISCIPLINE_ALLOW_COMMAND_CHANGE", "1");
const WEAKENED: &str = "config-integrity/gate-weakened";

fn config(gates: &str) -> String {
    format!("{HEAD}{gates}")
}

/// `base` committed on `main`, then `head` committed on `work`.
fn repo_with(base: &str, head: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base("discipline.toml", &config(base), "ci: base configuration");
    repo.write("discipline.toml", &config(head));
    repo.commit("chore: change the configuration");
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

fn check(repo: &Repo, extra: &[&str], env: &[(&str, &str)]) -> Run {
    let mut args = vec!["check", "--format", "json", "--base", "main"];
    args.extend(extra);
    repo.run(&args, env)
}

fn all(run: &Run) -> String {
    format!("exit {}\n{}{}", run.code, run.stdout, run.stderr)
}

fn codes(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

fn messages(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["message"].as_str().unwrap().to_string())
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

/// The run stopped before any gate on `key` of `gate`'s table.
fn stopped_on(run: &Run, gate: &str, key: &str) -> bool {
    run.code == 2
        && run.could_not_check() == ("configuration".to_string(), Some(gate.to_string()))
        && detail(run).contains(key)
}

/// A directory holding a stand-in `cargo`, and the `PATH` that puts it first. The
/// stand-in records that it ran and prints a passing test summary.
struct FakeCargo {
    _dir: tempfile::TempDir,
    path: String,
}

fn fake_cargo() -> FakeCargo {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("cargo");
    std::fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> CARGO_RAN\n\
         case \" $* \" in *race_canary*) echo 'ThreadSanitizer: data race'; exit 1;; esac\n\
         echo 'test result: ok. 1 passed'\n",
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", dir.path().display());
    FakeCargo { _dir: dir, path }
}

// ---- executed keys supplied through the inline override -----------------------------------

const TOUCH: &str = "sh -c 'touch RAN_FROM_OVERRIDE; echo 3 passed'";

/// One executed key set through `--config-override` over a file both sides agree on:
/// `(file, override, gate, finding)`.
const OVERRIDDEN_EXECUTED_KEYS: &[(&str, &str, &str, &str)] = &[
    (
        "[gates.command]\nenabled = true\n",
        "[gates.command]\ncommand = \"sh -c 'touch RAN_FROM_OVERRIDE; echo 3 passed'\"\n",
        "command",
        "command/untrusted-command-modification",
    ),
    (
        "[gates.test-floor]\nenabled = true\nmin_tests = 1\n",
        "[gates.test-floor]\ntest_command = \"sh -c 'touch RAN_FROM_OVERRIDE; echo 3 passed'\"\n",
        "test-floor",
        "test-floor/untrusted-test-command",
    ),
    (
        "[gates.msrv]\nenabled = true\npinned_version = \"1.90\"\n",
        "[gates.msrv]\ncommand = \"sh -c 'touch RAN_FROM_OVERRIDE; echo 3 passed'\"\n",
        "msrv",
        "msrv/untrusted-command-modification",
    ),
];

/// An executed key that reaches the run through the inline override is compared with the
/// base ref's file like one the change wrote: it is not run.
#[test]
fn an_executed_key_from_the_inline_override_is_not_run_without_the_runner_switch() {
    for (file, over, gate, finding) in OVERRIDDEN_EXECUTED_KEYS {
        assert!(over.contains(TOUCH), "{over}");
        for by_env in [false, true] {
            let repo = repo_unchanged(file);
            let run = if by_env {
                check(&repo, &[], &[("DISCIPLINE_CONFIG_OVERRIDE", over)])
            } else {
                check(&repo, &["--config-override", over], &[])
            };
            assert_eq!(
                codes(&run, gate),
                [finding.to_string()],
                "{gate} (env: {by_env}): {}",
                all(&run)
            );
            assert_eq!(run.code, 1, "{gate}: {}", all(&run));
            assert!(
                !repo.file(MARKER).exists(),
                "{gate} (env: {by_env}): the override's command ran"
            );
        }
    }
}

/// Control: with the runner switch the same override is run.
#[test]
fn an_executed_key_from_the_inline_override_runs_with_the_runner_switch() {
    for (file, over, gate, finding) in OVERRIDDEN_EXECUTED_KEYS {
        let repo = repo_unchanged(file);
        let run = check(&repo, &["--config-override", over], &[ALLOW]);
        assert!(
            !codes(&run, gate).contains(&finding.to_string()),
            "{gate}: {}",
            all(&run)
        );
        assert!(
            repo.file(MARKER).exists(),
            "{gate}: the authorised command did not run; {}",
            all(&run)
        );
    }
}

// ---- msrv: `pinned_version` ---------------------------------------------------------------

fn msrv(version: &str) -> String {
    format!("[gates.msrv]\nenabled = true\npinned_version = \"{version}\"\n")
}

const MSRV_UNPINNED: &str = "[gates.msrv]\nenabled = true\n";

#[test]
fn a_lowered_pinned_version_is_reported() {
    // `1.10` to `1.9` is lower as a version and higher as text.
    for (base, head) in [("1.90", "1.80"), ("1.10", "1.9"), ("1.90.1", "1.90")] {
        let repo = repo_with(&msrv(base), &msrv(head));
        let run = check(&repo, &[], &[]);
        let said = messages(&run, "config-integrity");
        assert_eq!(
            codes(&run, "config-integrity"),
            [WEAKENED],
            "{base} -> {head}: {}",
            all(&run)
        );
        assert!(
            said[0].contains("[msrv] `pinned_version` decreased"),
            "{said:?}"
        );
        assert_eq!(run.code, 1, "{}", all(&run));
    }
}

#[test]
fn a_removed_pinned_version_is_reported() {
    let repo = repo_with(&msrv("1.90"), MSRV_UNPINNED);
    let run = check(&repo, &[], &[]);
    let said = messages(&run, "config-integrity");
    assert_eq!(codes(&run, "config-integrity"), [WEAKENED], "{}", all(&run));
    assert!(
        said[0].contains("[msrv] `pinned_version` removed"),
        "{said:?}"
    );
}

/// Controls: a raised version, an equal one written differently, and one added where
/// the base had none are not weakenings; the directive of the gate lifts a lowered one.
#[test]
fn a_raised_or_added_pinned_version_is_not_reported_and_the_directive_lifts_a_lowered_one() {
    for (base, head) in [
        (msrv("1.80"), msrv("1.90")),
        (msrv("1.9"), msrv("1.10")),
        (msrv("1.90"), msrv("1.90.0")),
        (MSRV_UNPINNED.to_string(), msrv("1.70")),
    ] {
        let repo = repo_with(&base, &head);
        let run = check(&repo, &[], &[]);
        assert_eq!(
            codes(&run, "config-integrity"),
            Vec::<String>::new(),
            "{base} -> {head}: {}",
            all(&run)
        );
    }
    let repo = repo_with(&msrv("1.90"), &msrv("1.80"));
    let run = repo.check_with_pr(
        &[],
        "allow-gate-weakening: msrv the oldest supported compiler is 1.80\n",
    );
    assert_eq!(
        codes(&run, "config-integrity"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
    assert_eq!(
        run.outcome("config-integrity")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{}",
        all(&run)
    );
}

// ---- sanitizers: the canary and the sanitizer it belongs to ------------------------------

const CANARY_KEY: &str = "gates.sanitizers.canary";

fn sanitizers(name: &str, canary: bool) -> String {
    format!("[gates.sanitizers]\nenabled = true\nsanitizer = \"{name}\"\ncanary = {canary}\n")
}

/// The canary is a data race and its expected diagnostic is ThreadSanitizer's; under
/// another sanitizer it could not pass, so the pair is refused before anything runs.
#[test]
fn a_canary_with_a_sanitizer_other_than_thread_is_a_configuration_error() {
    for name in ["address", "memory", "leak"] {
        let repo = repo_unchanged(&sanitizers(name, true));
        let tool = fake_cargo();
        let run = check(&repo, &[], &[("PATH", &tool.path)]);
        assert!(
            stopped_on(&run, "sanitizers", CANARY_KEY),
            "{name}: {}",
            all(&run)
        );
        assert!(detail(&run).contains("thread"), "{}", detail(&run));
        assert!(
            !repo.file("CARGO_RAN").exists(),
            "{name}: cargo ran before the configuration was refused"
        );
    }
}

/// Controls: the canary under `thread`, another sanitizer without the canary, and the
/// refused pair in a gate that is switched off.
#[test]
fn the_canary_under_thread_and_another_sanitizer_without_it_still_run() {
    for (gates, ran) in [
        (sanitizers("thread", true), true),
        (sanitizers("address", false), true),
        (
            sanitizers("address", true).replace("enabled = true", "enabled = false"),
            false,
        ),
    ] {
        let repo = repo_unchanged(&gates);
        let tool = fake_cargo();
        let run = check(&repo, &[], &[("PATH", &tool.path)]);
        assert_eq!(run.code, 0, "{gates}: {}", all(&run));
        assert_eq!(repo.file("CARGO_RAN").exists(), ran, "{gates}");
    }
}

// ---- test-floor: a counting basis the base did not have -----------------------------------

const FLOOR: &str = "[gates.test-floor]\nenabled = true\nmin_tests = 1\n";
const REPORT_LINE: &str = "test_report = \"reports/junit.xml\"\n";
const COMMAND_LINE: &str = "test_command = \"sh -c 'echo 3 passed'\"\n";
const JUNIT: &str = "<testsuite tests=\"2\"><testcase classname=\"a\" name=\"one\"/><testcase classname=\"a\" name=\"two\"/></testsuite>\n";

fn floor_repo(base: &str, head: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(base)),
            ("reports/junit.xml", JUNIT),
        ],
        "ci: base configuration",
    );
    repo.write("discipline.toml", &config(head));
    repo.commit("chore: change the configuration");
    repo
}

#[test]
fn a_test_report_added_where_the_base_had_none_is_a_change_of_evidence() {
    let repo = floor_repo(FLOOR, &format!("{FLOOR}{REPORT_LINE}"));
    let run = check(&repo, &[], &[]);
    let said = messages(&run, "config-integrity");
    assert_eq!(codes(&run, "config-integrity"), [WEAKENED], "{}", all(&run));
    assert!(
        said[0].contains("[test-floor] `test_report` changed from unset"),
        "{said:?}"
    );
    assert_eq!(run.code, 1, "{}", all(&run));

    // The gate's existing directive lifts it.
    let run = repo.check_with_pr(
        &[],
        "allow-gate-weakening: test-floor the runner now writes a report\n",
    );
    assert_eq!(
        codes(&run, "config-integrity"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
}

/// The command is only read at all when the runner authorises it; it is then a new
/// counting basis like the report.
#[test]
fn a_test_command_added_where_the_base_had_none_is_a_change_of_evidence() {
    let repo = floor_repo(FLOOR, &format!("{FLOOR}{COMMAND_LINE}"));
    let run = check(&repo, &[], &[ALLOW]);
    let said = messages(&run, "config-integrity");
    assert_eq!(codes(&run, "config-integrity"), [WEAKENED], "{}", all(&run));
    assert!(
        said[0].contains("[test-floor] `test_command` changed from unset"),
        "{said:?}"
    );
}

/// Already judged: a counting basis the change removes.
#[test]
fn a_removed_test_report_or_test_command_is_reported() {
    for (line, key) in [(REPORT_LINE, "test_report"), (COMMAND_LINE, "test_command")] {
        let repo = floor_repo(&format!("{FLOOR}{line}"), FLOOR);
        let run = check(&repo, &[], &[ALLOW]);
        let said = messages(&run, "config-integrity");
        assert_eq!(codes(&run, "config-integrity"), [WEAKENED], "{}", all(&run));
        assert!(
            said[0].contains(&format!("[test-floor] `{key}` removed")),
            "{said:?}"
        );
    }
}

/// Control: the same basis on both sides is no change.
#[test]
fn an_unchanged_counting_basis_is_not_reported() {
    let both = format!("{FLOOR}{REPORT_LINE}");
    let repo = floor_repo(&both, &format!("{both}tolerance = 0\n"));
    let run = check(&repo, &[], &[]);
    assert_eq!(
        codes(&run, "config-integrity"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
}

// ---- configured patterns are compiled before any gate runs --------------------------------

/// A table whose one pattern does not compile: `(table, gate, key named in the error)`.
const BAD_PATTERNS: &[(&str, &str, &str)] = &[
    (
        "[gates.time-estimates]\nenabled = true\nextra_patterns = ['(a']\n",
        "time-estimates",
        "gates.time-estimates.extra_patterns",
    ),
    (
        "[gates.time-estimates]\nenabled = true\nallow_patterns = ['(a']\n",
        "time-estimates",
        "gates.time-estimates.allow_patterns",
    ),
    (
        "[gates.pii]\nenabled = true\nextra_patterns = ['(a']\n",
        "pii",
        "gates.pii.extra_patterns",
    ),
    (
        "[gates.pii]\nenabled = true\nallow_patterns = ['(a']\n",
        "pii",
        "gates.pii.allow_patterns",
    ),
    (
        "[gates.shell-secrets]\nenabled = true\nextra_secret_patterns = ['(a']\n",
        "shell-secrets",
        "gates.shell-secrets.extra_secret_patterns",
    ),
    (
        "[gates.shell-secrets]\nenabled = true\nallow_patterns = ['(a']\n",
        "shell-secrets",
        "gates.shell-secrets.allow_patterns",
    ),
    (
        "[gates.issue-link]\nenabled = true\npattern = '(a'\n",
        "issue-link",
        "gates.issue-link.pattern",
    ),
    (
        "[gates.archive-contents]\nenabled = true\narchive_path = \"dist/*.tgz\"\nforbidden_patterns = ['(a']\n",
        "archive-contents",
        "gates.archive-contents.forbidden_patterns",
    ),
    (
        "[gates.manifest-sync]\nenabled = true\n[[gates.manifest-sync.rules]]\nmanifest = \"absent.toml\"\nextract_regex = '(a'\nwatched_paths = [\"a/**\"]\n",
        "manifest-sync",
        "gates.manifest-sync.rules[0].extract_regex",
    ),
    (
        "[gates.version-lockstep]\nenabled = true\n[[gates.version-lockstep.groups]]\nname = \"g\"\n[[gates.version-lockstep.groups.sources]]\npath = \"AGENTS.md\"\nregex = '(a'\n",
        "version-lockstep",
        "gates.version-lockstep.groups[g].sources[0].regex",
    ),
    (
        "[gates.provenance-tags]\nenabled = true\nratio_satisfied_by = ['regex:(a']\n",
        "provenance-tags",
        "gates.provenance-tags.ratio_satisfied_by",
    ),
    (
        "[gates.command]\nenabled = true\ncommand = \"true\"\nsnapshot_ignore = ['(a']\n",
        "command",
        "gates.command.snapshot_ignore",
    ),
    (
        "[gates.command]\nenabled = true\n[[gates.command.commands]]\nname = \"api\"\ncommand = \"true\"\nsnapshot_ignore = ['(a']\n",
        "command",
        "gates.command.commands[api].snapshot_ignore",
    ),
];

/// Every configured pattern is compiled before any gate runs, and one that does not
/// compile stops the run naming the gate and the key.
#[test]
fn a_pattern_that_does_not_compile_stops_the_run_before_any_gate() {
    for (table, gate, key) in BAD_PATTERNS {
        let repo = repo_unchanged(table);
        let run = check(&repo, &[], &[]);
        assert!(
            stopped_on(&run, gate, &format!("`{key}`")),
            "{key}: {}",
            all(&run)
        );
        assert!(
            run.json()["outcomes"]
                .as_array()
                .is_none_or(|o| o.is_empty()),
            "{key}: a gate ran; {}",
            all(&run)
        );
    }
}

/// A gate that returned before reading its pattern passed with the pattern unread: a
/// staged run skips `issue-link`, and a manifest that predates the configuration is
/// skipped by `manifest-sync`.
#[test]
fn a_pattern_a_gate_would_not_have_reached_is_still_compiled() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &config("[gates.issue-link]\nenabled = true\npattern = '(a'\n"),
    );
    repo.git(&["add", "-A"]);
    let run = repo.run(&["check", "--format", "json", "--staged"], &[]);
    assert!(
        stopped_on(&run, "issue-link", "`gates.issue-link.pattern`"),
        "{}",
        all(&run)
    );
}

/// Controls: the same tables with a pattern that compiles run, and a pattern that does
/// not compile in a gate that is switched off is not read.
#[test]
fn patterns_that_compile_and_patterns_of_a_disabled_gate_do_not_stop_the_run() {
    for (table, gate, key) in BAD_PATTERNS {
        // `archive-contents`, `manifest-sync` and `version-lockstep` cannot run in this
        // repository for other reasons; their compiled form is covered by unit tests.
        if ["archive-contents", "manifest-sync", "command"].contains(gate) {
            continue;
        }
        let good = table.replace("(a", "(a)");
        let repo = repo_unchanged(&good);
        let run = check(&repo, &[], &[]);
        assert!(
            run.code != 2 || !detail(&run).contains(key),
            "{key}: {}",
            all(&run)
        );
        let off = table.replace("enabled = true", "enabled = false");
        let repo = repo_unchanged(&off);
        let run = check(&repo, &[], &[]);
        assert_eq!(run.code, 0, "{key} (gate off): {}", all(&run));
    }
}

// ---- ci-integrity: a side that could not be parsed is named -------------------------------

const WORKFLOW_TWO_JOBS: &str = "name: ci\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test --all\n  lint:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo clippy\n";
const WORKFLOW_ONE_JOB: &str = "name: ci\non: [push]\njobs:\n  lint:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo clippy\n";
const WORKFLOW_OTHER: &str = "name: other\non: [push]\njobs:\n  docs:\n    runs-on: ubuntu-latest\n    steps:\n      - run: mkdocs build\n";
const WORKFLOW_WITH_THE_STEP: &str = "name: other\non: [push]\njobs:\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test --all\n";
const NOT_YAML: &str = "jobs: [unclosed\n  test: {\n";
const CI: &str = "[gates.ci-integrity]\nenabled = true\n";

/// A removed job is looked for among the jobs the change added, in every workflow file.
/// A file whose head side does not parse was skipped without a word.
#[test]
fn a_workflow_whose_head_side_does_not_parse_is_named_when_added_jobs_are_read() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(CI)),
            (".github/workflows/ci.yml", WORKFLOW_TWO_JOBS),
            (".github/workflows/broken.yml", NOT_YAML),
        ],
        "ci: workflows",
    );
    repo.write(".github/workflows/ci.yml", WORKFLOW_ONE_JOB);
    repo.commit("ci: drop the test job");
    let run = check(&repo, &[], &[]);
    let said = notes(&run, "ci-integrity");
    assert!(
        said.contains(
            ".github/workflows/broken.yml: the head side could not be parsed, so its jobs were not compared"
        ),
        "{said}\n{}",
        all(&run)
    );
    // The removed job is still reported.
    assert!(!codes(&run, "ci-integrity").is_empty(), "{}", all(&run));
}

/// The base side of another workflow that does not parse: which of its jobs the change
/// added cannot be told. Every job of the file read as added, so a job that carries the
/// removed job's step made the removal a move.
#[test]
fn a_workflow_whose_base_side_does_not_parse_is_named_and_is_not_where_a_job_moved_to() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(CI)),
            (".github/workflows/ci.yml", WORKFLOW_TWO_JOBS),
            (".github/workflows/other.yml", NOT_YAML),
        ],
        "ci: workflows",
    );
    repo.write(".github/workflows/ci.yml", WORKFLOW_ONE_JOB);
    repo.write(".github/workflows/other.yml", WORKFLOW_WITH_THE_STEP);
    repo.commit("ci: drop the test job");
    let run = check(&repo, &[], &[]);
    let said = notes(&run, "ci-integrity");
    assert!(
        said.contains(
            ".github/workflows/other.yml: the base side could not be parsed, so its jobs were not compared"
        ),
        "{said}\n{}",
        all(&run)
    );
    assert!(!said.contains("treated as a rename"), "{said}");
    assert!(
        messages(&run, "ci-integrity")
            .iter()
            .any(|m| m.contains("'test'")),
        "{}",
        all(&run)
    );
}

/// Control: with a base side that parses, the same job in the other file is where the
/// removed job moved to.
#[test]
fn a_job_added_to_a_readable_workflow_is_where_a_removed_job_moved_to() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(CI)),
            (".github/workflows/ci.yml", WORKFLOW_TWO_JOBS),
            (".github/workflows/other.yml", WORKFLOW_OTHER),
        ],
        "ci: workflows",
    );
    repo.write(".github/workflows/ci.yml", WORKFLOW_ONE_JOB);
    repo.write(".github/workflows/other.yml", WORKFLOW_WITH_THE_STEP);
    repo.commit("ci: move the test job");
    let run = check(&repo, &[], &[]);
    let said = notes(&run, "ci-integrity");
    assert!(
        said.contains("treated as a rename"),
        "{said}\n{}",
        all(&run)
    );
    assert!(
        !messages(&run, "ci-integrity")
            .iter()
            .any(|m| m.contains("'test'")),
        "{}",
        all(&run)
    );
}

/// Control: with every file readable there is no such note.
#[test]
fn readable_workflows_add_no_note_when_added_jobs_are_read() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(CI)),
            (".github/workflows/ci.yml", WORKFLOW_TWO_JOBS),
            (".github/workflows/other.yml", WORKFLOW_OTHER),
        ],
        "ci: workflows",
    );
    repo.write(".github/workflows/ci.yml", WORKFLOW_ONE_JOB);
    repo.commit("ci: drop the test job");
    let run = check(&repo, &[], &[]);
    assert!(
        !notes(&run, "ci-integrity").contains("could not be parsed"),
        "{}",
        all(&run)
    );
}

const GITLAB_PIPELINE: &str = "test:\n  script:\n    - cargo test\n";

/// A deleted pipeline whose base side does not parse: its verification jobs could not
/// be listed, and nothing said so.
#[test]
fn a_deleted_pipeline_whose_base_side_does_not_parse_is_named() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(CI)),
            (".gitlab-ci.yml", NOT_YAML),
        ],
        "ci: pipeline",
    );
    repo.remove(".gitlab-ci.yml");
    repo.commit("ci: remove the pipeline");
    let run = check(&repo, &[], &[]);
    let said = notes(&run, "ci-integrity");
    assert!(
        said.contains(
            ".gitlab-ci.yml: the base side of this deleted pipeline could not be parsed, so its verification jobs were not compared"
        ),
        "{said}\n{}",
        all(&run)
    );
}

/// Control: a deleted pipeline that parses is reported as before, without the note.
#[test]
fn a_deleted_pipeline_that_parses_is_reported_without_the_note() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &config(CI)),
            (".gitlab-ci.yml", GITLAB_PIPELINE),
        ],
        "ci: pipeline",
    );
    repo.remove(".gitlab-ci.yml");
    repo.commit("ci: remove the pipeline");
    let run = check(&repo, &[], &[]);
    assert!(
        !notes(&run, "ci-integrity").contains("could not be parsed"),
        "{}",
        all(&run)
    );
    assert_eq!(
        codes(&run, "ci-integrity"),
        ["ci-integrity/verification-workflow-deleted"],
        "{}",
        all(&run)
    );
}

// ---- baseline: a file outside the repository ----------------------------------------------

const NOT_TOML: &str = "this is = = not toml [\n";

/// The directory of a baseline outside the repository can be a home directory: only the
/// file name is printed.
#[test]
fn a_baseline_outside_the_repository_is_named_by_its_file_name() {
    let repo = Repo::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let dir = elsewhere.path().file_name().unwrap().to_str().unwrap();
    for (content, said) in [
        (Some(NOT_TOML), "failed to parse baseline file `known.toml`"),
        (None, "baseline file `known.toml` does not exist"),
    ] {
        let outside = elsewhere.path().join("known.toml");
        match content {
            Some(c) => std::fs::write(&outside, c).unwrap(),
            None => std::fs::remove_file(&outside).unwrap(),
        }
        let run = repo.check(&["--baseline-file", outside.to_str().unwrap()]);
        assert_eq!(run.code, 2, "{}", all(&run));
        assert_eq!(run.could_not_check(), ("baseline".to_string(), None));
        let d = detail(&run);
        assert!(d.contains(said), "{d}");
        assert!(!d.contains(dir), "{d}");
        assert!(!run.stderr.contains(dir), "{}", run.stderr);
    }
}

const ONE_ENTRY: &str = "version = 2\n\n[[findings]]\ngate = \"pii\"\nrule = \"home-directory-path\"\npath = \"docs/plan.md\"\nfingerprint = \"0123456789abcdef\"\n";

/// `config-integrity` compares the baseline's two sides. One outside the repository is on
/// neither side of the change: the read of its base side failed the gate (exit 2) with
/// the absolute path in the error. It is now named, by its file name, in a note.
#[test]
fn config_integrity_does_not_compare_a_baseline_outside_the_repository() {
    let repo = repo_unchanged("");
    let elsewhere = tempfile::tempdir().unwrap();
    let dir = elsewhere.path().file_name().unwrap().to_str().unwrap();
    let outside = elsewhere.path().join("known.toml");
    std::fs::write(&outside, ONE_ENTRY).unwrap();
    let run = repo.check(&["--baseline-file", outside.to_str().unwrap()]);
    assert_eq!(run.code, 0, "{}", all(&run));
    assert!(
        notes(&run, "config-integrity").contains(
            "the baseline in force (`known.toml`) is outside the repository, so this change cannot edit it; baseline growth was not compared"
        ),
        "{}",
        all(&run)
    );
    assert!(!run.stdout.contains(dir), "{}", run.stdout);
    assert!(!run.stderr.contains(dir), "{}", run.stderr);
}

/// A baseline inside the repository named by an absolute path is compared like one named
/// by its relative path, and reported under the relative path.
#[test]
fn config_integrity_reports_a_grown_baseline_named_by_an_absolute_path_under_its_relative_path() {
    let repo = repo_unchanged("");
    repo.write("policy/known.toml", ONE_ENTRY);
    repo.commit("chore: grandfather a finding");
    let inside = repo.file("policy/known.toml");
    let dir = repo.path().file_name().unwrap().to_str().unwrap();
    for given in [inside.to_str().unwrap(), "policy/known.toml"] {
        let run = repo.check(&["--baseline-file", given]);
        let found = run.violations("config-integrity");
        assert_eq!(found.len(), 1, "{given}: {}", all(&run));
        assert_eq!(found[0]["file"], "policy/known.toml", "{}", all(&run));
        assert_eq!(
            found[0]["code"],
            "config-integrity/baseline-increased",
            "{}",
            all(&run)
        );
        assert!(!run.stdout.contains(dir), "{}", run.stdout);
    }
}

/// `head_report` is what the count is read from, as `test_report` is: added where the
/// base had none it is a change of evidence. `base_report` names what the head report is
/// compared with, and adding it beside a head report both sides have is not.
#[test]
fn a_head_report_added_is_a_change_of_evidence_and_a_base_report_added_is_not() {
    const HEAD_REPORT: &str = "head_report = \"reports/junit.xml\"\n";
    const BASE_REPORT: &str = "base_report = \"reports/junit.xml\"\n";
    let repo = floor_repo(FLOOR, &format!("{FLOOR}{HEAD_REPORT}"));
    let run = check(&repo, &[], &[]);
    let said = messages(&run, "config-integrity");
    assert_eq!(codes(&run, "config-integrity"), [WEAKENED], "{}", all(&run));
    assert!(
        said[0].contains("[test-floor] `head_report` changed from unset"),
        "{said:?}"
    );

    let both = format!("{FLOOR}{HEAD_REPORT}");
    let repo = floor_repo(&both, &format!("{both}{BASE_REPORT}"));
    let run = check(&repo, &[], &[]);
    assert_eq!(
        codes(&run, "config-integrity"),
        Vec::<String>::new(),
        "{}",
        all(&run)
    );
}
