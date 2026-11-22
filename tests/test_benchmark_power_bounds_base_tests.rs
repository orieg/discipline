//! Step-0 bounds of the base-tests study (`docs/benchmarks/base-tests-preregistration.toml`,
//! #1052): the Wilson ceiling functions in `src/guards/perf/bounds.rs` return their pinned
//! values, every sample size the pre-registration states is what they return, the Python mirror
//! in `scripts/benchmark_power_audit.py` agrees, and every population and compute count is the
//! one the committed #838 files give.

mod common;

use discipline::guards::perf::bounds::{
    sample_size_for_wilson_lower_bound_power, sample_size_for_wilson_upper_bound_power,
    wilson_pass_probability, wilson_upper_pass_probability,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

const PREREG: &str = "docs/benchmarks/base-tests-preregistration.toml";
const STRATA: &str = "docs/benchmarks/results-recall-confirmatory/strata.json";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn prereg() -> toml::Value {
    let raw = std::fs::read_to_string(root().join(PREREG)).expect("pre-registration exists");
    toml::from_str(&raw).expect("pre-registration parses")
}

fn strata() -> Value {
    let raw = std::fs::read_to_string(root().join(STRATA)).expect("strata.json exists");
    serde_json::from_str(&raw).expect("strata.json parses")
}

fn int(v: &toml::Value) -> usize {
    v.as_integer().expect("an integer") as usize
}

fn float(v: &toml::Value) -> f64 {
    v.as_float().expect("a float")
}

/// The repository owner of a SWE-bench instance id (`django__django-11099` -> `django`).
fn owner(instance_id: &str) -> &str {
    instance_id.split("__").next().expect("an owner prefix")
}

fn ids(path: &str) -> Vec<String> {
    std::fs::read_to_string(root().join(path))
        .expect("id file exists")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn counts_table(t: &toml::Value) -> BTreeMap<String, usize> {
    t.as_table()
        .expect("a table")
        .iter()
        .map(|(k, v)| (k.clone(), int(v)))
        .collect()
}

#[test]
fn wilson_upper_pass_probability_matches_the_exact_binomial_tail() {
    // Cross-checked with exact rational arithmetic (`fractions.Fraction`, `math.comb`).
    for (n, p, ceiling, expected) in [
        (83, 0.05, 0.15, 0.878_481_673_269_529_2),
        (82, 0.05, 0.15, 0.773_089_972_210_432_3),
        (272, 0.10, 0.15, 0.686_459_287_674_456_3),
        (272, 0.05, 0.10, 0.860_161_816_618_945_6),
    ] {
        let got = wilson_upper_pass_probability(n, p, ceiling, 0.95).unwrap();
        assert!(
            (got - expected).abs() < 1e-10,
            "n={n}, p={p}, ceiling={ceiling}: {got} != {expected}"
        );
    }
    // Even k = 0 has a Wilson upper bound above 0.10 at n = 5.
    assert_eq!(
        wilson_upper_pass_probability(5, 0.01, 0.10, 0.95).unwrap(),
        0.0
    );
}

/// The ceiling tail is the lower-bound tail reflected: the score interval of `(n - k) / n` is
/// `(1 - upper, 1 - lower)`.
#[test]
fn the_ceiling_bound_is_the_floor_bound_reflected() {
    for n in [1usize, 10, 37, 83, 112, 272, 1000] {
        for (p, c) in [(0.02, 0.15), (0.05, 0.15), (0.10, 0.20), (0.30, 0.45)] {
            let up = wilson_upper_pass_probability(n, p, c, 0.95).unwrap();
            let low = wilson_pass_probability(n, 1.0 - p, 1.0 - c, 0.95).unwrap();
            assert!(
                (up - low).abs() < 1e-9,
                "n={n}, p={p}, c={c}: {up} vs {low}"
            );
        }
    }
    assert_eq!(
        sample_size_for_wilson_upper_bound_power(0.10, 0.20, 0.95, 0.80, 2000).unwrap(),
        sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 2000).unwrap()
    );
}

/// The monotonicity choice: 74-78 meet the power, 79-82 fall below it again, so the
/// requirement is 83, the start of the last unbroken run.
#[test]
fn the_ceiling_requirement_is_the_last_unbroken_run() {
    let meets = |n: usize| wilson_upper_pass_probability(n, 0.05, 0.15, 0.95).unwrap() >= 0.80;
    assert!((74..=78).all(meets));
    assert!(!(79..=82).any(meets));
    assert_eq!(
        sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 2000).unwrap(),
        83
    );
    assert_eq!(
        sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 500).unwrap(),
        83
    );
}

#[test]
fn ceiling_bounds_reject_invalid_inputs() {
    assert!(wilson_upper_pass_probability(0, 0.05, 0.15, 0.95).is_err());
    assert!(wilson_upper_pass_probability(10, 0.0, 0.15, 0.95).is_err());
    assert!(wilson_upper_pass_probability(10, 0.05, 1.0, 0.95).is_err());
    assert!(wilson_upper_pass_probability(10, 0.05, 0.15, f64::NAN).is_err());
    // The ceiling must sit above the planning rate, and n_max must reach the power.
    // A ceiling at the planning rate is refused by its own guard, not by the power search.
    let err = sample_size_for_wilson_upper_bound_power(0.15, 0.15, 0.95, 0.80, 2000)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("must be above the expected proportion"),
        "{err}"
    );
    assert!(sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 50).is_err());
    assert!(sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 0).is_err());
    assert!(sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 1.0, 2000).is_err());
}

#[test]
fn the_study_is_a_draft_until_its_pilot_values_are_written() {
    let p = prereg();
    let s = &p["study"];
    assert_eq!(s["id"].as_str(), Some("base-tests-v1"));
    assert_eq!(s["issue"].as_integer(), Some(1052));
    let pilot = &p["pilot"];
    match s["status"].as_str().unwrap() {
        "draft" => {
            assert_eq!(pilot["observed_friction"].as_str(), Some("PILOT"));
            assert_eq!(pilot["observed_eligible_share"].as_str(), Some("PILOT"));
            assert_eq!(p["binary"]["sha256"].as_str(), Some("LOCK"));
        }
        "locked-pre-data" => {
            assert!(pilot["observed_friction"].as_float().is_some());
            assert!(pilot["observed_eligible_share"].as_float().is_some());
            for (k, v) in p["constants"].as_table().unwrap() {
                assert_ne!(v.as_str(), Some("LOCK"), "constant {k} unset at the lock");
            }
        }
        other => panic!("unexpected status {other}"),
    }
}

#[test]
fn the_population_counts_are_the_committed_id_files() {
    let p = prereg();
    let pop = &p["population"];
    let confirmatory = ids(pop["confirmatory_ids"].as_str().unwrap());
    let pilot = ids(pop["pilot_ids"].as_str().unwrap());
    assert_eq!(int(&pop["confirmatory_instances"]), confirmatory.len());
    assert_eq!(int(&pop["pilot_instances"]), pilot.len());
    let mut per_repo = BTreeMap::new();
    for id in &confirmatory {
        *per_repo.entry(owner(id).to_string()).or_insert(0usize) += 1;
    }
    assert_eq!(counts_table(&pop["confirmatory_per_repository"]), per_repo);
    assert_eq!(
        int(&p["power_and_bounds"]["q2_available"]),
        confirmatory.len()
    );
}

#[test]
fn the_q1_counts_are_the_committed_counterfactual_strata() {
    let p = prereg();
    let q1 = &p["q1_feasibility"];
    let s = strata();
    let mut per_repo = BTreeMap::new();
    for pair in s["counterfactual"]["pairs"].as_array().unwrap() {
        if matches!(pair["stratum"].as_str(), Some("mixed" | "test-side")) {
            let id = pair["instance_id"].as_str().unwrap();
            *per_repo.entry(owner(id).to_string()).or_insert(0usize) += 1;
        }
    }
    assert_eq!(counts_table(&q1["eligible_pairs_per_repository"]), per_repo);
    assert_eq!(int(&q1["eligible_pairs"]), per_repo.values().sum::<usize>());
    assert_eq!(
        int(&q1["sampled_pairs"]),
        per_repo.values().map(|&c| c.min(2)).sum::<usize>()
    );
    let salt = q1["q1_salt"].as_str().unwrap();
    assert_eq!(salt.len(), 32);
    assert!(salt.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn the_q2_sample_sizes_match_rust_bounds() {
    let pb = &prereg()["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["upper_bound_n_max"]);
    let available = int(&pb["q2_available"]);
    let (rate, ceiling) = (float(&pb["q2_planning_rate"]), float(&pb["q2_ceiling"]));
    let required =
        sample_size_for_wilson_upper_bound_power(rate, ceiling, conf, power, n_max).unwrap();
    assert_eq!(int(&pb["q2_required_gold_prs"]), required);
    let at_available = wilson_upper_pass_probability(available, rate, ceiling, conf).unwrap();
    assert!(
        (float(&pb["q2_power_at_available"]) - at_available).abs() < 5e-6,
        "{at_available}"
    );
    assert!(
        (float(&pb["q2_min_eligible_share"]) - required as f64 / available as f64).abs() < 5e-4
    );
    let verdict = |n: usize| {
        if n <= available {
            "PROCEED"
        } else {
            "REJECT_UNDETECTABLE"
        }
    };
    assert_eq!(pb["q2_step0_verdict"].as_str(), Some(verdict(required)));
    let rows = pb["q2_sensitivity"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| { float(&r["planning_rate"]) == rate && float(&r["ceiling"]) == ceiling }));
    for row in rows {
        let (r, c) = (float(&row["planning_rate"]), float(&row["ceiling"]));
        let n = sample_size_for_wilson_upper_bound_power(r, c, conf, power, n_max).unwrap();
        assert_eq!(int(&row["required_gold_prs"]), n, "({r}, {c})");
        assert_eq!(row["verdict"].as_str(), Some(verdict(n)), "({r}, {c})");
    }
}

#[test]
fn the_compute_figures_are_the_counterfactual_runner_records() {
    let c = &prereg()["compute"];
    let s = strata();
    let mut seconds: Vec<u64> = Vec::new();
    let mut timeouts = 0usize;
    for pair in s["counterfactual"]["pairs"].as_array().unwrap() {
        for run in pair["runs"].as_array().unwrap() {
            seconds.push(run["seconds"].as_u64().unwrap());
            timeouts += usize::from(run["timed_out"].as_bool().unwrap());
        }
    }
    seconds.sort_unstable();
    let n = seconds.len();
    let median = if n % 2 == 1 {
        seconds[n / 2] as f64
    } else {
        (seconds[n / 2 - 1] + seconds[n / 2]) as f64 / 2.0
    };
    assert_eq!(int(&c["counterfactual_runs"]), n);
    assert_eq!(
        c["counterfactual_median_seconds"].as_integer(),
        Some(median as i64)
    );
    assert_eq!(median.fract(), 0.0);
    assert_eq!(
        int(&c["counterfactual_max_seconds"]) as u64,
        *seconds.last().unwrap()
    );
    assert_eq!(int(&c["counterfactual_timeouts"]), timeouts);
}

/// `--base-tests-bounds-json` prints the values the pre-registration records, and the Python
/// ceiling probability agrees with the Rust one.
#[cfg(unix)]
#[test]
fn python_base_tests_bounds_json_matches_the_preregistration() {
    let out = common::script_command("python3")
        .args([
            "-I",
            "scripts/benchmark_power_audit.py",
            "--base-tests-bounds-json",
        ])
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let pb = &prereg()["power_and_bounds"];
    let q2 = &json["q2_primary"];
    assert_eq!(
        q2["planning_rate"].as_f64(),
        pb["q2_planning_rate"].as_float()
    );
    assert_eq!(q2["ceiling"].as_f64(), pb["q2_ceiling"].as_float());
    assert_eq!(
        q2["available"].as_u64(),
        Some(int(&pb["q2_available"]) as u64)
    );
    assert_eq!(
        q2["n_max"].as_u64(),
        Some(int(&pb["upper_bound_n_max"]) as u64)
    );
    assert_eq!(
        q2["required_gold_prs"].as_u64(),
        Some(int(&pb["q2_required_gold_prs"]) as u64)
    );
    assert_eq!(q2["verdict"].as_str(), pb["q2_step0_verdict"].as_str());
    let rust = wilson_upper_pass_probability(272, 0.05, 0.15, 0.95).unwrap();
    assert!((q2["power_at_available"].as_f64().unwrap() - rust).abs() < 1e-9);
    let grid = json["q2_grid"].as_array().unwrap();
    let rows = pb["q2_sensitivity"].as_array().unwrap();
    assert_eq!(grid.len(), rows.len());
    for (g, r) in grid.iter().zip(rows) {
        assert_eq!(g["planning_rate"].as_f64(), r["planning_rate"].as_float());
        assert_eq!(g["ceiling"].as_f64(), r["ceiling"].as_float());
        assert_eq!(
            g["required_gold_prs"].as_u64(),
            Some(int(&r["required_gold_prs"]) as u64)
        );
        let (p, c) = (float(&r["planning_rate"]), float(&r["ceiling"]));
        let rust = wilson_upper_pass_probability(272, p, c, 0.95).unwrap();
        assert!(
            (g["power_at_available"].as_f64().unwrap() - rust).abs() < 1e-9,
            "({p}, {c})"
        );
    }
}
