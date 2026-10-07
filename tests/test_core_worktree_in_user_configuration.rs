//! A `core.worktree` in the user's git configuration stops the run with a message that
//! says so (#651).
//!
//! The setting applies to every repository the user opens. Pointing at a directory that
//! does not exist, it made every run stop as "not inside a git repository" with the
//! library's error, which echoed the configured path. Pointing at another directory, the
//! run read that directory as the worktree. Both now stop, exit 2, naming the key and the
//! last component of the path only: the rest of the path can be a home directory.

mod common;
use common::Repo;

/// A home directory whose `.gitconfig` sets `core.worktree` to `worktree`.
fn home_with_worktree(worktree: &std::path::Path) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".gitconfig"),
        format!("[core]\n\tworktree = {}\n", worktree.display()),
    )
    .unwrap();
    home
}

fn check(repo: &Repo, home: &std::path::Path) -> (i32, String) {
    let run = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("HOME", home.to_str().unwrap())],
    );
    (run.code, run.stderr)
}

fn changed_repo() -> Repo {
    let repo = Repo::new();
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes");
    repo
}

#[test]
fn a_configured_worktree_that_does_not_exist_is_named_by_key_and_file_name() {
    let repo = changed_repo();
    let parent = tempfile::tempdir().unwrap();
    let missing = parent
        .path()
        .join("private-parent-dir")
        .join("missing-tree");
    let home = home_with_worktree(&missing);
    let (code, err) = check(&repo, home.path());
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("`core.worktree`"), "{err}");
    assert!(
        err.contains("`missing-tree`") && err.contains("does not exist"),
        "{err}"
    );
    assert!(!err.contains("private-parent-dir"), "{err}");
    assert!(!err.contains(parent.path().to_str().unwrap()), "{err}");
}

#[test]
fn a_configured_worktree_that_is_another_directory_is_refused() {
    let repo = changed_repo();
    let parent = tempfile::tempdir().unwrap();
    let other = parent.path().join("private-parent-dir").join("other-tree");
    std::fs::create_dir_all(&other).unwrap();
    let home = home_with_worktree(&other);
    let (code, err) = check(&repo, home.path());
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("`core.worktree`"), "{err}");
    assert!(
        err.contains("`other-tree`") && err.contains("not the directory"),
        "{err}"
    );
    assert!(!err.contains("private-parent-dir"), "{err}");
}

/// Control: a user configuration without the key changes nothing.
#[test]
fn a_user_configuration_without_the_key_runs_as_before() {
    let repo = changed_repo();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".gitconfig"),
        "[core]\n\tautocrlf = false\n",
    )
    .unwrap();
    let (code, err) = check(&repo, home.path());
    assert_eq!(code, 0, "{err}");
}
