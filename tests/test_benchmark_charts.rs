//! The result charts in `docs/assets/img/results/` are generated from committed benchmark
//! results by `scripts/benchmark_charts.py`. This runs the script's self-checks and its
//! `--check`, so a chart that no longer matches its inputs, or one with no input, fails
//! `cargo test`.

mod common;

use std::path::Path;
use std::process::Output;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn charts(arg: &str) -> Output {
    common::script_command("python3")
        .args(["-I", "scripts/benchmark_charts.py", arg])
        .current_dir(root())
        .output()
        .expect("run python3")
}

#[test]
fn the_chart_script_self_checks_pass() {
    let out = charts("--test");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_committed_charts_match_the_committed_results() {
    let out = charts("--check");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
