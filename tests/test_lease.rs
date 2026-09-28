//! `discipline lease` across two worktrees of one repository, through the real binary
//! (docs/ROADMAP.md, Phase 13 Step 2).

mod common;

use common::Repo;

/// A repository with a second worktree at `wt2` on branch `feat/b`, and a stacked branch
/// `feat/stack` that no worktree has checked out.
fn two_worktrees() -> Repo {
    let repo = Repo::new();
    repo.git(&["branch", "feat/stack"]);
    std::fs::write(repo.path().join(".git/info/exclude"), "wt2/\n").unwrap();
    repo.git(&["worktree", "add", "-q", "-b", "feat/b", "wt2"]);
    repo
}

#[test]
fn a_branch_leased_in_one_worktree_is_refused_to_another_until_stolen_or_released() {
    let repo = two_worktrees();
    // The main worktree (on `work`) claims its own branch and the stacked one.
    let take = repo.run(
        &[
            "lease",
            "take",
            "--agent",
            "claude-code",
            "--session",
            "s1",
            "--branch",
            "work",
            "--branch",
            "feat/stack",
        ],
        &[],
    );
    assert_eq!(take.code, 0, "{}{}", take.stdout, take.stderr);
    assert!(
        take.stdout
            .contains("worktree `main` holds work, feat/stack"),
        "{}",
        take.stdout
    );

    // From the second worktree, the stacked branch is claimed by another live lease.
    let check = repo.run_in_dir("wt2", &["lease", "check", "--branch", "feat/stack"], &[]);
    assert_eq!(check.code, 1, "{}{}", check.stdout, check.stderr);
    assert!(
        check
            .stderr
            .contains("`feat/stack` is leased by worktree `main` (claude-code session s1"),
        "{}",
        check.stderr
    );
    // Its own worktree's branch is free there, and the holder sees its own claim as free.
    assert_eq!(
        repo.run_in_dir("wt2", &["lease", "check", "--branch", "feat/b"], &[])
            .code,
        0
    );
    assert_eq!(
        repo.run(&["lease", "check", "--branch", "feat/stack"], &[])
            .code,
        0
    );

    // Taking it from the second worktree is refused, then allowed with --steal, and said.
    let refused = repo.run_in_dir(
        "wt2",
        &[
            "lease",
            "take",
            "--agent",
            "copilot",
            "--branch",
            "feat/stack",
        ],
        &[],
    );
    assert_ne!(refused.code, 0);
    assert!(
        refused.stderr.contains("take it with --steal"),
        "{}",
        refused.stderr
    );
    let stolen = repo.run_in_dir(
        "wt2",
        &[
            "lease",
            "take",
            "--agent",
            "copilot",
            "--branch",
            "feat/stack",
            "--steal",
        ],
        &[],
    );
    assert_eq!(stolen.code, 0, "{}", stolen.stderr);
    assert!(
        stolen
            .stderr
            .contains("took `feat/stack` from worktree `main`"),
        "{}",
        stolen.stderr
    );
    assert_eq!(
        repo.run(&["lease", "check", "--branch", "feat/stack"], &[])
            .code,
        1
    );

    // Both worktrees see the same leases (the common git directory).
    let list = repo.run_in_dir("wt2", &["lease", "list", "--json"], &[]);
    let rows: serde_json::Value = serde_json::from_str(&list.stdout).unwrap();
    let keys: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, vec!["main", "wt2"], "{}", list.stdout);
    assert_eq!(rows[0]["branches"], serde_json::json!(["work"]));
    assert_eq!(rows[1]["here"], true);

    // Released, the branch is free again.
    assert_eq!(repo.run_in_dir("wt2", &["lease", "release"], &[]).code, 0);
    assert_eq!(
        repo.run(&["lease", "check", "--branch", "feat/stack"], &[])
            .code,
        0
    );
}

#[test]
fn a_lease_past_its_ttl_claims_nothing() {
    let repo = two_worktrees();
    // A lease whose heartbeat is older than its time-to-live, as a crashed session leaves it.
    let dir = repo.path().join(".git/discipline/leases");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.json"),
        r#"{"agent":"claude-code","session":"s1","worktree":"/w","branches":["feat/stack"],"taken_at":1000,"heartbeat":1000,"ttl_secs":60}"#,
    )
    .unwrap();
    assert_eq!(
        repo.run_in_dir("wt2", &["lease", "check", "--branch", "feat/stack"], &[])
            .code,
        0
    );
    let list = repo.run(&["lease", "list"], &[]);
    assert!(list.stdout.starts_with("stale"), "{}", list.stdout);
    // And the branch can be taken without --steal.
    let take = repo.run_in_dir("wt2", &["lease", "take", "--branch", "feat/stack"], &[]);
    assert_eq!(take.code, 0, "{}", take.stderr);
}

#[test]
fn a_corrupt_lease_file_could_not_check() {
    let repo = two_worktrees();
    let dir = repo.path().join(".git/discipline/leases");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.json"), "{").unwrap();
    let check = repo.run_in_dir("wt2", &["lease", "check", "--branch", "feat/stack"], &[]);
    assert_eq!(
        check.code, 2,
        "a guard never passes over a lease it cannot read: {}",
        check.stderr
    );
}

/// Plain `git` in `dir` with the installed hooks (the harness's own git runs without
/// hooks), `discipline` first on `PATH`.
fn git_with_hooks(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_discipline"))
        .parent()
        .unwrap()
        .to_path_buf();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = std::process::Command::new("git");
    // An explicit identity: the runner has no global one, and GIT_* is cleared below.
    cmd.args([
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=discipline test",
        "-c",
        "user.email=test@example.invalid",
    ])
    .args(args)
    .current_dir(dir)
    .env("PATH", path);
    for (k, _) in std::env::vars() {
        if k.starts_with("GIT_") || k.starts_with("DISCIPLINE_") {
            cmd.env_remove(k);
        }
    }
    cmd.output().unwrap()
}

/// As [`git_with_hooks`], with the `discipline` found first on `PATH` taken from `bin`.
fn git_with_discipline_in(
    dir: &std::path::Path,
    args: &[&str],
    bin: &std::path::Path,
) -> std::process::Output {
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = std::process::Command::new("git");
    // An explicit identity: the runner has no global one, and GIT_* is cleared below.
    cmd.args([
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=discipline test",
        "-c",
        "user.email=test@example.invalid",
    ])
    .args(args)
    .current_dir(dir)
    .env("PATH", path);
    for (k, _) in std::env::vars() {
        if k.starts_with("GIT_") || k.starts_with("DISCIPLINE_") {
            cmd.env_remove(k);
        }
    }
    cmd.output().unwrap()
}

#[test]
fn the_ref_guard_refuses_moving_a_branch_another_worktree_leased() {
    let repo = two_worktrees();
    let install = repo.run(&["lease", "install-guard"], &[]);
    assert_eq!(install.code, 0, "{}{}", install.stdout, install.stderr);
    assert!(repo
        .path()
        .join(".git/hooks/reference-transaction")
        .is_file());
    let take = repo.run(
        &[
            "lease",
            "take",
            "--agent",
            "claude-code",
            "--session",
            "s1",
            "--branch",
            "feat/stack",
        ],
        &[],
    );
    assert_eq!(take.code, 0, "{}", take.stderr);

    let wt2 = repo.path().join("wt2");
    // From the second worktree: moving, resetting to, or deleting the leased branch is refused.
    for args in [
        vec!["branch", "-f", "feat/stack", "HEAD"],
        vec!["update-ref", "refs/heads/feat/stack", "HEAD"],
        vec!["branch", "-D", "feat/stack"],
    ] {
        let out = git_with_hooks(&wt2, &args);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} was allowed: {err}");
        assert!(
            err.contains("`feat/stack` is leased by worktree `main` (claude-code session s1"),
            "{args:?}: {err}"
        );
    }
    // Its own branch, and the holder's own update, go through.
    assert!(
        git_with_hooks(&wt2, &["commit", "-q", "--allow-empty", "-m", "wt2 work"])
            .status
            .success()
    );
    assert!(
        git_with_hooks(repo.path(), &["branch", "-f", "feat/stack", "HEAD"])
            .status
            .success()
    );

    // Released, the second worktree may move it.
    assert_eq!(repo.run(&["lease", "release"], &[]).code, 0);
    let out = git_with_hooks(&wt2, &["branch", "-f", "feat/stack", "HEAD"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // An earlier release's guard (same marker, older body) is rewritten.
    let hook = repo.path().join(".git/hooks/reference-transaction");
    let current = std::fs::read_to_string(&hook).unwrap();
    std::fs::write(
        &hook,
        current.replace("discipline lease --help >/dev/null 2>&1 || exit 0\n", ""),
    )
    .unwrap();
    assert_eq!(repo.run(&["lease", "install-guard"], &[]).code, 0);
    assert_eq!(
        std::fs::read_to_string(&hook).unwrap(),
        current,
        "the earlier guard was not updated"
    );
    // Installing twice is a no-op; a foreign hook is never rewritten.
    assert!(repo
        .run(&["lease", "install-guard"], &[])
        .stdout
        .contains("already installed"));
    std::fs::write(
        repo.path().join(".git/hooks/reference-transaction"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    let foreign = repo.run(&["lease", "install-guard"], &[]);
    assert_ne!(foreign.code, 0);
    assert!(
        foreign.stderr.contains("is not the lease guard"),
        "{}",
        foreign.stderr
    );
}

/// The failure the guard exists for: a rebase with `--update-refs` in one worktree would
/// rewrite a stacked branch that another worktree's session leased.
#[test]
fn a_rebase_that_would_update_a_leased_stacked_branch_is_refused() {
    let repo = two_worktrees();
    assert_eq!(repo.run(&["lease", "install-guard"], &[]).code, 0);
    let wt2 = repo.path().join("wt2");
    // feat/b carries two commits; feat/stack points at the first (a stacked branch).
    std::fs::write(wt2.join("one.txt"), "1\n").unwrap();
    assert!(git_with_hooks(&wt2, &["add", "one.txt"]).status.success());
    assert!(git_with_hooks(&wt2, &["commit", "-q", "-m", "one"])
        .status
        .success());
    assert!(
        git_with_hooks(&wt2, &["branch", "-f", "feat/stack", "HEAD"])
            .status
            .success()
    );
    std::fs::write(wt2.join("two.txt"), "2\n").unwrap();
    assert!(git_with_hooks(&wt2, &["add", "two.txt"]).status.success());
    assert!(git_with_hooks(&wt2, &["commit", "-q", "-m", "two"])
        .status
        .success());
    // The base moves; the main worktree's session leases the stacked branch.
    std::fs::write(repo.path().join("base.txt"), "b\n").unwrap();
    assert!(git_with_hooks(repo.path(), &["add", "base.txt"])
        .status
        .success());
    assert!(
        git_with_hooks(repo.path(), &["commit", "-q", "-m", "base moves"])
            .status
            .success()
    );
    assert_eq!(
        repo.run(
            &[
                "lease",
                "take",
                "--agent",
                "copilot",
                "--branch",
                "feat/stack"
            ],
            &[]
        )
        .code,
        0
    );
    let before = git_with_hooks(&wt2, &["rev-parse", "feat/stack"]).stdout;

    let out = git_with_hooks(&wt2, &["rebase", "-q", "--update-refs", "work"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("`feat/stack` is leased by worktree `main`"),
        "{err}"
    );
    // Whatever git did with the rest of the rebase, the leased branch did not move.
    let _ = git_with_hooks(&wt2, &["rebase", "--abort"]);
    assert_eq!(
        git_with_hooks(&wt2, &["rev-parse", "feat/stack"]).stdout,
        before
    );
}

/// An installed discipline older than `lease` (0.14.4 answers
/// `unrecognized subcommand 'lease'` with exit 2) has no guard to run. The hook lets
/// the update through silently rather than aborting every ref update in every worktree.
#[test]
fn the_guard_passes_silently_when_the_installed_discipline_predates_lease() {
    let repo = two_worktrees();
    assert_eq!(repo.run(&["lease", "install-guard"], &[]).code, 0);
    let take = repo.run(&["lease", "take", "--branch", "feat/stack"], &[]);
    assert_eq!(take.code, 0, "{}", take.stderr);
    // An older discipline first on PATH: it knows no `lease` subcommand.
    let old = tempfile::tempdir().unwrap();
    let shim = old.path().join("discipline");
    std::fs::write(
        &shim,
        "#!/bin/sh\necho \"error: unrecognized subcommand '$1'\" >&2\nexit 2\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let wt2 = repo.path().join("wt2");
    let out = git_with_discipline_in(&wt2, &["branch", "-f", "feat/stack", "HEAD"], old.path());
    assert!(
        out.status.success(),
        "an old discipline aborted the update: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("discipline"),
        "silent: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
