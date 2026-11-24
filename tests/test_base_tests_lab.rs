//! The `base-tests-v1` lab runner (#1052, `scripts/base_tests_lab.py`): its self-checks pass,
//! its dry-run plan draws the Part 1 sample the pre-registration records (recomputed here
//! independently, with this crate's own SHA-256), plans the Part 2 runs per instance, and
//! confirmatory mode is open once the lock has written every `LOCK` value.

mod common;

use discipline::report::gitlab::sha256_hex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

const SCRIPT: &str = "scripts/base_tests_lab.py";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn prereg() -> toml::Value {
    let raw =
        std::fs::read_to_string(root().join("docs/benchmarks/base-tests-preregistration.toml"))
            .expect("pre-registration exists");
    toml::from_str(&raw).expect("pre-registration parses")
}

fn owner(instance_id: &str) -> &str {
    instance_id.split("__").next().expect("an owner prefix")
}

fn run_script(args: &[&str]) -> std::process::Output {
    common::script_command("python3")
        .arg("-I")
        .arg(SCRIPT)
        .args(args)
        .current_dir(root())
        .output()
        .expect("run python3")
}

fn dry_run() -> Value {
    let out = run_script(&["run", "--mode", "confirmatory", "--dry-run", "--json"]);
    assert!(
        out.status.success(),
        "--dry-run failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the dry run prints JSON")
}

fn counts(v: &Value) -> BTreeMap<String, i64> {
    v.as_object()
        .expect("an object")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_i64().expect("a count")))
        .collect()
}

fn toml_counts(v: &toml::Value) -> BTreeMap<String, i64> {
    v.as_table()
        .expect("a table")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_integer().expect("a count")))
        .collect()
}

#[cfg(unix)]
#[test]
fn base_tests_lab_runner_passes_its_self_checks() {
    let out = run_script(&["--test"]);
    assert!(
        out.status.success(),
        "{SCRIPT} --test failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The dry run's Part 1 counts are the pre-registration's, and its sample is the one the rule
/// gives: per owner, the two eligible pairs with the smallest sha256(q1_salt || pair_id) hex.
#[cfg(unix)]
#[test]
fn dry_run_draws_the_preregistered_part1_sample() {
    let p = prereg();
    let q1 = &p["q1_feasibility"];
    let plan = dry_run();
    let part1 = &plan["part1"];
    assert_eq!(
        part1["sampled_pairs"].as_i64(),
        q1["sampled_pairs"].as_integer()
    );
    assert_eq!(part1["sampled_pairs"].as_i64(), Some(16));
    assert_eq!(
        part1["eligible_pairs"].as_i64(),
        q1["eligible_pairs"].as_integer()
    );
    let eligible = toml_counts(&q1["eligible_pairs_per_repository"]);
    assert_eq!(counts(&part1["eligible_per_repository"]), eligible);
    let want_sampled: BTreeMap<String, i64> = eligible
        .iter()
        .map(|(k, &v)| (k.clone(), v.min(2)))
        .collect();
    assert_eq!(counts(&part1["sampled_per_repository"]), want_sampled);

    // Independent recomputation from strata.json.
    let strata: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root().join("docs/benchmarks/results-recall-confirmatory/strata.json"),
        )
        .expect("strata.json exists"),
    )
    .expect("strata.json parses");
    let salt = q1["q1_salt"].as_str().expect("q1_salt");
    let mut by_owner: BTreeMap<&str, Vec<(String, &str)>> = BTreeMap::new();
    for pair in strata["counterfactual"]["pairs"].as_array().unwrap() {
        if matches!(pair["stratum"].as_str(), Some("mixed" | "test-side")) {
            let id = pair["pair_id"].as_str().unwrap();
            let rank = sha256_hex(format!("{salt}{id}").as_bytes());
            by_owner
                .entry(owner(pair["instance_id"].as_str().unwrap()))
                .or_default()
                .push((rank, id));
        }
    }
    let mut want = Vec::new();
    for ranked in by_owner.values_mut() {
        ranked.sort();
        want.extend(ranked.iter().take(2).map(|(_, id)| id.to_string()));
    }
    let got: Vec<String> = part1["sample"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["pair_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(got, want);
}

/// Part 2 plans a base-on-base control and then the gold run for every confirmatory and pilot
/// instance of the committed id files.
#[cfg(unix)]
#[test]
fn dry_run_plans_part2_control_then_gold_per_instance() {
    let p = prereg();
    let pop = &p["population"];
    let plan = dry_run();
    let part2 = &plan["part2"];
    assert_eq!(
        part2["instances"]["confirmatory"].as_i64(),
        pop["confirmatory_instances"].as_integer()
    );
    assert_eq!(
        part2["instances"]["pilot"].as_i64(),
        pop["pilot_instances"].as_integer()
    );
    let rows = part2["instances_plan"].as_array().unwrap();
    assert!(rows
        .iter()
        .all(|r| r["runs"] == serde_json::json!(["control", "gold"])));
    let mut per_repo = BTreeMap::new();
    for r in rows.iter().filter(|r| r["population"] == "confirmatory") {
        *per_repo
            .entry(r["owner"].as_str().unwrap().to_string())
            .or_insert(0i64) += 1;
    }
    assert_eq!(per_repo, toml_counts(&pop["confirmatory_per_repository"]));
}

/// Once the lock commit has written every `[constants]` and `[binary]` value and set the status
/// to `locked-pre-data`, the plan names no `LOCK` value and confirmatory mode is not refused.
/// The refusal itself (a draft, a `LOCK` value, an overlay) is in the runner's self-checks.
#[cfg(unix)]
#[test]
fn confirmatory_mode_is_open_once_the_lock_has_written_every_value() {
    let p = prereg();
    assert_eq!(p["study"]["status"].as_str(), Some("locked-pre-data"));
    for table in ["constants", "binary"] {
        for (k, v) in p[table].as_table().unwrap() {
            assert_ne!(v.as_str(), Some("LOCK"), "{table}.{k} still LOCK");
        }
    }
    let out = run_script(&["run", "--mode", "confirmatory", "--part", "2"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("refusing confirmatory run"),
        "a locked study is refused: {stderr}"
    );
    assert!(
        stderr.contains("run needs --dataset and --binary"),
        "{stderr}"
    );
    let plan = dry_run();
    assert_eq!(plan["status"].as_str(), Some("locked-pre-data"));
    assert_eq!(
        plan["lock_values_remaining"].as_array().map(Vec::len),
        Some(0),
        "{}",
        plan["lock_values_remaining"]
    );
    assert!(
        plan["confirmatory_refusal"].is_null(),
        "{}",
        plan["confirmatory_refusal"]
    );
}
