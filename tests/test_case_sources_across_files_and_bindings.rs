//! Case sources `assertion-reduction` did not resolve (#626):
//!
//! - a Go case table or row type declared in another file of the test's package;
//! - a Java `@ValueSource` that names a constant, which is one value;
//! - `.each` and `parametrize`, read by what the name they are reached through is bound
//!   to and not by the callee's last name;
//! - a property closure (`proptest!(|(x in ..)| { .. })`) in a helper a test calls.
//!
//! Each shape is driven through the real binary, with a control beside it.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const CASES: &str = "Test Cases Reduced In Parametrized Test";
const DECREASED: &str = "Assertion Count Decreased In Existing Test";
const VACUOUS: &str = "Vacuous Test Added";

/// Commits `base` on `main`, then `head` on a branch, and runs `check`.
fn change(base: &[(&str, &str)], head: &[(&str, &str)]) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    for (path, content) in base {
        repo.write(path, content);
    }
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    for (path, content) in head {
        repo.write(path, content);
    }
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

/// What `gate` reported: `(title, file)` of each violation.
fn reported(run: &Run, gate: &str) -> Vec<(String, String)> {
    run.violations(gate)
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["file"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn one(title: &str, file: &str) -> Vec<(String, String)> {
    vec![(title.to_string(), file.to_string())]
}

fn go_table(package: &str, rows: usize) -> String {
    let rows: String = (0..rows)
        .map(|i| format!("\t{{{i}, {i}, {}}},\n", 2 * i))
        .collect();
    format!("package {package}\n\nvar addCases = []struct {{\n\ta, b, sum int\n}}{{\n{rows}}}\n")
}

const GO_TEST: &str = "func TestAdd(t *testing.T) {\n\tfor _, c := range addCases {\n\t\tif Add(c.a, c.b) != c.sum {\n\t\t\tt.Errorf(\"%d + %d\", c.a, c.b)\n\t\t}\n\t}\n}\n";

/// A package-level case table moved to another file of the package keeps its rows: the
/// test read as one that lost its case list. The file may be new or one the change does
/// not touch on the base side.
#[test]
fn a_go_case_table_in_another_file_of_the_package_is_resolved() {
    let file = "calc/calc_test.go";
    let other = "calc/cases_test.go";
    let inline = format!(
        "{}\nimport \"testing\"\n\n{GO_TEST}",
        go_table("calc", 3).replace("package calc\n\n", "package calc\n\nvar unused = 1\n\n")
    );
    let alone = format!("package calc\n\nimport \"testing\"\n\n{GO_TEST}");

    let run = change(
        &[(file, &inline)],
        &[(file, &alone), (other, &go_table("calc", 3))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "moved: {}",
        run.stdout
    );

    // The table already stood in the other file: a row dropped there is seen from the
    // test only when the test's file is in the change.
    let touched = format!("{alone}\n// Adds.\n");
    let run = change(
        &[(file, &alone), (other, &go_table("calc", 3))],
        &[(file, &touched), (other, &go_table("calc", 2))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );

    // Control: moved, and a row lost on the way.
    let run = change(
        &[(file, &inline)],
        &[(file, &alone), (other, &go_table("calc", 2))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );

    // Control: a file of another package of the directory holds no table of this one.
    let run = change(
        &[(file, &inline)],
        &[(file, &alone), (other, &go_table("calc_test", 3))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );

    // Control: a file of another directory is another package.
    let run = change(
        &[(file, &inline)],
        &[
            (file, &alone),
            ("other/cases_test.go", &go_table("calc", 3)),
        ],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );
}

/// A row type declared in another file of the package is a row type: a table of it that
/// the test ranges over keeps its rows when the type's declaration moves out of the file.
#[test]
fn a_go_row_type_in_another_file_of_the_package_is_resolved() {
    let file = "calc/calc_test.go";
    let other = "calc/types_test.go";
    let row = "type addition struct {\n\ta, b, sum int\n}\n";
    let test = |rows: usize| {
        let rows: String = (0..rows)
            .map(|i| format!("\t\t{{{i}, {i}, {}}},\n", 2 * i))
            .collect();
        format!("func TestAdd(t *testing.T) {{\n\tfor _, c := range []addition{{\n{rows}\t}} {{\n\t\tif Add(c.a, c.b) != c.sum {{\n\t\t\tt.Errorf(\"%d\", c.a)\n\t\t}}\n\t}}\n}}\n")
    };
    let head = "package calc\n\nimport \"testing\"\n\n";
    let with_type = |rows: usize| format!("{head}{row}\n{}", test(rows));
    let without = |rows: usize| format!("{head}{}", test(rows));
    let types = format!("package calc\n\n{row}");

    let run = change(
        &[(file, &with_type(3))],
        &[(file, &without(3)), (other, &types)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );

    // A row dropped from a table whose row type another file declares.
    let run = change(
        &[(file, &without(3)), (other, &types)],
        &[(file, &without(2))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );
}

fn java(annotations: &str) -> String {
    format!("import org.junit.jupiter.params.ParameterizedTest;\nimport org.junit.jupiter.params.provider.*;\nimport static org.junit.jupiter.api.Assertions.*;\n\nclass NameTest {{\n    static final String FIRST = \"a\";\n\n    @ParameterizedTest\n{annotations}    void accepts(String name) {{\n        assertTrue(Names.valid(name));\n    }}\n}}\n")
}

/// `@ValueSource(strings = FIRST)` is one case, so a source dropped beside it is a case
/// dropped. It read as a source that cannot be counted, and nothing was compared.
#[test]
fn a_java_value_source_naming_a_constant_is_one_case() {
    let file = "src/test/java/NameTest.java";
    let run = change(
        &[(
            file,
            &java("    @NullSource\n    @ValueSource(strings = FIRST)\n"),
        )],
        &[(file, &java("    @ValueSource(strings = FIRST)\n"))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("2 to 1"), "{}", run.stdout);

    // Named constants in an array are one case each.
    let run = change(
        &[(
            file,
            &java("    @ValueSource(strings = {FIRST, Names.SECOND})\n"),
        )],
        &[(file, &java("    @ValueSource(strings = Names.SECOND)\n"))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(CASES, file),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("2 to 1"), "{}", run.stdout);

    // Control: the same sources kept.
    let both = java("    @NullSource\n    @ValueSource(strings = FIRST)\n");
    let run = change(&[(file, &both)], &[(file, &format!("{both}// Names.\n"))]);
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );
}

fn js(import: &str, rows: usize) -> String {
    let rows: Vec<String> = (0..rows).map(|i| format!("[{i}, {}]", i * 2)).collect();
    format!("{import}\nit.each([{}])('doubles %i', (n, twice) => {{\n  expect(double(n)).toBe(twice);\n}});\n", rows.join(", "))
}

/// `it.each` is a case source when `it` is the runner's: its global, a name imported
/// from a runner, or one derived from such a name. Under a name the file declares itself,
/// `each` is a project function, and rows dropped from its argument are not test cases.
#[test]
fn each_is_a_case_source_only_on_the_runner() {
    let file = "tests/double.test.js";
    for import in [
        "",
        "import { it, expect } from 'vitest';",
        "import { it, expect } from '@jest/globals';",
        "const { it } = require('node:test');",
        "import { it as base } from 'vitest';\nconst it = base.extend({});",
    ] {
        let run = change(&[(file, &js(import, 3))], &[(file, &js(import, 1))]);
        assert_eq!(
            reported(&run, "assertion-reduction"),
            one(CASES, file),
            "{import}: {}",
            run.stdout
        );
    }
    for import in ["function it() { return rows; }", "const it = makeTable();"] {
        let run = change(&[(file, &js(import, 3))], &[(file, &js(import, 1))]);
        assert_eq!(
            reported(&run, "assertion-reduction"),
            vec![],
            "{import}: {}",
            run.stdout
        );
        assert!(
            !notes(&run).contains("non-literal test case source"),
            "{import}: {}",
            run.stdout
        );
    }
}

fn notes(run: &Run) -> String {
    run.outcome("assertion-reduction")["notes"].to_string()
}

/// A runner function name imported from a project module (`import { test } from
/// './fixtures'`) may be the runner's own, handed on: its `each` is a case source whose
/// cases are not counted. Rows dropped there are not compared, and the note for a
/// non-literal case source says so; a counted list that moves under such a name is
/// reported as no longer a literal list.
#[test]
fn each_on_a_runner_name_imported_from_a_project_module_is_not_counted_and_is_noted() {
    let file = "tests/double.test.js";
    for import in [
        "import { it } from './fixtures';",
        "const { it } = require('../support/table');",
    ] {
        let run = change(&[(file, &js(import, 3))], &[(file, &js(import, 1))]);
        assert_eq!(
            reported(&run, "assertion-reduction"),
            vec![],
            "{import}: {}",
            run.stdout
        );
        assert!(
            notes(&run).contains("non-literal test case source in `doubles %i`"),
            "{import}: {}",
            run.stdout
        );
        let run = change(
            &[(file, &js("import { it } from 'vitest';", 3))],
            &[(file, &js(import, 3))],
        );
        assert_eq!(
            reported(&run, "assertion-reduction"),
            one(CASES, file),
            "{import}: {}",
            run.stdout
        );
        assert!(
            run.stdout.contains("no longer a literal list"),
            "{import}: {}",
            run.stdout
        );
    }
}

fn py(import: &str, decorator: &str, rows: usize) -> String {
    let rows: Vec<String> = (0..rows).map(|i| i.to_string()).collect();
    format!("{import}\n\n@{decorator}(\"n\", [{}])\ndef test_double(n):\n    assert double(n) == n * 2\n", rows.join(", "))
}

/// `parametrize` is a case source when it is pytest's, under whatever name the file
/// binds `pytest` or its `mark` to; a project function of that name is not one.
#[test]
fn parametrize_is_a_case_source_only_through_pytest() {
    let file = "tests/test_double.py";
    for (import, decorator) in [
        ("import pytest", "pytest.mark.parametrize"),
        ("import pytest as pt", "pt.mark.parametrize"),
        ("from pytest import mark", "mark.parametrize"),
        ("from pytest import mark as m", "m.parametrize"),
        (
            "import pytest\n\nparametrize = pytest.mark.parametrize",
            "parametrize",
        ),
        ("import pytest\n\ncases = pytest.mark", "cases.parametrize"),
    ] {
        let run = change(
            &[(file, &py(import, decorator, 3))],
            &[(file, &py(import, decorator, 1))],
        );
        assert_eq!(
            reported(&run, "assertion-reduction"),
            one(CASES, file),
            "{import} / {decorator}: {}",
            run.stdout
        );
    }
    for (import, decorator) in [
        ("from harness import mark", "mark.parametrize"),
        ("import harness", "harness.parametrize"),
        ("import harness as pytest", "pytest.mark.parametrize"),
        ("from harness import parametrize", "parametrize"),
        (
            "def parametrize(names, rows):\n    return lambda f: f",
            "parametrize",
        ),
    ] {
        let run = change(
            &[(file, &py(import, decorator, 3))],
            &[(file, &py(import, decorator, 1))],
        );
        assert_eq!(
            reported(&run, "assertion-reduction"),
            vec![],
            "{import} / {decorator}: {}",
            run.stdout
        );
    }
}

fn property_helper(checks: &str) -> String {
    format!("use proptest::prelude::*;\n\nfn holds_for_all() {{\n    proptest!(|(x in 0..10i32)| {{\n{checks}    }});\n}}\n\n#[test]\nfn runs() {{\n    holds_for_all();\n}}\n")
}

/// A property closure in a helper is read when a test calls the helper: an assertion
/// dropped from it is reported on the test, and a new test that checks only through such
/// a helper is not vacuous.
#[test]
fn a_property_closure_in_a_helper_is_read_through_the_test_that_calls_it() {
    let file = "tests/props.rs";
    let two = "        prop_assert!(x >= 0);\n        prop_assert_eq!(x + 0, x);\n";
    let one_check = "        prop_assert!(x >= 0);\n";
    let run = change(
        &[(file, &property_helper(two))],
        &[(file, &property_helper(one_check))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, file),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("`runs`"), "{}", run.stdout);

    let base = "#[test]\nfn old() {\n    assert_eq!(1 + 1, 2);\n}\n";
    let run = change(
        &[(file, base)],
        &[(file, &format!("{base}\n{}", property_helper(two)))],
    );
    assert_eq!(reported(&run, "vacuous-tests"), vec![], "{}", run.stdout);

    // Control: a closure that checks nothing leaves the new test vacuous.
    let run = change(
        &[(file, base)],
        &[(
            file,
            &format!("{base}\n{}", property_helper("        let _ = x;\n")),
        )],
    );
    assert_eq!(
        reported(&run, "vacuous-tests"),
        one(VACUOUS, file),
        "{}",
        run.stdout
    );
}
