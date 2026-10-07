//! Every glob an enabled gate is configured with is compiled before any gate runs, for
//! the configuration in force and for the change's own copy of it (#519).

mod common;
use common::Repo;

const HEAD: &str = common::CONFIG_HEAD;

fn detail(run: &common::Run) -> String {
    run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `[tests] paths` belongs to no gate: a glob that does not compile there used to match
/// nothing, so files declared as test scope were silently not treated as tests.
#[test]
fn an_invalid_tests_paths_glob_is_a_configuration_error() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[tests]\npaths = [\"checks/[a-z/**\"]\n"),
    );
    repo.commit("chore: declare test paths");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check(), ("configuration".to_string(), None));
    let d = detail(&run);
    assert!(
        d.contains("invalid glob") && d.contains("checks/[a-z/**") && d.contains("tests.paths"),
        "{d}"
    );
}

/// Under base policy the change's copy is not the configuration in force, so a glob that
/// does not compile in it used to pass and then stop every later run once merged.
#[test]
fn base_policy_refuses_a_change_that_adds_an_invalid_glob() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\"]\n"
        ),
    );
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/[a-z\"]\n"
        ),
    );
    repo.commit("chore: forbid keys");
    let run = repo.check(&["--policy-from", "base"]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("gate".to_string(), Some("scope-confinement".to_string()))
    );
    let d = detail(&run);
    assert!(d.contains("invalid glob") && d.contains("keys/[a-z"), "{d}");
}

/// Control: the same change with a glob that compiles is judged normally.
#[test]
fn base_policy_accepts_a_change_that_adds_a_valid_glob() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\"]\n"
        ),
    );
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "discipline.toml",
        &format!(
            "{HEAD}[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/{{a,b}}/[a-z]*\"]\n"
        ),
    );
    repo.commit("chore: forbid keys");
    let run = repo.check(&["--policy-from", "base"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// A gate that is off is not validated: its settings are not in force.
#[test]
fn a_disabled_gate_with_an_invalid_glob_does_not_stop_the_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.scope-confinement]\nenabled = false\nforbidden_paths = [\"[\"]\n"),
    );
    repo.commit("chore: policy");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// The three gates that compile their globs after an early return validated nothing on a
/// change with no files in their scope.
#[test]
fn an_invalid_glob_stops_the_run_even_when_the_gate_has_nothing_to_examine() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &format!("{HEAD}[gates.unsafe-budget]\nenabled = true\nexempt_paths = [\"[\"]\n"),
    );
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.git(&["commit", "-q", "--allow-empty", "-m", "chore: nothing"]);
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("gate".to_string(), Some("unsafe-budget".to_string()))
    );
}
