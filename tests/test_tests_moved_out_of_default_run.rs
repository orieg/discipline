//! `test-floor/tests-moved-out-of-default-run` (#628): a change that adds a rule a runner
//! or the repository reads (a Cargo workspace `exclude` entry, a Go build constraint, a
//! nested `go.mod`, a `go.work` `use` list, a pytest `testpaths` / `norecursedirs` /
//! `collect_ignore` entry, a Jest `testPathIgnorePatterns` or Vitest `exclude` entry, a
//! `.gitattributes` attribute) over test files the base side's default run collected, and
//! that still exist, is reported with the file that holds the rule, the number of tests
//! and the first files. Each rule comes with the controls that pin the other verdict: the
//! rule on both sides, the files new in the change, the tests deleted.
//!
//! Also here: the files `node --test` runs beside a Jest configuration, `go.work`, and
//! `[attr]` macros in `.gitattributes`.
//!
//! `Repo::new()` already holds `tests/a.rs` with two tests and no `Cargo.toml`, so every
//! count includes 2.

mod common;
use common::{Repo, Run, CONFIG_HEAD};
use serde_json::Value;

const MOVED: &str = "test-floor/tests-moved-out-of-default-run";
const BELOW_FLOOR: &str = "Test Count Below Floor";

const RS_3: &str = "\
#[test]
fn one() {
    assert_eq!(1, 1);
}

#[test]
fn two() {
    assert_eq!(2, 2);
}

#[test]
fn three() {
    assert_eq!(3, 3);
}
";
const RS_1: &str = "#[test]\nfn only() {\n    assert_eq!(1, 1);\n}\n";
const PACKAGE: &str = "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const WORKSPACE_MEMBERS: &str = "[workspace]\nmembers = [\"member\", \"out\"]\n";
const WORKSPACE_EXCLUDE: &str = "[workspace]\nmembers = [\"member\"]\nexclude = [\"out\"]\n";

const GO_3: &str = "\
package pkg

import \"testing\"

func TestOne(t *testing.T) {
	if one() != 1 {
		t.Fatal(\"one\")
	}
}

func TestTwo(t *testing.T) {
	if two() != 2 {
		t.Fatal(\"two\")
	}
}

func TestThree(t *testing.T) {
	if three() != 3 {
		t.Fatal(\"three\")
	}
}
";
const GO_MOD: &str = "module example.test/m\n\ngo 1.21\n";
const GO_MOD_SUB: &str = "module example.test/m/sub\n\ngo 1.21\n";
const GO_MOD_OTHER: &str = "module example.test/other\n\ngo 1.21\n";

const PY_3: &str = "\
def test_one():
    assert one() == 1


def test_two():
    assert two() == 2


def test_three():
    assert three() == 3
";
const PYTEST_PLAIN: &str = "[tool.pytest.ini_options]\nminversion = \"7.0\"\n";
const PYTEST_TESTPATHS: &str =
    "[tool.pytest.ini_options]\nminversion = \"7.0\"\ntestpaths = [\"tests\"]\n";
const PYTEST_NORECURSE: &str =
    "[tool.pytest.ini_options]\nminversion = \"7.0\"\nnorecursedirs = [\"slow\"]\n";

const JS_3: &str = "\
it('renders', () => { expect(render()).toBe(1); });
it('clicks', () => { expect(click()).toBe(2); });
it('submits', () => { expect(submit()).toBe(3); });
";
const JEST_DEFAULTS: &str = r#"{"name": "app", "jest": {}}"#;
const JEST_IGNORES_LEGACY: &str =
    r#"{"name": "app", "jest": {"testPathIgnorePatterns": ["/node_modules/", "/legacy/"]}}"#;
const VITEST_PLAIN: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
  },
});
";
const VITEST_EXCLUDES_LEGACY: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    exclude: ['**/node_modules/**', '**/legacy/**'],
  },
});
";

const NODE_TEST_3: &str = "\
import test from 'node:test';
import assert from 'node:assert/strict';

test('one', () => { assert.equal(one(), 1); });
test('two', () => { assert.equal(two(), 2); });
test('three', () => { assert.equal(three(), 3); });
";
const NODE_TEST_REQUIRE_3: &str = "\
const { test } = require('node:test');
const assert = require('node:assert');

test('one', () => { assert.equal(one(), 1); });
test('two', () => { assert.equal(two(), 2); });
test('three', () => { assert.equal(three(), 3); });
";
/// The words `node:test` in a comment and in a string, and no import of the module.
const NOT_NODE_TEST_3: &str = "\
// ported from node:test
const label = 'node:test';

it('one', () => { expect(one()).toBe(1); });
it('two', () => { expect(two()).toBe(2); });
it('three', () => { expect(three()).toBe(3); });
";

/// `base` on the base side, then `change` applied on the head side.
fn changed(base: &[(&str, &str)], change: impl FnOnce(&Repo)) -> Run {
    changed_with_body(base, change, None)
}

fn changed_with_body(base: &[(&str, &str)], change: impl FnOnce(&Repo), body: Option<&str>) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(base, "test: base suite");
    change(&repo);
    repo.commit("chore: change the suite");
    match body {
        Some(body) => repo.check_with_pr(&[], body),
        None => repo.check(&[]),
    }
}

/// The static count of a repository holding `files`, read from a change that touches
/// only a document.
fn counted(files: &[(&str, &str)]) -> (u64, Run) {
    let run = changed(files, |repo| {
        repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 3.\n")
    });
    (examined(&run), run)
}

fn examined(run: &Run) -> u64 {
    run.outcome("test-floor")["examined"]
        .as_u64()
        .unwrap_or_else(|| panic!("no examined count:\n{}", run.stdout))
}

fn notes(run: &Run) -> Vec<String> {
    run.outcome("test-floor")["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

fn has_note(run: &Run, parts: &[&str]) -> bool {
    notes(run)
        .iter()
        .any(|n| parts.iter().all(|p| n.contains(p)))
}

/// The findings of the new code.
fn moved(run: &Run) -> Vec<Value> {
    run.violations("test-floor")
        .into_iter()
        .filter(|v| v["code"] == MOVED)
        .collect()
}

/// Asserts one finding of the new code, in `file`, whose message holds every one of
/// `parts`, and returns it.
fn assert_moved(run: &Run, file: &str, parts: &[&str]) -> Value {
    let found = moved(run);
    assert_eq!(found.len(), 1, "{}", run.stdout);
    let finding = found[0].clone();
    assert_eq!(finding["file"], file, "{finding}");
    let message = finding["message"].as_str().unwrap();
    for part in parts {
        assert!(message.contains(part), "no {part:?} in: {message}");
    }
    finding
}

fn assert_not_moved(run: &Run) {
    assert!(moved(run).is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["overrides"], Value::Array(vec![]));
}

// ------------------------------------------------- Cargo workspace `exclude`

const CARGO_BASE: &[(&str, &str)] = &[
    ("Cargo.toml", WORKSPACE_MEMBERS),
    ("member/Cargo.toml", PACKAGE),
    ("member/tests/it.rs", RS_1),
    ("out/Cargo.toml", PACKAGE),
    ("out/tests/it.rs", RS_3),
];

#[test]
fn a_workspace_exclude_added_over_existing_tests_is_reported() {
    let run = changed(CARGO_BASE, |repo| {
        repo.write("Cargo.toml", WORKSPACE_EXCLUDE)
    });
    let finding = assert_moved(
        &run,
        "Cargo.toml",
        &["`exclude`", "3 test(s)", "1 file(s)", "`out/tests/it.rs`"],
    );
    assert_eq!(finding["severity"], "error", "{finding}");
    assert_eq!(run.titles("test-floor").len(), 1, "{}", run.stdout);
    assert_eq!(run.code, 1, "{}", run.stdout);
    // The count is where it was: the finding is what shows the move.
    assert_eq!(examined(&run), 6, "{}", run.stdout);
}

#[test]
fn a_workspace_exclude_on_both_sides_is_not_reported() {
    let mut files = CARGO_BASE.to_vec();
    files[0] = ("Cargo.toml", WORKSPACE_EXCLUDE);
    let (n, run) = counted(&files);
    assert_eq!(n, 6, "{}", run.stdout);
    assert_not_moved(&run);
    // Nor when the change touches the excluded tests themselves.
    let run = changed(&files, |repo| {
        repo.write("out/tests/it.rs", &format!("{RS_3}\n{RS_1}"))
    });
    assert_not_moved(&run);
}

#[test]
fn a_package_that_is_new_in_the_change_and_excluded_is_not_reported() {
    let run = changed(
        &[
            ("Cargo.toml", "[workspace]\nmembers = [\"member\"]\n"),
            ("member/Cargo.toml", PACKAGE),
            ("member/tests/it.rs", RS_1),
        ],
        |repo| {
            repo.write("Cargo.toml", WORKSPACE_EXCLUDE);
            repo.write("out/Cargo.toml", PACKAGE);
            repo.write("out/tests/it.rs", RS_3);
        },
    );
    assert_not_moved(&run);
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
}

#[test]
fn excluded_tests_that_the_change_deletes_are_the_floors_to_report() {
    let run = changed(CARGO_BASE, |repo| {
        repo.write("Cargo.toml", WORKSPACE_EXCLUDE);
        repo.remove("out/tests/it.rs");
    });
    assert!(moved(&run).is_empty(), "{}", run.stdout);
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    // A file that stays with no test left in it is a deletion too.
    let run = changed(CARGO_BASE, |repo| {
        repo.write("Cargo.toml", WORKSPACE_EXCLUDE);
        repo.write("out/tests/it.rs", "pub fn helper() {}\n");
    });
    assert!(moved(&run).is_empty(), "{}", run.stdout);
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn the_gates_directive_lifts_the_finding() {
    for body in [
        "allow-test-shrink: Cargo.toml the package moves to its own pipeline",
        "allow-gate-weakening: test-floor the package moves to its own pipeline",
    ] {
        let run = changed_with_body(
            CARGO_BASE,
            |repo| repo.write("Cargo.toml", WORKSPACE_EXCLUDE),
            Some(body),
        );
        assert!(moved(&run).is_empty(), "{body}: {}", run.stdout);
        let overrides = run.outcome("test-floor")["overrides"].clone();
        assert_eq!(
            overrides.as_array().unwrap().len(),
            1,
            "{body}: {overrides}"
        );
        assert_eq!(overrides[0]["code"], MOVED, "{body}: {overrides}");
    }
    // Controls: a subject that is not the file holding the rule lifts nothing, be it
    // another file, a moved test file the change touches, or the count's own subject.
    for body in [
        "allow-test-shrink: README.md the package moves to its own pipeline",
        "allow-test-shrink: out/tests/it.rs the package moves to its own pipeline",
        "allow-test-shrink: min_tests the package moves to its own pipeline",
    ] {
        let run = changed_with_body(
            CARGO_BASE,
            |repo| {
                repo.write("Cargo.toml", WORKSPACE_EXCLUDE);
                repo.write("out/tests/it.rs", &format!("{RS_3}\n// moved\n"));
            },
            Some(body),
        );
        assert_eq!(moved(&run).len(), 1, "{body}: {}", run.stdout);
        assert_eq!(
            run.outcome("test-floor")["overrides"],
            Value::Array(vec![]),
            "{body}"
        );
    }
}

/// `min_tests` is the count's subject: it lifts `Test Count Below Floor` and leaves the
/// rule that moved the tests reported. The file holding the rule lifts the rule.
#[test]
fn a_min_tests_subject_lifts_the_count_finding_only() {
    let base = [
        ("pyproject.toml", PYTEST_PLAIN),
        ("tests/test_a.py", PY_3),
        ("legacy/test_b.py", PY_3),
    ];
    let codes = |run: &Run, list: &str| -> Vec<String> {
        run.outcome("test-floor")[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["code"].as_str().unwrap().to_string())
            .collect()
    };
    const COUNT: &str = "test-floor/test-count-below-floor";
    // A plain count drop: `min_tests` lifts it.
    let run = changed_with_body(
        &base,
        |repo| repo.remove("legacy/test_b.py"),
        Some("allow-test-shrink: min_tests the legacy suite is retired"),
    );
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(codes(&run, "overrides"), vec![COUNT], "{}", run.stdout);
    // The count falls because a rule moved the tests out: `min_tests` lifts the count
    // finding, and the rule is then reported as a finding, not left as a note.
    let run = changed_with_body(
        &base,
        |repo| repo.write("pyproject.toml", PYTEST_TESTPATHS),
        Some("allow-test-shrink: min_tests the legacy suite is retired"),
    );
    assert_eq!(codes(&run, "overrides"), vec![COUNT], "{}", run.stdout);
    assert_eq!(codes(&run, "violations"), vec![MOVED], "{}", run.stdout);
    // The file holding the rule is a changed file, a subject of the count finding too:
    // it lifts both.
    let run = changed_with_body(
        &base,
        |repo| repo.write("pyproject.toml", PYTEST_TESTPATHS),
        Some("allow-test-shrink: pyproject.toml the legacy suite is retired"),
    );
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(
        codes(&run, "overrides"),
        vec![COUNT, MOVED],
        "{}",
        run.stdout
    );
    // The count holds (as many tests added) and only the rule is reported: `min_tests`
    // lifts nothing.
    let run = changed_with_body(
        &base,
        |repo| {
            repo.write("pyproject.toml", PYTEST_TESTPATHS);
            repo.write("tests/test_new.py", PY_3);
        },
        Some("allow-test-shrink: min_tests the legacy suite is retired"),
    );
    assert_eq!(codes(&run, "violations"), vec![MOVED], "{}", run.stdout);
    assert!(codes(&run, "overrides").is_empty(), "{}", run.stdout);
}

#[test]
fn a_file_the_configuration_declares_as_a_test_path_is_not_reported() {
    let declared = format!("{CONFIG_HEAD}\n[tests]\npaths = [\"out/tests/**\"]\n");
    let mut files = CARGO_BASE.to_vec();
    files.push(("discipline.toml", &declared));
    // Declared on both sides: it counts whatever the workspace says, and nothing moved.
    let run = changed(&files, |repo| repo.write("Cargo.toml", WORKSPACE_EXCLUDE));
    assert!(moved(&run).is_empty(), "{}", run.stdout);
    // The `exclude` entry on both sides, and the change takes the declaration away: the
    // entry is not the change's own.
    files[0] = ("Cargo.toml", WORKSPACE_EXCLUDE);
    let run = changed(&files, |repo| repo.write("discipline.toml", CONFIG_HEAD));
    assert!(moved(&run).is_empty(), "{}", run.stdout);
}

// ------------------------------------------------- Go: build constraint, `go.mod`, `go.work`

#[test]
fn a_build_tag_added_to_a_go_test_file_is_reported_at_its_line() {
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write(
            "pkg/a_test.go",
            &format!("// Package pkg.\n\n//go:build integration\n\n{GO_3}"),
        )
    });
    let finding = assert_moved(
        &run,
        "pkg/a_test.go",
        &["build constraint", "3 test(s)", "`pkg/a_test.go`"],
    );
    assert_eq!(finding["line"], 3, "{finding}");
    assert_eq!(examined(&run), 5, "{}", run.stdout);
    // Control: the tag on both sides.
    let tagged = format!("//go:build integration\n\n{GO_3}");
    let (_, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &tagged)]);
    assert_not_moved(&run);
    // Control: a constraint the platform satisfies takes nothing out.
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write(
            "pkg/a_test.go",
            &format!("//go:build linux || darwin\n\n{GO_3}"),
        )
    });
    assert_not_moved(&run);
}

#[test]
fn a_constraint_no_build_satisfies_is_reported_once_by_the_count() {
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write("pkg/a_test.go", &format!("//go:build ignore\n\n{GO_3}"))
    });
    // The tests left the count, and the count fell: one finding, with the rule in a note.
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert!(
        has_note(&run, &["build constraint", "`pkg/a_test.go`", "3 test(s)"]),
        "{:?}",
        notes(&run)
    );
    // With as many tests added elsewhere the count holds, and the move is the finding.
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write("pkg/a_test.go", &format!("//go:build ignore\n\n{GO_3}"));
        repo.write("pkg/b_test.go", GO_3);
    });
    let finding = assert_moved(&run, "pkg/a_test.go", &["build constraint", "3 test(s)"]);
    assert_eq!(finding["line"], 1, "{finding}");
    assert_eq!(run.titles("test-floor").len(), 1, "{}", run.stdout);
}

#[test]
fn a_go_mod_added_below_a_module_is_reported() {
    let run = changed(&[("go.mod", GO_MOD), ("sub/pkg/b_test.go", GO_3)], |repo| {
        repo.write("sub/go.mod", GO_MOD_SUB)
    });
    assert_moved(
        &run,
        "sub/go.mod",
        &["3 test(s)", "1 file(s)", "`sub/pkg/b_test.go`"],
    );
    assert_eq!(examined(&run), 5, "{}", run.stdout);
    // A `go.mod` added above an existing module is the file that nests it.
    let run = changed(
        &[("sub/go.mod", GO_MOD_SUB), ("sub/pkg/b_test.go", GO_3)],
        |repo| repo.write("go.mod", GO_MOD),
    );
    assert_moved(&run, "go.mod", &["3 test(s)", "`sub/pkg/b_test.go`"]);
    // Control: the nested module on both sides.
    let (_, run) = counted(&[
        ("go.mod", GO_MOD),
        ("sub/go.mod", GO_MOD_SUB),
        ("sub/pkg/b_test.go", GO_3),
    ]);
    assert_not_moved(&run);
    // Control: a nested module whose tests are new in the change.
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write("sub/go.mod", GO_MOD_SUB);
        repo.write("sub/pkg/b_test.go", GO_3);
    });
    assert_not_moved(&run);
}

const GO_WORK_BOTH: &str = "go 1.21\n\nuse (\n\t./a\n\t./b // the second module\n)\n";
const GO_WORK_ONE: &str = "go 1.21\n\nuse ./a\n";

#[test]
fn a_module_a_go_work_does_not_use_counts_with_a_note() {
    let files = [
        ("go.work", GO_WORK_ONE),
        ("a/go.mod", GO_MOD),
        ("a/pkg/a_test.go", GO_3),
        ("b/go.mod", GO_MOD_OTHER),
        ("b/pkg/b_test.go", GO_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "`go.work`",
                "`b`",
                "1 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: every module in the `use` list, in a block with a comment.
    let mut used = files;
    used[0] = ("go.work", GO_WORK_BOTH);
    let (n, run) = counted(&used);
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(!has_note(&run, &["`go.work`"]), "{:?}", notes(&run));
    // A `go.work` that cannot be read leaves every module below it not determined.
    used[0] = ("go.work", "go 1.21\n\nuse (\n\t./a\n");
    let (n, run) = counted(&used);
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(
        has_note(&run, &["`go.work`", "cannot be read", "2 file(s)"]),
        "{:?}",
        notes(&run)
    );
}

#[test]
fn a_go_work_does_not_bring_a_nested_module_into_the_outer_one() {
    // `go list ./...` at the root of a workspace that uses both lists the outer
    // module's packages only: the nested module stays its own, with the same note.
    let (n, run) = counted(&[
        ("go.work", "go 1.21\n\nuse (\n\t.\n\t./sub\n)\n"),
        ("go.mod", GO_MOD),
        ("pkg/a_test.go", GO_3),
        ("sub/go.mod", GO_MOD_SUB),
        ("sub/pkg/b_test.go", GO_3),
    ]);
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "`sub`",
                "./...",
                "1 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}

#[test]
fn a_module_taken_out_of_a_go_work_use_list_is_reported() {
    let base = [
        ("go.work", GO_WORK_BOTH),
        ("a/go.mod", GO_MOD),
        ("a/pkg/a_test.go", GO_3),
        ("b/go.mod", GO_MOD_OTHER),
        ("b/pkg/b_test.go", GO_3),
    ];
    let run = changed(&base, |repo| repo.write("go.work", GO_WORK_ONE));
    assert_moved(
        &run,
        "go.work",
        &["`use`", "3 test(s)", "`b/pkg/b_test.go`"],
    );
    assert_eq!(examined(&run), 8, "{}", run.stdout);
    // A `go.work` the change adds, which leaves a module out.
    let run = changed(&base[1..], |repo| repo.write("go.work", GO_WORK_ONE));
    assert_moved(&run, "go.work", &["`use`", "3 test(s)"]);
    // Control: a `go.work` the change adds that uses both.
    let run = changed(&base[1..], |repo| repo.write("go.work", GO_WORK_BOTH));
    assert_not_moved(&run);
}

// ------------------------------------------------- pytest

#[test]
fn pytest_testpaths_added_over_existing_tests_is_reported_once() {
    let base = [
        ("pyproject.toml", PYTEST_PLAIN),
        ("tests/test_a.py", PY_3),
        ("legacy/test_b.py", PY_3),
    ];
    // The tests leave the count and the count falls: the count finding, with a note.
    let run = changed(&base, |repo| repo.write("pyproject.toml", PYTEST_TESTPATHS));
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert_eq!(examined(&run), 5, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "`testpaths`",
                "`pyproject.toml`",
                "3 test(s)",
                "`legacy/test_b.py`"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // As many tests added elsewhere: the count holds, and the move is still reported.
    let run = changed(&base, |repo| {
        repo.write("pyproject.toml", PYTEST_TESTPATHS);
        repo.write("tests/test_new.py", PY_3);
    });
    assert_moved(
        &run,
        "pyproject.toml",
        &["`testpaths`", "3 test(s)", "`legacy/test_b.py`"],
    );
    assert_eq!(run.titles("test-floor").len(), 1, "{}", run.stdout);
    assert_eq!(examined(&run), 8, "{}", run.stdout);
    // Control: `testpaths` on both sides, and new tests outside it.
    let mut both = base;
    both[0] = ("pyproject.toml", PYTEST_TESTPATHS);
    let run = changed(&both, |repo| repo.write("legacy/test_c.py", PY_3));
    assert_not_moved(&run);
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
}

#[test]
fn pytest_norecursedirs_and_collect_ignore_added_over_existing_tests_are_reported() {
    let base = [
        ("pyproject.toml", PYTEST_PLAIN),
        ("tests/test_a.py", PY_3),
        ("tests/slow/test_s.py", PY_3),
    ];
    let run = changed(&base, |repo| {
        repo.write("pyproject.toml", PYTEST_NORECURSE);
        repo.write("tests/test_new.py", PY_3);
    });
    assert_moved(
        &run,
        "pyproject.toml",
        &["`norecursedirs`", "3 test(s)", "`tests/slow/test_s.py`"],
    );
    let run = changed(&base, |repo| {
        repo.write("tests/conftest.py", "collect_ignore = [\"slow\"]\n");
        repo.write("tests/test_new.py", PY_3);
    });
    assert_moved(
        &run,
        "tests/conftest.py",
        &["`collect_ignore`", "3 test(s)", "`tests/slow/test_s.py`"],
    );
    // Control: the list on both sides.
    let mut both = base.to_vec();
    both.push(("tests/conftest.py", "collect_ignore = [\"slow\"]\n"));
    let run = changed(&both, |repo| repo.write("tests/test_new.py", PY_3));
    assert_not_moved(&run);
}

// ------------------------------------------------- Jest and Vitest

#[test]
fn jest_ignore_patterns_added_over_existing_tests_are_reported() {
    let base = [
        ("package.json", JEST_DEFAULTS),
        ("src/a.test.js", JS_3),
        ("legacy/b.test.js", JS_3),
    ];
    let run = changed(&base, |repo| {
        repo.write("package.json", JEST_IGNORES_LEGACY);
        repo.write("src/c.test.js", JS_3);
    });
    assert_moved(
        &run,
        "package.json",
        &[
            "`testPathIgnorePatterns`",
            "3 test(s)",
            "`legacy/b.test.js`",
        ],
    );
    // Control: the pattern on both sides.
    let mut both = base;
    both[0] = ("package.json", JEST_IGNORES_LEGACY);
    let run = changed(&both, |repo| repo.write("src/c.test.js", JS_3));
    assert_not_moved(&run);
}

#[test]
fn a_vitest_exclude_added_over_existing_tests_is_reported() {
    let base = [
        ("package.json", r#"{"name": "app"}"#),
        ("vitest.config.ts", VITEST_PLAIN),
        ("src/a.test.ts", JS_3),
        ("legacy/b.test.ts", JS_3),
    ];
    let run = changed(&base, |repo| {
        repo.write("vitest.config.ts", VITEST_EXCLUDES_LEGACY);
        repo.write("src/c.test.ts", JS_3);
    });
    assert_moved(
        &run,
        "vitest.config.ts",
        &["`exclude`", "3 test(s)", "`legacy/b.test.ts`"],
    );
    let mut both = base;
    both[1] = ("vitest.config.ts", VITEST_EXCLUDES_LEGACY);
    let run = changed(&both, |repo| repo.write("src/c.test.ts", JS_3));
    assert_not_moved(&run);
}

// ------------------------------------------------- `.gitattributes`

#[test]
fn an_attribute_added_over_existing_tests_is_reported() {
    let base = [
        ("package.json", JEST_DEFAULTS),
        ("src/a.test.js", JS_3),
        ("third_party/b.test.js", JS_3),
    ];
    let run = changed(&base, |repo| {
        repo.write(".gitattributes", "third_party/** linguist-vendored\n");
        repo.write("src/c.test.js", JS_3);
    });
    assert_moved(
        &run,
        ".gitattributes",
        &[
            "`linguist-vendored`",
            "3 test(s)",
            "`third_party/b.test.js`",
        ],
    );
    // Without the new tests the count falls, and that is the one finding.
    let run = changed(&base, |repo| {
        repo.write(".gitattributes", "third_party/** linguist-vendored\n")
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert!(
        has_note(
            &run,
            &["`linguist-vendored`", "`.gitattributes`", "3 test(s)"]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: the attribute on both sides.
    let mut both = base.to_vec();
    both.push((".gitattributes", "third_party/** linguist-vendored\n"));
    let run = changed(&both, |repo| repo.write("src/c.test.js", JS_3));
    assert_not_moved(&run);
}

#[test]
fn several_files_under_one_rule_are_one_finding_that_names_the_first_three() {
    let base = [
        ("pyproject.toml", PYTEST_PLAIN),
        ("tests/test_a.py", PY_3),
        ("legacy/test_b.py", PY_3),
        ("legacy/test_c.py", PY_3),
        ("legacy/test_d.py", PY_3),
        ("legacy/test_e.py", PY_3),
        ("legacy/test_f.py", PY_3),
    ];
    let run = changed(&base, |repo| {
        repo.write("pyproject.toml", PYTEST_TESTPATHS);
        for name in ["g", "h", "i", "j", "k"] {
            repo.write(&format!("tests/test_{name}.py"), PY_3);
        }
    });
    let finding = assert_moved(
        &run,
        "pyproject.toml",
        &[
            "15 test(s)",
            "5 file(s)",
            "`legacy/test_b.py`",
            "`legacy/test_c.py`",
            "`legacy/test_d.py`",
            "2 more",
        ],
    );
    let message = finding["message"].as_str().unwrap();
    assert!(!message.contains("test_e.py"), "{message}");
}

// ------------------------------------------------- `[attr]` macros

#[test]
fn an_attribute_set_through_a_macro_leaves_the_file_out() {
    let files = |attributes: &'static str| {
        [
            ("package.json", JEST_DEFAULTS),
            (".gitattributes", attributes),
            ("third_party/a.test.js", JS_3),
        ]
    };
    for attributes in [
        "[attr]vend linguist-vendored\nthird_party/** vend\n",
        // Defined after its use, and through another macro.
        "third_party/** outer\n[attr]outer vend\n[attr]vend linguist-vendored\n",
    ] {
        let (n, run) = counted(&files(attributes));
        assert_eq!(n, 2, "{attributes:?}: {}", run.stdout);
        assert!(
            has_note(
                &run,
                &["`linguist-vendored`", "`.gitattributes`", "not counted"]
            ),
            "{attributes:?}: {:?}",
            notes(&run)
        );
    }
    // Controls: a macro that is unset or given a value expands to nothing, and a later
    // line that unsets the attribute wins over the macro.
    for attributes in [
        "[attr]vend linguist-vendored\nthird_party/** -vend\n",
        "[attr]vend linguist-vendored\nthird_party/** vend=true\n",
        "[attr]vend linguist-vendored\nthird_party/** vend\n*.js -linguist-vendored\n",
        "[attr]vend -linguist-vendored\nthird_party/** linguist-vendored vend\n",
    ] {
        let (n, run) = counted(&files(attributes));
        assert_eq!(n, 5, "{attributes:?}: {}", run.stdout);
    }
    // A macro defined below the top level is not one: git refuses the line.
    let (n, run) = counted(&[
        ("package.json", JEST_DEFAULTS),
        (
            "third_party/.gitattributes",
            "[attr]vend linguist-vendored\n*.js vend\n",
        ),
        ("third_party/a.test.js", JS_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

// ------------------------------------------------- `node:test` beside Jest

#[test]
fn a_node_test_file_counts_when_a_package_script_runs_node_test() {
    // The Jest configuration ignores the directory; the file is not Jest's to collect.
    let jest = |scripts: &str| {
        format!(
            r#"{{"name": "app", "scripts": {{{scripts}}}, "jest": {{"testPathIgnorePatterns": ["/node/"]}}}}"#
        )
    };
    let with_script = jest(r#""test": "jest", "test:node": "node --test node/""#);
    for source in [NODE_TEST_3, NODE_TEST_REQUIRE_3] {
        let (n, run) = counted(&[("package.json", &with_script), ("node/a.test.js", source)]);
        assert_eq!(n, 5, "{}", run.stdout);
        assert!(!has_note(&run, &["node --test"]), "{:?}", notes(&run));
    }
    // No script runs `node --test`: not counted, and the note says how many files.
    let without = jest(r#""test": "jest", "lint": "node --check node/a.test.js""#);
    let (n, run) = counted(&[("package.json", &without), ("node/a.test.js", NODE_TEST_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: ",
                "`node:test`",
                "`node --test`",
                "not counted",
                "1 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // The same file where Jest's default patterns would collect it: still `node --test`'s.
    let plain = r#"{"name": "app", "scripts": {"test": "jest"}, "jest": {}}"#;
    let (n, run) = counted(&[("package.json", plain), ("src/a.test.js", NODE_TEST_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    // `[tests] paths` names it: counted.
    let declared = format!("{CONFIG_HEAD}\n[tests]\npaths = [\"node/**\"]\n");
    let (n, run) = counted(&[
        ("package.json", &without),
        ("discipline.toml", &declared),
        ("node/a.test.js", NODE_TEST_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Control: the words in a comment and a string are not an import; Jest decides.
    let (n, run) = counted(&[("package.json", plain), ("src/a.test.js", NOT_NODE_TEST_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&[
        ("package.json", &without),
        ("node/a.test.js", NOT_NODE_TEST_3),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(!has_note(&run, &["node --test"]), "{:?}", notes(&run));
}

// ------------------------------------------------- Jest and `node_modules`

/// Jest 27.5 and 29.7 list no file below `node_modules` with `"testPathIgnorePatterns":
/// []` or a list that does not name it: the file map leaves those out first.
#[test]
fn jest_never_collects_below_node_modules_whatever_the_ignore_list() {
    for ignore in ["[]", r#"["/legacy/"]"#] {
        let package =
            format!(r#"{{"name": "app", "jest": {{"testPathIgnorePatterns": {ignore}}}}}"#);
        let (n, run) = counted(&[
            ("package.json", &package),
            ("src/a.test.js", JS_3),
            ("node_modules/p/z.test.js", JS_3),
        ]);
        assert_eq!(n, 5, "{ignore}: {}", run.stdout);
    }
}

// ------------------------------------------------- Vitest `exclude`

fn vitest_package(range: &str) -> String {
    format!(r#"{{"name": "app", "devDependencies": {{"vitest": "{range}"}}}}"#)
}

/// Vitest 1.6 leaves `dist/`, `cypress/` and `.cache/` out by default and Vitest 4.1
/// does not (both run on the same files).
#[test]
fn vitests_default_exclude_follows_the_major_the_manifest_pins() {
    let files = |package: &'static str| {
        [
            ("package.json", package),
            ("vitest.config.ts", VITEST_PLAIN),
            ("src/a.test.ts", JS_3),
            ("dist/b.test.ts", JS_3),
        ]
    };
    let one: &'static str = Box::leak(vitest_package("^1.6.1").into_boxed_str());
    let four: &'static str = Box::leak(vitest_package("^4.1.0").into_boxed_str());
    let any: &'static str = Box::leak(vitest_package("*").into_boxed_str());
    let (n, run) = counted(&files(one));
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&files(four));
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(
        !has_note(&run, &["depends on its version"]),
        "{:?}",
        notes(&run)
    );
    // No plain range: counted, with a note.
    let (n, run) = counted(&files(any));
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "default `exclude`",
                "1 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}

/// With `exclude: ['legacy']`, Vitest 1.6 and 4.1 run `node_modules/p/z.test.ts` (the
/// list replaces the default one) and `src/legacy/d.test.ts`, and not `legacy/b.test.ts`.
#[test]
fn a_vitest_exclude_replaces_the_default_list_and_names_directories() {
    let config = "export default { test: { exclude: ['legacy'] } };\n";
    let four = vitest_package("^4.1.0");
    let (n, run) = counted(&[
        ("package.json", &four),
        ("vitest.config.mjs", config),
        (
            "src/a.test.ts",
            "it('a', () => { expect(a()).toBe(1); });\n",
        ),
        ("legacy/b.test.ts", JS_3),
        ("node_modules/p/z.test.ts", &format!("{JS_3}{JS_3}{JS_3}")),
        ("src/legacy/d.test.ts", &format!("{JS_3}{JS_3}")),
    ]);
    // 2 + 1 + 9 + 6, and not the 3 of `legacy/b.test.ts`.
    assert_eq!(n, 18, "{}", run.stdout);
}

// ------------------------------------------------- Deno `include` / `exclude`

/// Three tests in the `describe` / `it` style of Deno's standard `bdd` module, which
/// the JavaScript pack reads (`Deno.test(...)` is not read as a test by any gate).
const DENO_3: &str = "\
import { it } from 'jsr:@std/testing/bdd';

it('one', () => { assertEquals(one(), 1); });
it('two', () => { assertEquals(two(), 2); });
it('three', () => { assertEquals(three(), 3); });
";

/// Each list was run with `deno test` (deno 2.6) over the same files.
#[test]
fn deno_include_and_exclude_lists_leave_files_out() {
    let files = |config: &'static str| {
        [
            ("deno.json", config),
            ("a_test.ts", DENO_3),
            ("legacy/b_test.ts", DENO_3),
            ("pkg/legacy/c_test.ts", DENO_3),
            ("checks/d_test.ts", DENO_3),
        ]
    };
    for (config, expected) in [
        ("{}", 14),
        (r#"{"test": {"exclude": ["legacy/"]}}"#, 11),
        (r#"{"test": {"exclude": ["legacy"]}}"#, 11),
        (r#"{"exclude": ["legacy/"]}"#, 11),
        (r#"{"test": {"exclude": ["leg*"]}}"#, 11),
        (r#"{"test": {"exclude": ["**/legacy/**"]}}"#, 8),
        (
            r#"{"exclude": ["legacy/"], "test": {"exclude": ["checks/"]}}"#,
            8,
        ),
        (r#"{"test": {"include": ["checks/"]}}"#, 5),
        (r#"{"test": {"include": ["./checks"]}}"#, 5),
        (r#"{"test": {"include": ["**/c_test.ts"]}}"#, 5),
        // A glob that matches a directory finds nothing in it.
        (r#"{"test": {"include": ["che*"]}}"#, 2),
        // The top-level `include` does not limit `deno test`.
        (r#"{"include": ["checks/"]}"#, 14),
        // Comments and trailing commas are read.
        (
            "{\n  // parked\n  \"test\": {\"exclude\": [\"legacy/\",],},\n}\n",
            11,
        ),
        (
            "{\n  \"test\": {\"exclude\": [\"legacy/\",],}, /* parked */\n}\n",
            11,
        ),
        // Not told from the file: a negated entry, a task with its own configuration.
        (
            r#"{"test": {"exclude": ["legacy/", "!legacy/b_test.ts"]}}"#,
            14,
        ),
        (
            r#"{"tasks": {"test": "deno test --config other.json"}, "test": {"exclude": ["legacy/"]}}"#,
            14,
        ),
        // A task that passes paths replaces `include`; `exclude` still applies.
        (
            r#"{"tasks": {"test": "deno test checks/"}, "test": {"include": ["legacy/"], "exclude": ["checks/"]}}"#,
            11,
        ),
    ] {
        let (n, run) = counted(&files(config));
        assert_eq!(n, expected, "{config}: {}", run.stdout);
    }
    // Beside a root `package.json` the runner may be another: the lists are not applied.
    let (n, run) = counted(&[
        ("package.json", r#"{"name": "app"}"#),
        ("deno.json", r#"{"test": {"exclude": ["legacy/"]}}"#),
        ("a_test.ts", DENO_3),
        ("legacy/b_test.ts", DENO_3),
    ]);
    assert_eq!(n, 8, "{}", run.stdout);
}

#[test]
fn a_deno_exclude_added_over_existing_tests_is_reported() {
    let base = [
        ("deno.json", "{}"),
        ("a_test.ts", DENO_3),
        ("legacy/b_test.ts", DENO_3),
    ];
    let run = changed(&base, |repo| {
        repo.write("deno.json", r#"{"test": {"exclude": ["legacy/"]}}"#);
        repo.write("c_test.ts", DENO_3);
    });
    // Deno's default names collected the file on the base side, and the count does not
    // fall: the rule is reported on the file that holds it.
    assert_moved(
        &run,
        "deno.json",
        &[
            "`exclude` / `test.exclude` in `deno.json`",
            "3 test(s) in 1 file(s)",
            "`legacy/b_test.ts`",
        ],
    );
    let run = changed(&base, |repo| {
        repo.write("deno.json", r#"{"test": {"exclude": ["legacy/"]}}"#)
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}
