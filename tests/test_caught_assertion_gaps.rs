//! `assertion-reduction/assertion-failure-caught`, handler by handler (#569): which handlers
//! around an assertion keep the test from failing, and which do not.
//!
//! Every test wraps the one assertion of an existing test and reads the lines reported.
//! The sources below are fixtures: text handed to the binary, never code of this repository.
//! A test named `pinned_...` records behaviour that is left as it is, with the reason.

mod common;
use common::{Repo, Run};

const GATE: &str = "assertion-reduction";
const CAUGHT: &str = "Assertion Failure Caught Inside Test";

/// One language's fixture: the file, the text around a test body, and the unwrapped body.
struct Lang {
    path: &'static str,
    before: &'static str,
    after: &'static str,
    plain: &'static str,
}

/// Body starts on line 2.
const PY: Lang = Lang {
    path: "tests/test_t.py",
    before: "def test_a():\n",
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
    after: "});\n",
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
    before: "using Xunit;\n\npublic class ATests {\n    [Fact]\n    public void Adds() {\n",
    after: "    }\n}\n",
    plain: "        Assert.Equal(4, Add(2, 2));\n",
};
/// Body starts on line 6.
const GO: Lang = Lang {
    path: "p_test.go",
    before: "package p\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n",
    after: "}\n",
    plain: "\trequire.Equal(t, 4, add(2, 2))\n",
};

fn run_files(lang: &Lang, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (lang.path, base),
            ("package.json", "{\"devDependencies\": {\"jest\": \"*\"}}\n"),
        ],
        "test: base",
    );
    repo.write(lang.path, head);
    repo.commit("test: change");
    repo.check(&[])
}

/// The base holds the plain assertion; the head holds `body` in its place.
fn run(lang: &Lang, body: &str) -> Run {
    run_files(
        lang,
        &format!("{}{}{}", lang.before, lang.plain, lang.after),
        &format!("{}{}{}", lang.before, body, lang.after),
    )
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

fn caught(lang: &Lang, body: &str) -> Vec<u64> {
    caught_in(&run(lang, body))
}

const NONE: Vec<u64> = Vec::new();

/// Runs every `(body, lines expected)` and fails once, listing each body that differs.
fn check_each(lang: &Lang, cases: &[(String, Vec<u64>)]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(body, want)| {
            let got = caught(lang, body);
            (got != *want).then(|| format!("expected {want:?}, reported {got:?}:\n{body}"))
        })
        .collect();
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

fn silent(body: impl Into<String>) -> (String, Vec<u64>) {
    (body.into(), NONE)
}

fn at(line: u64, body: impl Into<String>) -> (String, Vec<u64>) {
    (body.into(), vec![line])
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

const PY_COLLECT: &str = "    errors = []\n    try:\n        assert f() == 1\n    except AssertionError as e:\n        errors.append(e)\n";

#[test]
fn python_soft_assertions_collected_and_asserted_after_are_not_swallowed() {
    check_each(
        &PY,
        &[
            silent(format!("{PY_COLLECT}    assert not errors\n")),
            silent(format!(
                "{PY_COLLECT}    if errors:\n        raise AssertionError(errors)\n"
            )),
        ],
    );
}

#[test]
fn python_soft_assertions_collected_and_never_read_stay_reported() {
    check_each(
        &PY,
        &[
            at(4, PY_COLLECT),
            // Another list is asserted on.
            at(4, format!("{PY_COLLECT}    assert not others\n")),
            // The error is printed, not kept.
            at(
                3,
                "    try:\n        assert f() == 1\n    except AssertionError as e:\n        print(e)\n    assert not errors\n",
            ),
        ],
    );
}

const PY_RETRY_BREAK: &str = "    for _ in range(3):\n        try:\n            assert f() == 1\n            break\n        except AssertionError:\n            backoff()\n";
const PY_RETRY_RETURN: &str = "    for _ in range(3):\n        try:\n            assert f() == 1\n            return\n        except AssertionError:\n            continue\n";

#[test]
fn python_retry_loop_that_fails_after_its_last_attempt_is_not_swallowed() {
    check_each(
        &PY,
        &[
            silent(format!(
                "{PY_RETRY_BREAK}    else:\n        pytest.fail(\"never\")\n"
            )),
            silent(format!(
                "{PY_RETRY_BREAK}    else:\n        raise AssertionError(\"never\")\n"
            )),
            silent(format!("{PY_RETRY_RETURN}    pytest.fail(\"never\")\n")),
        ],
    );
}

#[test]
fn python_retry_loop_with_no_failure_after_it_stays_reported() {
    check_each(
        &PY,
        &[
            at(4, PY_RETRY_BREAK),
            at(4, PY_RETRY_RETURN),
            // A `break` leaves the loop for the statement after it, so a failure placed
            // there is not the retry's failure exit; a plain statement is not one either.
            at(4, format!("{PY_RETRY_BREAK}    print(\"done\")\n")),
            // The same handler outside a loop.
            at(
                3,
                "    try:\n        assert f() == 1\n    except AssertionError:\n        backoff()\n    else:\n        pytest.fail(\"never\")\n",
            ),
        ],
    );
}

fn py_try(handler: &str) -> String {
    format!("    try:\n        assert f() == 1\n{handler}")
}

#[test]
fn python_handlers_that_fail_the_test_or_catch_another_type_are_not_reported() {
    check_each(
        &PY,
        &[
            silent(py_try("    except AssertionError:\n        raise\n")),
            silent(py_try(
                "    except AssertionError:\n        pytest.fail(\"no\")\n",
            )),
            silent(py_try("    except ValueError:\n        pass\n")),
            silent(py_try(
                "    except (\n        ValueError,\n        KeyError,\n    ):\n        pass\n",
            )),
            silent(py_try("    except* ValueError:\n        pass\n")),
            silent(py_try("    finally:\n        cleanup()\n")),
        ],
    );
}

#[test]
fn python_plainly_swallowing_handlers_stay_reported() {
    check_each(
        &PY,
        &[
            at(3, py_try("    except AssertionError:\n        pass\n")),
            at(3, py_try("    except Exception as e:\n        print(e)\n")),
            at(3, py_try("    except BaseException:\n        pass\n")),
            at(3, py_try("    except:\n        pass\n")),
            at(
                3,
                py_try("    except (ValueError, AssertionError):\n        pass\n"),
            ),
        ],
    );
}

#[test]
fn python_multi_line_except_tuple_is_read() {
    check_each(
        &PY,
        &[at(
            3,
            py_try("    except (\n        ValueError,\n        AssertionError,\n    ):\n        pass\n"),
        )],
    );
}

#[test]
fn python_except_star_is_read() {
    check_each(
        &PY,
        &[at(3, py_try("    except* AssertionError:\n        pass\n"))],
    );
}

#[test]
fn python_contextlib_suppress_around_an_assertion_is_reported() {
    check_each(
        &PY,
        &[
            at(
                3,
                "    with contextlib.suppress(AssertionError):\n        assert f() == 1\n",
            ),
            at(
                3,
                "    with suppress(KeyError, Exception):\n        assert f() == 1\n",
            ),
        ],
    );
}

#[test]
fn python_context_managers_that_do_not_suppress_an_assertion_failure_are_not_reported() {
    check_each(
        &PY,
        &[
            silent("    with contextlib.suppress(KeyError):\n        assert f() == 1\n"),
            silent("    with open(p) as fh:\n        assert f() == 1\n"),
        ],
    );
}

/// Left as it is: a class the file does not explain is read as unrelated to
/// `AssertionError` in Python, while Java, Kotlin and C# report a type they do not know.
#[test]
fn pinned_python_handler_for_a_user_defined_type_is_not_reported() {
    check_each(
        &PY,
        &[silent(py_try("    except CheckFailed:\n        pass\n"))],
    );
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

const UNWIND: &str = "    let r = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n";

// Fixture lines kept outside the test bodies: they are source text of the repository
// under test, not checks these tests make.
const RESULT_ASSERTED: &str = "    assert!(r.is_err());\n";
const RESULT_BRANCHED: &str =
    "    if r.is_ok() {\n        panic!(\"should have panicked\");\n    }\n";
const OTHER_BINDING_ASSERTED: &str =
    "    let other_r: Result<(), ()> = Ok(());\n    assert!(other_r.is_ok());\n";

#[test]
fn rust_catch_unwind_result_that_is_checked_is_not_reported() {
    check_each(
        &RS,
        &[
            silent(format!("{UNWIND}{RESULT_ASSERTED}")),
            silent(format!("{UNWIND}    assert!(matches!(r, Err(_)));\n")),
            silent(format!("{UNWIND}    r.unwrap();\n")),
            silent(format!("{UNWIND}    r.expect(\"must not panic\");\n")),
            silent(format!("{UNWIND}    let _payload = r.unwrap_err();\n")),
            silent(format!(
                "{UNWIND}    if let Err(e) = r {{\n        std::panic::resume_unwind(e);\n    }}\n"
            )),
            silent(format!(
                "{UNWIND}    match r {{\n        Ok(()) => panic!(\"should have panicked\"),\n        Err(_) => {{}}\n    }}\n"
            )),
            silent(format!("{UNWIND}{RESULT_BRANCHED}")),
            silent(
                "    let r = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4)).is_err();\n    assert!(r);\n",
            ),
            silent("    std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4)).unwrap();\n"),
        ],
    );
}

#[test]
fn rust_catch_unwind_result_returned_from_the_test_is_not_reported() {
    let base = format!("{}{}{}", RS.before, RS.plain, RS.after);
    for head in [
        "#[test]\nfn adds() -> std::thread::Result<()> {\n    std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4))\n}\n",
        "#[test]\nfn adds() -> std::thread::Result<()> {\n    std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4))?;\n    Ok(())\n}\n",
        "#[test]\nfn adds() -> std::thread::Result<()> {\n    let r = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n    r\n}\n",
    ] {
        assert_eq!(caught_in(&run_files(&RS, &base, head)), NONE, "{head}");
    }
}

#[test]
fn rust_catch_unwind_result_that_is_dropped_stays_reported() {
    check_each(
        &RS,
        &[
            at(3, "    let _ = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n"),
            at(3, "    _ = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n"),
            at(3, "    std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4)).ok();\n"),
            at(3, "    let _r = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n"),
            at(3, format!("{UNWIND}    let _ = r;\n")),
            at(3, format!("{UNWIND}    drop(r);\n")),
            at(
                3,
                format!("{UNWIND}    match r {{\n        Ok(()) => {{}}\n        Err(_) => {{}}\n    }}\n"),
            ),
            at(
                3,
                format!("{UNWIND}    if let Err(e) = r {{\n        eprintln!(\"{{e:?}}\");\n    }}\n"),
            ),
        ],
    );
}

#[test]
fn rust_a_comment_naming_the_check_does_not_count_as_the_check() {
    check_each(
        &RS,
        &[at(
            3,
            format!("{UNWIND}    // r.is_err() is not looked at\n"),
        )],
    );
}

#[test]
fn rust_a_check_on_another_binding_does_not_count() {
    check_each(&RS, &[at(3, format!("{UNWIND}{OTHER_BINDING_ASSERTED}"))]);
}

const WRAPPED: &str =
    "std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert_eq!(add(2, 2), 4)))";

#[test]
fn rust_closure_inside_assert_unwind_safe_is_read() {
    check_each(&RS, &[at(3, format!("    let _ = {WRAPPED};\n"))]);
}

#[test]
fn rust_closure_inside_assert_unwind_safe_with_its_result_asserted_is_not_reported() {
    check_each(
        &RS,
        &[silent(format!(
            "    let r = {WRAPPED};\n    assert!(r.is_err());\n"
        ))],
    );
}

// ---------------------------------------------------------------------------
// JavaScript / TypeScript
// ---------------------------------------------------------------------------

#[test]
fn js_catch_that_hands_the_error_to_done_or_reject_is_not_reported() {
    check_each(
        &JS,
        &[
            silent("  try {\n    expect(add(2, 2)).toBe(4);\n    done();\n  } catch (e) {\n    done(e);\n  }\n"),
            silent("  return new Promise((resolve, reject) => {\n    try {\n      expect(add(2, 2)).toBe(4);\n      resolve();\n    } catch (e) {\n      reject(e);\n    }\n  });\n"),
        ],
    );
}

#[test]
fn js_catch_that_reports_success_stays_reported() {
    check_each(
        &JS,
        &[
            // `done()` with no argument reports success, and so does `resolve()`.
            at(3, "  try {\n    expect(add(2, 2)).toBe(4);\n  } catch (e) {\n    done();\n  }\n"),
            at(4, "  return new Promise((resolve, reject) => {\n    try {\n      expect(add(2, 2)).toBe(4);\n    } catch (e) {\n      resolve();\n    }\n  });\n"),
            at(3, "  try {\n    expect(add(2, 2)).toBe(4);\n  } catch (e) {}\n"),
            at(3, "  try {\n    expect(add(2, 2)).toBe(4);\n  } catch {\n    console.log(\"ignored\");\n  }\n"),
        ],
    );
}

#[test]
fn js_handlers_that_rethrow_or_only_clean_up_are_not_reported() {
    check_each(
        &JS,
        &[
            silent("  try {\n    expect(add(2, 2)).toBe(4);\n  } catch (e) {\n    throw e;\n  }\n"),
            silent("  try {\n    expect(add(2, 2)).toBe(4);\n  } finally {\n    cleanup();\n  }\n"),
        ],
    );
}

const JS_THEN: &str = "  return load().then((v) => expect(v).toBe(4))";

#[test]
fn js_promise_catch_that_discards_the_rejection_is_reported() {
    check_each(
        &JS,
        &[
            at(2, format!("{JS_THEN}.catch(() => {{}});\n")),
            at(2, format!("{JS_THEN}.catch(function () {{}});\n")),
            at(2, format!("{JS_THEN}.catch((e) => console.log(e));\n")),
            at(
                2,
                "  return expect(load()).resolves.toBe(4).catch(() => {});\n",
            ),
        ],
    );
}

#[test]
fn js_promise_catch_that_fails_the_test_or_follows_no_assertion_is_not_reported() {
    check_each(
        &JS,
        &[
            silent(format!("{JS_THEN}.catch((e) => {{ throw e; }});\n")),
            silent(format!("{JS_THEN}.catch((e) => done(e));\n")),
            silent(format!("{JS_THEN}.catch(done);\n")),
            silent("  expect(add(2, 2)).toBe(4);\n  return load().catch(() => {});\n"),
        ],
    );
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

fn java_try(catch: &str) -> String {
    format!("        try {{\n            assertEquals(4, add(2, 2));\n        }} {catch}\n")
}

#[test]
fn java_catch_of_a_type_an_assertion_failure_is_not_is_not_reported() {
    check_each(
        &JAVA,
        &[
            silent(java_try("catch (InterruptedException e) {\n        }")),
            silent(java_try("catch (Exception e) {\n        }")),
            silent(java_try("catch (RuntimeException e) {\n        }")),
            silent(java_try("catch (java.io.IOException myError) {\n        }")),
            silent(java_try(
                "catch (IllegalStateException | java.io.IOException e) {\n        }",
            )),
            silent(java_try("catch (OutOfMemoryError e) {\n        }")),
        ],
    );
}

#[test]
fn java_catch_of_an_assertion_failure_type_stays_reported() {
    check_each(
        &JAVA,
        &[
            at(8, java_try("catch (AssertionError e) {\n        }")),
            at(8, java_try("catch (Throwable t) {\n        }")),
            at(8, java_try("catch (Error e) {\n        }")),
            at(8, java_try("catch (final java.lang.AssertionError e) {\n        }")),
            at(8, java_try("catch (IllegalStateException | AssertionError e) {\n        }")),
            at(8, java_try("catch (org.opentest4j.AssertionFailedError e) {\n        }")),
            at(8, java_try("catch (ComparisonFailure e) {\n        }")),
            at(
                8,
                java_try("catch (AssertionError e) {\n            System.out.println(e);\n        }"),
            ),
            at(
                8,
                java_try("catch (java.io.IOException e) {\n            throw e;\n        } catch (AssertionError e) {\n        }"),
            ),
        ],
    );
}

/// The rule for a type that is neither a known ancestor of an assertion failure nor a
/// known unrelated type: reported.
#[test]
fn java_catch_of_a_type_that_is_not_known_is_reported() {
    check_each(
        &JAVA,
        &[
            at(8, java_try("catch (CheckFailure e) {\n        }")),
            at(
                8,
                java_try("catch (com.acme.CheckException e) {\n        }"),
            ),
        ],
    );
}

#[test]
fn java_handlers_that_fail_the_test_or_only_clean_up_are_not_reported() {
    check_each(
        &JAVA,
        &[
            silent(java_try("catch (AssertionError e) {\n            throw e;\n        }")),
            silent(java_try("catch (AssertionError e) {\n            fail(\"no\");\n        }")),
            silent(java_try(
                "catch (AssertionError e) {\n            throw new IllegalStateException(e);\n        }",
            )),
            silent(java_try("finally {\n            cleanup();\n        }")),
        ],
    );
}

/// The first handler that catches the failure is the one that runs: a later, broader one
/// never sees an assertion failure the first one rethrew.
#[test]
fn java_a_later_handler_does_not_swallow_what_an_earlier_one_rethrows() {
    check_each(
        &JAVA,
        &[silent(java_try(
            "catch (AssertionError e) {\n            throw e;\n        } catch (Throwable t) {\n        }",
        ))],
    );
}

fn java_with(catch: &str) -> String {
    format!(
        "        try (AutoCloseable r = open()) {{\n            assertEquals(4, add(2, 2));\n        }} {catch}\n"
    )
}

#[test]
fn java_try_with_resources_is_read() {
    check_each(
        &JAVA,
        &[at(8, java_with("catch (AssertionError e) {\n        }"))],
    );
}

#[test]
fn java_try_with_resources_whose_handler_does_not_swallow_is_not_reported() {
    check_each(
        &JAVA,
        &[
            silent(java_with(
                "catch (AssertionError e) {\n            throw e;\n        }",
            )),
            silent(java_with("catch (java.io.IOException e) {\n        }")),
            silent("        try (AutoCloseable r = open()) {\n            assertEquals(4, add(2, 2));\n        }\n"),
        ],
    );
}

#[test]
fn java_swallowing_handler_inside_a_lambda_of_the_test_is_reported() {
    check_each(
        &JAVA,
        &[at(
            9,
            "        Runnable check = () -> {\n            try {\n                assertEquals(4, add(2, 2));\n            } catch (AssertionError e) {\n            }\n        };\n        check.run();\n",
        )],
    );
}

// ---------------------------------------------------------------------------
// Kotlin
// ---------------------------------------------------------------------------

fn kt_try(catch: &str) -> String {
    format!("        try {{\n            assertEquals(4, add(2, 2))\n        }} {catch}\n")
}

#[test]
fn kotlin_catch_of_an_assertion_failure_type_is_reported() {
    check_each(
        &KT,
        &[
            at(7, kt_try("catch (e: AssertionError) {\n        }")),
            at(7, kt_try("catch (e: Throwable) {\n        }")),
            at(7, kt_try("catch (e: Error) {\n        }")),
            at(
                7,
                kt_try("catch (e: java.lang.AssertionError) {\n        }"),
            ),
            at(
                7,
                kt_try("catch (_: AssertionError) {\n            println(\"ignored\")\n        }"),
            ),
            at(7, kt_try("catch (e: CheckFailure) {\n        }")),
        ],
    );
}

#[test]
fn kotlin_handlers_that_do_not_swallow_an_assertion_failure_are_not_reported() {
    check_each(
        &KT,
        &[
            silent(kt_try("catch (e: Exception) {\n        }")),
            silent(kt_try("catch (e: java.io.IOException) {\n        }")),
            silent(kt_try("catch (e: AssertionError) {\n            throw e\n        }")),
            silent(kt_try("catch (e: AssertionError) {\n            fail(\"no\")\n        }")),
            silent(kt_try(
                "catch (e: AssertionError) {\n            assertTrue(e.message!!.contains(\"4\"))\n        }",
            )),
            silent(kt_try("finally {\n            cleanup()\n        }")),
        ],
    );
}

const KT_BLOCK: &str = "runCatching {\n            assertEquals(4, add(2, 2))\n        }";

#[test]
fn kotlin_run_catching_with_the_result_unused_is_reported() {
    check_each(
        &KT,
        &[
            at(7, format!("        {KT_BLOCK}\n")),
            at(7, format!("        {KT_BLOCK}.getOrNull()\n")),
            at(7, format!("        val r = {KT_BLOCK}\n")),
            at(
                7,
                format!("        {KT_BLOCK}.onFailure {{ println(it) }}\n"),
            ),
        ],
    );
}

#[test]
fn kotlin_run_catching_with_the_result_checked_is_not_reported() {
    check_each(
        &KT,
        &[
            silent(format!("        {KT_BLOCK}.getOrThrow()\n")),
            silent(format!("        {KT_BLOCK}.onFailure {{ throw it }}\n")),
            silent(format!("        {KT_BLOCK}.onFailure {{ fail(\"no\") }}\n")),
            silent(format!(
                "        val r = {KT_BLOCK}\n        assertTrue(r.isFailure)\n"
            )),
        ],
    );
}

// ---------------------------------------------------------------------------
// C#
// ---------------------------------------------------------------------------

fn cs_try(catch: &str) -> String {
    format!("        try {{\n            Assert.Equal(4, Add(2, 2));\n        }} {catch}\n")
}

#[test]
fn csharp_catch_of_a_type_an_assertion_failure_is_not_is_not_reported() {
    check_each(
        &CS,
        &[
            silent(cs_try("catch (IOException) {\n        }")),
            silent(cs_try("catch (System.IO.IOException e) {\n        }")),
            silent(cs_try("catch (InvalidOperationException e) {\n        }")),
        ],
    );
}

#[test]
fn csharp_a_string_that_mentions_an_assertion_is_not_an_assertion() {
    check_each(
        &CS,
        &[silent(
            "        Assert.Equal(4, Add(2, 2));\n        try {\n            Console.WriteLine(\"Assert.Equal done\");\n        } catch (Exception) {\n        }\n",
        )],
    );
}

#[test]
fn csharp_a_handler_that_prints_the_words_assert_fail_still_swallows() {
    check_each(
        &CS,
        &[at(
            7,
            cs_try("catch (Exception e) {\n            Console.WriteLine(\"Assert.Fail \" + e);\n        }"),
        )],
    );
}

#[test]
fn csharp_catch_of_an_assertion_failure_type_stays_reported() {
    check_each(
        &CS,
        &[
            at(7, cs_try("catch (Exception) {\n        }")),
            at(7, cs_try("catch {\n        }")),
            at(7, cs_try("catch (Exception e) when (e.Message != null) {\n        }")),
            at(7, cs_try("catch when (Flaky()) {\n        }")),
            // A filtered handler that rethrows may not run, so the next one is read.
            at(
                7,
                cs_try("catch (Exception e) when (e.Message != null) {\n            throw;\n        } catch {\n        }"),
            ),
            at(7, cs_try("catch (Xunit.Sdk.XunitException) {\n        }")),
            at(7, cs_try("catch (AssertionException) {\n        }")),
            at(7, cs_try("catch (AssertFailedException e) {\n        }")),
            at(
                7,
                cs_try("catch (IOException) {\n            throw;\n        } catch (Exception) {\n        }"),
            ),
        ],
    );
}

/// The rule for a type that is neither a known ancestor of an assertion failure nor a
/// known unrelated type: reported.
#[test]
fn csharp_catch_of_a_type_that_is_not_known_is_reported() {
    check_each(&CS, &[at(7, cs_try("catch (CheckFailure e) {\n        }"))]);
}

#[test]
fn csharp_handlers_that_fail_the_test_or_only_clean_up_are_not_reported() {
    check_each(
        &CS,
        &[
            silent(cs_try("catch (Exception) {\n            throw;\n        }")),
            silent(cs_try(
                "catch (Exception e) {\n            Assert.Fail(e.Message);\n        }",
            )),
            silent(cs_try("finally {\n            Cleanup();\n        }")),
            // The first handler that catches the failure rethrows it.
            silent(cs_try(
                "catch (Exception) {\n            throw;\n        } catch {\n        }",
            )),
        ],
    );
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn go_defer(handler: &str) -> String {
    format!("\tdefer func() {{\n{handler}\t}}()\n\trequire.Equal(t, 4, add(2, 2))\n")
}

fn go_recovered(action: &str) -> String {
    go_defer(&format!(
        "\t\tif r := recover(); r != nil {{\n\t\t\t{action}\n\t\t}}\n"
    ))
}

/// Does not depend on how `recover()` is read: a deferred function that fails the test is
/// not a swallowing handler under either reading.
#[test]
fn go_deferred_function_that_fails_the_test_is_not_reported() {
    check_each(
        &GO,
        &[
            silent(go_recovered("t.Fail()")),
            silent(go_recovered("require.Fail(t, \"panicked\")")),
            silent(go_recovered("assert.Fail(t, \"panicked\")")),
            silent(go_recovered("assert.Failf(t, \"panicked\", \"%v\", r)")),
        ],
    );
}

/// Pending the decision on `recover()`: `require` ends the test through `t.FailNow`, which
/// `recover()` does not intercept, and it is reported all the same. So is a deferred
/// function that only logs.
#[test]
fn pinned_go_require_after_a_recovering_defer_is_reported() {
    check_each(
        &GO,
        &[
            at(9, go_defer("\t\t_ = recover()\n")),
            at(11, go_recovered("log.Println(r)")),
        ],
    );
}

/// Pending the decision on `recover()`: `assert` marks the test failed through `t.Errorf`
/// and never panics, and it is reported all the same.
#[test]
fn pinned_go_assert_after_a_recovering_defer_is_reported() {
    let test = |body: &str| format!("{}{}{}", GO.before, body, GO.after);
    let run = run_files(
        &GO,
        &test("\tassert.Equal(t, 4, add(2, 2))\n"),
        &test("\tdefer func() {\n\t\t_ = recover()\n\t}()\n\tassert.Equal(t, 4, add(2, 2))\n"),
    );
    assert_eq!(caught_in(&run), vec![9]);
}

/// Pending the decision on `recover()`: handlers that already read as failing the test.
#[test]
fn pinned_go_recovering_defer_that_calls_fatal_or_panics_is_not_reported() {
    check_each(
        &GO,
        &[
            silent(go_recovered("t.Fatal(r)")),
            silent(go_recovered("t.Errorf(\"%v\", r)")),
            silent(go_recovered("panic(r)")),
            silent(go_recovered("t.FailNow()")),
        ],
    );
}

/// Pending the decision on `recover()`: `err.Error()` in the deferred function reads as a
/// call that fails the test, because the callee text contains `.Error`.
#[test]
fn pinned_go_recovering_defer_that_formats_an_error_is_not_reported() {
    check_each(
        &GO,
        &[silent(go_recovered("log.Println(r.(error).Error())"))],
    );
}

/// Pending the decision on `recover()`: the assertions after a recovering `defer` leave
/// the effective count, so a test the change adds with one is also reported as vacuous.
#[test]
fn pinned_go_added_test_with_a_recovering_defer_reads_as_vacuous() {
    let base = format!("{}{}{}", GO.before, GO.plain, GO.after);
    let added = format!(
        "{base}\nfunc TestSub(t *testing.T) {{\n\tdefer func() {{\n\t\t_ = recover()\n\t}}()\n\trequire.Equal(t, 0, sub(2, 2))\n}}\n"
    );
    let run = run_files(&GO, &base, &added);
    assert_eq!(run.titles(GATE), vec![CAUGHT.to_string()], "{}", run.stdout);
    assert_eq!(run.violations("vacuous-tests").len(), 1, "{}", run.stdout);
}

// ---------------------------------------------------------------------------
// Left as they are (each needs a decision; the reason is on the test)
// ---------------------------------------------------------------------------

/// Left: an assertion inside a callback in the `try` is not read, because whether the
/// callback runs before the `try` ends depends on the function it is passed to.
#[test]
fn pinned_assertion_in_a_callback_inside_the_try_is_not_reported() {
    check_each(
        &JS,
        &[silent(
            "  try {\n    [4].forEach((v) => expect(add(2, 2)).toBe(v));\n  } catch (e) {}\n",
        )],
    );
    check_each(
        &JAVA,
        &[silent(
            "        try {\n            java.util.List.of(4).forEach(v -> assertEquals(v, add(2, 2)));\n        } catch (AssertionError e) {\n        }\n",
        )],
    );
}

/// A swallowed assertion that is also a tautology is taken out of the effective count
/// once (#602), so a test the change adds with one real assertion beside it is not
/// vacuous. The swallowed assertion is still reported.
#[test]
fn a_swallowed_tautology_is_subtracted_once() {
    let base = format!("{}{}{}", PY.before, PY.plain, PY.after);
    let added = format!(
        "{base}\n\ndef test_b():\n    assert g() == 2\n    try:\n        assert True\n    except AssertionError:\n        pass\n"
    );
    let run = run_files(&PY, &base, &added);
    assert_eq!(caught_in(&run), vec![8]);
    assert_eq!(run.violations("vacuous-tests").len(), 0, "{}", run.stdout);
}

/// The gate counts a test the change adds among what it examined.
#[test]
fn a_swallowed_assertion_in_an_added_test_counts_the_test_as_examined() {
    let base = format!("{}{}{}", PY.before, PY.plain, PY.after);
    let added = format!(
        "{base}\n\ndef test_b():\n    try:\n        assert g() == 2\n    except AssertionError:\n        pass\n"
    );
    let run = run_files(&PY, &base, &added);
    assert_eq!(caught_in(&run), vec![7]);
    // The pair `test_a` and the added `test_b`.
    assert_eq!(run.outcome(GATE)["examined"], 2, "{}", run.stdout);
}
