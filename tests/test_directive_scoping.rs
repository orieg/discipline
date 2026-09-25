//! Table-driven scoping tests for EVERY directive in `src/tokens.rs` (S1).
//!
//! Asserts that:
//! 1. An unrelated reason lifts 0 findings.
//! 2. Naming ONE subject lifts exactly 1 finding.
//! 3. Overrides counter in report equals the number of lifted findings.
//!
//! Covers all 41 directives (31 canonical + 10 deprecated aliases).

mod common;
use common::Repo;
use discipline::guards::{GateOutcome, Severity};
use discipline::tokens::{
    self, parse_directives, DirectiveSubjectKind, OverrideSource, KNOWN_DIRECTIVES,
};

#[derive(Debug)]
pub struct DirectiveScopingResult {
    pub index: usize,
    pub directive: &'static str,
    pub is_canonical: bool,
    pub gate: &'static str,
    pub subject_kind: &'static str,
    pub sample_subject_1: &'static str,
    pub sample_subject_2: &'static str,
    pub unrelated_lifted: usize,
    pub single_subject_lifted: usize,
    pub overrides_count: usize,
    pub pass: bool,
}

fn subjects_for_directive(name: &str, kind: DirectiveSubjectKind) -> (&'static str, &'static str) {
    match name {
        "allow-unpinned-action" => ("actions/checkout@v4", "actions/setup-node@v3"),
        "allow-test-shrink" | "allow-floor-drop" => ("tests/test_a.rs", "tests/test_b.rs"),
        "allow-ci-weakening" => ("job-security-audit", "job-coverage-gate"),
        "no-issue" | "discipline:no-issue" => ("pr", "commit"),
        "allow-pr-checklist" | "allow-checklist" => ("test", "bench"),
        _ => match kind {
            DirectiveSubjectKind::FilePath => ("src/feature_a.rs", "src/feature_b.rs"),
            DirectiveSubjectKind::TestName => ("test_case_alpha", "test_case_beta"),
            DirectiveSubjectKind::RuleName => ("dead_code", "unused_variables"),
            DirectiveSubjectKind::BenchmarkArm => ("arm_throughput", "arm_latency"),
            DirectiveSubjectKind::ActionRef => ("actions/checkout@v4", "actions/setup-node@v3"),
            DirectiveSubjectKind::WorkflowJobOrStep => ("build-job", "test-job"),
            DirectiveSubjectKind::CommandName => ("cargo-mutants", "lcov"),
            DirectiveSubjectKind::DependencyName => ("tokio", "serde_json"),
            DirectiveSubjectKind::ChecklistItem => ("test", "bench"),
        },
    }
}

fn evaluate_findings_for_directive(
    gate: &'static str,
    directive_names: &[&str],
    raw_directive_text: &str,
    subjects: &[&str],
) -> GateOutcome {
    let mut outcome = GateOutcome::new(gate);
    let directives = parse_directives(raw_directive_text, OverrideSource::PrBody);

    for &subj in subjects {
        if let Some(ov) = tokens::find_override(&directives, gate, directive_names, subj) {
            outcome.overrides.push(ov);
        } else {
            outcome.push(
                Severity::Error,
                &format!("Violation on {subj}"),
                None,
                None,
                format!("Unexcused violation on subject '{subj}'"),
                "Provide a scoped override directive.",
            );
        }
    }
    outcome
}

#[test]
fn test_all_40_directives_table_driven_scoping() {
    assert_eq!(
        KNOWN_DIRECTIVES.len(),
        41,
        "KNOWN_DIRECTIVES must contain exactly 41 directives (31 canonical + 10 deprecated)"
    );

    let mut results = Vec::with_capacity(KNOWN_DIRECTIVES.len());

    for (i, &dir_name) in KNOWN_DIRECTIVES.iter().enumerate() {
        let spec = tokens::spec_for_directive(dir_name)
            .unwrap_or_else(|| panic!("missing spec for directive '{dir_name}'"));
        let alias_names = tokens::names_for_directive(dir_name);
        assert!(
            !alias_names.is_empty(),
            "missing aliases for directive '{dir_name}'"
        );

        let is_canonical = spec.canonical == dir_name;
        let (subj1, subj2) = subjects_for_directive(dir_name, spec.subject_kind);
        let subjects = [subj1, subj2];

        // 1. Unrelated reason test:
        // "<directive>: totally unrelated words here"
        let unrelated_text = format!("{dir_name}: totally unrelated words here\n");
        let outcome_unrelated =
            evaluate_findings_for_directive(spec.gate, alias_names, &unrelated_text, &subjects);

        let unrelated_lifted = subjects.len() - outcome_unrelated.violations.len();
        let unrelated_overrides = outcome_unrelated.overrides.len();
        assert_eq!(
            unrelated_lifted, 0,
            "Directive '{dir_name}' lifted {unrelated_lifted} findings with an unrelated reason! Expected 0."
        );
        assert_eq!(
            unrelated_overrides, 0,
            "Directive '{dir_name}' recorded {unrelated_overrides} overrides with an unrelated reason! Expected 0."
        );

        // 2. Naming Subject 1 test:
        // "<directive>: <subj1> legitimate justification here"
        let scoped_text_1 = format!("{dir_name}: {subj1} legitimate justification here\n");
        let outcome_scoped_1 =
            evaluate_findings_for_directive(spec.gate, alias_names, &scoped_text_1, &subjects);

        let single_lifted_1 = subjects.len() - outcome_scoped_1.violations.len();
        let single_overrides_1 = outcome_scoped_1.overrides.len();
        assert_eq!(
            single_lifted_1, 1,
            "Directive '{dir_name}' naming subject '{subj1}' lifted {single_lifted_1} findings! Expected 1."
        );
        assert_eq!(
            single_overrides_1, 1,
            "Directive '{dir_name}' naming subject '{subj1}' recorded {single_overrides_1} overrides! Expected 1."
        );
        assert_eq!(outcome_scoped_1.overrides[0].subject, subj1);

        // 3. Naming Subject 2 test:
        let scoped_text_2 = format!("{dir_name}: {subj2} legitimate justification here\n");
        let outcome_scoped_2 =
            evaluate_findings_for_directive(spec.gate, alias_names, &scoped_text_2, &subjects);

        let single_lifted_2 = subjects.len() - outcome_scoped_2.violations.len();
        let single_overrides_2 = outcome_scoped_2.overrides.len();
        assert_eq!(
            single_lifted_2, 1,
            "Directive '{dir_name}' naming subject '{subj2}' lifted {single_lifted_2} findings! Expected 1."
        );
        assert_eq!(
            single_overrides_2, 1,
            "Directive '{dir_name}' naming subject '{subj2}' recorded {single_overrides_2} overrides! Expected 1."
        );
        assert_eq!(outcome_scoped_2.overrides[0].subject, subj2);

        // 4. Naming Both Subjects test:
        let scoped_both = format!("{dir_name}: {subj1} and {subj2} legitimate justification\n");
        let outcome_both =
            evaluate_findings_for_directive(spec.gate, alias_names, &scoped_both, &subjects);
        assert_eq!(outcome_both.violations.len(), 0);
        assert_eq!(outcome_both.overrides.len(), 2);

        let kind_str = match spec.subject_kind {
            DirectiveSubjectKind::FilePath => "FilePath",
            DirectiveSubjectKind::TestName => "TestName",
            DirectiveSubjectKind::RuleName => "RuleName",
            DirectiveSubjectKind::BenchmarkArm => "BenchmarkArm",
            DirectiveSubjectKind::ActionRef => "ActionRef",
            DirectiveSubjectKind::WorkflowJobOrStep => "WorkflowJobOrStep",
            DirectiveSubjectKind::CommandName => "CommandName",
            DirectiveSubjectKind::DependencyName => "DependencyName",
            DirectiveSubjectKind::ChecklistItem => "ChecklistItem",
        };

        results.push(DirectiveScopingResult {
            index: i + 1,
            directive: dir_name,
            is_canonical,
            gate: spec.gate,
            subject_kind: kind_str,
            sample_subject_1: subj1,
            sample_subject_2: subj2,
            unrelated_lifted,
            single_subject_lifted: single_lifted_1,
            overrides_count: single_overrides_1,
            pass: true,
        });
    }

    // Print table for user reporting
    println!("\n=== TABLE-DRIVEN DIRECTIVE SCOPING RESULTS (40 DIRECTIVES) ===");
    println!(
        "| # | Directive | Status | Gate | Subject Kind | Sample Subject | Unrelated Lifts | 1-Subj Lifts | Overrides | Verdict |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for r in &results {
        let status = if r.is_canonical {
            "canonical"
        } else {
            "deprecated"
        };
        println!(
            "| {:2} | `{}` | {} | `{}` | {} | `{}` | {} | {} | {} | {} |",
            r.index,
            r.directive,
            status,
            r.gate,
            r.subject_kind,
            r.sample_subject_1,
            r.unrelated_lifted,
            r.single_subject_lifted,
            r.overrides_count,
            if r.pass { "**PASS**" } else { "**FAIL**" }
        );
    }
    println!("=== END DIRECTIVE SCOPING RESULTS ===\n");
}

#[test]
fn test_e2e_allow_test_shrink_scoping() {
    let repo = Repo::new();

    // Base branch has tests/a.rs
    repo.git(&["checkout", "-b", "feat/test-shrink"]);
    repo.git(&["rm", "-q", "tests/a.rs"]);
    repo.commit("test: remove tests/a.rs");

    // 1. Unrelated reason lifts 0 findings
    let run_unrelated = repo.check_with_pr(
        &[],
        "removes: tests/a.rs obsolete\nallow-test-shrink: totally unrelated words here\n",
    );
    assert_eq!(run_unrelated.code, 1);
    let floor_violations = run_unrelated.titles("test-floor");
    assert_eq!(
        floor_violations.len(),
        1,
        "unrelated reason must NOT lift test-floor finding"
    );

    // 2. Scoped directive naming tests/a.rs lifts exactly 1 finding
    let run_scoped = repo.check_with_pr(
        &[],
        "removes: tests/a.rs obsolete\nallow-test-shrink: tests/a.rs superseded by updated test harness\n",
    );
    assert_eq!(
        run_scoped.code, 0,
        "stdout: {}\nstderr: {}",
        run_scoped.stdout, run_scoped.stderr
    );
    let json = run_scoped.json();
    assert_eq!(json["errors"], 0);
    assert!(json["overrides"].as_u64().unwrap() >= 2); // removes + allow-test-shrink
}

#[test]
fn test_e2e_allow_unpinned_action_scoping() {
    let repo = Repo::new();

    // Add workflow with 2 unpinned third-party actions
    repo.write(
        ".github/workflows/ci.yml",
        r#"
name: CI
on: [push]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: codecov/codecov-action@v4
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: docker/build-push-action@v5
  ci-gate:
    needs: [lint, test]
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
"#,
    );
    repo.commit("ci: add unpinned actions");

    // 1. Unrelated reason lifts 0 findings
    let run_unrelated =
        repo.check_with_pr(&[], "allow-unpinned-action: totally unrelated words here\n");
    assert_eq!(run_unrelated.code, 1);
    let violations = run_unrelated.titles("ci-integrity");
    let unpinned = violations.iter().filter(|t| t.contains("Unpinned")).count();
    assert_eq!(
        unpinned, 2,
        "unrelated reason must NOT lift any unpinned action findings"
    );

    // 2. Naming ONE action lifts exactly 1 finding
    let run_scoped_one = repo.check_with_pr(
        &[],
        "allow-unpinned-action: codecov/codecov-action@v4 pinned by org policy\n",
    );
    assert_eq!(run_scoped_one.code, 1);
    let violations_one = run_scoped_one.titles("ci-integrity");
    let unpinned_one = violations_one
        .iter()
        .filter(|t| t.contains("Unpinned"))
        .count();
    assert_eq!(
        unpinned_one, 1,
        "naming codecov/codecov-action@v4 must lift exactly 1 unpinned action finding"
    );

    // 3. Naming BOTH actions lifts both findings
    let run_scoped_both = repo.check_with_pr(
        &[],
        "allow-unpinned-action: codecov/codecov-action@v4 pinned by org\nallow-unpinned-action: docker/build-push-action@v5 pinned by org\n",
    );
    let violations_both = run_scoped_both.titles("ci-integrity");
    let unpinned_both = violations_both
        .iter()
        .filter(|t| t.contains("Unpinned"))
        .count();
    assert_eq!(
        unpinned_both, 0,
        "naming both actions must lift both unpinned action findings"
    );
}

#[test]
fn test_e2e_allow_ci_weakening_scoping() {
    let repo = Repo::new();

    // Create workflow with 2 continue-on-error steps
    repo.write(
        ".github/workflows/ci.yml",
        r#"
name: CI
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - name: step-one
        continue-on-error: true
        run: cargo test
      - name: step-two
        continue-on-error: true
        run: cargo clippy
  ci-gate:
    needs: [test]
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
"#,
    );
    repo.commit("ci: add masked failures");

    // 1. Unrelated reason lifts 0 findings
    let run_unrelated =
        repo.check_with_pr(&[], "allow-ci-weakening: totally unrelated words here\n");
    assert_eq!(run_unrelated.code, 1);
    let violations = run_unrelated.titles("ci-integrity");
    let continue_on_err = violations
        .iter()
        .filter(|t| t.contains("continue-on-error"))
        .count();
    assert_eq!(
        continue_on_err, 2,
        "unrelated reason must NOT lift continue-on-error findings"
    );

    // 2. Naming step-one lifts exactly 1 finding
    let run_scoped_one = repo.check_with_pr(&[], "allow-ci-weakening: step-one temporary waiver\n");
    assert_eq!(run_scoped_one.code, 1);
    let violations_one = run_scoped_one.titles("ci-integrity");
    let continue_on_err_one = violations_one
        .iter()
        .filter(|t| t.contains("continue-on-error"))
        .count();
    assert_eq!(
        continue_on_err_one, 1,
        "naming step-one must lift exactly 1 continue-on-error finding"
    );

    // 3. Naming step-two lifts the other finding
    let run_scoped_two = repo.check_with_pr(&[], "allow-ci-weakening: step-two temporary waiver\n");
    assert_eq!(run_scoped_two.code, 1);
    let violations_two = run_scoped_two.titles("ci-integrity");
    let continue_on_err_two = violations_two
        .iter()
        .filter(|t| t.contains("continue-on-error"))
        .count();
    assert_eq!(
        continue_on_err_two, 1,
        "naming step-two must lift exactly 1 continue-on-error finding"
    );
}
