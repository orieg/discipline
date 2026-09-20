//! End-to-end integration tests for grandfathering baseline mode (`discipline baseline`).

mod common;
use common::Repo;

const WEAKENING_PR: &str =
    "allow-gate-weakening: baseline grandfathering pre-existing findings for adoption";

#[test]
fn test_baseline_write_and_grandfathering_passes() {
    let repo = Repo::new();
    // Introduce an unsafe block without safety comment on work branch
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: add unsafe without comment");

    // Without baseline, check MUST fail
    let initial_run = repo.check(&[]);
    assert_eq!(initial_run.code, 1, "{}", initial_run.stdout);
    assert_eq!(initial_run.json()["errors"], 1);
    assert_eq!(initial_run.json()["baselined"], 0);

    // Record findings into baseline
    let baseline_run = repo.run(&["baseline", "--write"], &[]);
    assert_eq!(baseline_run.code, 0, "{}", baseline_run.stdout);
    assert!(repo.file("discipline-baseline.toml").exists());

    // In a PR introducing the baseline, allow-gate-weakening: baseline authorizes the new baseline file
    let subsequent_run = repo.check_with_pr(&[], WEAKENING_PR);
    assert_eq!(
        subsequent_run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        subsequent_run.stdout, subsequent_run.stderr
    );
    let json = subsequent_run.json();
    assert_eq!(json["errors"], 0);
    assert_eq!(json["baselined"], 1);

    // Named note must report baselined finding
    let outcome = subsequent_run.outcome("unsafe-safety-comment");
    assert_eq!(outcome["baselined"], 1);
    let notes = outcome["notes"].as_array().unwrap();
    assert!(notes.iter().any(|n| n
        .as_str()
        .unwrap()
        .contains("1 baselined finding not blocking")));
}

#[test]
fn test_baseline_new_violation_fails_while_grandfathering_preexisting() {
    let repo = Repo::new();
    // Initial violation on work branch
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: initial unsafe");

    // Write baseline for the initial violation
    let baseline_run = repo.run(&["baseline", "--write"], &[]);
    assert_eq!(baseline_run.code, 0);

    // Add a NEW violation in a different file
    repo.write(
        "src/extra.rs",
        "pub fn write(p: *mut u8, v: u8) {\n    unsafe { *p = v; }\n}\n",
    );
    repo.commit("feat: add second unsafe");

    // Check MUST fail because of the new violation, while grandfathering the pre-existing one
    let run = repo.check_with_pr(&[], WEAKENING_PR);
    assert_eq!(run.code, 1, "{}", run.stdout);
    let json = run.json();
    assert_eq!(json["errors"], 1);
    assert_eq!(json["baselined"], 1);
}

#[test]
fn test_no_baseline_flag_bypasses_grandfathering() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: initial unsafe");

    repo.run(&["baseline", "--write"], &[]);
    assert!(repo.file("discipline-baseline.toml").exists());

    // Baseline is present, but --no-baseline must ignore it and fail
    let run = repo.check_with_pr(&["--no-baseline"], WEAKENING_PR);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(run.json()["errors"], 1);
    assert_eq!(run.json()["baselined"], 0);
}

#[test]
fn test_baseline_stale_entry_reported_as_note() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: initial unsafe");

    repo.run(&["baseline", "--write"], &[]);

    // Now resolve the finding by adding the required SAFETY comment
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    // SAFETY: caller guarantees valid pointer\n    unsafe { *p }\n}\n",
    );
    repo.commit("fix: add safety comment");

    let run = repo.check_with_pr(&[], WEAKENING_PR);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let json = run.json();
    assert_eq!(json["errors"], 0);
    assert_eq!(json["baselined"], 0);

    let outcome = run.outcome("unsafe-safety-comment");
    let notes = outcome["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("stale baseline entry (resolved findings)")),
        "expected stale baseline note in {notes:?}"
    );
}

#[test]
fn test_baseline_stable_across_line_shifts() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: initial unsafe");

    repo.run(&["baseline", "--write"], &[]);

    // Prepend 15 comment lines shifting the unsafe block down
    let shifted = format!(
        "{}\npub fn read(p: *const u8) -> u8 {{\n    unsafe {{ *p }}\n}}\n",
        (1..=15)
            .map(|i| format!("// line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    repo.write("src/lib.rs", &shifted);
    repo.commit("refactor: shift lines");

    // Fingerprint MUST still match even though line number changed from 2 to 17
    let run = repo.check_with_pr(&[], WEAKENING_PR);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let json = run.json();
    assert_eq!(json["errors"], 0);
    assert_eq!(json["baselined"], 1);
}

#[test]
fn test_baseline_config_integrity_ratchet() {
    let repo = Repo::new();

    // Baseline with 1 entry on main
    repo.commit_base(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
        "chore: base unsafe",
    );
    repo.run(&["baseline", "--write"], &[]);
    repo.commit_base(
        "discipline-baseline.toml",
        &std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap(),
        "chore: commit initial baseline",
    );

    // On work branch, add a second unsafe block and write baseline (growing from 1 to 2)
    repo.write(
        "src/extra.rs",
        "pub fn write(p: *mut u8, v: u8) {\n    unsafe { *p = v; }\n}\n",
    );
    repo.commit("feat: add second unsafe");
    repo.run(&["baseline", "--write"], &[]);
    repo.commit("chore: update baseline");

    // config-integrity gate MUST fire on baseline growth without directive
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(run
        .titles("config-integrity")
        .iter()
        .any(|t| t.contains("Baseline Grew Without Directive")));

    // With allow-gate-weakening: baseline directive, it passes
    let run_overridden = repo.check_with_pr(
        &[],
        "allow-gate-weakening: baseline grandfathering legacy module",
    );
    assert_eq!(run_overridden.code, 0, "{}", run_overridden.stdout);
    assert_eq!(
        run_overridden.outcome("config-integrity")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn test_empty_baseline_env_vars_do_not_crash() {
    let repo = Repo::new();
    let run = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("DISCIPLINE_BASELINE", ""), ("DISCIPLINE_NO_BASELINE", "")],
    );
    // Should execute cleanly (exit 0 on clean repo) and never exit with clap code 2
    assert_ne!(run.code, 2, "stderr: {}", run.stderr);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
}

#[test]
fn test_env_baseline_and_no_baseline_respected() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: initial unsafe");

    // Write to a custom baseline path
    let baseline_run = repo.run(
        &[
            "baseline",
            "--write",
            "--baseline-file",
            "custom-baseline.toml",
        ],
        &[],
    );
    assert_eq!(baseline_run.code, 0);
    assert!(repo.file("custom-baseline.toml").exists());

    // Check with DISCIPLINE_BASELINE pointing to custom file
    let run_custom = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[
            ("DISCIPLINE_BASELINE", "custom-baseline.toml"),
            ("PR_BODY", WEAKENING_PR),
        ],
    );
    assert_eq!(run_custom.code, 0, "{}", run_custom.stdout);
    assert_eq!(run_custom.json()["baselined"], 1);

    // Check with DISCIPLINE_NO_BASELINE=true ignores the custom baseline
    let run_ignored = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[
            ("DISCIPLINE_BASELINE", "custom-baseline.toml"),
            ("DISCIPLINE_NO_BASELINE", "true"),
            ("PR_BODY", WEAKENING_PR),
        ],
    );
    assert_eq!(run_ignored.code, 1);
    assert_eq!(run_ignored.json()["baselined"], 0);
}
