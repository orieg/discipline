//! `assertion-reduction` and the helpers a change does not hold in one file (#626):
//!
//! - a function of a file under a test directory that holds no test is an assertion
//!   helper only when a test names it;
//! - a helper in a test-support file the change does not touch stands for the checks it
//!   holds, as one in a changed file does;
//! - a failure exit guarded by an equality comparison in a helper of another file covers
//!   a dropped equality assertion, as one in a helper of the test's own file does;
//! - gocheck suite methods are tests, and a local bound to a testify assertion object
//!   carries assertions.
//!
//! Each shape is driven through the real binary, with a control that pins the other
//! verdict beside it.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WEAKENED: &str = "Test Helper Function Weakened";
const DECREASED: &str = "Assertion Count Decreased In Existing Test";
const VACUOUS: &str = "Vacuous Test Added";

/// Commits `base` on `main`, then `head` on a branch, and runs `check`.
fn change(base: &[(&str, &str)], head: &[(&str, &str)]) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    for (path, content) in base {
        repo.write(path, content);
    }
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    for (path, content) in head {
        repo.write(path, content);
    }
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

/// What `gate` reported: `(title, file)` of each violation.
fn reported(run: &Run, gate: &str) -> Vec<(String, String)> {
    run.violations(gate)
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["file"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn one(title: &str, file: &str) -> Vec<(String, String)> {
    vec![(title.to_string(), file.to_string())]
}

fn notes(run: &Run) -> String {
    run.outcome("assertion-reduction")["notes"].to_string()
}

const GEN: &str = "tests/support/gen.rs";

fn gen_rs(call: &str) -> String {
    format!("pub fn sample() -> u32 {{\n    let v = load().{call}();\n    v\n}}\n")
}

/// `tests/support/gen.rs` going from `.unwrap()` to `.unwrap_or_default()` weakens no
/// test when no test calls the function: it was reported as a weakened helper.
#[test]
fn a_function_no_test_names_is_not_an_assertion_helper() {
    let run = change(
        &[(GEN, &gen_rs("unwrap"))],
        &[(GEN, &gen_rs("unwrap_or_default"))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
}

/// Control: the same edit is reported when a test file the change does not touch calls
/// the function, when a changed test does, and when a helper that a test calls does.
#[test]
fn a_function_a_test_names_is_still_an_assertion_helper() {
    let unchanged =
        "mod support;\n\n#[test]\nfn uses() {\n    assert_eq!(support::gen::sample(), 3);\n}\n";
    let run = change(
        &[(GEN, &gen_rs("unwrap")), ("tests/uses.rs", unchanged)],
        &[(GEN, &gen_rs("unwrap_or_default"))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(WEAKENED, GEN),
        "{}",
        run.stdout
    );

    let touched = format!("// Reads the sample.\n{unchanged}");
    let run = change(
        &[(GEN, &gen_rs("unwrap")), ("tests/uses.rs", unchanged)],
        &[
            (GEN, &gen_rs("unwrap_or_default")),
            ("tests/uses.rs", &touched),
        ],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(WEAKENED, GEN),
        "{}",
        run.stdout
    );

    // Named through another function of the support file, which a test calls.
    let through = |call: &str| {
        format!(
            "{}\npub fn doubled() -> u32 {{\n    sample() * 2\n}}\n",
            gen_rs(call)
        )
    };
    let caller =
        "mod support;\n\n#[test]\nfn uses() {\n    assert_eq!(support::gen::doubled(), 6);\n}\n";
    let run = change(
        &[(GEN, &through("unwrap")), ("tests/uses.rs", caller)],
        &[(GEN, &through("unwrap_or_default"))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(WEAKENED, GEN),
        "{}",
        run.stdout
    );
}

const PY_HELPERS: &str = "tests/helpers.py";

fn py_helper(n: usize) -> String {
    let body: String = (0..n)
        .map(|i| format!("    assert r.f{i} == {i}\n"))
        .collect();
    let body = if body.is_empty() {
        "    pass\n".to_string()
    } else {
        body
    };
    format!("def check(r):\n{body}")
}

const PY_CALLING: &str =
    "from helpers import check\n\n\ndef test_create():\n    r = create()\n    check(r)\n";

/// The same rule in another pack, for a helper that asserts: one no test names is not
/// reported for losing its assertions; one a test of an unchanged file calls is. A
/// pytest `conftest.py` is judged as before, since a fixture is received as a parameter
/// and not called by name.
#[test]
fn a_python_helper_is_judged_by_whether_a_test_names_it() {
    let run = change(
        &[(PY_HELPERS, &py_helper(3))],
        &[(PY_HELPERS, &py_helper(1))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );

    let run = change(
        &[
            (PY_HELPERS, &py_helper(3)),
            ("tests/test_api.py", PY_CALLING),
        ],
        &[(PY_HELPERS, &py_helper(1))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(WEAKENED, PY_HELPERS),
        "{}",
        run.stdout
    );

    // A file that names another function of that name's file, but not this one.
    let other = "from helpers import build\n\n\ndef test_create():\n    assert build() == 1\n";
    let run = change(
        &[(PY_HELPERS, &py_helper(3)), ("tests/test_api.py", other)],
        &[(PY_HELPERS, &py_helper(1))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );

    let fixture = |n: usize| {
        let body: String = (0..n)
            .map(|i| format!("    assert r.f{i} == {i}\n"))
            .collect();
        format!("import pytest\n\n\n@pytest.fixture\ndef client():\n    r = connect()\n{body}    return r\n")
    };
    let run = change(
        &[("tests/conftest.py", &fixture(3))],
        &[("tests/conftest.py", &fixture(1))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(WEAKENED, "tests/conftest.py"),
        "{}",
        run.stdout
    );
}

fn py_test(body: &str) -> String {
    format!("from helpers import check\n\n\ndef test_create():\n    r = create()\n{body}")
}

/// Three assertions replaced by one call to a helper in a file the change does not
/// touch, which holds three: nothing is lost. The call stood for nothing because the
/// helper's file was outside the change.
#[test]
fn a_helper_in_an_unchanged_file_stands_for_the_checks_it_holds() {
    let inline = py_test("    assert r.f0 == 0\n    assert r.f1 == 1\n    assert r.f2 == 2\n");
    let calling = py_test("    check(r)\n");
    let run = change(
        &[(PY_HELPERS, &py_helper(3)), ("tests/test_api.py", &inline)],
        &[("tests/test_api.py", &calling)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );
    assert!(
        notes(&run).contains("read as moved into helper `check` (3 check(s))"),
        "{}",
        run.stdout
    );

    // Control: the unchanged helper holds one check, so two are lost.
    let run = change(
        &[(PY_HELPERS, &py_helper(1)), ("tests/test_api.py", &inline)],
        &[("tests/test_api.py", &calling)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, "tests/test_api.py"),
        "{}",
        run.stdout
    );

    // Control: the base test already called it, so the call adds nothing.
    let both = py_test("    check(r)\n    assert r.f3 == 3\n");
    let run = change(
        &[(PY_HELPERS, &py_helper(3)), ("tests/test_api.py", &both)],
        &[("tests/test_api.py", &calling)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, "tests/test_api.py"),
        "{}",
        run.stdout
    );

    // Control: the file is not under a test-support path.
    let run = change(
        &[
            ("app/helpers.py", &py_helper(3)),
            ("tests/test_api.py", &inline),
        ],
        &[("tests/test_api.py", &calling)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, "tests/test_api.py"),
        "{}",
        run.stdout
    );
}

const RS_HELPERS: &str = "tests/common/mod.rs";

fn rs_helper(condition: &str) -> String {
    format!("pub fn check(r: &R) {{\n    if {condition} {{\n        panic!(\"f1\");\n    }}\n}}\n")
}

fn rs_test(body: &str) -> String {
    format!("mod common;\n\n#[test]\nfn create() {{\n    let r = &make();\n{body}}}\n")
}

/// `assert_eq!(r.f1, 1)` replaced by a call to a helper of another file that panics
/// under `r.f1 != 1` is still an equality check, whether the helper's file is new in
/// the change or untouched by it. Under any other condition the strength is lost.
#[test]
fn an_equality_exit_in_a_helper_of_another_file_covers_a_dropped_equality_check() {
    let inline = rs_test("    assert_eq!(r.f1, 1);\n");
    let calling = rs_test("    common::check(r);\n");
    let file = "tests/api.rs";

    let run = change(
        &[(file, &inline)],
        &[(RS_HELPERS, &rs_helper("r.f1 != 1")), (file, &calling)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "new helper file: {}",
        run.stdout
    );
    assert!(
        notes(&run)
            .contains("read as moved into helpers that fail on an equality comparison (1 exit(s))"),
        "{}",
        run.stdout
    );

    let run = change(
        &[(RS_HELPERS, &rs_helper("r.f1 != 1")), (file, &inline)],
        &[(file, &calling)],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "unchanged helper file: {}",
        run.stdout
    );

    for condition in ["r.f1 < 1", "r.bad"] {
        let run = change(
            &[(file, &inline)],
            &[(RS_HELPERS, &rs_helper(condition)), (file, &calling)],
        );
        assert_eq!(
            reported(&run, "assertion-reduction"),
            one(DECREASED, file),
            "{condition}: {}",
            run.stdout
        );
        assert!(
            run.stdout.contains("weakened to a looser form"),
            "{}",
            run.stdout
        );
    }
}

const GOCHECK_HEAD: &str = "package x\n\nimport (\n\t\"testing\"\n\n\t. \"gopkg.in/check.v1\"\n)\n\ntype S struct{}\n\nvar _ = Suite(&S{})\n\nfunc Test(t *testing.T) { TestingT(t) }\n\n";

/// A gocheck suite method is a test: dropping its `c.Assert` / `c.Check` calls is
/// reported, a new one that checks nothing is vacuous, and `Equals` replaced by
/// `NotNil` is a loss of strength.
#[test]
fn gocheck_suite_methods_are_tests_with_assertions() {
    let file = "calc_test.go";
    let suite = |body: &str, more: &str| {
        format!("{GOCHECK_HEAD}func (s *S) TestAdd(c *C) {{\n{body}}}\n{more}")
    };
    let full = "\tc.Assert(Add(1, 1), Equals, 2)\n\tc.Check(Add(2, 2), Equals, 4)\n\tc.Assert(Open(), IsNil)\n";
    let run = change(
        &[(file, &suite(full, ""))],
        &[(file, &suite("\tc.Assert(Open(), IsNil)\n", ""))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, file),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("S.TestAdd"), "{}", run.stdout);

    let weaker = "\tc.Assert(Add(1, 1), NotNil)\n\tc.Check(Add(2, 2), Equals, 4)\n\tc.Assert(Open(), IsNil)\n";
    let run = change(&[(file, &suite(full, ""))], &[(file, &suite(weaker, ""))]);
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, file),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("weakened to a looser form"),
        "{}",
        run.stdout
    );

    let vacuous = "\nfunc (s *S) TestNew(c *C) {\n\tAdd(1, 1)\n}\n";
    let run = change(
        &[(file, &suite(full, ""))],
        &[(file, &suite(full, vacuous))],
    );
    assert_eq!(
        reported(&run, "vacuous-tests"),
        one(VACUOUS, file),
        "{}",
        run.stdout
    );

    // Control: a new suite method that asserts on `c` is not vacuous, and an unchanged
    // count is not reported.
    let checking = "\nfunc (s *S) TestNew(c *C) {\n\tc.Assert(Add(1, 1), Equals, 2)\n}\n";
    let run = change(
        &[(file, &suite(full, ""))],
        &[(file, &suite(full, checking))],
    );
    assert_eq!(reported(&run, "vacuous-tests"), vec![], "{}", run.stdout);
    assert_eq!(
        reported(&run, "assertion-reduction"),
        vec![],
        "{}",
        run.stdout
    );
}

/// `r := s.Require()` followed by `r.NoError(..)` is an assertion, as are
/// `require.New(t)` and `assert.New(t)` objects: dropping a call on one is reported, and
/// a test that asserts only through one is not vacuous.
#[test]
fn a_local_bound_to_a_testify_assertion_object_carries_assertions() {
    let file = "calc_test.go";
    let suite = |body: &str| {
        format!("package x\n\ntype Suite struct {{\n\tsuite.Suite\n}}\n\nfunc (s *Suite) TestAdd() {{\n\tr := s.Require()\n{body}}}\n")
    };
    let run = change(
        &[(
            file,
            &suite("\tr.NoError(open())\n\tr.Equal(2, Add(1, 1))\n"),
        )],
        &[(file, &suite("\tr.NoError(open())\n"))],
    );
    assert_eq!(
        reported(&run, "assertion-reduction"),
        one(DECREASED, file),
        "{}",
        run.stdout
    );

    let plain = |bind: &str, body: &str| {
        format!("package x\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {{\n\t{bind}\n{body}}}\n")
    };
    for bind in [
        "is := require.New(t)",
        "is := assert.New(t)",
        "var is = assert.New(t)",
    ] {
        let run = change(
            &[(
                file,
                &plain(bind, "\tis.NoError(open())\n\tis.Equal(2, Add(1, 1))\n"),
            )],
            &[(file, &plain(bind, "\tis.NoError(open())\n"))],
        );
        assert_eq!(
            reported(&run, "assertion-reduction"),
            one(DECREASED, file),
            "{bind}: {}",
            run.stdout
        );
    }

    let new_test = |body: &str| {
        format!("package x\n\nimport \"testing\"\n\nfunc TestOld(t *testing.T) {{\n\tt.Error(\"x\")\n}}\n\nfunc TestNew(t *testing.T) {{\n\tis := require.New(t)\n{body}}}\n")
    };
    let base =
        "package x\n\nimport \"testing\"\n\nfunc TestOld(t *testing.T) {\n\tt.Error(\"x\")\n}\n";
    let run = change(
        &[(file, base)],
        &[(file, &new_test("\tis.Equal(2, Add(1, 1))\n"))],
    );
    assert_eq!(reported(&run, "vacuous-tests"), vec![], "{}", run.stdout);
    // Control: building the object checks nothing, and a method on another local is no
    // assertion.
    let run = change(&[(file, base)], &[(file, &new_test("\tAdd(1, 1)\n"))]);
    assert_eq!(
        reported(&run, "vacuous-tests"),
        one(VACUOUS, file),
        "{}",
        run.stdout
    );
    let other = "package x\n\nimport \"testing\"\n\nfunc TestOld(t *testing.T) {\n\tt.Error(\"x\")\n}\n\nfunc TestNew(t *testing.T) {\n\tis := build(t)\n\tis.Equal(2, Add(1, 1))\n}\n";
    let run = change(&[(file, base)], &[(file, other)]);
    assert_eq!(
        reported(&run, "vacuous-tests"),
        one(VACUOUS, file),
        "{}",
        run.stdout
    );
}
