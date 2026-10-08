//! A base-side configuration that exists but does not load with this binary must not
//! read as "there is no base configuration" (#515). Each test commits a base
//! `discipline.toml` with a key this binary rejects, then judges a change against it.

mod common;
use common::Repo;

const HEAD: &str = common::CONFIG_HEAD;
/// Parses as TOML, fails the schema: the shape an older or newer binary's key has.
const UNLOADABLE: &str =
    "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\na_key_this_binary_rejects = 1\n";

fn repo_with_unloadable_base() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", UNLOADABLE);
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

/// The change repairs the configuration and switches `config-integrity` off in the same
/// edit. The base cannot say the gate was off, so it still runs and reports that the
/// comparison could not be made.
#[test]
fn config_integrity_still_runs_when_the_change_disables_it() {
    let repo = repo_with_unloadable_base();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.config-integrity]\nenabled = false\n[gates.pii]\nenabled = false\n"),
    );
    repo.commit("chore: repair the configuration");
    let run = repo.check(&[]);
    assert_eq!(
        run.titles("config-integrity"),
        vec!["Base Configuration Unreadable".to_string()],
        "{}{}",
        run.stdout,
        run.stderr
    );
    // A warning: the change that repairs the base must be able to merge.
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// The change switches the run to advisory. The base cannot say it was advisory already,
/// so the switch is not honoured and a real finding still fails the run.
#[test]
fn advisory_mode_is_not_honoured_over_an_unloadable_base() {
    let repo = repo_with_unloadable_base();
    repo.write("discipline.toml", &format!("{HEAD}mode = \"advisory\"\n"));
    repo.write(
        "src/swallow.py",
        "import os\n\n\ndef remove(path):\n    try:\n        os.remove(path)\n    except OSError:\n        pass\n",
    );
    repo.commit("feat: remove helper");
    let run = repo.check(&[]);
    assert!(
        run.titles("error-swallowing")
            .contains(&"Empty Error Handler Added".to_string()),
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

/// Control for the test above: with no base configuration at all, adopting in advisory
/// mode is honoured.
#[test]
fn advisory_mode_is_honoured_when_the_base_has_no_configuration() {
    let repo = Repo::new();
    repo.write("discipline.toml", &format!("{HEAD}mode = \"advisory\"\n"));
    repo.write(
        "src/swallow.py",
        "import os\n\n\ndef remove(path):\n    try:\n        os.remove(path)\n    except OSError:\n        pass\n",
    );
    repo.commit("feat: remove helper");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

#[test]
fn test_floor_names_the_base_ratchet_it_could_not_read() {
    let repo = repo_with_unloadable_base();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.test-floor]\nenabled = true\n"),
    );
    repo.commit("chore: repair the configuration");
    let run = repo.check(&[]);
    let notes = run.outcome("test-floor")["notes"].to_string();
    assert!(
        notes.contains("the base-side configuration does not load") && notes.contains("min_tests"),
        "{notes}"
    );
}

#[test]
fn command_names_the_base_ratchet_it_could_not_read() {
    let repo = repo_with_unloadable_base();
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.command]\nenabled = true\ncommand = \"echo 10 passed\"\ncount_pattern = '(\\d+) passed'\n"
        ),
    );
    repo.commit("chore: repair the configuration");
    let run = repo.run(
        &["check", "--base", "main", "--format", "json"],
        &[("DISCIPLINE_COMMAND", "echo 10 passed")],
    );
    let notes = run.outcome("command")["notes"].to_string();
    assert!(
        notes.contains("the base-side configuration does not load") && notes.contains("min_count"),
        "{notes}\n{}",
        run.stderr
    );
}

#[test]
fn config_integrity_names_a_base_baseline_that_does_not_parse() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", HEAD);
    repo.write("discipline-baseline.toml", "this is = = not toml [\n");
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.remove("discipline-baseline.toml");
    repo.commit("chore: drop the broken baseline");
    let run = repo.check(&[]);
    let notes = run.outcome("config-integrity")["notes"].to_string();
    assert!(
        notes.contains("discipline-baseline.toml") && notes.contains("does not parse"),
        "{notes}\n{}",
        run.stderr
    );
}
