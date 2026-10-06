//! Skips that take a condition, and what the `ignored-tests` gate reports for each (#597).
//!
//! A decorator, annotation or call that skips a test under a condition
//! (`@pytest.mark.skipif(..)`, `test.skipIf(..)`, `@DisabledIfEnvironmentVariable(..)`,
//! `assumeTrue(..)`) is a conditional skip, and its condition is evaluated the way the
//! condition of an `if` around a skip is: a skip that holds in CI is a CI-conditional
//! skip, one that holds only outside CI or reads no CI variable is a note, a condition
//! that is always true is an unconditional skip, and one that is always false is no skip.
//!
//! Each case drives the real binary over a base side whose test runs and a head side
//! whose test carries the skip. The sources under test are constants of this file: they
//! are fixtures, not tests of this suite.

mod common;
use common::Repo;

/// What `ignored-tests` reports for one change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// One `Test Conditionally Skipped` at `error`, exit 1: the test stops running in CI.
    CiSkip,
    /// One `Test Conditionally Skipped` at `note`, exit 0.
    Note,
    /// One `Existing Test Skipped` at `error`, exit 1: an unconditional skip.
    Unconditional,
    /// No finding, exit 0.
    Nothing,
}

struct Case {
    name: &'static str,
    path: &'static str,
    base: &'static str,
    head: &'static str,
    /// A `discipline.toml` of the base side, or `""`.
    config: &'static str,
    want: Want,
    /// Text the message of the finding carries, or `""`.
    message: &'static str,
}

struct Seen {
    code: i32,
    findings: Vec<(String, String, String)>,
}

fn run_change(case: &Case) -> Seen {
    let repo = Repo::new();
    let mut files = vec![(case.path, case.base)];
    if case.path.ends_with(".go") {
        files.push(("go.mod", "module example.com/p\n\ngo 1.22\n"));
    }
    if !case.config.is_empty() {
        files.push(("discipline.toml", case.config));
    }
    repo.commit_base_files(&files, "test: base suite");
    repo.write(case.path, case.head);
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

/// The cases that differ from what they want, one line each.
fn differing(cases: &[Case]) -> Vec<String> {
    let mut wrong = Vec::new();
    for case in cases {
        let seen = run_change(case);
        let message_ok = case.message.is_empty()
            || seen
                .findings
                .first()
                .is_some_and(|f| f.2.contains(case.message));
        if !matches_want(&seen, case.want) || !message_ok {
            wrong.push(format!(
                "{}: want {:?} `{}`, got exit {} {:?}",
                case.name, case.want, case.message, seen.code, seen.findings
            ));
        }
    }
    wrong
}

const fn case(
    name: &'static str,
    path: &'static str,
    base: &'static str,
    head: &'static str,
    want: Want,
) -> Case {
    Case {
        name,
        path,
        base,
        head,
        config: "",
        want,
        message: "",
    }
}

const PY: &str = "test_query.py";
const GO: &str = "p_test.go";
const JS: &str = "a.test.js";
const TS: &str = "a.test.ts";
const RS: &str = "tests/q.rs";
const JAVA: &str = "src/test/java/QTest.java";
const KT: &str = "src/test/kotlin/QTest.kt";

// --- Python -----------------------------------------------------------------------

/// A pytest file: `$module` goes before the test, `$decorators` on it, `$body` at its top.
macro_rules! py {
    ($module:expr, $decorators:expr, $body:expr) => {
        concat!(
            "import os\nimport sys\nimport unittest\nimport pytest\n\n",
            $module,
            "\n",
            $decorators,
            "def test_query():\n",
            $body,
            "    assert 1 + 1 == 2\n"
        )
    };
}

/// The same test as a method: `$class_decorators` on the class, `$class_body` at its top.
macro_rules! py_class {
    ($class_decorators:expr, $class_body:expr) => {
        concat!(
            "import os\nimport sys\nimport unittest\nimport pytest\n\n",
            $class_decorators,
            "class TestQuery:\n",
            $class_body,
            "    def test_query(self):\n        assert 1 + 1 == 2\n"
        )
    };
}

const PY_BASE: &str = py!("", "", "");
const PY_CLASS_BASE: &str = py_class!("", "");

macro_rules! py_decorated {
    ($name:expr, $decorator:expr, $want:expr) => {
        case(
            $name,
            PY,
            PY_BASE,
            py!("", concat!($decorator, "\n"), ""),
            $want,
        )
    };
}

const PY_SKIPIF: &[Case] = &[
    py_decorated!(
        "skipif(CI)",
        "@pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")",
        Want::CiSkip
    ),
    py_decorated!(
        "skipif(CI == 'true')",
        "@pytest.mark.skipif(os.environ.get(\"CI\") == \"true\", reason=\"x\")",
        Want::CiSkip
    ),
    py_decorated!(
        "skipif(condition=CI)",
        "@pytest.mark.skipif(condition=os.getenv(\"GITHUB_ACTIONS\"), reason=\"x\")",
        Want::CiSkip
    ),
    py_decorated!(
        "skipif(CI and platform)",
        "@pytest.mark.skipif(os.environ.get(\"CI\") and sys.platform == \"darwin\", reason=\"x\")",
        Want::CiSkip
    ),
    py_decorated!(
        "skipif(not CI)",
        "@pytest.mark.skipif(not os.environ.get(\"CI\"), reason=\"x\")",
        Want::Note
    ),
    py_decorated!(
        "skipif(platform)",
        "@pytest.mark.skipif(sys.platform == \"win32\", reason=\"x\")",
        Want::Note
    ),
    py_decorated!(
        "skipif(version)",
        "@pytest.mark.skipif(sys.version_info < (3, 10), reason=\"x\")",
        Want::Note
    ),
    py_decorated!(
        "skipif(another variable)",
        "@pytest.mark.skipif(os.environ.get(\"SLOW\"), reason=\"x\")",
        Want::Note
    ),
    case(
        "skipif(module constant)",
        PY,
        PY_BASE,
        py!(
            "IN_CI = bool(os.getenv(\"CI\"))\n",
            "@pytest.mark.skipif(IN_CI, reason=\"x\")\n",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "skipif(helper of the file)",
        PY,
        PY_BASE,
        py!(
            "def is_ci():\n    return os.environ.get(\"CI\") is not None\n",
            "@pytest.mark.skipif(is_ci(), reason=\"x\")\n",
            ""
        ),
        Want::CiSkip,
    ),
    py_decorated!(
        "unittest.skipIf(CI)",
        "@unittest.skipIf(os.environ.get(\"CI\"), \"x\")",
        Want::CiSkip
    ),
    py_decorated!(
        "unittest.skipIf(platform)",
        "@unittest.skipIf(sys.platform.startswith(\"win\"), \"x\")",
        Want::Note
    ),
    py_decorated!(
        "unittest.skipUnless(not CI)",
        "@unittest.skipUnless(not os.environ.get(\"CI\"), \"x\")",
        Want::CiSkip
    ),
    py_decorated!(
        "unittest.skipUnless(CI)",
        "@unittest.skipUnless(os.environ.get(\"CI\"), \"x\")",
        Want::Note
    ),
    py_decorated!(
        "unittest.skipUnless(platform)",
        "@unittest.skipUnless(sys.platform.startswith(\"linux\"), \"x\")",
        Want::Note
    ),
];

const PY_SKIPIF_STRING: &[Case] = &[
    py_decorated!(
        "skipif('platform')",
        "@pytest.mark.skipif(\"sys.platform == 'win32'\")",
        Want::Note
    ),
    py_decorated!(
        "skipif('CI')",
        "@pytest.mark.skipif(\"os.environ.get('CI')\")",
        Want::CiSkip
    ),
    py_decorated!(
        "skipif('not CI')",
        "@pytest.mark.skipif(\"not os.environ.get('CI')\")",
        Want::Note
    ),
    py_decorated!(
        "skipif('not an expression')",
        "@pytest.mark.skipif(\"this is ( not python\")",
        Want::Note
    ),
];

const PY_SKIPIF_CONSTANT: &[Case] = &[
    py_decorated!(
        "skipif(True)",
        "@pytest.mark.skipif(True, reason=\"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "skipif(1)",
        "@pytest.mark.skipif(1, reason=\"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "skipif(not False)",
        "@pytest.mark.skipif(not False, reason=\"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "skipif(1 == 1)",
        "@pytest.mark.skipif(1 == 1, reason=\"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "skipif('a' == 'a')",
        "@pytest.mark.skipif(\"a\" == \"a\", reason=\"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "skipif(True or platform)",
        "@pytest.mark.skipif(True or sys.platform == \"win32\", reason=\"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "unittest.skipIf(True)",
        "@unittest.skipIf(True, \"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "unittest.skipUnless(False)",
        "@unittest.skipUnless(False, \"x\")",
        Want::Unconditional
    ),
    py_decorated!(
        "skipif(False)",
        "@pytest.mark.skipif(False, reason=\"x\")",
        Want::Nothing
    ),
    py_decorated!(
        "skipif(0)",
        "@pytest.mark.skipif(0, reason=\"x\")",
        Want::Nothing
    ),
    py_decorated!(
        "skipif(1 == 2)",
        "@pytest.mark.skipif(1 == 2, reason=\"x\")",
        Want::Nothing
    ),
    py_decorated!(
        "unittest.skipUnless(True)",
        "@unittest.skipUnless(True, \"x\")",
        Want::Nothing
    ),
];

const PY_SKIPIF_INHERITED: &[Case] = &[
    case(
        "module pytestmark = skipif(CI)",
        PY,
        PY_BASE,
        py!(
            "pytestmark = pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")\n",
            "",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "module pytestmark = [skipif(CI), slow]",
        PY,
        PY_BASE,
        py!(
            "pytestmark = [pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\"), pytest.mark.slow]\n",
            "",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "module pytestmark = skipif(platform)",
        PY,
        PY_BASE,
        py!(
            "pytestmark = pytest.mark.skipif(sys.platform == \"win32\", reason=\"x\")\n",
            "",
            ""
        ),
        Want::Note,
    ),
    case(
        "module pytestmark = skipif(not CI)",
        PY,
        PY_BASE,
        py!(
            "pytestmark = pytest.mark.skipif(not os.environ.get(\"CI\"), reason=\"x\")\n",
            "",
            ""
        ),
        Want::Note,
    ),
    case(
        "module pytestmark = skipif(True)",
        PY,
        PY_BASE,
        py!("pytestmark = pytest.mark.skipif(True, reason=\"x\")\n", "", ""),
        Want::Unconditional,
    ),
    case(
        "class pytestmark = skipif(CI)",
        PY,
        PY_CLASS_BASE,
        py_class!(
            "",
            "    pytestmark = pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")\n\n"
        ),
        Want::CiSkip,
    ),
    case(
        "class pytestmark = [skipif(platform)]",
        PY,
        PY_CLASS_BASE,
        py_class!(
            "",
            "    pytestmark = [pytest.mark.skipif(sys.platform == \"win32\", reason=\"x\")]\n\n"
        ),
        Want::Note,
    ),
    case(
        "class decorator skipif(CI)",
        PY,
        PY_CLASS_BASE,
        py_class!("@pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")\n", ""),
        Want::CiSkip,
    ),
    case(
        "class decorator unittest.skipUnless(platform)",
        PY,
        PY_CLASS_BASE,
        py_class!("@unittest.skipUnless(sys.platform == \"linux\", \"x\")\n", ""),
        Want::Note,
    ),
    case(
        "class decorator skipif(True)",
        PY,
        PY_CLASS_BASE,
        py_class!("@pytest.mark.skipif(True, reason=\"x\")\n", ""),
        Want::Unconditional,
    ),
    // A case of a parametrized test skipped under a condition: read as a conditional skip
    // of the test.
    case(
        "pytest.param(marks=skipif(CI))",
        PY,
        PY_BASE,
        py!(
            "",
            "@pytest.mark.parametrize(\"n\", [1, pytest.param(2, marks=pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\"))])\n",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "pytest.param(marks=skipif(platform))",
        PY,
        PY_BASE,
        py!(
            "",
            "@pytest.mark.parametrize(\"n\", [1, pytest.param(2, marks=pytest.mark.skipif(sys.platform == \"win32\", reason=\"x\"))])\n",
            ""
        ),
        Want::Note,
    ),
];

/// Controls: an unconditional skip is still an error, and an `if` around a skip is read
/// as before.
const PY_CONTROLS: &[Case] = &[
    py_decorated!("pytest.mark.skip", "@pytest.mark.skip(reason=\"x\")", Want::Unconditional),
    py_decorated!("bare pytest.mark.skip", "@pytest.mark.skip", Want::Unconditional),
    py_decorated!("unittest.skip", "@unittest.skip(\"x\")", Want::Unconditional),
    py_decorated!("pytest.mark.xfail", "@pytest.mark.xfail", Want::Unconditional),
    case(
        "module pytestmark = skip",
        PY,
        PY_BASE,
        py!("pytestmark = pytest.mark.skip(reason=\"x\")\n", "", ""),
        Want::Unconditional,
    ),
    case(
        "module pytestmark = [skipif(platform), skip]",
        PY,
        PY_BASE,
        py!(
            "pytestmark = [pytest.mark.skipif(sys.platform == \"win32\", reason=\"x\"), pytest.mark.skip]\n",
            "",
            ""
        ),
        Want::Unconditional,
    ),
    case(
        "pytest.param(marks=skip)",
        PY,
        PY_BASE,
        py!(
            "",
            "@pytest.mark.parametrize(\"n\", [1, pytest.param(2, marks=pytest.mark.skip)])\n",
            ""
        ),
        Want::Unconditional,
    ),
    case(
        "pytest.skip() at the top",
        PY,
        PY_BASE,
        py!("", "", "    pytest.skip()\n"),
        Want::Unconditional,
    ),
    case(
        "if CI: pytest.skip()",
        PY,
        PY_BASE,
        py!("", "", "    if os.environ.get(\"CI\"):\n        pytest.skip()\n"),
        Want::CiSkip,
    ),
    case(
        "if platform: pytest.skip()",
        PY,
        PY_BASE,
        py!("", "", "    if sys.platform == \"win32\":\n        pytest.skip()\n"),
        Want::Note,
    ),
    // A skip in the `else` branch of a condition on no CI variable is an unconditional
    // skip.
    case(
        "else branch of another condition",
        PY,
        PY_BASE,
        py!(
            "HAVE_DB = False\n",
            "",
            "    if HAVE_DB:\n        pass\n    else:\n        pytest.skip()\n"
        ),
        Want::Unconditional,
    ),
];

#[test]
fn python_skipif_decorators_are_conditional_skips_read_by_their_condition() {
    let wrong = differing(PY_SKIPIF);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn python_skipif_with_a_string_condition_is_parsed_or_a_note() {
    let wrong = differing(PY_SKIPIF_STRING);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn python_skipif_with_a_constant_condition_is_unconditional_or_no_skip() {
    let wrong = differing(PY_SKIPIF_CONSTANT);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn python_skipif_on_a_module_class_or_case_applies_to_its_tests() {
    let wrong = differing(PY_SKIPIF_INHERITED);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn python_unconditional_skips_and_skips_under_an_if_are_read_as_before() {
    let wrong = differing(PY_CONTROLS);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- JavaScript / TypeScript ------------------------------------------------------

/// A Vitest / Bun / Mocha file: `$module` before the test, `$call` the test function,
/// `$callback` its callback head, `$body` at the top of the callback.
macro_rules! js {
    ($module:expr, $call:expr, $callback:expr, $body:expr) => {
        concat!(
            $module,
            "\n",
            $call,
            "('adds', ",
            $callback,
            " {\n",
            $body,
            "  expect(1 + 1).toBe(2);\n});\n"
        )
    };
}

/// The same test inside a suite opened by `$suite`.
macro_rules! js_suite {
    ($suite:expr) => {
        concat!(
            $suite,
            "('math', () => {\n  test('adds', () => {\n    expect(1 + 1).toBe(2);\n  });\n});\n"
        )
    };
}

const JS_BASE: &str = js!("", "test", "() =>", "");
const JS_SUITE_BASE: &str = js_suite!("describe");

macro_rules! js_call {
    ($name:expr, $call:expr, $want:expr) => {
        case($name, JS, JS_BASE, js!("", $call, "() =>", ""), $want)
    };
}

macro_rules! js_describe {
    ($name:expr, $suite:expr, $want:expr) => {
        case($name, JS, JS_SUITE_BASE, js_suite!($suite), $want)
    };
}

const JS_SKIP_IF: &[Case] = &[
    js_call!(
        "test.skipIf(CI)",
        "test.skipIf(process.env.CI)",
        Want::CiSkip
    ),
    js_call!(
        "it.skipIf(CI)",
        "it.skipIf(!!process.env.GITHUB_ACTIONS)",
        Want::CiSkip
    ),
    js_call!(
        "test.skipIf(!CI)",
        "test.skipIf(!process.env.CI)",
        Want::Note
    ),
    js_call!(
        "test.skipIf(platform)",
        "test.skipIf(process.platform === 'win32')",
        Want::Note
    ),
    case(
        "it.skipIf(module constant)",
        JS,
        JS_BASE,
        js!(
            "const isCI = !!process.env.CI;\n",
            "it.skipIf(isCI)",
            "() =>",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "typescript test.skipIf(CI)",
        TS,
        JS_BASE,
        js!("", "test.skipIf(process.env.CI)", "() =>", ""),
        Want::CiSkip,
    ),
    js_call!(
        "test.skipIf(true)",
        "test.skipIf(true)",
        Want::Unconditional
    ),
    js_call!(
        "test.skipIf(1 === 1)",
        "test.skipIf(1 === 1)",
        Want::Unconditional
    ),
    js_call!(
        "test.skipIf(!false)",
        "test.skipIf(!false)",
        Want::Unconditional
    ),
    js_call!("test.skipIf(false)", "test.skipIf(false)", Want::Nothing),
    js_describe!(
        "describe.skipIf(CI)",
        "describe.skipIf(process.env.CI)",
        Want::CiSkip
    ),
    js_describe!(
        "describe.skipIf(!CI)",
        "describe.skipIf(!process.env.CI)",
        Want::Note
    ),
    js_describe!(
        "describe.skipIf(platform)",
        "describe.skipIf(process.platform === 'win32')",
        Want::Note
    ),
    js_describe!(
        "describe.skipIf(true)",
        "describe.skipIf(true)",
        Want::Unconditional
    ),
    js_describe!(
        "describe.skipIf(false)",
        "describe.skipIf(false)",
        Want::Nothing
    ),
];

const JS_RUN_IF: &[Case] = &[
    js_call!(
        "test.runIf(!CI)",
        "test.runIf(!process.env.CI)",
        Want::CiSkip
    ),
    js_call!("test.runIf(CI)", "test.runIf(process.env.CI)", Want::Note),
    js_call!(
        "test.runIf(platform)",
        "test.runIf(process.platform === 'linux')",
        Want::Note
    ),
    js_call!(
        "test.runIf(false)",
        "test.runIf(false)",
        Want::Unconditional
    ),
    js_call!("test.runIf(true)", "test.runIf(true)", Want::Nothing),
    js_describe!(
        "describe.runIf(!CI)",
        "describe.runIf(!process.env.CI)",
        Want::CiSkip
    ),
    js_describe!(
        "describe.runIf(CI)",
        "describe.runIf(process.env.CI)",
        Want::Note
    ),
    js_describe!(
        "describe.runIf(platform)",
        "describe.runIf(process.platform === 'linux')",
        Want::Note
    ),
    js_describe!(
        "describe.runIf(false)",
        "describe.runIf(false)",
        Want::Unconditional
    ),
    js_describe!(
        "describe.runIf(true)",
        "describe.runIf(true)",
        Want::Nothing
    ),
];

/// Mocha's `this.skip()` as a statement of a test.
const JS_THIS_SKIP: &[Case] = &[
    case(
        "this.skip() at the top",
        JS,
        JS_BASE,
        js!("", "it", "function ()", "  this.skip();\n"),
        Want::Unconditional,
    ),
    case(
        "this.skip() under another variable",
        JS,
        JS_BASE,
        js!(
            "",
            "it",
            "function ()",
            "  if (process.env.SLOW) {\n    this.skip();\n  }\n"
        ),
        Want::Note,
    ),
    case(
        "this.skip() under a platform check",
        JS,
        JS_BASE,
        js!(
            "",
            "it",
            "function ()",
            "  if (process.platform === 'win32') this.skip();\n"
        ),
        Want::Note,
    ),
    case(
        "this.skip() outside CI",
        JS,
        JS_BASE,
        js!(
            "",
            "it",
            "function ()",
            "  if (!process.env.CI) {\n    this.skip();\n  }\n"
        ),
        Want::Note,
    ),
    case(
        "this.skip() in CI",
        JS,
        JS_BASE,
        js!(
            "",
            "it",
            "function ()",
            "  if (process.env.CI) {\n    this.skip();\n  }\n"
        ),
        Want::CiSkip,
    ),
];

const JS_CONTROLS: &[Case] = &[
    js_call!("test.skip", "test.skip", Want::Unconditional),
    js_call!("it.skip", "it.skip", Want::Unconditional),
    js_call!("xit", "xit", Want::Unconditional),
    js_call!("test.todo", "test.todo", Want::Unconditional),
    js_describe!("describe.skip", "describe.skip", Want::Unconditional),
    js_describe!("xdescribe", "xdescribe", Want::Unconditional),
    case(
        "if (CI) return",
        JS,
        JS_BASE,
        js!("", "test", "() =>", "  if (process.env.CI) return;\n"),
        Want::CiSkip,
    ),
    // A call that only reads like a skip is not one.
    case(
        "other.skip() is not this.skip()",
        JS,
        JS_BASE,
        js!("", "it", "function ()", "  queue.skip();\n"),
        Want::Nothing,
    ),
];

#[test]
fn javascript_skip_if_is_a_conditional_skip_read_by_its_condition() {
    let wrong = differing(JS_SKIP_IF);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn javascript_run_if_is_a_conditional_skip_under_the_opposite_condition() {
    let wrong = differing(JS_RUN_IF);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn mocha_this_skip_is_an_unconditional_skip_or_a_conditional_one_under_an_if() {
    let wrong = differing(JS_THIS_SKIP);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn javascript_unconditional_skips_are_read_as_before() {
    let wrong = differing(JS_CONTROLS);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- Rust and Go: the forms read before this change, pinned -------------------------

macro_rules! rs {
    ($attrs:expr) => {
        concat!(
            $attrs,
            "#[test]\nfn adds() {\n    assert_eq!(1 + 1, 2);\n}\n"
        )
    };
}

macro_rules! go {
    ($body:expr) => {
        concat!(
            "package p\n\nimport (\n\t\"os\"\n\t\"runtime\"\n\t\"testing\"\n)\n\nvar _ = os.Getenv\nvar _ = runtime.GOOS\n\nfunc TestA(t *testing.T) {\n",
            $body,
            "\tif 1+1 != 2 {\n\t\tt.Fatal(\"math\")\n\t}\n}\n"
        )
    };
}

const RS_BASE: &str = rs!("");
const GO_BASE: &str = go!("");

const RS_GO_PINNED: &[Case] = &[
    case(
        "cfg_attr(ci, ignore)",
        RS,
        RS_BASE,
        rs!("#[cfg_attr(ci, ignore)]\n"),
        Want::CiSkip,
    ),
    case(
        "cfg_attr(not(ci), ignore)",
        RS,
        RS_BASE,
        rs!("#[cfg_attr(not(ci), ignore)]\n"),
        Want::Note,
    ),
    case(
        "cfg_attr(target_os, ignore)",
        RS,
        RS_BASE,
        rs!("#[cfg_attr(target_os = \"windows\", ignore)]\n"),
        Want::Note,
    ),
    case(
        "#[ignore]",
        RS,
        RS_BASE,
        rs!("#[ignore]\n"),
        Want::Unconditional,
    ),
    case(
        "go if CI { t.Skip() }",
        GO,
        GO_BASE,
        go!("\tif os.Getenv(\"CI\") != \"\" {\n\t\tt.Skip()\n\t}\n"),
        Want::CiSkip,
    ),
    case(
        "go if outside CI { t.Skip() }",
        GO,
        GO_BASE,
        go!("\tif os.Getenv(\"CI\") == \"\" {\n\t\tt.Skip()\n\t}\n"),
        Want::Note,
    ),
    case(
        "go if platform { t.Skip() }",
        GO,
        GO_BASE,
        go!("\tif runtime.GOOS == \"windows\" {\n\t\tt.Skip()\n\t}\n"),
        Want::Note,
    ),
    case(
        "go t.Skip()",
        GO,
        GO_BASE,
        go!("\tt.Skip()\n"),
        Want::Unconditional,
    ),
];

#[test]
fn rust_cfg_attr_ignore_and_go_skip_under_an_if_are_read_by_their_condition() {
    let wrong = differing(RS_GO_PINNED);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- Rust: a `cfg` on a test is judged by its parsed predicate ----------------------

const RS_CFG: &[Case] = &[
    // A feature whose name merely contains the letters is not a CI predicate.
    case(
        "cfg(feature = \"ci_skip_list\")",
        RS,
        RS_BASE,
        rs!("#[cfg(feature = \"ci_skip_list\")]\n"),
        Want::Note,
    ),
    case(
        "cfg(feature = \"skip_ci\")",
        RS,
        RS_BASE,
        rs!("#[cfg(feature = \"skip_ci\")]\n"),
        Want::Note,
    ),
    // A CI predicate in a spelling the text rule did not list.
    case(
        "cfg(not(any(miri, ci)))",
        RS,
        RS_BASE,
        rs!("#[cfg(not(any(miri, ci)))]\n"),
        Want::Unconditional,
    ),
    case(
        "cfg(not(github_actions))",
        RS,
        RS_BASE,
        rs!("#[cfg(not(github_actions))]\n"),
        Want::Unconditional,
    ),
    case(
        "cfg(all(unix, not(ci)))",
        RS,
        RS_BASE,
        rs!("#[cfg(all(unix, not(ci)))]\n"),
        Want::Unconditional,
    ),
    // Controls: the spellings the text rule listed.
    case(
        "cfg(not(ci))",
        RS,
        RS_BASE,
        rs!("#[cfg(not(ci))]\n"),
        Want::Unconditional,
    ),
    case(
        "cfg(not(any(ci, miri)))",
        RS,
        RS_BASE,
        rs!("#[cfg(not(any(ci, miri)))]\n"),
        Want::Unconditional,
    ),
    case(
        "cfg(not(skip_ci))",
        RS,
        RS_BASE,
        rs!("#[cfg(not(skip_ci))]\n"),
        Want::Unconditional,
    ),
    // Control: a predicate that holds in CI leaves the test there.
    case("cfg(ci)", RS, RS_BASE, rs!("#[cfg(ci)]\n"), Want::Nothing),
    case(
        "cfg(unix)",
        RS,
        RS_BASE,
        rs!("#[cfg(unix)]\n"),
        Want::Nothing,
    ),
];

#[test]
fn rust_cfg_on_a_test_is_judged_by_its_predicate_not_its_text() {
    let wrong = differing(RS_CFG);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- Java ---------------------------------------------------------------------------

/// A JUnit file: `$class_annotations` on the class, `$members` before the test,
/// `$annotations` on it, `$body` at its top.
macro_rules! java {
    ($class_annotations:expr, $members:expr, $annotations:expr, $body:expr) => {
        concat!(
            "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.assertEquals;\n\n",
            $class_annotations,
            "class QTest {\n",
            $members,
            $annotations,
            "    @Test\n    void adds() {\n",
            $body,
            "        assertEquals(2, 1 + 1);\n    }\n}\n"
        )
    };
}

const JAVA_BASE: &str = java!("", "", "", "");

macro_rules! java_annotated {
    ($name:expr, $annotation:expr, $want:expr) => {
        case(
            $name,
            JAVA,
            JAVA_BASE,
            java!("", "", concat!("    ", $annotation, "\n"), ""),
            $want,
        )
    };
}

macro_rules! java_first {
    ($name:expr, $statement:expr, $want:expr) => {
        case(
            $name,
            JAVA,
            JAVA_BASE,
            java!("", "", "", concat!("        ", $statement, "\n")),
            $want,
        )
    };
}

const JAVA_ANNOTATIONS: &[Case] = &[
    java_annotated!(
        "DisabledIfEnvironmentVariable(CI)",
        "@DisabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")",
        Want::CiSkip
    ),
    java_annotated!(
        "DisabledIfEnvironmentVariable(another)",
        "@DisabledIfEnvironmentVariable(named = \"SLOW\", matches = \".*\")",
        Want::Note
    ),
    java_annotated!(
        "EnabledIfEnvironmentVariable(CI)",
        "@EnabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")",
        Want::Note
    ),
    java_annotated!(
        "EnabledIfEnvironmentVariable(another)",
        "@EnabledIfEnvironmentVariable(named = \"DB_URL\", matches = \".+\")",
        Want::Note
    ),
    java_annotated!(
        "DisabledIfSystemProperty(ci)",
        "@DisabledIfSystemProperty(named = \"ci\", matches = \"true\")",
        Want::CiSkip
    ),
    java_annotated!(
        "DisabledIfSystemProperty(os.arch)",
        "@DisabledIfSystemProperty(named = \"os.arch\", matches = \".*32.*\")",
        Want::Note
    ),
    java_annotated!(
        "EnabledIfSystemProperty(ci)",
        "@EnabledIfSystemProperty(named = \"ci\", matches = \"true\")",
        Want::Note
    ),
    java_annotated!("DisabledOnOs", "@DisabledOnOs(OS.WINDOWS)", Want::Note),
    java_annotated!("EnabledOnOs", "@EnabledOnOs(OS.LINUX)", Want::Note),
    java_annotated!("DisabledOnJre", "@DisabledOnJre(JRE.JAVA_8)", Want::Note),
    java_annotated!("DisabledIf(method)", "@DisabledIf(\"isSlow\")", Want::Note),
    case(
        "class DisabledIfEnvironmentVariable(CI)",
        JAVA,
        JAVA_BASE,
        java!(
            "@DisabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")\n",
            "",
            "",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "class DisabledOnOs",
        JAVA,
        JAVA_BASE,
        java!("@DisabledOnOs(OS.WINDOWS)\n", "", "", ""),
        Want::Note,
    ),
    // Controls.
    java_annotated!("Disabled", "@Disabled(\"x\")", Want::Unconditional),
    java_annotated!("bare Disabled", "@Disabled", Want::Unconditional),
    case(
        "class Disabled",
        JAVA,
        JAVA_BASE,
        java!("@Disabled\n", "", "", ""),
        Want::Unconditional,
    ),
    java_annotated!("another annotation", "@Tag(\"slow\")", Want::Nothing),
];

const JAVA_ASSUMPTIONS: &[Case] = &[
    java_first!(
        "assumeFalse(CI != null)",
        "assumeFalse(System.getenv(\"CI\") != null);",
        Want::CiSkip
    ),
    java_first!(
        "Assumptions.assumeTrue(CI == null)",
        "Assumptions.assumeTrue(System.getenv(\"CI\") == null);",
        Want::CiSkip
    ),
    java_first!(
        "Assume.assumeTrue(GITHUB_ACTIONS == null)",
        "Assume.assumeTrue(System.getenv(\"GITHUB_ACTIONS\") == null);",
        Want::CiSkip
    ),
    java_first!(
        "assumeFalse(\"true\".equals(CI))",
        "assumeFalse(\"true\".equals(System.getenv(\"CI\")));",
        Want::CiSkip
    ),
    java_first!(
        "assumeTrue(!\"true\".equals(CI) && DB != null)",
        "assumeTrue(!\"true\".equals(System.getenv(\"CI\")) && System.getenv(\"DB\") != null);",
        Want::CiSkip
    ),
    java_first!(
        "assumeFalse(CI != null || slow)",
        "assumeFalse(System.getenv(\"CI\") != null || Boolean.getBoolean(\"slow\"));",
        Want::CiSkip
    ),
    java_first!(
        "assumingThat(CI == null, ..)",
        "assumingThat(System.getenv(\"CI\") == null, () -> assertEquals(2, 1 + 1));",
        Want::CiSkip
    ),
    java_first!(
        "assumeTrue(CI == null, message)",
        "assumeTrue(System.getenv(\"CI\") == null, \"not in CI\");",
        Want::CiSkip
    ),
    case(
        "assumeTrue(local == null)",
        JAVA,
        JAVA_BASE,
        java!(
            "",
            "",
            "",
            "        String ci = System.getenv(\"CI\");\n        assumeTrue(ci == null);\n"
        ),
        Want::CiSkip,
    ),
    case(
        "assumeFalse(helper of the file)",
        JAVA,
        JAVA_BASE,
        java!(
            "",
            "    private static boolean inCi() {\n        return System.getenv(\"CI\") != null;\n    }\n\n",
            "",
            "        assumeFalse(inCi());\n"
        ),
        Want::CiSkip,
    ),
    java_first!(
        "assumeTrue(CI != null)",
        "assumeTrue(System.getenv(\"CI\") != null);",
        Want::Note
    ),
    java_first!(
        "assumeTrue(\"true\".equals(CI))",
        "assumeTrue(\"true\".equals(System.getenv(\"CI\")));",
        Want::Note
    ),
    java_first!(
        "assumeFalse(CI == null)",
        "assumeFalse(System.getenv(\"CI\") == null);",
        Want::Note
    ),
    java_first!(
        "assumeTrue(platform)",
        "assumeTrue(System.getProperty(\"os.name\").startsWith(\"Linux\"));",
        Want::Note
    ),
    java_first!(
        "assumeTrue(another variable)",
        "assumeTrue(System.getenv(\"DB_URL\") != null);",
        Want::Note
    ),
    java_first!("assumeTrue(false)", "assumeTrue(false);", Want::Unconditional),
    java_first!("assumeFalse(true)", "assumeFalse(true);", Want::Unconditional),
    java_first!("assumeTrue(true)", "assumeTrue(true);", Want::Nothing),
    // Control: a call that is not an assumption.
    java_first!(
        "requireTrue(CI == null)",
        "requireTrue(System.getenv(\"CI\") == null);",
        Want::Nothing
    ),
];

#[test]
fn java_conditional_annotations_are_conditional_skips() {
    let wrong = differing(JAVA_ANNOTATIONS);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn java_assumptions_are_conditional_skips_read_by_their_condition() {
    let wrong = differing(JAVA_ASSUMPTIONS);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- Kotlin -------------------------------------------------------------------------

macro_rules! kt {
    ($class_annotations:expr, $annotations:expr, $body:expr) => {
        concat!(
            "import org.junit.jupiter.api.Test\nimport org.junit.jupiter.api.Assertions.assertEquals\n\n",
            $class_annotations,
            "class QTest {\n",
            $annotations,
            "    @Test\n    fun adds() {\n",
            $body,
            "        assertEquals(2, 1 + 1)\n    }\n}\n"
        )
    };
}

const KT_BASE: &str = kt!("", "", "");

macro_rules! kt_annotated {
    ($name:expr, $annotation:expr, $want:expr) => {
        case(
            $name,
            KT,
            KT_BASE,
            kt!("", concat!("    ", $annotation, "\n"), ""),
            $want,
        )
    };
}

macro_rules! kt_first {
    ($name:expr, $statement:expr, $want:expr) => {
        case(
            $name,
            KT,
            KT_BASE,
            kt!("", "", concat!("        ", $statement, "\n")),
            $want,
        )
    };
}

const KOTLIN: &[Case] = &[
    kt_annotated!(
        "DisabledIfEnvironmentVariable(CI)",
        "@DisabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")",
        Want::CiSkip
    ),
    kt_annotated!(
        "DisabledIfEnvironmentVariable(another)",
        "@DisabledIfEnvironmentVariable(named = \"SLOW\", matches = \".*\")",
        Want::Note
    ),
    kt_annotated!(
        "EnabledIfEnvironmentVariable(CI)",
        "@EnabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")",
        Want::Note
    ),
    kt_annotated!(
        "DisabledIfSystemProperty(ci)",
        "@DisabledIfSystemProperty(named = \"ci\", matches = \"true\")",
        Want::CiSkip
    ),
    kt_annotated!("DisabledOnOs", "@DisabledOnOs(OS.WINDOWS)", Want::Note),
    kt_annotated!("DisabledIf(method)", "@DisabledIf(\"isSlow\")", Want::Note),
    case(
        "class DisabledIfEnvironmentVariable(CI)",
        KT,
        KT_BASE,
        kt!(
            "@DisabledIfEnvironmentVariable(named = \"CI\", matches = \"true\")\n",
            "",
            ""
        ),
        Want::CiSkip,
    ),
    kt_first!(
        "assumeFalse(CI != null)",
        "assumeFalse(System.getenv(\"CI\") != null)",
        Want::CiSkip
    ),
    kt_first!(
        "Assumptions.assumeTrue(CI == null)",
        "Assumptions.assumeTrue(System.getenv(\"CI\") == null)",
        Want::CiSkip
    ),
    kt_first!(
        "assumeTrue(CI != \"true\" && DB != null)",
        "assumeTrue(System.getenv(\"CI\") != \"true\" && System.getenv(\"DB\") != null)",
        Want::CiSkip
    ),
    kt_first!(
        "assumeTrue(CI != null)",
        "assumeTrue(System.getenv(\"CI\") != null)",
        Want::Note
    ),
    kt_first!(
        "assumeTrue(platform)",
        "assumeTrue(System.getProperty(\"os.name\").startsWith(\"Linux\"))",
        Want::Note
    ),
    kt_first!(
        "assumeTrue(false)",
        "assumeTrue(false)",
        Want::Unconditional
    ),
    kt_first!("assumeTrue(true)", "assumeTrue(true)", Want::Nothing),
    // Controls.
    kt_annotated!("Disabled", "@Disabled(\"x\")", Want::Unconditional),
    kt_annotated!("another annotation", "@Tag(\"slow\")", Want::Nothing),
];

#[test]
fn kotlin_conditional_annotations_and_assumptions_are_conditional_skips() {
    let wrong = differing(KOTLIN);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- a CI variable added to a skip that was already CI-conditional ------------------

const APPROVE_CI: &str =
    "[meta]\nversion = 1\nname = \"t\"\n[gates.ignored-tests]\napproved_predicates = [\"CI\"]\n";
const APPROVE_BOTH: &str = "[meta]\nversion = 1\nname = \"t\"\n[gates.ignored-tests]\napproved_predicates = [\"CI\", \"GITHUB_ACTIONS\"]\n";
const CI_SKIP_WARNING: &str =
    "[meta]\nversion = 1\nname = \"t\"\n[gates.ignored-tests]\nci_skip_severity = \"warning\"\n";

const GO_CI: &str = go!("\tif os.Getenv(\"CI\") != \"\" {\n\t\tt.Skip()\n\t}\n");
const GO_CI_OR_GITHUB: &str = go!(
    "\tif os.Getenv(\"CI\") != \"\" || os.Getenv(\"GITHUB_ACTIONS\") != \"\" {\n\t\tt.Skip()\n\t}\n"
);
const GO_GITHUB: &str = go!("\tif os.Getenv(\"GITHUB_ACTIONS\") != \"\" {\n\t\tt.Skip()\n\t}\n");
const GO_CI_AND_SHORT: &str =
    go!("\tif os.Getenv(\"CI\") != \"\" && testing.Short() {\n\t\tt.Skip()\n\t}\n");
const GO_CI_REWORDED: &str = go!("\tif \"\" != os.Getenv(\"CI\") {\n\t\tt.Skip()\n\t}\n");

const fn added(
    name: &'static str,
    base: &'static str,
    head: &'static str,
    config: &'static str,
    want: Want,
    message: &'static str,
) -> Case {
    Case {
        name,
        path: GO,
        base,
        head,
        config,
        want,
        message,
    }
}

const ADDED_CI_VARIABLE: &[Case] = &[
    added(
        "CI -> CI || GITHUB_ACTIONS",
        GO_CI,
        GO_CI_OR_GITHUB,
        "",
        Want::CiSkip,
        "adds CI variable `GITHUB_ACTIONS`",
    ),
    added(
        "CI (approved) -> CI || GITHUB_ACTIONS",
        GO_CI,
        GO_CI_OR_GITHUB,
        APPROVE_CI,
        Want::CiSkip,
        "adds CI variable `GITHUB_ACTIONS`",
    ),
    added(
        "CI -> GITHUB_ACTIONS",
        GO_CI,
        GO_GITHUB,
        "",
        Want::CiSkip,
        "adds CI variable `GITHUB_ACTIONS`",
    ),
];

/// Controls: nothing is reported when no unapproved CI variable is added.
const NO_ADDED_CI_VARIABLE: &[Case] = &[
    added(
        "CI -> CI || GITHUB_ACTIONS, both approved",
        GO_CI,
        GO_CI_OR_GITHUB,
        APPROVE_BOTH,
        Want::Nothing,
        "",
    ),
    added(
        "unchanged",
        GO_CI_OR_GITHUB,
        GO_CI_OR_GITHUB,
        "",
        Want::Nothing,
        "",
    ),
    added(
        "narrower: CI -> CI && short",
        GO_CI,
        GO_CI_AND_SHORT,
        "",
        Want::Nothing,
        "",
    ),
    added("reworded", GO_CI, GO_CI_REWORDED, "", Want::Nothing, ""),
    added(
        "a variable removed: CI || GITHUB_ACTIONS -> CI",
        GO_CI_OR_GITHUB,
        GO_CI,
        "",
        Want::Nothing,
        "",
    ),
];

#[test]
fn a_ci_variable_added_to_a_ci_conditional_skip_is_reported_unless_approved() {
    let wrong = differing(ADDED_CI_VARIABLE);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_ci_conditional_skip_that_gains_no_unapproved_ci_variable_is_not_reported() {
    let wrong = differing(NO_ADDED_CI_VARIABLE);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn ci_skip_severity_sets_the_severity_of_an_added_ci_variable() {
    let seen = run_change(&added(
        "warning",
        GO_CI,
        GO_CI_OR_GITHUB,
        CI_SKIP_WARNING,
        Want::CiSkip,
        "",
    ));
    assert_eq!(seen.code, 0, "{:?}", seen.findings);
    assert_eq!(seen.findings.len(), 1, "{:?}", seen.findings);
    assert_eq!(seen.findings[0].0, "Test Conditionally Skipped");
    assert_eq!(seen.findings[0].1, "warning");
}

// --- a name spelled as a CI variable, bound to something the file does not define ---

const CI_NAME_BOUND_ELSEWHERE: &[Case] = &[
    case(
        "python CI = helpers.in_ci()",
        PY,
        PY_BASE,
        py!(
            "CI = helpers.in_ci()\n",
            "",
            "    if CI:\n        pytest.skip()\n"
        ),
        Want::CiSkip,
    ),
    case(
        "python CI = settings.ci_mode",
        PY,
        PY_BASE,
        py!(
            "CI = settings.ci_mode\n",
            "",
            "    if CI:\n        pytest.skip()\n"
        ),
        Want::CiSkip,
    ),
    case(
        "python skipif(CI = helpers.in_ci())",
        PY,
        PY_BASE,
        py!(
            "GITHUB_ACTIONS = helpers.on_github()\n",
            "@pytest.mark.skipif(GITHUB_ACTIONS, reason=\"x\")\n",
            ""
        ),
        Want::CiSkip,
    ),
    case(
        "javascript const CI = helpers.inCi()",
        JS,
        JS_BASE,
        js!(
            "const CI = helpers.inCi();\n",
            "test",
            "() =>",
            "  if (CI) return;\n"
        ),
        Want::CiSkip,
    ),
    case(
        "go CI := helpers.InCI()",
        GO,
        GO_BASE,
        go!("\tCI := helpers.InCI()\n\tif CI {\n\t\tt.Skip()\n\t}\n"),
        Want::CiSkip,
    ),
];

/// Controls: the same name bound to a literal, to a read of another variable, or to a
/// helper of the file that reads no CI variable is a conditional skip on no CI variable.
const CI_NAME_BOUND_TO_SOMETHING_ELSE: &[Case] = &[
    case(
        "python CI = False",
        PY,
        PY_BASE,
        py!("CI = False\n", "", "    if CI:\n        pytest.skip()\n"),
        Want::Note,
    ),
    case(
        "python CI = read of another variable",
        PY,
        PY_BASE,
        py!(
            "CI = os.environ.get(\"FLAVOR\")\n",
            "",
            "    if CI:\n        pytest.skip()\n"
        ),
        Want::Note,
    ),
    case(
        "python CI = helper of the file",
        PY,
        PY_BASE,
        py!(
            "def slow():\n    return sys.platform == \"win32\"\n\nCI = slow()\n",
            "",
            "    if CI:\n        pytest.skip()\n"
        ),
        Want::Note,
    ),
    case(
        "python lower-case ci = make_client()",
        PY,
        PY_BASE,
        py!(
            "",
            "",
            "    ci = make_client()\n    if ci:\n        pytest.skip()\n"
        ),
        Want::Note,
    ),
    case(
        "go CI := false",
        GO,
        GO_BASE,
        go!("\tCI := false\n\tif CI {\n\t\tt.Skip()\n\t}\n"),
        Want::Note,
    ),
];

#[test]
fn a_ci_spelled_name_bound_to_a_call_the_file_does_not_define_is_a_ci_skip() {
    let wrong = differing(CI_NAME_BOUND_ELSEWHERE);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_ci_spelled_name_bound_to_a_literal_or_a_resolved_value_stays_a_note() {
    let wrong = differing(CI_NAME_BOUND_TO_SOMETHING_ELSE);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- approved_predicates and the message of a decorator skip ------------------------

const SKIPIF_CI: &str = py!(
    "",
    "@pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")\n",
    ""
);

const SKIPIF_CI_REWORDED: &str = py!(
    "",
    "@pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"another reason\")\n",
    ""
);

const SKIPIF_MESSAGES: &[Case] = &[
    Case {
        name: "the message carries the condition",
        path: PY,
        base: PY_BASE,
        head: SKIPIF_CI,
        config: "",
        want: Want::CiSkip,
        message:
            "Test `test_query` is conditionally skipped under predicate `os.environ.get(\"CI\")`.",
    },
    Case {
        name: "an approved predicate waives it",
        path: PY,
        base: PY_BASE,
        head: SKIPIF_CI,
        config: APPROVE_CI,
        want: Want::Nothing,
        message: "",
    },
    Case {
        name: "skipUnless reports the opposite condition",
        path: PY,
        base: PY_BASE,
        head: py!(
            "",
            "@unittest.skipUnless(not os.environ.get(\"CI\"), \"x\")\n",
            ""
        ),
        config: "",
        want: Want::CiSkip,
        message: "under predicate `not (not os.environ.get(\"CI\"))`",
    },
    // Unchanged on both sides: the gate reports a change, not a state.
    Case {
        name: "a decorator on both sides is not reported",
        path: PY,
        base: SKIPIF_CI,
        head: SKIPIF_CI_REWORDED,
        config: "",
        want: Want::Nothing,
        message: "",
    },
];

#[test]
fn a_skipif_ci_skip_names_its_condition_and_takes_approved_predicates() {
    let wrong = differing(SKIPIF_MESSAGES);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
