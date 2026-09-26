//! End-to-end tests for `bench-regression` citation freshness and `paired-ratio` mode,
//! driving the real binary against throwaway repositories. `gh` is replaced by a script
//! that serves recorded API responses, so nothing here touches the network.

mod common;
use common::*;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---- helpers --------------------------------------------------------------------------

/// The GitHub API the citation checks read: a loopback forge with canned responses.
type FakeGh = FakeForge;

fn commit_at(repo: &Repo, message: &str, epoch: i64) {
    repo.git(&["add", "-A"]);
    let date = format!("{epoch} +0000");
    let out = Command::new("git")
        .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(["commit", "-q", "--allow-empty", "-m", message])
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_DATE", &date)
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const RUN_URL: &str = "https://github.com/acme/widgets/actions/runs/4401";
const RUN_API: &str = "repos/acme/widgets/actions/runs/4401";

fn bench_files(dir: &Path) -> (PathBuf, PathBuf) {
    let base = dir.join("base_bench.json");
    let head = dir.join("head_bench.json");
    std::fs::write(&base, r#"{"arms": {"sync_map_insert": 1000}}"#).unwrap();
    std::fs::write(&head, r#"{"arms": {"sync_map_insert": 1060}}"#).unwrap();
    (base, head)
}

fn sourced_args<'a>(base: &'a str, head: &'a str, extra_config: &'a str) -> Vec<String> {
    vec![
        "check".into(),
        "--format".into(),
        "json".into(),
        "--base".into(),
        "main".into(),
        "--suite".into(),
        "bench".into(),
        "--bench-base-file".into(),
        base.into(),
        "--bench-head-file".into(),
        head.into(),
        "--config-override".into(),
        format!(
            "[gates.bench-regression]\nseverity = \"error\"\ntolerance_pct = 5.0\nrequire_sourced_override = true\n{extra_config}"
        ),
    ]
}

fn run_with(repo: &Repo, args: &[String], env: &[(&str, &str)]) -> Run {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.run(&args, env)
}

fn notes(run: &Run) -> Vec<String> {
    run.outcome("bench-regression")["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

// ---- citation freshness -----------------------------------------------------------------

#[cfg(unix)]
#[test]
fn sourced_override_citing_a_fresh_run_is_admitted() {
    let repo = Repo::new();
    repo.commit("init");
    let head_sha = repo.git_output(&["rev-parse", "HEAD"]);
    let gh = FakeGh::start();
    gh.serve(
        RUN_API,
        json!({"conclusion": "success", "head_sha": head_sha}),
    );
    gh.serve(
        &format!("repos/acme/widgets/compare/{head_sha}...{head_sha}"),
        json!({"status": "identical"}),
    );
    let (base, head) = bench_files(repo.path());
    let args = sourced_args(base.to_str().unwrap(), head.to_str().unwrap(), "");
    let body = format!("allow-regression: sync_map_insert trade measured in {RUN_URL}");
    let run = run_with(
        &repo,
        &args,
        &[
            ("PR_BODY", body.as_str()),
            ("DISCIPLINE_FORGE_API_URL", gh.url().as_str()),
        ],
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(
        run.outcome("bench-regression")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[cfg(unix)]
#[test]
fn sourced_override_citing_a_cancelled_run_leaves_the_gate_armed() {
    let repo = Repo::new();
    repo.commit("init");
    let gh = FakeGh::start();
    gh.serve(
        RUN_API,
        json!({"conclusion": "cancelled", "head_sha": "dfc5f456"}),
    );
    let (base, head) = bench_files(repo.path());
    let args = sourced_args(base.to_str().unwrap(), head.to_str().unwrap(), "");
    let body = format!("allow-regression: sync_map_insert +6% measured in CI run {RUN_URL}");
    let run = run_with(
        &repo,
        &args,
        &[
            ("PR_BODY", body.as_str()),
            ("DISCIPLINE_FORGE_API_URL", gh.url().as_str()),
        ],
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    let titles = run.titles("bench-regression");
    assert!(
        titles.contains(
            &"Regression Override Void (Citation Does Not Measure This Code)".to_string()
        ),
        "{titles:?}"
    );
    assert!(titles.contains(&"Deterministic Counter Regressed".to_string()));
    let v = run.violations("bench-regression");
    assert!(v.iter().any(|v| v["message"]
        .as_str()
        .unwrap()
        .contains("run 4401 concluded `cancelled`")));
}

#[cfg(unix)]
#[test]
fn failure_run_is_admitted_only_when_its_measurement_job_reached_the_guard() {
    let repo = Repo::new();
    repo.commit("init");
    let head_sha = repo.git_output(&["rev-parse", "HEAD"]);
    let gh = FakeGh::start();
    gh.serve(
        RUN_API,
        json!({"conclusion": "failure", "head_sha": head_sha}),
    );
    gh.serve(
        &format!("repos/acme/widgets/compare/{head_sha}...{head_sha}"),
        json!({"status": "identical"}),
    );
    let step = |n: u64, name: &str, c: &str| json!({"number": n, "name": name, "conclusion": c});
    let jobs = |bench_step: &str| {
        json!({"total_count": 1, "jobs": [{
            "name": "Perf / Counts",
            "conclusion": "failure",
            "steps": [
                step(1, "Set up job", "success"),
                step(2, "Counts (this branch)", bench_step),
                step(3, "Enforce Regression Guard", "failure"),
            ]
        }]})
    };
    let jobs_api = format!("{RUN_API}/jobs?per_page=100&page=1");
    let (base, head) = bench_files(repo.path());
    let config = "citation_measurement_jobs = [{ job = \"Perf / Counts\", guard = \"Enforce Regression Guard\" }]\n";
    let args = sourced_args(base.to_str().unwrap(), head.to_str().unwrap(), config);
    let body = format!("allow-regression: sync_map_insert +6% counts in {RUN_URL}");
    let bin = gh.url();
    let env_ok = [
        ("PR_BODY", body.as_str()),
        ("DISCIPLINE_FORGE_API_URL", bin.as_str()),
    ];

    // The guard tripped on numbers the job measured: the run holds the report.
    gh.serve(&jobs_api, jobs("success"));
    let run = run_with(&repo, &args, &env_ok);
    assert_eq!(run.code, 0, "{}", run.stdout);

    // The benchmark step crashed first: the guard failed on output that never existed.
    gh.serve(&jobs_api, jobs("failure"));
    let run = run_with(&repo, &args, &env_ok);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(run
        .violations("bench-regression")
        .iter()
        .any(|v| v["message"]
            .as_str()
            .unwrap()
            .contains("concluded `failure` before the regression guard ran")));
}

#[test]
fn sourced_override_is_undecidable_without_network_and_stays_armed() {
    let repo = Repo::new();
    repo.commit("init");
    let (base, head) = bench_files(repo.path());
    let args = sourced_args(base.to_str().unwrap(), head.to_str().unwrap(), "");
    let body = format!("allow-regression: sync_map_insert trade measured in {RUN_URL}");
    let run = run_with(&repo, &args, &[("PR_BODY", body.as_str())]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(run
        .titles("bench-regression")
        .contains(&"Regression Override Unverified (Citation Undecidable)".to_string()));
    assert!(notes(&run)
        .iter()
        .any(|n| n.contains("citation not verified") && n.contains("run 4401")));
    assert!(run.outcome("bench-regression")["overrides"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn artifact_citation_regenerated_before_the_code_change_is_void() {
    let repo = Repo::new();
    repo.write("results/fallback.json", "{\"reallocations\": 100}\n");
    repo.write("crates/core/src/lib.rs", "pub fn f() {}\n");
    commit_at(&repo, "chore: artifact", 1_700_000_000);
    repo.git(&["branch", "-f", "main", "HEAD"]);
    repo.write("crates/core/src/lib.rs", "pub fn f() { let _ = 1; }\n");
    commit_at(
        &repo,
        "feat: change the code the artifact describes",
        1_700_000_100,
    );

    let (base, head) = bench_files(repo.path());
    let config = "citation_source_paths = [\"crates\"]\n";
    let args = sourced_args(base.to_str().unwrap(), head.to_str().unwrap(), config);
    let body = "allow-regression: sync_map_insert trade, see results/fallback.json";
    let run = run_with(&repo, &args, &[("PR_BODY", body)]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(run.violations("bench-regression").iter().any(|v| v["message"]
        .as_str()
        .unwrap()
        .contains("`results/fallback.json` was last regenerated before this branch's newest change under crates")));

    // Regenerated after the change: admitted.
    repo.write("results/fallback.json", "{\"reallocations\": 77}\n");
    commit_at(&repo, "chore: regenerate the artifact", 1_700_000_200);
    let run = run_with(&repo, &args, &[("PR_BODY", body)]);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

// ---- paired-ratio mode ------------------------------------------------------------------

const BASELINE: &str = "benchmarks/ratio-baseline.json";
const JITTER: [f64; 10] = [
    -0.002, 0.001, 0.0, 0.002, -0.001, 0.001, 0.0, -0.002, 0.002, 0.0,
];

fn rounds(center: f64, twin: f64) -> Value {
    Value::Array(
        JITTER
            .iter()
            .enumerate()
            .map(|(i, j)| {
                json!({
                    "subject": center * (1.0 + j) * twin,
                    "twin": twin,
                    "order": if i % 2 == 0 { "subject-first" } else { "twin-first" }
                })
            })
            .collect(),
    )
}

fn control_rounds(center: f64) -> Value {
    Value::Array(
        JITTER
            .iter()
            .enumerate()
            .map(|(i, j)| {
                json!({
                    "a": center * (1.0 + j),
                    "b": 1.0,
                    "order": if i % 2 == 0 { "a-first" } else { "b-first" }
                })
            })
            .collect(),
    )
}

fn ratio_run(commit: &str, runner: &str, cells: &[(&str, f64)], control: f64) -> Value {
    let cells: serde_json::Map<String, Value> = cells
        .iter()
        .map(|(id, c)| (id.to_string(), json!({ "rounds": rounds(*c, 1.0) })))
        .collect();
    json!({
        "schema": "discipline-bench-ratio/v1",
        "provenance": {
            "platform": "linux-glibc-x86_64",
            "runner_class": "ubuntu-24.04",
            "runner_id": runner,
            "commit": commit,
            "twin": {"identity": "reference-build", "version": "1.0.5"}
        },
        "axes": {"timing": {
            "adverse": "up",
            "cells": cells,
            "controls": {"ctl.read": {"rounds": control_rounds(control)}}
        }}
    })
}

fn write_json(path: &Path, v: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// Derives a baseline with the real `discipline bench derive` from three same-commit runs
/// whose cells scatter by up to 2%, and returns its text.
fn derived_baseline(repo: &Repo) -> String {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (i, (a, b)) in [(1.00, 2.00), (1.01, 2.02), (0.99, 1.98)]
        .iter()
        .enumerate()
    {
        let p = dir.path().join(format!("run{i}.json"));
        write_json(
            &p,
            &ratio_run(
                "abc123",
                &format!("runner-{i}"),
                &[("map_get", *a), ("map_insert", *b)],
                1.0,
            ),
        );
        paths.push(p.display().to_string());
    }
    let out_path = dir.path().join("baseline.json");
    let mut args = vec!["bench", "derive"];
    args.extend(paths.iter().map(String::as_str));
    let out_str = out_path.display().to_string();
    args.extend(["--baseline", out_str.as_str()]);
    let run = repo.run(&args, &[]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    std::fs::read_to_string(out_path).unwrap()
}

fn paired_config() -> String {
    format!(
        "[gates.bench-regression]\nseverity = \"error\"\nmode = \"paired-ratio\"\nratio_baseline = \"{BASELINE}\"\n"
    )
}

fn paired_check(repo: &Repo, run_file: &Path, env: &[(&str, &str)]) -> Run {
    let config = paired_config();
    repo.run(
        &[
            "check",
            "--format",
            "json",
            "--base",
            "main",
            "--suite",
            "bench",
            "--bench-head-file",
            run_file.to_str().unwrap(),
            "--config-override",
            &config,
        ],
        env,
    )
}

fn paired_repo() -> (Repo, String) {
    let repo = Repo::new();
    let baseline = derived_baseline(&repo);
    repo.commit_base(BASELINE, &baseline, "chore: ratio baseline");
    (repo, baseline)
}

#[test]
fn bench_derive_records_derived_floors_and_twin_identity() {
    let repo = Repo::new();
    let text = derived_baseline(&repo);
    let b: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(b["schema"], "discipline-bench-ratio-baseline/v1");
    let plat = &b["platforms"]["linux-glibc-x86_64"];
    assert_eq!(plat["twin"]["version"], "1.0.5");
    assert_eq!(plat["derived_from"]["runs"], 3);
    assert_eq!(plat["derived_from"]["distinct_runners"], 3);
    let derived = &plat["axes"]["timing"]["derived"];
    assert!(derived["axis_floor_pct"].as_f64().unwrap() >= 1.0);
    assert!(plat["axes"]["timing"]["cells"]["map_get"]["floor_pct"]
        .as_f64()
        .is_some());

    // A single run is refused: a floor needs repeated runs of the same code.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("one.json");
    write_json(&p, &ratio_run("abc123", "r", &[("map_get", 1.0)], 1.0));
    let run = repo.run(&["bench", "derive", p.to_str().unwrap()], &[]);
    assert_ne!(run.code, 0);
}

#[test]
fn paired_ratio_clean_run_passes() {
    let (repo, _) = paired_repo();
    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    write_json(
        &run_file,
        &ratio_run(
            "def456",
            "r9",
            &[("map_get", 1.0), ("map_insert", 2.0)],
            1.0,
        ),
    );
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.outcome("bench-regression")["examined"], 2);
}

#[test]
fn paired_ratio_true_regression_fails() {
    let (repo, _) = paired_repo();
    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    write_json(
        &run_file,
        &ratio_run(
            "def456",
            "r9",
            &[("map_get", 1.30), ("map_insert", 2.0)],
            1.0,
        ),
    );
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(
        run.titles("bench-regression"),
        vec!["Paired Ratio Regressed"]
    );
    assert!(run.violations("bench-regression")[0]["message"]
        .as_str()
        .unwrap()
        .contains("timing/map_get"));
}

#[test]
fn paired_ratio_contaminated_control_is_not_comparable_never_a_regression() {
    let (repo, _) = paired_repo();
    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    write_json(
        &run_file,
        &ratio_run(
            "def456",
            "r9",
            &[("map_get", 1.30), ("map_insert", 2.0)],
            1.25,
        ),
    );
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    let titles = run.titles("bench-regression");
    assert!(
        titles.contains(&"Paired Ratio Not Comparable".to_string()),
        "{titles:?}"
    );
    assert!(
        !titles.contains(&"Paired Ratio Regressed".to_string()),
        "{titles:?}"
    );
}

#[test]
fn paired_ratio_run_without_controls_is_refused() {
    let (repo, _) = paired_repo();
    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    let mut v = ratio_run("def456", "r9", &[("map_get", 1.0)], 1.0);
    v["axes"]["timing"]["controls"] = json!({});
    write_json(&run_file, &v);
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("no in-situ control cells"),
        "{}",
        run.stderr
    );
}

#[test]
fn paired_ratio_baseline_is_read_from_the_base_ref_and_loosening_is_flagged() {
    let (repo, baseline) = paired_repo();
    // Head widens every floor so the regression would pass against it.
    let mut loose: Value = serde_json::from_str(&baseline).unwrap();
    for cell in loose["platforms"]["linux-glibc-x86_64"]["axes"]["timing"]["cells"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        cell["floor_pct"] = json!(49.0);
    }
    repo.write(BASELINE, &serde_json::to_string_pretty(&loose).unwrap());
    repo.commit("chore: widen floors");

    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    write_json(
        &run_file,
        &ratio_run(
            "def456",
            "r9",
            &[("map_get", 1.30), ("map_insert", 2.0)],
            1.0,
        ),
    );
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    let titles = run.titles("bench-regression");
    assert!(
        titles.contains(&"Paired Ratio Regressed".to_string()),
        "base floors govern: {titles:?}"
    );
    assert!(
        titles.contains(&"Ratio Baseline Loosened".to_string()),
        "{titles:?}"
    );

    // A scoped directive naming the baseline lifts the loosening and is audited; the
    // regression is still judged against the base ref's floors.
    let body = format!("allow-regression: {BASELINE} re-derived after the runner image change");
    let run = paired_check(&repo, &run_file, &[("PR_BODY", body.as_str())]);
    let titles = run.titles("bench-regression");
    assert!(
        !titles.contains(&"Ratio Baseline Loosened".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"Paired Ratio Regressed".to_string()),
        "{titles:?}"
    );
    let overrides = run.outcome("bench-regression")["overrides"].clone();
    assert!(overrides
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["reason"].as_str().unwrap().contains("re-derived")));
}

#[test]
fn paired_ratio_baseline_changed_with_source_is_a_violation_unless_named() {
    let (repo, baseline) = paired_repo();
    let mut tighter: Value = serde_json::from_str(&baseline).unwrap();
    tighter["platforms"]["linux-glibc-x86_64"]["derived_from"]["runs"] = json!(3);
    repo.write(
        BASELINE,
        &(serde_json::to_string_pretty(&tighter).unwrap() + "\n\n"),
    );
    repo.write("src/lib.rs", &format!("{GOOD_LIB}\npub fn g() {{}}\n"));
    repo.commit("feat: change code and baseline together");

    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    write_json(
        &run_file,
        &ratio_run(
            "def456",
            "r9",
            &[("map_get", 1.0), ("map_insert", 2.0)],
            1.0,
        ),
    );
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(
        run.titles("bench-regression"),
        vec!["Ratio Baseline Changed With Source"]
    );
    let body =
        format!("allow-regression: {BASELINE} refreshed with the allocator change it measures");
    let run = paired_check(&repo, &run_file, &[("PR_BODY", body.as_str())]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert_eq!(
        run.outcome("bench-regression")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn paired_ratio_twin_version_change_invalidates_the_baseline() {
    let (repo, _) = paired_repo();
    let dir = tempfile::tempdir().unwrap();
    let run_file = dir.path().join("run.json");
    let mut v = ratio_run(
        "def456",
        "r9",
        &[("map_get", 1.0), ("map_insert", 2.0)],
        1.0,
    );
    v["provenance"]["twin"]["version"] = json!("1.0.6");
    write_json(&run_file, &v);
    let run = paired_check(&repo, &run_file, &[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(run.violations("bench-regression")[0]["message"]
        .as_str()
        .unwrap()
        .contains("baseline invalidated by twin change"));
}
