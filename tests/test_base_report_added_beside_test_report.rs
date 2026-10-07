//! `base_report` added beside an existing `test_report` is a change of evidence (#651).
//!
//! With `test_report` alone, `test-floor` compares the head report with the same file as
//! the base ref committed it. `base_report` names a file the runner supplies for the base
//! side instead. Adding it where `test_report` already stood moves the base side of the
//! identity comparison from the committed file to a runner file, and `config-integrity`
//! reports it as it reports `test_report` added, lifted by the same directive.
//!
//! Controls: `base_report` arriving together with the first report is judged with that
//! report, and `base_report` added beside `head_report` alone, where nothing was compared
//! before, adds a check.

mod common;
use common::{Repo, Run};

const WEAKENED: &str = "Gate Weakened By This Change";
const HEAD: &str = "[meta]\nversion = 1\nname = \"t\"\n\n[gates.test-floor]\nenabled = false\n";

fn run_change(base_keys: &str, head_keys: &str, body: Option<&str>) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(
        &[("discipline.toml", &format!("{HEAD}{base_keys}"))],
        "ci: base configuration",
    );
    repo.write("discipline.toml", &format!("{HEAD}{head_keys}"));
    repo.commit("chore: change the configuration");
    repo.check_with_pr_metadata(&[], Some("chore: change the configuration"), body)
}

fn weakenings(run: &Run) -> Vec<String> {
    run.violations("config-integrity")
        .iter()
        .map(|v| {
            assert_eq!(v["title"], WEAKENED, "{v}");
            v["message"].as_str().unwrap().to_string()
        })
        .collect()
}

const TEST_REPORT: &str = "test_report = \"reports/junit.xml\"\n";
const BASE_REPORT: &str = "base_report = \"runner/base.xml\"\n";

#[test]
fn base_report_added_beside_test_report_is_reported_and_lifted_by_the_directive() {
    let both = format!("{TEST_REPORT}{BASE_REPORT}");
    let run = run_change(TEST_REPORT, &both, None);
    let found = weakenings(&run);
    assert!(
        found.len() == 1
            && found[0].starts_with(
                "[test-floor] `base_report` changed from unset to \"runner/base.xml\""
            )
            && found[0].contains("as the base ref committed it"),
        "{found:?}\n{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.code, 1);

    let lifted = "allow-gate-weakening: test-floor the base report now comes from the base job";
    let run = run_change(TEST_REPORT, &both, Some(lifted));
    assert_eq!(weakenings(&run), Vec::<String>::new());
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.outcome("config-integrity")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn base_report_arriving_with_the_first_report_is_judged_with_that_report() {
    let run = run_change("", &format!("{TEST_REPORT}{BASE_REPORT}"), None);
    let found = weakenings(&run);
    assert!(
        found.len() == 1 && found[0].starts_with("[test-floor] `test_report` changed from unset"),
        "{found:?}"
    );
}

#[test]
fn base_report_added_beside_head_report_alone_adds_a_check() {
    let head_report = "head_report = \"runner/head.xml\"\n";
    let run = run_change(head_report, &format!("{head_report}{BASE_REPORT}"), None);
    assert_eq!(weakenings(&run), Vec::<String>::new());
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

#[test]
fn base_report_unchanged_beside_test_report_is_not_reported() {
    let both = format!("{TEST_REPORT}{BASE_REPORT}");
    let run = run_change(&both, &both, None);
    assert_eq!(weakenings(&run), Vec::<String>::new());
}
