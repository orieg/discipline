//! Collection rules of `test-floor`'s static count (#650), each compared with the tool
//! it models, run offline on small cases: `Deno.test` and Deno's default test names
//! (deno 2.6), a negated Deno `exclude` entry, Vitest and Jest run with no
//! configuration of their own (Vitest 1.6, 3.1, 4.1; Jest 27.5, 29.7), a Jest
//! `testMatch` pattern that cannot match an absolute path, the configurations Jest
//! refuses to run with, the configuration-file names Vitest left out by default before
//! Vitest 4, a Cargo target switched off in its own manifest (cargo 1.98), a
//! `node --test` script of a nested `package.json`, and a `go.work` read as the go tool
//! reads it (go 1.25).
//!
//! `Repo::new()` already holds `tests/a.rs` with two tests and no `Cargo.toml`, so every
//! count includes 2 unless the test replaces that file's package.

mod common;
use common::{Repo, Run};
use serde_json::Value;

const MOVED: &str = "test-floor/tests-moved-out-of-default-run";
const BELOW_FLOOR: &str = "Test Count Below Floor";

/// Three tests in the `describe` / `it` style, which every side of each change reads.
const IT_3: &str = "\
it('renders', () => { expect(render()).toBe(1); });
it('clicks', () => { expect(click()).toBe(2); });
it('submits', () => { expect(submit()).toBe(3); });
";

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
const GO_MOD: &str = "module example.test/app\n\ngo 1.21\n";

const NODE_TEST_3: &str = "\
import test from 'node:test';
import assert from 'node:assert/strict';

test('one', () => { assert.equal(one(), 1); });
test('two', () => { assert.equal(two(), 2); });
test('three', () => { assert.equal(three(), 3); });
";

/// `base` on the base side, then `change` applied on the head side.
fn changed(base: &[(&str, &str)], change: impl FnOnce(&Repo)) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(base, "test: base suite");
    change(&repo);
    repo.commit("chore: change the suite");
    repo.check(&[])
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

fn moved(run: &Run) -> Vec<Value> {
    run.violations("test-floor")
        .into_iter()
        .filter(|v| v["code"] == MOVED)
        .collect()
}

/// Asserts one `tests-moved-out-of-default-run` finding, in `file`, whose message holds
/// every one of `parts`.
fn assert_moved(run: &Run, file: &str, parts: &[&str]) {
    let found = moved(run);
    assert_eq!(found.len(), 1, "{}", run.stdout);
    assert_eq!(found[0]["file"], file, "{}", found[0]);
    let message = found[0]["message"].as_str().unwrap();
    for part in parts {
        assert!(message.contains(part), "no {part:?} in: {message}");
    }
}

// ------------------------------------------------------------- `Deno.test`

/// `deno test` (deno 2.6) on this file: `8 passed (5 steps) | 0 failed | 4 ignored
/// (1 step)`. Each registration is one test, whatever its shape; a step is reported
/// beside the tests.
const DENO_SHAPES: &str = "\
import { assertEquals } from 'jsr:@std/assert';

Deno.test('named', () => { assertEquals(one(), 1); });
Deno.test({ name: 'object', fn() { assertEquals(one(), 1); } });
Deno.test({ name: 'object-arrow', fn: () => { assertEquals(one(), 1); } });
Deno.test(function fnName() { assertEquals(one(), 1); });
Deno.test('with options', { permissions: {} }, () => { assertEquals(one(), 1); });
Deno.test({ name: 'options-then-fn', ignore: true }, () => { assertEquals(one(), 1); });
Deno.test.ignore('ignored', () => { assertEquals(one(), 1); });
Deno.test.ignore({ name: 'ignored-object', fn() { assertEquals(one(), 1); } });
Deno.test({ name: 'ignore-key', ignore: true, fn() { assertEquals(one(), 1); } });
Deno.test({ name: 'ignore-false', ignore: false, fn() { assertEquals(one(), 1); } });
Deno.test({ name: 'ignore-computed', ignore: Deno.build.os === 'windows', fn() { assertEquals(one(), 1); } });
Deno.test('steps', async (t) => {
  await t.step('step one', () => { assertEquals(one(), 1); });
  await t.step({ name: 'step two', fn: () => { assertEquals(one(), 1); } });
  await t.step(function stepThree() { assertEquals(one(), 1); });
  await t.step('nested', async (t2) => {
    await t2.step('inner', () => { assertEquals(one(), 1); });
  });
  await t.step({ name: 'step ignored', ignore: true, fn: () => { assertEquals(one(), 1); } });
});
";

#[test]
fn deno_test_registrations_are_counted() {
    let (n, run) = counted(&[("deno.json", "{}"), ("shapes_test.ts", DENO_SHAPES)]);
    // The 8 tests Deno runs and the 5 steps it runs, each step as a subtest.
    assert_eq!(n, 2 + 8 + 5, "{}", run.stdout);
    assert!(
        has_note(&run, &["head: 5 ignored / skipped test(s)"]),
        "{:?}",
        notes(&run)
    );
}

/// `deno test` on the first file runs `b` and `c`, filters `a` out, and fails the run
/// "because the \"only\" option was used": `.only` is flagged as `it.only` is.
#[test]
fn deno_test_only_is_flagged_and_a_local_deno_is_not_the_runtime() {
    let only = "\
Deno.test('a', () => { check(1); });
Deno.test.only('b', () => { check(2); });
Deno.test({ name: 'c', only: true, fn() { check(3); } });
";
    let (n, run) = counted(&[("deno.json", "{}"), ("only_test.ts", only)]);
    assert_eq!(n, 2 + 1, "{}", run.stdout);
    assert!(
        has_note(&run, &["head: 2 ignored / skipped test(s)"]),
        "{:?}",
        notes(&run)
    );
    // Control: a file that binds `Deno` itself registers nothing with the runtime.
    let local = "\
import { Deno } from './shim.ts';

Deno.test('a', () => { check(1); });
";
    let (n, run) = counted(&[("deno.json", "{}"), ("local_test.ts", local)]);
    assert_eq!(n, 2, "{}", run.stdout);
}

// ------------------------------------------------------ Deno default names

/// `deno test` (deno 2.6) in a directory of these names runs `a_test.ts`, `b.test.ts`,
/// `test.ts`, `_test.ts` and `__tests__/plain.ts`, and none of the others.
#[test]
fn denos_default_names_decide_in_a_deno_only_repository() {
    let files = [
        ("deno.json", "{}"),
        ("a_test.ts", IT_3),
        ("b.test.ts", IT_3),
        ("test.ts", IT_3),
        ("_test.ts", IT_3),
        ("__tests__/plain.ts", IT_3),
        ("plain.ts", IT_3),
        ("sub/c.spec.ts", IT_3),
        ("b_Test.ts", IT_3),
        ("tests/plain.ts", IT_3),
        ("a_test.d.ts", IT_3),
        ("__tests__/types.d.ts", IT_3),
        ("node_modules/p/z_test.ts", IT_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 2 + 5 * 3, "{}", run.stdout);
    assert!(
        !has_note(&run, &["runner collection unknown"]),
        "{:?}",
        notes(&run)
    );
    // Control: beside a root `package.json` the runner may be another one.
    let mut with_package = files.to_vec();
    with_package.push(("package.json", r#"{"name": "app"}"#));
    let (n, run) = counted(&with_package);
    assert_eq!(n, 2 + 12 * 3, "{}", run.stdout);
}

/// Each list was run with `deno test` (deno 2.6) over files of these names.
#[test]
fn deno_include_entries_and_a_negated_exclude_are_read_as_deno_reads_them() {
    let files = |config: &'static str| {
        [
            ("deno.json", config),
            ("a_test.ts", IT_3),
            ("plain.ts", IT_3),
            ("sub/b_test.ts", IT_3),
            ("sub/plain.ts", IT_3),
            ("other/d_test.ts", IT_3),
            ("other/keep/e_test.ts", IT_3),
            ("other/keep/plain.ts", IT_3),
        ]
    };
    for (config, tests) in [
        ("{}", 4),
        // A directory entry: the default names below it.
        (r#"{"test": {"include": ["sub"]}}"#, 1),
        // A file entry and a glob entry run the file whatever its name.
        (r#"{"test": {"include": ["sub/plain.ts"]}}"#, 1),
        (r#"{"test": {"include": ["sub/*.ts"]}}"#, 2),
        (r#"{"test": {"include": ["**/plain.ts"]}}"#, 3),
        // The last entry that holds a file decides.
        (r#"{"test": {"exclude": ["other", "!other/keep"]}}"#, 3),
        (
            r#"{"test": {"exclude": ["other", "!other/keep/e_test.ts"]}}"#,
            3,
        ),
        (
            r#"{"exclude": ["other"], "test": {"exclude": ["!other/keep"]}}"#,
            3,
        ),
        (
            r#"{"test": {"exclude": ["other", "!other/keep", "other/keep/e_test.ts"]}}"#,
            2,
        ),
        (r#"{"test": {"exclude": ["other/keep", "!other/keep"]}}"#, 4),
        (r#"{"test": {"exclude": ["o*", "!other/keep"]}}"#, 3),
        // Bringing a file back does not make its name a test name.
        (
            r#"{"test": {"exclude": ["other", "!other/keep/plain.ts"]}}"#,
            2,
        ),
        // With a glob `include`, what the negation brings back runs whatever its name.
        (
            r#"{"test": {"include": ["**/*.ts"], "exclude": ["other", "!other/keep"]}}"#,
            6,
        ),
    ] {
        let (n, run) = counted(&files(config));
        assert_eq!(n, 2 + tests * 3, "{config}: {}", run.stdout);
        assert!(
            !has_note(&run, &["runner collection unknown"]),
            "{config}: {:?}",
            notes(&run)
        );
    }
    // Not evaluated: every file counts, and the note says why.
    for (config, reason) in [
        // deno: "The negation of '!other/keep' is never reached".
        (
            r#"{"test": {"exclude": ["!other/keep", "other"]}}"#,
            "refuses to run",
        ),
        (
            r#"{"test": {"exclude": ["other"]}, "exclude": ["!other/keep"]}"#,
            "refuses to run",
        ),
        // deno leaves `other/keep/e_test.ts` out here; the rule behind it is not read.
        (
            r#"{"test": {"exclude": ["other/**", "!other/keep/**"]}}"#,
            "is a glob",
        ),
        (
            r#"{"tasks": {"test": "deno test sub/"}, "test": {"exclude": ["other"]}}"#,
            "paths of its own",
        ),
    ] {
        let (n, run) = counted(&files(config));
        let expected = if reason == "paths of its own" { 4 } else { 7 };
        assert_eq!(n, 2 + expected * 3, "{config}: {}", run.stdout);
        assert!(
            has_note(&run, &["head: runner collection unknown", reason]),
            "{config}: {:?}",
            notes(&run)
        );
    }
}

/// `deno test` (deno 2.6): `"vendor": true` leaves the root `vendor` directory out, and
/// a workspace member's own `test.exclude` applies below it (a `deno.json` below the
/// root that is not a member changes nothing).
#[test]
fn deno_vendor_and_workspace_members() {
    let (n, run) = counted(&[
        ("deno.json", r#"{"vendor": true}"#),
        ("a_test.ts", IT_3),
        ("vendor/v_test.ts", IT_3),
    ]);
    assert_eq!(n, 2 + 3, "{}", run.stdout);
    let nested = r#"{"test": {"exclude": ["skip"]}}"#;
    let (n, run) = counted(&[
        ("deno.json", "{}"),
        ("a_test.ts", IT_3),
        ("nested/deno.json", nested),
        ("nested/skip/s_test.ts", IT_3),
    ]);
    assert_eq!(n, 2 + 6, "{}", run.stdout);
    assert!(
        !has_note(&run, &["runner collection unknown"]),
        "{:?}",
        notes(&run)
    );
    let (n, run) = counted(&[
        ("deno.json", r#"{"workspace": ["./nested"]}"#),
        ("a_test.ts", IT_3),
        ("nested/deno.json", nested),
        ("nested/skip/s_test.ts", IT_3),
    ]);
    assert_eq!(n, 2 + 6, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "Deno workspace",
                "1 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------------- runners with no configuration

const JS_FILES: [(&str, &str); 4] = [
    ("src/a.test.ts", IT_3),
    ("src/plain.ts", IT_3),
    ("__tests__/b.ts", IT_3),
    ("src/c.spec.js", IT_3),
];

fn with(first: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    first.iter().copied().chain(JS_FILES).collect()
}

/// Vitest 1.6, 3.1 and 4.1 run `a.test.ts` and `c.spec.tsx` with a `vite.config.ts`
/// that has no `test` block, and with no configuration file at all.
#[test]
fn vitest_as_a_dependency_with_no_test_block_runs_with_its_defaults() {
    let package = r#"{"name": "app", "devDependencies": {"vitest": "^4.1.0"}}"#;
    let vite = "export default { plugins: [] };\n";
    for files in [
        with(&[("package.json", package), ("vite.config.ts", vite)]),
        with(&[("package.json", package)]),
    ] {
        let (n, run) = counted(&files);
        assert_eq!(n, 2 + 6, "{}", run.stdout);
        assert!(
            !has_note(&run, &["runner collection unknown"]),
            "{:?}",
            notes(&run)
        );
    }
    // Control: a `vite.config.*` in a repository that does not depend on Vitest
    // configures no test runner.
    let no_vitest = r#"{"name": "app", "devDependencies": {"vite": "^5.0.0"}}"#;
    let (n, run) = counted(&with(&[
        ("package.json", no_vitest),
        ("vite.config.ts", vite),
    ]));
    assert_eq!(n, 2 + 12, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &["runner collection unknown (no runner config found)"]
        ),
        "{:?}",
        notes(&run)
    );
}

/// Jest 27.5 and 29.7 list `__tests__/plain.js`, `a.test.js`, `b.spec.ts` and `test.js`
/// with a `package.json` that has no `jest` key.
#[test]
fn jest_as_a_dependency_with_no_jest_key_runs_with_its_defaults() {
    let package = r#"{"name": "app", "devDependencies": {"jest": "^29.7.0"}}"#;
    let (n, run) = counted(&with(&[("package.json", package)]));
    assert_eq!(n, 2 + 9, "{}", run.stdout);
    assert!(
        !has_note(&run, &["runner collection unknown"]),
        "{:?}",
        notes(&run)
    );
    // Control: with both runners as dependencies and neither configured, the files
    // are not known to be either's.
    let both = r#"{"name": "app", "devDependencies": {"jest": "^29.7.0", "vitest": "^4.1.0"}}"#;
    let (n, run) = counted(&with(&[("package.json", both)]));
    assert_eq!(n, 2 + 12, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &["runner collection unknown (no runner config found)"]
        ),
        "{:?}",
        notes(&run)
    );
}

// --------------------------------------------------------- Jest `testMatch`

/// Jest 27.5 and 29.7 list nothing for `"testMatch": ["src/**/*.js"]` or
/// `["./src/**/*.js"]`, and `b.spec.ts` alone when `"**/b.spec.ts"` is added.
#[test]
fn a_jest_test_match_pattern_that_cannot_match_an_absolute_path_matches_nothing() {
    let files = |package: &'static str| {
        [
            ("package.json", package),
            ("src/a.test.js", IT_3),
            ("src/plain.js", IT_3),
            ("b.spec.ts", IT_3),
        ]
    };
    for (package, tests) in [
        (r#"{"jest": {"testMatch": ["src/**/*.js"]}}"#, 0),
        (r#"{"jest": {"testMatch": ["./src/**/*.js"]}}"#, 0),
        (
            r#"{"jest": {"testMatch": ["src/**/*.js", "**/b.spec.ts"]}}"#,
            1,
        ),
        (r#"{"jest": {"testMatch": ["<rootDir>/src/**/*.js"]}}"#, 2),
    ] {
        let (n, run) = counted(&files(package));
        assert_eq!(n, 2 + tests * 3, "{package}: {}", run.stdout);
        assert!(
            !has_note(&run, &["runner collection unknown"]),
            "{package}: {:?}",
            notes(&run)
        );
    }
    // Control: a pattern that starts with a wildcard can match (`*/**/a.test.js` does
    // in Jest), and is still not evaluated.
    let (n, run) = counted(&files(r#"{"jest": {"testMatch": ["*/**/a.test.js"]}}"#));
    assert_eq!(n, 2 + 9, "{}", run.stdout);
    assert!(
        has_note(&run, &["runner collection unknown", "neither `<rootDir>`"]),
        "{:?}",
        notes(&run)
    );
    // A change that replaces a matching pattern with one that matches nothing takes
    // the tests out of the default run.
    let run = changed(
        &files(r#"{"jest": {"testMatch": ["<rootDir>/src/**/*.js"]}}"#),
        |repo| {
            repo.write(
                "package.json",
                r#"{"jest": {"testMatch": ["src/**/*.js"]}}"#,
            )
        },
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

/// Jest 27.5 and 29.7 stop with "Configuration options testMatch and testRegex cannot be
/// used together"; Jest 29.5 and 29.7 stop with "Multiple configurations found" for a
/// `jest` key beside a `jest.config.json` (Jest 27.5 warns and reads the file).
#[test]
fn a_configuration_jest_refuses_to_run_with_is_not_evaluated() {
    let both = r#"{"jest": {"testMatch": ["**/a.test.js"], "testRegex": "b\\.spec"}}"#;
    let files = |package: &'static str| {
        [
            ("package.json", package),
            ("a.test.js", IT_3),
            ("b.spec.js", IT_3),
            ("plain.js", IT_3),
        ]
    };
    let (n, run) = counted(&files(both));
    assert_eq!(n, 2 + 9, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "`testMatch` and `testRegex`",
                "3 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // An empty `testRegex` is no conflict: Jest lists what `testMatch` matches.
    let empty = r#"{"jest": {"testMatch": ["**/a.test.js"], "testRegex": ""}}"#;
    let (n, run) = counted(&files(empty));
    assert_eq!(n, 2 + 3, "{}", run.stdout);
    // Two configurations.
    let mut two = files(r#"{"jest": {"testMatch": ["**/a.test.js"]}}"#).to_vec();
    two.push(("jest.config.json", r#"{"testMatch": ["**/b.spec.js"]}"#));
    let (n, run) = counted(&two);
    assert_eq!(n, 2 + 9, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "more than one jest configuration",
                "3 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: a script that names one of them with `--config` leaves no conflict.
    let mut named = two.clone();
    named[0] = (
        "package.json",
        r#"{"scripts": {"test": "jest --config jest.config.json"}, "jest": {"testMatch": ["**/a.test.js"]}}"#,
    );
    let (n, run) = counted(&named);
    assert_eq!(n, 2 + 3, "{}", run.stdout);
}

// ------------------------------------------------- Vitest default `exclude`

/// Vitest 1.6 and 3.1 run neither `vite.config.test.ts` nor `sub/jest.config.spec.ts`
/// (nor `dist/b.test.ts`); Vitest 4.1 runs all three. `playwright.config.test.ts` is in
/// no version's list.
#[test]
fn vitests_older_default_exclude_holds_configuration_file_names() {
    let config = "export default { test: { environment: 'node' } };\n";
    let files = |package: &'static str| {
        [
            ("package.json", package),
            ("vitest.config.ts", config),
            ("src/a.test.ts", IT_3),
            ("vite.config.test.ts", IT_3),
            ("sub/jest.config.spec.ts", IT_3),
            ("playwright.config.test.ts", IT_3),
            ("dist/b.test.ts", IT_3),
        ]
    };
    let package = |range: &str| -> &'static str {
        Box::leak(
            format!(r#"{{"name": "app", "devDependencies": {{"vitest": "{range}"}}}}"#)
                .into_boxed_str(),
        )
    };
    for (range, tests) in [("^1.6.1", 2), ("^3.1.3", 2), ("^4.1.0", 5)] {
        let (n, run) = counted(&files(package(range)));
        assert_eq!(n, 2 + tests * 3, "{range}: {}", run.stdout);
        assert!(
            !has_note(&run, &["runner collection unknown"]),
            "{range}: {:?}",
            notes(&run)
        );
    }
    // Vitest 2 was not available to compare with: the two files count, with a note.
    let (n, run) = counted(&files(package("^2.1.0")));
    assert_eq!(n, 2 + 4 * 3, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "vitest 2 was not compared",
                "2 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // No plain range: counted, with a note.
    let (n, run) = counted(&files(package("*")));
    assert_eq!(n, 2 + 5 * 3, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "named like a tool's configuration",
                "2 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------------- a rename into an existing rule

/// The workspace `exclude` entry is on both sides, so the rule is not the change's own
/// and the finding is not reported; the count does not move, and the note names the
/// file under its new path.
#[test]
fn a_test_file_renamed_under_an_existing_rule_is_named_in_a_note() {
    let workspace = "[workspace]\nmembers = [\"member\"]\nexclude = [\"out\"]\n";
    let run = changed(
        &[
            ("Cargo.toml", workspace),
            ("member/Cargo.toml", PACKAGE),
            ("member/tests/it.rs", RS_1),
            ("member/tests/more.rs", RS_3),
            ("out/Cargo.toml", PACKAGE),
            ("out/tests/it.rs", RS_1),
        ],
        |repo| repo.git(&["mv", "member/tests/more.rs", "out/tests/more.rs"]),
    );
    assert!(moved(&run).is_empty(), "{}", run.stdout);
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: `out/tests/more.rs` (renamed from `member/tests/more.rs`) was collected on the base side",
                "`exclude` list of the workspace",
                "still counted"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------- a Cargo target its manifest turns off

const LIB: &str = "mod inner;\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn lib_unit() {\n        assert_eq!(1, 1);\n    }\n}\n";
const LIB_WITHOUT_MOD: &str =
    "#[cfg(test)]\nmod tests {\n    #[test]\n    fn lib_unit() {\n        assert_eq!(1, 1);\n    }\n}\n";

fn cargo_base() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", LIB),
        ("src/inner.rs", RS_3),
        ("tests/it.rs", RS_3),
        ("tests/other.rs", RS_1),
    ]
}

fn manifest(extra: &str) -> String {
    format!("{PACKAGE}{extra}")
}

/// `cargo test -- --list` (cargo 1.98) in a crate of this shape lists no test of the
/// target once its manifest sets `test = false` on it, none of `tests/` with
/// `autotests = false`, and none of a module whose `mod` line is gone. In each of these
/// tests the change adds as many tests as it switches off, so the count does not show it.
#[test]
fn a_test_target_switched_off_over_existing_tests_is_reported() {
    let run = changed(&cargo_base(), |repo| {
        repo.write(
            "Cargo.toml",
            &manifest("\n[[test]]\nname = \"it\"\ntest = false\n"),
        );
        repo.write("tests/new.rs", RS_3);
    });
    assert_moved(
        &run,
        "Cargo.toml",
        &[
            "`test = false` or `harness = false` on a `[[test]]` target in `Cargo.toml`",
            "3 test(s) in 1 file(s)",
            "`tests/it.rs`",
            "no longer in the static count",
        ],
    );
}

/// The tests move into the library, which still runs.
#[test]
fn autotests_switched_off_over_existing_tests_is_reported() {
    let run = changed(&cargo_base(), |repo| {
        repo.write(
            "Cargo.toml",
            &PACKAGE.replace("edition", "autotests = false\nedition"),
        );
        repo.write(
            "src/more.rs",
            &format!("{RS_3}\n{RS_3}").replace("fn ", "fn m_"),
        );
        repo.write("src/lib.rs", &format!("mod more;\n{LIB}"));
    });
    assert_moved(
        &run,
        "Cargo.toml",
        &[
            "`autotests = false` in `Cargo.toml`",
            "6 test(s) in 3 file(s)",
            "`tests/a.rs`",
            "`tests/it.rs`",
            "`tests/other.rs`",
        ],
    );
}

#[test]
fn a_library_whose_tests_are_switched_off_over_existing_tests_is_reported() {
    let run = changed(&cargo_base(), |repo| {
        repo.write("Cargo.toml", &manifest("\n[lib]\ntest = false\n"));
        repo.write(
            "tests/new.rs",
            &format!("{RS_3}\n{RS_1}").replace("fn ", "fn n_"),
        );
    });
    assert_moved(
        &run,
        "Cargo.toml",
        &[
            "under `[lib]` in `Cargo.toml`",
            "4 test(s) in 2 file(s)",
            "`src/inner.rs`",
            "`src/lib.rs`",
        ],
    );
}

/// `cargo test -- --list` (cargo 1.98) lists the binary's unit tests until its `[[bin]]`
/// sets `test = false`.
#[test]
fn a_binary_whose_tests_are_switched_off_over_existing_tests_is_reported() {
    let main = "mod part;\n\nfn main() {}\n";
    let mut base = cargo_base();
    base.push(("src/main.rs", main));
    base.push(("src/part.rs", RS_3));
    let run = changed(&base, |repo| {
        repo.write(
            "Cargo.toml",
            &manifest("\n[[bin]]\nname = \"pkg\"\ntest = false\n"),
        );
        repo.write("tests/new.rs", RS_3);
    });
    assert_moved(
        &run,
        "Cargo.toml",
        &[
            "on a `[[bin]]` target in `Cargo.toml`",
            "3 test(s) in 1 file(s)",
            "`src/part.rs`",
        ],
    );
}

/// The file that held the `mod` line is the one named.
#[test]
fn a_mod_line_removed_over_existing_tests_is_reported() {
    let run = changed(&cargo_base(), |repo| {
        repo.write("src/lib.rs", LIB_WITHOUT_MOD);
        repo.write("tests/new.rs", RS_3);
    });
    assert_moved(
        &run,
        "src/lib.rs",
        &[
            "The `mod` declaration no longer in `src/lib.rs`",
            "3 test(s) in 1 file(s)",
            "`src/inner.rs`",
        ],
    );
}

#[test]
fn a_cargo_target_that_was_already_off_or_whose_tests_are_gone_is_not_reported() {
    let off = manifest("\n[[test]]\nname = \"it\"\ntest = false\n");
    // The key is on both sides.
    let mut base = cargo_base();
    base[0] = ("Cargo.toml", Box::leak(off.clone().into_boxed_str()));
    let run = changed(&base, |repo| repo.write("tests/new.rs", RS_3));
    assert!(moved(&run).is_empty(), "{}", run.stdout);
    // The change deletes the tests with the target: the count reports it.
    let run = changed(&cargo_base(), |repo| {
        repo.write("Cargo.toml", &off);
        repo.remove("tests/it.rs");
    });
    assert!(moved(&run).is_empty(), "{}", run.stdout);
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    // With nothing added, the count finding reports the move, and the rule is a note
    // beside it: one finding for one cause.
    let run = changed(&cargo_base(), |repo| repo.write("Cargo.toml", &off));
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert!(
        has_note(
            &run,
            &["head: `test = false` or `harness = false` on a `[[test]]` target in `Cargo.toml`"]
        ),
        "{:?}",
        notes(&run)
    );
}

/// A file under `src/` that no crate root reaches is not compiled as far as the model
/// can tell: it is left out, and a note gives the number of such files that hold tests.
#[test]
fn a_source_file_no_crate_root_reaches_is_left_out_with_a_note() {
    let macro_root = "declare_modules!(inner);\n";
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", macro_root),
        ("src/inner.rs", RS_3),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: a Rust file under `src/` that no crate root reaches",
                "a macro of another crate",
                "in 1 file(s) are not counted"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: a file the root declares counts, with no such note.
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", "mod inner;\n"),
        ("src/inner.rs", RS_3),
    ]);
    assert_eq!(n, 2 + 3, "{}", run.stdout);
    assert!(
        !has_note(&run, &["no crate root reaches"]),
        "{:?}",
        notes(&run)
    );
}

/// The same under `tests/`: a file that is no target and that no target declares as a
/// module (cargo 1.98 lists none of its tests) is left out with a note of its own.
#[test]
fn a_file_under_tests_no_target_reaches_is_left_out_with_a_note() {
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", "declare_modules!(inner);\n"),
        ("src/inner.rs", RS_3),
        ("tests/it.rs", "mod common;\n"),
        ("tests/common/mod.rs", RS_1),
        ("tests/disabled/old.rs", RS_3),
        ("tests/disabled/older.rs", RS_3),
    ]);
    // `tests/a.rs` and the module `tests/it.rs` declares.
    assert_eq!(n, 2 + 1, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: a Rust file under `tests/` that no test target reaches",
                "a macro of another crate",
                "in 2 file(s) are not counted"
            ]
        ),
        "{:?}",
        notes(&run)
    );
    // Its own count: the file under `src/` is in the other note.
    assert!(
        has_note(
            &run,
            &[
                "head: a Rust file under `src/` that no crate root reaches",
                "in 1 file(s) are not counted"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------------- `node --test` in a nested manifest

#[test]
fn a_node_test_script_of_a_nested_manifest_runs_the_files_below_it() {
    let root = r#"{"name": "root", "private": true}"#;
    let api = r#"{"name": "api", "scripts": {"test": "node --test"}}"#;
    let web = r#"{"name": "web", "scripts": {"lint": "node --check x.js"}}"#;
    let (n, run) = counted(&[
        ("package.json", root),
        ("packages/api/package.json", api),
        ("packages/api/test/a.test.js", NODE_TEST_3),
        ("packages/web/package.json", web),
        ("packages/web/test/b.test.js", NODE_TEST_3),
    ]);
    assert_eq!(n, 2 + 3, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &["head: ", "`node --test`", "in 1 file(s) are not counted"]
        ),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------------------------------------ `go.work`

/// `go list -m` (go 1.25) loads the module at `./app` for `use "./\x61pp"`, and stops
/// with `unknown directive: bogus` for a line it does not know.
#[test]
fn a_go_work_is_read_as_the_go_tool_reads_it() {
    let files = |work: &'static str| {
        [
            ("go.work", work),
            ("app/go.mod", GO_MOD),
            ("app/x_test.go", GO_3),
        ]
    };
    let (n, run) = counted(&files("go 1.21\n\nuse \"./\\x61pp\"\n"));
    assert_eq!(n, 2 + 3, "{}", run.stdout);
    assert!(
        !has_note(&run, &["runner collection unknown"]),
        "{:?}",
        notes(&run)
    );
    let (n, run) = counted(&files("go 1.21\n\nuse ./app\nbogus x\n"));
    assert_eq!(n, 2 + 3, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            &[
                "head: runner collection unknown",
                "`go.work` cannot be read",
                "1 file(s)"
            ]
        ),
        "{:?}",
        notes(&run)
    );
}
