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
        .contains("1 finding grandfathered by baseline in this gate (not blocking)")));
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
    let note = notes
        .iter()
        .find(|n| {
            n.as_str()
                .unwrap()
                .contains("stale baseline entry (resolved findings)")
        })
        .expect("expected stale baseline note in outcome notes")
        .as_str()
        .unwrap();
    assert!(
        note.contains("`Unsafe Without SAFETY Comment` in `src/lib.rs`"),
        "expected sample rule and path in stale baseline note: {note}"
    );
}

#[test]
fn test_baseline_stale_entry_suppressed_when_gate_disabled() {
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

    // Check with the gate explicitly disabled
    let run = repo.check_with_pr(&["--disable", "unsafe-safety-comment"], WEAKENING_PR);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let outcome = run.outcome("unsafe-safety-comment");
    assert!(!outcome["enabled"].as_bool().unwrap());
    let notes = outcome["notes"].as_array().unwrap();
    assert!(
        !notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("stale baseline entry")),
        "stale baseline note must be suppressed for disabled gates, got: {notes:?}"
    );
}

#[test]
fn test_baseline_write_prints_directive_hint_for_config_integrity() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: initial unsafe");

    let run = repo.run(&["baseline", "--write"], &[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        run.stdout
            .contains("allow-gate-weakening: baseline initial grandfathered baseline"),
        "expected directive hint in: {}",
        run.stdout
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
fn test_baseline_one_for_one_swap_without_growth_fails_integrity() {
    let repo = Repo::new();

    // Baseline with finding A on main (measured against HEAD~1)
    repo.commit_base(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
        "chore: base unsafe A",
    );
    repo.run(&["baseline", "--write", "--base", "HEAD~1"], &[]);
    repo.commit_base(
        "discipline-baseline.toml",
        &std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap(),
        "chore: commit initial baseline A",
    );

    // On work branch, fix finding A (add safety comment) and introduce finding B in src/extra.rs
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    // SAFETY: valid pointer\n    unsafe { *p }\n}\n",
    );
    repo.write(
        "src/extra.rs",
        "pub fn write(p: *mut u8, v: u8) {\n    unsafe { *p = v; }\n}\n",
    );
    repo.commit("feat: resolve A and add B");
    // Re-write baseline against main: count is still 1, but finding B replaces finding A
    repo.run(&["baseline", "--write", "--base", "main"], &[]);
    repo.commit("chore: update baseline with finding B");

    // Check MUST fail integrity because head baseline contains a new fingerprint not in base baseline
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(run
        .titles("config-integrity")
        .iter()
        .any(|t| t.contains("Baseline Contains New Findings Without Directive")));

    // With allow-gate-weakening: baseline directive, it passes
    let run_overridden = repo.check_with_pr(
        &[],
        "allow-gate-weakening: baseline swapping grandfathered finding",
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

#[test]
fn test_baseline_malformed_toml_fails_closed_exit_2() {
    let repo = Repo::new();
    repo.write(
        "discipline-baseline.toml",
        "version = 1\n[[findings\nmalformed toml content\n",
    );
    repo.commit("chore: commit broken baseline");

    let run = repo.check(&[]);
    assert_eq!(
        run.code, 2,
        "malformed baseline TOML must fail closed with exit code 2\nstderr:\n{}",
        run.stderr
    );
    assert!(run.stderr.contains("failed to parse baseline file"));
}

#[test]
fn test_baseline_explicit_missing_file_fails_closed_exit_2() {
    let repo = Repo::new();
    let run = repo.check(&["--baseline-file", "non-existent-baseline.toml"]);
    assert_eq!(
        run.code, 2,
        "explicit non-existent baseline file must fail closed with exit code 2\nstderr:\n{}",
        run.stderr
    );
    assert!(run.stderr.contains("does not exist"));
}

#[test]
fn test_baseline_duplicate_fingerprint_count_decrementing() {
    let repo = Repo::new();
    let two_unsafes = "pub fn f1(p: *const u8) -> u8 {\n    unsafe { *p }\n}\npub fn f2(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n";
    repo.commit_base("src/lib.rs", two_unsafes, "chore: base two unsafes");
    repo.run(&["baseline", "--write", "--base", "HEAD~1"], &[]);
    repo.commit_base(
        "discipline-baseline.toml",
        &std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap(),
        "chore: commit baseline with 2 grandfathered entries",
    );

    // On work branch, add a third identical unsafe function f3
    let three_unsafes = "pub fn f1(p: *const u8) -> u8 {\n    unsafe { *p }\n}\npub fn f2(p: *const u8) -> u8 {\n    unsafe { *p }\n}\npub fn f3(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n";
    repo.write("src/lib.rs", three_unsafes);
    repo.commit("feat: add third unsafe");

    let run = repo.check_with_pr(&[], WEAKENING_PR);
    assert_eq!(run.code, 1, "{}", run.stdout);
    let json = run.json();
    assert_eq!(
        json["errors"], 1,
        "exactly 1 non-grandfathered error should remain"
    );
    assert_eq!(
        json["baselined"], 2,
        "exactly 2 grandfathered findings should be baselined"
    );
}

#[test]
fn test_baseline_partial_suite_write_preserves_other_suites() {
    let repo = Repo::new();

    // 1. Initial commit with an unsafe block (agent-guard) and scratch file (hygiene)
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.write(".claude/notes.md", "# Scratch\n");
    repo.commit("feat: initial unsafe and scratch");

    // Write initial baseline covering all suites
    let init_run = repo.run(&["baseline", "--write"], &[]);
    assert_eq!(init_run.code, 0, "{}", init_run.stdout);

    let baseline_content = std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap();
    assert!(baseline_content.contains("unsafe-safety-comment"));
    assert!(baseline_content.contains("agent-scratch"));

    // 2. On work branch, add a second unsafe block in src/extra.rs
    repo.write(
        "src/extra.rs",
        "pub fn write(p: *mut u8, v: u8) {\n    unsafe { *p = v; }\n}\n",
    );
    repo.commit("feat: add second unsafe");

    // Write baseline specifying ONLY --suite agent-guard
    let partial_run = repo.run(&["baseline", "--write", "--suite", "agent-guard"], &[]);
    assert_eq!(partial_run.code, 0, "{}", partial_run.stdout);

    let updated_content = std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap();
    // agent-guard findings must be updated
    assert!(updated_content.contains("src/extra.rs"));
    // hygiene findings MUST BE PRESERVED!
    assert!(
        updated_content.contains("agent-scratch"),
        "baseline must preserve grandfathered entries from unexamined suites"
    );
}
