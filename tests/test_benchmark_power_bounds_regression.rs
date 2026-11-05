//! Step-0 bounds of the regression-value study
//! (`docs/benchmarks/regression-value-preregistration.toml`): every sample size the
//! pre-registration states is what the Rust function in `src/guards/perf/bounds.rs` returns
//! for its stated inputs, and the Python mirrors in `scripts/benchmark_power_audit.py` agree
//! with the Rust ones. The bound functions' own pinned values are in `bounds.rs`'s unit tests
//! and the Python self-check.

mod common;

use discipline::guards::perf::bounds::{
    difference_pass_probability, kappa_sample_size, newcombe_difference_interval,
    sample_size_for_difference_lower_bound_power, sample_size_for_wilson_lower_bound,
    sample_size_for_wilson_lower_bound_power, zero_event_upper_bound,
};
use serde_json::Value;
use std::path::Path;

const PREREG: &str = "docs/benchmarks/regression-value-preregistration.toml";

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

/// `expr` evaluated with `scripts/benchmark_power_audit.py` imported as `b`, as JSON.
fn power_audit_eval(expr: &str) -> Value {
    let program = format!(
        "import json, sys\nsys.path.insert(0, 'scripts')\nimport benchmark_power_audit as b\nprint(json.dumps({expr}))\n"
    );
    let out = common::script_command("python3")
        .args(["-I", "-c", &program])
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "python3 failed on {expr}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("JSON from python3")
}

#[test]
fn the_study_is_a_draft_until_its_pilot_values_are_written() {
    let p = prereg();
    let s = &p["study"];
    assert_eq!(s["id"].as_str(), Some("regression-value-v1"));
    let status = s["status"].as_str().unwrap();
    let pilot = p["pilot"].as_table().unwrap();
    let pilot_keys = [
        "valid_share",
        "in_scope_share",
        "replayable_share",
        "candidates_per_repository",
        "cases_per_repository",
        "eligible_control_share",
    ];
    let unset = pilot_keys
        .iter()
        .filter(|k| pilot[**k].as_str() == Some("PILOT"))
        .count();
    match status {
        "draft" => assert_eq!(unset, pilot_keys.len(), "a draft writes no pilot value"),
        "locked-pre-data" => assert_eq!(unset, 0, "the lock writes every pilot value"),
        other => panic!("unexpected status {other}"),
    }
    // The pilot salt is 16 random bytes in hex, committed before any repository is assigned.
    let salt = pilot["pilot_salt"].as_str().unwrap();
    assert_eq!(salt.len(), 32);
    assert!(salt.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn v1_sample_sizes_match_rust_bounds() {
    let p = prereg();
    let pb = &p["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["wilson_n_max"]);
    let (rate, floor) = (float(&pb["v1_assumed_rate"]), float(&pb["v1_floor"]));
    assert_eq!(
        int(&pb["v1_point_estimate_n"]),
        sample_size_for_wilson_lower_bound(rate, floor, conf, n_max).unwrap()
    );
    let required =
        sample_size_for_wilson_lower_bound_power(rate, floor, conf, power, n_max).unwrap();
    assert_eq!(int(&pb["v1_required_n"]), required);
    let zero = zero_event_upper_bound(required, conf).unwrap();
    assert!((float(&pb["v1_zero_event_upper_bound"]) - zero).abs() < 1e-15);
}

#[test]
fn v2_sample_sizes_match_rust_bounds() {
    let p = prereg();
    let pb = &p["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["difference_n_max"]);
    let floor = float(&pb["v2_floor"]);
    let per_case = int(&pb["v2_controls_per_case"]);
    assert_eq!(per_case, int(&p["controls"]["controls_per_case"]));
    let (case, control) = (float(&pb["v2_case_rate"]), float(&pb["v2_control_rate"]));
    // v6's block rate: 41 of 232 pull requests.
    assert!((control - 41.0 / 232.0).abs() < 5e-4, "{control}");
    assert_eq!(
        int(&pb["v2_required_cases"]),
        sample_size_for_difference_lower_bound_power(
            case, control, floor, per_case, conf, power, n_max
        )
        .unwrap()
    );
    for row in pb["v2_sensitivity"].as_array().unwrap() {
        let (c, k) = (float(&row["case_rate"]), float(&row["control_rate"]));
        let got =
            sample_size_for_difference_lower_bound_power(c, k, floor, per_case, conf, power, n_max);
        match &row["required_cases"] {
            toml::Value::Integer(n) => assert_eq!(got.unwrap(), *n as usize, "({c}, {k})"),
            toml::Value::String(s) if s == "beyond-n-max" => {
                assert!(got.is_err(), "({c}, {k}) is said to be beyond n_max")
            }
            other => panic!("unexpected required_cases {other}"),
        }
    }
}

#[test]
fn human_subset_size_matches_rust_bounds() {
    let p = prereg();
    let h = &p["adjudication"]["human"];
    assert_eq!(
        int(&h["human_subset_n"]),
        kappa_sample_size(
            float(&h["planning_kappa"]),
            float(&h["planning_majority_share"]),
            float(&h["kappa_target_half_width"]),
            p["power_and_bounds"]["confidence_level"]
                .as_float()
                .unwrap(),
        )
        .unwrap()
    );
}

#[test]
fn rust_and_python_difference_bounds_agree() {
    for (k1, n1, k2, n2) in [
        (56, 70, 48, 80),
        (0, 10, 0, 20),
        (13, 45, 16, 90),
        (45, 45, 0, 90),
    ] {
        let (l, u) = newcombe_difference_interval(k1, n1, k2, n2, 0.95).unwrap();
        let py = power_audit_eval(&format!(
            "b.newcombe_difference_interval({k1}, {n1}, {k2}, {n2}, 0.95)"
        ));
        assert!(
            (py[0].as_f64().unwrap() - l).abs() < 1e-12,
            "{k1}/{n1} - {k2}/{n2}"
        );
        assert!(
            (py[1].as_f64().unwrap() - u).abs() < 1e-12,
            "{k1}/{n1} - {k2}/{n2}"
        );
    }
    for (n1, p1, n2, p2, floor) in [(20, 0.5, 40, 0.2, 0.0), (45, 0.3, 90, 0.177, 0.0)] {
        let r = difference_pass_probability(n1, p1, n2, p2, floor, 0.95).unwrap();
        let py = power_audit_eval(&format!(
            "b.difference_pass_probability({n1}, {p1}, {n2}, {p2}, {floor}, 0.95)"
        ));
        assert!(
            (py.as_f64().unwrap() - r).abs() < 1e-12,
            "({n1}, {p1}, {n2}, {p2})"
        );
    }
}

/// `--regression-bounds-json` prints the values the pre-registration records.
#[test]
fn python_regression_bounds_json_matches_the_preregistration() {
    let out = common::script_command("python3")
        .args([
            "-I",
            "scripts/benchmark_power_audit.py",
            "--regression-bounds-json",
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
    let p = prereg();
    let pb = &p["power_and_bounds"];
    let v1 = &json["v1_value"];
    assert_eq!(
        v1["assumed_rate"].as_f64(),
        pb["v1_assumed_rate"].as_float()
    );
    assert_eq!(v1["floor"].as_f64(), pb["v1_floor"].as_float());
    assert_eq!(
        v1["required_n_point"].as_u64(),
        Some(int(&pb["v1_point_estimate_n"]) as u64)
    );
    assert_eq!(
        v1["required_n_power"].as_u64(),
        Some(int(&pb["v1_required_n"]) as u64)
    );
    let v2 = &json["v2_discrimination"];
    assert_eq!(v2["case_rate"].as_f64(), pb["v2_case_rate"].as_float());
    assert_eq!(
        v2["control_rate"].as_f64(),
        pb["v2_control_rate"].as_float()
    );
    assert_eq!(v2["floor"].as_f64(), pb["v2_floor"].as_float());
    assert_eq!(
        v2["controls_per_case"].as_u64(),
        Some(int(&pb["v2_controls_per_case"]) as u64)
    );
    assert_eq!(
        v2["n_max"].as_u64(),
        Some(int(&pb["difference_n_max"]) as u64)
    );
    assert_eq!(
        v2["required_cases_power"].as_u64(),
        Some(int(&pb["v2_required_cases"]) as u64)
    );
}
