//! The head-tests-on-base study (#1071, `docs/benchmarks/head-tests-on-base-preregistration.toml`):
//! the study script's self-checks run, and the numbers the pre-registration records are the
//! ones its committed population scan and verdict functions give.

mod common;

use serde_json::Value;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn prereg() -> toml::Value {
    let text = std::fs::read_to_string(
        root().join("docs/benchmarks/head-tests-on-base-preregistration.toml"),
    )
    .expect("read pre-registration");
    toml::from_str(&text).expect("parse pre-registration")
}

#[cfg(unix)]
#[test]
fn head_tests_on_base_script_passes_its_self_checks() {
    let out = common::script_command("python3")
        .args(["-I", "scripts/head_tests_on_base.py", "--test"])
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "head_tests_on_base.py --test failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The population counts in the pre-registration are those of `population.json`.
#[test]
fn recorded_population_matches_the_committed_scan() {
    let scan: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root().join("docs/benchmarks/results-head-tests-on-base/population.json"),
        )
        .expect("read population.json"),
    )
    .expect("parse population.json");
    let rows = scan["pull_requests"].as_array().expect("pull_requests");
    let in_population = rows
        .iter()
        .filter(|r| r["in_population"].as_bool() == Some(true))
        .count();
    let pre = prereg();
    let pop = &pre["population"];
    assert_eq!(
        pop["total_pull_requests"].as_integer(),
        Some(rows.len() as i64)
    );
    assert_eq!(
        pop["in_population"].as_integer(),
        Some(in_population as i64)
    );
    let per_repo = pop["in_population_per_repo"]
        .as_table()
        .expect("in_population_per_repo");
    let recorded: i64 = per_repo.values().map(|v| v.as_integer().unwrap()).sum();
    assert_eq!(recorded, in_population as i64);
    for (repo, count) in per_repo {
        let scanned = rows
            .iter()
            .filter(|r| r["repo"] == repo.as_str() && r["in_population"].as_bool() == Some(true))
            .count();
        assert_eq!(count.as_integer(), Some(scanned as i64), "{repo}");
    }
}

/// The `majority_threshold` table in the pre-registration is what the script computes.
#[cfg(unix)]
#[test]
fn recorded_majority_thresholds_are_computed() {
    let pre = prereg();
    let table = pre["verdict"]["majority_threshold"]
        .as_table()
        .expect("majority_threshold");
    assert!(!table.is_empty());
    for (n, k) in table {
        let program = format!(
            "import sys\nsys.path.insert(0, 'scripts')\nimport head_tests_on_base as h\nprint(h.majority_threshold({n}))\n"
        );
        let out = common::script_command("python3")
            .args(["-I", "-c", &program])
            .current_dir(root())
            .output()
            .expect("run python3");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(got, k.as_integer().unwrap().to_string(), "n = {n}");
    }
}
