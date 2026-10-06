//! A `command` key that decides what evidence the gate accepts, added where the base left
//! it to a default, is judged by the value that default stood for.
//!
//! Each case commits a configuration on `main`, changes it on `work`, and runs the real
//! binary against `main`. Every command a case runs is declared on the base side and
//! prints a fixed line, so no preset's own tool is started.

mod common;
use common::{Repo, Run};

const WEAKENED: &str = "Gate Weakened By This Change";

/// `cargo-mutants` with a base-side command that prints the preset's "nothing ran" line.
const MUTANTS_NOTHING_RAN: &str =
    "[gates.command]\npreset = \"cargo-mutants\"\ncommand = \"echo 0 mutants tested\"\n";
/// `cargo-mutants` with a base-side command that prints a line the preset forbids.
const MUTANTS_ONE_SURVIVED: &str =
    "[gates.command]\npreset = \"cargo-mutants\"\ncommand = \"echo 3 mutants tested, 1 survived\"\n";
/// `sanitizers` with a base-side canary that fails without the preset's diagnostic.
const SANITIZERS_QUIET_CANARY: &str =
    "[gates.command]\npreset = \"sanitizers\"\ncommand = \"true\"\n\
     canary_command = \"sh -c 'echo harmless; exit 1'\"\n";
/// `cargo-public-api` with a base-side command printing one line of API.
const PUBLIC_API: &str =
    "[gates.command]\npreset = \"cargo-public-api\"\ncommand = \"echo pub fn a()\"\n";
/// The same preset on a `commands` entry.
const PUBLIC_API_ENTRY: &str = "[gates.command]\n\n[[gates.command.commands]]\nname = \"api\"\n\
     preset = \"cargo-public-api\"\ncommand = \"echo pub fn a()\"\n";
/// A command with no preset: every key falls back to the built-in default.
const NO_PRESET: &str = "[gates.command]\ncommand = \"echo 4 passed\"\n";

fn config(gates: &str) -> String {
    format!("[meta]\nversion = 1\nname = \"t\"\n\n{gates}")
}

/// `base` committed on `main` with `base_files`, then `head` and `head_files` on `work`.
fn repo_with_files(
    base: &str,
    base_files: &[(&str, &str)],
    head: &str,
    head_files: &[(&str, &str)],
) -> Repo {
    let repo = Repo::new();
    let cfg = config(base);
    let mut files = vec![("discipline.toml", cfg.as_str())];
    files.extend_from_slice(base_files);
    repo.commit_base_files(&files, "ci: base configuration");
    repo.write("discipline.toml", &config(head));
    for (rel, content) in head_files {
        repo.write(rel, content);
    }
    repo.commit("chore: change the configuration");
    repo
}

fn repo_with(base: &str, head: &str) -> Repo {
    repo_with_files(base, &[], head, &[])
}

/// The messages of the weakenings `config-integrity` reported.
fn weakenings(run: &Run) -> Vec<String> {
    run.violations("config-integrity")
        .iter()
        .map(|v| {
            assert_eq!(v["title"], WEAKENED, "{v}");
            v["message"].as_str().unwrap().to_string()
        })
        .collect()
}

fn codes(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

/// Whether exactly one weakening was reported and it names `key` of the `command` gate.
fn only_weakening_names(found: &[String], key: &str) -> bool {
    found.len() == 1 && found[0].starts_with(&format!("[command] `{key}` "))
}

// ---- keys that replace a preset's default --------------------------------------

/// The preset's `0 mutants tested` guard is replaced by a pattern the tool never prints,
/// so a run that tested nothing passes.
#[test]
fn a_zero_items_pattern_added_over_a_preset_default_is_reported() {
    let repo = repo_with(
        MUTANTS_NOTHING_RAN,
        &format!("{MUTANTS_NOTHING_RAN}zero_items_pattern = \"never printed\"\n"),
    );
    let run = repo.check(&[]);
    let found = weakenings(&run);
    // The loosening is real: the head-side gate no longer sees the empty run.
    assert_eq!(codes(&run, "command"), Vec::<String>::new());
    assert!(
        found[..].iter().any(|m| m.contains("\"0 mutants tested\"")),
        "the report names the preset's default: {found:?}"
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(
        only_weakening_names(&found, "zero_items_pattern"),
        "{found:?}\n{}{}",
        run.stdout,
        run.stderr
    );
}

/// The preset's `ThreadSanitizer: data race` diagnostic is replaced by one the canary
/// prints anyway.
#[test]
fn a_canary_diagnostic_added_over_a_preset_default_is_reported() {
    let repo = repo_with(
        SANITIZERS_QUIET_CANARY,
        &format!("{SANITIZERS_QUIET_CANARY}canary_expected_diagnostic = \"harmless\"\n"),
    );
    let run = repo.check(&[]);
    let found = weakenings(&run);
    assert_eq!(codes(&run, "command"), Vec::<String>::new());
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(
        only_weakening_names(&found, "canary_expected_diagnostic"),
        "{found:?}\n{}{}",
        run.stdout,
        run.stderr
    );
}

/// The preset compares with `public-api.txt`; the change points the comparison at a file
/// it wrote itself.
#[test]
fn a_snapshot_added_over_a_preset_default_is_reported() {
    let repo = repo_with_files(
        PUBLIC_API,
        &[("public-api.txt", "pub fn a()\n")],
        &format!("{PUBLIC_API}snapshot = \"docs/api2.txt\"\n"),
        &[("docs/api2.txt", "pub fn a()\n")],
    );
    let run = repo.check(&[]);
    let found = weakenings(&run);
    assert_eq!(codes(&run, "command"), Vec::<String>::new());
    assert!(
        only_weakening_names(&found, "snapshot"),
        "{found:?}\n{}{}",
        run.stdout,
        run.stderr
    );
}

/// The same on a `commands` entry: an entry is judged as a whole, so the report is one
/// lost entry.
#[test]
fn a_snapshot_added_to_an_entry_over_its_preset_default_is_reported() {
    let repo = repo_with_files(
        PUBLIC_API_ENTRY,
        &[("public-api.txt", "pub fn a()\n")],
        &format!("{PUBLIC_API_ENTRY}snapshot = \"docs/api2.txt\"\n"),
        &[("docs/api2.txt", "pub fn a()\n")],
    );
    let run = repo.check(&[]);
    let found = weakenings(&run);
    assert_eq!(codes(&run, "command"), Vec::<String>::new());
    assert_eq!(
        found,
        vec!["[command] `commands` lost 1 entr(y/ies).".to_string()],
        "{}{}",
        run.stdout,
        run.stderr
    );
}

// ---- controls ------------------------------------------------------------------

/// Control: writing down the value the preset already supplies changes nothing.
#[test]
fn a_key_set_to_its_preset_default_is_not_a_weakening() {
    let zero = repo_with(
        MUTANTS_NOTHING_RAN,
        &format!("{MUTANTS_NOTHING_RAN}zero_items_pattern = \"0 mutants tested\"\n"),
    );
    let run = zero.check(&[]);
    assert_eq!(weakenings(&run), Vec::<String>::new());
    // The guard is still the preset's: the empty run is still reported.
    assert_eq!(codes(&run, "command"), vec!["command/zero-items-executed"]);

    let canary = repo_with(
        SANITIZERS_QUIET_CANARY,
        &format!(
            "{SANITIZERS_QUIET_CANARY}canary_expected_diagnostic = \"ThreadSanitizer: data race\"\n"
        ),
    );
    let run = canary.check(&[]);
    assert_eq!(weakenings(&run), Vec::<String>::new());
    assert_eq!(
        codes(&run, "command"),
        vec!["command/canary-diagnostic-missing"]
    );

    let api = [("public-api.txt", "pub fn a()\n")];
    let snapshot = repo_with_files(
        PUBLIC_API,
        &api,
        &format!("{PUBLIC_API}snapshot = \"public-api.txt\"\n"),
        &[],
    );
    assert_eq!(weakenings(&snapshot.check(&[])), Vec::<String>::new());

    let entry = repo_with_files(
        PUBLIC_API_ENTRY,
        &api,
        &format!("{PUBLIC_API_ENTRY}snapshot = \"public-api.txt\"\n"),
        &[],
    );
    let run = entry.check(&[]);
    assert_eq!(weakenings(&run), Vec::<String>::new());
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// Control: keys that only add to what the preset checks are not reported. `forbid_output`
/// is added to the preset's list, and no preset supplies `count_pattern` or `min_count`.
#[test]
fn a_tightening_added_over_a_preset_is_not_a_weakening() {
    let repo = repo_with(
        MUTANTS_ONE_SURVIVED,
        &format!(
            "{MUTANTS_ONE_SURVIVED}forbid_output = [\"timeout\"]\n\
             count_pattern = '(\\d+) mutants tested'\nmin_count = 2\n"
        ),
    );
    let run = repo.check(&[]);
    assert_eq!(weakenings(&run), Vec::<String>::new());
    // The preset's own pattern is still in force beside the added one.
    assert_eq!(codes(&run, "command"), vec!["command/forbidden-output"]);
}

/// Control: a key both sides set is judged as before, changed or not.
#[test]
fn a_key_both_sides_set_is_judged_as_before() {
    let base = format!("{MUTANTS_NOTHING_RAN}zero_items_pattern = \"0 mutants\"\n");
    let unchanged = repo_with(&base, &format!("{base}timeout_seconds = 30\n"));
    assert_eq!(weakenings(&unchanged.check(&[])), Vec::<String>::new());

    let changed = repo_with(
        &base,
        &format!("{MUTANTS_NOTHING_RAN}zero_items_pattern = \"never printed\"\n"),
    );
    let found = weakenings(&changed.check(&[]));
    assert_eq!(
        found,
        vec![
            "[command] `zero_items_pattern` changed from \"0 mutants\" to \"never printed\"."
                .to_string()
        ]
    );
}

/// The existing directive lifts the new report as it lifts every other weakening of the
/// gate.
#[test]
fn the_directive_lifts_a_key_added_over_a_preset_default() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        &config(MUTANTS_NOTHING_RAN),
        "ci: base configuration",
    );
    repo.write(
        "discipline.toml",
        &config(&format!(
            "{MUTANTS_NOTHING_RAN}zero_items_pattern = \"never printed\"\n"
        )),
    );
    repo.commit(
        "chore: change the guard\n\nallow-gate-weakening: command the tool reports an empty run another way",
    );
    let run = repo.check(&[]);
    let outcome = run.outcome("config-integrity");
    assert_eq!(weakenings(&run), Vec::<String>::new());
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        outcome["overrides"].as_array().unwrap().len(),
        1,
        "{outcome}"
    );
}

/// Under `--policy-from base` the base copy is in force, so the preset's guard still
/// fires, and the edit is still reported for review.
#[test]
fn policy_from_base_keeps_the_preset_default_and_still_reports_the_edit() {
    let repo = repo_with(
        MUTANTS_NOTHING_RAN,
        &format!("{MUTANTS_NOTHING_RAN}zero_items_pattern = \"never printed\"\n"),
    );
    let run = repo.check(&["--policy-from", "base"]);
    let found = weakenings(&run);
    assert_eq!(codes(&run, "command"), vec!["command/zero-items-executed"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(
        only_weakening_names(&found, "zero_items_pattern"),
        "{found:?}\n{}{}",
        run.stdout,
        run.stderr
    );
}

// ---- shapes that were already judged, pinned ------------------------------------

/// An emptied or shorter `forbid_output` over a preset drops nothing: the preset's
/// patterns are added to the configured list, never replaced by it.
#[test]
fn a_shorter_forbid_output_does_not_drop_the_preset_patterns() {
    for list in ["[]", "[\"MISSED\"]"] {
        let repo = repo_with(
            MUTANTS_ONE_SURVIVED,
            &format!("{MUTANTS_ONE_SURVIVED}forbid_output = {list}\n"),
        );
        let run = repo.check(&[]);
        assert_eq!(weakenings(&run), Vec::<String>::new(), "{list}");
        assert_eq!(
            codes(&run, "command"),
            vec!["command/forbidden-output"],
            "{list}"
        );
    }
    // A configured pattern the change drops is a lost entry, as before.
    let base = format!("{MUTANTS_ONE_SURVIVED}forbid_output = [\"timeout\", \"panicked\"]\n");
    let repo = repo_with(
        &base,
        &format!("{MUTANTS_ONE_SURVIVED}forbid_output = [\"timeout\"]\n"),
    );
    assert_eq!(
        weakenings(&repo.check(&[])),
        vec!["[command] `forbid_output` lost 1 entr(y/ies).".to_string()]
    );
}

/// `allow_zero` and `snapshot_ignore` have a default on both sides, so adding them over a
/// preset was already compared with it.
#[test]
fn allow_zero_and_snapshot_ignore_added_over_a_preset_are_reported() {
    let zero = repo_with(
        MUTANTS_NOTHING_RAN,
        &format!("{MUTANTS_NOTHING_RAN}allow_zero = true\n"),
    );
    assert_eq!(
        weakenings(&zero.check(&[])),
        vec!["[command] `allow_zero` changed from false to true.".to_string()]
    );

    let api = [("public-api.txt", "pub fn a()\n")];
    let ignore = repo_with_files(
        PUBLIC_API,
        &api,
        &format!("{PUBLIC_API}snapshot_ignore = [\"^//\"]\n"),
        &[],
    );
    assert_eq!(
        weakenings(&ignore.check(&[])),
        vec!["[command] `snapshot_ignore` gained 1 entr(y/ies).".to_string()]
    );

    // A pattern that ignores every line leaves nothing to compare: the run cannot check.
    let all = repo_with_files(
        PUBLIC_API,
        &api,
        &format!("{PUBLIC_API}snapshot_ignore = [\".\"]\n"),
        &[],
    );
    let run = all.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check().1.as_deref(), Some("command"));
}

/// With no preset the built-in default of each of these keys is "no such check", so
/// adding one adds a check; the two keys that relax the gate are reported.
#[test]
fn keys_added_over_the_builtin_default_are_judged_by_what_unset_means() {
    let canary = format!("{NO_PRESET}canary_command = \"sh -c 'echo harmless; exit 1'\"\n");
    let adds_a_check = [
        (
            NO_PRESET.to_string(),
            "zero_items_pattern = \"never printed\"\n",
        ),
        (NO_PRESET.to_string(), "count_pattern = '(\\d+) passed'\n"),
        (
            NO_PRESET.to_string(),
            "count_pattern = '(\\d+) passed'\nmin_count = 1\n",
        ),
        (canary, "canary_expected_diagnostic = \"harmless\"\n"),
    ];
    for (base, added) in &adds_a_check {
        let run = repo_with(base, &format!("{base}{added}")).check(&[]);
        assert_eq!(weakenings(&run), Vec::<String>::new(), "{added}");
        assert_eq!(run.code, 0, "{added}\n{}{}", run.stdout, run.stderr);
    }

    let snapshot = repo_with_files(
        NO_PRESET,
        &[],
        &format!("{NO_PRESET}snapshot = \"out.txt\"\n"),
        &[("out.txt", "4 passed\n")],
    );
    assert_eq!(weakenings(&snapshot.check(&[])), Vec::<String>::new());

    let zero = repo_with(NO_PRESET, &format!("{NO_PRESET}allow_zero = true\n"));
    assert_eq!(
        weakenings(&zero.check(&[])),
        vec!["[command] `allow_zero` changed from false to true.".to_string()]
    );
    let ignore = repo_with_files(
        NO_PRESET,
        &[],
        &format!("{NO_PRESET}snapshot = \"out.txt\"\nsnapshot_ignore = [\"^//\"]\n"),
        &[("out.txt", "4 passed\n")],
    );
    assert_eq!(
        weakenings(&ignore.check(&[])),
        vec!["[command] `snapshot_ignore` gained 1 entr(y/ies).".to_string()]
    );
}

/// A `commands` entry is judged as a whole: a guard or a diagnostic added to one over its
/// preset's default was already one lost entry.
#[test]
fn a_guard_added_to_an_entry_over_its_preset_default_is_a_lost_entry() {
    let mutants = "[gates.command]\n\n[[gates.command.commands]]\nname = \"mutants\"\n\
                   preset = \"cargo-mutants\"\ncommand = \"echo 0 mutants tested\"\n";
    let sanitizers = "[gates.command]\n\n[[gates.command.commands]]\nname = \"san\"\n\
                      preset = \"sanitizers\"\ncommand = \"true\"\n\
                      canary_command = \"sh -c 'echo harmless; exit 1'\"\n";
    let lost = vec!["[command] `commands` lost 1 entr(y/ies).".to_string()];
    let zero = repo_with(
        mutants,
        &format!("{mutants}zero_items_pattern = \"never printed\"\n"),
    );
    assert_eq!(weakenings(&zero.check(&[])), lost);
    let diag = repo_with(
        sanitizers,
        &format!("{sanitizers}canary_expected_diagnostic = \"harmless\"\n"),
    );
    assert_eq!(weakenings(&diag.check(&[])), lost);
}
