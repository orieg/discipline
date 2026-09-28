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
