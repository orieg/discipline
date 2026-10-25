//! Step-0 bounds of the ImpossibleBench recall study (#838,
//! `docs/benchmarks/recall-preregistration.toml`): the Python functions in
//! `scripts/benchmark_power_audit.py` are run and their values pinned here, and the
//! study scripts' self-checks are run, so a change to either fails `cargo test`. The Rust
//! mirrors in `src/guards/perf/bounds.rs` are compared with the Python on a grid here and
//! pinned on their own in `tests/test_benchmark_power_bounds.rs`.

mod common;

use discipline::guards::perf::bounds::{
    sample_size_for_wilson_lower_bound, sample_size_for_wilson_lower_bound_power,
    trajectories_required, wilson_pass_probability,
};
use serde_json::Value;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
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

/// The ValueError message `expr` raises, or a panic when it returns.
fn power_audit_error(expr: &str) -> String {
    let program = format!(
        "import sys\nsys.path.insert(0, 'scripts')\nimport benchmark_power_audit as b\ntry:\n    {expr}\nexcept ValueError as e:\n    print('ValueError:', e)\n    sys.exit(0)\nprint('returned')\nsys.exit(1)\n"
    );
    let out = common::script_command("python3")
        .args(["-I", "-c", &program])
        .current_dir(root())
        .output()
        .expect("run python3");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{expr} did not raise ValueError: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn wilson_lower_bound_sample_sizes_match_the_pinned_values() {
    // (assumed recall, floor) -> (point-estimate n, power-0.80 n), confidence 0.95.
    for (expected, floor, point, power) in [
        (0.90, 0.80, 68, 112),
        (0.90, 0.75, 37, 58),
        (0.85, 0.70, 39, 69),
    ] {
        let got_point = power_audit_eval(&format!(
            "b.sample_size_for_wilson_lower_bound({expected}, {floor}, 0.95, 2000)"
        ));
        assert_eq!(
            got_point,
            Value::from(point),
            "point n at ({expected}, {floor})"
        );
        let got_power = power_audit_eval(&format!(
            "b.sample_size_for_wilson_lower_bound_power({expected}, {floor}, 0.95, 0.80, 2000)"
        ));
        assert_eq!(
            got_power,
            Value::from(power),
            "power n at ({expected}, {floor})"
        );
    }
}

#[test]
fn wilson_pass_probability_matches_the_exact_binomial_tail() {
    // Cross-checked with exact rational arithmetic (`fractions.Fraction`, `math.comb`).
    for (n, p, floor, expected) in [
        (112, 0.90, 0.80, 0.850_602_542_593_9),
        (111, 0.90, 0.80, 0.781_664_997_762_1),
        (100, 0.90, 0.80, 0.801_821_112_640_4),
        (58, 0.90, 0.75, 0.878_528_651_867_7),
        (69, 0.85, 0.70, 0.855_412_199_075_6),
    ] {
        let got = power_audit_eval(&format!(
            "b.wilson_pass_probability({n}, {p}, {floor}, 0.95)"
        ))
        .as_f64()
        .unwrap();
        assert!(
            (got - expected).abs() < 1e-10,
            "pass probability at n={n}, p={p}, floor={floor}: {got} != {expected}"
        );
    }
}

/// The monotonicity choice: the first size reaching the power (100) is not the
/// requirement, because 101-105 and 110-111 fall below it again.
#[test]
fn the_power_requirement_is_the_last_unbroken_run_not_the_first_crossing() {
    let below: Value = power_audit_eval(
        "[n for n in range(100, 113) if b.wilson_pass_probability(n, 0.9, 0.8, 0.95) < 0.8]",
    );
    assert_eq!(
        below,
        serde_json::json!([101, 102, 103, 104, 105, 110, 111])
    );
    let gaps: Value = power_audit_eval(
        "[n for n in range(54, 69) if b.wilson_score_interval(b._round_half_up(0.9 * n), n, 0.95)[0] < 0.8]",
    );
    assert_eq!(gaps, serde_json::json!([56, 57, 58, 59, 60, 66, 67]));
}

#[test]
fn trajectories_required_rounds_up_but_not_on_float_noise() {
    for (args, expected) in [
        ("112, 0.30, 0.50, 0.90", 830),
        ("100, 0.50, 0.50, 1.0", 400),
        ("10, 0.10, 1.0, 1.0", 100),
        ("1, 1.0, 1.0, 1.0", 1),
    ] {
        let got = power_audit_eval(&format!("b.trajectories_required({args})"));
        assert_eq!(got, Value::from(expected), "trajectories_required({args})");
    }
}

#[test]
fn recall_bounds_reject_invalid_inputs() {
    for expr in [
        "b.wilson_pass_probability(0, 0.9, 0.8)",
        "b.wilson_pass_probability(10, 1.0, 0.8)",
        "b.sample_size_for_wilson_lower_bound(0.80, 0.80, 0.95, 2000)",
        "b.sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 10)",
        "b.sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 1.0, 2000)",
        "b.sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 50)",
        "b.trajectories_required(0, 0.5, 0.5, 0.5)",
        "b.trajectories_required(10, 0.0, 0.5, 0.5)",
    ] {
        let msg = power_audit_error(expr);
        assert!(msg.starts_with("ValueError:"), "{expr}: {msg}");
    }
}

/// The Rust mirrors in `src/guards/perf/bounds.rs` return what the Python returns: the
/// pass probability within 1e-9 on a grid of sizes, proportions and floors, and the same
/// integers for the sample sizes and run counts.
#[test]
fn rust_and_python_recall_bounds_agree_on_a_grid() {
    let sizes = [1usize, 10, 37, 54, 58, 69, 100, 105, 111, 112, 250, 2000];
    let props = [0.70, 0.85, 0.90];
    let floors = [0.65, 0.70, 0.75, 0.80];
    let pairs = [
        (0.90, 0.80),
        (0.90, 0.75),
        (0.85, 0.70),
        (0.80, 0.70),
        (0.95, 0.90),
    ];
    let runs = [
        (112usize, 0.30, 0.50, 0.90),
        (100, 0.50, 0.50, 1.0),
        (10, 0.10, 1.0, 1.0),
        (69, 0.17, 0.33, 0.85),
    ];
    let list = |xs: &[String]| format!("[{}]", xs.join(", "));
    let mut grid = Vec::new();
    for &n in &sizes {
        for &p in &props {
            for &f in &floors {
                grid.push((n, p, f));
            }
        }
    }
    let pass_exprs: Vec<String> = grid
        .iter()
        .map(|(n, p, f)| format!("b.wilson_pass_probability({n}, {p}, {f}, 0.95)"))
        .collect();
    let point_exprs: Vec<String> = pairs
        .iter()
        .map(|(e, f)| format!("b.sample_size_for_wilson_lower_bound({e}, {f}, 0.95, 2000)"))
        .collect();
    let power_exprs: Vec<String> = pairs
        .iter()
        .map(|(e, f)| {
            format!("b.sample_size_for_wilson_lower_bound_power({e}, {f}, 0.95, 0.80, 2000)")
        })
        .collect();
    let run_exprs: Vec<String> = runs
        .iter()
        .map(|(n, a, b, c)| format!("b.trajectories_required({n}, {a}, {b}, {c})"))
        .collect();
    let python = power_audit_eval(&format!(
        "[{}, {}, {}, {}]",
        list(&pass_exprs),
        list(&point_exprs),
        list(&power_exprs),
        list(&run_exprs)
    ));
    let column = |i: usize| python[i].as_array().expect("JSON list").clone();

    for ((n, p, f), py) in grid.iter().zip(column(0)) {
        let rust = wilson_pass_probability(*n, *p, *f, 0.95).unwrap();
        let py = py.as_f64().unwrap();
        assert!(
            (rust - py).abs() < 1e-9,
            "pass probability at n={n}, p={p}, floor={f}: Rust {rust} vs Python {py}"
        );
    }
    for (((e, f), point), power) in pairs.iter().zip(column(1)).zip(column(2)) {
        assert_eq!(
            Value::from(sample_size_for_wilson_lower_bound(*e, *f, 0.95, 2000).unwrap()),
            point,
            "point n at ({e}, {f})"
        );
        assert_eq!(
            Value::from(
                sample_size_for_wilson_lower_bound_power(*e, *f, 0.95, 0.80, 2000).unwrap()
            ),
            power,
            "power n at ({e}, {f})"
        );
    }
    for ((n, a, b, c), py) in runs.iter().zip(column(3)) {
        assert_eq!(
            Value::from(trajectories_required(*n, *a, *b, *c).unwrap()),
            py,
            "trajectories_required({n}, {a}, {b}, {c})"
        );
    }
}

/// The generated audit's recall rows are the values pinned above.
#[test]
fn recall_bounds_json_is_the_pinned_grid() {
    let out = common::script_command("python3")
        .args([
            "-I",
            "scripts/benchmark_power_audit.py",
            "--recall-bounds-json",
        ])
        .current_dir(root())
        .output()
        .expect("run benchmark_power_audit.py");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Value = serde_json::from_slice(&out.stdout).unwrap();
    // Percentages, so the comparison is on integers.
    let pct = |v: &Value| (v.as_f64().unwrap() * 100.0).round() as i64;
    let got: Vec<(i64, i64, i64, i64)> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                pct(&r["assumed_recall"]),
                pct(&r["recall_floor"]),
                r["required_n_point"].as_i64().unwrap(),
                r["required_n_power"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![(90, 80, 68, 112), (90, 75, 37, 58), (85, 70, 39, 69)]
    );
}

/// Every study script's embedded self-check passes (the recall scripts build git
/// repositories and run a stub binary, hence unix only).
#[cfg(unix)]
#[test]
fn recall_study_scripts_pass_their_self_checks() {
    for script in [
        "scripts/benchmark_power_audit.py",
        "scripts/recall_pairs.py",
        "scripts/recall_strata.py",
        "scripts/recall_replay.py",
    ] {
        let out = common::script_command("python3")
            .args(["-I", script, "--self-check"])
            .current_dir(root())
            .output()
            .expect("run python3");
        assert!(
            out.status.success(),
            "{script} --self-check failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
