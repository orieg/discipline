//! Conditional skips the packs read before, in the forms left open by #613 (#629): a
//! pytest mark reached through a name of the file, `@pytest.mark.xfail(<condition>)`, a
//! Rust `cfg` on a Cargo feature named as a CI variable, a skip in the `else` branch of a
//! condition, and JUnit's `assumeThat`, assumptions under an `if` and
//! `@DisabledIf("method")`.
//!
//! Each case drives the real binary over a base side whose test runs and a head side
//! whose test carries the skip. The sources under test are fixtures built by this file,
//! not tests of this suite.

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
    name: String,
    path: &'static str,
    base: String,
    head: String,
    config: &'static str,
    /// Other files of the base side.
    extra: &'static [(&'static str, &'static str)],
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
    let mut files = vec![(case.path, case.base.as_str())];
    files.extend_from_slice(case.extra);
    if !case.config.is_empty() {
        files.push(("discipline.toml", case.config));
    }
    repo.commit_base_files(&files, "test: base suite");
    repo.write(case.path, &case.head);
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

fn check(cases: Vec<Case>) {
    assert!(!cases.is_empty());
    let wrong = differing(&cases);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A file of one language: what goes beside the test, on it, and at the top of its body.
type Template = fn(&str, &str, &str) -> String;

/// Cases whose head side puts `body` at the top of the test body.
fn bodies(path: &'static str, file: Template, rows: &[(&str, Want)]) -> Vec<Case> {
    rows.iter()
        .map(|(body, want)| Case {
            name: format!("{path}: {body}"),
            path,
            base: file("", "", ""),
            head: file("", "", body),
            config: "",
            extra: &[],
            want: *want,
            message: "",
        })
        .collect()
}

/// Cases whose head side adds members beside the test, marks on it and a body line.
fn shaped(path: &'static str, file: Template, rows: &[(&str, &str, &str, Want)]) -> Vec<Case> {
    rows.iter()
        .map(|(members, marks, body, want)| Case {
            name: format!("{path}: {members} | {marks} | {body}"),
            path,
            base: file("", "", ""),
            head: file(members, marks, body),
            config: "",
            extra: &[],
            want: *want,
            message: "",
        })
        .collect()
}

use Want::{CiSkip, Note, Nothing, Unconditional};

// --- Python -----------------------------------------------------------------------

const PY: &str = "test_query.py";

fn py(module: &str, decorators: &str, body: &str) -> String {
    let decorators = if decorators.is_empty() {
        String::new()
    } else {
        format!("{decorators}\n")
    };
    let body = if body.is_empty() {
        String::new()
    } else {
        format!("    {body}\n")
    };
    format!(
        "import os\nimport sys\nimport unittest\nimport pytest\n\n{module}\n\n{decorators}def test_query():\n{body}    assert 1 + 1 == 2\n"
    )
}

#[test]
fn python_a_mark_bound_to_a_name_of_the_file_is_judged_by_its_assignment() {
    check(shaped(
        PY,
        py,
        &[
            (
                "off_ci_only = pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")",
                "@off_ci_only",
                "",
                CiSkip,
            ),
            (
                "ci_only = pytest.mark.skipif(not os.environ.get(\"CI\"), reason=\"x\")",
                "@ci_only",
                "",
                Note,
            ),
            (
                "requires_db = pytest.mark.skipif(not shutil.which(\"psql\"), reason=\"x\")",
                "@requires_db",
                "",
                Note,
            ),
            (
                "IN_CI = bool(os.environ.get(\"CI\"))\nnot_on_ci = pytest.mark.skipif(IN_CI, reason=\"x\")",
                "@not_on_ci",
                "",
                CiSkip,
            ),
            (
                "posix = unittest.skipIf(sys.platform == \"win32\", \"x\")",
                "@posix",
                "",
                Note,
            ),
            (
                "off_ci = unittest.skipUnless(not os.environ.get(\"CI\"), \"x\")",
                "@off_ci",
                "",
                CiSkip,
            ),
            (
                "parked = pytest.mark.skip(reason=\"x\")",
                "@parked",
                "",
                Unconditional,
            ),
            ("parked = pytest.mark.skip", "@parked", "", Unconditional),
            (
                "always = pytest.mark.skipif(True, reason=\"x\")",
                "@always",
                "",
                Unconditional,
            ),
            (
                "never = pytest.mark.skipif(False, reason=\"x\")",
                "@never",
                "",
                Nothing,
            ),
            // A name that starts with `skip` is read by its assignment too, not by its
            // spelling.
            (
                "skip_on_ci = pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")",
                "@skip_on_ci",
                "",
                CiSkip,
            ),
            (
                "skip_windows = pytest.mark.skipif(sys.platform == \"win32\", reason=\"x\")",
                "@skip_windows",
                "",
                Note,
            ),
            // The same names in `pytestmark`.
            (
                "requires_db = pytest.mark.skipif(not shutil.which(\"psql\"), reason=\"x\")\npytestmark = requires_db",
                "",
                "",
                Note,
            ),
            (
                "off_ci = pytest.mark.skipif(os.environ.get(\"CI\"), reason=\"x\")\npytestmark = [pytest.mark.slow, off_ci]",
                "",
                "",
                CiSkip,
            ),
            ("from marks import requires_db\npytestmark = requires_db", "", "", Nothing),
            // A name another file binds is not followed: one spelled `skip..` is an
            // unconditional skip by its spelling, as before.
            ("from marks import skip_windows", "@skip_windows", "", Unconditional),
            // A mark that skips nothing, and a name another file binds.
            ("slow = pytest.mark.slow", "@slow", "", Nothing),
            ("from marks import requires_db", "@requires_db", "", Nothing),
            (
                "printer = functools.partial(print, os.environ.get(\"CI\"))",
                "@printer",
                "",
                Nothing,
            ),
        ],
    ));
}

#[test]
fn python_xfail_with_a_condition_is_judged_by_it() {
    check(shaped(
        PY,
        py,
        &[
            (
                "",
                "@pytest.mark.xfail(os.environ.get(\"CI\"), reason=\"x\")",
                "",
                CiSkip,
            ),
            (
                "",
                "@pytest.mark.xfail(condition=os.environ.get(\"GITHUB_ACTIONS\") == \"true\", reason=\"x\")",
                "",
                CiSkip,
            ),
            (
                "",
                "@pytest.mark.xfail(not os.environ.get(\"CI\"), reason=\"x\")",
                "",
                Note,
            ),
            (
                "",
                "@pytest.mark.xfail(sys.platform == \"win32\", reason=\"x\", strict=True)",
                "",
                Note,
            ),
            (
                "",
                "@pytest.mark.xfail(\"os.environ.get('CI')\", reason=\"x\")",
                "",
                CiSkip,
            ),
            (
                "",
                "@pytest.mark.xfail(\"sys.platform == 'win32'\")",
                "",
                Note,
            ),
            ("", "@pytest.mark.xfail(True, reason=\"x\")", "", Unconditional),
            ("", "@pytest.mark.xfail(False, reason=\"x\")", "", Nothing),
            (
                "known_in_ci = pytest.mark.xfail(os.environ.get(\"CI\"), reason=\"x\")",
                "@known_in_ci",
                "",
                CiSkip,
            ),
            // No condition: as before.
            ("", "@pytest.mark.xfail", "", Unconditional),
            ("", "@pytest.mark.xfail(reason=\"x\")", "", Unconditional),
            (
                "",
                "@pytest.mark.xfail(reason=\"x\", raises=ValueError, strict=True)",
                "",
                Unconditional,
            ),
            (
                "known = pytest.mark.xfail(reason=\"x\")",
                "@known",
                "",
                Unconditional,
            ),
        ],
    ));
}

// --- the else branch ----------------------------------------------------------------

const GO: &str = "p_test.go";
const JS: &str = "a.test.js";

fn go(_module: &str, _marks: &str, body: &str) -> String {
    format!(
        "package p\n\nimport (\n\t\"os\"\n\t\"runtime\"\n\t\"testing\"\n)\n\nvar _ = os.Getenv\nvar _ = runtime.GOOS\n\nfunc TestA(t *testing.T) {{\n\t{body}\n\tif 1+1 != 2 {{\n\t\tt.Fatal(\"math\")\n\t}}\n}}\n"
    )
}

fn mocha(_module: &str, _marks: &str, body: &str) -> String {
    format!(
        "const assert = require('assert');\n\nit('adds', function () {{\n  {body}\n  assert.strictEqual(1 + 1, 2);\n}});\n"
    )
}

fn with_go_mod(mut cases: Vec<Case>) -> Vec<Case> {
    for case in &mut cases {
        case.extra = &[("go.mod", "module example.com/p\n\ngo 1.22\n")];
    }
    cases
}

#[test]
fn a_skip_in_the_else_branch_runs_under_the_negated_condition() {
    check(bodies(
        PY,
        py,
        &[
            (
                "if shutil.which(\"psql\"):\n        pass\n    else:\n        pytest.skip(\"x\")",
                Note,
            ),
            (
                "if os.environ.get(\"CI\"):\n        pass\n    else:\n        pytest.skip(\"x\")",
                Note,
            ),
            (
                "if not os.environ.get(\"CI\"):\n        pass\n    else:\n        pytest.skip(\"x\")",
                CiSkip,
            ),
            (
                "if shutil.which(\"psql\"):\n        pass\n    elif sys.platform == \"win32\":\n        pass\n    else:\n        pytest.skip(\"x\")",
                Note,
            ),
            // Constants are no condition.
            (
                "if False:\n        pass\n    else:\n        pytest.skip(\"x\")",
                Unconditional,
            ),
            ("pytest.skip(\"x\")", Unconditional),
        ],
    ));
    check(with_go_mod(bodies(
        GO,
        go,
        &[
            (
                "if runtime.GOOS == \"linux\" {\n\t\tt.Log(\"a\")\n\t} else {\n\t\tt.Skip(\"x\")\n\t}",
                Note,
            ),
            (
                "if os.Getenv(\"CI\") != \"\" {\n\t\tt.Log(\"a\")\n\t} else {\n\t\tt.Skip(\"x\")\n\t}",
                Note,
            ),
            (
                "if os.Getenv(\"CI\") == \"\" {\n\t\tt.Log(\"a\")\n\t} else {\n\t\tt.Skip(\"x\")\n\t}",
                CiSkip,
            ),
            ("t.Skip(\"x\")", Unconditional),
        ],
    )));
    check(bodies(
        JS,
        mocha,
        &[
            (
                "if (process.platform === 'linux') { console.log(1); } else { this.skip(); }",
                Note,
            ),
            (
                "if (process.env.CI) { console.log(1); } else { this.skip(); }",
                Note,
            ),
            (
                "if (!process.env.CI) { console.log(1); } else { this.skip(); }",
                CiSkip,
            ),
            (
                "if (false) { console.log(1); } else { this.skip(); }",
                Unconditional,
            ),
            ("this.skip();", Unconditional),
        ],
    ));
}

// --- Rust ---------------------------------------------------------------------------

const RS: &str = "tests/q.rs";

fn rs(_module: &str, attrs: &str, _body: &str) -> String {
    let attrs = if attrs.is_empty() {
        String::new()
    } else {
        format!("{attrs}\n")
    };
    format!("{attrs}#[test]\nfn adds() {{\n    assert_eq!(1 + 1, 2);\n}}\n")
}

/// `feature = "ci"` is read where it stands in the predicate, like the bare cfg `ci`: a
/// feature whose name only contains those letters is not a CI predicate, and a predicate
/// that holds where the feature is off skips outside CI.
#[test]
fn rust_a_cfg_on_a_feature_named_ci_is_read_from_the_predicate() {
    check(shaped(
        RS,
        rs,
        &[
            ("", "#[cfg_attr(feature = \"ci\", ignore)]", "", CiSkip),
            (
                "",
                "#[cfg_attr(all(unix, feature = \"ci\"), ignore)]",
                "",
                CiSkip,
            ),
            ("", "#[cfg_attr(not(feature = \"ci\"), ignore)]", "", Note),
            ("", "#[cfg_attr(feature = \"slow-ci\", ignore)]", "", Note),
            ("", "#[cfg_attr(feature = \"ci-tools\", ignore)]", "", Note),
            ("", "#[cfg_attr(target_os = \"ci\", ignore)]", "", Note),
            // The test is built only with the feature: skipped outside CI.
            ("", "#[cfg(feature = \"ci\")]", "", Note),
            ("", "#[cfg(feature = \"ci-tools\")]", "", Note),
            // Left out of every build that has the feature.
            ("", "#[cfg(not(feature = \"ci\"))]", "", Unconditional),
        ],
    ));
}

// --- JUnit --------------------------------------------------------------------------

const JAVA: &str = "src/test/java/QTest.java";
const KT: &str = "src/test/kotlin/QTest.kt";

fn java(members: &str, annotations: &str, body: &str) -> String {
    format!(
        "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.assertEquals;\n\nclass QTest {{\n    {members}\n    {annotations}\n    @Test\n    void adds() {{\n        {body}\n        assertEquals(2, 1 + 1);\n    }}\n}}\n"
    )
}

fn kt(members: &str, annotations: &str, body: &str) -> String {
    format!(
        "import org.junit.jupiter.api.Test\nimport org.junit.jupiter.api.Assertions.assertEquals\n\nclass QTest {{\n    {members}\n    {annotations}\n    @Test\n    fun adds() {{\n        {body}\n        assertEquals(2, 1 + 1)\n    }}\n}}\n"
    )
}

#[test]
fn junit_assume_that_and_assume_not_null_are_read_by_the_value_and_the_matcher() {
    check(bodies(
        JAVA,
        java,
        &[
            ("assumeThat(System.getenv(\"CI\"), nullValue());", CiSkip),
            (
                "Assume.assumeThat(System.getenv(\"CI\"), is(nullValue()));",
                CiSkip,
            ),
            ("assumeThat(System.getenv(\"CI\"), notNullValue());", Note),
            ("assumeThat(System.getenv(\"CI\"), is(\"true\"));", Note),
            (
                "assumeThat(System.getenv(\"CI\"), equalTo(\"false\"));",
                CiSkip,
            ),
            (
                "assumeThat(\"only off CI\", System.getenv(\"CI\"), nullValue());",
                CiSkip,
            ),
            (
                "assumeThat(System.getProperty(\"os.name\"), is(\"Linux\"));",
                Note,
            ),
            ("assumeThat(System.getenv(\"HOME\"), notNullValue());", Note),
            ("assumeNotNull(System.getenv(\"CI\"));", Note),
            ("assumeNotNull(System.getenv(\"DATABASE_URL\"));", Note),
        ],
    ));
    check(bodies(
        KT,
        kt,
        &[
            ("assumeThat(System.getenv(\"CI\"), nullValue())", CiSkip),
            ("assumeThat(System.getenv(\"CI\"), notNullValue())", Note),
            (
                "assumeThat(System.getProperty(\"os.name\"), equalTo(\"Linux\"))",
                Note,
            ),
        ],
    ));
}

#[test]
fn junit_an_assumption_under_an_if_is_read_with_the_condition_around_it() {
    check(bodies(
        JAVA,
        java,
        &[
            (
                "if (System.getenv(\"CI\") != null) { assumeTrue(databaseUp()); }",
                CiSkip,
            ),
            (
                "if (System.getenv(\"CI\") == null) { assumeTrue(databaseUp()); }",
                Note,
            ),
            (
                "if (isLinux()) { assumeTrue(System.getenv(\"CI\") == null); }",
                CiSkip,
            ),
            (
                "if (isLinux()) { assumeTrue(System.getenv(\"CI\") != null); }",
                Note,
            ),
            ("if (isLinux()) { assumeTrue(databaseUp()); }", Note),
            (
                "if (System.getenv(\"CI\") == null) { run(); } else { assumeTrue(databaseUp()); }",
                CiSkip,
            ),
            (
                "if (System.getenv(\"CI\") != null) { run(); } else { assumeTrue(databaseUp()); }",
                Note,
            ),
            ("if (isLinux()) { assumeTrue(false); }", Note),
            ("if (isLinux()) { assumeTrue(true); }", Nothing),
            ("if (isLinux()) { run(); }", Nothing),
        ],
    ));
    check(bodies(
        KT,
        kt,
        &[
            (
                "if (System.getenv(\"CI\") != null) { assumeTrue(databaseUp()) }",
                CiSkip,
            ),
            (
                "if (System.getenv(\"CI\") == null) { assumeTrue(databaseUp()) }",
                Note,
            ),
            (
                "if (isLinux()) { assumeTrue(System.getenv(\"CI\") == null) }",
                CiSkip,
            ),
            ("if (isLinux()) { assumeTrue(databaseUp()) }", Note),
            (
                "if (System.getenv(\"CI\") == null) { run() } else { assumeTrue(databaseUp()) }",
                CiSkip,
            ),
            (
                "if (System.getenv(\"CI\") != null) { run() } else { assumeTrue(databaseUp()) }",
                Note,
            ),
            ("if (isLinux()) { run() }", Nothing),
        ],
    ));
}

const JAVA_ON_CI: &str = "boolean onCi() { return System.getenv(\"CI\") != null; }";
const JAVA_OFF_CI: &str = "static boolean offCi() { return System.getenv(\"CI\") == null; }";
const KT_ON_CI: &str = "fun onCi(): Boolean = System.getenv(\"CI\") != null";
const KT_OFF_CI: &str = "fun offCi(): Boolean { return System.getenv(\"CI\") == null }";

#[test]
fn junit_disabled_if_and_enabled_if_are_read_by_the_method_of_the_class_they_name() {
    check(shaped(
        JAVA,
        java,
        &[
            (JAVA_ON_CI, "@DisabledIf(\"onCi\")", "", CiSkip),
            (JAVA_OFF_CI, "@DisabledIf(\"offCi\")", "", Note),
            (JAVA_ON_CI, "@EnabledIf(\"onCi\")", "", Note),
            (JAVA_OFF_CI, "@EnabledIf(\"offCi\")", "", CiSkip),
            (
                JAVA_ON_CI,
                "@DisabledIf(value = \"onCi\", disabledReason = \"x\")",
                "",
                CiSkip,
            ),
            (
                "boolean onLinux() { return System.getProperty(\"os.name\").startsWith(\"Linux\"); }",
                "@DisabledIf(\"onLinux\")",
                "",
                Note,
            ),
            // A method of another class, and a name the class does not have.
            (
                JAVA_ON_CI,
                "@DisabledIf(\"com.example.Conditions#onCi\")",
                "",
                Note,
            ),
            ("", "@DisabledIf(\"onCi\")", "", Note),
        ],
    ));
    check(shaped(
        KT,
        kt,
        &[
            (KT_ON_CI, "@DisabledIf(\"onCi\")", "", CiSkip),
            (KT_OFF_CI, "@DisabledIf(\"offCi\")", "", Note),
            (KT_ON_CI, "@EnabledIf(\"onCi\")", "", Note),
            (KT_OFF_CI, "@EnabledIf(\"offCi\")", "", CiSkip),
            ("", "@DisabledIf(\"onCi\")", "", Note),
        ],
    ));
}

// --- names that are only spelled like a CI variable --------------------------------

/// `BUILD_ID` and the `CI_` prefix name a CI variable as the argument of an environment
/// read. An identifier with that spelling is not one.
#[test]
fn build_id_and_the_ci_prefix_count_in_an_environment_read_only() {
    check(bodies(
        PY,
        py,
        &[
            (
                "if os.environ.get(\"BUILD_ID\"):\n        pytest.skip(\"x\")",
                CiSkip,
            ),
            (
                "if os.getenv(\"CI_JOB_ID\"):\n        pytest.skip(\"x\")",
                CiSkip,
            ),
            (
                "if \"CI_COMMIT_SHA\" in os.environ:\n        pytest.skip(\"x\")",
                CiSkip,
            ),
            ("if BUILD_ID:\n        pytest.skip(\"x\")", Note),
            ("if settings.BUILD_ID:\n        pytest.skip(\"x\")", Note),
            ("if CI_JOB_ID:\n        pytest.skip(\"x\")", Note),
            (
                "if build_id(\"CI_JOB_ID\"):\n        pytest.skip(\"x\")",
                Note,
            ),
        ],
    ));
    check(with_go_mod(bodies(
        GO,
        go,
        &[
            (
                "if os.Getenv(\"BUILD_ID\") != \"\" {\n\t\tt.Skip(\"x\")\n\t}",
                CiSkip,
            ),
            (
                "if os.Getenv(\"CI_JOB_ID\") != \"\" {\n\t\tt.Skip(\"x\")\n\t}",
                CiSkip,
            ),
            ("if BUILD_ID != \"\" {\n\t\tt.Skip(\"x\")\n\t}", Note),
            ("if cfg.CI_JOB_ID != \"\" {\n\t\tt.Skip(\"x\")\n\t}", Note),
        ],
    )));
    check(bodies(
        JS,
        mocha,
        &[
            ("if (process.env.BUILD_ID) { this.skip(); }", CiSkip),
            ("if (process.env.CI_JOB_ID) { this.skip(); }", CiSkip),
            ("if (BUILD_ID) { this.skip(); }", Note),
            ("if (config.CI_JOB_ID) { this.skip(); }", Note),
        ],
    ));
}

// --- the documented default of ci_skip_severity --------------------------------------

/// The generated configuration reference gives the default of `ci_skip_severity` as the
/// gate's severity, which is what an unset key resolves to.
#[test]
fn the_configuration_reference_gives_the_default_of_ci_skip_severity() {
    let reference = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/CONFIGURATION.md"),
    )
    .unwrap();
    let row = reference
        .lines()
        .find(|l| l.starts_with("| `gates.ignored-tests.ci_skip_severity` |"))
        .expect("the reference has a row for ci_skip_severity");
    let cells: Vec<&str> = row.split('|').map(str::trim).collect();
    assert_eq!(cells[3], "*(the gate's `severity`)*", "{row}");

    // The key resolves that way: with the gate at `warning` and no `ci_skip_severity`, a
    // CI-conditional skip is a warning.
    let seen = run_change(&Case {
        name: "gate severity".to_string(),
        path: PY,
        base: py("", "", ""),
        head: py(
            "",
            "",
            "if os.environ.get(\"CI\"):\n        pytest.skip(\"x\")",
        ),
        config: "[gates.ignored-tests]\nseverity = \"warning\"\n",
        extra: &[],
        want: CiSkip,
        message: "",
    });
    assert_eq!(seen.findings.len(), 1, "{:?}", seen.findings);
    assert_eq!(seen.findings[0].0, "Test Conditionally Skipped");
    assert_eq!(seen.findings[0].1, "warning");
}
