//! `test-budget` reads the fuzz targets of a fuzz manifest from its parsed TOML (#678):
//! the `name` of each `[[bin]]` entry, wherever the key stands in the entry and however
//! the entry is written. Text that only looks like an entry (a comment, the inside of a
//! string) is not a target, and a manifest that does not parse is not a manifest with no
//! targets.
//!
//! Each case drives the real binary on a change to `fuzz/Cargo.toml`.

mod common;
use common::{Repo, Run};

const PACKAGE: &str = "[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n\n";

/// A repository whose base holds `base` as `fuzz/Cargo.toml`, with `head` on the branch.
fn manifest_change(base: &str, head: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base(
        "fuzz/Cargo.toml",
        &format!("{PACKAGE}{base}"),
        "base: fuzz manifest",
    );
    repo.write("fuzz/Cargo.toml", &format!("{PACKAGE}{head}"));
    repo.commit("fuzz: edit the manifest");
    repo
}

/// The targets `test-budget` reports as removed from the harness list, in report order.
fn removed(run: &Run) -> Vec<String> {
    run.violations("test-budget")
        .iter()
        .filter(|v| v["code"] == "test-budget/fuzz-target-removed")
        .map(|v| {
            v["message"]
                .as_str()
                .unwrap()
                .split('`')
                .nth(1)
                .unwrap()
                .to_string()
        })
        .collect()
}

fn notes(run: &Run) -> Vec<String> {
    run.outcome("test-budget")["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

// ---- removals a pattern over the text does not see ----------------------------------

#[test]
fn a_removed_target_whose_name_is_not_the_first_key_is_reported() {
    let keep = "[[bin]]\npath = \"fuzz_targets/parse.rs\"\nname = \"parse\"\ntest = false\n";
    let drop = "\n[[bin]]\npath = \"fuzz_targets/eval.rs\"\nname = \"eval\"\ntest = false\n";
    let repo = manifest_change(&format!("{keep}{drop}"), keep);
    let run = repo.check(&[]);
    assert_eq!(removed(&run), ["eval"], "{}", run.stdout);
    assert_eq!(run.code, 1);
}

#[test]
fn a_removed_target_under_a_header_with_a_trailing_comment_is_reported() {
    let keep = "[[bin]]\nname = \"parse\"\n";
    let drop = "\n[[bin]] # the evaluator\nname = \"eval\"\n";
    let repo = manifest_change(&format!("{keep}{drop}"), keep);
    let run = repo.check(&[]);
    assert_eq!(removed(&run), ["eval"], "{}", run.stdout);
}

#[test]
fn a_removed_target_written_with_a_literal_string_is_reported() {
    let keep = "[[bin]]\nname = 'parse'\n";
    let drop = "\n[[bin]]\nname = 'eval'\n";
    let repo = manifest_change(&format!("{keep}{drop}"), keep);
    let run = repo.check(&[]);
    assert_eq!(removed(&run), ["eval"], "{}", run.stdout);
}

/// A top-level `bin = [..]` array is the same value as `[[bin]]` tables. It stands before
/// the first table header, so the fixture writes the whole file.
#[test]
fn a_removed_target_of_an_inline_array_is_reported() {
    let parse = "  { name = \"parse\", path = \"fuzz_targets/parse.rs\" },\n";
    let eval = "  { name = \"eval\", path = \"fuzz_targets/eval.rs\" },\n";
    let repo = Repo::new();
    repo.commit_base(
        "fuzz/Cargo.toml",
        &format!("bin = [\n{parse}{eval}]\n\n{PACKAGE}"),
        "base: fuzz manifest",
    );
    repo.write(
        "fuzz/Cargo.toml",
        &format!("bin = [\n{parse}]\n\n{PACKAGE}"),
    );
    repo.commit("fuzz: edit the manifest");
    let run = repo.check(&[]);
    assert_eq!(removed(&run), ["eval"], "{}", run.stdout);
}

/// The entry is gone and a comment on one line still spells it: the target is removed.
#[test]
fn a_target_left_only_in_a_comment_is_reported_as_removed() {
    let keep = "[[bin]]\nname = \"parse\"\n";
    let repo = manifest_change(
        &format!("{keep}\n[[bin]]\nname = \"eval\"\n"),
        &format!("{keep}\n# [[bin]] name = \"eval\"\n"),
    );
    let run = repo.check(&[]);
    assert_eq!(removed(&run), ["eval"], "{}", run.stdout);
}

/// The entry is gone and a multi-line string still spells it.
#[test]
fn a_target_left_only_inside_a_string_is_reported_as_removed() {
    let keep = "[[bin]]\nname = \"parse\"\n";
    let repo = manifest_change(
        &format!("{keep}\n[[bin]]\nname = \"eval\"\n"),
        &format!(
            "{keep}\n[package.metadata]\nretired = \"\"\"\n[[bin]]\nname = \"eval\"\n\"\"\"\n"
        ),
    );
    let run = repo.check(&[]);
    assert_eq!(removed(&run), ["eval"], "{}", run.stdout);
}

// ---- removals a pattern over the text reports and that did not happen -----------------

#[test]
fn a_target_whose_keys_are_reordered_is_not_reported() {
    let repo = manifest_change(
        "[[bin]]\nname = \"parse\"\npath = \"fuzz_targets/parse.rs\"\n",
        "[[bin]]\npath = \"fuzz_targets/parse.rs\"\nname = \"parse\"\n",
    );
    let run = repo.check(&[]);
    assert_eq!(removed(&run), [] as [&str; 0], "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

#[test]
fn a_target_that_gains_a_comment_above_its_name_is_not_reported() {
    let repo = manifest_change(
        "[[bin]]\nname = \"parse\"\n",
        "[[bin]]\n# reads one statement\nname = \"parse\"\n",
    );
    let run = repo.check(&[]);
    assert_eq!(removed(&run), [] as [&str; 0], "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

#[test]
fn a_target_rewritten_with_a_literal_string_is_not_reported() {
    let repo = manifest_change("[[bin]]\nname = \"parse\"\n", "[[bin]]\nname = 'parse'\n");
    let run = repo.check(&[]);
    assert_eq!(removed(&run), [] as [&str; 0], "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

/// A comment that spelled an entry was never a target, so deleting it removes none.
#[test]
fn deleting_a_comment_that_spells_an_entry_is_not_a_removal() {
    let keep = "[[bin]]\nname = \"parse\"\n";
    let repo = manifest_change(&format!("{keep}\n# [[bin]] name = \"old_eval\"\n"), keep);
    let run = repo.check(&[]);
    assert_eq!(removed(&run), [] as [&str; 0], "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

/// The same for a multi-line string.
#[test]
fn deleting_a_string_that_spells_an_entry_is_not_a_removal() {
    let keep = "[[bin]]\nname = \"parse\"\n";
    let repo = manifest_change(
        &format!(
            "{keep}\n[package.metadata]\nexample = \"\"\"\n[[bin]]\nname = \"old_eval\"\n\"\"\"\n"
        ),
        &format!("{keep}\n[package.metadata]\nexample = \"see the fuzzing guide\"\n"),
    );
    let run = repo.check(&[]);
    assert_eq!(removed(&run), [] as [&str; 0], "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

// ---- a manifest that does not parse -------------------------------------------------

/// The head manifest is not TOML: its targets cannot be read, and that is not the same
/// as a manifest that lists none or all of them. The run could not check.
#[test]
fn a_head_manifest_that_does_not_parse_stops_the_check() {
    let base = "[[bin]]\nname = \"parse\"\n\n[[bin]]\nname = \"eval\"\n";
    // The unclosed array breaks the file; both entries still stand in the text.
    let repo = manifest_change(base, &format!("{base}\n[features]\ndefault = [\n"));
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let (reason, gate) = run.could_not_check();
    assert_eq!(reason, "gate");
    assert_eq!(gate.as_deref(), Some("test-budget"));
    assert!(
        run.stderr
            .contains("`fuzz/Cargo.toml` does not parse as TOML (line "),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("its fuzz targets could not be read"),
        "{}",
        run.stderr
    );
}

/// The base manifest is not TOML and the change repairs it: there is no base list to
/// compare with, and the notes say so. This gate does not stop the repair.
#[test]
fn a_base_manifest_that_does_not_parse_is_named_in_the_notes() {
    let head = "[[bin]]\nname = \"parse\"\n";
    let repo = manifest_change(&format!("{head}\n[features]\ndefault = [\n"), head);
    let run = repo.check(&[]);
    // `toolchain-config` reports the unparsed side as its own finding, so the exit code
    // is not this gate's to pin: the run was checked, and this gate reports nothing.
    assert_ne!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(
        run.violations("test-budget"),
        Vec::<serde_json::Value>::new()
    );
    let notes = notes(&run);
    assert!(
        notes.iter().any(|n| n.contains("`fuzz/Cargo.toml`")
            && n.contains("does not parse as TOML")
            && n.contains("on the base side")
            && n.contains("removed fuzz targets were not looked for")),
        "{notes:#?}"
    );
}

/// A manifest that parses gives no such note.
#[test]
fn a_manifest_that_parses_gives_no_note() {
    let repo = manifest_change(
        "[[bin]]\nname = \"parse\"\n",
        "[[bin]]\nname = \"parse\"\ntest = false\n",
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        !notes(&run).iter().any(|n| n.contains("does not parse")),
        "{:#?}",
        notes(&run)
    );
}

// ---- seed corpus --------------------------------------------------------------------

/// A deleted seed is reported whatever the change adds to the same directory: the count
/// is of deleted files, not the net size of the directory.
#[test]
fn seeds_added_to_a_corpus_directory_do_not_offset_a_deleted_seed() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("corpus/parse/seed-1", "one\n"),
            ("corpus/parse/seed-2", "two\n"),
        ],
        "base: two seeds",
    );
    repo.remove("corpus/parse/seed-1");
    repo.write("corpus/parse/seed-3", "three\n");
    repo.write("corpus/parse/seed-4", "four\n");
    repo.commit("corpus: one seed out, two in");
    let run = repo.check(&[]);
    let messages: Vec<String> = run
        .violations("test-budget")
        .iter()
        .filter(|v| v["code"] == "test-budget/seed-corpus-decreased")
        .map(|v| v["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        messages,
        ["Seed corpus directory `corpus/parse` lost 1 seed file(s) without an explicit override."],
        "{}",
        run.stdout
    );
}
