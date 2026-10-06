//! `test-floor` models each runner's collection so that a test that stops running also
//! leaves the static count (#557), and so that a file no runner in the repository could
//! run does not hold the count up (#577). Each case here is a configuration the model
//! read differently from the runner.
//!
//! `Repo::new()` already holds `tests/a.rs` with two tests and no `Cargo.toml`, so every
//! count includes 2.

mod common;
use common::Repo;

const BELOW_FLOOR: &str = "Test Count Below Floor";

const JS_3: &str = "\
it('renders', () => { expect(render()).toBe(1); });
it('clicks', () => { expect(click()).toBe(2); });
it('submits', () => { expect(submit()).toBe(3); });
";
const JS_1: &str = "it('renders', () => { expect(render()).toBe(1); });\n";

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

const PY_3: &str = "\
def test_one():
    assert one() == 1


def test_two():
    assert two() == 2


def test_three():
    assert three() == 3
";
const PY_1: &str = "def test_one():\n    assert one() == 1\n";

const PACKAGE: &str = "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// The static count of a repository holding `files`, read from a change that touches
/// only a document.
fn counted(files: &[(&str, &str)]) -> (u64, common::Run) {
    let repo = Repo::new();
    repo.commit_base_files(files, "test: base suite");
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 3.\n");
    repo.commit("docs: reorder the plan");
    let run = repo.check(&[]);
    let examined = run.outcome("test-floor")["examined"]
        .as_u64()
        .unwrap_or_else(|| panic!("no examined count:\n{}", run.stdout));
    (examined, run)
}

/// `base` on the base side, then `change` applied on the head side.
fn changed(base: &[(&str, &str)], change: impl FnOnce(&Repo)) -> common::Run {
    let repo = Repo::new();
    repo.commit_base_files(base, "test: base suite");
    change(&repo);
    repo.commit("chore: change the suite");
    repo.check(&[])
}

fn notes(run: &common::Run) -> Vec<String> {
    run.outcome("test-floor")["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

fn has_unknown_note(run: &common::Run) -> bool {
    notes(run)
        .iter()
        .any(|n| n.starts_with("head: runner collection unknown"))
}

fn jest(config: &str) -> String {
    format!(r#"{{"name": "app", "jest": {config}}}"#)
}

// ---------------------------------------------------------------- Jest / Vitest

#[test]
fn jest_test_regex_is_matched_as_jest_matches_the_absolute_path() {
    let pkg = jest(r#"{"testRegex": "/tests/.*\\.js$"}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("tests/a.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Control: a file the pattern does not name stays out.
    let (n, run) = counted(&[("package.json", &pkg), ("lib/a.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
}

#[test]
fn a_group_in_jest_test_match_is_not_read_as_literal_characters() {
    let pkg = jest(r#"{"testMatch": ["**/*.(test|spec).(ts|js)"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn jest_defaults_collect_a_file_named_test_js_and_not_a_test_helper() {
    let pkg = jest("{}");
    let (n, run) = counted(&[("package.json", &pkg), ("src/test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&[("package.json", &pkg), ("src/spec.ts", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    // `a.test.helper.js` does not end in `.test.js`: Jest's default patterns skip it.
    let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.helper.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    // Control: the usual name.
    let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn jest_projects_and_preset_leave_collection_undetermined() {
    for config in [
        r#"{"projects": ["<rootDir>/packages/*"], "testMatch": ["**/*.spec.js"]}"#,
        // A preset with no pattern of the configuration's own: the preset may set them.
        r#"{"preset": "some-preset"}"#,
    ] {
        let pkg = jest(config);
        let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.js", JS_3)]);
        assert_eq!(n, 5, "{config}: {}", run.stdout);
        assert!(has_unknown_note(&run), "{config}: {:?}", notes(&run));
    }
}

/// A preset only supplies defaults: the configuration's own `testMatch` overrides the
/// preset's, so it decides as it does with no preset.
#[test]
fn a_jest_preset_does_not_hide_the_configurations_own_test_match() {
    let pkg = jest(r#"{"preset": "ts-jest", "testMatch": ["**/src/**/*.test.js"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("parked/a.test.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));

    let pkg = jest(r#"{"preset": "ts-jest", "testRegex": "/src/.*\\.test\\.js$"}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("parked/a.test.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
}

#[test]
fn moving_a_test_out_of_test_match_under_a_jest_preset_is_a_count_drop() {
    let pkg = jest(r#"{"preset": "ts-jest", "testMatch": ["**/src/**/*.test.js"]}"#);
    let run = changed(&[("package.json", &pkg), ("src/a.test.js", JS_3)], |repo| {
        repo.remove("src/a.test.js");
        repo.write("parked/a.test.js", JS_3);
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

/// With a second runner present, a file Jest's patterns match is Jest's as usual; only
/// a file they do not match is undetermined.
#[test]
fn a_file_jest_matches_is_collected_beside_a_second_runner() {
    let pkg = r#"{"name": "app", "jest": {"testMatch": ["**/src/**/*.test.js"]}, "devDependencies": {"@playwright/test": "^1.0.0"}}"#;
    let (n, run) = counted(&[("package.json", pkg), ("src/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    let (n, run) = counted(&[("package.json", pkg), ("e2e/login.spec.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
    // Both together: one note, for the one file Jest does not match.
    let (n, run) = counted(&[
        ("package.json", pkg),
        ("src/a.test.js", JS_3),
        ("e2e/login.spec.js", JS_3),
    ]);
    assert_eq!(n, 8, "{}", run.stdout);
    let notes = notes(&run);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("head: runner collection unknown") && n.contains("1 file(s)")),
        "{notes:?}"
    );
}

#[test]
fn a_relative_jest_root_resolves_against_root_dir() {
    let pkg = jest(r#"{"rootDir": "packages/a", "roots": ["src"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("packages/a/src/x.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    // The repository's own `src/` is not under `packages/a/src`.
    let (n, run) = counted(&[("package.json", &pkg), ("src/x.test.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
}

#[test]
fn a_second_javascript_runner_leaves_collection_undetermined() {
    for dep in ["cypress", "@playwright/test"] {
        let pkg = format!(
            r#"{{"name": "app", "jest": {{"testMatch": ["**/src/**/*.test.js"]}}, "devDependencies": {{"{dep}": "^1.0.0"}}}}"#
        );
        let (n, run) = counted(&[("package.json", &pkg), ("e2e/login.cy.js", JS_3)]);
        assert_eq!(n, 5, "{dep}: {}", run.stdout);
        assert!(has_unknown_note(&run), "{dep}: {:?}", notes(&run));
    }
    // Control: with Jest alone the configured pattern decides.
    let pkg = jest(r#"{"testMatch": ["**/src/**/*.test.js"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("e2e/login.cy.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn jest_test_path_ignore_patterns_exclude_a_file() {
    let pkg = jest(r#"{"testPathIgnorePatterns": ["/legacy/"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("legacy/a.test.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    // Control: a file the pattern does not name.
    let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
}

/// The security property: a change that only edits the runner configuration so that
/// tests stop running lowers the count.
#[test]
fn adding_a_jest_ignore_pattern_over_existing_tests_is_a_count_drop() {
    let run = changed(
        &[("package.json", &jest("{}")), ("legacy/a.test.js", JS_3)],
        |repo| {
            repo.write(
                "package.json",
                &jest(r#"{"testPathIgnorePatterns": ["/legacy/"]}"#),
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

#[test]
fn a_negated_jest_test_match_pattern_excludes_a_file() {
    let pkg = jest(r#"{"testMatch": ["**/*.test.js", "!**/legacy/**"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("legacy/a.test.js", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    let (n, run) = counted(&[("package.json", &pkg), ("src/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_jest_test_match_pattern_that_is_not_anchored_is_undetermined() {
    // Jest matches the absolute path, which `tests/**` never starts.
    let pkg = jest(r#"{"testMatch": ["tests/**/*.test.js"]}"#);
    let (n, run) = counted(&[("package.json", &pkg), ("tests/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_vitest_exclude_leaves_collection_undetermined() {
    let pkg = r#"{"name": "app", "vitest": {"exclude": ["**/legacy/**"]}}"#;
    let (n, run) = counted(&[("package.json", pkg), ("legacy/a.test.js", JS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_nested_package_with_its_own_jest_configuration_is_not_judged_by_the_root_one() {
    let root = jest(r#"{"testMatch": ["**/*.spec.js"]}"#);
    let nested = r#"{"name": "a", "jest": {}}"#;
    let (n, run) = counted(&[
        ("package.json", &root),
        ("packages/a/package.json", nested),
        ("packages/a/src/x.test.js", JS_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
    // Control: a nested manifest with no runner configuration changes nothing.
    let (n, run) = counted(&[
        ("package.json", &root),
        ("packages/a/package.json", r#"{"name": "a"}"#),
        ("packages/a/src/x.test.js", JS_3),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
}

// ---------------------------------------------------------------- Cargo / Rust

#[test]
fn a_cargo_test_target_with_test_false_or_no_harness_is_not_counted() {
    for key in ["test = false", "harness = false"] {
        let manifest =
            format!("{PACKAGE}\n[[test]]\nname = \"off\"\npath = \"tests/off.rs\"\n{key}\n");
        let (n, run) = counted(&[("Cargo.toml", &manifest), ("tests/off.rs", RS_3)]);
        assert_eq!(n, 2, "{key}: {}", run.stdout);
    }
    // Control: the same target with neither key.
    let manifest = format!("{PACKAGE}\n[[test]]\nname = \"off\"\npath = \"tests/off.rs\"\n");
    let (n, run) = counted(&[("Cargo.toml", &manifest), ("tests/off.rs", RS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn setting_test_false_on_a_cargo_target_is_a_count_drop() {
    let base = format!("{PACKAGE}\n[[test]]\nname = \"it\"\npath = \"tests/it.rs\"\n");
    let run = changed(&[("Cargo.toml", &base), ("tests/it.rs", RS_3)], |repo| {
        repo.write("Cargo.toml", &format!("{base}test = false\n"))
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn autotests_false_leaves_out_a_test_file_no_target_names() {
    let manifest = format!(
        "{}autotests = false\n\n[[test]]\nname = \"listed\"\npath = \"tests/listed.rs\"\n",
        PACKAGE
    );
    let (n, run) = counted(&[
        ("Cargo.toml", &manifest),
        ("tests/listed.rs", RS_1),
        ("tests/unlisted.rs", RS_3),
    ]);
    // Only `tests/listed.rs` (1): the fixture's own `tests/a.rs` is not listed either.
    assert_eq!(n, 1, "{}", run.stdout);
    // Control: with auto-discovery on, all three files are targets.
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        ("tests/listed.rs", RS_1),
        ("tests/unlisted.rs", RS_3),
    ]);
    assert_eq!(n, 6, "{}", run.stdout);
}

#[test]
fn lib_test_false_leaves_out_the_unit_tests_of_a_library_only_package() {
    // The library root declares the module, so `src/checks.rs` is library code.
    let lib = ("src/lib.rs", "mod checks;\n");
    let manifest = format!("{PACKAGE}\n[lib]\ntest = false\n");
    let (n, run) = counted(&[("Cargo.toml", &manifest), lib, ("src/checks.rs", RS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // A binary beside the library runs its own unit tests, and this one declares no
    // module: the library's file is still left out. `cargo test -- --list` on this
    // package lists the two tests of `tests/a.rs` and nothing from `src/checks.rs`.
    let (n, run) = counted(&[
        ("Cargo.toml", &manifest),
        lib,
        ("src/checks.rs", RS_3),
        ("src/main.rs", "fn main() {}\n"),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // Control: the default.
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), lib, ("src/checks.rs", RS_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn a_file_under_tests_that_is_a_module_of_no_target_is_not_counted() {
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        ("tests/disabled/b.rs", RS_3),
        ("tests/it/main.rs", "mod foo;\n"),
        ("tests/it/foo.rs", RS_3),
        ("tests/it/orphan.rs", RS_3),
    ]);
    // `tests/a.rs` (2) and `tests/it/foo.rs` (3).
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_module_declared_through_a_path_attribute_or_a_nested_module_is_counted() {
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        (
            "tests/it.rs",
            "#[path = \"shared/cases.rs\"]\nmod cases;\nmod deep;\n",
        ),
        ("tests/shared/cases.rs", RS_3),
        ("tests/deep/mod.rs", "mod leaf;\n"),
        ("tests/deep/leaf.rs", RS_3),
    ]);
    assert_eq!(n, 8, "{}", run.stdout);
}

#[test]
fn modules_a_macro_may_declare_are_counted_with_a_note() {
    let (n, run) = counted(&[
        ("Cargo.toml", PACKAGE),
        ("tests/it.rs", "automod::dir!(\"tests/cases\");\n"),
        ("tests/cases/one.rs", RS_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn dropping_the_mod_line_of_a_test_module_is_a_count_drop() {
    let run = changed(
        &[
            ("Cargo.toml", PACKAGE),
            ("tests/it/main.rs", "mod foo;\n"),
            ("tests/it/foo.rs", RS_3),
        ],
        |repo| repo.write("tests/it/main.rs", "// mod foo;\n"),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn moving_a_test_file_into_a_subdirectory_of_tests_is_a_count_drop() {
    let run = changed(&[("Cargo.toml", PACKAGE), ("tests/it.rs", RS_3)], |repo| {
        repo.remove("tests/it.rs");
        repo.write("tests/disabled/it.rs", RS_3);
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn a_workspace_member_test_target_outside_tests_is_counted() {
    let member = format!("{PACKAGE}\n[[test]]\nname = \"x\"\npath = \"checks/x.rs\"\n");
    let (n, run) = counted(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/m\"]\n"),
        ("crates/m/Cargo.toml", &member),
        ("crates/m/checks/x.rs", RS_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn files_beside_a_lib_path_outside_src_are_counted_with_a_note() {
    let member = format!("{PACKAGE}\n[lib]\npath = \"lib.rs\"\n");
    let (n, run) = counted(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/m\"]\n"),
        ("crates/m/Cargo.toml", &member),
        ("crates/m/lib.rs", RS_3),
        ("crates/m/util.rs", RS_3),
    ]);
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
}

/// Not decided (#557): a package the workspace excludes is not built by `cargo test`
/// at the workspace root, and is often tested by its own invocation. It still counts.
#[test]
fn a_package_the_workspace_excludes_still_counts() {
    let (n, run) = counted(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"member\"]\nexclude = [\"out\"]\n",
        ),
        ("member/Cargo.toml", PACKAGE),
        ("member/tests/it.rs", RS_1),
        ("out/Cargo.toml", PACKAGE),
        ("out/tests/it.rs", RS_3),
    ]);
    assert_eq!(n, 6, "{}", run.stdout);
}

#[test]
fn a_test_behind_cfg_false_is_not_counted() {
    let src = format!(
        "#[cfg(false)]\n{RS_1}\n#[cfg(true)]\n#[test]\nfn kept() {{\n    assert_eq!(1, 1);\n}}\n"
    );
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/c.rs", &src)]);
    assert_eq!(n, 3, "{}", run.stdout);
}

#[test]
fn a_module_cfg_separated_by_a_comment_or_written_as_an_inner_attribute_is_read() {
    let commented = "\
#[cfg(any())]
// parked until the service is back
mod parked {
    #[test]
    fn one() {
        assert_eq!(1, 1);
    }
}
";
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/c.rs", commented)]);
    assert_eq!(n, 2, "{}", run.stdout);

    let inner = "\
mod parked {
    #![cfg(any())]

    #[test]
    fn one() {
        assert_eq!(1, 1);
    }
}
";
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/c.rs", inner)]);
    assert_eq!(n, 2, "{}", run.stdout);

    let file_level = format!("#![cfg(any())]\n\n{RS_3}");
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/c.rs", &file_level)]);
    assert_eq!(n, 2, "{}", run.stdout);

    // Control: a module with no cfg.
    let plain = "mod live {\n    #[test]\n    fn one() {\n        assert_eq!(1, 1);\n    }\n}\n";
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/c.rs", plain)]);
    assert_eq!(n, 3, "{}", run.stdout);
}

#[test]
fn a_target_specific_optional_dependency_is_a_feature() {
    let manifest = format!(
        "{PACKAGE}\n[target.'cfg(unix)'.dependencies]\nfoo = {{ version = \"1\", optional = true }}\n"
    );
    let src = format!("#[cfg(feature = \"foo\")]\n{RS_3}");
    let (n, run) = counted(&[("Cargo.toml", &manifest), ("tests/f.rs", &src)]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Control: a feature nothing declares never builds.
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/f.rs", &src)]);
    assert_eq!(n, 4, "{}", run.stdout);
}

#[test]
fn not_test_inside_a_larger_cfg_does_not_park_the_test() {
    let src = format!("#[cfg(any(not(test), unix))]\n{RS_3}");
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/f.rs", &src)]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Control: `not(test)` alone never builds under `cargo test`.
    let src = format!("#[cfg(not(test))]\n{RS_3}");
    let (n, run) = counted(&[("Cargo.toml", PACKAGE), ("tests/f.rs", &src)]);
    assert_eq!(n, 4, "{}", run.stdout);
}

/// Base tests are judged with the base manifests and head tests with the head ones.
#[test]
fn a_feature_removed_from_the_manifest_parks_the_tests_behind_it_on_the_head_side_only() {
    let with_feature = format!("{PACKAGE}\n[features]\nslow = []\n");
    let src = format!("#[cfg(feature = \"slow\")]\n{RS_3}");
    let run = changed(
        &[("Cargo.toml", &with_feature), ("tests/f.rs", &src)],
        |repo| repo.write("Cargo.toml", PACKAGE),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome("test-floor")["examined"], 4, "{}", run.stdout);
}

// ---------------------------------------------------------------- Go

#[test]
fn go_directories_the_go_tool_ignores_are_not_counted() {
    let (n, run) = counted(&[
        ("go.mod", "module example.test/m\n\ngo 1.21\n"),
        ("pkg/a_test.go", GO_3),
        ("vendor/dep/a_test.go", GO_3),
        ("pkg/testdata/x/a_test.go", GO_3),
        ("_old/a_test.go", GO_3),
        (".hidden/a_test.go", GO_3),
        ("pkg/_parked_test.go", GO_3),
        ("pkg/.parked_test.go", GO_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Code directly inside a directory named `vendor` is an ordinary package.
    let (n, run) = counted(&[
        ("go.mod", "module example.test/m\n\ngo 1.21\n"),
        ("vendor/a_test.go", GO_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn removing_a_go_vendor_directory_is_not_a_count_drop() {
    let run = changed(
        &[
            ("go.mod", "module example.test/m\n\ngo 1.21\n"),
            ("pkg/a_test.go", GO_3),
            ("vendor/dep/a_test.go", GO_3),
        ],
        |repo| repo.remove("vendor"),
    );
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["examined"], 5, "{}", run.stdout);
}

/// Control for the Go rules: removing a package's own tests is still a drop.
#[test]
fn removing_a_go_package_test_file_is_still_a_count_drop() {
    let run = changed(
        &[
            ("go.mod", "module example.test/m\n\ngo 1.21\n"),
            ("pkg/a_test.go", GO_3),
        ],
        |repo| repo.remove("pkg/a_test.go"),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

/// Decided (#593): `//go:build ignore` keeps a file out of every build by convention,
/// so its tests are not counted. A tag the test invocation may set still counts, with a
/// note (`tests/test_runner_collection_decisions.rs`).
#[test]
fn a_go_file_behind_build_ignore_is_not_counted() {
    let src = format!("//go:build ignore\n\n{GO_3}");
    let (n, run) = counted(&[
        ("go.mod", "module example.test/m\n\ngo 1.21\n"),
        ("pkg/a_test.go", &src),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
}

// ---------------------------------------------------------------- pytest

#[test]
fn a_pytest_testpath_written_with_a_leading_dot_slash_is_read() {
    let cfg = "[tool.pytest.ini_options]\ntestpaths = [\"./tests\"]\n";
    let (n, run) = counted(&[("pyproject.toml", cfg), ("tests/test_a.py", PY_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&[("pyproject.toml", cfg), ("other/test_o.py", PY_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
}

#[test]
fn a_pytest_testpath_glob_matches_the_directories_it_names() {
    let cfg = "[tool.pytest.ini_options]\ntestpaths = [\"pkgs/*/tests\"]\n";
    let (n, run) = counted(&[("pyproject.toml", cfg), ("pkgs/x/tests/test_x.py", PY_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&[("pyproject.toml", cfg), ("other/test_o.py", PY_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    // `*` does not cross a directory.
    let (n, run) = counted(&[("pyproject.toml", cfg), ("pkgs/x/y/tests/test_x.py", PY_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
}

#[test]
fn an_empty_pytest_ini_shadows_pyproject_toml() {
    let (n, run) = counted(&[
        (
            "pyproject.toml",
            "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
        ),
        ("pytest.ini", ""),
        ("other/test_o.py", PY_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn pyproject_toml_wins_over_tox_ini_and_setup_cfg() {
    let (n, run) = counted(&[
        (
            "pyproject.toml",
            "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
        ),
        ("setup.cfg", "[tool:pytest]\ntestpaths = other\n"),
        ("tests/test_a.py", PY_3),
        ("other/test_o.py", PY_1),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&[
        ("tox.ini", "[pytest]\ntestpaths = tests\n"),
        ("setup.cfg", "[tool:pytest]\ntestpaths = other\n"),
        ("tests/test_a.py", PY_3),
        ("other/test_o.py", PY_1),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Control: `setup.cfg` alone decides.
    let (n, run) = counted(&[
        ("setup.cfg", "[tool:pytest]\ntestpaths = other\n"),
        ("tests/test_a.py", PY_3),
        ("other/test_o.py", PY_1),
    ]);
    assert_eq!(n, 3, "{}", run.stdout);
}

/// Not fixed (#557): the Python pack names tests by pytest's default patterns, so a
/// `python_functions` override is not applied. pytest would collect `check_two` only.
#[test]
fn a_python_functions_override_is_not_applied_to_test_names() {
    let (n, run) = counted(&[
        (
            "pyproject.toml",
            "[tool.pytest.ini_options]\npython_functions = [\"check_*\"]\n",
        ),
        (
            "tests/test_e.py",
            "def test_one():\n    assert one() == 1\n\n\ndef check_two():\n    assert two() == 2\n",
        ),
    ]);
    assert_eq!(n, 3, "{}", run.stdout);
}

// ---------------------------------------------------------------- #577

const VENDORED_JS: &str = "site/assets/javascripts/bundle.js";

#[test]
fn javascript_in_a_repository_with_no_javascript_manifest_is_not_counted() {
    let run = changed(
        &[
            ("pytest.ini", "[pytest]\n"),
            ("tests/test_a.py", PY_3),
            (VENDORED_JS, JS_3),
        ],
        |repo| repo.remove("site"),
    );
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["examined"], 5, "{}", run.stdout);
    let notes = notes(&run);
    // Left out with a note on the side that holds the file, never silently.
    assert!(
        notes.iter().any(|n| n.starts_with("base: ")
            && n.contains("not counted")
            && n.contains("1 file(s)")),
        "{notes:?}"
    );
}

/// Control: a manifest anywhere in the repository is a sign of a JavaScript runner, and
/// the blind spot #521 closed stays closed.
#[test]
fn javascript_counts_when_a_package_manifest_exists_anywhere() {
    for manifest in ["package.json", "web/package.json"] {
        let run = changed(
            &[
                ("pytest.ini", "[pytest]\n"),
                (manifest, r#"{"name": "app"}"#),
                ("web/src/App.test.js", JS_3),
            ],
            |repo| repo.write("web/src/App.test.js", JS_1),
        );
        assert_eq!(
            run.titles("test-floor"),
            vec![BELOW_FLOOR],
            "{manifest}: {}",
            run.stdout
        );
        assert_eq!(
            run.outcome("test-floor")["examined"],
            3,
            "{manifest}: {}",
            run.stdout
        );
    }
}

/// Control: a manifest added by the change is read on the head side only, so deleting
/// the manifest together with the tests is still a drop.
#[test]
fn removing_the_package_manifest_with_the_tests_is_still_a_count_drop() {
    let run = changed(
        &[
            ("package.json", r#"{"name": "app"}"#),
            ("src/App.test.js", JS_3),
        ],
        |repo| {
            repo.remove("package.json");
            repo.remove("src/App.test.js");
        },
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}
