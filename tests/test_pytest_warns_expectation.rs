//! `pytest.warns` is an expected warning, of the kind `assertWarns` is (#626): its class
//! and its `match` pattern are compared across a change as those of `pytest.raises` are.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WIDENED: &str = "Expected Exception Or Panic Widened";
const FILE: &str = "tests/test_sut.py";

fn file(import: &str, body: &str) -> String {
    let body: String = body.lines().map(|l| format!("    {l}\n")).collect();
    format!("{import}\n\n\ndef test_warns():\n{body}    assert ready()\n")
}

fn change(import: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.write(FILE, &file(import, base));
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(FILE, &file(import, head));
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

/// The message of each expected-exception finding.
fn widened(run: &Run) -> Vec<String> {
    run.violations("assertion-reduction")
        .iter()
        .filter(|v| v["title"] == WIDENED)
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect()
}

fn reports(import: &str, base: &str, head: &str, what: &str) {
    let run = change(import, base, head);
    let got = widened(&run);
    assert_eq!(got.len(), 1, "{base} -> {head}: {}", run.stdout);
    assert!(got[0].contains(what), "{base} -> {head}: {}", got[0]);
}

fn silent(import: &str, base: &str, head: &str) {
    let run = change(import, base, head);
    assert_eq!(
        widened(&run),
        Vec::<String>::new(),
        "{base} -> {head}: {}",
        run.stdout
    );
}

#[test]
fn a_pytest_warns_matcher_or_class_given_up_is_reported() {
    let pytest = "import pytest";
    reports(
        pytest,
        "with pytest.warns(DeprecationWarning, match=\"old api\"):\n    sut.run()",
        "with pytest.warns(DeprecationWarning):\n    sut.run()",
        "matcher was removed",
    );
    reports(
        pytest,
        "with pytest.warns(DeprecationWarning):\n    sut.run()",
        "with pytest.warns(Warning):\n    sut.run()",
        "from `DeprecationWarning` to `Warning`",
    );
    reports(
        pytest,
        "with pytest.warns(expected_warning=DeprecationWarning):\n    sut.run()",
        "with pytest.warns():\n    sut.run()",
        "type was removed",
    );
    reports(
        "from pytest import warns",
        "with warns(UserWarning, match=\"slow\") as record:\n    sut.run()",
        "with warns(UserWarning) as record:\n    sut.run()",
        "matcher was removed",
    );
    // The call form.
    reports(
        pytest,
        "pytest.warns(DeprecationWarning, sut.run, 1)",
        "pytest.warns(Warning, sut.run, 1)",
        "from `DeprecationWarning` to `Warning`",
    );
}

#[test]
fn a_pytest_warns_kept_or_narrowed_and_a_project_warns_are_not_reported() {
    let pytest = "import pytest";
    let kept = "with pytest.warns(DeprecationWarning, match=\"old api\"):\n    sut.run()";
    silent(pytest, kept, &format!("{kept}\nsut.reset()"));
    silent(
        pytest,
        "with pytest.warns(Warning):\n    sut.run()",
        "with pytest.warns(DeprecationWarning, match=\"old\"):\n    sut.run()",
    );
    // A `warns` that is not pytest's.
    silent(
        "import harness",
        "with harness.warns(DeprecationWarning, match=\"old api\"):\n    sut.run()",
        "with harness.warns(DeprecationWarning):\n    sut.run()",
    );
    silent(
        "from harness import warns",
        "with warns(DeprecationWarning, match=\"old api\"):\n    sut.run()",
        "with warns(DeprecationWarning):\n    sut.run()",
    );
}
