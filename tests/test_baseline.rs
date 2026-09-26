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
        note.contains("`unsafe-safety-comment/safety-comment-missing` in `src/lib.rs`"),
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
        .any(|t| t.contains("Baseline Increased")));

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
        .any(|t| t.contains("Baseline Contains New Findings")));

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

#[test]
fn baseline_write_refuses_to_replace_an_unreadable_baseline() {
    let repo = Repo::new();
    repo.commit_base("README.md", "x\n", "base");
    repo.write("discipline-baseline.toml", "this is [[[ not toml\n");
    let run = repo.run(&["baseline", "--write", "--suite", "integrity"], &[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.stderr.contains("could not be read"), "{}", run.stderr);
    // The file is left as it was, not rewritten without the other suites' entries.
    let after = std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap();
    assert_eq!(after, "this is [[[ not toml\n");
}

/// Rewrite the version-2 baseline `baseline --write` produced into the version-1 file an
/// older release wrote: the same findings, fingerprinted on their titles.
fn downgrade_to_v1(repo: &Repo, base: &str) {
    let run = repo.run(&["check", "--format", "json", "--base", base], &[]);
    let report: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let mut toml = String::from("version = 1\n");
    for o in report["outcomes"].as_array().unwrap() {
        for v in o["violations"].as_array().unwrap() {
            let gate: &'static str =
                Box::leak(v["gate"].as_str().unwrap().to_string().into_boxed_str());
            let violation = discipline::guards::Violation {
                gate,
                code: v["code"].as_str().unwrap().to_string(),
                fingerprint: String::new(),
                legacy_title: {
                    let code = v["code"].as_str().unwrap();
                    let message = v["message"].as_str().unwrap().to_string();
                    discipline::findings::FINDINGS
                        .iter()
                        .find(|k| format!("{}/{}", k.gates[0], k.code) == code)
                        .and_then(|k| match k.v1 {
                            discipline::findings::V1::Same | discipline::findings::V1::Site => None,
                            discipline::findings::V1::Was(t) => Some(t.to_string()),
                            discipline::findings::V1::Message => Some(message),
                        })
                },
                severity: discipline::config::Severity::Error,
                title: v["title"].as_str().unwrap().to_string(),
                file: v["file"].as_str().map(str::to_string),
                line: v["line"].as_u64().map(|l| l as usize),
                message: v["message"].as_str().unwrap().to_string(),
                remediation: None,
            };
            let fp = discipline::baseline::fingerprint_for_version(
                &violation,
                |f| std::fs::read_to_string(repo.file(f)).ok(),
                1,
            );
            toml.push_str(&format!(
                "\n[[findings]]\ngate = \"{gate}\"\nrule = \"{}\"\npath = \"{}\"\nfingerprint = \"{fp}\"\n",
                violation.legacy_title.as_deref().unwrap_or(&violation.title),
                violation.file.clone().unwrap_or_default()
            ));
        }
    }
    std::fs::write(repo.file("discipline-baseline.toml"), toml).unwrap();
}

#[test]
fn a_version_one_baseline_matches_until_migrated_and_a_migration_alone_is_accepted() {
    let repo = Repo::new();
    repo.commit_base(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
        "chore: base unsafe",
    );
    downgrade_to_v1(&repo, "HEAD~1");
    repo.commit_base(
        "discipline-baseline.toml",
        &std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap(),
        "chore: version-1 baseline",
    );
    let measured = |repo: &Repo| {
        repo.check_with_pr(&["--base", "HEAD~2"], WEAKENING_PR)
            .json()
    };

    // 1. A version-1 file still grandfathers its finding, and says it is deprecated.
    let v1 = measured(&repo);
    assert_eq!(v1["baselined"], 1, "{v1:#}");
    assert!(
        v1["deprecations"][0]
            .as_str()
            .is_some_and(|d| d.contains("fingerprint version 1")),
        "{v1:#}"
    );

    // 2. `--migrate` rewrites it to version 2; the finding stays grandfathered.
    let migrate = repo.run(&["baseline", "--migrate"], &[]);
    assert_eq!(migrate.code, 0, "{}\n{}", migrate.stdout, migrate.stderr);
    assert!(
        migrate
            .stdout
            .contains("1 entry migrated, 0 stale entries dropped"),
        "{}",
        migrate.stdout
    );
    let migrated = std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap();
    assert!(migrated.contains("version = 2"), "{migrated}");
    assert!(
        migrated.contains("rule = \"unsafe-safety-comment/safety-comment-missing\""),
        "{migrated}"
    );
    let v2 = measured(&repo);
    assert_eq!(v2["baselined"], 1, "{v2:#}");
    assert!(v2.get("deprecations").is_none(), "{v2:#}");

    // 3. The migration committed on its own passes config-integrity without a directive.
    repo.commit("chore: migrate the baseline");
    let alone = repo.check(&[]);
    assert_eq!(alone.code, 0, "{}", alone.stdout);
    assert!(
        alone.outcome("config-integrity")["notes"]
            .to_string()
            .contains("migrated from fingerprint version 1 to 2"),
        "{}",
        alone.stdout
    );

    // 4. Mixed with another change, it is a finding: the migration cannot be verified.
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes");
    let mixed = repo.check(&[]);
    assert_eq!(mixed.code, 1, "{}", mixed.stdout);
    let codes: Vec<String> = mixed
        .violations("config-integrity")
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        codes,
        ["config-integrity/baseline-migration-not-alone"],
        "{}",
        mixed.stdout
    );

    // 5. A "migration" that moves an entry to another path is a swap, not a migration.
    repo.git(&["reset", "-q", "--hard", "HEAD~2"]);
    repo.run(&["baseline", "--migrate"], &[]);
    let swapped = std::fs::read_to_string(repo.file("discipline-baseline.toml"))
        .unwrap()
        .replace("path = \"src/lib.rs\"", "path = \"src/other.rs\"");
    std::fs::write(repo.file("discipline-baseline.toml"), swapped).unwrap();
    repo.commit("chore: migrate the baseline");
    let swap = repo.check(&[]);
    assert_eq!(swap.code, 1, "{}", swap.stdout);
    assert!(
        swap.violations("config-integrity")
            .iter()
            .any(|v| v["code"] == "config-integrity/baseline-new-findings"),
        "{}",
        swap.stdout
    );
}

#[test]
fn json_and_sarif_carry_the_baseline_fingerprint_and_a_rule_per_code() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: unsafe");
    let json = repo.check(&[]).json();
    let fp = json["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|o| o["violations"].as_array().unwrap())
        .map(|v| v["fingerprint"].as_str().unwrap().to_string())
        .next()
        .unwrap();
    assert_eq!(fp.len(), 64);

    // The fingerprint is what `baseline --write` records.
    repo.run(&["baseline", "--write"], &[]);
    let recorded = std::fs::read_to_string(repo.file("discipline-baseline.toml")).unwrap();
    assert!(recorded.contains(&fp), "{recorded}");

    let sarif = repo.run(
        &[
            "check",
            "--format",
            "sarif",
            "--base",
            "main",
            "--no-baseline",
        ],
        &[],
    );
    let sarif: serde_json::Value = serde_json::from_str(&sarif.stdout).unwrap();
    let result = &sarif["runs"][0]["results"][0];
    assert_eq!(
        result["ruleId"],
        "unsafe-safety-comment/safety-comment-missing"
    );
    assert_eq!(
        result["partialFingerprints"]["disciplineFingerprint/v2"],
        fp.as_str()
    );
    let rules: Vec<&str> = sarif["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        rules.contains(&"unsafe-safety-comment/safety-comment-missing"),
        "{rules:?}"
    );
}
