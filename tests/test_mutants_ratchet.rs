//! `scripts/mutants_ratchet.py` judges the scheduled mutation run
//! (`.github/workflows/mutants.yml`, `docs/ARCHITECTURE.md` §8.7): it reads every shard's
//! cargo-mutants `outcomes.json`, counts the surviving mutants per file and compares the
//! counts with the committed ratchet. Each case runs the script over the fixtures under
//! `tests/fixtures/mutants/`: two finished shards with three survivors (two in
//! `src/ast/bounds.rs`, one in `src/guards/ci_skip_set.rs`), a timeout and an unviable
//! mutant, and one ratchet per verdict.

mod common;

use std::path::{Path, PathBuf};

/// Exit codes of the script: pass, a file above the ratchet, data that could not be read.
const PASS: i32 = 0;
const ABOVE: i32 = 1;
const UNREADABLE: i32 = 2;

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
    /// What the script appended to `--summary`, the job summary in CI.
    summary: String,
    /// What it wrote to `--write-ratchet`, if anything.
    computed: Option<String>,
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(rel: &str) -> PathBuf {
    root().join("tests/fixtures/mutants").join(rel)
}

/// The script over the outcomes in `outcomes` (relative to the fixtures), expecting
/// `shards` shards, against the fixture ratchet `ratchet_<name>.json`.
fn ratchet(outcomes: &str, shards: u32, name: &str, extra: &[&str]) -> Outcome {
    let tmp = tempfile::tempdir().unwrap();
    let summary = tmp.path().join("summary.md");
    let computed = tmp.path().join("computed.json");
    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--outcomes-dir")
        .arg(fixture(outcomes))
        .arg("--shards")
        .arg(shards.to_string())
        .arg("--ratchet")
        .arg(fixture(&format!("ratchet_{name}.json")))
        .arg("--summary")
        .arg(&summary)
        .arg("--write-ratchet")
        .arg(&computed)
        .args(extra)
        .output()
        .expect("run scripts/mutants_ratchet.py");
    Outcome {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        summary: std::fs::read_to_string(&summary).unwrap_or_default(),
        computed: std::fs::read_to_string(&computed).ok(),
    }
}

/// `src/ast/bounds.rs` has two survivors against a ratchet of one: the run fails, and
/// the summary names the file and lists each survivor with its line and mutation.
/// Killed mutant: the survivor list left out of the summary.
#[test]
fn a_file_above_its_ratchet_fails_and_lists_every_survivor() {
    let run = ratchet("run", 2, "above", &[]);
    assert_eq!(run.code, ABOVE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary
            .contains("### Above the ratchet\n\n| File | Missed | Ratchet |\n|---|---|---|\n| `src/ast/bounds.rs` | 2 | 1 |\n"),
        "{}",
        run.summary
    );
    for survivor in [
        "- `src/ast/bounds.rs:41`: replace within_limit -> bool with true",
        "- `src/ast/bounds.rs:47`: replace <= with > in within_limit",
        "- `src/guards/ci_skip_set.rs:88`: delete ! in is_skipped",
    ] {
        assert!(
            run.summary.contains(survivor),
            "{survivor}:\n{}",
            run.summary
        );
    }
    // A timed-out mutant is listed apart and not counted as surviving.
    assert!(
        run.summary
            .contains("### Timed out (1, not counted as surviving)"),
        "{}",
        run.summary
    );
    // The report goes to standard output as well as the summary file.
    assert_eq!(run.stdout, run.summary);
}

/// Counts equal to the ratchet pass, with nothing to lower.
/// Killed mutant: the comparison `found > allowed` turned into `found >= allowed`.
#[test]
fn counts_equal_to_the_ratchet_pass() {
    let run = ratchet("run", 2, "equal", &["--expect-version", "27.1.0"]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("**Passed:**"), "{}", run.summary);
    assert!(!run.summary.contains("can lower"), "{}", run.summary);
}

/// A count below the ratchet passes and names the file whose count can be lowered.
/// Killed mutant: a lower count reported as above.
#[test]
fn a_count_below_the_ratchet_passes_and_says_it_can_lower() {
    let run = ratchet("run", 2, "lower", &[]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary.contains("### Below the ratchet (can lower)")
            && run.summary.contains("| `src/ast/bounds.rs` | 2 | 3 |"),
        "{}",
        run.summary
    );
}

/// A file the ratchet does not list counts as zero: its one survivor fails the run.
#[test]
fn a_file_absent_from_the_ratchet_counts_as_zero() {
    let run = ratchet("run", 2, "absent_file", &[]);
    assert_eq!(run.code, ABOVE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary
            .contains("| `src/guards/ci_skip_set.rs` | 1 | 0 |"),
        "{}",
        run.summary
    );
}

/// `"bootstrap": true` passes whatever the counts and prints the ratchet to commit, and
/// writes it to `--write-ratchet`.
/// Killed mutant: bootstrap mode compared like a ratchet of zeros (exit 1).
#[test]
fn bootstrap_passes_and_prints_the_computed_ratchet() {
    let run = ratchet("run", 2, "bootstrap", &[]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("**Bootstrap:**"), "{}", run.summary);
    let computed: serde_json::Value = serde_json::from_str(
        run.computed
            .as_deref()
            .expect("the computed ratchet is written"),
    )
    .unwrap();
    assert_eq!(
        computed["missed"],
        serde_json::json!({"src/ast/bounds.rs": 2, "src/guards/ci_skip_set.rs": 1})
    );
    assert!(computed.get("bootstrap").is_none(), "{computed}");
    assert!(
        run.summary.contains("\"src/ast/bounds.rs\": 2"),
        "{}",
        run.summary
    );
}

/// The run expects three shards and the third left no outcomes: the data is incomplete,
/// so the run fails closed even in bootstrap mode, and reads no missing shard as zero.
/// Killed mutant: a shard whose `outcomes.json` cannot be read skipped instead of reported.
#[test]
fn a_missing_shard_fails_closed() {
    let run = ratchet("run", 3, "bootstrap", &[]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary.contains("**Could not check:**") && run.summary.contains("shard 2: "),
        "{}",
        run.summary
    );
    assert!(
        run.computed.is_none(),
        "no ratchet is computed from partial data"
    );
}

/// A shard whose run was cut off (`end_time` is null, as cargo-mutants leaves it until
/// the run finishes) fails closed: its counts are partial.
/// Killed mutant: the `end_time` check removed.
#[test]
fn an_unfinished_shard_fails_closed() {
    let run = ratchet("unfinished", 2, "bootstrap", &[]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("did not finish"), "{}", run.summary);
}

/// A shard job that did not succeed fails the comparison whatever its files hold, and so
/// does an outcome written by another cargo-mutants version than the pinned one.
#[test]
fn a_failed_shard_job_or_another_version_fails_closed() {
    let run = ratchet("run", 2, "equal", &["--shards-result", "failure"]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("ended `failure`"), "{}", run.summary);

    let run = ratchet("run", 2, "equal", &["--expect-version", "26.0.0"]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("expected 26.0.0"), "{}", run.summary);
}

/// The script's own embedded cases pass, and the committed ratchet parses.
#[test]
fn self_check_passes_and_the_committed_ratchet_is_valid() {
    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--self-check")
        .output()
        .expect("run scripts/mutants_ratchet.py --self-check");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--validate")
        .arg(root().join("tests/fixtures/mutants_ratchet.json"))
        .output()
        .expect("run scripts/mutants_ratchet.py --validate");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
