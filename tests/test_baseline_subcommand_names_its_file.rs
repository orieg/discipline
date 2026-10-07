//! The `baseline` subcommand names its file as every other message does (#651).
//!
//! `--baseline-file` (or `DISCIPLINE_BASELINE`) can point outside the repository, into a
//! directory that is the runner's. The progress, "nothing to migrate" and refusal lines of
//! `discipline baseline` echoed the argument as typed, directory included. They now name
//! the file by its path in the repository, or by its file name alone when it is outside.

mod common;
use common::Repo;

/// A directory outside the repository whose name must not reach any output.
fn outside() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("runner-private-dir");
    std::fs::create_dir(&dir).unwrap();
    (
        parent,
        dir.join("known.toml"),
        "runner-private-dir".to_string(),
    )
}

fn output(repo: &Repo, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let run = repo.run(args, env);
    (run.code, format!("{}{}", run.stdout, run.stderr))
}

#[test]
fn nothing_to_migrate_names_the_file_only() {
    let repo = Repo::new();
    let (_keep, file, private) = outside();
    let path = file.to_str().unwrap();
    let (code, out) = output(
        &repo,
        &["baseline", "--migrate", "--baseline-file", path],
        &[],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("no baseline at `known.toml`; nothing to migrate."),
        "{out}"
    );
    assert!(!out.contains(&private), "{out}");

    // Through the environment variable, and for a file that already has the version.
    std::fs::write(&file, "version = 2\n").unwrap();
    let (code, out) = output(
        &repo,
        &["baseline", "--migrate"],
        &[("DISCIPLINE_BASELINE", path)],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("`known.toml` already uses fingerprint version 2; nothing to migrate."),
        "{out}"
    );
    assert!(!out.contains(&private), "{out}");
}

#[test]
fn the_dry_run_and_the_write_name_the_file_only() {
    let repo = Repo::new();
    let (_keep, file, private) = outside();
    let path = file.to_str().unwrap();
    let (code, out) = output(
        &repo,
        &["baseline", "--whole-tree", "--baseline-file", path],
        &[],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("to record them to known.toml."), "{out}");
    assert!(!out.contains(&private), "{out}");

    let (code, out) = output(
        &repo,
        &[
            "baseline",
            "--whole-tree",
            "--write",
            "--baseline-file",
            path,
        ],
        &[],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("grandfathered findings to known.toml"),
        "{out}"
    );
    assert!(!out.contains(&private), "{out}");
    assert!(file.is_file());
}

#[test]
fn the_refusal_to_write_part_of_a_version_one_file_names_the_file_only() {
    let repo = Repo::new();
    let (_keep, file, private) = outside();
    std::fs::write(&file, "version = 1\n").unwrap();
    let path = file.to_str().unwrap();
    let (code, out) = output(
        &repo,
        &[
            "baseline",
            "--whole-tree",
            "--write",
            "--suite",
            "hygiene",
            "--baseline-file",
            path,
        ],
        &[],
    );
    assert_ne!(code, 0, "{out}");
    assert!(
        out.contains("`known.toml` uses fingerprint version 1"),
        "{out}"
    );
    assert!(!out.contains(&private), "{out}");
}

/// Control: a file inside the repository is named by its path there.
#[test]
fn a_file_inside_the_repository_is_named_by_its_path() {
    let repo = Repo::new();
    let (code, out) = output(
        &repo,
        &[
            "baseline",
            "--migrate",
            "--baseline-file",
            "policy/known.toml",
        ],
        &[],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("no baseline at `policy/known.toml`; nothing to migrate."),
        "{out}"
    );
}
