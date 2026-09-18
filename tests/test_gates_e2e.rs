//! One negative control (the gate must fire) and one positive control (the
//! gate must stay silent on the legitimate form) per gate, end to end through
//! the released interface: the binary, a real git repository, JSON output.

mod common;
use common::{Repo, GOOD_LIB, GOOD_TEST};

#[test]
fn clean_change_passes_and_reports_what_it_examined() {
    let repo = Repo::new();
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1 then Phase 2, gated on tests.\n",
    );
    repo.commit("docs: reword");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let json = run.json();
    assert_eq!(json["errors"], 0);
    assert!(run.outcome("pii")["examined"].as_u64().unwrap() >= 4);
    assert!(json["planned_gates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|g| g == "miri"));
}

// ---- assertion-reduction ---------------------------------------------------

#[test]
fn assertion_reduction_fires_on_weakening_and_on_removal() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    assert!(1 + 1 == 2);\n    assert!(2 + 2 == 4);\n}\n\n#[test]\nfn orders() {\n}\n",
    );
    repo.commit("test: simplify");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(run.titles("assertion-reduction").len(), 2, "{}", run.stdout);
}

#[test]
fn assertion_reduction_override_must_name_the_test() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("    assert_eq!(x + 3, 4);\n", ""),
    );
    repo.commit("test: trim\n\nallow-assertion-drop: orders covered elsewhere");
    assert_eq!(
        repo.check(&[]).titles("assertion-reduction").len(),
        1,
        "wrong test named"
    );

    repo.commit("test: justify\n\nallow-assertion-drop: adds second case moved to proptest");
    let run = repo.check(&[]);
    assert!(
        run.titles("assertion-reduction").is_empty(),
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 0);
}

#[test]
fn adding_assertions_is_not_a_reduction() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "assert!(x < 2);",
            "assert!(x < 2);\n    let y = 3; assert_eq!(y, 1 + 2);",
        ),
    );
    repo.commit("test: more");
    assert_eq!(repo.check(&[]).code, 0);
}

#[test]
fn unanalysed_languages_are_named_not_silently_passed() {
    let repo = Repo::new();
    repo.write("tools/check.py", "def test_nothing():\n    pass\n");
    repo.write("web/app.ts", "export const x = 1;\n");
    repo.commit("feat: tooling");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 0,
        "hygiene and integrity gates still apply to any language"
    );
    for gate in [
        "assertion-reduction",
        "vacuous-tests",
        "ignored-tests",
        "unsafe-safety-comment",
    ] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            notes.contains("2 changed source file(s)") && notes.contains("NOT analysed"),
            "{gate}: {notes}"
        );
    }
    assert!(run.outcome("pii")["notes"]
        .to_string()
        .contains("no PR body"));
    assert!(!run.outcome("pii")["notes"]
        .to_string()
        .contains("NOT analysed"));

    // A Rust-only change carries no such note.
    let rust_only = Repo::new();
    rust_only.write(
        "tests/b.rs",
        "#[test]\nfn real() { let x = 2; assert_eq!(x * 2, 4); }\n",
    );
    rust_only.commit("test: rust");
    let rust_run = rust_only.check(&[]);
    assert_eq!(rust_run.code, 0);
    assert!(!rust_run.outcome("assertion-reduction")["notes"]
        .to_string()
        .contains("NOT analysed"));
}

#[test]
fn unsupported_source_files_include_phpt_and_group_by_extension() {
    let repo = Repo::new();
    repo.write("ext/judy.c", "void foo() {}\n");
    repo.write("ext/judy.h", "#define FOO 1\n");
    repo.write("tests/001.phpt", "--TEST--\ntest\n");
    repo.commit("feat: c and phpt");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0);
    for gate in [
        "assertion-reduction",
        "vacuous-tests",
        "ignored-tests",
        "unsafe-safety-comment",
    ] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            notes.contains("3 changed source file(s)")
                && notes.contains("2 .c/.h, 1 .phpt")
                && notes.contains("NOT analysed"),
            "gate {gate} should format extension breakdown: {notes}"
        );
    }
}

// ---- vacuous-tests ---------------------------------------------------------

#[test]
fn vacuous_tests_fire_on_empty_and_tautological_tests_only() {
    let repo = Repo::new();
    repo.write(
        "tests/b.rs",
        "#[test]\nfn ghost() {}\n\n#[test]\nfn tautology() { assert!(true); }\n\n#[test]\nfn real() { let x = 2; assert_eq!(x * 2, 4); }\n",
    );
    repo.commit("test: add");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(run.titles("vacuous-tests").len(), 2, "{}", run.stdout);
}

#[test]
fn vacuous_tests_honor_configured_assert_helpers() {
    let repo = Repo::new();
    repo.write(
        "tests/b.rs",
        "#[test]\nfn via_helper() { check_invariants(3); }\n",
    );
    repo.commit("test: add");
    assert_eq!(repo.check(&[]).titles("vacuous-tests").len(), 1);
    let run = repo.check(&[
        "--config-override",
        "[gates.vacuous-tests]\nassert_helper_fns = [\"check_invariants\"]",
    ]);
    assert!(run.titles("vacuous-tests").is_empty(), "{}", run.stdout);
}

#[test]
fn tautology_constant_expressions_fire_vacuous_tests_gate() {
    let repo = Repo::new();
    repo.write(
        "tests/b.rs",
        "#[test]\nfn t_math() { assert!(1 + 1 > 0); }\n\
         #[test]\nfn t_eq() { assert_eq!(1, 1); }\n\
         #[test]\nfn t_ne() { assert_ne!(1, 2); }\n\
         #[test]\nfn t_msg() { assert!(1 == 1, \"user: {}\", x); }\n\
         #[test]\nfn t_real() { let x = 1; assert!(x > 0); }\n",
    );
    repo.commit("test: add constant tautologies");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let violations = run.titles("vacuous-tests");
    assert_eq!(violations.len(), 4, "{}", run.stdout);
}

#[test]
fn idiomatic_result_and_unwrap_tests_pass_vacuous_tests_gate() {
    let repo = Repo::new();
    repo.write(
        "tests/b.rs",
        "#[test]\nfn parses() -> Result<(), Box<dyn std::error::Error>> {\n    let _n: i32 = \"4\".parse()?;\n    Ok(())\n}\n\
         #[test]\nfn unwraps() {\n    let _n: i32 = \"4\".parse().unwrap();\n}\n\
         #[test]\nfn expects() {\n    let _n: i32 = \"4\".parse().expect(\"valid\");\n}\n\
         #[test]\nfn empty_fallible() -> Result<(), Box<dyn std::error::Error>> {\n    let _n = 4;\n    Ok(())\n}\n",
    );
    repo.commit("test: add fallible and unwrap tests");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let violations = run.titles("vacuous-tests");
    // Only empty_fallible should be flagged
    assert_eq!(violations.len(), 1, "{}", run.stdout);
    assert_eq!(violations[0], "Vacuous Test Added");
}

// ---- ignored-tests ---------------------------------------------------------

#[test]
fn ignored_tests_fire_and_accept_a_scoped_override() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("#[test]\nfn orders", "#[test]\n#[ignore]\nfn orders"),
    );
    repo.commit("test: quiet");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Newly Marked #[ignore]"]
    );

    repo.write(
        "body.md",
        "Summary\n\nallow-ignore: orders needs the new allocator first\n",
    );
    let run = repo.check(&["--pr-body-file", "body.md"]);
    assert!(run.titles("ignored-tests").is_empty(), "{}", run.stdout);
}

#[test]
fn ignored_tests_fire_on_cfg_attr_ignore() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders",
            "#[test]\n#[cfg_attr(all(), ignore)]\nfn orders",
        ),
    );
    repo.commit("test: conditional ignore");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Newly Marked #[ignore]"]
    );
}

// ---- unsafe-safety-comment -------------------------------------------------

#[test]
fn unsafe_gate_fires_on_undocumented_block_not_on_documented_one() {
    let repo = Repo::new();
    let documented = format!("{GOOD_LIB}\npub fn again(p: *const u8) -> u8 {{\n    // SAFETY: same contract as `read`.\n    let v = unsafe {{ *p }};\n    v\n}}\n");
    repo.write("src/lib.rs", &documented);
    repo.commit("feat: again");
    assert!(repo.check(&[]).titles("unsafe-safety-comment").is_empty());

    repo.write(
        "src/lib.rs",
        &format!("{documented}\npub fn bad(p: *const u8) -> u8 {{ let v = unsafe{{ *p }}; v }}\n"),
    );
    repo.commit("feat: bad");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("unsafe-safety-comment").len(),
        1,
        "{}",
        run.stdout
    );
}

#[test]
fn unsafe_gate_fires_when_a_safety_comment_is_deleted() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        &GOOD_LIB.replace(
            "    // SAFETY: callers pass a pointer that is valid for reads.\n",
            "",
        ),
    );
    repo.commit("refactor: tidy comments");
    assert_eq!(repo.check(&[]).titles("unsafe-safety-comment").len(), 1);
}

// ---- deletion-rationale ----------------------------------------------------

#[test]
fn deletion_needs_a_scoped_line_anchored_rationale() {
    let repo = Repo::new();
    repo.git(&["rm", "-q", "tests/a.rs"]);
    repo.commit(
        "chore: cleanup\n\nWe rely on the removes: tests/a.rs token described in the docs.",
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "prose mention must not arm the override");
    assert_eq!(
        run.titles("deletion-rationale"),
        vec!["File Deleted Without Rationale"]
    );

    repo.commit("chore: justify\n\nremoves: tests/other.rs wrong file");
    assert_eq!(
        repo.check(&[]).titles("deletion-rationale").len(),
        1,
        "unscoped override"
    );

    repo.commit("chore: justify\n\nremoves: tests/a.rs superseded by the property suite");
    let run = repo.check(&[]);
    assert!(
        run.titles("deletion-rationale").is_empty(),
        "{}",
        run.stdout
    );
}

#[test]
fn rename_is_not_a_deletion_but_a_removed_test_is() {
    let repo = Repo::new();
    repo.git(&["mv", "tests/a.rs", "tests/arith.rs"]);
    repo.commit("refactor: rename");
    assert!(repo.check(&[]).titles("deletion-rationale").is_empty());

    repo.write(
        "tests/arith.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
            "",
        ),
    );
    repo.commit("test: drop");
    assert_eq!(
        repo.check(&[]).titles("deletion-rationale"),
        vec!["Test Removed Without Rationale"]
    );
}

#[test]
fn rename_test_by_name_similarity_passes_without_removes_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("#[test]\nfn adds()", "#[test]\nfn adds_integers()"),
    );
    repo.commit("test: rename adds to adds_integers");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert!(run.titles("deletion-rationale").is_empty());
    assert!(run.titles("assertion-reduction").is_empty());
}

#[test]
fn rename_and_gut_fires_assertion_reduction() {
    let repo = Repo::new();
    // Rename adds to adds_integers and gut its assertions
    let gutted = GOOD_TEST.replace(
        "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 3, 4);\n}",
        "#[test]\nfn adds_integers() {\n    let x = 1;\n    assert!(x + 1 == 2);\n}",
    );
    repo.write("tests/a.rs", &gutted);
    repo.commit("test: rename and reduce assertions");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(run.titles("assertion-reduction").len(), 1);
}

#[test]
fn r1_test_swap_without_directive_fails_assertion_reduction() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1 + 1;\n    assert_eq!(x, 2);\n    assert_eq!(x + 2, 4);\n}\n",
    );
    repo.commit("feat: base adds");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn zq() {\n    let x = 1 + 1;\n    assert!(x > 0);\n}\n",
    );
    repo.commit("test: swap adds with zq");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let titles = run.titles("assertion-reduction");
    assert_eq!(titles, vec!["Assertion Reduction In Existing Test"]);
    let outcome = run.outcome("assertion-reduction");
    let violations = outcome["violations"].as_array().unwrap();
    let msg = violations[0]["message"].as_str().unwrap();
    assert!(msg.contains("adds"), "message should name old test: {msg}");
    assert!(msg.contains("zq"), "message should name new test: {msg}");
}

#[test]
fn r1_rename_and_gut_with_truthful_removes_still_fails_assertion_reduction() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1 + 1;\n    assert_eq!(x, 2);\n    assert_eq!(x + 2, 4);\n}\n",
    );
    repo.commit("feat: base adds");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn zq() {\n    let x = 1 + 1;\n    assert!(x > 0);\n}\n",
    );
    repo.commit("test: rename adds to zq\n\nremoves: adds renamed to zq");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.titles("assertion-reduction"),
        vec!["Assertion Reduction In Existing Test"]
    );
}

#[test]
fn r1_two_tests_replaced_by_two_weaker_ones_fails() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn t1() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 2, 3);\n}\n#[test]\nfn t2() {\n    let y = 4;\n    assert_eq!(y, 4);\n    assert_eq!(y + 1, 5);\n    assert_eq!(y + 2, 6);\n}\n",
    );
    repo.commit("feat: initial tests");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn w1() {\n    let x = 1;\n    assert!(x > 0);\n}\n#[test]\nfn w2() {\n    let y = 2;\n    assert!(y > 0);\n}\n",
    );
    repo.commit("test: replace with weaker tests");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(run.titles("assertion-reduction").len(), 2);
}

#[test]
fn r1_long_test_deleted_while_unrelated_one_line_test_added_fails() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn test_security_audit() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n    assert_eq!(x + 2, 3);\n    assert_eq!(x + 3, 4);\n    assert_eq!(x + 4, 5);\n}\n",
    );
    repo.commit("feat: long test");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn test_dummy_hello() {\n    let b = true;\n    assert!(b);\n}\n",
    );
    repo.commit("test: replace long test with dummy");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.titles("assertion-reduction"),
        vec!["Assertion Reduction In Existing Test"]
    );
}

#[test]
fn r1_pure_rename_opposite_names_passes() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn is_valid() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n}\n",
    );
    repo.commit("feat: valid");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn is_invalid() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n}\n",
    );
    repo.commit("refactor: rename is_valid to is_invalid");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert!(run.titles("assertion-reduction").is_empty());
    assert!(run.titles("deletion-rationale").is_empty());
}

#[test]
fn r1_one_test_split_into_two_with_total_assertions_not_lower_passes() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn test_combo() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n    assert!(x > 0);\n}\n",
    );
    repo.commit("feat: combo test");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn test_part1() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n}\n#[test]\nfn test_part2() {\n    let x = 3;\n    assert!(x > 0);\n}\n",
    );
    repo.commit("refactor: split test_combo into part1 and part2");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert!(run.titles("assertion-reduction").is_empty());
    assert!(run.titles("deletion-rationale").is_empty());
}

#[test]
fn r1_forced_pair_allow_assertion_drop_must_name_old_test() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n}\n",
    );
    repo.commit("feat: base adds");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn zq() {\n    let x = 1;\n    assert!(x > 0);\n}\n",
    );
    // Naming new test zq must FAIL:
    repo.commit("test: drop\n\nallow-assertion-drop: zq reason");
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(run.code, 1, "naming new test should fail");

    // Naming old test adds must PASS:
    let repo2 = Repo::new();
    repo2.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x, 1);\n    assert_eq!(x + 1, 2);\n}\n",
    );
    repo2.commit("feat: base adds");
    repo2.write(
        "tests/a.rs",
        "#[test]\nfn zq() {\n    let x = 1;\n    assert!(x > 0);\n}\n",
    );
    repo2.commit("test: drop\n\nallow-assertion-drop: adds reason");
    let run2 = repo2.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run2.code, 0,
        "naming old test must pass: stdout: {}\nstderr: {}",
        run2.stdout, run2.stderr
    );
}

// ---- time-estimates --------------------------------------------------------

#[test]
fn time_estimates_fire_in_prose_not_in_fences_or_marked_lines() {
    let repo = Repo::new();
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 2 (1 week).\n\n```\nsleep for 3 days\n```\n\nBanned: \"2 weeks\" <!-- discipline:allow(time-estimates) -->\n\nArtifact retention is 30 days.\n",
    );
    repo.commit("docs: plan");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let outcome = run.outcome("time-estimates");
    assert_eq!(
        outcome["violations"].as_array().unwrap().len(),
        1,
        "{}",
        run.stdout
    );
    assert_eq!(outcome["violations"][0]["line"], 3);
    assert_eq!(outcome["inline_exemptions"], 1);
}

#[test]
fn time_estimates_are_configurable_off_exempt_warning_and_extended() {
    let repo = Repo::new();
    repo.write(
        "docs/plan.md",
        "Ships in 3 weeks.\nDue at the summer milestone.\n",
    );
    repo.commit("docs: plan");

    let off = repo.check(&["--disable", "time-estimates"]);
    assert_eq!(off.code, 0);
    assert_eq!(off.outcome("time-estimates")["enabled"], false);

    let exempt = repo.check(&[
        "--config-override",
        "[gates.time-estimates]\nexempt_paths = [\"docs/**\"]",
    ]);
    assert_eq!(exempt.code, 0);

    let warn = repo.check(&[
        "--config-override",
        "[gates.time-estimates]\nseverity = \"warning\"",
    ]);
    assert_eq!(warn.code, 0);
    assert_eq!(warn.json()["warnings"], 1);
    assert_eq!(
        repo.check(&[
            "--fail-on-warnings",
            "--config-override",
            "[gates.time-estimates]\nseverity = \"warning\""
        ])
        .code,
        1
    );

    let extended = repo.check(&[
        "--config-override",
        "[gates.time-estimates]\nextra_patterns = [\"(?i)summer milestone\"]",
    ]);
    assert_eq!(extended.titles("time-estimates").len(), 2);
}

#[test]
fn pr_body_is_scanned_for_time_estimates() {
    let repo = Repo::new();
    repo.write("docs/x.md", "fine\n");
    repo.commit("docs: x");
    let run = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("PR_BODY", "Will land next sprint.")],
    );
    assert_eq!(run.code, 1);
    assert_eq!(
        run.outcome("time-estimates")["violations"][0]["file"],
        "<PR body>"
    );
}

#[test]
fn time_estimates_contextual_exemptions_and_discrimination() {
    let repo = Repo::new();
    repo.write(
        "docs/good.md",
        r#"# System Architecture
Expanse is a replacement for the 20-year-old C library.
Invariants unchecked for 20 years (§6.5) hold.
The 20-year invariants hold.
Bug survived 19 years before discovery.
Went undetected for ~19 years.

**Q1 — what do our patches buy?**
### Q2: Why judy?
Q3. How does this scale?
(Q1) What is the throughput?
Both numbers are true; only Q1's is ours.
Quoting Q2 as the vendoring speedup.

Budget: each shard has its own 180-minute budget.
Cancelled at its 60-minute cap on every scheduled run.
Half of GitHub's 6-hour default.
The 6-hour gap between runs.
Ran for over two hours of wall time on the same core.
Loadavg: that is a 1-min average decaying from build (5-min 1.32, 15-min 0.47).
Citation: Doug Baskins, [*A 10-Minute Description of How Judy Arrays Work*](https://judy.sourceforge.net/doc/10minutes.htm) (2002).

| shard | Miri-visible tests | wall, reference host | job, hosted runner | note |
|---|---:|---:|---:|---|
| `lib-set` | 44 | 1,119 s | 29 min | largest remaining item |
| `lib-cursor` | 12 | 873 s | 22 min | model unchanged |
"#,
    );
    repo.commit("docs: good");
    let clean_run = repo.check(&[]);
    assert_eq!(
        clean_run.code, 0,
        "clean docs should pass with 0 time-estimate violations: stdout: {}\nstderr: {}",
        clean_run.stdout, clean_run.stderr
    );

    // Negative control: planning durations and bypassed lines MUST fire
    let bad_repo = Repo::new();
    bad_repo.write(
        "docs/bad.md",
        r#"# Roadmap
Ship in 3 weeks; update cache ttl.
Target: Q2.
Done in Q3.
Working for 3 weeks on migration.

| Phase | Duration |
|---|---|
| Phase 1 | 2 weeks |
"#,
    );
    bad_repo.commit("docs: bad");
    let bad_run = bad_repo.check(&[]);
    assert_eq!(bad_run.code, 1);
    let outcome = bad_run.outcome("time-estimates");
    let violations = outcome["violations"].as_array().unwrap();
    // Must catch: "3 weeks" on line 2, "Q2" on line 3, "Q3" on line 4, "3 weeks" on line 5, "2 weeks" on line 9
    assert_eq!(violations.len(), 5, "violations: {violations:?}");
}

// ---- pii -------------------------------------------------------------------

#[test]
fn pii_fires_on_home_paths_and_lan_ips_without_echoing_the_user() {
    let repo = Repo::new();
    let leak = format!(
        "built in /{}/alice/src\nCI path /{}/runner/work is fine\n",
        "Users", "home"
    );
    repo.write("docs/notes.md", &leak);
    repo.write(
        "conf.txt",
        &format!("host = {}\n", ["192", "168", "4", "7"].join(".")),
    );
    repo.commit("docs: notes");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(run.titles("pii").len(), 2, "{}", run.stdout);
    assert!(
        !run.stdout.contains("alice"),
        "user name echoed into the report"
    );

    let relaxed = repo.check(&[
        "--config-override",
        "[gates.pii]\nlan_ips = false\nallowed_users = [\"alice\"]",
    ]);
    assert!(relaxed.titles("pii").is_empty(), "{}", relaxed.stdout);
}

#[test]
fn hostname_denylist_comes_from_config_or_secret_env_and_is_never_echoed() {
    let repo = Repo::new();
    repo.write("docs/bench.md", "measured on Thunderbox, 8 cores\n");
    repo.commit("docs: bench");
    assert_eq!(repo.check(&[]).code, 0, "no denylist, no finding");

    let run = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("DISCIPLINE_HOSTNAME_DENYLIST", "otherhost, thunderbox")],
    );
    assert_eq!(run.code, 1);
    assert_eq!(run.titles("pii").len(), 1);
    assert!(!run.stdout.to_lowercase().contains("thunderbox"));

    let inline = repo.check(&[
        "--config-override",
        "[gates.pii]\nhostname_denylist = [\"thunderbox\"]",
    ]);
    assert_eq!(inline.code, 1);
}

#[test]
fn active_config_exempt_from_hostname_denylist() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"demo\"\n\n[gates.pii]\nenabled = true\nhostname_denylist = [\"my-internal-server\"]\n",
    );
    repo.commit("config: set hostname denylist");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 0,
        "active config should not flag its own hostname denylist: {}",
        run.stdout
    );
    assert_eq!(run.titles("pii").len(), 0);

    // But if another file contains it, it must still be caught
    repo.write("src/lib.rs", "// connects to my-internal-server\n");
    let run2 = repo.check(&[]);
    assert_eq!(run2.code, 1);
    // Restore clean src/lib.rs
    repo.write("src/lib.rs", "// clean\n");

    // And if discipline.toml contains a home-directory path, it MUST still fire
    repo.write(
        "discipline.toml",
        &format!(
            "[meta]\nversion = 1\nname = \"demo\"\n\n[gates.pii]\nenabled = true\nhostname_denylist = [\"my-internal-server\"]\n# /{}/alice/config\n",
            "Users"
        ),
    );
    let run3 = repo.check(&[]);
    assert_eq!(run3.code, 1);
    assert_eq!(run3.titles("pii").len(), 1);
}

#[test]
fn pii_rfc1918_network_id_exemption_and_json_streaming() {
    let repo = Repo::new();
    repo.write(
        "examples/ip-range-lookup.php",
        r#"<?php
$ranges = [
    '10.0.0.0/8' => 'private-net',
    '172.16.0.0/12' => 'private-net',
    '192.168.0.0/16' => 'private-net',
    '10.0.1.0' => 'subnet-base',
    '10.0.1.255' => 'broadcast',
];
"#,
    );
    repo.write(
        "data/clean.json",
        r#"{"network": "192.168.0.0/16", "broadcast": "10.0.1.255"}"#,
    );
    // 2.5 MiB clean text file — must NOT be skipped with "over the size cap"
    let large_clean = "clean line with no secrets\n".repeat(100_000);
    repo.write("data/large_clean.txt", &large_clean);
    repo.commit("feat: network config");

    let clean_run = repo.check(&[]);
    assert_eq!(
        clean_run.code, 0,
        "stdout: {}\nstderr: {}",
        clean_run.stdout, clean_run.stderr
    );
    let pii_outcome = clean_run.outcome("pii");
    assert_eq!(pii_outcome["violations"].as_array().unwrap().len(), 0);
    let notes = pii_outcome["notes"].to_string();
    assert!(!notes.contains("over the size cap"));

    // Negative control: host IPs, JSON string leaks, and leaks inside > 2 MiB files
    let bad_repo = Repo::new();
    bad_repo.write("examples/lookup.php", "echo lookup('192.168.1.50');\n"); // discipline:allow(pii)
    bad_repo.write("data/leak.json", r#"{"client_ip": "10.0.1.5"}"#); // discipline:allow(pii)
    let mut large_leak = "clean padding line\n".repeat(100_000);
    large_leak.push_str("connect to 172.16.5.9\n"); // discipline:allow(pii)
    bad_repo.write("data/large_leak.txt", &large_leak);
    bad_repo.commit("feat: bad configs");

    let bad_run = bad_repo.check(&[]);
    assert_eq!(bad_run.code, 1);
    let outcome = bad_run.outcome("pii");
    let bad_violations = outcome["violations"].as_array().unwrap();
    assert_eq!(bad_violations.len(), 3, "violations: {bad_violations:?}");
}

// ---- agent-scratch / agents-md ---------------------------------------------

#[test]
fn agent_scratch_fires_on_tracked_scratch_state() {
    let repo = Repo::new();
    repo.write(".claude/settings.local.json", "{}\n");
    repo.write("scratch/notes.txt", "x\n");
    // A developer's global gitignore commonly lists agent directories; force
    // the add so the fixture does not depend on the machine running the test.
    repo.git(&[
        "add",
        "-f",
        ".claude/settings.local.json",
        "scratch/notes.txt",
    ]);
    repo.commit("chore: oops");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(run.titles("agent-scratch").len(), 2);
    let tuned = repo.check(&[
        "--config-override",
        "[gates.agent-scratch]\nexempt_paths = [\"scratch/**\"]",
    ]);
    assert_eq!(tuned.titles("agent-scratch").len(), 1);
}

#[test]
fn agents_md_gate_fires_when_missing_or_forked() {
    let repo = Repo::new();
    repo.write("CLAUDE.md", "# A different guide\n");
    repo.commit("docs: claude");
    assert_eq!(
        repo.check(&[]).titles("agents-md"),
        vec!["Forked Agent Guide"]
    );

    repo.git(&["rm", "-q", "AGENTS.md", "CLAUDE.md"]);
    repo.commit("chore: drop\n\nremoves: AGENTS.md CLAUDE.md experiment");
    assert_eq!(
        repo.check(&[]).titles("agents-md"),
        vec!["Missing AGENTS.md"]
    );
}

// ---- config-integrity ------------------------------------------------------

const CONFIG_HEAD: &str = "[meta]\nversion = 1\nname = \"t\"\n";

#[test]
fn a_change_cannot_weaken_its_own_config_without_a_scoped_token() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.commit("chore: config");
    repo.git(&["checkout", "-q", "-B", "work"]);

    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.vacuous-tests]\nenabled = false\n"),
    );
    repo.commit("chore: tune");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("config-integrity"),
        vec!["Gate Weakened By This Change"]
    );

    repo.commit("chore: explain\n\nallow-gate-weakening: vacuous-tests suite asserts through snapshot macros");
    assert!(repo.check(&[]).titles("config-integrity").is_empty());

    // Tightening needs no token.
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.pii]\nhostname_denylist = [\"h\"]\n"),
    );
    repo.git(&["commit", "-q", "-am", "chore: tighten", "--amend"]);
}

// ---- fail-closed behavior --------------------------------------------------

#[test]
fn could_not_check_is_exit_2_never_a_pass() {
    let repo = Repo::new();

    let unresolved = repo.check(&["--base", "origin/does-not-exist"]);
    assert_eq!(
        unresolved.code, 2,
        "unknown base must not read as an empty diff"
    );
    assert!(unresolved.stderr.contains("does not resolve"));

    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.pii]\nlan_ipz = false\n"),
    );
    assert_eq!(repo.check(&[]).code, 2, "typo'd key");
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.miri]\nenabled = true\n"),
    );
    let planned = repo.check(&[]);
    assert_eq!(planned.code, 2);
    assert!(planned.stderr.contains("planned"));
    std::fs::remove_file(repo.file("discipline.toml")).unwrap();

    assert_eq!(
        repo.check(&["--suite", "verification"]).code,
        2,
        "empty suite must not pass"
    );
    assert_eq!(repo.check(&["--enable", "no-such-gate"]).code, 2);
    assert_eq!(repo.check(&["--enable", "pii", "--disable", "pii"]).code, 2);
    assert_eq!(repo.check(&["--config", "missing.toml"]).code, 2);
    assert_eq!(repo.check(&["--config-override", "gates = 3"]).code, 2);

    let outside = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"))
        .args(["check"])
        .current_dir(outside.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "not a git repository");
}

// ---- pre-commit (--staged) -------------------------------------------------

#[test]
fn staged_mode_inspects_the_index_and_softens_only_overridable_findings() {
    let repo = Repo::new();
    repo.write("tests/b.rs", "#[test]\nfn ghost() {}\n");
    repo.git(&["add", "tests/b.rs"]);
    let run = repo.check(&["--staged"]);
    assert_eq!(run.code, 1, "vacuous test has no override, stays an error");

    repo.git(&["reset", "-q", "--hard"]);
    repo.git(&["rm", "-q", "tests/a.rs"]);
    let run = repo.check(&["--staged"]);
    assert_eq!(
        run.code, 0,
        "no commit message exists yet to carry removes:"
    );
    assert_eq!(run.json()["warnings"], 1);
}

#[test]
fn unstaged_edits_are_invisible_to_staged_mode() {
    let repo = Repo::new();
    repo.write("tests/b.rs", "#[test]\nfn ghost() {}\n");
    assert_eq!(repo.check(&["--staged"]).code, 0);
}

// ---- auxiliary commands ----------------------------------------------------

#[test]
fn self_test_passes_and_gates_lists_effective_state() {
    let repo = Repo::new();
    let st = repo.run(&["self-test"], &[]);
    assert_eq!(st.code, 0, "{}", st.stdout);
    assert!(!st.stdout.contains("FAIL"));

    let gates = repo.run(&["gates", "--disable", "pii"], &[]);
    assert_eq!(gates.code, 0);
    let pii_line = gates
        .stdout
        .lines()
        .find(|l| l.starts_with("pii "))
        .unwrap();
    assert!(pii_line.contains("off"), "{pii_line}");
    assert!(gates
        .stdout
        .lines()
        .any(|l| l.starts_with("miri ") && l.contains("planned")));
}

#[test]
fn init_writes_a_config_that_loads_and_refuses_to_overwrite() {
    let repo = Repo::new();
    assert_eq!(repo.run(&["init", "--name", "demo"], &[]).code, 0);
    assert_eq!(
        repo.run(&["gates"], &[]).code,
        0,
        "generated config must load"
    );
    assert_eq!(repo.run(&["init"], &[]).code, 2);
}

#[test]
fn running_from_subdirectory_resolves_repo_config() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"demo\"\n\n[gates.pii]\nenabled = true\nhostname_denylist = [\"secret-internal.corp\"]\nexempt_paths = [\"discipline.toml\"]\n",
    );
    repo.commit("chore: add repo discipline.toml with denylist");
    repo.git(&["checkout", "-q", "-B", "work"]);

    repo.write(
        "docs/architecture.md",
        "Contact secret-internal.corp for keys.\n",
    );
    repo.commit("docs: mention host");

    // Running from subdirectory 'docs' must find root discipline.toml and fail on pii.
    let run = repo.run_in_dir(
        "docs",
        &["check", "--format", "json", "--base", "main"],
        &[],
    );
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert!(!run
        .stderr
        .contains("no discipline.toml; using built-in defaults"));
    assert_eq!(run.titles("pii").len(), 1);
}

#[test]
fn config_override_flag_compares_against_base_config() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"demo\"\n\n[gates.time-estimates]\nenabled = true\n",
    );
    repo.commit("chore: add discipline.toml");
    repo.git(&["checkout", "-q", "-B", "work"]);

    // Agent introduces a loose config and tries to use it via --config.
    repo.write(
        "loose.toml",
        "[meta]\nversion = 1\nname = \"demo\"\n\n[gates.time-estimates]\nenabled = false\n",
    );
    repo.commit("chore: add loose config");

    let run = repo.check(&["--config", "loose.toml"]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.titles("config-integrity").len(),
        1,
        "stdout: {}",
        run.stdout
    );
    assert!(
        run.outcome("config-integrity")["violations"][0]["message"]
            .as_str()
            .unwrap()
            .contains("time-estimates"),
        "stdout: {}",
        run.stdout
    );
}

#[test]
fn md_file_with_nul_byte_still_fires_time_estimates() {
    let repo = Repo::new();
    repo.write("docs/roadmap.md", "Ships in 3 weeks.\0\n");
    repo.commit("docs: add roadmap");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(run.titles("time-estimates").len(), 1);
}

#[test]
fn md_file_with_nul_byte_still_fires_pii() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"demo\"\n\n[gates.pii]\nenabled = true\nhostname_denylist = [\"leaked-host.corp\"]\nexempt_paths = [\"discipline.toml\"]\n",
    );
    repo.commit("chore: add discipline.toml");
    repo.git(&["checkout", "-q", "-B", "work"]);

    repo.write("docs/secret.md", "Host is leaked-host.corp.\0\n");
    repo.commit("docs: add secret");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 1,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(run.titles("pii").len(), 1);
}

#[test]
fn rs_file_with_nul_byte_fails_closed_exit_2() {
    let repo = Repo::new();
    repo.write("src/bad.rs", "pub fn foo() {}\0");
    repo.commit("feat: add bad.rs");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 2,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
}

#[test]
fn discipline_toml_with_nul_byte_fails_closed_exit_2() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"demo\"\0\n",
    );
    repo.commit("chore: add bad discipline.toml");
    repo.git(&["checkout", "-q", "-B", "work"]);

    let run = repo.check(&[]);
    assert_eq!(
        run.code, 2,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
}

#[test]
fn empty_tree_first_commit_staged_mode_passes() {
    let dir = tempfile::tempdir().unwrap();
    let repo_path = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(repo_path)
        .status()
        .unwrap();

    let doc_path = repo_path.join("docs/plan.md");
    std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
    std::fs::write(&doc_path, "# Plan\n\nPhase 1 then Phase 2.\n").unwrap();

    std::fs::write(repo_path.join("AGENTS.md"), "# Agent Guide\n").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("AGENTS.md", repo_path.join("CLAUDE.md")).unwrap();
        std::os::unix::fs::symlink("AGENTS.md", repo_path.join("GEMINI.md")).unwrap();
    }

    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo_path)
        .status()
        .unwrap();

    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"));
    cmd.args(["check", "--staged", "--format", "json"])
        .current_dir(repo_path);
    for var in [
        "PR_BODY",
        "GITHUB_STEP_SUMMARY",
        "DISCIPLINE_CONFIG",
        "DISCIPLINE_CONFIG_OVERRIDE",
        "DISCIPLINE_ENABLE",
        "DISCIPLINE_DISABLE",
        "DISCIPLINE_BASE_REF",
        "DISCIPLINE_FAIL_ON_WARNINGS",
        "DISCIPLINE_HOSTNAME_DENYLIST",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["base"], "empty tree (first commit)");
}

#[test]
fn empty_repo_unstaged_mode_fails_closed_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let repo_path = dir.path();
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(repo_path)
        .status()
        .unwrap();

    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"));
    cmd.args(["check", "--base", "main", "--format", "json"])
        .current_dir(repo_path);
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

// ---- Directives & Overrides Policy (T1 + T2) -------------------------------

#[test]
fn override_record_audit_trail_and_step_outputs() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
            "",
        ),
    );
    repo.commit("test: remove orders\n\nremoves: tests/a.rs orders moved to proptest");

    let step_summary_file = repo.dir.path().join("step_summary.md");
    let step_output_file = repo.dir.path().join("step_output.txt");

    let run = repo.run(
        &["check", "--format", "github-summary", "--base", "main"],
        &[
            ("GITHUB_STEP_SUMMARY", step_summary_file.to_str().unwrap()),
            ("GITHUB_OUTPUT", step_output_file.to_str().unwrap()),
        ],
    );

    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("Status: PASS"));
    assert!(run
        .stdout
        .contains("override applied: `removes: tests/a.rs orders moved to proptest` on `orders`"));
    assert!(run.stdout.contains("overrides: 1"));

    // Check GITHUB_OUTPUT contents
    let step_output = std::fs::read_to_string(&step_output_file).unwrap();
    assert!(step_output.contains("overrides=1"), "{step_output}");
    assert!(
        step_output.contains("overridden_gates=deletion-rationale"),
        "{step_output}"
    );
    assert!(step_output.contains("status=pass"), "{step_output}");

    // Check GITHUB_STEP_SUMMARY contents
    let step_summary = std::fs::read_to_string(&step_summary_file).unwrap();
    assert!(
        step_summary.contains("#### Overrides Applied (1)"),
        "{step_summary}"
    );
    assert!(
        step_summary.contains("| `deletion-rationale` | `removes` | `orders` |"),
        "{step_summary}"
    );

    // Check JSON output
    let json_run = repo.check(&[]);
    assert_eq!(json_run.code, 0);
    let json = json_run.json();
    assert_eq!(json["overrides"], 1);
    let outcome = json_run.outcome("deletion-rationale");
    let overrides = outcome["overrides"].as_array().unwrap();
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0]["directive"], "removes");
    assert_eq!(overrides[0]["subject"], "orders");
    assert_eq!(overrides[0]["hidden"], false);
}

#[test]
fn fail_on_overrides_blocks_change_with_exit_1() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
            "",
        ),
    );
    repo.commit("test: remove orders\n\nremoves: tests/a.rs orders moved to proptest");

    let run = repo.check(&["--fail-on-overrides"]);
    assert_eq!(run.code, 1, "should fail when --fail-on-overrides is set");

    let terminal_run = repo.run(&["check", "--base", "main", "--fail-on-overrides"], &[]);
    assert_eq!(terminal_run.code, 1);
    assert!(terminal_run.stdout.contains("Status: FAILED"));
    assert!(terminal_run
        .stdout
        .contains("failure: applied overrides require human sign-off"));
}

#[test]
fn hidden_directives_rejected_by_default_and_accepted_when_configured() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
            "",
        ),
    );
    // Hidden in HTML comments
    repo.commit("test: remove orders\n\n<!-- removes: tests/a.rs orders moved to proptest -->");

    // By default, allow_hidden = false, so directive is ignored
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "hidden directive must be rejected by default");
    assert_eq!(run.titles("deletion-rationale").len(), 1);
    let outcome = run.outcome("deletion-rationale");
    let notes = outcome["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("hidden directive")),
        "expected note about hidden directive being ignored"
    );

    // Now configure allow_hidden = true in discipline.toml
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\nallow_hidden = true\n",
    );
    repo.commit("chore: allow hidden directives");
    let run_allowed = repo.check(&[]);
    assert_eq!(
        run_allowed.code, 0,
        "hidden directive must be accepted when allow_hidden = true: {}",
        run_allowed.stdout
    );
    let json = run_allowed.json();
    assert_eq!(json["overrides"], 1);
    let outcome = run_allowed.outcome("deletion-rationale");
    assert_eq!(outcome["overrides"][0]["hidden"], true);
}

#[test]
fn directive_sources_policy_restricts_sources() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\nsources = [\"pr-body\"]\n",
    );
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
            "",
        ),
    );
    // Commit message directive when sources = ["pr-body"]
    repo.commit("test: remove orders\n\nremoves: tests/a.rs orders moved to proptest");

    let run_blocked = repo.check(&[]);
    assert_eq!(
        run_blocked.code, 1,
        "commit directive must be ignored when sources = [pr-body]"
    );
    let outcome = run_blocked.outcome("deletion-rationale");
    let notes = outcome["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("commit-message directives are disabled by policy")),
        "expected note about commit directives disabled"
    );

    // Now pass the directive in PR_BODY
    let run_pr_body = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("PR_BODY", "removes: tests/a.rs orders moved to proptest")],
    );
    assert_eq!(
        run_pr_body.code, 0,
        "pr-body directive must be accepted: {}",
        run_pr_body.stdout
    );
    let json = run_pr_body.json();
    assert_eq!(json["overrides"], 1);
    assert_eq!(json_pr_body_outcome_source(&json), "PrBody");
}

fn json_pr_body_outcome_source(json: &serde_json::Value) -> String {
    json["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "deletion-rationale")
        .unwrap()["overrides"][0]["source"]["type"]
        .as_str()
        .unwrap()
        .to_string()
}
