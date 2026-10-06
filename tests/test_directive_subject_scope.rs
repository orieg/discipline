//! A directive names its subject at the start of its text, and a word of its reason
//! never names one (#611).
//!
//! Each case has a finding whose subject is an ordinary word or path, and a directive of
//! the right kind written for another subject whose prose happens to contain it. The
//! finding stays reported; the documented form `<directive>: <subject> <reason>` lifts
//! it, before and after the change.

mod common;
use common::{FakeForge, Repo, Run};

const ARITH_BASE: &str =
    "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 3, 4);\n}\n";
const ARITH_WEAKENED: &str = "#[test]\nfn adds() {\n    let x = 1;\n    assert!(x + 1 == 2);\n}\n";

/// The pull request body line that hid the action fixture's finding: written for another
/// test, with the word `adds` in its reason.
const BODY_FOR_ANOTHER_TEST: &str = "allow-assertion-drop: test_assertj_and_custom_vocab the helper body left the fixture, so the configured helper adds to the total and not to the strong count";
const BODY_NAMING_ADDS: &str =
    "allow-assertion-drop: adds the second equality moved to the property suite";
const BODY_QUOTING_ADDS_FIRST: &str =
    "allow-assertion-drop: \"adds\" the second equality moved to the property suite";
const BODY_QUOTING_ADDS_LATER: &str =
    "allow-assertion-drop: test_assertj_and_custom_vocab keeps what \"adds\" used to check";
const BODY_ADDS_INSIDE_A_WORD: &str =
    "allow-assertion-drop: test_padds_up the name readds nothing and adds_up is another test";

const HANDLER_BASE: &str =
    "def load(p):\n    try:\n        return open(p).read()\n    except FileNotFoundError:\n        pass\n";
const HANDLER_SWALLOWS: &str = "def load(p):\n    try:\n        return open(p).read()\n    except FileNotFoundError:\n        pass\n    except OSError:\n        return None\n";
const BODY_SWALLOW_PROSE_NAMES_FILE: &str =
    "allow-swallow: pkg/io.py a missing cache file is the normal first run, as config.py already assumes";
const BODY_SWALLOW_PROSE_NAMES_DIR: &str =
    "allow-swallow: pkg/io.py a missing cache file is the normal first run for every loader in pkg/";
const BODY_SWALLOW_EACH_FILE: &str = "allow-swallow: pkg/io.py a missing cache file is the normal first run\nallow-swallow: config.py the defaults apply when the file is unreadable";

const CONFIG_BASE: &str = "[meta]\nversion = 1\nname = \"t\"\n";
const CONFIG_TWO_GATES_OFF: &str = "[meta]\nversion = 1\nname = \"t\"\n\n[gates.pii]\nenabled = false\n\n[gates.time-estimates]\nenabled = false\n";
const BODY_WEAKENING_PROSE_NAMES_GATE: &str = "allow-gate-weakening: time-estimates the plan documents come from a vendor and the pii gate stays under review";
const BODY_WEAKENING_EACH_GATE: &str = "allow-gate-weakening: time-estimates the plan documents come from a vendor\nallow-gate-weakening: pii the fixtures carry sample addresses";

const OLD_TEST: &str = "#[test]\nfn legacy() {\n    let x = 1;\n    assert_eq!(x, 1);\n}\n";
const BODY_REMOVES_PROSE_NAMES_DIR: &str =
    "removes: tests/legacy/old.rs the cases moved to another suite kept under tests/";
const BODY_REMOVES_PROSE_NAMES_FILE: &str =
    "removes: tests/legacy/old.rs the cases moved next to what a.rs covered";
const BODY_REMOVES_DIR_FIRST: &str = "removes: tests/ the suite moved to the property harness";
const BODY_REMOVES_FILE_NAME_FIRST: &str =
    "removes: old.rs the cases moved to the property harness\nremoves: tests/a.rs superseded by the same harness";

/// Subjects of the overrides `gate` applied.
fn lifted(run: &Run, gate: &str) -> Vec<String> {
    let mut subjects: Vec<String> = run.outcome(gate)["overrides"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|o| o["subject"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    subjects.sort();
    subjects
}

/// Files of the violations `gate` reported.
fn reported_files(run: &Run, gate: &str) -> Vec<String> {
    let mut files: Vec<String> = run
        .violations(gate)
        .iter()
        .map(|v| v["file"].as_str().unwrap_or("").to_string())
        .collect();
    files.sort();
    files.dedup();
    files
}

fn notes(run: &Run, gate: &str) -> Vec<String> {
    run.outcome(gate)["notes"]
        .as_array()
        .map(|a| a.iter().map(|n| n.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn unused(run: &Run) -> Vec<String> {
    run.json()["unused_directives"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|u| u["directive"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The action's negative-control fixture: one test, `adds`, with an equality weakened.
fn weakened_adds() -> Repo {
    let repo = Repo::new();
    repo.commit_base("tests/arith.rs", ARITH_BASE, "test: arithmetic");
    repo.write("tests/arith.rs", ARITH_WEAKENED);
    repo.commit("test: tidy");
    repo
}

fn two_new_swallows() -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[("pkg/io.py", HANDLER_BASE), ("pkg/config.py", HANDLER_BASE)],
        "feat: loaders",
    );
    repo.write("pkg/io.py", HANDLER_SWALLOWS);
    repo.write("pkg/config.py", HANDLER_SWALLOWS);
    repo.commit("fix: quiet the failures");
    repo
}

fn two_gates_switched_off() -> Repo {
    let repo = Repo::new();
    repo.commit_base("discipline.toml", CONFIG_BASE, "chore: configure");
    repo.write("discipline.toml", CONFIG_TWO_GATES_OFF);
    repo.commit("chore: tune");
    repo
}

fn two_test_files_deleted() -> Repo {
    let repo = Repo::new();
    repo.commit_base("tests/legacy/old.rs", OLD_TEST, "test: legacy");
    repo.git(&["rm", "-q", "tests/a.rs", "tests/legacy/old.rs"]);
    repo.commit("test: drop two files");
    repo
}

// ---- a test name ------------------------------------------------------------

#[test]
fn a_word_in_the_reason_does_not_lift_the_test_it_spells() {
    let repo = weakened_adds();
    let run = repo.check_with_pr(&[], BODY_FOR_ANOTHER_TEST);
    assert_eq!(
        lifted(&run, "assertion-reduction"),
        Vec::<String>::new(),
        "a directive written for another test lifted `adds`"
    );
    assert_eq!(
        reported_files(&run, "assertion-reduction"),
        vec!["tests/arith.rs"]
    );
    assert_eq!(run.code, 1);
}

#[test]
fn the_directive_that_no_longer_lifts_says_which_subject_it_names() {
    let repo = weakened_adds();
    let run = repo.check_with_pr(&[], BODY_FOR_ANOTHER_TEST);
    let said = notes(&run, "assertion-reduction");
    assert!(
        said.iter().any(|n| n.contains("allow-assertion-drop")
            && n.contains("`test_assertj_and_custom_vocab`")
            && n.contains("`adds`")
            && n.contains("start of the directive")),
        "no note names the subject the directive was read with: {said:?}"
    );
    // It lifted nothing, so it is listed as unused, by name.
    assert_eq!(unused(&run), vec!["allow-assertion-drop"]);
}

#[test]
fn a_subject_quoted_later_in_the_reason_does_not_lift() {
    let repo = weakened_adds();
    let run = repo.check_with_pr(&[], BODY_QUOTING_ADDS_LATER);
    assert_eq!(
        lifted(&run, "assertion-reduction"),
        Vec::<String>::new(),
        "a subject quoted inside the prose lifted `adds`"
    );
    assert_eq!(run.titles("assertion-reduction").len(), 1);
}

#[test]
fn a_subject_inside_a_longer_word_never_lifts_control() {
    let repo = weakened_adds();
    let run = repo.check_with_pr(&[], BODY_ADDS_INSIDE_A_WORD);
    assert_eq!(lifted(&run, "assertion-reduction"), Vec::<String>::new());
    assert_eq!(run.titles("assertion-reduction").len(), 1);
}

#[test]
fn the_test_named_first_is_lifted_control() {
    let repo = weakened_adds();
    let run = repo.check_with_pr(&[], BODY_NAMING_ADDS);
    assert_eq!(lifted(&run, "assertion-reduction"), vec!["adds"]);
    assert!(run.titles("assertion-reduction").is_empty());
    assert!(unused(&run).is_empty(), "{:?}", unused(&run));
    assert!(
        !notes(&run, "assertion-reduction")
            .iter()
            .any(|n| n.contains("start of the directive")),
        "a directive that lifted its subject needs no note"
    );
}

#[test]
fn the_test_quoted_first_is_lifted_control() {
    let repo = weakened_adds();
    let run = repo.check_with_pr(&[], BODY_QUOTING_ADDS_FIRST);
    assert_eq!(lifted(&run, "assertion-reduction"), vec!["adds"]);
    assert!(run.titles("assertion-reduction").is_empty());
}

const TOTALS_BASE: &str =
    "#[test]\nfn works() {\n    let x = 2;\n    assert_eq!(x + 1, 3);\n    assert_eq!(x + 3, 5);\n}\n";
const TOTALS_WEAKENED: &str =
    "#[test]\nfn works() {\n    let x = 2;\n    assert!(x + 1 == 3);\n}\n";
const BODY_FILE_LIFTED_AFTER_A_MENTION: &str = "allow-assertion-drop: test_other the helper adds to the total\nallow-assertion-drop: tests/arith.rs the second equality moved to the property suite";

#[test]
fn no_note_when_another_directive_lifts_the_finding_by_its_file() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("tests/arith.rs", ARITH_BASE),
            ("tests/totals.rs", TOTALS_BASE),
        ],
        "test: arithmetic",
    );
    repo.write("tests/arith.rs", ARITH_WEAKENED);
    repo.write("tests/totals.rs", TOTALS_WEAKENED);
    repo.commit("test: tidy");
    let run = repo.check_with_pr(&[], BODY_FILE_LIFTED_AFTER_A_MENTION);
    // `adds` is lifted through its file; `works` stays reported.
    assert_eq!(lifted(&run, "assertion-reduction"), vec!["tests/arith.rs"]);
    assert_eq!(
        reported_files(&run, "assertion-reduction"),
        vec!["tests/totals.rs"]
    );
    let said: Vec<String> = notes(&run, "assertion-reduction")
        .into_iter()
        .filter(|n| n.contains("start of the directive"))
        .collect();
    assert!(
        said.is_empty(),
        "a note about a finding that was lifted: {said:?}"
    );
}

// ---- a path -----------------------------------------------------------------

#[test]
fn a_file_name_in_the_reason_does_not_lift_that_file() {
    let repo = two_new_swallows();
    let run = repo.check_with_pr(&[], BODY_SWALLOW_PROSE_NAMES_FILE);
    assert_eq!(lifted(&run, "error-swallowing"), vec!["pkg/io.py"]);
    assert_eq!(
        reported_files(&run, "error-swallowing"),
        vec!["pkg/config.py"]
    );
    // The gate tried the path and then its file name: one note, naming the path.
    let said: Vec<String> = notes(&run, "error-swallowing")
        .into_iter()
        .filter(|n| n.contains("start of the directive"))
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("`pkg/io.py`") && said[0].contains("`pkg/config.py`"),
        "{said:?}"
    );
}

#[test]
fn a_directory_in_the_reason_does_not_lift_the_files_under_it() {
    let repo = two_new_swallows();
    let run = repo.check_with_pr(&[], BODY_SWALLOW_PROSE_NAMES_DIR);
    assert_eq!(lifted(&run, "error-swallowing"), vec!["pkg/io.py"]);
    assert_eq!(
        reported_files(&run, "error-swallowing"),
        vec!["pkg/config.py"]
    );
}

#[test]
fn a_path_or_its_file_name_written_first_is_lifted_control() {
    let repo = two_new_swallows();
    let run = repo.check_with_pr(&[], BODY_SWALLOW_EACH_FILE);
    assert_eq!(
        lifted(&run, "error-swallowing"),
        vec!["pkg/config.py", "pkg/io.py"]
    );
    assert!(run.titles("error-swallowing").is_empty());
}

// ---- a gate id --------------------------------------------------------------

#[test]
fn a_gate_id_in_the_reason_does_not_lift_that_gates_weakening() {
    let repo = two_gates_switched_off();
    let run = repo.check_with_pr(&[], BODY_WEAKENING_PROSE_NAMES_GATE);
    assert_eq!(lifted(&run, "config-integrity"), vec!["time-estimates"]);
    assert_eq!(run.titles("config-integrity").len(), 1);
    assert!(
        notes(&run, "config-integrity")
            .iter()
            .any(|n| n.contains("`time-estimates`") && n.contains("`pii`")),
        "{:?}",
        notes(&run, "config-integrity")
    );
}

#[test]
fn each_gate_id_written_first_is_lifted_control() {
    let repo = two_gates_switched_off();
    let run = repo.check_with_pr(&[], BODY_WEAKENING_EACH_GATE);
    assert_eq!(
        lifted(&run, "config-integrity"),
        vec!["pii", "time-estimates"]
    );
    assert!(run.titles("config-integrity").is_empty());
}

// ---- `removes:` -------------------------------------------------------------

#[test]
fn a_directory_in_a_removes_reason_does_not_lift_another_deletion() {
    let repo = two_test_files_deleted();
    let run = repo.check_with_pr(&[], BODY_REMOVES_PROSE_NAMES_DIR);
    assert_eq!(
        lifted(&run, "deletion-rationale"),
        vec!["tests/legacy/old.rs"]
    );
    assert_eq!(
        reported_files(&run, "deletion-rationale"),
        vec!["tests/a.rs"]
    );
}

#[test]
fn a_file_name_in_a_removes_reason_does_not_lift_another_deletion() {
    let repo = two_test_files_deleted();
    let run = repo.check_with_pr(&[], BODY_REMOVES_PROSE_NAMES_FILE);
    assert_eq!(
        lifted(&run, "deletion-rationale"),
        vec!["tests/legacy/old.rs"]
    );
    assert_eq!(
        reported_files(&run, "deletion-rationale"),
        vec!["tests/a.rs"]
    );
}

#[test]
fn a_directory_written_first_lifts_the_deletions_under_it_control() {
    let repo = two_test_files_deleted();
    let run = repo.check_with_pr(&[], BODY_REMOVES_DIR_FIRST);
    assert_eq!(
        lifted(&run, "deletion-rationale"),
        vec!["tests/a.rs", "tests/legacy/old.rs"]
    );
    assert!(run.titles("deletion-rationale").is_empty());
}

#[test]
fn a_file_name_written_first_lifts_that_deletion_control() {
    let repo = two_test_files_deleted();
    let run = repo.check_with_pr(&[], BODY_REMOVES_FILE_NAME_FIRST);
    assert_eq!(
        lifted(&run, "deletion-rationale"),
        vec!["tests/a.rs", "tests/legacy/old.rs"]
    );
    assert!(run.titles("deletion-rationale").is_empty());
}

// ---- a benchmark arm (`allow-regression` with a sourced override) --------------

const RUN_URL: &str = "https://github.com/acme/widgets/actions/runs/4401";
const RUN_API: &str = "repos/acme/widgets/actions/runs/4401";
const SOURCED: &str = "[gates.bench-regression]\nseverity = \"error\"\ntolerance_pct = 5.0\nrequire_sourced_override = true\n";
const ARMS_BASE: &str = r#"{"arms": {"sync_map_insert": 1000, "get": 1000}}"#;
const ARMS_BOTH_REGRESSED: &str = r#"{"arms": {"sync_map_insert": 1060, "get": 1060}}"#;
const ARMS_ONE_REGRESSED: &str = r#"{"arms": {"sync_map_insert": 1060, "get": 1000}}"#;
const BODY_REGRESSION_PROSE_NAMES_ARM: &str = "allow-regression: sync_map_insert trade measured in https://github.com/acme/widgets/actions/runs/4401 and the readers get slower with it";

/// `bench-regression` over two counter files, with a forge that vouches for the cited run.
fn sourced_bench_run(head_arms: &str, body: &str) -> Run {
    let repo = Repo::new();
    repo.commit("init");
    let head_sha = repo.git_output(&["rev-parse", "HEAD"]);
    let forge = FakeForge::start();
    forge.serve(
        RUN_API,
        serde_json::json!({"conclusion": "success", "head_sha": head_sha}),
    );
    forge.serve(
        &format!("repos/acme/widgets/compare/{head_sha}...{head_sha}"),
        serde_json::json!({"status": "identical"}),
    );
    let base = repo.file("base_bench.json");
    let head = repo.file("head_bench.json");
    std::fs::write(&base, ARMS_BASE).unwrap();
    std::fs::write(&head, head_arms).unwrap();
    repo.run(
        &[
            "check",
            "--format",
            "json",
            "--base",
            "main",
            "--suite",
            "bench",
            "--bench-base-file",
            base.to_str().unwrap(),
            "--bench-head-file",
            head.to_str().unwrap(),
            "--config-override",
            SOURCED,
        ],
        &[
            ("PR_BODY", body),
            ("DISCIPLINE_FORGE_API_URL", forge.url().as_str()),
        ],
    )
}

#[cfg(unix)]
#[test]
fn an_arm_in_a_regression_reason_is_not_approved_by_it() {
    let run = sourced_bench_run(ARMS_BOTH_REGRESSED, BODY_REGRESSION_PROSE_NAMES_ARM);
    assert!(RUN_URL.ends_with("4401"));
    assert_eq!(lifted(&run, "bench-regression"), vec!["sync_map_insert"]);
    let messages: Vec<String> = run
        .violations("bench-regression")
        .iter()
        .map(|v| v["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("`get`"), "{messages:?}");
    assert_eq!(run.code, 1);
}

#[cfg(unix)]
#[test]
fn the_arm_written_first_in_a_regression_reason_is_approved_control() {
    let run = sourced_bench_run(ARMS_ONE_REGRESSED, BODY_REGRESSION_PROSE_NAMES_ARM);
    assert_eq!(lifted(&run, "bench-regression"), vec!["sync_map_insert"]);
    assert!(run.titles("bench-regression").is_empty());
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
}
