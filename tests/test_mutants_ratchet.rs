//! `scripts/mutants_ratchet.py` plans and judges the scheduled mutation run
//! (`.github/workflows/mutants.yml`, `docs/ARCHITECTURE.md` §8.7). Each run tests one
//! slice of the files, picked by the week the run was created in, plus the mutants in
//! the code changed since the ratchet's previous measured commit (the diff slice).
//!
//! Judging fixtures under `tests/fixtures/mutants/`: `plan.json` puts
//! `src/ast/bounds.rs` in the slice (shard 0, three mutants, two survivors) and
//! `src/guards/ci_skip_set.rs` in the diff slice (shard 1, one survivor, a timeout and an
//! unviable mutant); `src/guards/untouched.rs` is in neither. One ratchet per verdict.
//! Planning fixtures: `list/all.json` (five files holding 5, 4, 3, 2 and 1 mutants),
//! `list/diff.json` (two mutants in `alpha.rs`, one in `gamma.rs`) and `ratchet_plan*.json`
//! (three slices, `gamma.rs` already in slice 2).

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// Exit codes of the script: pass, a file above the ratchet, data that could not be read.
const PASS: i32 = 0;
const ABOVE: i32 = 1;
const UNREADABLE: i32 = 2;

/// The `previous_commit` of every fixture ratchet but `ratchet_plan_no_previous.json`.
const PREVIOUS: &str = "c0ffee0000000000000000000000000000000000";
/// The commit the judging cases record as this run's.
const HEAD: &str = "beef000000000000000000000000000000000000";

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
    /// What the script appended to `--summary`, the job summary in CI.
    summary: String,
    /// What it wrote to `--write-ratchet`, if anything.
    computed: Option<Value>,
    /// What `--plan` wrote to `--plan-out`, if anything.
    plan: Option<Value>,
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(rel: &str) -> PathBuf {
    root().join("tests/fixtures/mutants").join(rel)
}

/// The script with `args`, writing its report, computed ratchet and plan under `tmp`.
fn command(tmp: &Path, args: &[OsString]) -> Command {
    let mut cmd = common::script_command("python3");
    cmd.arg(root().join("scripts/mutants_ratchet.py"))
        .args(args)
        .arg("--summary")
        .arg(tmp.join("summary.md"))
        .arg("--write-ratchet")
        .arg(tmp.join("computed.json"))
        .arg("--plan-out")
        .arg(tmp.join("plan"));
    cmd
}

fn collect(tmp: &Path, out: Output) -> Outcome {
    let read_json = |p: PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| serde_json::from_str::<Value>(&s).unwrap())
    };
    Outcome {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        summary: std::fs::read_to_string(tmp.join("summary.md")).unwrap_or_default(),
        computed: read_json(tmp.join("computed.json")),
        plan: read_json(tmp.join("plan/plan.json")),
    }
}

/// The script judging the outcomes in `outcomes` (relative to the fixtures) against the
/// fixture plan `plan` and the fixture ratchet `ratchet_<name>.json`, as run 200 at
/// commit `HEAD`.
fn ratchet(outcomes: &str, plan: &str, name: &str, extra: &[&str]) -> Outcome {
    let tmp = tempfile::tempdir().unwrap();
    let mut args: Vec<OsString> = vec![
        "--outcomes-dir".into(),
        fixture(outcomes).into(),
        "--plan-file".into(),
        fixture(plan).into(),
        "--ratchet".into(),
        fixture(&format!("ratchet_{name}.json")).into(),
        "--run-id".into(),
        "200".into(),
        "--commit".into(),
        HEAD.into(),
    ];
    args.extend(extra.iter().map(OsString::from));
    let out = command(tmp.path(), &args)
        .output()
        .expect("run scripts/mutants_ratchet.py");
    collect(tmp.path(), out)
}

/// `--plan` over `list/all.json` with `ratchet_<name>.json`, two shards.
fn plan(name: &str, extra: &[&str]) -> Outcome {
    let tmp = tempfile::tempdir().unwrap();
    let mut args: Vec<OsString> = vec![
        "--plan".into(),
        "--mutants-list".into(),
        fixture("list/all.json").into(),
        "--ratchet".into(),
        fixture(&format!("ratchet_{name}.json")).into(),
        "--shards".into(),
        "2".into(),
    ];
    args.extend(extra.iter().map(OsString::from));
    let out = command(tmp.path(), &args)
        .output()
        .expect("run scripts/mutants_ratchet.py --plan");
    collect(tmp.path(), out)
}

fn entry(computed: &Value, file: &str) -> Value {
    computed["files"][file].clone()
}

fn slice_files(plan: &Value) -> Vec<String> {
    plan["slice_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------
// Judging

/// `src/ast/bounds.rs` has two survivors against a ratchet of one: the run fails, and
/// the summary names the file and lists each survivor with its line and mutation.
/// Killed mutant: the survivor list left out of the summary.
#[test]
fn a_slice_file_above_its_ratchet_fails_and_lists_every_survivor() {
    let run = ratchet("run", "plan.json", "above", &[]);
    assert_eq!(run.code, ABOVE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary.contains(
            "### Above the ratchet\n\n| File | Missed | Ratchet | Measured by |\n|---|---|---|---|\n| `src/ast/bounds.rs` | 2 | 1 | slice |\n"
        ),
        "{}",
        run.summary
    );
    for survivor in [
        "- `src/ast/bounds.rs:41`: replace within_limit -> bool with true",
        "- `src/ast/bounds.rs:47`: replace <= with > in within_limit",
        "- `src/guards/ci_skip_set.rs:88`: delete ! in is_skipped (diff slice)",
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

/// Counts equal to the ratchet pass, with nothing to lower; a count below it passes and
/// names the file whose count can be lowered.
/// Killed mutants: `found > allowed` turned into `found >= allowed`; a lower count
/// reported as above.
#[test]
fn counts_equal_to_or_below_the_ratchet_pass() {
    let run = ratchet("run", "plan.json", "equal", &["--expect-version", "27.1.0"]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("**Passed:**"), "{}", run.summary);
    assert!(!run.summary.contains("can lower"), "{}", run.summary);

    let run = ratchet("run", "plan.json", "lower", &[]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary.contains("### Below the ratchet (can lower)")
            && run.summary.contains("| `src/ast/bounds.rs` | 2 | 3 |"),
        "{}",
        run.summary
    );
}

/// The computed ratchet gives the slice's file its new count and this run's id, and
/// leaves every other file as it was: `ci_skip_set.rs` keeps its count of 0 although the
/// diff slice found one survivor in it, and `untouched.rs` keeps its count and run id.
/// The run's commit becomes the previous measured commit.
/// Killed mutant: every file in the assignment updated, not only the slice's.
#[test]
fn a_run_updates_only_its_slice_files() {
    let run = ratchet("run", "plan.json", "lower", &[]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    let computed = run.computed.expect("the computed ratchet is written");
    // Positive control: the slice's file is updated.
    assert_eq!(
        entry(&computed, "src/ast/bounds.rs"),
        json!({"slice": 0, "missed": 2, "run_id": 200})
    );
    // Negative controls: a file in the diff slice and a file in neither keep their entries.
    assert_eq!(
        entry(&computed, "src/guards/ci_skip_set.rs"),
        json!({"slice": 1, "missed": 1, "run_id": 100})
    );
    assert_eq!(
        entry(&computed, "src/guards/untouched.rs"),
        json!({"slice": 1, "missed": 4, "run_id": 100})
    );
    assert_eq!(computed["previous_commit"], json!(HEAD));
    assert_eq!(computed["slices"], json!(2));
}

/// A file the ratchet has not measured yet cannot fail the run, whether it is listed
/// with `missed: null` (`ratchet_bootstrap.json`, nothing measured) or not listed at all
/// (`ratchet_absent_file.json`): `bounds.rs`'s two survivors are its first count, and the
/// diff slice's survivor in `ci_skip_set.rs` is listed, not compared. The same survivors
/// against counts of 0 fail.
/// Killed mutant: an unmeasured file compared as a count of 0.
#[test]
fn a_file_not_measured_yet_does_not_fail_the_run() {
    for (name, bootstrap) in [
        (
            "bootstrap",
            "**Bootstrap:** 1 of 3 file(s) measured; slice(s) not run yet: 1.",
        ),
        (
            "absent_file",
            "**Bootstrap:** 2 of 3 file(s) measured; slice(s) not run yet: 1.",
        ),
    ] {
        let run = ratchet("run", "plan.json", name, &[]);
        assert_eq!(run.code, PASS, "{name}: {}{}", run.stdout, run.stderr);
        assert!(
            run.summary.contains("### Measured for the first time")
                && run.summary.contains("| `src/ast/bounds.rs` | 2 |"),
            "{name}: {}",
            run.summary
        );
        assert!(
            run.summary
                .contains("### Diff-slice survivors in files not measured yet")
                && run.summary.contains("| `src/guards/ci_skip_set.rs` | 1 |"),
            "{name}: {}",
            run.summary
        );
        // `ci_skip_set.rs` is in slice 1, which has not run: the bootstrap is not complete.
        assert!(run.summary.contains(bootstrap), "{name}: {}", run.summary);
        let computed = run.computed.expect("the computed ratchet is written");
        assert_eq!(
            entry(&computed, "src/ast/bounds.rs"),
            json!({"slice": 0, "missed": 2, "run_id": 200}),
            "{name}"
        );
        // The unlisted file takes the plan's slice, still not measured.
        assert_eq!(
            entry(&computed, "src/guards/ci_skip_set.rs"),
            json!({"slice": 1, "missed": null, "run_id": null}),
            "{name}"
        );
    }

    // Positive control: the same survivors against measured counts of 0.
    let run = ratchet("run", "plan.json", "zero", &[]);
    assert_eq!(run.code, ABOVE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary
            .contains("| `src/ast/bounds.rs` | 2 | 0 | slice |")
            && run
                .summary
                .contains("| `src/guards/ci_skip_set.rs` | 1 | 0 | diff |"),
        "{}",
        run.summary
    );
}

/// The diff slice measures part of a file, so its survivors are a floor: one survivor in
/// the changed code of `ci_skip_set.rs` fails against a count of 0, and passes against
/// a count of 1.
/// Killed mutant: diff-slice survivors never compared.
#[test]
fn diff_slice_survivors_above_the_count_fail() {
    let run = ratchet("run", "plan.json", "diff_above", &[]);
    assert_eq!(run.code, ABOVE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary
            .contains("| `src/guards/ci_skip_set.rs` | 1 | 0 | diff |"),
        "{}",
        run.summary
    );
    // The diff slice never writes its partial count.
    let computed = run.computed.expect("the computed ratchet is written");
    assert_eq!(
        entry(&computed, "src/guards/ci_skip_set.rs"),
        json!({"slice": 1, "missed": 0, "run_id": 100})
    );

    let run = ratchet("run", "plan.json", "equal", &[]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
}

/// The plan expects a third shard with one diff mutant, and it left no outcomes: the
/// data is incomplete, so the run fails closed even though every file it holds is
/// measured, and computes no ratchet. With the plan that expects two shards, the same
/// outcomes pass.
/// Killed mutant: a shard whose `outcomes.json` cannot be read skipped instead of reported.
#[test]
fn a_missing_shard_fails_closed() {
    let run = ratchet("run", "plan_three_shards.json", "bootstrap", &[]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary.contains("**Could not check:**") && run.summary.contains("shard 2 (diff): "),
        "{}",
        run.summary
    );
    assert!(
        run.computed.is_none(),
        "no ratchet is computed from partial data"
    );
    assert_eq!(ratchet("run", "plan.json", "bootstrap", &[]).code, PASS);
}

/// A shard whose run was cut off (`end_time` is null, as cargo-mutants leaves it until
/// the run finishes) fails closed, even against a ratchet whose files are not measured
/// yet: the bootstrap rule never covers an unfinished shard. The same plan over finished
/// outcomes reads them.
/// Killed mutant: the `end_time` check removed.
#[test]
fn an_unfinished_shard_fails_closed() {
    let run = ratchet("unfinished", "plan_unfinished.json", "bootstrap", &[]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("did not finish"), "{}", run.summary);
    assert!(run.computed.is_none());

    // Negative control: finished outcomes with the plan's counts.
    let run = ratchet("run", "plan.json", "bootstrap", &[]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
}

/// A shard whose outcomes hold another number of mutants than the plan expects fails
/// closed: `plan_unfinished.json` expects one diff mutant on shard 1, and the finished
/// outcomes hold three.
#[test]
fn a_shard_with_another_count_than_the_plan_fails_closed() {
    let run = ratchet("run", "plan_unfinished.json", "bootstrap", &[]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(
        run.summary.contains("3 mutants, the plan expects 1"),
        "{}",
        run.summary
    );
}

/// A shard job that did not succeed fails the comparison whatever its files hold, and so
/// does an outcome written by another cargo-mutants version than the pinned one.
#[test]
fn a_failed_shard_job_or_another_version_fails_closed() {
    let run = ratchet("run", "plan.json", "equal", &["--shards-result", "failure"]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("ended `failure`"), "{}", run.summary);

    let run = ratchet("run", "plan.json", "equal", &["--expect-version", "26.0.0"]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("expected 26.0.0"), "{}", run.summary);
}

// ---------------------------------------------------------------------------------
// Planning

/// Three consecutive weeks walk the three slices: each file of the scope is in exactly
/// one of them. `gamma.rs` keeps the slice the ratchet gives it (2); the others are
/// placed largest first on the lightest slice. `removed.rs`, in the ratchet but no
/// longer in scope, is in no slice. A fourth week starts the rotation again, and two
/// times in the same week (Monday 00:00 and Sunday 23:59 UTC) pick the same slice.
/// The ratchet has no previous commit, so no diff list is needed.
/// Killed mutants: the slice taken from the day instead of the week; a file placed on
/// every slice; the ratchet's slice ignored.
#[test]
fn slice_selection_is_deterministic_and_covers_every_file_once_per_rotation() {
    let weeks = [
        "2026-10-31T03:31:00Z",
        "2026-11-07T03:31:00Z",
        "2026-11-14T03:31:00Z",
    ];
    let mut seen: Vec<String> = Vec::new();
    let mut slices: Vec<u64> = Vec::new();
    for created in weeks {
        let run = plan(
            "plan_no_previous",
            &["--run-created", created, "--slice", ""],
        );
        assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
        let p = run.plan.expect("the plan is written");
        slices.push(p["slice"].as_u64().unwrap());
        seen.extend(slice_files(&p));
        assert_eq!(
            p["assignment"],
            json!({
                "src/ast/alpha.rs": 0,
                "src/ast/beta.rs": 1,
                "src/guards/delta.rs": 2,
                "src/guards/epsilon.rs": 1,
                "src/guards/gamma.rs": 2,
            })
        );
    }
    assert_eq!(slices, [0, 1, 2]);
    seen.sort();
    assert_eq!(
        seen,
        [
            "src/ast/alpha.rs",
            "src/ast/beta.rs",
            "src/guards/delta.rs",
            "src/guards/epsilon.rs",
            "src/guards/gamma.rs",
        ],
        "every file once across the rotation"
    );

    // The fourth week tests the first week's slice again.
    let again = plan(
        "plan_no_previous",
        &["--run-created", "2026-11-21T03:31:00Z"],
    );
    assert_eq!(again.plan.unwrap()["slice"], json!(0));

    // The same week, early and late, gives the same plan; the next week does not.
    let monday = plan(
        "plan_no_previous",
        &["--run-created", "2026-10-26T00:00:00Z"],
    );
    let sunday = plan(
        "plan_no_previous",
        &["--run-created", "2026-11-01T23:59:59Z"],
    );
    let next = plan(
        "plan_no_previous",
        &["--run-created", "2026-11-02T00:00:00Z"],
    );
    assert_eq!(monday.plan, sunday.plan);
    assert_ne!(sunday.plan.unwrap()["slice"], next.plan.unwrap()["slice"]);
    // The plan's report lists the slice's files with their mutants.
    assert!(
        monday.summary.contains("| `src/ast/alpha.rs` | 5 |"),
        "{}",
        monday.summary
    );
}

/// A dispatched run may name the slice; one outside the ratchet's slices is refused.
/// (These plans use the ratchet without a previous commit, so they need no diff list.)
#[test]
fn a_named_slice_replaces_the_week_and_is_range_checked() {
    let run = plan("plan_no_previous", &["--slice", "2"]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    let p = run.plan.unwrap();
    assert_eq!(p["slice"], json!(2));
    assert_eq!(p["week"], Value::Null);
    assert_eq!(
        slice_files(&p),
        ["src/guards/delta.rs", "src/guards/gamma.rs"]
    );

    let run = plan("plan_no_previous", &["--slice", "3"]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.plan.is_none());
}

/// The ratchet's `previous_commit` is the diff slice's base: `--print-previous-commit`
/// gives it to the workflow's `git diff`, and the plan counts the diff's mutants outside
/// the slice (the one in `gamma.rs`; the two in `alpha.rs` are in slice 0, measured
/// whole). Without a previous commit the diff slice is skipped with a note, whatever
/// diff list is given.
/// Killed mutants: the diff slice run without a previous commit; slice files counted
/// in the diff slice.
#[test]
fn the_stored_previous_commit_drives_the_diff_slice() {
    let diff = fixture("list/diff.json");
    let diff = diff.to_str().unwrap();

    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--print-previous-commit")
        .arg(fixture("ratchet_plan.json"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("{PREVIOUS}\n")
    );
    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--print-previous-commit")
        .arg(fixture("ratchet_plan_no_previous.json"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "\n");

    // Positive: a previous commit, so the diff slice runs on the mutants outside the slice.
    let run = plan("plan", &["--slice", "0", "--diff-list", diff]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    let p = run.plan.unwrap();
    assert_eq!(
        p["diff"],
        json!({"base": PREVIOUS, "run": true, "mutants": 1, "note": null})
    );
    assert_eq!(
        p["expect"],
        json!([{"slice": 3, "diff": 1}, {"slice": 2, "diff": 0}])
    );
    assert!(
        run.summary
            .contains(&format!("Diff slice since `{PREVIOUS}`: 1 mutants")),
        "{}",
        run.summary
    );

    // Negative: no previous commit, so no diff slice, with a note.
    let run = plan("plan_no_previous", &["--slice", "0", "--diff-list", diff]);
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    let p = run.plan.unwrap();
    assert_eq!(p["diff"]["run"], json!(false));
    assert_eq!(p["diff"]["base"], Value::Null);
    assert_eq!(p["expect"][0]["diff"], json!(0));
    assert!(
        run.summary
            .contains("Diff slice skipped: no previous measured commit"),
        "{}",
        run.summary
    );

    // A previous commit with no diff list is missing data, not an empty diff.
    let run = plan("plan", &["--slice", "0"]);
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);

    // An empty list (cargo-mutants prints nothing for a diff that touches no mutant).
    let empty = fixture("list/diff_empty.json");
    let run = plan(
        "plan",
        &["--slice", "0", "--diff-list", empty.to_str().unwrap()],
    );
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    let p = run.plan.unwrap();
    assert_eq!(p["diff"]["run"], json!(false));
    assert!(
        p["diff"]["note"]
            .as_str()
            .unwrap()
            .contains("holds no mutant outside the slice"),
        "{p}"
    );
}

/// A shard's share must fit its capacity: slice 0 (five mutants over two shards, three
/// on shard 0) fits a capacity of 3 but leaves no room for the diff slice, which is then
/// skipped with a note; a capacity of 2 cannot hold the slice, and the plan fails.
#[test]
fn the_plan_checks_a_shard_capacity() {
    let diff = fixture("list/diff.json");
    let diff = diff.to_str().unwrap();
    let run = plan(
        "plan",
        &[
            "--slice",
            "0",
            "--diff-list",
            diff,
            "--per-shard-capacity",
            "3",
        ],
    );
    assert_eq!(run.code, PASS, "{}{}", run.stdout, run.stderr);
    let p = run.plan.unwrap();
    assert_eq!(p["diff"]["run"], json!(false));
    assert!(
        p["diff"]["note"]
            .as_str()
            .unwrap()
            .contains("more than the 0 left per shard"),
        "{p}"
    );

    let run = plan(
        "plan",
        &[
            "--slice",
            "0",
            "--diff-list",
            diff,
            "--per-shard-capacity",
            "2",
        ],
    );
    assert_eq!(run.code, UNREADABLE, "{}{}", run.stdout, run.stderr);
    assert!(run.summary.contains("raise `slices`"), "{}", run.summary);
}

/// The slice count is derived, not chosen: about 20,500 mutants at about 150 s each,
/// a 192 s baseline per cargo-mutants run (two per shard), 16 shards and the 320-minute
/// step, less 10% for variance and 25% for the diff slice, give 112 mutants per shard
/// and 16 slices. The workflow's limits and the committed ratchet's `slices` agree with
/// the script. The full sweep needs 1,282 mutants per shard, eleven times a shard's
/// capacity.
#[test]
fn the_slice_count_is_derived_from_the_measured_cost_and_the_workflow_limits() {
    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--sizing")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s: Value = serde_json::from_slice(&out.stdout).unwrap();
    for (key, want) in [
        ("step_seconds", 19_200),
        ("budget_seconds", 18_816),
        ("usable_seconds", 16_934),
        ("per_shard_capacity", 112),
        ("slice_per_shard", 84),
        ("slice_capacity", 1_344),
        ("slices", 16),
        ("mean_slice_mutants", 1_282),
        ("mean_slice_per_shard", 81),
        ("run_capacity", 1_792),
        ("full_sweep_per_shard", 1_282),
    ] {
        assert_eq!(s[key], json!(want), "{key}: {s}");
    }
    // The job fits the hosted runner's limit, and the step fits the job.
    assert!(s["step_timeout_minutes"].as_u64() < s["job_timeout_minutes"].as_u64());
    assert!(s["job_timeout_minutes"].as_u64() <= s["hosted_job_limit_minutes"].as_u64());

    let workflow = std::fs::read_to_string(root().join(".github/workflows/mutants.yml")).unwrap();
    assert!(
        workflow.contains(&format!("MUTANTS_SHARDS: {}", s["shards"])),
        "MUTANTS_SHARDS"
    );
    let matrix: Vec<String> = (0..s["shards"].as_u64().unwrap())
        .map(|k| k.to_string())
        .collect();
    assert!(
        workflow.contains(&format!("shard: [{}]", matrix.join(", "))),
        "the shard matrix"
    );
    assert!(
        workflow.contains(&format!(
            "- name: Test the shard's mutants\n        timeout-minutes: {}\n",
            s["step_timeout_minutes"]
        )),
        "the step's timeout-minutes"
    );
    assert!(
        workflow.contains(&format!(
            "runs-on: ubuntu-latest\n    timeout-minutes: {}\n",
            s["job_timeout_minutes"]
        )),
        "the shard job's timeout-minutes"
    );
    let committed: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/fixtures/mutants_ratchet.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(committed["slices"], s["slices"]);
}

/// The script's own embedded cases pass, and the committed ratchet parses; a ratchet in
/// the format before slices (`bootstrap`, top-level `missed`) is refused.
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
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old.json");
    std::fs::write(&old, r#"{"bootstrap": true, "missed": {}}"#).unwrap();
    let out = common::script_command("python3")
        .arg(root().join("scripts/mutants_ratchet.py"))
        .arg("--validate")
        .arg(&old)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(UNREADABLE));
}
