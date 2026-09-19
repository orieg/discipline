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
    repo.write("tools/check.swift", "func testNothing() {}\n");
    repo.write("src/App.kt", "class App {}\n");
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
fn unsupported_source_files_group_by_extension() {
    let repo = Repo::new();
    repo.write("ext/judy.scala", "class Foo {}\n");
    repo.write("ext/judy.swift", "class Bar {}\n");
    repo.write("tests/001.kt", "fun testFoo() {}\n");
    repo.commit("feat: scala, swift, and kotlin");
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
                && notes.contains("1 .kt, 1 .scala, 1 .swift")
                && notes.contains("NOT analysed"),
            "gate {gate} should format extension breakdown: {notes}"
        );
    }
}

#[test]
fn phpt_source_files_are_analysed_by_golden_pack() {
    let repo = Repo::new();
    repo.write(
        "tests/001.phpt",
        "--TEST--\nSample\n--FILE--\n<?php echo 1;\n--EXPECT--\n1\n",
    );
    repo.commit("feat: phpt test");
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
            !notes.contains("NOT analysed"),
            "gate {gate} should analyse phpt files: {notes}"
        );
    }
}

#[test]
fn go_source_files_are_analysed_by_go_pack() {
    let repo = Repo::new();
    repo.write(
        "calc_test.go",
        "package calc_test\nimport \"testing\"\nfunc TestCalc(t *testing.T) { if 1+1 != 2 { t.Fatalf(\"fail\") } }\n",
    );
    repo.commit("feat: go test");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0);
    for gate in ["assertion-reduction", "vacuous-tests", "ignored-tests"] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            !notes.contains("NOT analysed"),
            "gate {gate} should analyse go files: {notes}"
        );
    }
}

#[test]
fn php_source_files_are_analysed_by_php_pack() {
    let repo = Repo::new();
    repo.write(
        "tests/CalcTest.php",
        "<?php\nclass CalcTest extends TestCase {\n  public function testCalc() { $this->assertEquals(2, 1 + 1); }\n}\n",
    );
    repo.commit("feat: php test");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0);
    for gate in ["assertion-reduction", "vacuous-tests", "ignored-tests"] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            !notes.contains("NOT analysed"),
            "gate {gate} should analyse php files: {notes}"
        );
    }
}

#[test]
fn c_cpp_source_files_are_analysed_by_c_cpp_pack() {
    let repo = Repo::new();
    repo.write(
        "crates/expanse-capi/smoke/modern_api_smoke.c",
        "#include <assert.h>\nint main(void) { int v = 42; assert(v == 42); return 0; }\n",
    );
    repo.write(
        "tests/test_vector.cpp",
        "TEST(VectorSuite, Push) { int s = 1; EXPECT_EQ(s, 1); }\n",
    );
    repo.commit("feat: c and cpp tests");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    for gate in ["assertion-reduction", "vacuous-tests", "ignored-tests"] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            !notes.contains("NOT analysed"),
            "gate {gate} should analyse c/cpp files: {notes}"
        );
    }
}

#[test]
fn csharp_source_files_are_analysed_by_csharp_pack() {
    let repo = Repo::new();
    repo.write(
        "tests/ExpanseMapTests.cs",
        "using Xunit;\npublic class ExpanseMapTests {\n    [Fact]\n    public void BasicCrud() { int val = 42; Assert.Equal(42, val); }\n}\n",
    );
    repo.commit("feat: csharp tests");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    for gate in ["assertion-reduction", "vacuous-tests", "ignored-tests"] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            !notes.contains("NOT analysed"),
            "gate {gate} should analyse csharp files: {notes}"
        );
    }
}

#[test]
fn ruby_source_files_are_analysed_by_ruby_pack() {
    let repo = Repo::new();
    repo.write(
        "test/test_expanse.rb",
        "class TestExpanse < Minitest::Test\n  def test_crud\n    val = 42\n    assert_equal 42, val\n  end\nend\n",
    );
    repo.commit("feat: ruby tests");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    for gate in ["assertion-reduction", "vacuous-tests", "ignored-tests"] {
        let notes = run.outcome(gate)["notes"].to_string();
        assert!(
            !notes.contains("NOT analysed"),
            "gate {gate} should analyse ruby files: {notes}"
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
    assert_eq!(run.titles("ignored-tests"), vec!["Test Newly Skipped"]);

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
    assert_eq!(run.titles("ignored-tests"), vec!["Test Newly Skipped"]);
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
fn f1_repro_gutted_test_with_unrelated_offsetting_test_fails_without_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/math.rs",
        "#[test]\nfn adds() {\n    assert_eq!(1 + 1, 2);\n    assert_eq!(2 + 2, 4);\n}\n#[test]\nfn multiplies() {\n    assert_eq!(2 * 3, 6);\n}\n",
    );
    repo.commit("feat: base math tests");

    // Change `adds` to a single `assert!(x > 0)` and append a new `unrelated` test with 2x assert_eq!
    repo.write(
        "tests/math.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    assert!(x > 0);\n}\n#[test]\nfn multiplies() {\n    assert_eq!(2 * 3, 6);\n}\n#[test]\nfn unrelated() {\n    assert_eq!(10, 10);\n    assert_eq!(20, 20);\n}\n",
    );
    repo.commit("feat: gut adds and add unrelated");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "gutted test must fail closed even when offset: {}{}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);
    assert!(outcome["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("adds"));
}

#[test]
fn r1_test_split_requires_allow_assertion_drop_directive() {
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

    // Without directive: must fail because test_combo's assertions dropped
    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "split without directive must fail assertion-reduction"
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);

    // With directive naming old test: passes
    let run_pass = repo.run(
        &["check", "--base", "HEAD~1", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-assertion-drop: test_combo split into part1 and part2",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("assertion-reduction");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn f1_repro_python_gutted_test_with_unrelated_offsetting_test_fails_without_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/test_math.py",
        "def test_adds():\n    assert 1 + 1 == 2\n    assert 2 + 2 == 4\n\ndef test_multiplies():\n    assert 2 * 3 == 6\n",
    );
    repo.commit("feat: python math tests");

    repo.write(
        "tests/test_math.py",
        "def test_adds():\n    assert 1 > 0\n\ndef test_multiplies():\n    assert 2 * 3 == 6\n\ndef test_unrelated():\n    assert 10 == 10\n    assert 20 == 20\n",
    );
    repo.commit("feat: gut adds and add unrelated");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "python gutted test must fail closed: {}{}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);
    assert!(outcome["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("test_adds"));
}

#[test]
fn f1_repro_javascript_gutted_test_with_unrelated_offsetting_test_fails_without_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/math.test.js",
        "test('adds', () => {\n    expect(1 + 1).toBe(2);\n    expect(2 + 2).toBe(4);\n});\ntest('multiplies', () => {\n    expect(2 * 3).toBe(6);\n});\n",
    );
    repo.commit("feat: js math tests");

    repo.write(
        "tests/math.test.js",
        "test('adds', () => {\n    expect(1 > 0).toBeTruthy();\n});\ntest('multiplies', () => {\n    expect(2 * 3).toBe(6);\n});\ntest('unrelated', () => {\n    expect(10).toBe(10);\n    expect(20).toBe(20);\n});\n",
    );
    repo.commit("feat: gut adds and add unrelated");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "js gutted test must fail closed: {}{}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);
    assert!(outcome["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("adds"));
}

#[test]
fn f4_javascript_matcher_weakening_detected_and_accepts_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/calc.test.js",
        "test('calc', () => {\n    expect(1 + 1).toBe(2);\n    expect({ a: 1 }).toEqual({ a: 1 });\n    expect([1, 2]).toHaveLength(2);\n    expect({ a: 1 }).toHaveProperty('a', 1);\n    expect(spy).toHaveBeenCalledWith(42);\n});\n",
    );
    repo.commit("feat: initial strong js tests");

    // Matcher count unchanged (5 assertions), but all weakened to weak matchers:
    // toBe -> toBeTruthy, toEqual -> toBeDefined, toHaveLength -> toBeDefined,
    // toHaveProperty -> toBeTruthy, toHaveBeenCalledWith -> toHaveBeenCalled
    repo.write(
        "tests/calc.test.js",
        "test('calc', () => {\n    expect(1 + 1).toBeTruthy();\n    expect({ a: 1 }).toBeDefined();\n    expect([1, 2]).toBeDefined();\n    expect({ a: 1 }).toBeTruthy();\n    expect(spy).toHaveBeenCalled();\n});\n",
    );
    repo.commit("feat: weaken matchers to truthy and defined");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "js matcher weakening must flag violation with exit 1: {}{}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);
    assert!(outcome["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("dropped from 5 to 0"));

    // Can be excused by directive
    let run_pass = repo.run(
        &["check", "--base", "HEAD~1", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-assertion-drop: calc intentional matcher weakening for broad compatibility",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("assertion-reduction");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn f1_repro_phpt_gutted_test_with_unrelated_offsetting_test_fails_without_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/adds.phpt",
        "--TEST--\nadds test\n--FILE--\n<?php\necho '1\n2\n3\n4\n';\n--EXPECT--\n1\n2\n3\n4\n",
    );
    repo.commit("feat: phpt base test");

    repo.write(
        "tests/adds.phpt",
        "--TEST--\nadds test\n--FILE--\n<?php\necho '1\n';\n--EXPECT--\n1\n",
    );
    repo.write(
        "tests/unrelated.phpt",
        "--TEST--\nunrelated test\n--FILE--\n<?php\necho '10\n20\n30\n';\n--EXPECT--\n10\n20\n30\n",
    );
    repo.commit("feat: gut adds and add unrelated");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 1,
        "phpt gutted test must fail closed: {}{}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);
    assert!(outcome["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("adds test"));
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

#[test]
fn f2_repro_brief_time_estimates_all_fire_and_negative_controls_pass() {
    let repo = Repo::new();
    repo.write(
        "docs/good.md",
        r#"# Good
The job took 29 min on the hosted runner.
Miri shards are capped at 180 minutes.
Runs once a day; the cache is a month old.
**Q1 — what do our patches buy?**
libjudy is 19 years old.
"#,
    );
    repo.commit("docs: good");
    let clean_run = repo.check(&[]);
    assert_eq!(
        clean_run.code, 0,
        "clean docs must pass with exit 0: stdout: {}\nstderr: {}",
        clean_run.stdout, clean_run.stderr
    );

    let bad_repo = Repo::new();
    bad_repo.write(
        "docs/bad.md",
        r#"# Planning
The capacity work is 2 weeks.
The migration took a decision; rollout is 3 weeks.
Timeout budget aside, we ship in 2 weeks.
Work limit: 3 sprints of effort.
Expect it in two weeks.
ETA: a month.
Since 2005 we planned this; done in 6 months.
The cap on scope means about 4 weeks (measured).
This is 2 weeks of work.
Phase 2 takes 3 weeks.
Maximum effort: 10 engineer-days.
The firewall change lands in 5 days.
Phase 3 (1 week).

| Task | Latency |
|---|---|
| Rewrite parser | 2 weeks |
"#,
    );
    bad_repo.commit("docs: bad");
    let bad_run = bad_repo.check(&[]);
    assert_eq!(bad_run.code, 1);
    let outcome = bad_run.outcome("time-estimates");
    let violations = outcome["violations"].as_array().unwrap();
    assert_eq!(
        violations.len(),
        14,
        "expected all 14 brief planning lines to fire: {violations:#?}"
    );
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
        repo.check(&["--suite", "quality"]).code,
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
fn newly_added_nul_byte_in_source_flags_violation_exit_1_and_accepts_directive() {
    let repo = Repo::new();
    repo.write(
        "tests/string_to_entry_005.phpt",
        "--TEST--\nJudy string with nul\n--FILE--\n<?php $j->set(\"foo\0bar\", 123);\n--EXPECT--\nok\n",
    );
    repo.commit("feat: add phpt with nul");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 1,
        "newly added NUL byte must flag violation with exit 1: stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-nul: tests/string_to_entry_005.phpt binary cache payload",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("assertion-reduction");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn pre_existing_nul_on_base_analyzed_lossily_without_exit_2() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "tests/string_to_entry_005.phpt",
        "--TEST--\nJudy string with nul\n--FILE--\n<?php $j->set(\"foo\0bar\", 123);\n--EXPECT--\nok\n",
    );
    repo.commit("feat: add phpt with nul on main");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("docs/note.md", "# Note\nClean update.\n");
    repo.commit("docs: update note");

    let run = repo.check(&[]);
    assert_eq!(
        run.code, 0,
        "pre-existing NUL on base must analyze lossily and pass with exit 0: stdout: {}\nstderr: {}",
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
        "GITHUB_BASE_REF",
        "GITHUB_EVENT_PATH",
        "GITEA_BASE_REF",
        "GITEA_EVENT_PATH",
        "FORGEJO_BASE_REF",
        "FORGEJO_EVENT_PATH",
        "FORGEJO_ACTIONS",
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

#[test]
fn config_override_sources_reset_narrows_directive_sources() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
            "",
        ),
    );
    repo.commit("test: remove orders");

    // By default, sources are ["pr-body", "commits"]. A pr-body override works:
    let run_default = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("PR_BODY", "removes: tests/a.rs orders removed")],
    );
    assert_eq!(
        run_default.code, 0,
        "default sources must accept pr-body override: {}",
        run_default.stdout
    );

    // Override sources with reset to commits-only:
    let override_toml = "[directives]\nsources = [\"__reset__\", \"commits\"]\n";
    let run_narrowed = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "json",
            "--config-override",
            override_toml,
        ],
        &[("PR_BODY", "removes: tests/a.rs orders removed")],
    );
    assert_eq!(
        run_narrowed.code, 1,
        "pr-body directive must be rejected when sources reset to commits only"
    );
    let outcome = run_narrowed.outcome("deletion-rationale");
    let notes = outcome["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("PR-body directives are disabled by policy")),
        "expected note about PR-body directives disabled by policy, got: {:?}",
        notes
    );
}

#[test]
fn diff_command_accepts_config_and_json_out() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    assert!(1 + 1 == 2);\n}\n",
    );
    repo.commit("test: drop assertion\n\nremoves: orders removed");
    let json_file = repo.file("report.json");
    let json_path = json_file.to_string_lossy().to_string();

    // 1. diff command with --json-out and --disable
    let run = repo.run(
        &[
            "diff",
            "--base",
            "main",
            "--disable",
            "assertion-reduction",
            "--format",
            "terminal",
            "--json-out",
            &json_path,
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(json_file.exists());
    let content = std::fs::read_to_string(&json_file).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(json["errors"], 0);

    // 2. without disabling assertion-reduction, diff fails
    let run_fail = repo.run(&["diff", "--base", "main", "--format", "json"], &[]);
    assert_eq!(run_fail.code, 1);
    let fail_json: serde_json::Value = serde_json::from_str(&run_fail.stdout).unwrap();
    assert!(fail_json["errors"].as_u64().unwrap() >= 1);
}

#[test]
fn commit_msg_file_flag_is_accepted() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("assert_eq!(x + 3, 4);\n", ""),
    );
    repo.commit("test: drop assertion without justification");

    let msg_file = repo.file("commit_msg.txt");
    std::fs::write(
        &msg_file,
        "feat: update\n\nallow-assertion-drop: adds test updated\n",
    )
    .unwrap();

    let run = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--commit-msg-file",
            &msg_file.to_string_lossy(),
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let json = run.json();
    assert_eq!(json["overrides"], 1);
}

#[test]
fn golden_output_gate_fires_on_modified_snapshot_and_accepts_override() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    repo.write("tests/snapshots/result.snap", "output: line 1\n");
    repo.commit("feat: initial snapshot");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Modify snapshot without override
    repo.write("tests/snapshots/result.snap", "output: modified\n");
    repo.commit("feat: modify snapshot");

    let run_fail = repo.check(&[]);
    assert_eq!(run_fail.code, 1);
    let outcome = run_fail.outcome("golden-output");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);
    assert_eq!(outcome["examined"], 1);
    assert_eq!(
        outcome["violations"][0]["file"],
        "tests/snapshots/result.snap"
    );

    // With scoped override directive
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-golden-update: tests/snapshots/result.snap re-blessed output",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("golden-output");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn golden_output_gate_ignores_added_snapshot() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn f() {}\n");
    repo.commit("feat: initial");

    repo.write("tests/snapshots/new.snap", "output: new\n");
    repo.commit("feat: add new snapshot");

    let run = repo.check(&[]);
    assert_eq!(run.code, 0);
    let outcome = run.outcome("golden-output");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome["examined"], 0);
}

#[test]
fn phpt_assertion_reduction_and_ignored_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_phpt = "--TEST--\nJudy test\n--FILE--\n<?php echo 1;\n--EXPECT--\nline 1\nline 2\n";
    repo.write("tests/judy.phpt", base_phpt);
    repo.commit("feat: initial phpt test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Head shrinks EXPECT from 2 lines to 1
    let weaker_phpt = "--TEST--\nJudy test\n--FILE--\n<?php echo 1;\n--EXPECT--\nline 1\n";
    repo.write("tests/judy.phpt", weaker_phpt);
    repo.commit("test: weaken expect");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 1);

    // Now test XFAIL addition
    let repo2 = Repo::new();
    repo2.git(&["checkout", "main"]);
    repo2.write("tests/judy.phpt", base_phpt);
    repo2.commit("feat: initial phpt test");
    repo2.git(&["checkout", "-B", "work", "main"]);

    let xfail_phpt = "--TEST--\nJudy test\n--XFAIL--\nKnown bug\n--FILE--\n<?php echo 1;\n--EXPECT--\nline 1\nline 2\n";
    repo2.write("tests/judy.phpt", xfail_phpt);
    repo2.commit("test: mark xfail");

    let run_xfail = repo2.check(&[]);
    assert_eq!(run_xfail.code, 1);
    let outcome_xfail = run_xfail.outcome("ignored-tests");
    assert_eq!(outcome_xfail["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn python_source_files_are_analysed_by_python_pack() {
    let repo = Repo::new();
    repo.write(
        "tests/test_math.py",
        "def test_square():\n    x = 3\n    assert x * x == 9\n",
    );
    repo.commit("feat: python test");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let outcome = run.outcome("assertion-reduction");
    assert!(!outcome["notes"].to_string().contains("NOT analysed"));
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 0);
}

#[test]
fn python_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_py = "def test_calc():\n    assert 1 + 1 == 2\n    assert 2 + 2 == 4\n";
    repo.write("tests/test_calc.py", base_py);
    repo.commit("feat: initial python test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_py = "def test_calc():\n    assert 1 + 1 == 2\n";
    repo.write("tests/test_calc.py", weaker_py);
    repo.commit("test: weaken assertions");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write("tests/test_empty.py", "def test_nothing():\n    pass\n");
    repo2.commit("test: add empty test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn python_ignored_tests_and_skip_decorators_detected() {
    let repo = Repo::new();
    repo.write(
        "tests/test_skip.py",
        "import pytest\n\n@pytest.mark.skip(reason=\"not ready\")\ndef test_skipped():\n    assert 1 + 1 == 2\n",
    );
    repo.commit("test: add skipped test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: test_skipped skipped until new backend is ready",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn f5_python_context_managers_and_pytestmark_detected() {
    // 1. Context manager assertions (with self.assertRaises) pass vacuous-tests gate
    let repo = Repo::new();
    repo.write(
        "tests/test_raises.py",
        "import unittest\n\nclass TestErrors(unittest.TestCase):\n    def test_val_error(self):\n        with self.assertRaises(ValueError):\n            int('invalid')\n",
    );
    repo.commit("test: add assertRaises test");
    let run = repo.check(&[]);
    assert_eq!(
        run.code, 0,
        "assertRaises context manager must count as assertion and not be vacuous: {}{}",
        run.stdout, run.stderr
    );
    let outcome_vac = run.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 0);

    // 2. Class-level pytestmark skip detected under ignored-tests gate
    let repo2 = Repo::new();
    repo2.write(
        "tests/test_class_skip.py",
        "import unittest\nimport pytest\n\nclass TestSkipped(unittest.TestCase):\n    pytestmark = pytest.mark.skip(reason='class wip')\n\n    def test_something(self):\n        self.assertEqual(1 + 1, 2)\n",
    );
    repo2.commit("test: add class with pytestmark skip");
    let run_skip = repo2.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo2.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: test_something class is work in progress",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
}

#[test]
fn javascript_source_files_are_analysed_by_javascript_pack() {
    let repo = Repo::new();
    repo.write(
        "tests/auth.test.ts",
        "describe('Auth', () => {\n    it('authenticates', () => {\n        expect(1 + 1).toBe(2);\n    });\n});\n",
    );
    repo.commit("feat: ts test");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let outcome = run.outcome("assertion-reduction");
    assert!(!outcome["notes"].to_string().contains("NOT analysed"));
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 0);
}

#[test]
fn javascript_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_js =
        "test('calc', () => {\n    expect(1 + 1).toBe(2);\n    expect(2 + 2).toBe(4);\n});\n";
    repo.write("tests/calc.test.js", base_js);
    repo.commit("feat: initial js test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_js = "test('calc', () => {\n    expect(1 + 1).toBe(2);\n});\n";
    repo.write("tests/calc.test.js", weaker_js);
    repo.commit("test: weaken assertions");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write("tests/empty.test.js", "test('empty', () => {});\n");
    repo2.commit("test: add empty test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn javascript_ignored_tests_and_skips_detected() {
    let repo = Repo::new();
    repo.write(
        "tests/skip.test.ts",
        "it.skip('skips this test', () => {\n    expect(1 + 1).toBe(2);\n});\n",
    );
    repo.commit("test: add skipped test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: skips this test skipped for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn f6_python_pack_depth_e2e() {
    // 1. Negative control: clean suite passes cleanly
    let repo_clean = Repo::new();
    repo_clean.write(
        "tests/test_clean.py",
        include_str!("fixtures/python/clean_suite.py"),
    );
    repo_clean.commit("test: add clean python tests");
    let run_clean = repo_clean.check(&[]);
    assert_eq!(
        run_clean.code, 0,
        "clean python tests must pass: {}{}",
        run_clean.stdout, run_clean.stderr
    );

    // 2. Positive control: vacuous & tautological tests detected
    let repo_vac = Repo::new();
    repo_vac.write(
        "tests/test_vacuous.py",
        include_str!("fixtures/python/vacuous_suite.py"),
    );
    repo_vac.commit("test: add vacuous python tests");
    let run_vac = repo_vac.check(&[]);
    assert_eq!(run_vac.code, 1);
    let out_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(out_vac["violations"].as_array().unwrap().len(), 5);

    // 3. Positive control: skips detected at all levels
    let repo_skip = Repo::new();
    repo_skip.write(
        "tests/test_skips.py",
        include_str!("fixtures/python/skips_suite.py"),
    );
    repo_skip.commit("test: add skips python tests");
    let run_skip = repo_skip.check(&[]);
    assert_eq!(run_skip.code, 1);
    let out_skip = run_skip.outcome("ignored-tests");
    assert_eq!(out_skip["violations"].as_array().unwrap().len(), 4);

    // 4. Scoped directive lifts skip
    let run_lift = repo_skip.run(
        &["check", "--base", "main", "--format", "json"],
        &[("PR_BODY", "allow-ignore: test_skipped_fn intentional skip")],
    );
    let out_lift = run_lift.outcome("ignored-tests");
    assert_eq!(out_lift["overrides"].as_array().unwrap().len(), 1);

    // 5. Parse error fails closed under assertion-reduction
    let repo_err = Repo::new();
    repo_err.write(
        "tests/test_broken.py",
        include_str!("fixtures/python/syntax_error.py"),
    );
    repo_err.commit("test: add broken python syntax");
    let run_err = repo_err.check(&[]);
    assert_eq!(run_err.code, 1);
    let out_err = run_err.outcome("assertion-reduction");
    assert!(out_err["violations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["title"] == "Source File Could Not Be Fully Parsed"));
}

#[test]
fn f6_javascript_pack_depth_e2e() {
    // 1. Negative control: clean suite passes cleanly
    let repo_clean = Repo::new();
    repo_clean.write(
        "tests/clean.test.js",
        include_str!("fixtures/javascript/clean_suite.js"),
    );
    repo_clean.commit("test: add clean js tests");
    let run_clean = repo_clean.check(&[]);
    assert_eq!(
        run_clean.code, 0,
        "clean js tests must pass: {}{}",
        run_clean.stdout, run_clean.stderr
    );

    // 2. Positive control: vacuous & tautological tests detected
    let repo_vac = Repo::new();
    repo_vac.write(
        "tests/vacuous.test.js",
        include_str!("fixtures/javascript/vacuous_suite.js"),
    );
    repo_vac.commit("test: add vacuous js tests");
    let run_vac = repo_vac.check(&[]);
    assert_eq!(run_vac.code, 1);
    let out_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(out_vac["violations"].as_array().unwrap().len(), 4);

    // 3. Positive control: skips detected at all levels
    let repo_skip = Repo::new();
    repo_skip.write(
        "tests/skips.test.js",
        include_str!("fixtures/javascript/skips_suite.js"),
    );
    repo_skip.commit("test: add skips js tests");
    let run_skip = repo_skip.check(&[]);
    assert_eq!(run_skip.code, 1);
    let out_skip = run_skip.outcome("ignored-tests");
    assert_eq!(out_skip["violations"].as_array().unwrap().len(), 4);

    // 4. Scoped directive lifts skip
    let run_lift = repo_skip.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: skipped test function intentional skip",
        )],
    );
    let out_lift = run_lift.outcome("ignored-tests");
    assert_eq!(out_lift["overrides"].as_array().unwrap().len(), 1);

    // 5. Parse error fails closed under assertion-reduction
    let repo_err = Repo::new();
    repo_err.write(
        "tests/broken.test.js",
        include_str!("fixtures/javascript/syntax_error.js"),
    );
    repo_err.commit("test: add broken js syntax");
    let run_err = repo_err.check(&[]);
    assert_eq!(run_err.code, 1);
    let out_err = run_err.outcome("assertion-reduction");
    assert!(out_err["violations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["title"] == "Source File Could Not Be Fully Parsed"));
}

#[test]
fn f6_golden_pack_depth_e2e() {
    // 1. Negative control: clean PHPT suite passes cleanly
    let repo_clean = Repo::new();
    repo_clean.write(
        "tests/clean.phpt",
        include_str!("fixtures/golden/clean.phpt"),
    );
    repo_clean.commit("test: add clean phpt test");
    let run_clean = repo_clean.check(&[]);
    assert_eq!(
        run_clean.code, 0,
        "clean phpt test must pass: {}{}",
        run_clean.stdout, run_clean.stderr
    );

    // 2. Positive control: vacuous empty expectation detected
    let repo_vac = Repo::new();
    repo_vac.write(
        "tests/vacuous.phpt",
        include_str!("fixtures/golden/vacuous.phpt"),
    );
    repo_vac.commit("test: add vacuous phpt test");
    let run_vac = repo_vac.check(&[]);
    assert_eq!(run_vac.code, 1);
    let out_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(out_vac["violations"].as_array().unwrap().len(), 1);

    // 3. Positive control: skips detected
    let repo_skip = Repo::new();
    repo_skip.write(
        "tests/skip.phpt",
        include_str!("fixtures/golden/skips.phpt"),
    );
    repo_skip.commit("test: add skipped phpt test");
    let run_skip = repo_skip.check(&[]);
    assert_eq!(run_skip.code, 1);
    let out_skip = run_skip.outcome("ignored-tests");
    assert_eq!(out_skip["violations"].as_array().unwrap().len(), 1);

    // 4. Scoped directive lifts skip
    let run_lift = repo_skip.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: Skipped PHPT Test intentional skip",
        )],
    );
    assert_eq!(run_lift.code, 0, "{}{}", run_lift.stdout, run_lift.stderr);
    let out_lift = run_lift.outcome("ignored-tests");
    assert_eq!(out_lift["overrides"].as_array().unwrap().len(), 1);

    // 5. Parse error fails closed under assertion-reduction
    let repo_err = Repo::new();
    repo_err.write(
        "tests/broken.phpt",
        include_str!("fixtures/golden/syntax_error.phpt"),
    );
    repo_err.commit("test: add broken phpt syntax");
    let run_err = repo_err.check(&[]);
    assert_eq!(run_err.code, 1);
    let out_err = run_err.outcome("assertion-reduction");
    assert!(out_err["violations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["title"] == "Source File Could Not Be Fully Parsed"));
}

// ---- bench-regression ------------------------------------------------------

#[test]
fn bench_regression_tracks_callgrind_instructions_and_accepts_override() {
    let repo = Repo::new();
    repo.commit_base(
        "target/iai/bench/callgrind.bench.out",
        "events: Ir\nsummary: 100000\n",
        "base: callgrind baseline",
    );

    // +1.0% regression (> 0.5% tolerance)
    repo.write(
        "target/iai/bench/callgrind.bench.out",
        "events: Ir\nsummary: 101000\n",
    );
    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(run_fail.code, 1);
    let json_fail = run_fail.json();
    assert_eq!(json_fail["errors"], 1);
    assert_eq!(json_fail["warnings"], 0);
    let outcome_fail = json_fail["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert_eq!(
        outcome_fail["violations"][0]["title"],
        "Instruction Count Regressed"
    );
    assert!(outcome_fail["violations"][0]["message"]
        .as_str()
        .unwrap()
        .contains("+1.00%"));

    // Override with allow-regression: bench <reason>
    let run_pass = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: bench trade instructions for lower memory",
    );
    assert_eq!(run_pass.code, 0);
    let json_pass = run_pass.json();
    assert_eq!(json_pass["errors"], 0);
    assert_eq!(json_pass["overrides"], 1);
}

#[test]
fn bench_regression_tracks_go_benchmarks_and_accepts_override() {
    let repo = Repo::new();
    let base_txt = "goos: darwin\ngoarch: arm64\npkg: gobench\ncpu: Apple M1\nBenchmarkSearch-8   \t200000000\t         5.835 ns/op\t       0 B/op\t       0 allocs/op\nBenchmarkInsert-8   \t100000000\t        10.200 ns/op\t       0 B/op\t       0 allocs/op\nPASS\n";
    repo.commit_base("benchmarks/go.txt", base_txt, "base: go baseline");

    // Wall-clock data without CI: point-estimate increase degrades to not_comparable (exit 0)
    let head_regressed = "goos: darwin\ngoarch: arm64\npkg: gobench\ncpu: Apple M1\nBenchmarkSearch-8   \t200000000\t        10.500 ns/op\t       0 B/op\t       0 allocs/op\nBenchmarkInsert-8   \t100000000\t        10.200 ns/op\t       0 B/op\t       0 allocs/op\nPASS\n";
    repo.write("benchmarks/go.txt", head_regressed);
    let run_not_comparable = repo.check(&["--suite", "bench"]);
    assert_eq!(
        run_not_comparable.code, 0,
        "wall-clock benchmark without CI must degrade to not comparable, never a point-estimate failure"
    );
    let json = run_not_comparable.json();
    assert_eq!(json["errors"], 0);
    let outcome = json["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert!(outcome["notes"].as_array().unwrap().iter().any(|n| n
        .as_str()
        .unwrap()
        .contains("not comparable (no CI available)")));

    // Removing BenchmarkSearch without an override fails (exit 1)
    let head_removed = "goos: darwin\ngoarch: arm64\npkg: gobench\ncpu: Apple M1\nBenchmarkInsert-8   \t100000000\t        10.200 ns/op\t       0 B/op\t       0 allocs/op\nPASS\n";
    repo.write("benchmarks/go.txt", head_removed);
    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(run_fail.code, 1);
    let json_fail = run_fail.json();
    assert_eq!(json_fail["errors"], 1);
    let outcome_fail = json_fail["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert_eq!(outcome_fail["violations"][0]["title"], "Benchmark Removed");

    let run_pass = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: BenchmarkSearch justified for removal",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 1);
}

#[test]
fn bench_regression_tracks_google_benchmark_json_and_accepts_override() {
    let repo = Repo::new();
    let base_json = r#"{"benchmarks": [
        {"name": "BM_StringCreation", "cpu_time": 120.0, "time_unit": "ns"},
        {"name": "BM_StringCopy", "cpu_time": 50.0, "time_unit": "ns"}
    ]}"#;
    repo.commit_base("build/benchmarks.json", base_json, "base: gbench baseline");

    // Wall-clock data without CI: point-estimate increase degrades to not_comparable (exit 0)
    let head_regressed = r#"{"benchmarks": [
        {"name": "BM_StringCreation", "cpu_time": 999.0, "time_unit": "ns"},
        {"name": "BM_StringCopy", "cpu_time": 50.0, "time_unit": "ns"}
    ]}"#;
    repo.write("build/benchmarks.json", head_regressed);
    let run_not_comparable = repo.check(&["--suite", "bench"]);
    assert_eq!(
        run_not_comparable.code, 0,
        "wall-clock benchmark without CI must degrade to not comparable, never a point-estimate failure"
    );
    let json = run_not_comparable.json();
    assert_eq!(json["errors"], 0);
    let outcome = json["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert!(outcome["notes"].as_array().unwrap().iter().any(|n| n
        .as_str()
        .unwrap()
        .contains("not comparable (no CI available)")));

    // Removing BM_StringCreation entirely without an override fails (exit 1)
    let head_removed = r#"{"benchmarks": [
        {"name": "BM_StringCopy", "cpu_time": 50.0, "time_unit": "ns"}
    ]}"#;
    repo.write("build/benchmarks.json", head_removed);
    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(run_fail.code, 1);
    let json_fail = run_fail.json();
    assert_eq!(json_fail["errors"], 1);
    let outcome_fail = json_fail["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert_eq!(outcome_fail["violations"][0]["title"], "Benchmark Removed");

    let run_pass = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: BM_StringCreation justified for removal",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 1);
}

#[test]
fn bench_regression_tracks_pytest_benchmark_json_and_accepts_override() {
    let repo = Repo::new();
    let base_json = r#"{"benchmarks": [
        {"name": "test_serialize", "stats": {"mean": 0.0010}},
        {"name": "test_deserialize", "stats": {"mean": 0.0020}}
    ]}"#;
    repo.commit_base(
        "reports/pytest_bench.json",
        base_json,
        "base: pytest baseline",
    );

    // Wall-clock data without CI: point-estimate increase degrades to not_comparable (exit 0)
    let head_regressed = r#"{"benchmarks": [
        {"name": "test_serialize", "stats": {"mean": 0.050}},
        {"name": "test_deserialize", "stats": {"mean": 0.0020}}
    ]}"#;
    repo.write("reports/pytest_bench.json", head_regressed);
    let run_not_comparable = repo.check(&["--suite", "bench"]);
    assert_eq!(
        run_not_comparable.code, 0,
        "wall-clock benchmark without CI must degrade to not comparable, never a point-estimate failure"
    );
    let json = run_not_comparable.json();
    assert_eq!(json["errors"], 0);
    let outcome = json["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert!(outcome["notes"].as_array().unwrap().iter().any(|n| n
        .as_str()
        .unwrap()
        .contains("not comparable (no CI available)")));

    // Removing test_serialize without an override fails (exit 1)
    let head_removed = r#"{"benchmarks": [
        {"name": "test_deserialize", "stats": {"mean": 0.0020}}
    ]}"#;
    repo.write("reports/pytest_bench.json", head_removed);
    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(run_fail.code, 1);
    let json_fail = run_fail.json();
    assert_eq!(json_fail["errors"], 1);
    let outcome_fail = json_fail["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert_eq!(outcome_fail["violations"][0]["title"], "Benchmark Removed");

    let run_pass = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: test_serialize justified for removal",
    );
    assert_eq!(run_pass.code, 0);
    let json_pass = run_pass.json();
    assert_eq!(json_pass["errors"], 0);
    assert_eq!(json_pass["overrides"], 1);
}

#[test]
fn bench_audit_case1_overlapping_ci_point_regression_passes() {
    // Audit Case 1: Point estimate +2%, but 95% CIs [900, 1100] vs [920, 1120] at 0.5% tolerance.
    // Under naive point-estimate comparison: +2% > 0.5% -> FAILED (statistically invalid).
    // Under conservative interval bounds: L_head (920) <= U_base (1100) -> delta_min <= 0 -> PASSES.
    // Mutant: naive point-estimate comparison fails with exit 1.
    let repo = Repo::new();
    let base_json = r#"{
      "mean": {
        "point_estimate": 1000.0,
        "confidence_interval": {
          "confidence_level": 0.95,
          "lower_bound": 900.0,
          "upper_bound": 1100.0
        }
      }
    }"#;
    repo.commit_base(
        "target/criterion/algo/estimates.json",
        base_json,
        "base: criterion baseline with 95% CI",
    );

    let head_json = r#"{
      "mean": {
        "point_estimate": 1020.0,
        "confidence_interval": {
          "confidence_level": 0.95,
          "lower_bound": 920.0,
          "upper_bound": 1120.0
        }
      }
    }"#;
    repo.write("target/criterion/algo/estimates.json", head_json);

    let run = repo.check(&["--suite", "bench"]);
    assert_eq!(
        run.code, 0,
        "Overlapping confidence intervals must pass without requiring an override directive"
    );
    let json = run.json();
    assert_eq!(json["errors"], 0);
    let outcome = json["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 0);
}

#[test]
fn bench_real_criterion_fixtures_regression_and_override() {
    let repo = Repo::new();
    let base_json = include_str!("fixtures/bench/criterion/base/estimates.json");
    repo.commit_base(
        "target/criterion/fib_20/estimates.json",
        base_json,
        "base: real criterion baseline",
    );

    // Overlapping run with head estimates.json passes (exit 0)
    let head_json = include_str!("fixtures/bench/criterion/head/estimates.json");
    repo.write("target/criterion/fib_20/estimates.json", head_json);
    let run_pass = repo.check(&["--suite", "bench"]);
    assert_eq!(run_pass.code, 0);

    // Statistically verified regression: lower_bound (30,000) > base upper_bound (22,140)
    let regressed_json = r#"{
      "mean": {
        "point_estimate": 31000.0,
        "confidence_interval": {
          "confidence_level": 0.95,
          "lower_bound": 30000.0,
          "upper_bound": 32000.0
        }
      }
    }"#;
    repo.write("target/criterion/fib_20/estimates.json", regressed_json);
    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(run_fail.code, 1);
    let json_fail = run_fail.json();
    assert_eq!(json_fail["errors"], 1);
    assert_eq!(
        json_fail["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["gate"] == "bench-regression")
            .unwrap()["violations"][0]["title"],
        "Benchmark Performance Regressed"
    );

    let run_override = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: fib_20 algorithmic refactor to recursive formulation",
    );
    assert_eq!(run_override.code, 0);
    assert_eq!(run_override.json()["overrides"], 1);
}

#[test]
fn bench_audit_case2_regression_deleted_with_removes_fails() {
    // Audit Case 2: 3x regression + artifact deleted with `removes:` -> FAIL (exit 1).
    // A generic `removes:` on the file does NOT silently lift benchmark deletions;
    // benchmark removal requires its own scoped `allow-regression:` directive.
    // Mutant: `removes:` lifts deletion.
    let repo = Repo::new();
    repo.commit_base(
        "target/iai/bench/callgrind.bench.out",
        "events: Ir\nsummary: 100000\n",
        "base: callgrind artifact",
    );

    repo.remove("target/iai/bench/callgrind.bench.out");

    // Generic `removes:` directive
    let run_fail = repo.check_with_pr(
        &["--suite", "bench"],
        "removes: target/iai/bench/callgrind.bench.out deleted old benchmarks",
    );
    assert_eq!(
        run_fail.code, 1,
        "Generic removes: must NOT lift deleted benchmark artifact"
    );
    let json_fail = run_fail.json();
    assert_eq!(
        json_fail["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["gate"] == "bench-regression")
            .unwrap()["violations"][0]["title"],
        "Benchmark Artifact Deleted"
    );

    // Scoped allow-regression directive
    let run_pass = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: target/iai/bench/callgrind.bench.out intentionally retired obsolete benchmark suite",
    );
    assert_eq!(
        run_pass.code, 0,
        "allow-regression: directive lifts benchmark artifact deletion"
    );
    assert_eq!(run_pass.json()["overrides"], 1);
}

#[test]
fn bench_audit_case3_head_artifact_garbage_fails_exit_2() {
    // Audit Case 3: Head artifact garbage (`{"mean":"n/a"}`) -> FAIL (exit 2).
    // Mutant: silent skip.
    let repo = Repo::new();
    let base_json = r#"{"mean": {"point_estimate": 1000.0}}"#;
    repo.commit_base(
        "target/criterion/algo/estimates.json",
        base_json,
        "base: valid estimates.json",
    );

    repo.write("target/criterion/algo/estimates.json", r#"{"mean":"n/a"}"#);
    let run = repo.check(&["--suite", "bench"]);
    assert_eq!(
        run.code, 2,
        "Garbage/unparseable benchmark artifact must fail-closed with exit code 2"
    );
    assert!(
        run.stderr.contains("unparseable") || run.stderr.contains("malformed"),
        "stderr: {}",
        run.stderr
    );
}

#[test]
fn bench_audit_case4_benchmark_renamed_lacks_baseline_fails() {
    // Audit Case 4: Benchmark renamed (no base entry) -> FAIL (exit 1).
    // Mutant: silent pass.
    let repo = Repo::new();
    let base_json =
        r#"{"benchmarks": [{"name": "BM_OldName", "cpu_time": 100.0, "time_unit": "ns"}]}"#;
    repo.commit_base(
        "build/benchmarks.json",
        base_json,
        "base: original benchmark name",
    );

    let head_json =
        r#"{"benchmarks": [{"name": "BM_RenamedSearch", "cpu_time": 100.0, "time_unit": "ns"}]}"#;
    repo.write("build/benchmarks.json", head_json);

    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(
        run_fail.code, 1,
        "Renamed or new benchmark lacking baseline entry must fail exit 1"
    );
    let json_fail = run_fail.json();
    let outcome = json_fail["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "bench-regression")
        .unwrap();
    let titles: Vec<_> = outcome["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"New or Renamed Benchmark Lacks Baseline"),
        "violations: {:?}",
        titles
    );

    let run_pass = repo.check_with_pr(
        &["--suite", "bench"],
        "allow-regression: build/benchmarks.json renamed benchmark",
    );
    assert_eq!(run_pass.code, 0);
    assert!(run_pass.json()["overrides"].as_u64().unwrap() >= 1);
}

#[test]
fn bench_provenance_tracking_and_cross_host_flag() {
    // Provenance tracking: Mismatched host -> FAIL (exit 1) unless --allow-cross-host-bench
    let repo = Repo::new();
    let base_json = r#"{
      "host": "github-actions-ubuntu-x86_64",
      "benchmarks": [{"name": "BM_Process", "cpu_time": 100.0, "time_unit": "ns"}]
    }"#;
    repo.commit_base(
        "build/benchmarks.json",
        base_json,
        "base: runner provenance recorded",
    );

    let head_json = r#"{
      "host": "developer-laptop-m2-mac",
      "benchmarks": [{"name": "BM_Process", "cpu_time": 100.0, "time_unit": "ns"}]
    }"#;
    repo.write("build/benchmarks.json", head_json);

    let run_fail = repo.check(&["--suite", "bench"]);
    assert_eq!(run_fail.code, 1);
    let json_fail = run_fail.json();
    assert_eq!(
        json_fail["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["gate"] == "bench-regression")
            .unwrap()["violations"][0]["title"],
        "Cross-Host Benchmark Comparison Mismatch"
    );

    // Bypass flag --allow-cross-host-bench allows cross-host diff
    let run_pass = repo.check(&["--suite", "bench", "--allow-cross-host-bench"]);
    assert_eq!(run_pass.code, 0);
}

#[test]
fn java_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_java = r#"
package com.example;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.assertEquals;

class CalcTest {
    @Test
    void testCalc() {
        assertEquals(2, 1 + 1);
        assertEquals(4, 2 + 2);
    }
}
"#;
    repo.write("src/test/java/com/example/CalcTest.java", base_java);
    repo.commit("feat: initial java test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_java = r#"
package com.example;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.assertEquals;

class CalcTest {
    @Test
    void testCalc() {
        assertEquals(2, 1 + 1);
    }
}
"#;
    repo.write("src/test/java/com/example/CalcTest.java", weaker_java);
    repo.commit("test: weaken assertions in java");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write(
        "src/test/java/com/example/EmptyTest.java",
        "package com.example;\nimport org.junit.jupiter.api.Test;\nclass EmptyTest {\n    @Test\n    void empty() {}\n}\n",
    );
    repo2.commit("test: add empty java test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn java_disabled_tests_and_skips_detected() {
    let repo = Repo::new();
    repo.write(
        "src/test/java/com/example/SkipTest.java",
        r#"
package com.example;

import org.junit.jupiter.api.Disabled;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.assertEquals;

class SkipTest {
    @Disabled("temporarily disabled")
    @Test
    void skipsThisTest() {
        assertEquals(2, 1 + 1);
    }
}
"#,
    );
    repo.commit("test: add disabled java test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: SkipTest.skipsThisTest disabled for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn go_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_go = r#"package calc_test

import "testing"

func TestCalc(t *testing.T) {
    if 1+1 != 2 {
        t.Fatalf("unexpected")
    }
    if 2+2 != 4 {
        t.Fatalf("unexpected")
    }
}
"#;
    repo.write("calc_test.go", base_go);
    repo.commit("feat: initial go test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_go = r#"package calc_test

import "testing"

func TestCalc(t *testing.T) {
    if 1+1 != 2 {
        t.Fatalf("unexpected")
    }
}
"#;
    repo.write("calc_test.go", weaker_go);
    repo.commit("test: weaken assertions in go");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write(
        "empty_test.go",
        "package empty_test\n\nimport \"testing\"\n\nfunc TestEmpty(t *testing.T) {}\n",
    );
    repo2.commit("test: add empty go test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn go_skipped_tests_detected_and_accepts_override() {
    let repo = Repo::new();
    repo.write(
        "skip_test.go",
        r#"package skip_test

import "testing"

func TestSkipped(t *testing.T) {
    t.Skip("temporarily skipped")
    if 1+1 != 2 {
        t.Fatalf("unexpected")
    }
}
"#,
    );
    repo.commit("test: add skipped go test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: TestSkipped skipped for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn php_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_php = r#"<?php
class CalcTest extends TestCase {
    public function testCalc() {
        $this->assertEquals(2, 1 + 1);
        $this->assertEquals(4, 2 + 2);
    }
}
"#;
    repo.write("tests/CalcTest.php", base_php);
    repo.commit("feat: initial php test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_php = r#"<?php
class CalcTest extends TestCase {
    public function testCalc() {
        $this->assertEquals(2, 1 + 1);
    }
}
"#;
    repo.write("tests/CalcTest.php", weaker_php);
    repo.commit("test: weaken assertions in php");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write(
        "tests/EmptyTest.php",
        "<?php\nclass EmptyTest extends TestCase {\n    public function testEmpty() {}\n}\n",
    );
    repo2.commit("test: add empty php test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn php_skipped_tests_detected_and_accepts_override() {
    let repo = Repo::new();
    repo.write(
        "tests/SkipTest.php",
        r#"<?php
class SkipTest extends TestCase {
    public function testSkipped() {
        $this->markTestSkipped("temporarily skipped");
        $this->assertEquals(2, 1 + 1);
    }
}
"#,
    );
    repo.commit("test: add skipped php test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: SkipTest::testSkipped skipped for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn c_cpp_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_cpp = r#"
TEST(CalcSuite, Calc) {
    EXPECT_EQ(2, 1 + 1);
    EXPECT_EQ(4, 2 + 2);
}
"#;
    repo.write("tests/CalcTest.cpp", base_cpp);
    repo.commit("feat: initial cpp test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_cpp = r#"
TEST(CalcSuite, Calc) {
    EXPECT_EQ(2, 1 + 1);
}
"#;
    repo.write("tests/CalcTest.cpp", weaker_cpp);
    repo.commit("test: weaken assertions in cpp");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write("tests/EmptyTest.cpp", "TEST(EmptySuite, Empty) {}\n");
    repo2.commit("test: add empty cpp test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn c_cpp_skipped_tests_detected_and_accepts_override() {
    let repo = Repo::new();
    repo.write(
        "tests/SkipTest.cpp",
        r#"
TEST(SkipSuite, DISABLED_Skipped) {
    EXPECT_EQ(2, 1 + 1);
}
"#,
    );
    repo.commit("test: add skipped cpp test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: SkipSuite::DISABLED_Skipped skipped for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn csharp_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_cs = r#"
using Xunit;
public class CalcTests {
    [Fact]
    public void TestCalc() {
        Assert.Equal(2, 1 + 1);
        Assert.Equal(4, 2 + 2);
    }
}
"#;
    repo.write("tests/CalcTests.cs", base_cs);
    repo.commit("feat: initial csharp test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_cs = r#"
using Xunit;
public class CalcTests {
    [Fact]
    public void TestCalc() {
        Assert.Equal(2, 1 + 1);
    }
}
"#;
    repo.write("tests/CalcTests.cs", weaker_cs);
    repo.commit("test: weaken assertions in csharp");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write(
        "tests/EmptyTests.cs",
        "using Xunit;\npublic class EmptyTests {\n    [Fact]\n    public void Empty() {}\n}\n",
    );
    repo2.commit("test: add empty csharp test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn csharp_skipped_tests_detected_and_accepts_override() {
    let repo = Repo::new();
    repo.write(
        "tests/SkipTests.cs",
        r#"
using Xunit;
public class SkipTests {
    [Fact(Skip = "temporary skip")]
    public void SkippedTest() {
        Assert.Equal(2, 1 + 1);
    }
}
"#,
    );
    repo.commit("test: add skipped csharp test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: SkippedTest skipped for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn ruby_assertion_reduction_and_vacuous_tests_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "main"]);
    let base_rb = r#"
class CalcTest < Minitest::Test
  def test_calc
    assert_equal 2, 1 + 1
    assert_equal 4, 2 + 2
  end
end
"#;
    repo.write("test/test_calc.rb", base_rb);
    repo.commit("feat: initial ruby test");
    repo.git(&["checkout", "-B", "work", "main"]);

    // Weaken assertions: 2 assertions reduced to 1
    let weaker_rb = r#"
class CalcTest < Minitest::Test
  def test_calc
    assert_equal 2, 1 + 1
  end
end
"#;
    repo.write("test/test_calc.rb", weaker_rb);
    repo.commit("test: weaken assertions in ruby");

    let run_weak = repo.check(&[]);
    assert_eq!(run_weak.code, 1);
    let outcome_weak = run_weak.outcome("assertion-reduction");
    assert_eq!(outcome_weak["violations"].as_array().unwrap().len(), 1);

    // Vacuous test addition
    let repo2 = Repo::new();
    repo2.write(
        "test/test_empty.rb",
        "class EmptyTest < Minitest::Test\n  def test_empty\n  end\nend\n",
    );
    repo2.commit("test: add empty ruby test");
    let run_vac = repo2.check(&[]);
    assert_eq!(run_vac.code, 1);
    let outcome_vac = run_vac.outcome("vacuous-tests");
    assert_eq!(outcome_vac["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn ruby_skipped_tests_detected_and_accepts_override() {
    let repo = Repo::new();
    repo.write(
        "test/test_skip.rb",
        r#"
class SkipTest < Minitest::Test
  def test_skipped
    skip "work in progress"
    assert_equal 2, 1 + 1
  end
end
"#,
    );
    repo.commit("test: add skipped ruby test");

    let run_skip = repo.check(&[]);
    assert_eq!(run_skip.code, 1);
    let outcome_skip = run_skip.outcome("ignored-tests");
    assert_eq!(outcome_skip["violations"].as_array().unwrap().len(), 1);

    // Lifted with scoped override
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "PR_BODY",
            "allow-ignore: test_skipped skipped for refactoring",
        )],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("ignored-tests");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_pass["overrides"].as_array().unwrap().len(), 1);
}

// ---- command ---------------------------------------------------------------

#[test]
fn command_gate_runs_mock_tool_and_enforces_ratchet() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
command = "echo \"test result: ok. 10 passed\""
count_pattern = '(\d+) passed'
min_count = 5
"#,
    );
    repo.commit("ci: configure command gate on base");
    repo.git(&["checkout", "-q", "-B", "work"]);

    // 1. Clean run on base configuration
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let outcome = run.outcome("command");
    assert_eq!(outcome["examined"], 1);
    assert_eq!(outcome["violations"].as_array().unwrap().len(), 0);

    // 2. Untrusted PR text tampering guard: PR modifies command in discipline.toml
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
command = "echo \"malicious tampering in PR\""
count_pattern = '(\d+) passed'
min_count = 5
"#,
    );
    repo.commit("test: attempt untrusted command edit in PR");
    let run_untrusted = repo.check(&[]);
    assert_eq!(run_untrusted.code, 1);
    assert!(run_untrusted
        .titles("command")
        .contains(&"Untrusted Command Modification".to_string()));

    // Revert the untrusted edit
    repo.git(&["checkout", "-q", "HEAD~1", "--", "discipline.toml"]);
    repo.commit("chore: restore discipline.toml");

    // 3. Count ratchet regression via runner command
    let run_ratchet = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("DISCIPLINE_COMMAND", "echo \"test result: ok. 2 passed\"")],
    );
    assert_eq!(run_ratchet.code, 1);
    assert!(run_ratchet
        .titles("command")
        .contains(&"Count Ratchet Regression".to_string()));

    // 4. Lifted via scoped override directive
    let run_override = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[
            ("DISCIPLINE_COMMAND", "echo \"test result: ok. 2 passed\""),
            (
                "PR_BODY",
                "allow-command: default justified test prune for refactoring",
            ),
        ],
    );
    assert_eq!(
        run_override.code, 0,
        "{}{}",
        run_override.stdout, run_override.stderr
    );
    let outcome_ov = run_override.outcome("command");
    assert_eq!(outcome_ov["violations"].as_array().unwrap().len(), 0);
    assert_eq!(outcome_ov["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn command_gate_forbid_output_and_canary_detected() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
command = "echo \"all tests passed\""
canary_command = "echo \"canary triggered but harmless\""
canary_expected_diagnostic = "CRITICAL_SECURITY_PANIC"
forbid_output = ["FORBIDDEN_FLAG"]
"#,
    );
    repo.commit("ci: configure command gate with canary and forbid_output");
    repo.git(&["checkout", "-q", "-B", "work"]);

    // Canary missing expected diagnostic -> violation
    let run_canary = repo.check(&[]);
    assert_eq!(run_canary.code, 1);
    assert!(run_canary
        .titles("command")
        .contains(&"Canary Diagnostic Missing".to_string()));

    // Forbidden output in primary command -> violation
    let run_forbid = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "DISCIPLINE_COMMAND",
            "echo \"warning: FORBIDDEN_FLAG encountered\"",
        )],
    );
    assert_eq!(run_forbid.code, 1);
    assert!(run_forbid
        .titles("command")
        .contains(&"Forbidden Output Detected".to_string()));
}

#[test]
fn command_gate_missing_binary_and_timeout_fail_closed_exit_2() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);

    // 1. Missing tool in PATH triggers exit code 2
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
command = "non_existent_binary_xyz_12345"
"#,
    );
    repo.commit("ci: configure non-existent command");
    repo.git(&["checkout", "-q", "-B", "work"]);

    let run_missing = repo.check(&[]);
    assert_eq!(
        run_missing.code, 2,
        "missing binary must trigger exit code 2 (could not check)"
    );
    assert!(run_missing.stderr.contains("not found in PATH"));

    // 2. Timeout triggers exit code 2
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
command = "sleep 3"
timeout_seconds = 1
"#,
    );
    repo.commit("ci: configure sleep with 1s timeout");
    repo.git(&["checkout", "-q", "-B", "work"]);

    let run_timeout = repo.check(&[]);
    assert_eq!(
        run_timeout.code, 2,
        "timeout must trigger exit code 2 (could not check)"
    );
    assert!(run_timeout.stderr.contains("timed out after 1s"));
}

#[test]
fn command_gate_staged_mode_fails_closed_on_failure() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
command = "sh -c 'exit 1'"
"#,
    );
    repo.commit("ci: configure failing command gate");

    // Stage a file to enter staged mode
    repo.write("README.md", "# Test\n");
    repo.git(&["add", "README.md"]);

    let run_staged = repo.check(&["--staged"]);
    assert_eq!(
        run_staged.code, 1,
        "command failure in --staged mode must exit 1 (fail-closed), not 0:\nstdout: {}\nstderr: {}",
        run_staged.stdout, run_staged.stderr
    );
    assert!(run_staged
        .titles("command")
        .contains(&"Command Exited With Error".to_string()));
}

#[test]
fn dependency_delta_fires_on_wildcard_and_accepts_override() {
    let repo = Repo::new();
    repo.commit_base(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.0\"\n",
        "base: init cargo",
    );

    // Add wildcard dependency in head
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"*\"\n",
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert!(run
        .titles("dependency-delta")
        .contains(&"Wildcard Dependency Version".to_string()));

    // Override with allow-dependency
    let run_pass = repo.check_with_pr(
        &[],
        "allow-dependency: serde temporary unpinned version for testing",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 1);
}

#[test]
fn dependency_delta_enforces_deny_toml_bans_and_git_pins() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("deny.toml", "[bans]\ndeny = [ { name = \"banned-crate\" } ]\n"),
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.0\"\n",
            ),
        ],
        "base: init deny and cargo",
    );

    // Add banned crate and unpinned git dependency
    let head_cargo = r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
serde = "1.0.0"
banned-crate = "0.2.0"
git-dep = { git = "https://github.com/example/repo.git", branch = "main" }
"#;
    repo.write("Cargo.toml", head_cargo);
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let titles = run.titles("dependency-delta");
    assert!(titles.contains(&"Banned Dependency".to_string()));
    assert!(titles.contains(&"Unpinned Git Dependency".to_string()));

    // Override both dependencies
    let run_pass = repo.check_with_pr(
        &[],
        "allow-dependency: banned-crate audited exception\nallow-dependency: git-dep tracking upstream dev",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 2);
}

#[test]
fn dependency_delta_language_neutral_package_json_and_pyproject() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (
                "package.json",
                "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"lodash\": \"4.17.21\"\n  }\n}\n",
            ),
            (
                "pyproject.toml",
                "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"urllib3==1.26.0\"]\n",
            ),
        ],
        "base: init npm and python manifests",
    );

    // Add wildcard to package.json and wildcard to pyproject.toml
    let head_pkg = "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"lodash\": \"4.17.21\",\n    \"axios\": \"latest\"\n  }\n}\n";
    let head_py = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"urllib3==1.26.0\",\n    \"requests == *\",\n]\n";
    repo.write("package.json", head_pkg);
    repo.write("pyproject.toml", head_py);

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let titles = run.titles("dependency-delta");
    assert_eq!(
        titles
            .iter()
            .filter(|t| *t == "Wildcard Dependency Version")
            .count(),
        2
    );

    // Override both
    let run_pass = repo.check_with_pr(
        &[],
        "allow-dependency: axios latest required for build test\nallow-dependency: requests unpinned version",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 2);
}

#[test]
fn dependency_delta_fires_on_new_direct_dependency_in_cargo_toml() {
    let repo = Repo::new();
    repo.commit_base(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.0\"\n",
        "base: init cargo",
    );

    // Add base64 as a new direct dependency
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.0\"\nbase64 = \"0.22\"\n",
    );

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert!(run
        .titles("dependency-delta")
        .contains(&"New Direct Dependency Added".to_string()));

    // Override with allow-dependency
    let run_pass = repo.check_with_pr(
        &[],
        "allow-dependency: base64 added for base64 encoding support",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 1);
}

#[test]
fn dependency_delta_fires_on_new_direct_dependency_in_package_json() {
    let repo = Repo::new();
    repo.commit_base(
        "package.json",
        "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"lodash\": \"4.17.21\"\n  }\n}\n",
        "base: init npm",
    );

    // Add totally-real-pkg as a new direct dependency
    repo.write(
        "package.json",
        "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"lodash\": \"4.17.21\",\n    \"totally-real-pkg\": \"^1.0.0\"\n  }\n}\n",
    );

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert!(run
        .titles("dependency-delta")
        .contains(&"New Direct Dependency Added".to_string()));
}

#[test]
fn dependency_delta_fires_on_loosened_constraint_and_source_shift() {
    let repo = Repo::new();
    repo.commit_base(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"=1.0.0\"\ntokio = \"1.0.0\"\n",
        "base: init pinned cargo",
    );

    // Loosen serde constraint to ^1.0.0, shift tokio to git
    repo.write(
        "Cargo.toml",
        r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
serde = "^1.0.0"
tokio = { git = "https://github.com/tokio-rs/tokio.git", tag = "tokio-1.0.0" }
"#,
    );

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let titles = run.titles("dependency-delta");
    assert!(titles.contains(&"Loosened Dependency Constraint".to_string()));
    assert!(titles.contains(&"Dependency Source Modified".to_string()));
}

#[test]
fn dependency_delta_reports_lockfile_growth_in_notes() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.0\"\n",
            ),
            (
                "Cargo.lock",
                "version = 3\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n",
            ),
        ],
        "base: init with lockfile",
    );

    // Grow Cargo.lock by adding another package
    repo.write(
        "Cargo.lock",
        "version = 3\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
    );

    let run = repo.check(&[]);
    let outcome = run.outcome("dependency-delta");
    let notes = outcome["notes"].as_array().unwrap();
    let note_str = notes
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        note_str.contains("lockfile `Cargo.lock` package count: base 1, head 2 (+1)"),
        "notes must report lockfile package count delta:\n{note_str}"
    );
}

#[test]
fn test_budget_fires_on_rust_proptest_reduction_and_accepts_override() {
    let repo = Repo::new();
    let base_code = r#"
#[test]
fn property_test() {
    let config = ProptestConfig {
        cases: 10000,
        max_shrink_iters: 5000,
        ..Default::default()
    };
}
"#;
    repo.commit_base(
        "tests/prop.rs",
        base_code,
        "base: add proptest with 10000 cases",
    );

    // Head reduces cases and shrink iters
    let head_code = r#"
#[test]
fn property_test() {
    let config = ProptestConfig {
        cases: 1000,
        max_shrink_iters: 500,
        ..Default::default()
    };
}
"#;
    repo.write("tests/prop.rs", head_code);

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let titles = run.titles("test-budget");
    assert_eq!(
        titles
            .iter()
            .filter(|t| *t == "Test Budget Reduced")
            .count(),
        2
    );

    // Override with allow-test-shrink
    let run_pass = repo.check_with_pr(
        &[],
        "allow-test-shrink: proptest cases reduced for fast iteration\nallow-test-shrink: proptest max_shrink_iters reduced for fast iteration",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 2);
}

#[test]
fn test_budget_fires_on_workflow_fuzz_flag_drop_and_accepts_override() {
    let repo = Repo::new();
    let base_wf = r#"name: Fuzz
jobs:
  fuzz:
    env:
      PROPTEST_CASES: 50000
    steps:
      - run: cargo fuzz run target -max_total_time 7200
"#;
    repo.commit_base(
        ".github/workflows/fuzz.yml",
        base_wf,
        "base: add fuzz workflow",
    );

    // Head drops cases and duration
    let head_wf = r#"name: Fuzz
jobs:
  fuzz:
    env:
      PROPTEST_CASES: 5000
    steps:
      - run: cargo fuzz run target -max_total_time 600
"#;
    repo.write(".github/workflows/fuzz.yml", head_wf);

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let titles = run.titles("test-budget");
    assert_eq!(
        titles
            .iter()
            .filter(|t| *t == "Test Budget Reduced")
            .count(),
        2
    );

    // Override both flags
    let run_pass = repo.check_with_pr(
        &[],
        "allow-test-shrink: PROPTEST_CASES trimmed in branch\nallow-test-shrink: fuzz -max_total_time trimmed in branch",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 2);
}

#[test]
fn test_budget_fires_on_hypothesis_and_fuzz_target_removal() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (
                "fuzz/Cargo.toml",
                "[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n\n[[bin]]\nname = \"parse_fuzz\"\n\n[[bin]]\nname = \"eval_fuzz\"\n",
            ),
            (
                "tests/test_h.py",
                "@settings(max_examples=2000)\ndef test_p(): pass\n",
            ),
        ],
        "base: init fuzz manifest and python hypothesis test",
    );

    // Head removes eval_fuzz and drops max_examples
    repo.write(
        "fuzz/Cargo.toml",
        "[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n\n[[bin]]\nname = \"parse_fuzz\"\n",
    );
    repo.write(
        "tests/test_h.py",
        "@settings(max_examples=200)\ndef test_p(): pass\n",
    );

    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    let titles = run.titles("test-budget");
    assert!(titles.contains(&"Fuzz Target Removed".to_string()));
    assert!(titles.contains(&"Test Budget Reduced".to_string()));

    // Override both
    let run_pass = repo.check_with_pr(
        &[],
        "allow-test-shrink: eval_fuzz target retired\nallow-test-shrink: hypothesis max_examples reduced",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 2);
}

#[test]
fn command_preset_cargo_mutants_zero_selected_and_failure_controls() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
preset = "cargo-mutants"
"#,
    );
    repo.commit("ci: configure cargo-mutants preset");
    repo.git(&["checkout", "-q", "-B", "work"]);

    // Negative control 1: Zero mutants tested
    let run_zero = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("DISCIPLINE_COMMAND", "echo \"0 mutants tested\"")],
    );
    assert_eq!(run_zero.code, 1);
    assert!(run_zero
        .titles("command")
        .contains(&"Zero Items Selected Or Executed".to_string()));

    // Negative control 2: Mutant survived
    let run_survived = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "DISCIPLINE_COMMAND",
            "echo \"2 mutants tested: 1 survived\"",
        )],
    );
    assert_eq!(run_survived.code, 1);
    assert!(run_survived
        .titles("command")
        .contains(&"Forbidden Output Detected".to_string()));

    // Positive control: Clean mutation run
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("DISCIPLINE_COMMAND", "echo \"5 mutants tested: 5 caught\"")],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    let outcome_pass = run_pass.outcome("command");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);

    // Override directive covers the preset by name
    let run_override = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[
            ("DISCIPLINE_COMMAND", "echo \"0 mutants tested\""),
            (
                "PR_BODY",
                "allow-command: cargo-mutants no mutants generated on doc diff",
            ),
        ],
    );
    assert_eq!(run_override.code, 0);
    let outcome_ov = run_override.outcome("command");
    assert_eq!(outcome_ov["overrides"].as_array().unwrap().len(), 1);
}

#[test]
fn command_preset_loom_zero_tests_guard_and_pass() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        r#"[meta]
version = 1
name = "test-repo"

[gates.command]
preset = "loom"
"#,
    );
    repo.commit("ci: configure loom preset");
    repo.git(&["checkout", "-q", "-B", "work"]);

    // Negative control: 0 tests run
    let run_zero = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("DISCIPLINE_COMMAND", "echo \"running 0 tests\"")],
    );
    assert_eq!(run_zero.code, 1);
    assert!(run_zero
        .titles("command")
        .contains(&"Zero Items Selected Or Executed".to_string()));

    // Positive control: Permutations executed and passed
    let run_pass = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[(
            "DISCIPLINE_COMMAND",
            "echo \"running 4 tests\ntest loom_concurrent_ring ... ok\ntest result: ok. 4 passed\"",
        )],
    );
    assert_eq!(run_pass.code, 0);
    let outcome_pass = run_pass.outcome("command");
    assert_eq!(outcome_pass["violations"].as_array().unwrap().len(), 0);
}

#[test]
fn command_preset_cargo_deny_enforces_policy_file_retention_and_accepts_override() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("deny.toml", "[bans]\ndeny = []\n"),
            (
                "discipline.toml",
                r#"[meta]
version = 1
name = "test-repo"

[gates.deletion-rationale]
enabled = false

[gates.command]
preset = "cargo-deny"
"#,
            ),
        ],
        "base: init deny and command preset",
    );

    // Delete policy file deny.toml on head branch
    repo.git(&["rm", "-q", "deny.toml"]);
    let run_del = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("DISCIPLINE_COMMAND", "echo \"deny check ok\"")],
    );
    assert_eq!(run_del.code, 1);
    assert!(run_del
        .titles("command")
        .contains(&"Policy File Deleted".to_string()));

    // Override with allow-command
    let run_override = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[
            ("DISCIPLINE_COMMAND", "echo \"deny check ok\""),
            (
                "PR_BODY",
                "allow-command: cargo-deny removing legacy deny policy in favor of new checker",
            ),
        ],
    );
    assert_eq!(run_override.code, 0);
    assert_eq!(
        run_override.outcome("command")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn compile_time_assertions_deletion_triggers_assertion_reduction_and_accepts_override() {
    let repo = Repo::new();
    let base_src = r#"
pub struct Invariant {
    pub a: u64,
    pub b: u64,
}

const _: () = assert!(std::mem::size_of::<Invariant>() == 16);
const _: () = assert!(std::mem::align_of::<Invariant>() == 8);
"#;
    repo.commit_base("src/lib.rs", base_src, "base: layout assertions");

    // Deleting align_of assert reduces compile-time assertions from 2 to 1
    let head_src = r#"
pub struct Invariant {
    pub a: u64,
    pub b: u64,
}

const _: () = assert!(std::mem::size_of::<Invariant>() == 16);
"#;
    repo.write("src/lib.rs", head_src);

    let run_fail = repo.check(&["--suite", "agent-guard"]);
    assert_eq!(run_fail.code, 1);
    let outcome_fail = run_fail.outcome("assertion-reduction");
    assert_eq!(outcome_fail["violations"].as_array().unwrap().len(), 1);
    let v = &outcome_fail["violations"][0];
    assert_eq!(v["title"], "Assertion Reduction In Existing Test");
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains("compile-time-assertions"));
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains("dropped from 2 to 1"));

    // Passing with allow-assertion-drop override
    let run_pass = repo.check_with_pr(
        &["--suite", "agent-guard"],
        "allow-assertion-drop: compile-time-assertions align_of check no longer needed",
    );
    assert_eq!(run_pass.code, 0);
    assert_eq!(run_pass.json()["overrides"], 1);
}

#[test]
fn agent_prompt_format_produces_repair_and_never_emits_directives() {
    let repo = Repo::new();
    let base_src = r#"
#[test]
fn test_calc() {
    assert_eq!(1 + 1, 2);
    assert_eq!(2 + 2, 4);
}
"#;
    repo.commit_base("tests/calc.rs", base_src, "base: calc tests");

    // Remove one assertion on head
    let head_src = r#"
#[test]
fn test_calc() {
    assert_eq!(1 + 1, 2);
}
"#;
    repo.write("tests/calc.rs", head_src);

    let run = repo.run(
        &["check", "--base", "main", "--format", "agent-prompt"],
        &[],
    );
    assert_eq!(run.code, 1);
    let out = run.stdout;

    assert!(out.contains("Discipline gatekeeper detected violations in your changes"));
    assert!(out.contains("Location: tests/calc.rs:3"));
    assert!(out
        .contains("Problem: Test `test_calc`: equality / pattern assertions dropped from 2 to 1"));
    assert!(out.contains("Repair: Restore the assertions that were removed or weakened"));

    // Verify ZERO directive syntax leaked
    let forbidden = [
        "allow-assertion-drop",
        "allow-command",
        "allow-dependency",
        "discipline:allow",
        "allow(",
        "removes:",
        "deletes:",
    ];
    for tok in forbidden {
        assert!(!out.contains(tok), "agent-prompt leaked directive: {tok}");
    }
}

#[test]
fn install_hooks_creates_executable_pre_commit_hook() {
    let repo = Repo::new();
    let hook_path = repo.path().join(".git").join("hooks").join("pre-commit");
    assert!(!hook_path.exists());

    // Run discipline install-hooks via binary
    let run = repo.run(&["install-hooks"], &[]);
    assert_eq!(run.code, 0);
    assert!(hook_path.exists());

    let content = std::fs::read_to_string(&hook_path).unwrap();
    assert!(content.contains("exec discipline check --staged"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&hook_path).unwrap().permissions();
        assert_ne!(perms.mode() & 0o111, 0, "hook must be executable");
    }

    // Running again is idempotent
    let run2 = repo.run(&["install-hooks"], &[]);
    assert_eq!(run2.code, 0);
    assert!(run2.stdout.contains("already configured") || run2.stdout.contains("installed"));
}

#[test]
fn java_csharp_ruby_fixtures_parsed_and_discriminate() {
    let repo = Repo::new();

    // 1. Clean suites in Java, C#, Ruby pass cleanly
    let java_clean = std::fs::read_to_string("tests/fixtures/java/clean_suite.java").unwrap();
    let csharp_clean = std::fs::read_to_string("tests/fixtures/csharp/clean_suite.cs").unwrap();
    let ruby_clean = std::fs::read_to_string("tests/fixtures/ruby/clean_suite.rb").unwrap();

    repo.write("tests/CleanTest.java", &java_clean);
    repo.write("tests/CleanTest.cs", &csharp_clean);
    repo.write("tests/clean_test.rb", &ruby_clean);
    repo.commit("feat: add clean test suites");

    let run_clean = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run_clean.code, 0,
        "clean suites should pass: stdout: {}\nstderr: {}",
        run_clean.stdout, run_clean.stderr
    );
    assert_eq!(run_clean.titles("vacuous-tests").len(), 0);
    assert_eq!(run_clean.titles("ignored-tests").len(), 0);

    // 2. Skips and vacuous suites trigger violations
    let java_bad = std::fs::read_to_string("tests/fixtures/java/skips_and_vacuous.java").unwrap();
    let csharp_bad = std::fs::read_to_string("tests/fixtures/csharp/skips_and_vacuous.cs").unwrap();
    let ruby_bad = std::fs::read_to_string("tests/fixtures/ruby/skips_and_vacuous.rb").unwrap();

    repo.write("tests/BadTest.java", &java_bad);
    repo.write("tests/BadTest.cs", &csharp_bad);
    repo.write("tests/bad_test.rb", &ruby_bad);
    repo.commit("test: add skips and vacuous suites");

    let run_bad = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(run_bad.code, 1, "skips and vacuous suites must fail");

    let vacuous_violations = run_bad.outcome("vacuous-tests")["violations"]
        .as_array()
        .unwrap()
        .clone();
    let ignored_violations = run_bad.outcome("ignored-tests")["violations"]
        .as_array()
        .unwrap()
        .clone();

    assert!(
        !vacuous_violations.is_empty(),
        "vacuous tests must be detected"
    );
    assert!(
        !ignored_violations.is_empty(),
        "ignored tests must be detected"
    );

    // 3. Syntax error fixtures mark parse errors
    let java_syntax = std::fs::read_to_string("tests/fixtures/java/syntax_error.java").unwrap();
    let csharp_syntax = std::fs::read_to_string("tests/fixtures/csharp/syntax_error.cs").unwrap();
    let ruby_syntax = std::fs::read_to_string("tests/fixtures/ruby/syntax_error.rb").unwrap();

    let registry = discipline::ast::default_registry();
    let vocab = discipline::ast::AssertVocabulary::default();

    let java_facts = registry
        .find_pack("Test.java")
        .unwrap()
        .extract("Test.java", &java_syntax, &vocab)
        .unwrap();
    assert!(
        java_facts.has_parse_errors,
        "java syntax error must set has_parse_errors"
    );

    let csharp_facts = registry
        .find_pack("Test.cs")
        .unwrap()
        .extract("Test.cs", &csharp_syntax, &vocab)
        .unwrap();
    assert!(
        csharp_facts.has_parse_errors,
        "csharp syntax error must set has_parse_errors"
    );

    let ruby_facts = registry
        .find_pack("test.rb")
        .unwrap()
        .extract("test.rb", &ruby_syntax, &vocab)
        .unwrap();
    assert!(
        ruby_facts.has_parse_errors,
        "ruby syntax error must set has_parse_errors"
    );
}

#[test]
fn go_php_c_cpp_fixtures_parsed_and_discriminate() {
    let repo = Repo::new();

    // 1. Clean suites in Go, PHP, C++ pass cleanly
    let go_clean = std::fs::read_to_string("tests/fixtures/go/clean_suite.go").unwrap();
    let php_clean = std::fs::read_to_string("tests/fixtures/php/clean_suite.php").unwrap();
    let cpp_clean = std::fs::read_to_string("tests/fixtures/c_cpp/clean_suite.cpp").unwrap();

    repo.write("tests/clean_test.go", &go_clean);
    repo.write("tests/CleanTest.php", &php_clean);
    repo.write("tests/clean_test.cpp", &cpp_clean);
    repo.commit("feat: add clean test suites in go, php, cpp");

    let run_clean = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run_clean.code, 0,
        "clean suites should pass: stdout: {}\nstderr: {}",
        run_clean.stdout, run_clean.stderr
    );
    assert_eq!(run_clean.titles("vacuous-tests").len(), 0);
    assert_eq!(run_clean.titles("ignored-tests").len(), 0);

    // 2. Skips and vacuous suites trigger violations
    let go_bad = std::fs::read_to_string("tests/fixtures/go/skips_and_vacuous.go").unwrap();
    let php_bad = std::fs::read_to_string("tests/fixtures/php/skips_and_vacuous.php").unwrap();
    let cpp_bad = std::fs::read_to_string("tests/fixtures/c_cpp/skips_and_vacuous.cpp").unwrap();

    repo.write("tests/bad_test.go", &go_bad);
    repo.write("tests/BadTest.php", &php_bad);
    repo.write("tests/bad_test.cpp", &cpp_bad);
    repo.commit("test: add skips and vacuous suites in go, php, cpp");

    let run_bad = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(run_bad.code, 1, "skips and vacuous suites must fail");

    let vacuous_violations = run_bad.outcome("vacuous-tests")["violations"]
        .as_array()
        .unwrap()
        .clone();
    let ignored_violations = run_bad.outcome("ignored-tests")["violations"]
        .as_array()
        .unwrap()
        .clone();

    assert!(
        !vacuous_violations.is_empty(),
        "vacuous tests must be detected"
    );
    assert!(
        !ignored_violations.is_empty(),
        "ignored tests must be detected"
    );

    // 3. Syntax error fixtures mark parse errors
    let go_syntax = std::fs::read_to_string("tests/fixtures/go/syntax_error.go").unwrap();
    let php_syntax = std::fs::read_to_string("tests/fixtures/php/syntax_error.php").unwrap();
    let cpp_syntax = std::fs::read_to_string("tests/fixtures/c_cpp/syntax_error.cpp").unwrap();

    let registry = discipline::ast::default_registry();
    let vocab = discipline::ast::AssertVocabulary::default();

    let go_facts = registry
        .find_pack("test.go")
        .unwrap()
        .extract("test.go", &go_syntax, &vocab)
        .unwrap();
    assert!(
        go_facts.has_parse_errors,
        "go syntax error must set has_parse_errors"
    );

    let php_facts = registry
        .find_pack("Test.php")
        .unwrap()
        .extract("Test.php", &php_syntax, &vocab)
        .unwrap();
    assert!(
        php_facts.has_parse_errors,
        "php syntax error must set has_parse_errors"
    );

    let cpp_facts = registry
        .find_pack("test.cpp")
        .unwrap()
        .extract("test.cpp", &cpp_syntax, &vocab)
        .unwrap();
    assert!(
        cpp_facts.has_parse_errors,
        "cpp syntax error must set has_parse_errors"
    );
}

#[test]
fn go_benchmarks_do_not_trigger_vacuous_tests() {
    let repo = Repo::new();

    let go_bench = r#"package main

import "testing"

func TestReal(t *testing.T) {
	t.Fatal("fail")
}

func BenchmarkFastLoop(b *testing.B) {
	for i := 0; i < b.N; i++ {
		// timing loop without assertions
	}
}
"#;
    repo.write("bench_test.go", go_bench);
    repo.commit("feat: add benchmark and test");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run.code, 0,
        "benchmark should not trigger vacuous test: stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(run.titles("vacuous-tests").len(), 0);
}

#[test]
fn unannotated_skip_rationale_enforced() {
    let repo = Repo::new();

    let code = r#"
#[test]
#[ignore]
fn test_something() {
    let x = 1 + 2;
    assert_eq!(x, 3);
}
"#;
    repo.write("tests/my_test.rs", code);
    repo.commit("test: add ignored test");

    // 1. With vacuous placeholder "todo", it is rejected
    let run_todo = repo.check_with_pr(&["--base", "HEAD~1"], "allow-ignore: test_something todo");
    assert_eq!(run_todo.code, 1, "todo rationale must be rejected");
    let outcome = run_todo.outcome("ignored-tests");
    let violations = outcome["violations"].as_array().unwrap();
    assert!(!violations.is_empty(), "expected ignored-tests violation");
    assert!(violations[0]["message"]
        .as_str()
        .unwrap()
        .contains("lacks a substantive rationale or issue tracker reference"));

    // 2. With substantive rationale, it passes
    let run_good = repo.check_with_pr(
        &["--base", "HEAD~1"],
        "allow-ignore: test_something #42 fix flaky timer",
    );
    assert_eq!(
        run_good.code, 0,
        "substantive rationale must be accepted, stdout: {}\nstderr: {}",
        run_good.stdout, run_good.stderr
    );
}

#[test]
fn bench_noise_warning_and_margin() {
    let repo = Repo::new();

    let base_json = r#"{
  "benchmarks": [
    {
      "name": "BM_Scan",
      "cpu_time": 100.0,
      "time_unit": "ns",
      "stddev": 30.0
    }
  ]
}"#;
    let head_json = r#"{
  "benchmarks": [
    {
      "name": "BM_Scan",
      "cpu_time": 101.0,
      "time_unit": "ns",
      "stddev": 35.0
    }
  ]
}"#;
    repo.write("bench.json", base_json);
    repo.commit("bench: base");

    repo.write("bench.json", head_json);
    repo.commit("bench: head");

    // With max_noise_cv = 0.20, CV = 30/100 = 30% > 20% -> warning emitted in notes
    repo.write(
        "discipline.toml",
        &format!(
            "{CONFIG_HEAD}[gates.bench-regression]\nmax_noise_cv = 0.20\nnoise_margin_pct = 2.0\n"
        ),
    );

    let run = repo.check(&["--base", "HEAD~1"]);
    // Since noise_margin_pct = 2.0%, regression of 1.0% is within tolerance
    assert_eq!(
        run.code, 0,
        "noise margin should permit mild variation, stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let outcome = run.outcome("bench-regression");
    let notes = outcome["notes"].as_array().unwrap();
    let has_noise_warning = notes
        .iter()
        .any(|n| n.as_str().unwrap().contains("exhibits high variance"));
    assert!(
        has_noise_warning,
        "expected high variance warning in notes: {notes:?}"
    );
}

#[test]
fn pii_secrets_detection_and_redaction() {
    let repo = Repo::new();

    let priv_header = format!("{}-BEGIN RSA PRIVATE KEY-----", "----");
    let aws_token = format!("{}1234567890ABCDEF", "AKIA");
    let gh_token = format!("{}123456789012345678901234567890123456", "ghp_");

    let secret_content = format!(
        "\n# Config\nPRIVATE_KEY=\"{priv_header}\"\nAWS_KEY=\"{aws_token}\"\nGH_TOKEN=\"{gh_token}\"\n"
    );
    repo.write("config.env", &secret_content);
    repo.commit("chore: commit credentials");

    let run = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(run.code, 1, "committed secrets must trigger pii violation");

    let outcome = run.outcome("pii");
    let violations = outcome["violations"].as_array().unwrap();
    assert!(!violations.is_empty(), "expected pii secret violations");

    // Check that secret token values are REDACTED and never echoed in output
    for v in violations {
        let msg = v["message"].as_str().unwrap();
        assert!(!msg.contains(&aws_token), "AWS key must not be echoed");
        assert!(!msg.contains(&gh_token), "GitHub token must not be echoed");
    }

    // Waived with inline directive on the same line
    let waived_content = format!(
        "\n# Config\nPRIVATE_KEY=\"{priv_header}\" <!-- discipline:allow(pii) -->\nAWS_KEY=\"{aws_token}\" <!-- discipline:allow(pii) -->\nGH_TOKEN=\"{gh_token}\" <!-- discipline:allow(pii) -->\n"
    );
    repo.write("config.env", &waived_content);
    repo.commit("chore: waive secrets for testing");

    let run_waived = repo.check(&["--base", "HEAD~1"]);
    assert_eq!(
        run_waived.code, 0,
        "waived secrets must pass: stdout: {}\nstderr: {}",
        run_waived.stdout, run_waived.stderr
    );
}
