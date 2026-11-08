//! Step-0 bounds of the prevention study (`docs/benchmarks/prevention-preregistration.toml`,
//! #1036): every sample size the pre-registration states is what the Rust function in
//! `src/guards/perf/bounds.rs` returns for its stated inputs, and the Python mirrors in
//! `scripts/benchmark_power_audit.py` agree with the Rust ones. The bound functions' own
//! pinned values are in `bounds.rs`'s unit tests and the Python self-check.

mod common;

use discipline::guards::perf::bounds::{
    design_effect_inflated_n, difference_pass_probability, filter_residual_rate,
    noninferiority_pass_probability, sample_size_for_difference_lower_bound_power,
    sample_size_for_noninferiority_power, wilson_score_interval,
};
use serde_json::Value;
use std::path::Path;

const PREREG: &str = "docs/benchmarks/prevention-preregistration.toml";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn prereg() -> toml::Value {
    let raw = std::fs::read_to_string(root().join(PREREG)).expect("pre-registration exists");
    toml::from_str(&raw).expect("pre-registration parses")
}

fn int(v: &toml::Value) -> usize {
    v.as_integer().expect("an integer") as usize
}

fn float(v: &toml::Value) -> f64 {
    v.as_float().expect("a float")
}

#[test]
fn the_study_is_a_draft_until_its_pilot_values_are_written() {
    let p = prereg();
    let s = &p["study"];
    assert_eq!(s["id"].as_str(), Some("prevention-v1"));
    assert_eq!(s["issue"].as_integer(), Some(1036));
    let icc = &p["pilot"]["observed_icc"];
    match s["status"].as_str().unwrap() {
        "draft" => assert_eq!(icc.as_str(), Some("PILOT"), "a draft writes no pilot value"),
        "locked-pre-data" => assert!(icc.as_float().is_some(), "the lock writes the pilot ICC"),
        other => panic!("unexpected status {other}"),
    }
    let arms: Vec<&str> = p["arms"]["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(arms, ["off", "on", "twin"]);
}

#[test]
fn the_planning_rates_are_the_838_counts() {
    // 212 of 1069 impossible-split runs and 432 of 536 `original` runs passed in #838's
    // confirmatory `access_full` arm; the Wilson intervals are the ones the text quotes.
    let pb = &prereg()["power_and_bounds"];
    assert!((float(&pb["h1_off_rate"]) - 212.0 / 1069.0).abs() < 5e-4);
    assert!((float(&pb["h2_rate"]) - 432.0 / 536.0).abs() < 5e-4);
    let (lo, hi) = wilson_score_interval(212, 1069, 0.95).unwrap();
    assert_eq!(
        (format!("{lo:.3}"), format!("{hi:.3}")),
        ("0.176".into(), "0.223".into())
    );
    let (lo, hi) = wilson_score_interval(432, 536, 0.95).unwrap();
    assert_eq!(
        (format!("{lo:.3}"), format!("{hi:.3}")),
        ("0.770".into(), "0.837".into())
    );
    let (lo, hi) = wilson_score_interval(20, 533, 0.95).unwrap();
    assert_eq!(
        (format!("{lo:.3}"), format!("{hi:.3}")),
        ("0.024".into(), "0.057".into())
    );
}

#[test]
fn h1_sample_sizes_match_rust_bounds() {
    let pb = &prereg()["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["difference_n_max"]);
    let (off, on, floor) = (
        float(&pb["h1_off_rate"]),
        float(&pb["h1_on_rate"]),
        float(&pb["h1_floor"]),
    );
    let runs = sample_size_for_difference_lower_bound_power(off, on, floor, 1, conf, power, n_max)
        .unwrap();
    assert_eq!(int(&pb["h1_required_runs_per_arm"]), runs);
    // Two impossible runs per instance, worst-case correlation.
    let inflated = design_effect_inflated_n(runs, 2.0, 1.0).unwrap();
    assert_eq!(int(&pb["h1_required_runs_inflated"]), inflated);
    assert_eq!(int(&pb["h1_required_instances"]), inflated.div_ceil(2));
    for row in pb["h1_sensitivity"].as_array().unwrap() {
        let r = float(&row["on_rate"]);
        assert_eq!(
            int(&row["required_runs_per_arm"]),
            sample_size_for_difference_lower_bound_power(off, r, floor, 1, conf, power, n_max)
                .unwrap(),
            "on_rate {r}"
        );
    }
}

/// H1's Step 0 from the measured block rate: the recorded on-rate and power are what the Rust
/// functions return, the verdict follows from the power, and an undetectable H1 keeps the
/// study a draft.
#[test]
fn h1_step0_verdict_follows_from_the_measured_block_rate() {
    let p = prereg();
    let pb = &p["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["difference_n_max"]);
    let floor = float(&pb["h1_floor"]);
    let off = 212.0 / 1069.0;
    let block = int(&pb["h1_step0_block_k"]) as f64 / int(&pb["h1_step0_block_n"]) as f64;
    let on = filter_residual_rate(off, block).unwrap();
    assert!((float(&pb["h1_step0_on_rate"]) - on).abs() < 5e-4, "{on}");
    let p_at_max = difference_pass_probability(n_max, off, n_max, on, floor, conf).unwrap();
    assert!(
        (float(&pb["h1_step0_power_at_n_max"]) - p_at_max).abs() < 5e-4,
        "{p_at_max}"
    );
    let verdict = pb["h1_step0_verdict"].as_str().unwrap();
    if p_at_max < power {
        assert_eq!(verdict, "REJECT_UNDETECTABLE");
        assert!(sample_size_for_difference_lower_bound_power(
            off, on, floor, 1, conf, power, n_max
        )
        .is_err());
        assert_eq!(
            p["study"]["status"].as_str(),
            Some("draft"),
            "an undetectable H1 cannot be locked"
        );
    } else {
        assert_eq!(verdict, "PROCEED");
    }
}

#[test]
fn h2_sample_sizes_match_rust_bounds() {
    let pb = &prereg()["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["noninferiority_n_max"]);
    let (rate, margin) = (float(&pb["h2_rate"]), float(&pb["h2_margin"]));
    let runs =
        sample_size_for_noninferiority_power(rate, rate, margin, conf, power, n_max).unwrap();
    assert_eq!(int(&pb["h2_required_runs_per_arm"]), runs);
    for row in pb["h2_sensitivity"].as_array().unwrap() {
        let m = float(&row["margin"]);
        assert_eq!(
            int(&row["required_runs_per_arm"]),
            sample_size_for_noninferiority_power(rate, rate, m, conf, power, n_max).unwrap(),
            "margin {m}"
        );
    }
}

/// `--prevention-bounds-json` prints the values the pre-registration records, and the Python
/// non-inferiority probability agrees with the Rust one.
#[test]
fn python_prevention_bounds_json_matches_the_preregistration() {
    let out = common::script_command("python3")
        .args([
            "-I",
            "scripts/benchmark_power_audit.py",
            "--prevention-bounds-json",
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
    let h1 = &json["h1_prevention"];
    assert_eq!(h1["off_rate"].as_f64(), pb["h1_off_rate"].as_float());
    assert_eq!(h1["on_rate"].as_f64(), pb["h1_on_rate"].as_float());
    assert_eq!(h1["floor"].as_f64(), pb["h1_floor"].as_float());
    assert_eq!(
        h1["n_max"].as_u64(),
        Some(int(&pb["difference_n_max"]) as u64)
    );
    assert_eq!(
        h1["required_runs_per_arm"].as_u64(),
        Some(int(&pb["h1_required_runs_per_arm"]) as u64)
    );
    let st = &json["h1_step0"];
    assert_eq!(
        st["block_k"].as_u64(),
        Some(int(&pb["h1_step0_block_k"]) as u64)
    );
    assert_eq!(
        st["block_n"].as_u64(),
        Some(int(&pb["h1_step0_block_n"]) as u64)
    );
    assert_eq!(st["verdict"].as_str(), pb["h1_step0_verdict"].as_str());
    assert!((st["on_rate"].as_f64().unwrap() - float(&pb["h1_step0_on_rate"])).abs() < 5e-4);
    assert!(
        (st["power_at_n_max"].as_f64().unwrap() - float(&pb["h1_step0_power_at_n_max"])).abs()
            < 5e-4
    );
    let h2 = &json["h2_cost"];
    assert_eq!(h2["rate"].as_f64(), pb["h2_rate"].as_float());
    assert_eq!(h2["margin"].as_f64(), pb["h2_margin"].as_float());
    assert_eq!(
        h2["n_max"].as_u64(),
        Some(int(&pb["noninferiority_n_max"]) as u64)
    );
    assert_eq!(
        h2["required_runs_per_arm"].as_u64(),
        Some(int(&pb["h2_required_runs_per_arm"]) as u64)
    );

    let program = "import json, sys\nsys.path.insert(0, 'scripts')\nimport benchmark_power_audit as b\nprint(json.dumps([b.noninferiority_pass_probability(n1, p1, n2, p2, m) for (n1, p1, n2, p2, m) in [(14, 0.7, 16, 0.75, 0.2), (60, 0.8, 60, 0.8, 0.1), (33, 0.4, 47, 0.5, 0.05)]]))\n";
    let out = common::script_command("python3")
        .args(["-I", "-c", program])
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let py: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    for (i, (n1, p1, n2, p2, m)) in [
        (14, 0.7, 16, 0.75, 0.2),
        (60, 0.8, 60, 0.8, 0.1),
        (33, 0.4, 47, 0.5, 0.05),
    ]
    .into_iter()
    .enumerate()
    {
        let r = noninferiority_pass_probability(n1, p1, n2, p2, m, 0.95).unwrap();
        assert!(
            (py[i].as_f64().unwrap() - r).abs() < 1e-12,
            "({n1}, {p1}, {n2}, {p2}, {m}): {r}"
        );
    }
}
