//! End-to-end tests for the rule that decides whether a path is test code (#598):
//! a production file must not pass as a test file because its name only starts or
//! ends with the letters of a test name, and a real test-file convention must keep
//! its leniency in `error-swallowing` and `stub-bodies`.

mod common;
use common::{Repo, Run};

const HANDLER: &str = "error-swallowing/empty-error-handler-added";
const RECLASSIFIED: &str = "error-swallowing/test-path-reclassification";
const STUB_ADDED: &str = "stub-bodies/stub-body-added";
const DISCARDED: &str = "error-swallowing/result-discarded";

const JAVA_BASE: &str = "class Repo {\n  int m() {\n    return g() * 2;\n  }\n}\n";
const JAVA_HEAD: &str = "class Repo {\n  int m() {\n    try {\n      g();\n    } catch (Exception e) {}\n    return 1;\n  }\n\n  int n() {\n    throw new UnsupportedOperationException(\"not implemented\");\n  }\n}\n";
const CS_BASE: &str = "class Repo {\n  int M() {\n    return G() * 2;\n  }\n}\n";
const CS_HEAD: &str = "class Repo {\n  int M() {\n    try { G(); } catch (Exception e) { }\n    return 1;\n  }\n  int N() {\n    throw new NotImplementedException();\n  }\n}\n";
const PHP_BASE: &str = "<?php\nclass Repo {\n  function m($x) {\n    return g($x) * 2;\n  }\n}\n";
const PHP_HEAD: &str = "<?php\nclass Repo {\n  function m($x) {\n    try { g($x); } catch (\\Exception $e) { }\n    return 1;\n  }\n  function n($x) {\n    throw new \\RuntimeException('not implemented');\n  }\n}\n";
const KT_BASE: &str = "class Repo {\n  fun m(): Int {\n    return g() * 2\n  }\n}\n";
const KT_HEAD: &str = "class Repo {\n  fun m(): Int {\n    try { g() } catch (e: Exception) { }\n    return 1\n  }\n\n  fun n(): Int {\n    TODO(\"not implemented\")\n  }\n}\n";
const SCALA_BASE: &str = "object Repo {\n  def m(): Int = {\n    g() * 2\n  }\n}\n";
const SCALA_HEAD: &str = "object Repo {\n  def m(): Int = {\n    try { g() } catch { case e: Exception => }\n    1\n  }\n\n  def n(): Int = ???\n}\n";
const PY_BASE: &str = "def load(p):\n    return open(p).read()\n";
const PY_HEAD: &str = "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p):\n    raise NotImplementedError\n";
const JS_BASE: &str = "export function load(x) {\n  return fetch(x);\n}\n";
const JS_HEAD: &str = "export function load(x) {\n  try { return fetch(x); } catch (e) {}\n}\n\nexport function save(x) {\n  throw new Error(\"not implemented\");\n}\n";
const GO_HEAD: &str = "package pkg\n\nimport \"os\"\n\nfunc Flush(f *os.File) {\n\t_, _ = f.Write(nil)\n}\n\nfunc Load(p string) int {\n\tpanic(\"not implemented\")\n}\n";

/// A production file that was deleted, and the unrelated content of the test-named
/// file added in its place: too different for git to pair the two as a rename.
const PY_DELETED: &str = "import json\n\n\ndef parse(text):\n    data = json.loads(text)\n    return [row for row in data if row]\n\n\ndef count(text):\n    return len(parse(text))\n";
const PY_WITH_A_TEST: &str = "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p):\n    raise NotImplementedError\n\n\ndef test_load():\n    assert load(\"missing\") is None\n";

/// `path` exists on `main` as `base` and the change rewrites it as `head`: the file
/// is neither added nor renamed, so only its path decides whether it is test code.
fn modified(path: &str, base: &str, head: &str, config: Option<&str>) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    if let Some(toml) = config {
        repo.write("discipline.toml", toml);
    }
    repo.write(path, base);
    repo.commit("feat: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(path, head);
    repo.commit("fix: change");
    repo.check(&[])
}

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

/// A file renamed from `base` to `head`, with a pull-request body.
fn renamed(base: (&str, &str), head: (&str, &str), config: Option<&str>, body: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    if let Some(toml) = config {
        repo.write("discipline.toml", toml);
    }
    repo.write(base.0, base.1);
    repo.commit("feat: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.git(&["rm", "-q", base.0]);
    repo.write(head.0, head.1);
    repo.commit("refactor: move");
    let status = repo.git_output(&["diff", "-M", "--name-status", "main", "HEAD"]);
    assert!(
        status.lines().any(|l| l.starts_with('R')),
        "the fixture must be a detected rename: {status}"
    );
    repo.check_with_pr(&[], body)
}

/// Lines both sides share, so that a rename is above git's similarity threshold.
fn padded(comment: &str, body: &str) -> String {
    let mut s = String::new();
    for n in [
        "one", "two", "three", "four", "five", "six", "seven", "eight",
    ] {
        s.push_str(&format!("{comment} shared line number {n} of the file\n"));
    }
    s.push_str(body);
    s
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

fn production() -> (Vec<String>, Vec<String>) {
    (vec![HANDLER.to_string()], vec![STUB_ADDED.to_string()])
}

/// What a Go production file holding `GO_HEAD` gives: Go has no handler to leave empty.
fn go_production() -> (Vec<String>, Vec<String>) {
    (vec![DISCARDED.to_string()], vec![STUB_ADDED.to_string()])
}

/// Whether a gate's notes say that a file was judged as production code for its lack
/// of tests.
fn judged_for_its_tests(run: &Run, gate: &str) -> bool {
    notes(run, gate).contains("judged as production code")
}

/// `[tests] paths` naming `path` alone.
fn declaring(path: &str) -> String {
    format!("[tests]\npaths = [\"{path}\"]\n")
}

fn test_code() -> (Vec<String>, Vec<String>) {
    (Vec::new(), Vec::new())
}

// ---- Item 1: a name that only starts or ends like a test name. ----

/// The letters `test` at the end of another word are not a test name: `Latest`,
/// `Contest`, `Protest` end with them and hold no word boundary before them.
#[test]
fn a_name_that_only_ends_with_the_letters_of_a_test_name_is_production_code() {
    for (path, base, head) in [
        ("src/main/java/Latest.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/Contest.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/repotest.java", JAVA_BASE, JAVA_HEAD),
        ("src/Domain/Contest.php", PHP_BASE, PHP_HEAD),
        ("src/Domain/Protest.php", PHP_BASE, PHP_HEAD),
        ("src/Core/Contests.cs", CS_BASE, CS_HEAD),
        ("src/main/kotlin/AUDIT.kt", KT_BASE, KT_HEAD),
    ] {
        let run = modified(path, base, head, None);
        assert_eq!(both(&run), production(), "{path}: {}", run.stdout);
        assert_eq!(run.code, 1, "{path}");
    }
}

/// `Test` at the start of a name is a test name only when a word ends there:
/// `Testimonial` and `Tester` go on in lower case.
#[test]
fn a_name_that_only_starts_with_the_letters_of_a_test_name_is_production_code() {
    for (path, base, head) in [
        (
            "src/main/java/TestimonialController.java",
            JAVA_BASE,
            JAVA_HEAD,
        ),
        ("src/main/java/Testament.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/Tester.java", JAVA_BASE, JAVA_HEAD),
        ("src/Core/Testimonial.cs", CS_BASE, CS_HEAD),
        ("src/Core/Testable.cs", CS_BASE, CS_HEAD),
    ] {
        let run = modified(path, base, head, None);
        assert_eq!(both(&run), production(), "{path}: {}", run.stdout);
        assert_eq!(run.code, 1, "{path}");
    }
}

/// Control: every real convention still gives a lenient test file.
#[test]
fn control_every_test_file_convention_is_still_test_code() {
    for (path, base, head) in [
        ("src/main/java/RepoTest.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/RepoTests.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/RepoTestCase.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/TestRepo.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/HTTPTest.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/LatestTest.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/repo_test.java", JAVA_BASE, JAVA_HEAD),
        ("src/main/java/TestingSupport.java", JAVA_BASE, JAVA_HEAD),
        ("src/test/java/Repo.java", JAVA_BASE, JAVA_HEAD),
        ("src/Core/RepoTests.cs", CS_BASE, CS_HEAD),
        ("src/Core/TestRepo.cs", CS_BASE, CS_HEAD),
        ("src/Core/repo_tests.cs", CS_BASE, CS_HEAD),
        ("src/Domain/RepoTest.php", PHP_BASE, PHP_HEAD),
        ("src/Domain/repo_test.php", PHP_BASE, PHP_HEAD),
        ("src/main/kotlin/RepoIT.kt", KT_BASE, KT_HEAD),
        ("src/main/kotlin/RepoSpec.kt", KT_BASE, KT_HEAD),
        ("src/main/scala/RepoSuite.scala", SCALA_BASE, SCALA_HEAD),
        ("app/test_repo.py", PY_BASE, PY_HEAD),
        ("app/repo_test.py", PY_BASE, PY_HEAD),
        ("web/repo.test.ts", JS_BASE, JS_HEAD),
        ("web/repo.spec.js", JS_BASE, JS_HEAD),
        ("web/sub/__tests__/repo.js", JS_BASE, JS_HEAD),
        ("tests/repo.py", PY_BASE, PY_HEAD),
    ] {
        let run = modified(path, base, head, None);
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
        assert_eq!(run.code, 0, "{path}");
    }
}

/// Control: a name that sits at a word boundary and is not a test (`TestDataBuilder`,
/// `test_runner`) is still read as test code. The name alone cannot settle it.
#[test]
fn control_a_test_word_at_a_boundary_is_test_code_whatever_the_file_holds() {
    for (path, base, head) in [
        ("src/Core/TestDataBuilder.cs", CS_BASE, CS_HEAD),
        ("app/test_runner.py", PY_BASE, PY_HEAD),
    ] {
        let run = modified(path, base, head, None);
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
        assert_eq!(run.code, 0, "{path}");
    }
}

/// Control: `[tests] paths` declares a file whose name the rule does not match.
#[test]
fn control_a_declared_glob_makes_a_near_miss_name_test_code() {
    let config = "[tests]\npaths = [\"src/main/java/Latest.java\"]\n";
    let run = modified(
        "src/main/java/Latest.java",
        JAVA_BASE,
        JAVA_HEAD,
        Some(config),
    );
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
    assert_eq!(run.code, 0);
}

// ---- Item 2: suffix forms and directories at the root. ----

/// `.test.` and `.spec.` are test names before every JavaScript and TypeScript
/// extension, not only `.js` and `.ts`.
#[test]
fn test_and_spec_suffixes_cover_every_javascript_extension() {
    for ext in ["mjs", "cjs", "mts", "cts", "jsx", "tsx"] {
        for word in ["test", "spec"] {
            let path = format!("web/util.{word}.{ext}");
            let run = modified(&path, JS_BASE, JS_HEAD, None);
            assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
        }
    }
}

/// `util.test.js` renamed to `util.test.mjs` was test code and still is.
#[test]
fn a_test_file_renamed_to_another_module_extension_keeps_test_scope() {
    let run = renamed(
        ("web/util.test.js", &padded("//", JS_BASE)),
        ("web/util.test.mjs", &padded("//", JS_HEAD)),
        None,
        "no-issue: test fixture",
    );
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
}

/// A test directory is one at any depth, the first component included.
#[test]
fn a_test_directory_at_the_root_is_test_scope() {
    for (path, base, head) in [
        ("__tests__/repo.js", JS_BASE, JS_HEAD),
        ("__tests__/deep/repo.ts", JS_BASE, JS_HEAD),
        ("it/Repo.scala", SCALA_BASE, SCALA_HEAD),
        ("androidTest/Repo.kt", KT_BASE, KT_HEAD),
    ] {
        let run = modified(path, base, head, None);
        assert_eq!(both(&run), test_code(), "{path}: {}", run.stdout);
    }
}

/// Control: a directory or file whose name only contains a test directory's name
/// is production code.
#[test]
fn control_a_name_that_contains_a_test_directory_name_is_production_code() {
    for (path, base, head) in [
        ("my__tests__/repo.js", JS_BASE, JS_HEAD),
        ("web/contest/repo.js", JS_BASE, JS_HEAD),
        ("submit/Repo.scala", SCALA_BASE, SCALA_HEAD),
    ] {
        let run = modified(path, base, head, None);
        assert_eq!(both(&run), production(), "{path}: {}", run.stdout);
    }
}

// ---- Item 3: test code by a declared glob on the base side. ----

/// A file that is test code on the base side only through `[tests] paths`, renamed
/// into a conventional test path, was never production code: nothing is reclassified.
#[test]
fn a_declared_test_file_renamed_into_a_conventional_test_path_is_not_reclassified() {
    let config = "[tests]\npaths = [\"qa/**\"]\n";
    let run = renamed(
        ("qa/checks.py", &padded("#", PY_BASE)),
        ("app/test_checks.py", &padded("#", PY_HEAD)),
        Some(config),
        "no-issue: test fixture",
    );
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

/// Control: the same rename without the declaration is production code moved into
/// test scope, and is reported.
#[test]
fn control_an_undeclared_file_renamed_into_a_test_path_is_reclassified() {
    let run = renamed(
        ("qa/checks.py", &padded("#", PY_BASE)),
        ("app/test_checks.py", &padded("#", PY_HEAD)),
        None,
        "no-issue: test fixture",
    );
    assert_eq!(
        both(&run),
        (
            vec![HANDLER.to_string(), RECLASSIFIED.to_string()],
            vec![STUB_ADDED.to_string()]
        ),
        "{}",
        run.stdout
    );
}

// ---- Item 4: a production file deleted and added again under a test name. ----

/// A production file is deleted and a test-named file of the same stem is added whose
/// content git does not pair with it. The added file holds no test: it is judged as
/// production code, and both gates name both paths and say why.
#[test]
fn a_deleted_production_file_added_again_under_a_test_name_is_production_code() {
    for (gone, path, src) in [
        ("app/loader.py", "app/test_loader.py", PY_HEAD),
        ("lib/loader.py", "app/loader_test.py", PY_HEAD),
        (
            "src/main/java/Loader.java",
            "src/main/java/TestLoader.java",
            JAVA_HEAD,
        ),
        (
            "src/main/java/Loader.java",
            "src/main/java/LoaderTest.java",
            JAVA_HEAD,
        ),
        ("web/loader.js", "web/loader.test.js", JS_HEAD),
        ("web/loader.js", "web/loader.spec.mjs", JS_HEAD),
    ] {
        let run = added(&[(path, src)], Some((gone, PY_DELETED)), None);
        assert_eq!(both(&run), production(), "{path}: {}", run.stdout);
        for gate in ["error-swallowing", "stub-bodies"] {
            let notes = notes(&run, gate);
            assert!(
                notes.contains(path) && notes.contains(gone) && notes.contains("holds no test"),
                "{path}: {gate}: {notes}"
            );
        }
    }
}

/// The same added file with nothing deleted beside it holds no test either: a name
/// cannot make it test code, so it is judged as production code, and the note names it
/// alone (#630). Beside each row, the same file under a `[tests] paths` glob stays test
/// code.
#[test]
fn an_added_test_named_file_with_no_test_is_production_code_with_nothing_deleted() {
    for (path, src) in [
        ("app/test_loader.py", PY_HEAD),
        ("src/main/java/TestLoader.java", JAVA_HEAD),
        ("web/loader.test.js", JS_HEAD),
    ] {
        let run = added(&[(path, src)], None, None);
        assert_eq!(both(&run), production(), "{path}: {}", run.stdout);
        for gate in ["error-swallowing", "stub-bodies"] {
            let notes = notes(&run, gate);
            assert!(
                notes.contains(path)
                    && notes.contains("holds no test")
                    && !notes.contains("deletes the production file"),
                "{path}: {gate}: {notes}"
            );
        }
        let declared = added(&[(path, src)], None, Some(&declaring(path)));
        assert_eq!(both(&declared), test_code(), "{path}: {}", declared.stdout);
        assert!(
            !judged_for_its_tests(&declared, "error-swallowing"),
            "{path}: {}",
            declared.stdout
        );
    }
}

/// Control: Go compiles a `_test.go` file into test binaries only, so with nothing
/// deleted beside it a new one is test code with no test in it. The same source under
/// a production name is reported by both gates.
#[test]
fn control_an_added_go_test_file_with_no_deleted_counterpart_is_test_code() {
    let run = added(&[("pkg/helpers_test.go", GO_HEAD)], None, None);
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
    assert!(
        !judged_for_its_tests(&run, "error-swallowing"),
        "{}",
        run.stdout
    );
    let reported = added(&[("pkg/helpers.go", GO_HEAD)], None, None);
    assert_eq!(both(&reported), go_production(), "{}", reported.stdout);
}

/// A deleted production file whose stem is another one is not paired: the added file
/// is judged as production code for holding no test, and the note does not name the
/// deleted file. Control: declared under `[tests] paths`, it stays test code.
#[test]
fn a_deleted_file_with_another_stem_does_not_pair_and_the_added_file_is_production_code() {
    let gone = Some(("app/reader.py", PY_DELETED));
    let run = added(&[("app/test_loader.py", PY_HEAD)], gone, None);
    assert_eq!(both(&run), production(), "{}", run.stdout);
    for gate in ["error-swallowing", "stub-bodies"] {
        let notes = notes(&run, gate);
        assert!(
            notes.contains("app/test_loader.py") && !notes.contains("app/reader.py"),
            "{gate}: {notes}"
        );
    }
    let config = declaring("app/test_loader.py");
    let declared = added(&[("app/test_loader.py", PY_HEAD)], gone, Some(&config));
    assert_eq!(both(&declared), test_code(), "{}", declared.stdout);
}

/// The production file of the matching stem is still there, changed in the same
/// change, so nothing is paired: the added file is judged as production code for
/// holding no test, and the note does not say a file was deleted. Control: the same
/// added file with a test that checks something stays test code.
#[test]
fn a_changed_file_that_is_not_deleted_does_not_pair_and_the_added_file_is_production_code() {
    let beside_a_changed_file = |added_src: &str| {
        let repo = Repo::new();
        repo.git(&["checkout", "-q", "main"]);
        repo.write("app/loader.py", PY_DELETED);
        repo.commit("feat: base");
        repo.git(&["checkout", "-q", "-B", "work"]);
        repo.write("app/loader.py", PY_BASE);
        repo.write("app/test_loader.py", added_src);
        repo.commit("feat: add");
        repo.check(&[])
    };
    let run = beside_a_changed_file(PY_HEAD);
    assert_eq!(both(&run), production(), "{}", run.stdout);
    for gate in ["error-swallowing", "stub-bodies"] {
        let notes = notes(&run, gate);
        assert!(
            notes.contains("app/test_loader.py") && !notes.contains("deletes the production file"),
            "{gate}: {notes}"
        );
    }
    let with_a_test = beside_a_changed_file(PY_WITH_A_TEST);
    assert_eq!(both(&with_a_test), test_code(), "{}", with_a_test.stdout);
}

/// A deleted TEST file of the matching stem is not paired, the base side not being
/// production code: the added file is judged as production code for holding no test,
/// and the note does not name the deleted file. Control: with the added file declared
/// as well, it stays test code.
#[test]
fn a_deleted_test_file_with_the_matching_stem_does_not_pair_and_the_added_file_is_production_code()
{
    let unpaired = |run: &Run, gone: &str| {
        assert_eq!(both(run), production(), "{gone}: {}", run.stdout);
        for gate in ["error-swallowing", "stub-bodies"] {
            let notes = notes(run, gate);
            assert!(
                notes.contains("app/test_loader.py") && !notes.contains(gone),
                "{gone}: {gate}: {notes}"
            );
        }
    };
    let in_directory = added(
        &[("app/test_loader.py", PY_HEAD)],
        Some(("tests/loader.py", PY_DELETED)),
        None,
    );
    unpaired(&in_directory, "tests/loader.py");
    let config = "[tests]\npaths = [\"qa/**\"]\n";
    let declared = added(
        &[("app/test_loader.py", PY_HEAD)],
        Some(("qa/loader.py", PY_DELETED)),
        Some(config),
    );
    unpaired(&declared, "qa/loader.py");
    let config = "[tests]\npaths = [\"qa/**\", \"app/test_*.py\"]\n";
    let both_declared = added(
        &[("app/test_loader.py", PY_HEAD)],
        Some(("qa/loader.py", PY_DELETED)),
        Some(config),
    );
    assert_eq!(
        both(&both_declared),
        test_code(),
        "{}",
        both_declared.stdout
    );
}

/// Control: one test in the added file and it is left alone. This is the stated limit
/// of the rule.
#[test]
fn control_an_added_test_named_file_with_a_test_is_test_code() {
    let run = added(
        &[("app/test_loader.py", PY_WITH_A_TEST)],
        Some(("app/loader.py", PY_DELETED)),
        None,
    );
    assert_eq!(both(&run), test_code(), "{}", run.stdout);
    assert!(
        !judged_for_its_tests(&run, "error-swallowing"),
        "{}",
        run.stdout
    );
}

/// With the production file deleted, an added file under a test directory holds no
/// test and is judged as production code: a directory name does not make it test code,
/// and it is not paired with the deleted file. Control: one `[tests] paths` declares is
/// test code whether or not it holds a test, under a test directory or outside one.
#[test]
fn an_added_file_in_a_test_directory_is_production_code_and_a_declared_one_is_test_code() {
    let gone = Some(("app/loader.py", PY_DELETED));
    for path in ["tests/loader.py", "tests/test_loader.py"] {
        let run = added(&[(path, PY_HEAD)], gone, None);
        assert_eq!(both(&run), production(), "{path}: {}", run.stdout);
        for gate in ["error-swallowing", "stub-bodies"] {
            let notes = notes(&run, gate);
            assert!(
                notes.contains(path) && !notes.contains("app/loader.py"),
                "{path}: {gate}: {notes}"
            );
        }
        let declared = added(&[(path, PY_HEAD)], gone, Some(&declaring("tests/**")));
        assert_eq!(both(&declared), test_code(), "{path}: {}", declared.stdout);
    }
    let config = "[tests]\npaths = [\"app/test_*.py\"]\n";
    let declared = added(&[("app/test_loader.py", PY_HEAD)], gone, Some(config));
    assert_eq!(both(&declared), test_code(), "{}", declared.stdout);
}

/// `repo.go` is deleted and a `repo_test.go` without a test is added in the same
/// directory: the Go toolchain's rule for the name does not exempt it, and it is judged
/// as production code with both paths in the notes (#630). Control: the deleted file
/// is in another directory, which is another Go package, and the added file stays test
/// code.
#[test]
fn a_go_file_deleted_and_added_again_as_a_test_file_is_production_code() {
    let go_deleted = "package pkg\n\nfunc Run(p string) int {\n\treturn len(p) * 2\n}\n";
    let run = added(
        &[("pkg/repo_test.go", GO_HEAD)],
        Some(("pkg/repo.go", go_deleted)),
        None,
    );
    assert_eq!(both(&run), go_production(), "{}", run.stdout);
    for gate in ["error-swallowing", "stub-bodies"] {
        let notes = notes(&run, gate);
        assert!(
            notes.contains("pkg/repo_test.go")
                && notes.contains("pkg/repo.go")
                && notes.contains("holds no test"),
            "{gate}: {notes}"
        );
    }
    let elsewhere = added(
        &[("pkg/repo_test.go", GO_HEAD)],
        Some(("other/repo.go", go_deleted)),
        None,
    );
    assert_eq!(both(&elsewhere), test_code(), "{}", elsewhere.stdout);
}

/// Control: an added production file is reported as before, with no note about test
/// names.
#[test]
fn control_an_added_production_file_is_reported_without_the_note() {
    let run = added(&[("app/loader.py", PY_HEAD)], None, None);
    assert_eq!(both(&run), production(), "{}", run.stdout);
    assert!(
        !judged_for_its_tests(&run, "error-swallowing"),
        "{}",
        run.stdout
    );
}

// ---- Item 5: what `allow-swallow` on a path lifted. ----

fn override_codes(run: &Run, gate: &str) -> Vec<String> {
    let mut got: Vec<String> = run.outcome(gate)["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["code"].as_str().unwrap().to_string())
        .collect();
    got.sort();
    got
}

/// Pinned: `allow-swallow: <path>` lifts the reclassification and each handler
/// finding of the file, and every lifted finding has an override record of its own
/// carrying its code.
#[test]
fn control_each_finding_allow_swallow_lifts_has_its_own_override_record() {
    let run = renamed(
        ("app/service.py", &padded("#", PY_BASE)),
        ("app/test_service.py", &padded("#", PY_HEAD)),
        None,
        "allow-swallow: app/test_service.py the loader is a fixture now",
    );
    assert_eq!(
        override_codes(&run, "error-swallowing"),
        vec![HANDLER.to_string(), RECLASSIFIED.to_string()],
        "{}",
        run.stdout
    );
    assert!(codes(&run, "error-swallowing").is_empty(), "{}", run.stdout);
}

/// The lift of a reclassification says so in the gate's notes: the override line of
/// the terminal report shows the directive and the path, not the finding.
#[test]
fn a_lifted_reclassification_is_named_in_the_notes() {
    let run = renamed(
        ("app/service.py", &padded("#", PY_BASE)),
        ("app/test_service.py", &padded("#", PY_HEAD)),
        None,
        "allow-swallow: app/test_service.py the loader is a fixture now",
    );
    let notes = notes(&run, "error-swallowing");
    assert!(
        notes.contains("test-path-reclassification")
            && notes.contains("app/test_service.py")
            && notes.contains("app/service.py")
            && notes.contains("1 handler finding"),
        "{notes}"
    );
}

/// Control: a lift of handler findings alone adds no such note.
#[test]
fn control_a_lift_without_a_reclassification_adds_no_note() {
    let repo = Repo::new();
    repo.write("app/service.py", PY_HEAD);
    repo.commit("feat: add");
    let run = repo.check_with_pr(&[], "allow-swallow: app/service.py best effort");
    assert_eq!(
        override_codes(&run, "error-swallowing"),
        vec![HANDLER.to_string()],
        "{}",
        run.stdout
    );
    assert!(
        !notes(&run, "error-swallowing").contains("test-path-reclassification"),
        "{}",
        run.stdout
    );
}
