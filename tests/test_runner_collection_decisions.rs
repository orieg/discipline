//! `test-floor` runner collection: the shapes #593 lists. Each case is a configuration
//! or a file layout the model read differently from the runner, or a decision on how a
//! file no default invocation runs is counted.
//!
//! Every rule that takes a file out of the count comes as a pair: the rule on both sides
//! of the change (clean), and the rule added by the change (the count drops, or the
//! head side carries a note the base side does not).
//!
//! `Repo::new()` already holds `tests/a.rs` with two tests and `src/lib.rs` with none,
//! and no `Cargo.toml`, so every count includes 2.

mod common;
use common::Repo;

const BELOW_FLOOR: &str = "Test Count Below Floor";
const MOVED_OUT: &str = "Tests Moved Out Of Default Run";

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
const GO_MOD: &str = "module example.test/m\n\ngo 1.21\n";
const GO_MOD_NESTED: &str = "module example.test/m/sub\n\ngo 1.21\n";

const PY_3: &str = "\
def test_one():
    assert one() == 1


def test_two():
    assert two() == 2


def test_three():
    assert three() == 3
";
const PY_1: &str = "def test_one():\n    assert one() == 1\n";

/// One function pytest's default `python_functions` collects, two a `check_*` override
/// collects.
const PY_CHECKS: &str = "\
def test_one():
    assert one() == 1


def check_two():
    assert two() == 2


def check_three():
    assert three() == 3
";

/// A class pytest's default `python_classes` collects, and one a `Suite*` override does.
const PY_CLASSES: &str = "\
class TestDefault:
    def test_a(self):
        assert a() == 1


class SuiteCustom:
    def test_b(self):
        assert b() == 2

    def test_c(self):
        assert c() == 3
";

const RB_1: &str =
    "require 'minitest/autorun'\n\nclass ATest < Minitest::Test\n  def test_one\n    assert_equal 1, one\n  end\nend\n";

const PACKAGE: &str = "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const WORKSPACE_MEMBERS: &str = "[workspace]\nmembers = [\"member\", \"out\"]\n";
const WORKSPACE_EXCLUDE: &str = "[workspace]\nmembers = [\"member\"]\nexclude = [\"out\"]\n";

const PLAIN_PACKAGE_JSON: &str = r#"{"name": "app"}"#;
const JEST_DEFAULTS: &str = r#"{"name": "app", "jest": {}}"#;
const JEST_29: &str = r#"{"name": "app", "jest": {}, "devDependencies": {"jest": "^29.7.0"}}"#;
const JEST_30: &str = r#"{"name": "app", "jest": {}, "devDependencies": {"jest": "~30.0.2"}}"#;
const JEST_ANY: &str = r#"{"name": "app", "jest": {}, "devDependencies": {"jest": "*"}}"#;
const JEST_SCRIPT_CONFIG: &str =
    r#"{"name": "app", "scripts": {"test": "jest --ci --config config/jest.json"}}"#;
const JEST_SCRIPT_SHORT_CONFIG: &str =
    r#"{"name": "app", "scripts": {"test": "npx jest -c=jest.unit.json"}}"#;
const JEST_SCRIPT_ROOT_DIR: &str =
    r#"{"name": "app", "jest": {}, "scripts": {"test": "jest --rootDir web"}}"#;
const JEST_SCRIPT_SCRIPT_CONFIG: &str =
    r#"{"name": "app", "scripts": {"test": "jest --config jest.unit.config.js"}}"#;
const JEST_SCRIPT_COMPOUND_CONFIG: &str =
    r#"{"name": "app", "scripts": {"test": "tsc -p . && jest --config config/jest.json"}}"#;
const JEST_SCRIPT_COMPOUND_PLAIN: &str = r#"{"name": "app", "jest": {"testMatch": ["**/checks/**/*.js"]}, "scripts": {"test": "tsc -p . && jest"}}"#;
const JEST_CONFIG_IN_DIR: &str = r#"{"rootDir": "..", "testMatch": ["**/checks/**/*.js"]}"#;
const JEST_CONFIG_AT_ROOT: &str = r#"{"testMatch": ["**/checks/**/*.js"]}"#;

const VITEST_INCLUDE: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['checks/**/*.test.ts'],
  },
});
";
const VITEST_EXCLUDE: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    exclude: ['**/node_modules/**', '**/legacy/**'],
  },
});
";
const VITEST_NO_EXCLUDE: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
  },
});
";
const VITEST_SPREAD: &str = "\
import { configDefaults, defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    exclude: [...configDefaults.exclude, '**/legacy/**'],
  },
});
";
const VITEST_FUNCTION: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig(({ mode }) => ({
  test: { include: [mode === 'ci' ? 'checks/**' : 'src/**'] },
}));
";
const VITE_WITH_TEST_BLOCK: &str = "\
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [],
  test: {
    include: ['checks/**/*.test.ts'],
  },
});
";
const VITE_WITHOUT_TEST_BLOCK: &str = "\
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [],
});
";
const VITEST_KEY_IN_PACKAGE: &str = r#"{"name": "app", "vitest": {"include": ["checks/**"]}}"#;

const CONFTEST_LITERAL: &str =
    "collect_ignore = [\"test_parked.py\", \"deep\"]\ncollect_ignore_glob = [\"*_skip.py\"]\n";
const CONFTEST_EMPTY: &str = "collect_ignore = []\n";
const CONFTEST_DYNAMIC: &str =
    "import sys\n\ncollect_ignore = []\nif sys.version_info < (3, 12):\n    collect_ignore.append(\"test_parked.py\")\n";

const LIB_DECLARES_USED: &str = "pub mod used;\n";
const LIB_DECLARES_BOTH: &str = "pub mod used;\npub mod extra;\n";
const LIB_WITH_MACRO: &str = "macro_rules! declare {\n    ($name:ident) => {\n        mod $name;\n    };\n}\n\ndeclare!(used);\n";
const LIB_WITH_PATH_AND_INLINE: &str =
    "#[path = \"elsewhere/moved.rs\"]\nmod moved;\n\nmod outer {\n    mod inner;\n}\n";
const MAIN_DECLARES_BINPART: &str = "mod binpart;\n\nfn main() {}\n";
const LIB_DECLARES_LIBPART: &str = "pub mod libpart;\n";

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

/// A note of `side` that holds every one of `parts`.
fn has_note(run: &common::Run, side: &str, parts: &[&str]) -> bool {
    notes(run)
        .iter()
        .any(|n| n.starts_with(side) && parts.iter().all(|p| n.contains(p)))
}

fn examined(run: &common::Run) -> u64 {
    run.outcome("test-floor")["examined"].as_u64().unwrap()
}

/// The one `Tests Moved Out Of Default Run` finding of a run, which must be its only
/// `test-floor` finding and be reported on `file`.
fn assert_moved_out_on(run: &common::Run, file: &str) {
    assert_eq!(run.titles("test-floor"), vec![MOVED_OUT], "{}", run.stdout);
    let finding = &run.violations("test-floor")[0];
    assert_eq!(
        finding["code"], "test-floor/tests-moved-out-of-default-run",
        "{finding}"
    );
    assert_eq!(finding["file"], file, "{finding}");
}

// ------------------------------------------------- 1. Cargo workspace `exclude`

#[test]
fn a_package_the_workspace_excludes_counts_with_a_note_naming_it() {
    let (n, run) = counted(&[
        ("Cargo.toml", WORKSPACE_EXCLUDE),
        ("member/Cargo.toml", PACKAGE),
        ("member/tests/it.rs", RS_1),
        ("out/Cargo.toml", PACKAGE),
        ("out/tests/it.rs", RS_3),
    ]);
    assert_eq!(n, 6, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: runner collection unknown",
            &["`out`", "`exclude`", "1 file(s)"]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: a member is collected, with no note.
    let (n, run) = counted(&[
        ("Cargo.toml", WORKSPACE_MEMBERS),
        ("member/Cargo.toml", PACKAGE),
        ("member/tests/it.rs", RS_1),
        ("out/Cargo.toml", PACKAGE),
        ("out/tests/it.rs", RS_3),
    ]);
    assert_eq!(n, 6, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn adding_a_package_to_the_workspace_exclude_is_reported_and_noted_on_the_head_side() {
    let run = changed(
        &[
            ("Cargo.toml", WORKSPACE_MEMBERS),
            ("member/Cargo.toml", PACKAGE),
            ("member/tests/it.rs", RS_1),
            ("out/Cargo.toml", PACKAGE),
            ("out/tests/it.rs", RS_3),
        ],
        |repo| repo.write("Cargo.toml", WORKSPACE_EXCLUDE),
    );
    // The count does not move: the excluded package is still counted. The finding on
    // the workspace manifest is what reports the change.
    assert_moved_out_on(&run, "Cargo.toml");
    assert_eq!(examined(&run), 6, "{}", run.stdout);
    assert!(
        has_note(&run, "head: ", &["`out`", "`exclude`"]),
        "{:?}",
        notes(&run)
    );
    assert!(
        !has_note(&run, "base: ", &["`exclude`"]),
        "{:?}",
        notes(&run)
    );
    // The file whose collection the change made undetermined is named.
    assert!(
        has_note(&run, "head: ", &["`out/tests/it.rs`", "base side"]),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------------------- 2. Go build constraints and modules

fn go_with(header: &str) -> String {
    format!("{header}{GO_3}")
}

#[test]
fn a_go_test_file_no_build_selects_is_not_counted() {
    for header in [
        "//go:build ignore\n\n",
        "// +build ignore\n\n",
        "//go:build linux && !linux\n\n",
        "// Copyright the authors.\n\n//go:build ignore\n\n",
    ] {
        let (n, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &go_with(header))]);
        assert_eq!(n, 2, "{header:?}: {}", run.stdout);
    }
    // Control: no constraint.
    let (n, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
}

/// Control: a constraint the platform, the toolchain or the absence of a tag satisfies
/// is built by a default `go test` somewhere, and a comment that is not a constraint
/// (after the package clause, or a `+build` line with no blank line after it)
/// constrains nothing.
#[test]
fn a_go_test_file_a_default_build_selects_is_counted_without_a_note() {
    for header in [
        "//go:build linux || darwin || windows\n\n",
        "//go:build cgo && (amd64 || arm64)\n\n",
        "//go:build go1.21\n\n",
        "//go:build unix\n\n",
        "//go:build !integration\n\n",
        "// +build linux darwin\n\n",
        "// +build integration\n",
    ] {
        let (n, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &go_with(header))]);
        assert_eq!(n, 5, "{header:?}: {}", run.stdout);
        assert!(!has_unknown_note(&run), "{header:?}: {:?}", notes(&run));
    }
    let after_package = format!("{GO_3}\n//go:build ignore\n");
    let (n, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &after_package)]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn a_go_test_file_behind_a_custom_tag_counts_with_a_note_naming_the_tag() {
    for (header, tag) in [
        ("//go:build integration\n\n", "`integration`"),
        ("// +build e2e\n\n", "`e2e`"),
        ("//go:build linux && slow\n\n", "`slow`"),
    ] {
        let (n, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &go_with(header))]);
        assert_eq!(n, 5, "{header:?}: {}", run.stdout);
        assert!(
            has_note(&run, "head: runner collection unknown", &[tag, "-tags"]),
            "{header:?}: {:?}",
            notes(&run)
        );
    }
}

#[test]
fn adding_a_custom_build_tag_to_a_go_test_file_is_reported_and_names_the_file_and_the_tag() {
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write("pkg/a_test.go", &go_with("//go:build integration\n\n"))
    });
    assert_moved_out_on(&run, "pkg/a_test.go");
    assert_eq!(examined(&run), 5, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: ",
            &["`pkg/a_test.go`", "`integration`", "base side"]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: the tag on both sides is the same note on both, and no transition.
    let tagged = go_with("//go:build integration\n\n");
    let (_, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &tagged)]);
    assert!(
        !has_note(&run, "head: ", &["base side"]),
        "{:?}",
        notes(&run)
    );
    assert!(
        has_note(&run, "base: ", &["`integration`"]),
        "{:?}",
        notes(&run)
    );
}

#[test]
fn adding_build_ignore_to_a_go_test_file_is_a_count_drop() {
    let run = changed(&[("go.mod", GO_MOD), ("pkg/a_test.go", GO_3)], |repo| {
        repo.write("pkg/a_test.go", &go_with("//go:build ignore\n\n"))
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    // On both sides it was never counted: clean.
    let ignored = go_with("//go:build ignore\n\n");
    let (n, run) = counted(&[("go.mod", GO_MOD), ("pkg/a_test.go", &ignored)]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
}

#[test]
fn a_nested_go_module_counts_with_a_note_naming_it() {
    let (n, run) = counted(&[
        ("go.mod", GO_MOD),
        ("pkg/a_test.go", GO_3),
        ("sub/go.mod", GO_MOD_NESTED),
        ("sub/pkg/b_test.go", GO_3),
    ]);
    assert_eq!(n, 8, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: runner collection unknown",
            &["`sub`", "./...", "1 file(s)"]
        ),
        "{:?}",
        notes(&run)
    );
    // Control: a module that is below no other module is the default invocation's own.
    let (n, run) = counted(&[("sub/go.mod", GO_MOD_NESTED), ("sub/pkg/b_test.go", GO_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn turning_a_go_directory_into_a_nested_module_is_reported_and_noted_on_the_head_side() {
    let run = changed(&[("go.mod", GO_MOD), ("sub/pkg/b_test.go", GO_3)], |repo| {
        repo.write("sub/go.mod", GO_MOD_NESTED)
    });
    assert_moved_out_on(&run, "sub/go.mod");
    assert_eq!(examined(&run), 5, "{}", run.stdout);
    assert!(
        has_note(&run, "head: ", &["`sub/pkg/b_test.go`", "base side"]),
        "{:?}",
        notes(&run)
    );
}

// ------------------------------------------------- 3. `.gitattributes`

#[test]
fn a_vendored_or_generated_test_file_is_left_out_with_a_note() {
    for (attributes, attribute) in [
        ("third_party/** linguist-vendored\n", "`linguist-vendored`"),
        (
            "third_party/** linguist-generated=true\n",
            "`linguist-generated`",
        ),
    ] {
        let (n, run) = counted(&[
            ("package.json", PLAIN_PACKAGE_JSON),
            (".gitattributes", attributes),
            ("third_party/lib/a.test.js", JS_3),
            ("src/b.test.js", JS_1),
        ]);
        assert_eq!(n, 3, "{attributes:?}: {}", run.stdout);
        assert!(
            has_note(
                &run,
                "head: ",
                &[attribute, "`.gitattributes`", "not counted", "1 file(s)"]
            ),
            "{attributes:?}: {:?}",
            notes(&run)
        );
    }
    // Any language: a Go file, and a nested attributes file.
    let (n, run) = counted(&[
        ("go.mod", GO_MOD),
        ("gen/.gitattributes", "*_test.go linguist-generated\n"),
        ("gen/pkg/a_test.go", GO_3),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(
        has_note(&run, "head: ", &["`gen/.gitattributes`", "not counted"]),
        "{:?}",
        notes(&run)
    );
}

/// Control: an attribute that is unset, false, or reset by a nearer file excludes
/// nothing, and neither does a pattern that names only the directory.
#[test]
fn an_attribute_that_is_not_set_leaves_the_file_counted() {
    for (root, nested) in [
        ("third_party/** -linguist-vendored\n", None),
        ("third_party/** linguist-vendored=false\n", None),
        ("third_party/ linguist-vendored\n", None),
        ("third_party linguist-vendored\n", None),
        (
            "third_party/** linguist-vendored\n",
            Some("*.test.js -linguist-vendored\n"),
        ),
        (
            "third_party/** linguist-vendored\nthird_party/lib/** !linguist-vendored\n",
            None,
        ),
    ] {
        let mut files = vec![
            ("package.json", PLAIN_PACKAGE_JSON),
            (".gitattributes", root),
            ("third_party/lib/a.test.js", JS_3),
        ];
        if let Some(nested) = nested {
            files.push(("third_party/lib/.gitattributes", nested));
        }
        let (n, run) = counted(&files);
        assert_eq!(n, 5, "{root:?} {nested:?}: {}", run.stdout);
    }
}

#[test]
fn adding_a_vendored_attribute_over_existing_tests_is_a_count_drop() {
    let base = [
        ("package.json", PLAIN_PACKAGE_JSON),
        ("lib/a.test.js", JS_3),
    ];
    let run = changed(&base, |repo| {
        repo.write(".gitattributes", "lib/** linguist-vendored\n")
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    // Widening a pattern that held nothing is the same drop.
    let narrow = [
        ("package.json", PLAIN_PACKAGE_JSON),
        (".gitattributes", "lib/dist/** linguist-generated\n"),
        ("lib/a.test.js", JS_3),
    ];
    let run = changed(&narrow, |repo| {
        repo.write(".gitattributes", "lib/** linguist-generated\n")
    });
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    // On both sides the files were never counted: clean.
    let (n, run) = counted(&[
        ("package.json", PLAIN_PACKAGE_JSON),
        (".gitattributes", "lib/** linguist-vendored\n"),
        ("lib/a.test.js", JS_3),
    ]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
}

/// A path named in `[tests] paths` always counts, whatever its attributes.
#[test]
fn a_declared_test_path_counts_whatever_its_attributes() {
    let (n, run) = counted(&[
        ("package.json", PLAIN_PACKAGE_JSON),
        (".gitattributes", "lib/** linguist-vendored\n"),
        ("discipline.toml", "[tests]\npaths = [\"lib/**\"]\n"),
        ("lib/a.test.js", JS_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

/// One attributes layout, and the paths the count must leave out according to
/// `git check-attr` itself.
struct AttributeCase {
    name: &'static str,
    attributes: &'static [(&'static str, &'static str)],
    paths: &'static [&'static str],
}

const ATTRIBUTE_CASES: &[AttributeCase] = &[
    AttributeCase {
        name: "a directory pattern and its recursive form",
        attributes: &[(".gitattributes", "vendor/ linguist-vendored\nonly.test.js/ linguist-vendored\nthird/** linguist-vendored\nflat/* linguist-vendored\n")],
        paths: &[
            "vendor/a.test.js",
            "x/only.test.js",
            "third/a.test.js",
            "third/deep/er/a.test.js",
            "flat/a.test.js",
            "flat/deep/a.test.js",
        ],
    },
    AttributeCase {
        name: "double-star patterns",
        attributes: &[(".gitattributes", "**/gen/** linguist-generated\na/**/z.test.js linguist-vendored\n**/top.test.js linguist-vendored\nb/**c/d.test.js linguist-vendored\n")],
        paths: &[
            "gen/a.test.js",
            "x/gen/a.test.js",
            "x/y/gen/deep/a.test.js",
            "x/generated/a.test.js",
            "a/z.test.js",
            "a/b/c/z.test.js",
            "top.test.js",
            "k/top.test.js",
            "b/c/d.test.js",
            "b/xc/d.test.js",
            "b/x/c/d.test.js",
        ],
    },
    AttributeCase {
        name: "a nested file overrides the root",
        attributes: &[
            (".gitattributes", "*.test.js linguist-vendored\n"),
            ("keep/.gitattributes", "*.test.js -linguist-vendored\nagain/*.test.js linguist-vendored\n"),
            ("keep/reset/.gitattributes", "*.test.js !linguist-vendored\n"),
        ],
        paths: &[
            "a.test.js",
            "other/a.test.js",
            "keep/a.test.js",
            "keep/deep/a.test.js",
            "keep/again/a.test.js",
            "keep/reset/a.test.js",
        ],
    },
    AttributeCase {
        name: "unset, values, and the later line winning",
        attributes: &[(".gitattributes", "u/** linguist-vendored\nu/** -linguist-vendored\nt/** linguist-vendored=true\nf/** linguist-vendored=false\ny/** linguist-vendored=yes\nw/** -linguist-vendored\nw/** linguist-vendored\nw/keep.test.js -linguist-vendored\n")],
        paths: &[
            "u/a.test.js",
            "t/a.test.js",
            "f/a.test.js",
            "y/a.test.js",
            "w/a.test.js",
            "w/keep.test.js",
        ],
    },
    AttributeCase {
        name: "a pattern with no slash matches at any depth, an anchored one does not",
        attributes: &[(".gitattributes", "bundle.test.js linguist-generated\n*.min.test.js linguist-generated\n/root.test.js linguist-generated\ndir/only.test.js linguist-generated\n")],
        paths: &[
            "bundle.test.js",
            "a/b/bundle.test.js",
            "a/x.min.test.js",
            "root.test.js",
            "a/root.test.js",
            "dir/only.test.js",
            "a/dir/only.test.js",
        ],
    },
    AttributeCase {
        name: "wildcards, classes, escapes, quotes, comments and negative patterns",
        attributes: &[(".gitattributes", "# q/** linguist-vendored\nq/?.test.js linguist-vendored\nc/[ab]*.test.js linguist-vendored\nn/[!a]*.test.js linguist-vendored\ne/\\*.test.js linguist-vendored\n\"s p/**\" linguist-vendored\n!neg/** linguist-vendored\nm/**\tlinguist-generated linguist-vendored\n")],
        paths: &[
            "q/a.test.js",
            "q/ab.test.js",
            "c/a1.test.js",
            "c/c1.test.js",
            "n/a1.test.js",
            "n/b1.test.js",
            "e/a.test.js",
            "s p/a.test.js",
            "neg/a.test.js",
            "m/a.test.js",
        ],
    },
];

/// The matcher is checked against git: for every path, `git check-attr` says whether
/// either attribute is set, and the count leaves out exactly those files.
#[test]
fn the_attribute_matcher_agrees_with_git_check_attr() {
    for case in ATTRIBUTE_CASES {
        let repo = Repo::new();
        let mut files: Vec<(&str, &str)> = vec![("package.json", PLAIN_PACKAGE_JSON)];
        files.extend(case.attributes.iter().copied());
        files.extend(case.paths.iter().map(|p| (*p, JS_1)));
        repo.commit_base_files(&files, "test: base suite");
        repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 3.\n");
        repo.commit("docs: reorder the plan");

        let mut expected = 2;
        let mut left_out = Vec::new();
        for path in case.paths {
            let set = ["linguist-vendored", "linguist-generated"]
                .iter()
                .any(|attribute| {
                    let out = repo.git_output(&["check-attr", attribute, "--", path]);
                    out.ends_with(": set") || out.ends_with(": true")
                });
            if set {
                left_out.push(*path);
            } else {
                expected += 1;
            }
        }
        // The case discriminates: git sets the attribute on some paths and not on others.
        assert!(
            !left_out.is_empty() && left_out.len() < case.paths.len(),
            "{}: {left_out:?}",
            case.name
        );
        let run = repo.check(&[]);
        assert_eq!(
            examined(&run),
            expected,
            "{}: git leaves out {left_out:?}\n{}",
            case.name,
            run.stdout
        );
    }
}

// ------------------------------------------------- 4. The no-manifest rule is JavaScript only

/// Control: Python, Ruby and C have no required manifest, so their tests count with
/// none in the repository.
#[test]
fn python_ruby_and_c_with_no_manifest_still_count() {
    let (n, run) = counted(&[("checks/test_a.py", PY_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    let (n, run) = counted(&[("test/a_test.rb", RB_1)]);
    assert_eq!(n, 3, "{}", run.stdout);
}

// ------------------------------------------------- 5. Manifest-less JavaScript suites

#[test]
fn a_bun_or_deno_configuration_is_a_runner_configuration() {
    for manifest in [
        "bunfig.toml",
        "tools/bunfig.toml",
        "deno.json",
        "deno.jsonc",
    ] {
        let (n, run) = counted(&[(manifest, ""), ("checks/a.test.ts", JS_3)]);
        assert_eq!(n, 5, "{manifest}: {}", run.stdout);
        assert!(has_unknown_note(&run), "{manifest}: {:?}", notes(&run));
    }
}

#[test]
fn a_bare_node_test_suite_is_left_out_with_a_note_saying_how_to_declare_it() {
    let (n, run) = counted(&[("test/a.test.mjs", JS_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: ",
            &["not counted", "1 file(s)", "`[tests] paths`"]
        ),
        "{:?}",
        notes(&run)
    );
    // Declared, it counts.
    let (n, run) = counted(&[
        ("discipline.toml", "[tests]\npaths = [\"test/**\"]\n"),
        ("test/a.test.mjs", JS_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

// ------------------------------------------------- 6. pytest

#[test]
fn pytest_python_functions_names_the_tests() {
    let cfg = "[pytest]\npython_functions = check_*\n";
    let (n, run) = counted(&[("pytest.ini", cfg), ("tests/test_e.py", PY_CHECKS)]);
    assert_eq!(n, 4, "{}", run.stdout);
    // A bare prefix is a prefix.
    let cfg = "[pytest]\npython_functions = check\n";
    let (n, run) = counted(&[("pytest.ini", cfg), ("tests/test_e.py", PY_CHECKS)]);
    assert_eq!(n, 4, "{}", run.stdout);
    // Control: unset, pytest's default names apply.
    let (n, run) = counted(&[("pytest.ini", "[pytest]\n"), ("tests/test_e.py", PY_CHECKS)]);
    assert_eq!(n, 3, "{}", run.stdout);
}

#[test]
fn pytest_python_classes_names_the_test_classes() {
    let cfg = "[pytest]\npython_files = spec_*.py\npython_classes = Suite*\n";
    let (n, run) = counted(&[("pytest.ini", cfg), ("checks/spec_b.py", PY_CLASSES)]);
    assert_eq!(n, 4, "{}", run.stdout);
    // Control: unset, a class named `Test*` is the one collected.
    let cfg = "[pytest]\npython_files = spec_*.py\n";
    let (n, run) = counted(&[("pytest.ini", cfg), ("checks/spec_b.py", PY_CLASSES)]);
    assert_eq!(n, 3, "{}", run.stdout);
}

#[test]
fn renaming_what_pytest_collects_away_from_the_tests_is_a_count_drop() {
    let run = changed(
        &[("pytest.ini", "[pytest]\n"), ("tests/test_a.py", PY_3)],
        |repo| repo.write("pytest.ini", "[pytest]\npython_functions = check_*\n"),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn pytest_norecursedirs_leaves_a_directory_out() {
    // The default list.
    for dir in [
        "build",
        "dist",
        "venv",
        "node_modules",
        ".tox",
        "pkg.egg",
        "a/build",
    ] {
        let path = format!("{dir}/test_b.py");
        let (n, run) = counted(&[("pytest.ini", "[pytest]\n"), (&path, PY_3)]);
        assert_eq!(n, 2, "{dir}: {}", run.stdout);
    }
    // A configured list replaces the default one.
    let cfg = "[pytest]\nnorecursedirs = parked old_*\n";
    for (path, expected) in [
        ("parked/test_b.py", 2),
        ("a/old_suite/test_b.py", 2),
        ("build/test_b.py", 5),
        ("checks/test_b.py", 5),
    ] {
        let (n, run) = counted(&[("pytest.ini", cfg), (path, PY_3)]);
        assert_eq!(n, expected, "{path}: {}", run.stdout);
    }
    // A directory named in `testpaths` is where collection starts: it is not checked.
    let cfg = "[pytest]\ntestpaths = build\n";
    let (n, run) = counted(&[("pytest.ini", cfg), ("build/test_b.py", PY_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
    // Control: with no pytest configuration the runner is not known to be pytest.
    let (n, run) = counted(&[("build/test_b.py", PY_3)]);
    assert_eq!(n, 5, "{}", run.stdout);
}

#[test]
fn moving_a_test_under_a_directory_pytest_does_not_recurse_into_is_a_count_drop() {
    let run = changed(
        &[("pytest.ini", "[pytest]\n"), ("checks/test_a.py", PY_3)],
        |repo| {
            repo.remove("checks/test_a.py");
            repo.write("build/test_a.py", PY_3);
        },
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    let run = changed(
        &[("pytest.ini", "[pytest]\n"), ("checks/test_a.py", PY_3)],
        |repo| repo.write("pytest.ini", "[pytest]\nnorecursedirs = checks\n"),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn a_conftest_collect_ignore_list_leaves_its_entries_out() {
    let files = [
        ("pytest.ini", "[pytest]\n"),
        ("checks/conftest.py", CONFTEST_LITERAL),
        ("checks/test_kept.py", PY_1),
        ("checks/test_parked.py", PY_3),
        ("checks/deep/test_h.py", PY_3),
        ("checks/more/test_z_skip.py", PY_3),
        ("other/test_parked.py", PY_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 4, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // A list that is built at run time is not read: counted, with a note.
    let files = [
        ("pytest.ini", "[pytest]\n"),
        ("checks/conftest.py", CONFTEST_DYNAMIC),
        ("checks/test_parked.py", PY_3),
        ("other/test_o.py", PY_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 6, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: runner collection unknown",
            &["conftest.py", "1 file(s)"]
        ),
        "{:?}",
        notes(&run)
    );
}

#[test]
fn adding_a_test_file_to_collect_ignore_is_a_count_drop() {
    let run = changed(
        &[
            ("pytest.ini", "[pytest]\n"),
            ("checks/conftest.py", CONFTEST_EMPTY),
            ("checks/test_parked.py", PY_3),
        ],
        |repo| repo.write("checks/conftest.py", CONFTEST_LITERAL),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

/// pytest refuses a `[pytest]` section in `setup.cfg`, and reads `[tool:pytest]` there.
#[test]
fn a_pytest_section_in_setup_cfg_is_not_a_pytest_configuration() {
    let files = [
        ("setup.cfg", "[pytest]\ntestpaths = other\n"),
        ("tests/test_a.py", PY_3),
        ("other/test_o.py", PY_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 6, "{}", run.stdout);
    // Control: the section pytest does read there.
    let files = [
        ("setup.cfg", "[tool:pytest]\ntestpaths = other\n"),
        ("tests/test_a.py", PY_3),
        ("other/test_o.py", PY_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 3, "{}", run.stdout);
    // Control: `tox.ini` is read with a `[pytest]` section.
    let files = [
        ("tox.ini", "[pytest]\ntestpaths = other\n"),
        ("tests/test_a.py", PY_3),
        ("other/test_o.py", PY_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 3, "{}", run.stdout);
}

#[test]
fn a_hidden_pytest_ini_is_read_after_pytest_ini() {
    let hidden = "[pytest]\npython_files = spec_*.py\n";
    let (n, run) = counted(&[(".pytest.ini", hidden), ("checks/test_a.py", PY_3)]);
    assert_eq!(n, 2, "{}", run.stdout);
    // `pytest.ini` shadows it.
    let (n, run) = counted(&[
        (".pytest.ini", hidden),
        ("pytest.ini", "[pytest]\n"),
        ("checks/test_a.py", PY_3),
    ]);
    assert_eq!(n, 5, "{}", run.stdout);
}

// ------------------------------------------------- 7. Jest

#[test]
fn a_jest_configuration_named_by_the_test_script_is_the_one_read() {
    let files = [
        ("package.json", JEST_SCRIPT_CONFIG),
        ("config/jest.json", JEST_CONFIG_IN_DIR),
        ("checks/a.js", JS_3),
        ("src/b.test.js", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // `-c`, and `rootDir` defaulting to the directory of the configuration file.
    let files = [
        ("package.json", JEST_SCRIPT_SHORT_CONFIG),
        ("jest.unit.json", JEST_CONFIG_AT_ROOT),
        ("checks/a.js", JS_3),
        ("src/b.test.js", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_jest_root_dir_given_by_the_test_script_limits_collection() {
    let files = [
        ("package.json", JEST_SCRIPT_ROOT_DIR),
        ("web/a.test.js", JS_3),
        ("tools/b.test.js", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_jest_script_that_cannot_be_read_leaves_collection_undetermined() {
    for package in [JEST_SCRIPT_SCRIPT_CONFIG, JEST_SCRIPT_COMPOUND_CONFIG] {
        let files = [
            ("package.json", package),
            ("config/jest.json", JEST_CONFIG_IN_DIR),
            ("jest.unit.config.js", "module.exports = {};\n"),
            ("src/b.test.js", JS_3),
        ];
        let (n, run) = counted(&files);
        assert_eq!(n, 5, "{package}: {}", run.stdout);
        assert!(has_unknown_note(&run), "{package}: {:?}", notes(&run));
    }
    // Control: a compound script that passes Jest no configuration flag changes nothing.
    let files = [
        ("package.json", JEST_SCRIPT_COMPOUND_PLAIN),
        ("checks/a.js", JS_3),
        ("src/b.test.js", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn moving_a_test_out_of_the_configuration_the_script_names_is_a_count_drop() {
    let run = changed(
        &[
            ("package.json", JEST_SCRIPT_CONFIG),
            ("config/jest.json", JEST_CONFIG_IN_DIR),
            ("checks/a.js", JS_3),
        ],
        |repo| {
            repo.remove("checks/a.js");
            repo.write("parked/a.js", JS_3);
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
fn jest_default_patterns_for_module_extensions_follow_the_jest_major() {
    for ext in ["mjs", "cjs", "mts", "cts"] {
        let path = format!("src/a.test.{ext}");
        // Jest 29 and earlier: the default patterns do not match the extension.
        let (n, run) = counted(&[("package.json", JEST_29), (&path, JS_3)]);
        assert_eq!(n, 2, "{ext}: {}", run.stdout);
        // Jest 30: they do.
        let (n, run) = counted(&[("package.json", JEST_30), (&path, JS_3)]);
        assert_eq!(n, 5, "{ext}: {}", run.stdout);
        assert!(!has_unknown_note(&run), "{ext}: {:?}", notes(&run));
        // A version that is not a plain range, or no version: counted, with a note.
        for package in [JEST_ANY, JEST_DEFAULTS] {
            let (n, run) = counted(&[("package.json", package), (&path, JS_3)]);
            assert_eq!(n, 5, "{ext} {package}: {}", run.stdout);
            assert!(
                has_note(&run, "head: runner collection unknown", &["version"]),
                "{ext} {package}: {:?}",
                notes(&run)
            );
        }
    }
    // Control: the other extensions do not depend on the version.
    for package in [JEST_29, JEST_30, JEST_ANY, JEST_DEFAULTS] {
        let (n, run) = counted(&[("package.json", package), ("src/a.test.js", JS_3)]);
        assert_eq!(n, 5, "{package}: {}", run.stdout);
        assert!(!has_unknown_note(&run), "{package}: {:?}", notes(&run));
    }
}

#[test]
fn renaming_a_test_to_an_extension_jest_29_does_not_match_is_a_count_drop() {
    let run = changed(
        &[("package.json", JEST_29), ("src/a.test.js", JS_3)],
        |repo| {
            repo.remove("src/a.test.js");
            repo.write("src/a.test.mjs", JS_3);
        },
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

// ------------------------------------------------- 8. Vitest

#[test]
fn a_literal_vitest_configuration_is_read() {
    for config in ["vitest.config.ts", "vitest.config.mjs"] {
        let files = [
            ("package.json", PLAIN_PACKAGE_JSON),
            (config, VITEST_INCLUDE),
            ("checks/a.test.ts", JS_3),
            ("src/b.test.ts", JS_3),
        ];
        let (n, run) = counted(&files);
        assert_eq!(n, 5, "{config}: {}", run.stdout);
        assert!(!has_unknown_note(&run), "{config}: {:?}", notes(&run));
    }
    let files = [
        ("package.json", PLAIN_PACKAGE_JSON),
        ("vitest.config.ts", VITEST_EXCLUDE),
        ("src/a.test.ts", JS_3),
        ("src/legacy/b.test.ts", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn a_vite_configuration_with_a_test_block_is_a_vitest_configuration() {
    let files = [
        ("package.json", PLAIN_PACKAGE_JSON),
        ("vite.config.ts", VITE_WITH_TEST_BLOCK),
        ("checks/a.test.ts", JS_3),
        ("src/b.test.ts", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // Control: with no `test` block it configures no test runner.
    let files = [
        ("package.json", PLAIN_PACKAGE_JSON),
        ("vite.config.ts", VITE_WITHOUT_TEST_BLOCK),
        ("src/b.test.ts", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: runner collection unknown",
            &["no runner config found"]
        ),
        "{:?}",
        notes(&run)
    );
}

/// Control: a configuration that is computed is not read.
#[test]
fn a_vitest_configuration_that_is_not_a_literal_leaves_collection_undetermined() {
    for config in [VITEST_SPREAD, VITEST_FUNCTION] {
        let files = [
            ("package.json", PLAIN_PACKAGE_JSON),
            ("vitest.config.ts", config),
            ("src/legacy/b.test.ts", JS_3),
        ];
        let (n, run) = counted(&files);
        assert_eq!(n, 5, "{config}: {}", run.stdout);
        assert!(has_unknown_note(&run), "{config}: {:?}", notes(&run));
    }
}

/// Vitest reads no configuration from `package.json`.
#[test]
fn a_vitest_key_in_package_json_is_not_a_configuration() {
    let files = [
        ("package.json", VITEST_KEY_IN_PACKAGE),
        ("src/b.test.js", JS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(
        has_note(
            &run,
            "head: runner collection unknown",
            &["no runner config found"]
        ),
        "{:?}",
        notes(&run)
    );
}

#[test]
fn adding_a_vitest_exclude_over_existing_tests_is_a_count_drop() {
    let run = changed(
        &[
            ("package.json", PLAIN_PACKAGE_JSON),
            ("vitest.config.ts", VITEST_NO_EXCLUDE),
            ("src/legacy/b.test.ts", JS_3),
        ],
        |repo| repo.write("vitest.config.ts", VITEST_EXCLUDE),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

// ------------------------------------------------- 9. Cargo: modules under `src/`

#[test]
fn a_file_under_src_that_no_crate_root_declares_is_not_counted() {
    let files = [
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", LIB_DECLARES_USED),
        ("src/used.rs", RS_1),
        ("src/orphan.rs", RS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 3, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // Control: declared, it counts.
    let files = [
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", LIB_DECLARES_BOTH),
        ("src/used.rs", RS_1),
        ("src/extra.rs", RS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 6, "{}", run.stdout);
}

#[test]
fn modules_of_a_crate_root_are_followed_as_cargo_builds_them() {
    // `#[path]`, a module nested in an inline one, and a binary under `src/bin/`.
    let files = [
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", LIB_WITH_PATH_AND_INLINE),
        ("src/elsewhere/moved.rs", RS_1),
        ("src/outer/inner.rs", RS_1),
        ("src/bin/tool.rs", MAIN_DECLARES_BINPART),
        ("src/bin/binpart.rs", RS_1),
        ("src/bin/unused/helper.rs", RS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // A macro that may expand to `mod`: what is not reached counts, with a note.
    let files = [
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", LIB_WITH_MACRO),
        ("src/used.rs", RS_3),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 5, "{}", run.stdout);
    assert!(has_unknown_note(&run), "{:?}", notes(&run));
}

#[test]
fn dropping_the_mod_line_of_a_source_module_is_a_count_drop() {
    let run = changed(
        &[
            ("Cargo.toml", PACKAGE),
            ("src/lib.rs", LIB_DECLARES_BOTH),
            ("src/used.rs", RS_1),
            ("src/extra.rs", RS_3),
        ],
        |repo| repo.write("src/lib.rs", LIB_DECLARES_USED),
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
}

#[test]
fn lib_test_false_beside_a_binary_leaves_out_the_library_and_keeps_the_binary() {
    let manifest = format!("{PACKAGE}\n[lib]\ntest = false\n");
    let files = [
        ("Cargo.toml", manifest.as_str()),
        ("src/lib.rs", LIB_DECLARES_LIBPART),
        ("src/libpart.rs", RS_3),
        ("src/main.rs", MAIN_DECLARES_BINPART),
        ("src/binpart.rs", RS_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 3, "{}", run.stdout);
    assert!(!has_unknown_note(&run), "{:?}", notes(&run));
    // A binary switched off as well runs nothing under `src/`.
    let both_off =
        format!("{manifest}\n[[bin]]\nname = \"pkg\"\npath = \"src/main.rs\"\ntest = false\n");
    let files = [
        ("Cargo.toml", both_off.as_str()),
        ("src/lib.rs", LIB_DECLARES_LIBPART),
        ("src/libpart.rs", RS_3),
        ("src/main.rs", MAIN_DECLARES_BINPART),
        ("src/binpart.rs", RS_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 2, "{}", run.stdout);
    // Control: the default runs both.
    let files = [
        ("Cargo.toml", PACKAGE),
        ("src/lib.rs", LIB_DECLARES_LIBPART),
        ("src/libpart.rs", RS_3),
        ("src/main.rs", MAIN_DECLARES_BINPART),
        ("src/binpart.rs", RS_1),
    ];
    let (n, run) = counted(&files);
    assert_eq!(n, 6, "{}", run.stdout);
}

/// Control: a package with no crate root tracked has nothing to follow modules from, so
/// the files under `src/` count as before.
#[test]
fn a_package_with_no_crate_root_counts_its_source_files() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.remove("src/lib.rs");
    repo.write("Cargo.toml", PACKAGE);
    repo.write("src/checks.rs", RS_3);
    repo.commit("test: base suite");
    repo.git(&["checkout", "-q", "-B", "work", "main"]);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 3.\n");
    repo.commit("docs: reorder the plan");
    let run = repo.check(&[]);
    assert_eq!(examined(&run), 5, "{}", run.stdout);
}
