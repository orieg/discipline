//! End-to-end tests for files that are test scope by a name or directory heuristic
//! and hold no test (#630): `error-swallowing` and `stub-bodies` judge a file the
//! change adds there as production code, unless `[tests] paths` declares it or a
//! build tool makes it test code by construction.

mod common;
use common::{Repo, Run};

const HANDLER: &str = "error-swallowing/empty-error-handler-added";
const DISCARDED: &str = "error-swallowing/result-discarded";
const STUB_ADDED: &str = "stub-bodies/stub-body-added";

const PY: &str = "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p):\n    raise NotImplementedError\n";
const PY_BASE: &str = "def load(p):\n    return open(p).read()\n";
const PY_WITH_A_TEST: &str = "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p):\n    raise NotImplementedError\n\n\ndef test_load():\n    assert load(\"missing\") is None\n";
const PY_WITH_AN_EMPTY_TEST: &str = "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p):\n    raise NotImplementedError\n\n\ndef test_placeholder():\n    pass\n";
const CS: &str = "class TestDataBuilder {\n  int M() {\n    try { G(); } catch (Exception e) { }\n    return 1;\n  }\n  int N() {\n    throw new NotImplementedException();\n  }\n}\n";
const JAVA: &str = "class Fixtures {\n  int m() {\n    try {\n      g();\n    } catch (Exception e) {}\n    return 1;\n  }\n\n  int n() {\n    throw new UnsupportedOperationException(\"not implemented\");\n  }\n}\n";
const JS: &str = "export function load(x) {\n  try { return fetch(x); } catch (e) {}\n}\n\nexport function save(x) {\n  throw new Error(\"not implemented\");\n}\n";
const GO: &str = "package pkg\n\nimport \"os\"\n\nfunc Flush(f *os.File) {\n\t_, _ = f.Write(nil)\n}\n\nfunc Load(p string) int {\n\tpanic(\"not implemented\")\n}\n";
const GO_WITH_A_TEST: &str = "package pkg\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nfunc Flush(f *os.File) {\n\t_, _ = f.Write(nil)\n}\n\nfunc Load(p string) int {\n\tpanic(\"not implemented\")\n}\n\nfunc TestLoad(t *testing.T) {\n\tif Load(\"x\") != 1 {\n\t\tt.Fatal(\"load\")\n\t}\n}\n";
const GO_DELETED: &str = "package pkg\n\nfunc Run(p string) int {\n\treturn len(p) * 2\n}\n";
const SWIFT: &str = "func load() {\n  do {\n    try g()\n  } catch {\n  }\n}\n\nfunc save() -> Int {\n  fatalError(\"not implemented\")\n}\n";
const RUST: &str = "pub fn cleanup(p: &str) {\n    let _ = std::fs::remove_file(p);\n}\n\npub fn load() -> u8 {\n    todo!()\n}\n";

/// The change adds `files`, and deletes `deleted` when one is named.
fn added(files: &[(&str, &str)], deleted: Option<(&str, &str)>, config: Option<&str>) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    if let Some(toml) = config {
        repo.write("discipline.toml", toml);
    }
    if let Some((path, src)) = deleted {
        repo.write(path, src);
    }
    repo.commit("feat: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    if let Some((path, _)) = deleted {
        repo.git(&["rm", "-q", path]);
    }
    for (path, src) in files {
        repo.write(path, src);
    }
    repo.commit("feat: add");
    if deleted.is_some() {
        let status = repo.git_output(&["diff", "-M", "--name-status", "main", "HEAD"]);
        assert!(
            !status.lines().any(|l| l.starts_with('R')),
            "the fixture must not be a detected rename: {status}"
        );
    }
    repo.check(&[])
}

/// `path` exists on `main` as `base` and the change rewrites it as `head`.
fn modified(path: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(path, base);
    repo.commit("feat: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(path, head);
    repo.commit("fix: change");
    repo.check(&[])
}

fn codes(run: &Run, gate: &str) -> Vec<String> {
    let mut got: Vec<String> = run
        .violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect();
    got.sort();
    got
}

fn notes(run: &Run, gate: &str) -> String {
    run.outcome(gate)["notes"].to_string()
}

/// `(error-swallowing codes, stub-bodies codes)` of a run.
fn both(run: &Run) -> (Vec<String>, Vec<String>) {
    (codes(run, "error-swallowing"), codes(run, "stub-bodies"))
}

fn production(handler: &str) -> (Vec<String>, Vec<String>) {
    (vec![handler.to_string()], vec![STUB_ADDED.to_string()])
}

fn test_code() -> (Vec<String>, Vec<String>) {
    (Vec::new(), Vec::new())
}

/// The note both gates leave on a file judged as production code for its lack of tests.
fn assert_named_in_the_notes(run: &Run, path: &str, holds: &str) {
    for gate in ["error-swallowing", "stub-bodies"] {
        let notes = notes(run, gate);
        assert!(
            notes.contains(path)
                && notes.contains(&format!("and {holds}; it is judged as production code")),
            "{path}: {gate}: {notes}"
        );
    }
}

// ---- A new file that is test scope by its name or directory only, with no test. ----

/// A file the change adds under a test-looking name, or under a test-looking
/// directory, with no test in it: both gates judge it as production code and name it.
#[test]
fn an_added_file_in_test_scope_by_name_only_with_no_test_is_production_code() {
    for (path, src, handler) in [
        ("src/Core/TestDataBuilder.cs", CS, HANDLER),
        ("app/test_runner.py", PY, HANDLER),
        ("tests/support/helpers.py", PY, HANDLER),
        ("src/main/java/TestFixtures.java", JAVA, HANDLER),
        ("web/__tests__/setup.js", JS, HANDLER),
        ("pkg/testutil/files.go", GO, DISCARDED),
        ("src/testutil/files.rs", RUST, DISCARDED),
    ] {
        let run = added(&[(path, src)], None, None);
        assert_eq!(both(&run), production(handler), "{path}: {}", run.stdout);
        assert_eq!(run.code, 1, "{path}");
        assert_named_in_the_notes(&run, path, "holds no test");
    }
}

/// Control: a new test file that holds a test with an assertion is test code, and no
/// note is left.
#[test]
fn control_an_added_test_file_that_holds_a_test_is_test_code() {
    for path in ["tests/test_x.py", "app/test_runner.py"] {
        let run = added(&[(path, PY_WITH_A_TEST)], None, None);
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
        assert!(
            !notes(&run, "error-swallowing").contains("judged as production code"),
            "{path}: {}",
            run.stdout
        );
    }
}

/// Control: a file `[tests] paths` declares is test code whether or not it holds a test.
#[test]
fn control_an_added_file_under_a_declared_path_is_test_code() {
    let config = "[tests]\npaths = [\"tests/support/**\", \"app/test_runner.py\"]\n";
    for path in ["tests/support/helpers.py", "app/test_runner.py"] {
        let run = added(&[(path, PY)], None, Some(config));
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
    }
}

/// Control: a file a build tool compiles or loads as test code only stays test code
/// with no test in it. Each source is reported under a production path, so the
/// silence here is the path's doing.
#[test]
fn control_a_file_that_is_test_code_by_construction_stays_test_code() {
    for (path, production_path, src, handler) in [
        ("pkg/helpers_test.go", "pkg/helpers.go", GO, DISCARDED),
        ("tests/common/mod.rs", "src/common/mod.rs", RUST, DISCARDED),
        ("benches/support.rs", "src/support.rs", RUST, DISCARDED),
        ("examples/demo.rs", "src/demo.rs", RUST, DISCARDED),
        ("tests/conftest.py", "app/setup.py", PY, HANDLER),
        ("conftest.py", "setup_env.py", PY, HANDLER),
        (
            "Pkg/Tests/AppTests/Support.swift",
            "Pkg/Sources/App/Support.swift",
            SWIFT,
            HANDLER,
        ),
        (
            "svc/src/test/java/com/x/Fixtures.java",
            "svc/src/main/java/com/x/Fixtures.java",
            JAVA,
            HANDLER,
        ),
    ] {
        let run = added(&[(path, src)], None, None);
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
        assert!(
            !notes(&run, "error-swallowing").contains("judged as production code"),
            "{path}: {}",
            run.stdout
        );
        let reported = added(&[(production_path, src)], None, None);
        assert_eq!(
            both(&reported),
            production(handler),
            "{production_path}: {}",
            reported.stdout
        );
    }
}

/// Control: a file the base already had keeps its classification: a support file
/// under a test directory, changed by this change, is test code as before.
#[test]
fn control_a_file_the_base_already_had_keeps_its_classification() {
    for path in ["tests/support/helpers.py", "app/test_runner.py"] {
        let run = modified(path, PY_BASE, PY);
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
        assert!(
            !notes(&run, "error-swallowing").contains("judged as production code"),
            "{path}: {}",
            run.stdout
        );
    }
}

/// Control: a new production file is reported without the note.
#[test]
fn control_an_added_production_file_is_reported_without_the_note() {
    let run = added(&[("app/runner.py", PY)], None, None);
    assert_eq!(both(&run), production(HANDLER), "{}", run.stdout);
    assert!(
        !notes(&run, "error-swallowing").contains("judged as production code"),
        "{}",
        run.stdout
    );
}

/// Control: a new test-named file with nothing either gate reports leaves no note.
#[test]
fn control_a_clean_added_file_leaves_no_note() {
    let run = added(&[("tests/support/helpers.py", PY_BASE)], None, None);
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
    for gate in ["error-swallowing", "stub-bodies"] {
        assert!(
            !notes(&run, gate).contains("judged as production code"),
            "{gate}: {}",
            run.stdout
        );
    }
}

// ---- One token test must not keep the whole file exempt. ----

/// A test that checks nothing is not a test for this rule: a new test-named file
/// whose only test has no assertion is judged as production code, and the note counts
/// the tests it holds, so that a file with tests in it is not said to hold none.
#[test]
fn an_added_file_whose_only_test_checks_nothing_is_production_code() {
    for path in ["app/test_runner.py", "tests/test_x.py"] {
        let run = added(&[(path, PY_WITH_AN_EMPTY_TEST)], None, None);
        assert_eq!(both(&run), production(HANDLER), "{path}: {}", run.stdout);
        assert_named_in_the_notes(&run, path, "holds 1 test(s), none of which checks anything");
    }
}

// ---- A production file deleted and added again as a Go `_test.go` file. ----

/// `repo.go` is deleted and `repo_test.go` is added in the same directory with no
/// test in it: the added file is judged as production code, and both gates name both
/// paths.
#[test]
fn a_go_file_deleted_and_added_again_as_a_test_file_is_production_code() {
    let run = added(
        &[("pkg/repo_test.go", GO)],
        Some(("pkg/repo.go", GO_DELETED)),
        None,
    );
    assert_eq!(both(&run), production(DISCARDED), "{}", run.stdout);
    for gate in ["error-swallowing", "stub-bodies"] {
        let notes = notes(&run, gate);
        assert!(
            notes.contains("pkg/repo_test.go")
                && notes.contains("pkg/repo.go")
                && notes.contains("holds no test"),
            "{gate}: {notes}"
        );
    }
}

/// Control: the deleted file is in another directory, which is another Go package:
/// the `_test.go` file is not paired and stays test code.
#[test]
fn control_a_go_file_deleted_in_another_package_does_not_pair() {
    let run = added(
        &[("pkg/repo_test.go", GO)],
        Some(("other/repo.go", GO_DELETED)),
        None,
    );
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
}

/// Control: the added `_test.go` file holds a test that checks something and stays
/// test code.
#[test]
fn control_a_go_test_file_with_a_test_is_test_code() {
    let run = added(
        &[("pkg/repo_test.go", GO_WITH_A_TEST)],
        Some(("pkg/repo.go", GO_DELETED)),
        None,
    );
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
}

// ---- What a lift of a reclassification covers. ----

/// A directive written as `path:line` lifts the handler on that line and not the move
/// into test scope, and the note left by a lift by path says so.
#[test]
fn a_lift_by_path_and_line_lifts_a_handler_and_not_the_move() {
    let shared: String = (1..=8)
        .map(|n| format!("# shared line number {n} of the file\n"))
        .collect();
    let renamed = |body: &str| {
        let repo = Repo::new();
        repo.git(&["checkout", "-q", "main"]);
        repo.write("app/service.py", &format!("{shared}{PY_BASE}"));
        repo.commit("feat: base");
        repo.git(&["checkout", "-q", "-B", "work"]);
        repo.git(&["rm", "-q", "app/service.py"]);
        repo.write("app/test_service.py", &format!("{shared}{PY}"));
        repo.commit("refactor: move");
        let status = repo.git_output(&["diff", "-M", "--name-status", "main", "HEAD"]);
        assert!(
            status.lines().any(|l| l.starts_with('R')),
            "the fixture must be a detected rename: {status}"
        );
        repo.check_with_pr(&[], body)
    };
    let by_line = renamed("allow-swallow: app/test_service.py:12 the loader is a fixture now");
    assert_eq!(
        codes(&by_line, "error-swallowing"),
        vec!["error-swallowing/test-path-reclassification".to_string()],
        "{}",
        by_line.stdout
    );
    let by_path = renamed("allow-swallow: app/test_service.py the loader is a fixture now");
    assert!(
        codes(&by_path, "error-swallowing").is_empty(),
        "{}",
        by_path.stdout
    );
    let notes = notes(&by_path, "error-swallowing");
    assert!(
        notes.contains(
            "`allow-swallow: app/test_service.py:<line>` lifts one handler and not the move"
        ),
        "{notes}"
    );
}
