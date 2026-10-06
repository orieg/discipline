//! End-to-end tests for the test-path classification of `error-swallowing` and
//! `stub-bodies` on renames (#565): a change must not escape either gate by making
//! a production file look like a test file.

mod common;
use common::{Repo, Run};

/// Lines both sides share, so that the rename is above git's similarity threshold
/// whatever the body does.
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

/// Commit `base` on `main`, then move it to `head` on a branch and run the check.
/// `config` is the whole `discipline.toml`, present on both sides.
fn moved(base: (&str, &str), head: (&str, &str), config: Option<&str>) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    if let Some(toml) = config {
        repo.write("discipline.toml", toml);
    }
    repo.write(base.0, base.1);
    repo.commit("feat: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    if base.0 != head.0 {
        repo.git(&["rm", "-q", base.0]);
    }
    repo.write(head.0, head.1);
    repo.commit("refactor: move");
    if base.0 != head.0 {
        let status = repo.git_output(&["diff", "-M", "--name-status", "main", "HEAD"]);
        assert!(
            status.lines().any(|l| l.starts_with('R')),
            "the fixture must be a detected rename: {status}"
        );
    }
    repo.check(&[])
}

/// A file the change adds, with no base side.
fn added(head: (&str, &str), config: Option<&str>) -> Run {
    let repo = Repo::new();
    if let Some(toml) = config {
        repo.git(&["checkout", "-q", "main"]);
        repo.write("discipline.toml", toml);
        repo.commit("chore: configuration");
        repo.git(&["checkout", "-q", "-B", "work"]);
    }
    repo.write(head.0, head.1);
    repo.commit("feat: add");
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

const HANDLER: &str = "error-swallowing/empty-error-handler-added";
const RECLASSIFIED: &str = "error-swallowing/test-path-reclassification";
const STUB_ADDED: &str = "stub-bodies/stub-body-added";
const BODY_REPLACED: &str = "stub-bodies/body-replaced-by-stub";

fn go_base() -> String {
    padded(
        "//",
        "package pkg\n\nfunc Load(p string) int {\n\treturn run(p) * 2\n}\n",
    )
}

fn go_head() -> String {
    padded(
        "//",
        "package pkg\n\nfunc Load(p string) int {\n\tpanic(\"not implemented\")\n}\n",
    )
}

fn py_base() -> String {
    padded("#", "def load(p):\n    return open(p).read()\n")
}

fn py_head() -> String {
    padded(
        "#",
        "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p):\n    raise NotImplementedError\n",
    )
}

fn js_base() -> String {
    padded("//", "export function load(x) {\n  return fetch(x);\n}\n")
}

fn js_head() -> String {
    padded(
        "//",
        "export function load(x) {\n  try { return fetch(x); } catch (e) {}\n}\n\nexport function save(x) {\n  throw new Error(\"not implemented\");\n}\n",
    )
}

fn cpp_base() -> String {
    padded("//", "int load(int fd) {\n  return g(fd) * 2;\n}\n")
}

fn cpp_head() -> String {
    padded(
        "//",
        "int load(int fd) {\n  try { g(fd); } catch (...) { }\n  return 1;\n}\n\nint save(int fd) {\n  throw std::logic_error(\"not implemented\");\n}\n",
    )
}

fn kt_base() -> String {
    padded(
        "//",
        "class Repo {\n  fun m(): Int {\n    return g() * 2\n  }\n}\n",
    )
}

fn kt_head() -> String {
    padded(
        "//",
        "class Repo {\n  fun m(): Int {\n    try { g() } catch (e: Exception) { }\n    return 1\n  }\n\n  fun n(): Int {\n    TODO(\"not implemented\")\n  }\n}\n",
    )
}

fn java_base() -> String {
    padded(
        "//",
        "class Repo {\n  int m() {\n    return g() * 2;\n  }\n}\n",
    )
}

fn java_head() -> String {
    padded(
        "//",
        "class Repo {\n  int m() {\n    try {\n      g();\n    } catch (Exception e) {}\n    return 1;\n  }\n\n  int n() {\n    throw new UnsupportedOperationException(\"not implemented\");\n  }\n}\n",
    )
}

const CS_HEAD: &str = "class TestDataBuilder {\n  int M() {\n    try { G(); } catch (Exception e) { }\n    return 1;\n  }\n  int N() {\n    throw new NotImplementedException();\n  }\n}\n";

fn assert_reported_as_production(label: &str, run: &Run, reclassified: bool) {
    assert_eq!(run.code, 1, "{label}: {}", run.stdout);
    let mut expected = vec![HANDLER.to_string()];
    if reclassified {
        expected.push(RECLASSIFIED.to_string());
    }
    assert_eq!(codes(run, "error-swallowing"), expected, "{label}");
    assert_eq!(codes(run, "stub-bodies"), vec![STUB_ADDED], "{label}");
}

fn assert_test_code(label: &str, run: &Run) {
    assert_eq!(run.code, 0, "{label}: {}", run.stdout);
    assert!(
        codes(run, "error-swallowing").is_empty(),
        "{label}: {:?}",
        codes(run, "error-swallowing")
    );
    assert!(
        codes(run, "stub-bodies").is_empty(),
        "{label}: {:?}",
        codes(run, "stub-bodies")
    );
}

// ---- Defects: each of these fails on v0.18.0. ----

/// A rename inside one pack that changes the extension and moves the file into
/// test scope: whether the path moved into test scope does not depend on the
/// grammar, so the file stays production code, is parsed with the head grammar,
/// and the move is reported.
#[test]
fn extension_rename_inside_one_pack_into_test_scope_stays_production_code() {
    let (js_b, js_h) = (js_base(), js_head());
    let (cpp_b, cpp_h) = (cpp_base(), cpp_head());
    let (kt_b, kt_h) = (kt_base(), kt_head());
    for (base, base_src, head, head_src) in [
        ("src/Repo.cc", &cpp_b, "src/RepoTest.cpp", &cpp_h),
        ("src/Repo.kt", &kt_b, "src/RepoTest.kts", &kt_h),
        ("web/api.js", &js_b, "tests/api.mjs", &js_h),
        ("web/api.ts", &js_b, "web/__tests__/api.tsx", &js_h),
    ] {
        let label = format!("{base} -> {head}");
        let run = moved((base, base_src), (head, head_src), None);
        assert_reported_as_production(&label, &run, true);
        for gate in ["error-swallowing", "stub-bodies"] {
            let notes = notes(&run, gate);
            assert!(
                notes.contains(base) && notes.contains("still judged as production code"),
                "{label}: {gate}: {notes}"
            );
        }
    }
}

/// The same across packs, and from a base path no pack reads.
#[test]
fn rename_across_packs_into_test_scope_stays_production_code() {
    let (java_b, kt_h) = (java_base(), kt_head());
    let (py_b, py_h) = (py_base(), py_head());
    for (base, base_src, head, head_src) in [
        (
            "src/main/java/Repo.java",
            &java_b,
            "src/main/java/RepoTest.kt",
            &kt_h,
        ),
        ("docs/service.txt", &py_b, "app/test_service.py", &py_h),
    ] {
        let label = format!("{base} -> {head}");
        let run = moved((base, base_src), (head, head_src), None);
        assert_reported_as_production(&label, &run, true);
    }
}

/// The base name is production code for the base pack and test code for the head
/// pack (`RepoSpec` is no Java test name and is a Kotlin one): the file was
/// production code on the base side and stays it.
#[test]
fn rename_across_packs_whose_conventions_differ_on_the_name_stays_production_code() {
    let run = moved(
        ("src/main/java/RepoSpec.java", &java_base()),
        ("src/main/java/RepoSpec.kt", &kt_head()),
        None,
    );
    assert_reported_as_production("RepoSpec.java -> RepoSpec.kt", &run, true);
}

/// A rename of production code into a path only `[tests] paths` reads as test
/// code stays production code, and both gates say why in their notes.
#[test]
fn rename_into_a_declared_test_path_stays_production_code_with_a_note() {
    let config = "[tests]\npaths = [\"qa/**\"]\n";
    let run = moved(
        ("app/service.py", &py_base()),
        ("qa/service.py", &py_head()),
        Some(config),
    );
    assert_reported_as_production("app/service.py -> qa/service.py", &run, false);
    for gate in ["error-swallowing", "stub-bodies"] {
        let notes = notes(&run, gate);
        assert!(
            notes.contains("qa/service.py")
                && notes.contains("app/service.py")
                && notes.contains("[tests] paths"),
            "{gate}: {notes}"
        );
    }
    // The same with a changed extension.
    let run = moved(
        ("web/api.js", &js_base()),
        ("qa/api.mjs", &js_head()),
        Some(config),
    );
    assert_reported_as_production("web/api.js -> qa/api.mjs", &run, false);
}

/// The rename check is counted: a renamed file with no handler at all is not
/// "no error handlers in changed files" over zero examined items.
#[test]
fn the_rename_check_is_counted_as_examined() {
    let run = moved(
        ("pkg/handler.go", &go_base()),
        ("pkg/handler_test.go", &go_head()),
        None,
    );
    assert_eq!(codes(&run, "error-swallowing"), vec![RECLASSIFIED]);
    let outcome = run.outcome("error-swallowing");
    assert_eq!(outcome["examined"], 1, "{outcome}");
    assert!(
        !notes(&run, "error-swallowing").contains("no error handlers"),
        "{outcome}"
    );
}

// ---- Controls: these pass on v0.18.0 and must keep passing. ----

/// A rename that only changes the FILE NAME into a pack's test-file naming rule,
/// extension unchanged, was closed by the base anchor in v0.18.0.
#[test]
fn control_same_extension_rename_into_a_test_file_name_stays_production_code() {
    let run = moved(
        ("pkg/handler.go", &go_base()),
        ("pkg/handler_test.go", &go_head()),
        None,
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(codes(&run, "error-swallowing"), vec![RECLASSIFIED]);
    assert_eq!(codes(&run, "stub-bodies"), vec![BODY_REPLACED]);
    let (py_b, py_h) = (py_base(), py_head());
    let (js_b, js_h) = (js_base(), js_head());
    for (base, base_src, head, head_src) in [
        ("app/service.py", &py_b, "app/test_service.py", &py_h),
        ("web/util.js", &js_b, "web/util.test.js", &js_h),
    ] {
        let run = moved((base, base_src), (head, head_src), None);
        assert_reported_as_production(&format!("{base} -> {head}"), &run, true);
    }
}

/// A production file renamed to another production name is reported, with no
/// reclassification, whether or not the extension changes.
#[test]
fn control_production_to_production_rename_is_reported_without_reclassification() {
    let (py_b, py_h) = (py_base(), py_head());
    let (js_b, js_h) = (js_base(), js_head());
    let (cpp_b, cpp_h) = (cpp_base(), cpp_head());
    for (base, base_src, head, head_src, note) in [
        ("app/service.py", &py_b, "app/other.py", &py_h, false),
        ("web/api.js", &js_b, "web/api.mjs", &js_h, true),
        ("src/Repo.cc", &cpp_b, "src/Repo.cpp", &cpp_h, true),
    ] {
        let label = format!("{base} -> {head}");
        let run = moved((base, base_src), (head, head_src), None);
        assert_reported_as_production(&label, &run, false);
        assert_eq!(
            notes(&run, "error-swallowing").contains("classified by its new path"),
            note,
            "{label}: {}",
            notes(&run, "error-swallowing")
        );
    }
}

/// A genuine test file, test code on the base side already, stays leniently
/// treated when it is edited, renamed, moved to another extension, or ported to
/// another language inside test scope.
#[test]
fn control_a_base_side_test_file_stays_test_code() {
    let (py_b, py_h) = (py_base(), py_head());
    let (js_b, js_h) = (js_base(), js_head());
    let (java_b, kt_h) = (java_base(), kt_head());
    for (base, base_src, head, head_src) in [
        ("tests/a_test.py", &py_b, "tests/a_test.py", &py_h),
        ("tests/a_test.py", &py_b, "tests/b_test.py", &py_h),
        ("tests/util.js", &js_b, "tests/util.mjs", &js_h),
        (
            "src/test/java/RepoTest.java",
            &java_b,
            "src/test/kotlin/RepoTest.kt",
            &kt_h,
        ),
    ] {
        let label = format!("{base} -> {head}");
        let run = moved((base, base_src), (head, head_src), None);
        assert_test_code(&label, &run);
        assert!(
            !run.stdout.contains("test-path-reclassification"),
            "{label}: {}",
            run.stdout
        );
    }
    // Test scope by a declared glob only, on both sides.
    let run = moved(
        ("qa/a.js", &js_b),
        ("qa/a.mjs", &js_h),
        Some("[tests]\npaths = [\"qa/**\"]\n"),
    );
    assert_test_code("declared qa/a.js -> qa/a.mjs", &run);
}

/// A test file renamed out of test scope is production code from then on, with
/// or without an extension change.
#[test]
fn control_a_rename_out_of_test_scope_is_production_code() {
    let (py_b, py_h) = (py_base(), py_head());
    let (js_b, js_h) = (js_base(), js_head());
    for (base, base_src, head, head_src) in [
        ("tests/a_test.py", &py_b, "app/b.py", &py_h),
        ("tests/util.js", &js_b, "web/util.mjs", &js_h),
    ] {
        let run = moved((base, base_src), (head, head_src), None);
        assert_reported_as_production(&format!("{base} -> {head}"), &run, false);
    }
}

// ---- Documented behaviour, pinned as it is (see the report on #565). ----

/// A file the change adds has no base side and is classified by its head path:
/// `docs/GATES.md` states it ("Files the change adds are classified by their
/// head path").
#[test]
fn pinned_an_added_file_is_classified_by_its_head_path() {
    let py_h = py_head();
    assert_test_code(
        "added app/test_service.py",
        &added(("app/test_service.py", &py_h), None),
    );
    assert_test_code(
        "added tests/service.py",
        &added(("tests/service.py", &py_h), None),
    );
    let run = added(("app/service.py", &py_h), None);
    assert_reported_as_production("added app/service.py", &run, false);
}

/// The base path is known only when git's similarity detection pairs the two
/// sides. A delete and an add it does not pair are an added file, classified by
/// its head path: `docs/GATES.md` lists this as not closed.
#[test]
fn pinned_a_rename_git_does_not_detect_is_an_added_file() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "pkg/handler.go",
        "package pkg\n\nfunc Load(p string) int {\n\treturn run(p) * 2\n}\n",
    );
    repo.commit("feat: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.git(&["rm", "-q", "pkg/handler.go"]);
    repo.write(
        "pkg/handler_test.go",
        "package pkg\n\nimport \"os\"\n\nfunc Fetch(name string) int {\n\tpanic(\"not implemented\")\n}\n\nfunc Store(name string) int {\n\tpanic(\"not implemented\")\n}\n",
    );
    repo.commit("refactor: move");
    let status = repo.git_output(&["diff", "-M", "--name-status", "main", "HEAD"]);
    assert!(
        !status.lines().any(|l| l.starts_with('R')),
        "the fixture must not be a detected rename: {status}"
    );
    // The deletion is not silent (`deletion-rationale` asks for a `removes:` line), but
    // neither gate under test reports the stub bodies.
    let run = repo.check(&[]);
    assert!(
        codes(&run, "error-swallowing").is_empty() && codes(&run, "stub-bodies").is_empty(),
        "{}",
        run.stdout
    );
    assert_eq!(
        codes(&run, "deletion-rationale"),
        vec!["deletion-rationale/file-deleted-without-rationale"],
        "{}",
        run.stdout
    );
}

/// A file-name rule matches at a word boundary (#598): a name that only starts or
/// ends with the letters of a test name is production code, and a name with a test
/// word at a boundary is whole-file test code whatever the file holds.
#[test]
fn pinned_a_test_word_at_a_boundary_is_test_code_and_a_near_miss_name_is_not() {
    let (py_h, java_h) = (py_head(), java_head());
    for (path, src, test_code) in [
        (
            "src/main/java/TestimonialController.java",
            java_h.as_str(),
            false,
        ),
        ("src/main/java/Latest.java", java_h.as_str(), false),
        ("app/test_runner.py", py_h.as_str(), true),
        ("src/TestDataBuilder.cs", CS_HEAD, true),
    ] {
        let run = added((path, src), None);
        if test_code {
            assert_test_code(path, &run);
        } else {
            assert_reported_as_production(path, &run, false);
        }
    }
    let run = added(("src/main/java/Other.java", &java_h), None);
    assert_reported_as_production("added Other.java", &run, false);
}
