//! A test's assertion drop is excused as "moved into helper" only for the checks the helper
//! can account for (#531). Each case drives the real binary over a Python test file and a
//! shared helper file that is part of the same change.

mod common;
use common::{Repo, Run};

const HELPERS: &str = "tests/helpers.py";
const TESTS: &str = "tests/test_api.py";
const DECREASED: &str = "Assertion Count Decreased In Existing Test";

const THREE_INLINE: &str = "    assert resp.status == 200\n    assert resp.body[\"id\"] == 1\n    assert resp.body[\"name\"] == \"a\"\n";

fn test_file(import: &str, body: &str) -> String {
    format!("from helpers import {import}\n\ndef test_create():\n    resp = create()\n{body}")
}

/// Puts `helper` and `test` on the base side, then commits `head_helper` and `head_test`
/// as the change and runs `check`.
fn run_change(helper: &str, test: &str, head_helper: &str, head_test: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(
        &[(HELPERS, helper), (TESTS, test)],
        "test: api tests with a shared helper",
    );
    repo.write(HELPERS, head_helper);
    repo.write(TESTS, head_test);
    repo.commit("refactor: move checks into the shared helper");
    repo.check(&[])
}

fn assert_reported(run: &Run, from: usize, to: usize) {
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.titles("assertion-reduction"),
        vec![DECREASED.to_string()],
        "{}",
        run.stdout
    );
    let message = run.violations("assertion-reduction")[0]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        message,
        format!("Test `test_create`: effective assertions dropped from {from} to {to}.")
    );
}

fn assert_excused(run: &Run, helper: &str) {
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(
        run.titles("assertion-reduction").is_empty(),
        "{}",
        run.stdout
    );
    let notes = run.outcome("assertion-reduction")["notes"].to_string();
    assert!(
        notes.contains(&format!("read as moved into helper `{helper}`")),
        "{notes}"
    );
}

/// The reproduction of #531: three inline assertions replaced by one call to a helper that
/// holds one check. The helper accounts for one; the other two are a drop.
#[test]
fn three_assertions_replaced_by_a_call_to_a_one_check_helper_is_reported() {
    let helper = "def check_status(resp):\n    assert resp.status == 200\n";
    let run = run_change(
        helper,
        &test_file("check_status", THREE_INLINE),
        &format!("# shared checks\n{helper}"),
        &test_file("check_status", "    check_status(resp)\n"),
    );
    assert_reported(&run, 3, 1);
}

/// Control: the helper already holds the three checks the test drops, so the move is whole.
#[test]
fn three_assertions_replaced_by_a_call_to_a_three_check_helper_is_excused() {
    let helper = format!("def check_resp(resp):\n{THREE_INLINE}");
    let run = run_change(
        &helper,
        &test_file("check_resp", THREE_INLINE),
        &format!("# shared checks\n{helper}"),
        &test_file("check_resp", "    check_resp(resp)\n"),
    );
    assert_excused(&run, "check_resp");
}

/// The base test already called the helper: only the checks the helper gained in this
/// change can stand for the test's dropped assertions. One gained, three dropped.
#[test]
fn already_called_helper_gaining_one_check_does_not_excuse_three_dropped() {
    let helper = "def check_status(resp):\n    assert resp.status == 200\n";
    let body = format!("    check_status(resp)\n{THREE_INLINE}");
    let run = run_change(
        helper,
        &test_file("check_status", &body),
        &format!("{helper}    assert resp.body[\"id\"] == 1\n"),
        &test_file("check_status", "    check_status(resp)\n"),
    );
    assert_reported(&run, 3, 1);
}

/// Control: the already-called helper gains the three checks the test drops.
#[test]
fn already_called_helper_gaining_three_checks_excuses_three_dropped() {
    let helper = "def check_status(resp):\n    assert resp.ok == True\n";
    let body = format!("    check_status(resp)\n{THREE_INLINE}");
    let run = run_change(
        helper,
        &test_file("check_status", &body),
        &format!("{helper}{THREE_INLINE}"),
        &test_file("check_status", "    check_status(resp)\n"),
    );
    assert_excused(&run, "check_status");
}

/// The helper is matched by the exact name of the call: `precheck` gaining checks says
/// nothing about a test that calls `check`. `precheck` is defined first, so a suffix match
/// would resolve the call to it.
#[test]
fn checks_gained_by_precheck_do_not_excuse_a_test_calling_check() {
    let check = "def check(resp):\n    assert resp.ok == True\n";
    let precheck = "def precheck(resp):\n    assert resp.ready == True\n";
    let body = format!("    check(resp)\n{THREE_INLINE}");
    let run = run_change(
        &format!("{precheck}\n{check}"),
        &test_file("check", &body),
        &format!("{precheck}{THREE_INLINE}\n{check}"),
        &test_file("check", "    check(resp)\n"),
    );
    assert_reported(&run, 3, 0);
}
