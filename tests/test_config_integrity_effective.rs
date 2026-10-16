//! `config-integrity` compares the enabled state each side's configuration file gives a
//! gate, defaults applied, independent of the command line: a gate switched off by
//! `--disable` alone is the operator's flag, but a change that deletes the gate's
//! `enabled = true` table (so the gate falls back to its default, off) weakens the file,
//! and `--disable` in CI must not hide that.

mod common;
use common::{Repo, CONFIG_HEAD};

const CODE: &str = "config-integrity/gate-weakened";

/// A repository whose base configuration enables `unsafe-budget` (off by default).
fn repo() -> Repo {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.unsafe-budget]\nenabled = true\n"),
    );
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    repo
}

fn weakened(run: &common::Run) -> Vec<String> {
    run.violations("config-integrity")
        .iter()
        .filter(|v| v["code"].as_str() == Some(CODE))
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn deleting_an_enabled_table_is_a_weakening_under_disable() {
    let r = repo();
    r.write("discipline.toml", CONFIG_HEAD);
    r.commit("chore: drop the unsafe budget table");
    let run = r.check(&["--disable", "unsafe-budget"]);
    let got = weakened(&run);
    assert!(
        got.iter().any(|m| m.contains("[unsafe-budget]")),
        "the file now leaves the gate off: {got:?}\n{}",
        run.stdout
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
}

#[test]
fn a_literal_enabled_false_is_a_weakening_under_disable() {
    let r = repo();
    r.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.unsafe-budget]\nenabled = false\n"),
    );
    r.commit("chore: switch the unsafe budget off");
    let got = weakened(&r.check(&["--disable", "unsafe-budget"]));
    assert!(got.iter().any(|m| m.contains("[unsafe-budget]")), "{got:?}");
}

#[test]
fn disable_alone_is_not_a_weakening_of_the_file() {
    let r = repo();
    r.write("src/lib.rs", "pub fn f() {}\n");
    r.commit("feat: add a module");
    let run = r.check(&["--disable", "unsafe-budget"]);
    assert!(weakened(&run).is_empty(), "{}", run.stdout);
}

/// What the documentation says about `--config-override`: it is compared like the file,
/// since the override (or `DISCIPLINE_CONFIG_OVERRIDE`) is part of the configuration in
/// force; only `--disable` alone is left out by default.
#[test]
fn a_config_override_that_switches_a_gate_off_is_compared() {
    let r = repo();
    r.write("src/lib.rs", "pub fn f() {}\n");
    r.commit("feat: add a module");
    let run = r.check(&[
        "--config-override",
        "[gates.unsafe-budget]\nenabled = false",
    ]);
    let got = weakened(&run);
    assert!(
        got.iter().any(|m| m.contains("[unsafe-budget]")),
        "{got:?}\n{}",
        run.stdout
    );
}
