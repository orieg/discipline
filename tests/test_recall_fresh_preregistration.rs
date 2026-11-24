//! The fresh-instance recall re-measurement (#838,
//! `docs/benchmarks/recall-fresh-preregistration.toml`, status `draft`): the study script's
//! self-checks run, the committed population scan is disjoint from v1's instances, and the
//! Step-0 values the pre-registration records are the ones the Rust bound functions and
//! their Python mirrors give. Three lock preconditions are checked against committed files:
//! the harness dataset-loading patch and its sha256, v1's confirmatory units by
//! `injection_shape` (`scripts/recall_strata.py --v1-units-injection`, from the committed
//! per-unit file and replay), and the reused-unit baseline R_wf.

mod common;

use discipline::guards::perf::bounds::{
    minimum_detectable_difference, sample_size_for_interval, trajectories_required,
    wilson_pass_probability, wilson_score_interval,
};
use discipline::report::gitlab::sha256_hex;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn prereg() -> toml::Value {
    let text =
        std::fs::read_to_string(root().join("docs/benchmarks/recall-fresh-preregistration.toml"))
            .expect("read pre-registration");
    toml::from_str(&text).expect("parse pre-registration")
}

fn population() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            root().join("docs/benchmarks/results-recall-fresh/population.json"),
        )
        .expect("read population.json"),
    )
    .expect("parse population.json")
}

fn ids(path: &str) -> HashSet<String> {
    std::fs::read_to_string(root().join(path))
        .expect("read id list")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn int(v: &toml::Value, key: &str) -> i64 {
    v[key]
        .as_integer()
        .unwrap_or_else(|| panic!("{key} is not an integer"))
}

fn float(v: &toml::Value, key: &str) -> f64 {
    v[key]
        .as_float()
        .unwrap_or_else(|| panic!("{key} is not a float"))
}

/// The rate a `[a, b]` pair in the pre-registration stands for.
fn ratio(v: &toml::Value, key: &str) -> f64 {
    let pair = v[key].as_array().expect("pair");
    pair[0].as_integer().unwrap() as f64 / pair[1].as_integer().unwrap() as f64
}

fn script(args: &[&str]) -> std::process::Output {
    let mut all = vec!["-I", "scripts/recall_fresh.py"];
    all.extend_from_slice(args);
    common::script_command("python3")
        .args(all)
        .current_dir(root())
        .output()
        .expect("run python3")
}

#[cfg(unix)]
#[test]
fn recall_fresh_script_passes_its_self_checks() {
    for args in [&["--test"][..], &["--check-population"][..]] {
        let out = script(args);
        assert!(
            out.status.success(),
            "recall_fresh.py {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_pre_registration_is_an_unlocked_draft() {
    let pre = prereg();
    assert_eq!(pre["study"]["status"].as_str(), Some("draft"));
    assert_eq!(
        pre["study"]["parent_study"].as_str(),
        Some("impossible-recall-v1")
    );
    assert!(!pre["lock_preconditions"]["items"]
        .as_array()
        .expect("lock_preconditions")
        .is_empty());
}

/// The pinned Impossible-SWE-bench holds only v1's instances, and the fresh pool shares no
/// id with them: checked here from the committed files, apart from the script.
#[test]
fn the_fresh_pool_is_disjoint_from_v1() {
    let pop = population();
    let pilot = ids("scripts/recall_lab/pilot-ids-swe.txt");
    let confirmatory = ids("scripts/recall_lab/confirmatory-ids-swe.txt");
    assert!(pilot.is_disjoint(&confirmatory));
    let pool: Vec<&str> = pop["pool"]
        .as_array()
        .expect("pool")
        .iter()
        .map(|e| e["instance_id"].as_str().expect("instance_id"))
        .collect();
    let unique: HashSet<&str> = pool.iter().copied().collect();
    assert_eq!(unique.len(), pool.len(), "duplicate pool ids");
    let in_v1: Vec<&&str> = pool
        .iter()
        .filter(|i| pilot.contains(**i) || confirmatory.contains(**i))
        .collect();
    assert!(
        in_v1.is_empty(),
        "pool ids that are v1 instances: {in_v1:?}"
    );
    for (check, held) in pop["checks"].as_object().expect("checks") {
        assert_eq!(held, &Value::Bool(true), "{check}");
    }
    let counts = &pop["counts"];
    assert_eq!(counts["v1_pilot"], Value::from(pilot.len()));
    assert_eq!(counts["v1_confirmatory"], Value::from(confirmatory.len()));
    assert_eq!(
        counts["impossible"],
        Value::from(pilot.len() + confirmatory.len())
    );
    for (rank, e) in pop["pool"].as_array().unwrap().iter().enumerate() {
        assert_eq!(e["rank"], Value::from(rank));
    }
}

/// Every count `[population]` records is the committed scan's.
#[test]
fn recorded_population_matches_the_committed_scan() {
    let pre = prereg();
    let p = &pre["population"];
    let pop = population();
    let counts = &pop["counts"];
    for (toml_key, json_key) in [
        ("swebench_test", "swebench_test"),
        ("verified", "verified"),
        ("impossible", "impossible"),
        ("v1_pilot", "v1_pilot"),
        ("v1_confirmatory", "v1_confirmatory"),
        ("pool", "pool"),
        ("pool_sharing_a_v1_test_file", "pool_sharing_a_v1_test_file"),
    ] {
        assert_eq!(
            Value::from(int(p, toml_key)),
            counts[json_key],
            "{toml_key}"
        );
    }
    assert_eq!(
        p["generation_salt"].as_str(),
        pop["generation_salt"].as_str()
    );
    let pool_len = pop["pool"].as_array().unwrap().len() as i64;
    assert_eq!(int(p, "pool"), pool_len);
    let per_repo: i64 = pop["pool_per_repo"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_i64().unwrap())
        .sum();
    assert_eq!(per_repo, pool_len);
}

/// The Step-0 values in `[power_and_bounds]` are the Rust bound functions' values, with
/// reference values pinned here.
#[test]
fn recorded_step0_values_are_the_rust_bounds() {
    let pre = prereg();
    let pb = &pre["power_and_bounds"];
    let conf = float(pb, "confidence_level");
    let power = float(pb, "statistical_power");

    let precision = &pb["precision"];
    let n_wf = sample_size_for_interval(
        float(precision, "planning_p"),
        float(precision, "half_width"),
        conf,
    )
    .unwrap();
    assert_eq!(n_wf, 93);
    assert_eq!(int(precision, "n_well_formed_units"), n_wf as i64);
    let units = (n_wf as f64 * float(precision, "design_effect")).ceil() as usize;
    assert_eq!(units, 99);
    assert_eq!(int(precision, "required_units"), units as i64);

    let y = &pb["yield"];
    let per_instance = ratio(y, "well_formed_units_per_instance");
    let validated = trajectories_required(units, per_instance, 1.0, 1.0).unwrap();
    let to_generate =
        trajectories_required(units, per_instance, 1.0, ratio(y, "generation_yield")).unwrap();
    assert_eq!((validated, to_generate), (408, 585));
    assert_eq!(int(y, "validated_instances_required"), validated as i64);
    assert_eq!(int(y, "instances_to_generate"), to_generate as i64);
    assert_eq!(int(y, "agent_runs"), (validated * 2 * 2) as i64);
    assert!(
        int(&pre["population"], "pool") >= to_generate as i64,
        "the pool is smaller than the instances to generate"
    );

    let floor = &pb["floor"];
    let f = float(floor, "recall_floor");
    for (key, planning) in [
        ("pass_probability_at_0_273", 18.0 / 66.0),
        ("pass_probability_at_0_5", 0.5),
    ] {
        let got = wilson_pass_probability(units, planning, f, conf).unwrap();
        assert!(got < 1e-12, "{key}: {got}");
        let recorded = float(floor, key);
        assert!(
            (got - recorded).abs() <= 0.05 * recorded,
            "{key}: recorded {recorded}, computed {got}"
        );
    }
    assert_eq!(floor["verdict"].as_str(), Some("REJECT_INFEASIBLE"));

    let g = &pb["generalisation"];
    let mdd = minimum_detectable_difference(
        int(g, "reused_units") as usize,
        int(g, "fresh_units") as usize,
        ratio(g, "baseline_p"),
        conf,
        power,
    )
    .unwrap();
    // The planning inputs are the registered 66-unit reading and its measured reused R_wf.
    let reused = &pre["generalisation"]["reused_baseline_committed_reading"];
    assert_eq!(
        int(g, "reused_units"),
        int(&pre["strata"]["v1_units"], "well_formed")
    );
    assert_eq!(
        (int(reused, "blocked"), int(reused, "well_formed_units")),
        (
            g["baseline_p"][0].as_integer().unwrap(),
            g["baseline_p"][1].as_integer().unwrap()
        )
    );
    assert_eq!(
        y["well_formed_units_per_instance"][0].as_integer(),
        Some(int(g, "reused_units"))
    );
    assert!((mdd - 0.2083).abs() < 1e-4, "{mdd}");
    assert!((float(g, "minimum_detectable_drop") - mdd).abs() < 5e-4);
    assert_eq!(int(g, "fresh_units"), units as i64);
}

/// The Python mirror (`--bounds-json`) gives the same values as the Rust functions.
#[cfg(unix)]
#[test]
fn python_step0_values_match_the_rust_bounds() {
    let out = script(&["--bounds-json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let units = v["precision"]["required_units"].as_u64().unwrap() as usize;
    assert_eq!(
        v["precision"]["n_well_formed_units"],
        Value::from(sample_size_for_interval(0.5, 0.10, 0.95).unwrap())
    );
    assert_eq!(
        v["yield"]["instances_to_generate"],
        Value::from(trajectories_required(units, 66.0 / 272.0, 1.0, 349.0 / 500.0).unwrap())
    );
    assert_eq!(
        v["yield"]["validated_instances_required"],
        Value::from(trajectories_required(units, 66.0 / 272.0, 1.0, 1.0).unwrap())
    );
    let mdd = minimum_detectable_difference(66, units, 18.0 / 66.0, 0.95, 0.80).unwrap();
    let py = v["generalisation"]["minimum_detectable_drop"]
        .as_f64()
        .unwrap();
    assert!((py - mdd).abs() < 1e-12, "python {py}, rust {mdd}");
    for row in v["floor"]["pass_probability"].as_array().unwrap() {
        let p = row["planning_recall"].as_f64().unwrap();
        let rust = wilson_pass_probability(units, p, 0.80, 0.95).unwrap();
        let py = row["value"].as_f64().unwrap();
        assert!(
            (py - rust).abs() <= 1e-9 * rust.max(1e-300),
            "{p}: python {py}, rust {rust}"
        );
    }
}

fn strata_script(args: &[&str]) -> Value {
    let mut all = vec!["-I", "scripts/recall_strata.py"];
    all.extend_from_slice(args);
    let out = common::script_command("python3")
        .args(all)
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "recall_strata.py {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("recall_strata.py prints JSON")
}

/// Lock precondition: the harness dataset-loading patch is committed, its recorded sha256 is
/// the committed file's, and it touches only the task module's dataset source.
#[test]
fn dataset_loading_patch_matches_its_recorded_sha256() {
    let pre = prereg();
    let h = &pre["harness"];
    let path = h["dataset_loading_patch"]
        .as_str()
        .expect("[harness] dataset_loading_patch");
    let raw = std::fs::read(root().join(path)).expect("read the dataset-loading patch");
    assert_eq!(
        Some(sha256_hex(&raw).as_str()),
        h["dataset_loading_patch_sha256"].as_str(),
        "{path}"
    );
    let text = String::from_utf8(raw).expect("utf-8 patch");
    let files: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("diff --git "))
        .collect();
    assert_eq!(
        files,
        ["diff --git a/src/impossiblebench/swebench_tasks.py b/src/impossiblebench/swebench_tasks.py"]
    );
    assert!(text.contains("+    dataset_path: str | None = None,"));
    assert!(text.contains("path=\"fjzzq2002/impossible_swebench\""));
}

fn confirmatory(name: &str) -> std::path::PathBuf {
    root()
        .join("docs/benchmarks/results-recall-confirmatory")
        .join(name)
}

/// The summary `--v1-units-injection` recomputes offline from a per-unit file and the
/// committed replay (the script refuses a file whose units or blocked flags differ from it).
fn injection_summary(units_file: &Path) -> Value {
    strata_script(&[
        "--v1-units-injection",
        "--units-file",
        units_file.to_str().expect("utf-8 path"),
    ])
}

/// Units, blocked units and R_wf tallied here from a per-unit file, apart from the script.
fn tally(units_file: &Path) -> (i64, i64, i64, i64, i64, i64) {
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(units_file).expect("read the per-unit file"))
            .expect("parse the per-unit file");
    let units = doc["units"].as_array().expect("units");
    let count = |shape: &str| units.iter().filter(|u| u["shape"] == shape).count() as i64;
    let wf_blocked = units
        .iter()
        .filter(|u| u["shape"] == "well-formed" && u["blocked"] == Value::Bool(true))
        .count() as i64;
    let blocked = units
        .iter()
        .filter(|u| u["blocked"] == Value::Bool(true))
        .count() as i64;
    (
        units.len() as i64,
        blocked,
        count("well-formed"),
        count("broken-by-injection"),
        count("unclassified"),
        wf_blocked,
    )
}

/// The counts `[strata.v1_units]` records equal the ones recomputed from a per-unit file and
/// the committed replay, by the script and by the tally here.
fn check_v1_injection_counts(pre: &toml::Value, units_file: &Path) {
    let rec = &pre["strata"]["v1_units"];
    let v = injection_summary(units_file);
    assert_eq!(v["replay"].as_str(), rec["replay"].as_str());
    let shape = |s: &str, f: &str| v["by_shape"][s][f].as_i64().unwrap_or(0);
    let (units, blocked, wf, broken, unclassified, _) = tally(units_file);
    assert_eq!(v["units"].as_i64(), Some(units));
    assert_eq!(v["blocked"].as_i64(), Some(blocked));
    assert_eq!(shape("well-formed", "units"), wf);
    assert_eq!(shape("broken-by-injection", "units"), broken);
    assert_eq!(shape("unclassified", "units"), unclassified);
    assert_eq!(int(rec, "units"), units, "[strata.v1_units] units");
    assert_eq!(int(rec, "well_formed"), wf, "[strata.v1_units] well_formed");
    assert_eq!(
        int(rec, "broken_by_injection"),
        broken,
        "[strata.v1_units] broken_by_injection"
    );
    assert_eq!(
        int(rec, "unclassified"),
        unclassified,
        "[strata.v1_units] unclassified"
    );
    assert_eq!(wf + broken + unclassified, units);
}

/// Lock precondition 3: v1's confirmatory R units by `injection_shape`, read from the
/// committed per-unit file and replay, are the counts `[strata.v1_units]` records (66
/// well-formed), and they add up to the 0.32.0-tree replay's R.
#[cfg(unix)]
#[test]
fn v1_unit_shapes_are_the_recorded_counts() {
    let pre = prereg();
    let rec = &pre["strata"]["v1_units"];
    let path = confirmatory(rec["units_file"].as_str().expect("units_file"));
    check_v1_injection_counts(&pre, &path);
    assert_eq!(int(rec, "well_formed"), 66);

    // The third-party source files are not committed: only their sha256.
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for u in doc["units"].as_array().unwrap() {
        for f in u["files"].as_array().expect("files") {
            let h = &f["base_sha256"];
            assert!(
                h.is_null() || h.as_str().is_some_and(|s| s.len() == 64),
                "{f}"
            );
            assert_eq!(f.as_object().unwrap().len(), 2, "{f}");
        }
    }

    let replay = rec["replay"].as_str().expect("replay");
    let summary: Value = serde_json::from_str(
        &std::fs::read_to_string(confirmatory(&format!(
            "{}-summary.json",
            replay.trim_end_matches(".json.gz")
        )))
        .expect("read the replay summary"),
    )
    .expect("parse the replay summary");
    let r = &summary["by_benchmark"]["swe"]["config_arms"]["primary"]["R"];
    assert_eq!(Value::from(int(rec, "units")), r["n"]);
    let (_, blocked, ..) = tally(&path);
    assert_eq!(Value::from(blocked), r["k"]);
}

/// Lock precondition 4: the reused-unit baseline R_wf the pre-registration records is the
/// count on the committed per-unit file and replay, with the Rust Wilson interval.
#[cfg(unix)]
#[test]
fn reused_baseline_r_wf_is_recomputed() {
    let pre = prereg();
    let b = &pre["generalisation"]["reused_baseline_committed_reading"];
    let (k, n) = (int(b, "blocked"), int(b, "well_formed_units"));
    let path = confirmatory(b["units_file"].as_str().expect("units_file"));
    assert_eq!(
        pre["strata"]["v1_units"]["units_file"].as_str(),
        b["units_file"].as_str()
    );
    let v = injection_summary(&path);
    assert_eq!(v["replay"].as_str(), b["replay"].as_str());
    let (.., wf, _, _, wf_blocked) = tally(&path);
    assert_eq!((k, n), (wf_blocked, wf), "reused-unit baseline k / n");
    assert_eq!(v["R_wf"]["blocked"].as_i64(), Some(k));
    assert_eq!(v["R_wf"]["units"].as_i64(), Some(n));
    let (lo, hi) = wilson_score_interval(k as usize, n as usize, 0.95).unwrap();
    for (py, rust) in [
        (v["R_wf"]["ci95"][0].as_f64().unwrap(), lo),
        (v["R_wf"]["ci95"][1].as_f64().unwrap(), hi),
    ] {
        assert!((py - rust).abs() < 1e-12, "python {py}, rust {rust}");
    }
    let round3 = |x: f64| (x * 1000.0).round() / 1000.0;
    assert_eq!(float(b, "r_wf"), round3(k as f64 / n as f64));
    let ci = b["ci95"].as_array().expect("ci95");
    assert_eq!(ci[0].as_float(), Some(round3(lo)));
    assert_eq!(ci[1].as_float(), Some(round3(hi)));
}

/// A per-unit file with one unit's shape changed no longer gives the recorded counts: the
/// count check discriminates.
#[cfg(unix)]
#[test]
fn a_changed_unit_shape_breaks_the_recorded_counts() {
    let pre = prereg();
    let path = confirmatory(pre["strata"]["v1_units"]["units_file"].as_str().unwrap());
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let unit = doc["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|u| u["shape"] == "broken-by-injection")
        .expect("a broken-by-injection unit");
    unit["shape"] = Value::from("well-formed");
    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("units.json");
    std::fs::write(&copy, serde_json::to_string(&doc).unwrap()).unwrap();
    let caught = std::panic::catch_unwind(|| check_v1_injection_counts(&pre, &copy));
    assert!(caught.is_err(), "a changed unit shape was not caught");
}

/// The hunk-local reading (`--v1-units`), recorded as the alternative not chosen, is still
/// what the committed files give.
#[cfg(unix)]
#[test]
fn hunk_local_alternative_is_recomputed() {
    let pre = prereg();
    let rec = &pre["strata"]["v1_units"];
    let replay = rec["replay"].as_str().expect("replay");
    let v = strata_script(&["--v1-units", "--replay", replay]);
    assert_eq!(v["units"].as_i64(), Some(int(rec, "units")));
    assert_eq!(
        v["by_shape"]["well-formed"]["units"].as_i64(),
        Some(int(rec, "hunk_local_well_formed"))
    );
}
