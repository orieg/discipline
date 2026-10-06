//! CI-conditional skips the `ignored-tests` gate missed or reported wrongly (#564).
//!
//! A test that skips itself when a CI variable is set stops running where the merge gate
//! runs. Each case drives the real binary over a base side whose test runs and a head
//! side whose test carries the skip. The gaps: a CI read that reaches the skip through a
//! variable, a constant or a helper of the same file; a CI variable outside the short
//! list; an outer or later `if`; and a skip that holds only OUTSIDE CI, which is not a
//! CI-conditional skip because the test still runs there.

mod common;
use common::{Repo, CONFIG_HEAD};

/// What `ignored-tests` reports for one change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// One `Test Conditionally Skipped` at `error`, exit 1: the test stops running in CI.
    CiSkip,
    /// One `Test Conditionally Skipped` at `note`, exit 0: a conditional skip that no CI
    /// variable decides.
    Note,
    /// One `Existing Test Skipped` at `error`: read as an unconditional skip.
    Unconditional,
    /// No finding, exit 0.
    Nothing,
}

struct Case {
    name: String,
    path: &'static str,
    base: String,
    head: String,
    want: Want,
}

struct Seen {
    code: i32,
    findings: Vec<(String, String, String)>,
}

fn run_change(path: &str, base: &str, head: &str, config: Option<&str>) -> Seen {
    let repo = Repo::new();
    let mut files = vec![(path, base)];
    if path.ends_with(".go") {
        files.push(("go.mod", "module example.com/p\n\ngo 1.22\n"));
    }
    if let Some(config) = config {
        files.push(("discipline.toml", config));
    }
    repo.commit_base_files(&files, "test: base suite");
    repo.write(path, head);
    repo.commit("test: change the test");
    let run = repo.check(&[]);
    let findings = run
        .violations("ignored-tests")
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
                v["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    Seen {
        code: run.code,
        findings,
    }
}

fn matches_want(seen: &Seen, want: Want) -> bool {
    let one = |title: &str, severity: &str| {
        seen.findings.len() == 1 && seen.findings[0].0 == title && seen.findings[0].1 == severity
    };
    match want {
        Want::CiSkip => one("Test Conditionally Skipped", "error") && seen.code == 1,
        Want::Note => one("Test Conditionally Skipped", "note") && seen.code == 0,
        Want::Unconditional => one("Existing Test Skipped", "error") && seen.code == 1,
        Want::Nothing => seen.findings.is_empty() && seen.code == 0,
    }
}

/// Runs every case and reports all that differ, so one run shows the whole table.
fn check_cases(cases: Vec<Case>) {
    let mut wrong = Vec::new();
    for case in &cases {
        let seen = run_change(case.path, &case.base, &case.head, None);
        if !matches_want(&seen, case.want) {
            wrong.push(format!(
                "{}: want {:?}, got exit {} {:?}",
                case.name, case.want, seen.code, seen.findings
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} cases differ:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

// --- one test per language, with the lines under test spliced in -------------------

/// A pytest file: `module` goes before the test, `body` at the top of the test.
fn py(module: &str, body: &str) -> String {
    format!(
        "import os\nimport pytest\n\n{module}\ndef test_query():\n{body}    assert 1 + 1 == 2\n"
    )
}

/// A Go test file: `package` goes before the test, `body` at the top of the test.
fn go(package: &str, body: &str) -> String {
    format!(
        "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nvar _ = os.Getenv\n{package}\nfunc TestA(t *testing.T) {{\n{body}\tif 1+1 != 2 {{\n\t\tt.Fatal(\"math\")\n\t}}\n}}\n"
    )
}

/// A Jest / Vitest / Mocha file: `module` goes before the test, `body` at the top of the
/// callback, and `call` is the test function (`test`, `it`, `test.runIf(..)`).
fn js_with(module: &str, call: &str, callback: &str, body: &str) -> String {
    format!("{module}\n{call}('adds', {callback} {{\n{body}  expect(1 + 1).toBe(2);\n}});\n")
}

fn js(module: &str, body: &str) -> String {
    js_with(module, "test", "() =>", body)
}

/// A Rust integration test: `items` goes before the test, `attrs` on it, `body` at the
/// top of it.
fn rs(items: &str, attrs: &str, body: &str) -> String {
    format!("{items}\n{attrs}#[test]\nfn adds() {{\n{body}    assert_eq!(1 + 1, 2);\n}}\n")
}

const PY: &str = "test_query.py";
const GO: &str = "p_test.go";
const JS: &str = "a.test.js";
const TS: &str = "a.test.ts";
const RS: &str = "tests/q.rs";

fn case(name: &str, path: &'static str, head: String, want: Want) -> Case {
    let base = match path {
        PY => py("", ""),
        GO => go("", ""),
        JS | TS => js("", ""),
        RS => rs("", "", ""),
        other => panic!("no base for {other}"),
    };
    Case {
        name: name.to_string(),
        path,
        base,
        head,
        want,
    }
}

// --- 1. the CI read reaches the skip through a name of the same file ---------------

#[test]
fn ci_read_through_a_local_variable_is_a_ci_skip() {
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "",
                "    in_ci = os.environ.get(\"CI\")\n    if in_ci:\n        pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go",
            GO,
            go(
                "",
                "\tinCI := os.Getenv(\"CI\") != \"\"\n\tif inCI {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript",
            JS,
            js("", "  const isCI = process.env.CI;\n  if (isCI) return;\n"),
            Want::CiSkip,
        ),
        case(
            "rust",
            RS,
            rs(
                "",
                "",
                "    let in_ci = std::env::var(\"CI\").is_ok();\n    if in_ci {\n        return;\n    }\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

#[test]
fn ci_read_through_a_module_level_constant_is_a_ci_skip() {
    check_cases(vec![
        case(
            "python skip",
            PY,
            py(
                "IN_CI = os.environ.get(\"CI\")\n",
                "    if IN_CI:\n        pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "python bool() and return",
            PY,
            py(
                "IN_CI = bool(os.getenv(\"CI\"))\n",
                "    if IN_CI:\n        return\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go skip",
            GO,
            go(
                "var inCI = os.Getenv(\"CI\") != \"\"\n",
                "\tif inCI {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go return",
            GO,
            go(
                "var inCI = os.Getenv(\"CI\") != \"\"\n",
                "\tif inCI {\n\t\treturn\n\t}\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript",
            JS,
            js("const isCI = !!process.env.CI;\n", "  if (isCI) return;\n"),
            Want::CiSkip,
        ),
        case(
            "typescript",
            TS,
            js(
                "const isCI: boolean = !!process.env.CI;\n",
                "  if (isCI) return;\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript destructured",
            JS,
            js("const { CI } = process.env;\n", "  if (CI) return;\n"),
            Want::CiSkip,
        ),
        case(
            "rust",
            RS,
            rs(
                "const IN_CI: bool = option_env!(\"CI\").is_some();\n",
                "",
                "    if IN_CI {\n        return;\n    }\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

#[test]
fn ci_read_through_a_helper_of_the_same_file_is_a_ci_skip() {
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "def is_ci():\n    return os.environ.get(\"CI\") is not None\n",
                "    if is_ci():\n        pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "python helper that branches",
            PY,
            py(
                "def is_ci():\n    if os.environ.get(\"CI\"):\n        return True\n    return False\n",
                "    if is_ci():\n        pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go",
            GO,
            go(
                "func isCI() bool {\n\treturn os.Getenv(\"CI\") != \"\"\n}\n",
                "\tif isCI() {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript function",
            JS,
            js(
                "function isCI() {\n  return Boolean(process.env.CI);\n}\n",
                "  if (isCI()) return;\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript arrow",
            JS,
            js(
                "const inCI = () => process.env.CI === 'true';\n",
                "  if (inCI()) return;\n",
            ),
            Want::CiSkip,
        ),
        case(
            "rust tail expression",
            RS,
            rs(
                "fn in_ci() -> bool {\n    std::env::var(\"CI\").is_ok()\n}\n",
                "",
                "    if in_ci() {\n        return;\n    }\n",
            ),
            Want::CiSkip,
        ),
        case(
            "rust return",
            RS,
            rs(
                "fn in_ci() -> bool {\n    return std::env::var_os(\"CI\").is_some();\n}\n",
                "",
                "    if in_ci() {\n        return;\n    }\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

/// A test callback closes over the variables of the callback or test around it.
#[test]
fn ci_read_through_a_variable_of_an_enclosing_callback_is_a_ci_skip() {
    let describe = |setup: &str, body: &str| {
        format!(
            "describe('math', () => {{\n{setup}  test('adds', () => {{\n{body}    expect(1 + 1).toBe(2);\n  }});\n}});\n"
        )
    };
    let subtest = |setup: &str, body: &str| {
        format!(
            "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nvar _ = os.Getenv\n\nfunc TestA(t *testing.T) {{\n{setup}\tt.Run(\"adds\", func(t *testing.T) {{\n{body}\t\tif 1+1 != 2 {{\n\t\t\tt.Fatal(\"math\")\n\t\t}}\n\t}})\n}}\n"
        )
    };
    check_cases(vec![
        Case {
            name: "javascript describe".to_string(),
            path: JS,
            base: describe("", ""),
            head: describe(
                "  const isCI = !!process.env.CI;\n",
                "    if (isCI) return;\n",
            ),
            want: Want::CiSkip,
        },
        Case {
            name: "go subtest".to_string(),
            path: GO,
            base: subtest("", ""),
            head: subtest(
                "\tinCI := os.Getenv(\"CI\") != \"\"\n",
                "\t\tif inCI {\n\t\t\tt.Skip()\n\t\t}\n",
            ),
            want: Want::CiSkip,
        },
        // Control: the same shape on a variable that is not a CI read.
        Case {
            name: "go subtest, another variable".to_string(),
            path: GO,
            base: subtest("", ""),
            head: subtest(
                "\tslow := os.Getenv(\"SLOW\") != \"\"\n",
                "\t\tif slow {\n\t\t\tt.Skip()\n\t\t}\n",
            ),
            want: Want::Note,
        },
    ]);
}

/// Control: a name bound to something that is not a CI read stays a note.
#[test]
fn a_name_bound_to_something_else_is_not_a_ci_skip() {
    check_cases(vec![
        case(
            "python module constant",
            PY,
            py(
                "HAVE_DB = os.path.exists(\"db.sock\")\n",
                "    if not HAVE_DB:\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "python helper",
            PY,
            py(
                "def slow():\n    return os.environ.get(\"SLOW_TESTS\")\n",
                "    if slow():\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "go package variable",
            GO,
            go(
                "var short = os.Getenv(\"SHORT\") != \"\"\n",
                "\tif short {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::Note,
        ),
        case(
            "javascript local",
            JS,
            js("", "  const slow = process.env.SLOW;\n  if (slow) return;\n"),
            Want::Note,
        ),
        case(
            "rust local",
            RS,
            rs(
                "",
                "",
                "    let slow = std::env::var(\"SLOW\").is_ok();\n    if slow {\n        return;\n    }\n",
            ),
            Want::Note,
        ),
    ]);
}

/// `approved_predicates` and the message name the variable the condition reads, not the
/// name it reads it through.
#[test]
fn an_indirect_ci_skip_names_its_variable_and_takes_approved_predicates() {
    let head = py(
        "IN_CI = os.environ.get(\"CI\")\n",
        "    if IN_CI:\n        pytest.skip()\n",
    );
    let seen = run_change(PY, &py("", ""), &head, None);
    assert!(matches_want(&seen, Want::CiSkip), "{:?}", seen.findings);
    assert_eq!(
        seen.findings[0].2,
        "Test `test_query` is conditionally skipped under predicate `IN_CI`, which reads CI variable `CI`."
    );

    let approved = format!("{CONFIG_HEAD}[gates.ignored-tests]\napproved_predicates = [\"CI\"]\n");
    let seen = run_change(PY, &py("", ""), &head, Some(&approved));
    assert!(matches_want(&seen, Want::Nothing), "{:?}", seen.findings);

    // Control: approving another variable waives nothing.
    let other = format!("{CONFIG_HEAD}[gates.ignored-tests]\napproved_predicates = [\"TRAVIS\"]\n");
    let seen = run_change(PY, &py("", ""), &head, Some(&other));
    assert!(matches_want(&seen, Want::CiSkip), "{:?}", seen.findings);
}

// --- 2. a CI variable outside the short list ----------------------------------------

#[test]
fn more_ci_variables_are_read_as_ci() {
    let mut cases = Vec::new();
    for var in [
        "GITHUB_RUN_ID",
        "CI_JOB_ID",
        "CI_PIPELINE_ID",
        "BUILD_ID",
        "DRONE",
        "WOODPECKER",
    ] {
        cases.push(case(
            &format!("python {var}"),
            PY,
            py(
                "",
                &format!("    if os.environ.get(\"{var}\"):\n        pytest.skip()\n"),
            ),
            Want::CiSkip,
        ));
    }
    cases.push(case(
        "go DRONE",
        GO,
        go(
            "",
            "\tif os.Getenv(\"DRONE\") != \"\" {\n\t\tt.Skip()\n\t}\n",
        ),
        Want::CiSkip,
    ));
    cases.push(case(
        "javascript WOODPECKER",
        JS,
        js("", "  if (process.env.WOODPECKER) return;\n"),
        Want::CiSkip,
    ));
    cases.push(case(
        "rust CI_JOB_ID",
        RS,
        rs(
            "",
            "",
            "    if std::env::var(\"CI_JOB_ID\").is_ok() {\n        return;\n    }\n",
        ),
        Want::CiSkip,
    ));
    check_cases(cases);
}

/// Control: a variable that only resembles one stays a note.
#[test]
fn a_variable_that_is_not_a_ci_variable_stays_a_note() {
    check_cases(vec![
        case(
            "python CIRCUS",
            PY,
            py(
                "",
                "    if os.environ.get(\"CIRCUS\"):\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "python SKIP_SLOW",
            PY,
            py(
                "",
                "    if os.environ.get(\"SKIP_SLOW\"):\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "go BUILD_IDENTITY",
            GO,
            go(
                "",
                "\tif os.Getenv(\"BUILD_IDENTITY\") != \"\" {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::Note,
        ),
        case(
            "javascript DRONES",
            JS,
            js("", "  if (process.env.DRONES) return;\n"),
            Want::Note,
        ),
    ]);
}

// --- 3. an outer `if`, or a later one -----------------------------------------------

#[test]
fn a_ci_condition_on_an_outer_if_is_a_ci_skip() {
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "FLAKY = True\n",
                "    if os.environ.get(\"CI\"):\n        if FLAKY:\n            pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go",
            GO,
            go(
                "var flaky = true\n",
                "\tif os.Getenv(\"CI\") != \"\" {\n\t\tif flaky {\n\t\t\tt.Skip()\n\t\t}\n\t}\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript",
            JS,
            js(
                "const flaky = true;\n",
                "  if (process.env.CI) {\n    if (flaky) return;\n  }\n",
            ),
            Want::CiSkip,
        ),
        case(
            "rust",
            RS,
            rs(
                "const FLAKY: bool = true;\n",
                "",
                "    if std::env::var(\"CI\").is_ok() {\n        if FLAKY {\n            return;\n        }\n    }\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

#[test]
fn a_ci_condition_on_a_second_if_is_a_ci_skip() {
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "FLAKY = True\n",
                "    if FLAKY:\n        pytest.skip()\n    if os.environ.get(\"CI\"):\n        pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go",
            GO,
            go(
                "var flaky = true\n",
                "\tif flaky {\n\t\tt.Skip()\n\t}\n\tif os.Getenv(\"CI\") != \"\" {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::CiSkip,
        ),
        case(
            "javascript",
            JS,
            js(
                "",
                "  if (process.env.SLOW) return;\n  if (process.env.CI) return;\n",
            ),
            Want::CiSkip,
        ),
        case(
            "rust",
            RS,
            rs(
                "",
                "",
                "    if std::env::var(\"SLOW\").is_ok() {\n        return;\n    }\n    if std::env::var(\"CI\").is_ok() {\n        return;\n    }\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

/// Control: two conditions, neither on a CI variable, stay one note.
#[test]
fn two_conditions_without_a_ci_variable_stay_a_note() {
    check_cases(vec![
        case(
            "python nested",
            PY,
            py(
                "FLAKY = True\n",
                "    if os.environ.get(\"SLOW\"):\n        if FLAKY:\n            pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "go second if",
            GO,
            go(
                "var flaky = true\n",
                "\tif flaky {\n\t\tt.Skip()\n\t}\n\tif os.Getenv(\"SLOW\") != \"\" {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::Note,
        ),
    ]);
}

// --- 4. polarity: a skip that holds only outside CI ---------------------------------

#[test]
fn python_skip_outside_ci_is_not_a_ci_skip() {
    let skip = |cond: &str| {
        py(
            "",
            &format!("    if {cond}:\n        pytest.skip(\"needs the CI database\")\n"),
        )
    };
    check_cases(vec![
        case("not", PY, skip("not os.environ.get(\"CI\")"), Want::Note),
        case(
            "is None",
            PY,
            skip("os.environ.get(\"CI\") is None"),
            Want::Note,
        ),
        case(
            "== ''",
            PY,
            skip("os.environ.get(\"CI\", \"\") == \"\""),
            Want::Note,
        ),
        case(
            "!= 'true'",
            PY,
            skip("os.environ.get(\"CI\") != \"true\""),
            Want::Note,
        ),
        case(
            "== 'false'",
            PY,
            skip("os.getenv(\"CI\", \"false\") == \"false\""),
            Want::Note,
        ),
        case("not in", PY, skip("\"CI\" not in os.environ"), Want::Note),
        case(
            "not a module constant",
            PY,
            py(
                "IN_CI = bool(os.environ.get(\"CI\"))\n",
                "    if not IN_CI:\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "and another condition",
            PY,
            skip("not os.environ.get(\"CI\") and os.name == \"nt\""),
            Want::Note,
        ),
        case(
            "return",
            PY,
            py("", "    if not os.environ.get(\"CI\"):\n        return\n"),
            Want::Note,
        ),
        case(
            "else branch",
            PY,
            py(
                "",
                "    if os.environ.get(\"CI\"):\n        pass\n    else:\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
    ]);
}

#[test]
fn go_skip_outside_ci_is_not_a_ci_skip() {
    let skip = |cond: &str| {
        go(
            "",
            &format!("\tif {cond} {{\n\t\tt.Skip(\"needs the CI database\")\n\t}}\n"),
        )
    };
    check_cases(vec![
        case("== \"\"", GO, skip("os.Getenv(\"CI\") == \"\""), Want::Note),
        case("!= \"true\"", GO, skip("os.Getenv(\"CI\") != \"true\""), Want::Note),
        case("!ok", GO, skip("_, ok := os.LookupEnv(\"CI\"); !ok"), Want::Note),
        case("!(..)", GO, skip("!(os.Getenv(\"CI\") != \"\")"), Want::Note),
        case(
            "return",
            GO,
            go("", "\tif os.Getenv(\"CI\") == \"\" {\n\t\treturn\n\t}\n"),
            Want::Note,
        ),
        case(
            "else branch",
            GO,
            go(
                "",
                "\tif os.Getenv(\"CI\") != \"\" {\n\t\tt.Log(\"ci\")\n\t} else {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::Note,
        ),
    ]);
}

#[test]
fn javascript_return_outside_ci_is_not_a_ci_skip() {
    let ret = |cond: &str| js("", &format!("  if ({cond}) return;\n"));
    check_cases(vec![
        case("!", JS, ret("!process.env.CI"), Want::Note),
        case(
            "=== undefined",
            JS,
            ret("process.env.CI === undefined"),
            Want::Note,
        ),
        case(
            "!== 'true'",
            JS,
            ret("process.env.CI !== 'true'"),
            Want::Note,
        ),
        case("== null", JS, ret("process.env.CI == null"), Want::Note),
        case("typescript !", TS, ret("!process.env.CI"), Want::Note),
    ]);
}

#[test]
fn rust_skip_outside_ci_is_not_a_ci_skip() {
    let ret = |cond: &str| {
        rs(
            "",
            "",
            &format!("    if {cond} {{\n        return;\n    }}\n"),
        )
    };
    check_cases(vec![
        case(
            "is_err",
            RS,
            ret("std::env::var(\"CI\").is_err()"),
            Want::Note,
        ),
        case(
            "!is_ok",
            RS,
            ret("!std::env::var(\"CI\").is_ok()"),
            Want::Note,
        ),
        case(
            "is_none",
            RS,
            ret("std::env::var_os(\"CI\").is_none()"),
            Want::Note,
        ),
        case(
            "!= Ok(\"true\")",
            RS,
            ret("std::env::var(\"CI\").as_deref() != Ok(\"true\")"),
            Want::Note,
        ),
        case(
            "cfg_attr(not(ci), ignore)",
            RS,
            rs("", "#[cfg_attr(not(ci), ignore)]\n", ""),
            Want::Note,
        ),
        case(
            "cfg_attr(all(not(ci), unix), ignore)",
            RS,
            rs("", "#[cfg_attr(all(not(ci), unix), ignore)]\n", ""),
            Want::Note,
        ),
    ]);
}

/// Control: the opposite polarity, in the same spellings, is still a CI skip.
#[test]
fn skip_inside_ci_is_still_a_ci_skip_in_every_spelling() {
    let py_skip = |cond: &str| py("", &format!("    if {cond}:\n        pytest.skip()\n"));
    let go_skip = |cond: &str| go("", &format!("\tif {cond} {{\n\t\tt.Skip()\n\t}}\n"));
    let js_ret = |cond: &str| js("", &format!("  if ({cond}) return;\n"));
    let rs_ret = |cond: &str| {
        rs(
            "",
            "",
            &format!("    if {cond} {{\n        return;\n    }}\n"),
        )
    };
    check_cases(vec![
        case(
            "python get",
            PY,
            py_skip("os.environ.get(\"CI\")"),
            Want::CiSkip,
        ),
        case(
            "python is not None",
            PY,
            py_skip("os.environ.get(\"CI\") is not None"),
            Want::CiSkip,
        ),
        case(
            "python != ''",
            PY,
            py_skip("os.environ.get(\"CI\", \"\") != \"\""),
            Want::CiSkip,
        ),
        case(
            "python == 'true'",
            PY,
            py_skip("os.environ.get(\"CI\") == \"true\""),
            Want::CiSkip,
        ),
        case(
            "python in",
            PY,
            py_skip("\"CI\" in os.environ"),
            Want::CiSkip,
        ),
        case(
            "python subscript",
            PY,
            py_skip("os.environ[\"CI\"]"),
            Want::CiSkip,
        ),
        case(
            "python double negation",
            PY,
            py_skip("not (os.environ.get(\"CI\") is None)"),
            Want::CiSkip,
        ),
        case(
            "python or another condition",
            PY,
            py_skip("os.environ.get(\"CI\") or os.name == \"nt\""),
            Want::CiSkip,
        ),
        case(
            "go != \"\"",
            GO,
            go_skip("os.Getenv(\"CI\") != \"\""),
            Want::CiSkip,
        ),
        case(
            "go == \"true\"",
            GO,
            go_skip("os.Getenv(\"CI\") == \"true\""),
            Want::CiSkip,
        ),
        case(
            "go ok",
            GO,
            go_skip("_, ok := os.LookupEnv(\"CI\"); ok"),
            Want::CiSkip,
        ),
        case(
            "javascript bare",
            JS,
            js_ret("process.env.CI"),
            Want::CiSkip,
        ),
        case(
            "javascript !!",
            JS,
            js_ret("!!process.env.CI"),
            Want::CiSkip,
        ),
        case(
            "javascript === 'true'",
            JS,
            js_ret("process.env.CI === 'true'"),
            Want::CiSkip,
        ),
        case(
            "javascript !== undefined",
            JS,
            js_ret("process.env.CI !== undefined"),
            Want::CiSkip,
        ),
        case(
            "javascript bracket",
            JS,
            js_ret("process.env['CI']"),
            Want::CiSkip,
        ),
        case(
            "rust is_ok",
            RS,
            rs_ret("std::env::var(\"CI\").is_ok()"),
            Want::CiSkip,
        ),
        case(
            "rust is_some",
            RS,
            rs_ret("std::env::var_os(\"CI\").is_some()"),
            Want::CiSkip,
        ),
        case(
            "rust option_env",
            RS,
            rs_ret("option_env!(\"CI\").is_some()"),
            Want::CiSkip,
        ),
        case(
            "rust cfg_attr(ci, ignore)",
            RS,
            rs("", "#[cfg_attr(ci, ignore)]\n", ""),
            Want::CiSkip,
        ),
        case(
            "rust cfg_attr(any(ci, miri), ignore)",
            RS,
            rs("", "#[cfg_attr(any(ci, miri), ignore)]\n", ""),
            Want::CiSkip,
        ),
        // A condition this reading does not decide is reported: it involves the variable.
        case(
            "python undecided",
            PY,
            py_skip("len(os.environ.get(\"CI\", \"\")) > 3"),
            Want::CiSkip,
        ),
    ]);
}

/// The `else` branch of a condition that holds outside CI is where CI runs.
#[test]
fn a_skip_in_the_else_branch_of_an_outside_ci_condition_is_a_ci_skip() {
    // Both sides of the fix report an error here; what changes is which finding.
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "",
                "    if not os.environ.get(\"CI\"):\n        pass\n    else:\n        pytest.skip()\n",
            ),
            Want::CiSkip,
        ),
        case(
            "go",
            GO,
            go(
                "",
                "\tif os.Getenv(\"CI\") == \"\" {\n\t\tt.Log(\"local\")\n\t} else {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

/// Control: a skip in the `else` branch of a condition on no CI variable is read as
/// before, as an unconditional skip.
#[test]
fn a_skip_in_the_else_branch_of_another_condition_is_unchanged() {
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "HAVE_DB = False\n",
                "    if HAVE_DB:\n        pass\n    else:\n        pytest.skip()\n",
            ),
            Want::Unconditional,
        ),
        case(
            "go",
            GO,
            go(
                "var haveDB = false\n",
                "\tif haveDB {\n\t\tt.Log(\"db\")\n\t} else {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::Unconditional,
        ),
    ]);
}

/// A skip that held only outside CI and now holds in CI is a new CI-conditional skip.
#[test]
fn a_condition_flipped_from_outside_ci_to_inside_ci_is_reported() {
    let outside = py(
        "",
        "    if not os.environ.get(\"CI\"):\n        pytest.skip()\n",
    );
    let inside = py(
        "",
        "    if os.environ.get(\"CI\"):\n        pytest.skip()\n",
    );
    let seen = run_change(PY, &outside, &inside, None);
    assert!(matches_want(&seen, Want::CiSkip), "{:?}", seen.findings);

    // Control: the other direction makes the test run in CI again.
    let seen = run_change(PY, &inside, &outside, None);
    assert!(matches_want(&seen, Want::Nothing), "{:?}", seen.findings);

    // Control: unchanged on both sides.
    let seen = run_change(
        PY,
        &inside,
        &inside.replace("1 + 1 == 2", "2 + 2 == 4"),
        None,
    );
    assert!(matches_want(&seen, Want::Nothing), "{:?}", seen.findings);
}

// --- 5. a name spelled like a CI variable that is bound to something else -----------

#[test]
fn a_variable_named_ci_bound_to_something_else_is_not_a_ci_skip() {
    check_cases(vec![
        case(
            "python",
            PY,
            py(
                "",
                "    ci = os.path.exists(\"ci.sock\")\n    if ci:\n        pytest.skip()\n",
            ),
            Want::Note,
        ),
        case(
            "go",
            GO,
            go(
                "",
                "\tci := len(os.Args) > 3\n\tif ci {\n\t\tt.Skip()\n\t}\n",
            ),
            Want::Note,
        ),
        case(
            "javascript",
            JS,
            js("", "  const ci = makeClient();\n  if (ci) return;\n"),
            Want::Note,
        ),
        case(
            "rust",
            RS,
            rs(
                "",
                "",
                "    let ci = std::path::Path::new(\"ci.sock\").exists();\n    if ci {\n        return;\n    }\n",
            ),
            Want::Note,
        ),
    ]);
}

/// Control: a name spelled like a CI variable that this file does not bind (a fixture, an
/// import, an attribute of a settings object) is still read as one.
#[test]
fn a_name_spelled_like_a_ci_variable_and_not_bound_in_the_file_is_a_ci_skip() {
    check_cases(vec![
        case(
            "python attribute",
            PY,
            py("", "    if settings.CI:\n        pytest.skip()\n"),
            Want::CiSkip,
        ),
        case(
            "python bare name",
            PY,
            py("", "    if CI:\n        pytest.skip()\n"),
            Want::CiSkip,
        ),
        case(
            "javascript attribute",
            JS,
            js("", "  if (config.CI) return;\n"),
            Want::CiSkip,
        ),
    ]);
}

// --- 6. JavaScript / TypeScript framework forms -------------------------------------

#[test]
fn javascript_run_if_outside_ci_is_a_ci_skip() {
    check_cases(vec![
        case(
            "test.runIf(!process.env.CI)",
            JS,
            js_with("", "test.runIf(!process.env.CI)", "() =>", ""),
            Want::CiSkip,
        ),
        case(
            "it.runIf(!isCI) with a module constant",
            JS,
            js_with(
                "const isCI = !!process.env.CI;\n",
                "it.runIf(!isCI)",
                "() =>",
                "",
            ),
            Want::CiSkip,
        ),
        case(
            "typescript",
            TS,
            js_with("", "test.runIf(!process.env.CI)", "() =>", ""),
            Want::CiSkip,
        ),
    ]);
}

/// Control: a run condition that holds in CI, or is on no CI variable, is a conditional
/// skip that no CI variable decides: a note.
#[test]
fn javascript_run_if_that_runs_in_ci_is_a_note() {
    check_cases(vec![
        case(
            "test.runIf(process.env.CI)",
            JS,
            js_with("", "test.runIf(process.env.CI)", "() =>", ""),
            Want::Note,
        ),
        case(
            "test.runIf(platform)",
            JS,
            js_with("", "test.runIf(process.platform === 'linux')", "() =>", ""),
            Want::Note,
        ),
    ]);
}

#[test]
fn mocha_this_skip_under_a_ci_condition_is_a_ci_skip() {
    check_cases(vec![
        case(
            "function callback",
            JS,
            js_with(
                "",
                "it",
                "function ()",
                "  if (process.env.CI) {\n    this.skip();\n  }\n",
            ),
            Want::CiSkip,
        ),
        case(
            "module constant",
            JS,
            js_with(
                "const isCI = Boolean(process.env.GITHUB_ACTIONS);\n",
                "it",
                "function ()",
                "  if (isCI) {\n    this.skip();\n  }\n",
            ),
            Want::CiSkip,
        ),
    ]);
}

/// Control: `this.skip()` outside CI only, or under no CI variable, is a conditional skip
/// that no CI variable decides: a note.
#[test]
fn mocha_this_skip_under_another_condition_is_a_note() {
    check_cases(vec![
        case(
            "outside CI",
            JS,
            js_with(
                "",
                "it",
                "function ()",
                "  if (!process.env.CI) {\n    this.skip();\n  }\n",
            ),
            Want::Note,
        ),
        case(
            "another variable",
            JS,
            js_with(
                "",
                "it",
                "function ()",
                "  if (process.env.SLOW) {\n    this.skip();\n  }\n",
            ),
            Want::Note,
        ),
    ]);
}

// --- 7. a test that arrives with the skip -------------------------------------------

#[test]
fn a_new_test_with_an_indirect_ci_skip_is_reported() {
    let repo = Repo::new();
    repo.write(
        PY,
        &py(
            "IN_CI = os.environ.get(\"CI\")\n",
            "    if IN_CI:\n        pytest.skip()\n",
        ),
    );
    repo.commit("test: add a test");
    let run = repo.check(&[]);
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Conditionally Skipped"],
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.violations("ignored-tests")[0]["severity"], "error");
    assert_eq!(run.code, 1);

    // Control: the same test skipping only outside CI arrives with a note.
    let repo = Repo::new();
    repo.write(
        PY,
        &py(
            "IN_CI = os.environ.get(\"CI\")\n",
            "    if not IN_CI:\n        pytest.skip()\n",
        ),
    );
    repo.commit("test: add a test");
    let run = repo.check(&[]);
    assert_eq!(run.violations("ignored-tests")[0]["severity"], "note");
    assert_eq!(run.code, 0);
}

// --- 8. skip-if decorators (#597) ------------------------------------------------------

/// A `skipif`-style decorator or modifier is a conditional skip, read by its condition
/// the way an `if` around a skip is.
#[test]
fn skip_if_decorators_are_conditional_skips_read_by_their_condition() {
    let decorated = |cond: &str| {
        format!(
            "import os\nimport sys\nimport pytest\n\n@pytest.mark.skipif({cond}, reason=\"x\")\ndef test_query():\n    assert 1 + 1 == 2\n"
        )
    };
    check_cases(vec![
        case(
            "python skipif(CI)",
            PY,
            decorated("os.environ.get(\"CI\")"),
            Want::CiSkip,
        ),
        case(
            "python skipif(not CI)",
            PY,
            decorated("not os.environ.get(\"CI\")"),
            Want::Note,
        ),
        case(
            "python skipif(platform)",
            PY,
            decorated("sys.platform == \"win32\""),
            Want::Note,
        ),
        case(
            "javascript skipIf(CI)",
            JS,
            js_with("", "test.skipIf(process.env.CI)", "() =>", ""),
            Want::CiSkip,
        ),
        case(
            "javascript skipIf(!CI)",
            JS,
            js_with("", "test.skipIf(!process.env.CI)", "() =>", ""),
            Want::Note,
        ),
        case(
            "javascript skipIf(platform)",
            JS,
            js_with("", "test.skipIf(process.platform === 'win32')", "() =>", ""),
            Want::Note,
        ),
    ]);
}
