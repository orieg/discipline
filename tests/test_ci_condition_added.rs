//! A CI condition added to a test that already carries a conditional skip is a newly
//! added CI-conditional skip (#533): the test stops running in CI. Each case drives the
//! real binary over a base and a head side that both carry a conditional skip.

mod common;
use common::{Repo, Run};

const SHORT: &str = "testing.Short()";
const SHORT_OR_CI: &str = "testing.Short() || os.Getenv(\"CI\") != \"\"";
const CI_SKIP_WARNING: &str =
    "[meta]\nversion = 1\nname = \"t\"\n[gates.ignored-tests]\nci_skip_severity = \"warning\"\n";

/// A Go test file whose one test skips under `cond` and fails with `message`.
fn go_test(cond: &str, message: &str) -> String {
    format!(
        "package p\n\nimport (\n\t\"os\"\n\t\"runtime\"\n\t\"testing\"\n)\n\nvar _ = os.Getenv\nvar _ = runtime.GOOS\n\nfunc TestA(t *testing.T) {{\n\tif {cond} {{\n\t\tt.Skip()\n\t}}\n\tif 1+1 != 2 {{\n\t\tt.Fatal(\"{message}\")\n\t}}\n}}\n"
    )
}

/// A repository whose base side holds the Go test skipping under `base`, with the head
/// side skipping under `head`.
fn go_repo(base: &str, head: &str, extra_base: &[(&str, &str)]) -> Repo {
    let repo = Repo::new();
    let base_src = go_test(base, "math");
    let mut files = vec![
        ("go.mod", "module example.com/p\n\ngo 1.22\n"),
        ("p_test.go", base_src.as_str()),
    ];
    files.extend_from_slice(extra_base);
    repo.commit_base_files(&files, "test: base suite");
    repo.write("p_test.go", &go_test(head, "math"));
    repo.commit("test: change the skip condition");
    repo
}

fn assert_ci_skip_reported(run: &Run, test: &str, severity: &str) {
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Conditionally Skipped"],
        "{}{}",
        run.stdout,
        run.stderr
    );
    let v = &run.violations("ignored-tests")[0];
    assert_eq!(v["severity"], severity, "{v}");
    assert!(v["message"].as_str().unwrap().contains(test), "{v}");
    assert!(
        v["remediation"]
            .as_str()
            .unwrap()
            .contains(&format!("allow-ignore: {test} <reason>")),
        "{v}"
    );
}

fn assert_no_finding(run: &Run) {
    assert_eq!(
        run.titles("ignored-tests"),
        Vec::<String>::new(),
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

#[test]
fn go_ci_condition_added_to_a_conditional_skip_is_an_error() {
    let repo = go_repo(SHORT, SHORT_OR_CI, &[]);
    let run = repo.check(&[]);
    assert_ci_skip_reported(&run, "TestA", "error");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

#[test]
fn python_ci_condition_added_to_a_conditional_skip_is_an_error() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[(
            "test_db.py",
            "import os\nimport pytest\n\nHAVE_DB = False\n\ndef test_query():\n    if not HAVE_DB:\n        pytest.skip()\n    assert 1 + 1 == 2\n",
        )],
        "test: base suite",
    );
    repo.write(
        "test_db.py",
        "import os\nimport pytest\n\nHAVE_DB = False\n\ndef test_query():\n    if not HAVE_DB or os.environ.get(\"CI\"):\n        pytest.skip()\n    assert 1 + 1 == 2\n",
    );
    repo.commit("test: change the skip condition");
    let run = repo.check(&[]);
    assert_ci_skip_reported(&run, "test_query", "error");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

/// Control: a CI condition present on both sides is not a finding when the test body
/// changes elsewhere.
#[test]
fn ci_condition_on_both_sides_is_not_reported_when_the_body_changes() {
    let repo = Repo::new();
    let base_src = go_test(SHORT_OR_CI, "math");
    repo.commit_base_files(
        &[
            ("go.mod", "module example.com/p\n\ngo 1.22\n"),
            ("p_test.go", base_src.as_str()),
        ],
        "test: base suite",
    );
    repo.write("p_test.go", &go_test(SHORT_OR_CI, "arithmetic"));
    repo.commit("test: reword the failure message");
    assert_no_finding(&repo.check(&[]));
}

/// Control: a condition that changes without gaining a CI variable is not reported.
#[test]
fn changed_condition_without_a_ci_variable_is_not_reported() {
    let repo = go_repo(SHORT, "testing.Short() || runtime.GOOS == \"windows\"", &[]);
    assert_no_finding(&repo.check(&[]));
}

/// Control: dropping the CI condition makes the test run in CI again.
#[test]
fn ci_condition_removed_is_not_reported() {
    let repo = go_repo(SHORT_OR_CI, SHORT, &[]);
    assert_no_finding(&repo.check(&[]));
}

#[test]
fn ci_skip_severity_sets_the_severity_of_an_added_ci_condition() {
    let repo = go_repo(SHORT, SHORT_OR_CI, &[("discipline.toml", CI_SKIP_WARNING)]);
    let run = repo.check(&[]);
    assert_ci_skip_reported(&run, "TestA", "warning");
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

#[test]
fn allow_ignore_in_the_pr_body_lifts_an_added_ci_condition() {
    let repo = go_repo(SHORT, SHORT_OR_CI, &[]);
    let run = repo.check_with_pr(
        &[],
        "Summary\n\nallow-ignore: TestA flaky on the shared runner, tracked in #101\n",
    );
    assert_no_finding(&run);
    assert_eq!(
        run.outcome("ignored-tests")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{}",
        run.stdout
    );
}
