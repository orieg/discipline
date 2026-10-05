//! `test-floor` leaves a file out of the static count only when a parsed runner
//! configuration excludes it (#521). When collection is not determined (no runner
//! configuration, or a language with no runner model) the tests the language pack finds
//! in the file count, on both sides, and a note per side says so.
//!
//! `Repo::new()` already holds `tests/a.rs` with two tests, so every count includes 2.

mod common;
use common::Repo;

const BELOW_FLOOR: &str = "Test Count Below Floor";

/// Commits `before` on the base side, `after` on the head side, and runs the gates.
fn reduce(setup: &[(&str, &str)], path: &str, before: &str, after: &str) -> common::Run {
    let repo = Repo::new();
    let mut files = setup.to_vec();
    files.push((path, before));
    repo.commit_base_files(&files, "test: base suite");
    repo.write(path, after);
    repo.commit("test: trim suite");
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

const TSX_3: &str = "\
it('renders', () => { expect(render()).toBe(1); });
it('clicks', () => { expect(click()).toBe(2); });
it('submits', () => { expect(submit()).toBe(3); });
";
const TSX_1: &str = "it('renders', () => { expect(render()).toBe(1); });\n";

const SWIFT_3: &str = "\
import XCTest

final class LoginTests: XCTestCase {
    func testLogin() {
        XCTAssertEqual(login(), 1)
    }

    func testLogout() {
        XCTAssertEqual(logout(), 2)
    }

    func testRefresh() {
        XCTAssertEqual(refresh(), 3)
    }
}
";
const SWIFT_1: &str = "\
import XCTest

final class LoginTests: XCTestCase {
    func testLogin() {
        XCTAssertEqual(login(), 1)
    }
}
";

const UNITTEST_3: &str = "\
import unittest


class QuestionTests(unittest.TestCase):
    def test_one(self):
        self.assertEqual(one(), 1)

    def test_two(self):
        self.assertEqual(two(), 2)

    def test_three(self):
        self.assertEqual(three(), 3)
";
const UNITTEST_1: &str = "\
import unittest


class QuestionTests(unittest.TestCase):
    def test_one(self):
        self.assertEqual(one(), 1)
";

#[test]
fn a_test_tsx_file_counts_when_package_json_has_no_runner_configuration() {
    let run = reduce(
        &[("package.json", r#"{"name": "app"}"#)],
        "src/App.test.tsx",
        TSX_3,
        TSX_1,
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome("test-floor")["examined"], 3, "{}", run.stdout);
}

#[test]
fn a_swift_xctest_file_counts_though_swift_has_no_runner_model() {
    let run = reduce(&[], "MyAppTests/LoginTests.swift", SWIFT_3, SWIFT_1);
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome("test-floor")["examined"], 3, "{}", run.stdout);
}

#[test]
fn a_django_style_tests_py_counts_when_no_pytest_configuration_exists() {
    let run = reduce(&[], "polls/tests.py", UNITTEST_3, UNITTEST_1);
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome("test-floor")["examined"], 3, "{}", run.stdout);
}

/// Control: a parsed pytest configuration that excludes the file keeps it out.
#[test]
fn a_parsed_pytest_configuration_still_excludes_a_file_outside_testpaths() {
    let run = reduce(
        &[(
            "pyproject.toml",
            "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
        )],
        "scripts/checks.py",
        UNITTEST_3,
        UNITTEST_1,
    );
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["examined"], 2, "{}", run.stdout);
    let notes = notes(&run);
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("runner collection unknown")),
        "{notes:?}"
    );
}

/// Control: a parsed Jest configuration that excludes the file keeps it out.
#[test]
fn a_parsed_jest_configuration_still_excludes_a_file_outside_test_match() {
    let run = reduce(
        &[(
            "package.json",
            r#"{"name": "app", "jest": {"testMatch": ["**/*.spec.js"]}}"#,
        )],
        "src/a.test.js",
        TSX_3,
        TSX_1,
    );
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome("test-floor")["examined"], 2, "{}", run.stdout);
    let notes = notes(&run);
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("runner collection unknown")),
        "{notes:?}"
    );
}

#[test]
fn the_unknown_collection_note_names_the_side_and_the_number_of_files() {
    // Two files on the base side, one of them deleted on the head side.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("MyAppTests/LoginTests.swift", SWIFT_3),
            ("MyAppTests/OtherTests.swift", SWIFT_1),
        ],
        "test: base suite",
    );
    repo.remove("MyAppTests/OtherTests.swift");
    repo.commit("test: drop a suite");
    let run = repo.check(&[]);
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );

    let notes = notes(&run);
    let unknown: Vec<&String> = notes
        .iter()
        .filter(|n| n.contains("runner collection unknown"))
        .collect();
    // One note per side for the one reason.
    assert_eq!(unknown.len(), 2, "{notes:?}");
    assert!(
        unknown
            .iter()
            .any(|n| n.starts_with("head: ") && n.contains("1 file(s)")),
        "{notes:?}"
    );
    assert!(
        unknown
            .iter()
            .any(|n| n.starts_with("base: ") && n.contains("2 file(s)")),
        "{notes:?}"
    );
    // The count no longer falls back to a path rule, so the note must not say it does.
    assert!(
        !notes.iter().any(|n| n.contains("standard test paths")),
        "{notes:?}"
    );
}

#[test]
fn the_unknown_collection_note_does_not_quote_a_manifest_value() {
    let run = reduce(
        &[(
            "package.json",
            r#"{"name": "app", "jest": {"testRegex": ["[unclosed-marker"]}}"#,
        )],
        "src/a.test.js",
        TSX_3,
        TSX_1,
    );
    assert_eq!(
        run.titles("test-floor"),
        vec![BELOW_FLOOR],
        "{}",
        run.stdout
    );
    let notes = notes(&run);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("head: runner collection unknown")),
        "{notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("unclosed-marker")),
        "{notes:?}"
    );
}

/// A finding on a file whose collection is unknown is lifted by naming the removed
/// test, as it is for a collected file.
#[test]
fn a_removed_test_in_an_unknown_collection_file_is_a_valid_override_subject() {
    let repo = Repo::new();
    repo.commit_base("MyAppTests/LoginTests.swift", SWIFT_3, "test: base suite");
    repo.write(
        "MyAppTests/LoginTests.swift",
        &SWIFT_3.replace(
            "    func testRefresh() {\n        XCTAssertEqual(refresh(), 3)\n    }\n",
            "",
        ),
    );
    repo.commit("test: drop refresh\n\nallow-test-shrink: LoginTests.testRefresh covered by the session suite");
    let run = repo.check(&[]);
    let outcome = run.outcome("test-floor");
    assert!(run.titles("test-floor").is_empty(), "{}", run.stdout);
    assert_eq!(outcome["examined"], 4, "{}", run.stdout);
    assert!(
        outcome.to_string().contains("testRefresh"),
        "the override must be recorded on the outcome: {outcome}"
    );
}
