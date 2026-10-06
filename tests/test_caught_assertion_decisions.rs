//! `assertion-reduction/assertion-failure-caught` and `vacuous-tests` (#627): what a Go
//! `recover()` does and does not catch, a Python handler for a project type, an assertion
//! in a callback that runs before the `try` ends, a named promise handler, a `finally`
//! that returns, which branch of a `catch_unwind` result fails, NUnit calls that end a test
//! without failing it, the entry point of a testify suite, receiver calls, and configured
//! helper names.
//!
//! The sources below are fixtures: text handed to the binary, never code of this repository.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const GATE: &str = "assertion-reduction";
const VACUOUS: &str = "vacuous-tests";
const CAUGHT: &str = "Assertion Failure Caught Inside Test";

fn run_with(config: &str, path: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    let mut files = vec![
        (path, base),
        ("package.json", "{\"devDependencies\": {\"jest\": \"*\"}}\n"),
    ];
    if !config.is_empty() {
        files.push(("discipline.toml", config));
    }
    repo.commit_base_files(&files, "test: base");
    repo.write(path, head);
    repo.commit("test: change");
    repo.check(&[])
}

fn run_files(path: &str, base: &str, head: &str) -> Run {
    run_with("", path, base, head)
}

/// The lines of the swallowed assertions reported; any other finding of the gate fails.
fn caught_in(run: &Run) -> Vec<u64> {
    let violations = run.violations(GATE);
    assert!(
        violations.iter().all(|v| v["title"] == CAUGHT),
        "{violations:?}\n{}",
        run.stderr
    );
    violations
        .iter()
        .map(|v| v["line"].as_u64().expect("a caught assertion has a line"))
        .collect()
}

/// One language's fixture: the file, the text around a test body, and the unwrapped body.
struct Lang {
    path: &'static str,
    before: &'static str,
    after: &'static str,
    plain: &'static str,
}

impl Lang {
    fn file(&self, body: &str) -> String {
        format!("{}{}{}", self.before, body, self.after)
    }

    /// The base holds the plain assertion; the head holds `body` in its place.
    fn check_each(&self, cases: &[(String, Vec<u64>)]) {
        let cases: Vec<(String, String, Vec<u64>)> = cases
            .iter()
            .map(|(body, want)| (self.plain.to_string(), body.clone(), want.clone()))
            .collect();
        self.check_pairs(&cases);
    }

    /// Runs every `(base body, head body, lines expected)` and fails once, listing each
    /// head body that differs.
    fn check_pairs(&self, cases: &[(String, String, Vec<u64>)]) {
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(base, head, want)| {
                let got = caught_in(&run_files(self.path, &self.file(base), &self.file(head)));
                (got != *want).then(|| format!("expected {want:?}, reported {got:?}:\n{head}"))
            })
            .collect();
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }
}

fn silent(body: impl Into<String>) -> (String, Vec<u64>) {
    (body.into(), Vec::new())
}

fn at(line: u64, body: impl Into<String>) -> (String, Vec<u64>) {
    (body.into(), vec![line])
}

/// Body starts on line 5.
const PY: Lang = Lang {
    path: "tests/test_t.py",
    before: "from checks import CheckFailed, check_total\n\n\ndef test_a():\n",
    after: "",
    plain: "    assert f() == 1\n",
};
/// Body starts on line 3.
const RS: Lang = Lang {
    path: "tests/t.rs",
    before: "#[test]\nfn adds() {\n",
    after: "}\n",
    plain: "    assert_eq!(add(2, 2), 4);\n",
};
/// Body starts on line 2.
const JS: Lang = Lang {
    path: "a.test.js",
    before: "test('adds', (done) => {\n",
    after: "});\n\nfunction ignore(e) {\n  console.log(e);\n}\n\nconst quiet = (e) => {};\n\nfunction rethrow(e) {\n  throw e;\n}\n",
    plain: "  expect(add(2, 2)).toBe(4);\n",
};
/// Body starts on line 7.
const JAVA: Lang = Lang {
    path: "src/test/java/ATest.java",
    before: "import static org.junit.jupiter.api.Assertions.*;\nimport org.junit.jupiter.api.Test;\n\nclass ATest {\n    @Test\n    void adds() {\n",
    after: "    }\n}\n",
    plain: "        assertEquals(4, add(2, 2));\n",
};
/// Body starts on line 6.
const KT: Lang = Lang {
    path: "src/test/kotlin/ATest.kt",
    before: "import org.junit.jupiter.api.Test\n\nclass ATest {\n    @Test\n    fun adds() {\n",
    after: "    }\n}\n",
    plain: "        assertEquals(4, add(2, 2))\n",
};
/// Body starts on line 6.
const CS: Lang = Lang {
    path: "tests/ATests.cs",
    before:
        "using NUnit.Framework;\n\npublic class ATests {\n    [Test]\n    public void Adds() {\n",
    after: "    }\n}\n",
    plain: "        Assert.AreEqual(4, Add(2, 2));\n",
};
/// Body starts on line 6. `mustEqual` panics; it is declared after the test.
const GO: Lang = Lang {
    path: "p_test.go",
    before: "package p\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n",
    after: "}\n\nfunc mustEqual(want, got int) {\n\tif want != got {\n\t\tpanic(\"not equal\")\n\t}\n}\n",
    plain: "\trequire.Equal(t, 4, add(2, 2))\n",
};

// ---------------------------------------------------------------------------
// Go: what `recover()` catches
// ---------------------------------------------------------------------------

fn go_defer(handler: &str, check: &str) -> String {
    format!("\tdefer func() {{\n{handler}\t}}()\n{check}")
}

fn go_recovered(action: &str, check: &str) -> String {
    go_defer(
        &format!("\t\tif r := recover(); r != nil {{\n\t\t\t{action}\n\t\t}}\n"),
        check,
    )
}

const GO_PANICKING_CHECK: &str = "\tmustEqual(4, add(2, 2))\n";
const GO_SWALLOW: &str = "\t\t_ = recover()\n";

/// The base holds `check`; the head holds it after a `defer` of `handler`.
fn go_deferred(handler: &str, check: &str, want: &[u64]) -> (String, String, Vec<u64>) {
    (check.to_string(), go_defer(handler, check), want.to_vec())
}

/// The same, the deferred function acting on what it recovered.
fn go_acting(action: &str, check: &str, want: &[u64]) -> (String, String, Vec<u64>) {
    (
        check.to_string(),
        go_recovered(action, check),
        want.to_vec(),
    )
}

/// `require`, `assert` and `t.Fatal` end or mark the test without panicking, so a
/// recovering `defer` does not catch them.
#[test]
fn go_assertions_that_do_not_panic_are_not_caught_by_a_recovering_defer() {
    GO.check_pairs(&[
        go_deferred(GO_SWALLOW, GO.plain, &[]),
        go_acting("log.Println(r)", GO.plain, &[]),
        go_deferred(GO_SWALLOW, "\tassert.Equal(t, 4, add(2, 2))\n", &[]),
        go_deferred(
            GO_SWALLOW,
            "\tif add(2, 2) != 4 {\n\t\tt.Fatal(\"no\")\n\t}\n",
            &[],
        ),
        go_deferred(
            GO_SWALLOW,
            "\tif add(2, 2) != 4 {\n\t\tt.Errorf(\"no\")\n\t}\n",
            &[],
        ),
    ]);
}

/// A check that panics is swallowed by a deferred `recover()` that does not fail the test.
#[test]
fn go_panicking_check_after_a_swallowing_recover_is_reported() {
    GO.check_pairs(&[
        go_deferred(GO_SWALLOW, GO_PANICKING_CHECK, &[9]),
        go_acting("log.Println(r)", GO_PANICKING_CHECK, &[11]),
        // `err.Error()` formats the error; it does not fail the test.
        go_acting("log.Println(r.(error).Error())", GO_PANICKING_CHECK, &[11]),
        go_acting("_ = fmt.Errorf(\"%v\", r)", GO_PANICKING_CHECK, &[11]),
        // A panic written in the test itself.
        go_deferred(
            GO_SWALLOW,
            "\tif add(2, 2) != 4 {\n\t\tpanic(\"not equal\")\n\t}\n",
            &[10],
        ),
        // The deferred function is one declared in the file.
        (
            format!("{GO_PANICKING_CHECK}}}\n\nfunc quiet() {{\n{GO_SWALLOW}"),
            format!("\tdefer quiet()\n{GO_PANICKING_CHECK}}}\n\nfunc quiet() {{\n{GO_SWALLOW}"),
            vec![7],
        ),
    ]);
}

/// Controls: a deferred function that fails the test, or that does not recover, and a
/// check made before the `defer`.
#[test]
fn go_panicking_check_is_not_reported_when_the_panic_still_fails_the_test() {
    GO.check_pairs(&[
        go_acting("t.Fatal(r)", GO_PANICKING_CHECK, &[]),
        go_acting("t.Errorf(\"%v\", r)", GO_PANICKING_CHECK, &[]),
        go_acting("t.Error(r)", GO_PANICKING_CHECK, &[]),
        go_acting("t.FailNow()", GO_PANICKING_CHECK, &[]),
        go_acting("t.Fail()", GO_PANICKING_CHECK, &[]),
        go_acting("panic(r)", GO_PANICKING_CHECK, &[]),
        go_acting("require.Fail(t, \"panicked\")", GO_PANICKING_CHECK, &[]),
        // A testify call in the deferred function fails the test when it does not hold.
        go_deferred(
            "\t\tr := recover()\n\t\trequire.Nil(t, r)\n",
            GO_PANICKING_CHECK,
            &[],
        ),
        go_deferred("\t\tcleanup()\n", GO_PANICKING_CHECK, &[]),
        (
            GO_PANICKING_CHECK.to_string(),
            format!("{GO_PANICKING_CHECK}\tdefer func() {{\n{GO_SWALLOW}\t}}()\n"),
            Vec::new(),
        ),
        // A function of the file that does not panic.
        go_deferred(
            GO_SWALLOW,
            "\tquietly(add(2, 2))\n}\n\nfunc quietly(n int) {\n\tlog.Println(n)\n",
            &[],
        ),
    ]);
}

/// A `require` call inside the deferred function is not an assertion the `recover()`
/// catches, with or without one after the `defer`.
#[test]
fn go_require_inside_the_deferred_function_is_not_a_caught_assertion() {
    GO.check_pairs(&[
        go_deferred(
            "\t\tif r := recover(); r != nil {\n\t\t\tlog.Println(r)\n\t\t}\n\t\trequire.NotNil(t, state())\n",
            GO.plain,
            &[],
        ),
        go_deferred(
            "\t\t_ = recover()\n\t\trequire.NotNil(t, state())\n",
            GO_PANICKING_CHECK,
            &[],
        ),
    ]);
}

/// A test the change adds with a recovering `defer` and assertions that do not panic
/// has assertions that can fail: it is not vacuous, and nothing is reported as caught.
#[test]
fn go_added_test_with_a_recovering_defer_and_real_assertions_is_not_vacuous() {
    let base = GO.file(GO.plain);
    let added = format!(
        "{base}\nfunc TestSub(t *testing.T) {{\n\tdefer func() {{\n\t\t_ = recover()\n\t}}()\n\trequire.Equal(t, 0, sub(2, 2))\n}}\n"
    );
    let run = run_files(GO.path, &base, &added);
    assert_eq!(caught_in(&run), Vec::<u64>::new(), "{}", run.stdout);
    assert_eq!(run.violations(VACUOUS).len(), 0, "{}", run.stdout);
    // Control: the only check of the added test panics into the `recover()`.
    let swallowed = added.replace("require.Equal(t, 0, sub(2, 2))", "mustEqual(0, sub(2, 2))");
    let run = run_files(GO.path, &base, &swallowed);
    assert_eq!(caught_in(&run).len(), 1, "{}", run.stdout);
    assert_eq!(run.violations(VACUOUS).len(), 1, "{}", run.stdout);
}

// ---------------------------------------------------------------------------
// Go: the entry point of a testify suite
// ---------------------------------------------------------------------------

const GO_SUITE: &str = "package calc\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/suite\"\n)\n\ntype CalcSuite struct {\n\tsuite.Suite\n}\n\nfunc (s *CalcSuite) TestAdd() {\n\ts.Equal(4, Add(2, 2))\n}\n\n";

fn go_suite_file(entry: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(&[("calc.go", "package calc\n")], "test: base");
    repo.write("calc_suite_test.go", &format!("{GO_SUITE}{entry}"));
    repo.commit("test: add a suite");
    repo.check(&[])
}

/// `suite.Run(t, new(CalcSuite))` runs the suite's test methods: it is not a subtest
/// with no assertion, however the suite value is spelled.
#[test]
fn go_suite_entry_point_is_not_a_vacuous_test() {
    for entry in [
        "func TestCalcSuite(t *testing.T) {\n\tsuite.Run(t, new(CalcSuite))\n}\n",
        "func TestCalcSuite(t *testing.T) {\n\tsuite.Run(t, &CalcSuite{})\n}\n",
        "func TestCalcSuite(t *testing.T) {\n\ts := new(CalcSuite)\n\tsuite.Run(t, s)\n}\n",
        // The suite value comes from elsewhere: the entry function delegates.
        "func TestCalcSuite(t *testing.T) {\n\tsuite.Run(t, shared.NewSuite())\n}\n",
    ] {
        let run = go_suite_file(entry);
        assert_eq!(run.violations(VACUOUS).len(), 0, "{entry}\n{}", run.stdout);
        assert_eq!(run.violations(GATE).len(), 0, "{entry}\n{}", run.stdout);
    }
}

/// Controls: a subtest with no assertion and a suite method with none are still vacuous.
#[test]
fn go_subtest_and_suite_method_with_no_assertion_stay_vacuous() {
    let run = go_suite_file(
        "func TestCalcSuite(t *testing.T) {\n\tt.Run(\"empty\", func(t *testing.T) {\n\t\tAdd(2, 2)\n\t})\n}\n",
    );
    assert_eq!(run.violations(VACUOUS).len(), 1, "{}", run.stdout);
    let run = go_suite_file(
        "func (s *CalcSuite) TestNothing() {\n\tAdd(2, 2)\n}\n\nfunc TestCalcSuite(t *testing.T) {\n\tsuite.Run(t, new(CalcSuite))\n}\n",
    );
    let vacuous = run.violations(VACUOUS);
    assert_eq!(vacuous.len(), 1, "{}", run.stdout);
    assert!(
        vacuous[0]["message"]
            .as_str()
            .unwrap()
            .contains("TestNothing"),
        "{vacuous:?}"
    );
}

// ---------------------------------------------------------------------------
// Python: a handler for a type of the project
// ---------------------------------------------------------------------------

fn py_try(handler: &str) -> String {
    format!("    try:\n        assert f() == 1\n{handler}")
}

/// A handler that swallows a type not known to be unrelated to `AssertionError` is
/// reported, as it is in Java, Kotlin and C#.
#[test]
fn python_swallowing_handler_for_a_project_type_is_reported() {
    PY.check_each(&[
        at(6, py_try("    except CheckFailed:\n        pass\n")),
        at(6, py_try("    except checks.CheckFailed as e:\n        print(e)\n")),
        at(
            6,
            py_try("    except CheckFailed as e:\n        logger.warning(\"failed: %s\", e)\n"),
        ),
        at(
            7,
            "    for case in cases:\n        try:\n            assert f(case) == 1\n        except CheckFailed:\n            continue\n",
        ),
        at(6, py_try("    except (KeyError, CheckFailed):\n        pass\n")),
        at(
            6,
            "    with contextlib.suppress(CheckFailed):\n        assert f() == 1\n",
        ),
    ]);
}

/// Controls: a handler that re-raises, fails, asserts or does anything other than
/// swallow; and a standard type that `AssertionError` is not a subclass of.
#[test]
fn python_handler_that_does_not_swallow_or_names_an_unrelated_type_is_not_reported() {
    PY.check_each(&[
        silent(py_try("    except CheckFailed:\n        raise\n")),
        silent(py_try(
            "    except CheckFailed:\n        pytest.fail(\"no\")\n",
        )),
        silent(py_try(
            "    except CheckFailed as e:\n        assert e.code == 3\n",
        )),
        silent(py_try("    except CheckFailed:\n        recorded = True\n")),
        silent(py_try("    except KeyError:\n        pass\n")),
        silent(py_try("    except OSError:\n        pass\n")),
        silent(py_try(
            "    except (IOError, json.JSONDecodeError):\n        pass\n",
        )),
        silent("    with contextlib.suppress(FileNotFoundError):\n        assert f() == 1\n"),
    ]);
}

/// A class the file declares is read through its bases: one that derives from a standard
/// class beside `AssertionError` is unrelated; one that derives from `AssertionError`, or
/// from a class the file does not declare, may be an assertion failure.
#[test]
fn python_handler_for_a_class_of_the_file_is_read_through_its_bases() {
    let file = |classes: &str| {
        format!(
            "{classes}\n\ndef test_a():\n    try:\n        assert f() == 1\n    except Local:\n        pass\n"
        )
    };
    let base = "def test_a():\n    assert f() == 1\n";
    let reported = |classes: &str| caught_in(&run_files(PY.path, base, &file(classes))).len();
    assert_eq!(reported("class Local(ValueError):\n    pass\n"), 0);
    assert_eq!(reported("class Local(Exception):\n    pass\n"), 0);
    assert_eq!(
        reported("class Mid(RuntimeError):\n    pass\n\n\nclass Local(Mid):\n    pass\n"),
        0
    );
    assert_eq!(reported("class Local(AssertionError):\n    pass\n"), 1);
    assert_eq!(
        reported(
            "class Mid(AssertionError):\n    pass\n\n\nclass Local(Mid, ValueError):\n    pass\n"
        ),
        1
    );
    assert_eq!(reported("class Local(checks.Base):\n    pass\n"), 1);
}

// ---------------------------------------------------------------------------
// An assertion in a callback that runs before the `try` ends
// ---------------------------------------------------------------------------

#[test]
fn assertion_in_a_synchronous_callback_inside_a_swallowing_try_is_reported() {
    JS.check_each(&[
        at(
            3,
            "  try {\n    [4].forEach((v) => expect(add(2, 2)).toBe(v));\n  } catch (e) {}\n",
        ),
        at(
            4,
            "  try {\n    [4].map(function (v) {\n      expect(add(2, 2)).toBe(v);\n    });\n  } catch (e) {}\n",
        ),
        at(
            3,
            "  try {\n    cases.filter(Boolean).every((v) => expect(add(2, 2)).toBe(v));\n  } catch (e) {}\n",
        ),
    ]);
    JAVA.check_each(&[at(
        8,
        "        try {\n            java.util.List.of(4).forEach(v -> assertEquals(v, add(2, 2)));\n        } catch (AssertionError e) {\n        }\n",
    )]);
    KT.check_each(&[
        at(
            8,
            "        try {\n            add(2, 2).let {\n                assertEquals(4, it)\n            }\n        } catch (e: AssertionError) {\n        }\n",
        ),
        at(
            7,
            "        try {\n            listOf(4).forEach { assertEquals(it, add(2, 2)) }\n        } catch (e: AssertionError) {\n        }\n",
        ),
        at(
            7,
            "        try {\n            with(add(2, 2)) { assertEquals(4, this) }\n        } catch (e: AssertionError) {\n        }\n",
        ),
    ]);
    CS.check_each(&[at(
        7,
        "        try {\n            cases.ForEach(v => Assert.AreEqual(v, Add(2, 2)));\n        } catch (Exception) {\n        }\n",
    )]);
}

/// Controls: a callback handed to a function that is not known to run it before the
/// `try` ends is not read.
#[test]
fn assertion_in_any_other_callback_inside_a_try_is_not_reported() {
    JS.check_each(&[
        silent("  try {\n    setTimeout(() => expect(add(2, 2)).toBe(4), 0);\n  } catch (e) {}\n"),
        silent(
            "  try {\n    emitter.on('done', (v) => expect(add(2, 2)).toBe(v));\n  } catch (e) {}\n",
        ),
    ]);
    JAVA.check_each(&[silent(
        "        try {\n            executor.submit(() -> assertEquals(4, add(2, 2)));\n        } catch (AssertionError e) {\n        }\n",
    )]);
    KT.check_each(&[silent(
        "        try {\n            thread { assertEquals(4, add(2, 2)) }\n        } catch (e: AssertionError) {\n        }\n",
    )]);
    CS.check_each(&[silent(
        "        try {\n            Task.Run(() => Assert.AreEqual(4, Add(2, 2)));\n        } catch (Exception) {\n        }\n",
    )]);
}

// ---------------------------------------------------------------------------
// JavaScript: a promise handler named in the file
// ---------------------------------------------------------------------------

const JS_THEN: &str = "  return load().then((v) => expect(v).toBe(4))";

#[test]
fn javascript_named_promise_handler_is_judged_by_its_body() {
    JS.check_each(&[
        at(2, format!("{JS_THEN}.catch(ignore);\n")),
        at(2, format!("{JS_THEN}.catch(quiet);\n")),
        silent(format!("{JS_THEN}.catch(rethrow);\n")),
        // Not declared in the file: not read.
        silent(format!("{JS_THEN}.catch(done);\n")),
        silent(format!("{JS_THEN}.catch(handlers.ignore);\n")),
    ]);
}

// ---------------------------------------------------------------------------
// A `finally` that returns
// ---------------------------------------------------------------------------

#[test]
fn finally_that_returns_discards_the_failure_and_is_reported() {
    JAVA.check_each(&[
        at(
            8,
            "        try {\n            assertEquals(4, add(2, 2));\n        } finally {\n            return;\n        }\n",
        ),
        // A handler that rethrows does not help: the `return` discards that too.
        at(
            8,
            "        try {\n            assertEquals(4, add(2, 2));\n        } catch (AssertionError e) {\n            throw e;\n        } finally {\n            return;\n        }\n",
        ),
        silent(
            "        try {\n            assertEquals(4, add(2, 2));\n        } finally {\n            cleanup();\n        }\n",
        ),
        // A `return` in a lambda of the `finally` block is not one of the block.
        silent(
            "        try {\n            assertEquals(4, add(2, 2));\n        } finally {\n            later(() -> { return; });\n        }\n",
        ),
    ]);
    KT.check_each(&[
        at(
            7,
            "        try {\n            assertEquals(4, add(2, 2))\n        } finally {\n            return\n        }\n",
        ),
        at(
            7,
            "        try {\n            assertEquals(4, add(2, 2))\n        } catch (e: AssertionError) {\n            throw e\n        } finally {\n            return\n        }\n",
        ),
        silent(
            "        try {\n            assertEquals(4, add(2, 2))\n        } finally {\n            cleanup()\n        }\n",
        ),
        silent(
            "        try {\n            assertEquals(4, add(2, 2))\n        } finally {\n            later {\n                return@later\n            }\n        }\n",
        ),
    ]);
    JS.check_each(&[
        at(
            3,
            "  try {\n    expect(add(2, 2)).toBe(4);\n  } finally {\n    return;\n  }\n",
        ),
        silent("  try {\n    expect(add(2, 2)).toBe(4);\n  } finally {\n    cleanup();\n  }\n"),
    ]);
    PY.check_each(&[
        at(
            6,
            "    try:\n        assert f() == 1\n    finally:\n        return\n",
        ),
        silent("    try:\n        assert f() == 1\n    finally:\n        cleanup()\n"),
    ]);
}

// ---------------------------------------------------------------------------
// Rust: which branch of a `catch_unwind` result fails
// ---------------------------------------------------------------------------

const UNWIND: &str = "    let r = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n";

/// The result is looked at when the branch taken on a failure is the one that fails.
#[test]
fn rust_unwind_result_is_checked_only_when_the_failure_branch_fails() {
    let with = |rest: &str| format!("{UNWIND}{rest}");
    RS.check_each(&[
        silent(with(
            "    if r.is_err() {\n        panic!(\"failed\");\n    }\n",
        )),
        silent(with(
            "    if r.is_ok() {\n        done();\n    } else {\n        panic!(\"failed\");\n    }\n",
        )),
        silent(with(
            "    if !r.is_ok() {\n        panic!(\"failed\");\n    }\n",
        )),
        silent(with(
            "    if let Err(e) = r {\n        std::panic::resume_unwind(e);\n    }\n",
        )),
        silent(with(
            "    match r {\n        Ok(()) => {}\n        Err(e) => std::panic::resume_unwind(e),\n    }\n",
        )),
        silent(with(
            "    match r {\n        Ok(()) => {}\n        _ => panic!(\"failed\"),\n    }\n",
        )),
        // The branch that fails is the one taken when the assertion held.
        at(
            3,
            with("    if r.is_ok() {\n        panic!(\"should have panicked\");\n    }\n"),
        ),
        at(
            3,
            with("    if r.is_err() {\n        done();\n    } else {\n        panic!(\"should have panicked\");\n    }\n"),
        ),
        at(
            3,
            with("    if let Ok(()) = r {\n        panic!(\"should have panicked\");\n    }\n"),
        ),
        at(
            3,
            with("    match r {\n        Ok(()) => panic!(\"should have panicked\"),\n        Err(_) => {}\n    }\n"),
        ),
    ]);
}

// ---------------------------------------------------------------------------
// C#: NUnit calls that end the test without failing it
// ---------------------------------------------------------------------------

#[test]
fn nunit_calls_that_do_not_fail_the_test_leave_the_handler_swallowing() {
    let try_with = |handler: &str| {
        format!(
            "        try {{\n            Assert.AreEqual(4, Add(2, 2));\n        }} catch (Exception) {{\n            {handler}\n        }}\n"
        )
    };
    CS.check_each(&[
        at(7, try_with("Assert.Pass();")),
        at(7, try_with("Assert.Ignore(\"flaky\");")),
        at(7, try_with("Assert.Inconclusive(\"flaky\");")),
        silent(try_with("Assert.Fail(\"no\");")),
        silent(try_with("Assert.IsNotNull(state);")),
    ]);
}

// ---------------------------------------------------------------------------
// Configured helper names
// ---------------------------------------------------------------------------

/// A helper listed in `assert_helper_fns` is an assertion to the counters, and to the
/// caught-assertion detector too.
#[test]
fn configured_helper_inside_a_swallowing_handler_is_reported() {
    let config = format!(
        "{CONFIG_HEAD}\n[gates.assertion-reduction]\nassert_helper_fns = [\"check_total\", \"checkTotal\", \"CheckTotal\"]\n"
    );
    let cases: [(&Lang, &str, &str, u64); 6] = [
        (
            &PY,
            "    check_total(f())\n",
            "    try:\n        check_total(f())\n    except AssertionError:\n        pass\n",
            6,
        ),
        (
            &JS,
            "  checkTotal(add(2, 2));\n",
            "  try {\n    checkTotal(add(2, 2));\n  } catch (e) {}\n",
            3,
        ),
        (
            &JAVA,
            "        checkTotal(add(2, 2));\n",
            "        try {\n            checkTotal(add(2, 2));\n        } catch (AssertionError e) {\n        }\n",
            8,
        ),
        (
            &KT,
            "        checkTotal(add(2, 2))\n",
            "        try {\n            checkTotal(add(2, 2))\n        } catch (e: AssertionError) {\n        }\n",
            7,
        ),
        (
            &CS,
            "        CheckTotal(Add(2, 2));\n",
            "        try {\n            CheckTotal(Add(2, 2));\n        } catch (Exception) {\n        }\n",
            7,
        ),
        (
            &RS,
            "    check_total(add(2, 2));\n",
            "    let _ = std::panic::catch_unwind(|| check_total(add(2, 2)));\n",
            3,
        ),
    ];
    let mut wrong = Vec::new();
    for (lang, plain, wrapped, line) in cases {
        let (base, head) = (lang.file(plain), lang.file(wrapped));
        let got = caught_in(&run_with(&config, lang.path, &base, &head));
        if got != vec![line] {
            wrong.push(format!("{}: configured, reported {got:?}", lang.path));
        }
        // Control: the name is not configured, so the call is no assertion.
        let got = caught_in(&run_with(CONFIG_HEAD, lang.path, &base, &head));
        if !got.is_empty() {
            wrong.push(format!("{}: not configured, reported {got:?}", lang.path));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

// ---------------------------------------------------------------------------
// Receiver calls
// ---------------------------------------------------------------------------

/// A test the change adds whose only check is a method called on a receiver: `vacuous`
/// findings for the file `path` holding `src`.
fn vacuous_in_added(path: &str, src: &str) -> usize {
    let repo = Repo::new();
    repo.write(path, src);
    repo.commit("test: add");
    repo.check(&[]).violations(VACUOUS).len()
}

#[test]
fn receiver_call_is_followed_into_a_member_function_defined_in_its_class() {
    let file = |body: &str| {
        format!(
            "#include <gtest/gtest.h>\n\nstruct Checker {{\n  int n;\n  void done() {{\n{body}  }}\n}};\n\nTEST(Api, Create) {{\n  Checker c = Make();\n  c.done();\n}}\n"
        )
    };
    assert_eq!(
        vacuous_in_added("tests/api_test.cc", &file("    EXPECT_EQ(n, 1);\n")),
        0
    );
    assert_eq!(
        vacuous_in_added("tests/api_test.cc", &file("    Log(n);\n")),
        1
    );
}

#[test]
fn receiver_call_inside_a_macro_argument_is_followed() {
    let file = |body: &str| {
        format!(
            "struct A(Vec<u8>);\n\nimpl A {{\n    fn done(&self) -> usize {{\n{body}        self.0.len()\n    }}\n}}\n\n#[test]\nfn created() {{\n    let v = make();\n    println!(\"{{}}\", v.done());\n}}\n"
        )
    };
    assert_eq!(
        vacuous_in_added(
            "tests/t.rs",
            &file("        assert_eq!(self.0.capacity(), 0);\n")
        ),
        0
    );
    assert_eq!(
        vacuous_in_added("tests/t.rs", &file("        log(&self.0);\n")),
        1
    );
}

#[test]
fn receiver_call_without_parentheses_is_followed() {
    let file = |body: &str| {
        format!(
            "class Checker(r: R) {{\n  def done: Unit = {{\n{body}  }}\n}}\n\nclass ApiSpec extends AnyFunSuite {{\n  test(\"create\") {{\n    val c = new Checker(make())\n    c.done\n  }}\n}}\n"
        )
    };
    let path = "src/test/scala/ApiSpec.scala";
    assert_eq!(vacuous_in_added(path, &file("    assert(r.f1 == 1)\n")), 0);
    assert_eq!(vacuous_in_added(path, &file("    log(r)\n")), 1);
}
