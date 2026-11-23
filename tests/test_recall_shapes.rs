//! Part 3 of `base-tests-v1` (#1052, `docs/benchmarks/base-tests-preregistration.toml`,
//! `[q3_added_value]`): `scripts/recall_shapes.py` splits #838's 212 confirmatory gaming runs
//! by the shape of the injected test file, from the committed #838 files, into the counts the
//! 2026-11-09 breakdown on #838 records (77 well-formed, 99 broken, 36 with no test-file hunk),
//! and agrees per run with that breakdown's whole-file labels.

mod common;

use serde_json::Value;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_script(args: &[&str]) -> std::process::Output {
    common::script_command("python3")
        .args(["-I", "scripts/recall_shapes.py"])
        .args(args)
        .current_dir(root())
        .output()
        .expect("run python3")
}

fn shapes() -> Value {
    let out = run_script(&[]);
    assert!(
        out.status.success(),
        "recall_shapes.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("recall_shapes.py prints JSON")
}

fn cell(v: &Value) -> (u64, u64) {
    (
        v["runs"].as_u64().expect("runs"),
        v["blocked"].as_u64().expect("blocked"),
    )
}

fn ci(v: &Value) -> (f64, f64) {
    (
        v["ci95"][0].as_f64().expect("ci95 low"),
        v["ci95"][1].as_f64().expect("ci95 high"),
    )
}

#[cfg(unix)]
#[test]
fn recall_shapes_script_passes_its_self_checks() {
    let out = run_script(&["--self-check"]);
    assert!(
        out.status.success(),
        "recall_shapes.py --self-check failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The shape counts, block counts and Wilson intervals are the 2026-11-09 table on #838.
#[cfg(unix)]
#[test]
fn shapes_reproduce_the_838_breakdown_from_committed_files() {
    let v = shapes();
    let s = &v["shapes"];
    assert_eq!(cell(&s["well-formed"]), (77, 24));
    assert_eq!(cell(&s["broken"]), (99, 16));
    assert_eq!(cell(&s["no-test-file-hunk"]), (36, 4));
    assert_eq!(cell(&v["all"]), (212, 44));
    assert_eq!(s["broken"]["does_not_parse"].as_u64(), Some(61));
    assert_eq!(s["broken"]["outside_any_function"].as_u64(), Some(38));
    let round = |(lo, hi): (f64, f64)| {
        (
            (lo * 1000.0).round() / 1000.0,
            (hi * 1000.0).round() / 1000.0,
        )
    };
    assert_eq!(round(ci(&s["well-formed"])), (0.219, 0.422));
    assert_eq!(round(ci(&s["broken"])), (0.102, 0.247));
    assert_eq!(round(ci(&s["no-test-file-hunk"])), (0.044, 0.253));
    assert_eq!(round(ci(&v["all"])), (0.158, 0.267));
}

/// Per run, the hunk-local reading gives the shape the whole-file `ast` reading gave on the lab
/// host (`shapes-whole-file.json`), not only the same totals.
#[cfg(unix)]
#[test]
fn shapes_agree_per_run_with_the_whole_file_labels() {
    let out = run_script(&["--check-labels"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "recall_shapes.py --check-labels failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("212 / 212 runs agree"),
        "unexpected output: {stdout}"
    );
}

/// The pre-registration's Part 3 population states the counts the script computes.
#[cfg(unix)]
#[test]
fn preregistration_states_the_computed_shape_counts() {
    let raw =
        std::fs::read_to_string(root().join("docs/benchmarks/base-tests-preregistration.toml"))
            .expect("read pre-registration");
    let pre: toml::Value = toml::from_str(&raw).expect("parse pre-registration");
    let population = pre["q3_added_value"]["population"]
        .as_str()
        .expect("q3 population")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let v = shapes();
    let n = |k: &str| v["shapes"][k]["runs"].as_u64().expect("runs");
    for want in [
        format!("The {} confirmatory gaming runs", v["all"]["runs"]),
        format!("well-formed ({})", n("well-formed")),
        format!("broken by the injection ({})", n("broken")),
        format!("no test-file hunk ({})", n("no-test-file-hunk")),
    ] {
        assert!(
            population.contains(&want),
            "`[q3_added_value].population` does not state `{want}`: {population}"
        );
    }
}
