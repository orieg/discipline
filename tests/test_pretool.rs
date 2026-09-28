//! `hook run --event pre-tool` through the real binary: each agent's recorded payload
//! (tests/fixtures/pretool/) re-targeted at two worktrees of one repository
//! (docs/ROADMAP.md, Phase 13 Step 3).

mod common;

use common::Repo;
use std::io::Write;
use std::path::Path;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// `discipline hook run --agent <agent> --event pre-tool [extra]` in `dir`, `payload` on stdin.
fn pretool(dir: &Path, agent: &str, payload: &str, extra: &[&str]) -> Out {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"));
    cmd.args(["hook", "run", "--agent", agent, "--event", "pre-tool"])
        .args(extra)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, _) in std::env::vars() {
        if k.starts_with("GIT_") || k.starts_with("DISCIPLINE_") {
            cmd.env_remove(k);
        }
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn fixture(rel: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/pretool/{rel}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The recorded payload with its probe repository (`/work/repo`) moved to `cwd`, and its
/// edit target `/work/repo/a.txt` moved to `target`.
fn payload(rel: &str, cwd: &Path, target: &Path) -> String {
    fixture(rel)
        .replace("/work/repo/a.txt", target.to_str().unwrap())
        .replace("/work/repo", cwd.to_str().unwrap())
}

/// A repository whose main worktree holds a second one at `wt2` (as agent worktrees
/// often sit inside the main checkout).
fn two_worktrees() -> (Repo, std::path::PathBuf, std::path::PathBuf) {
    let repo = Repo::new();
    std::fs::write(repo.path().join(".git/info/exclude"), "wt2/\n").unwrap();
    repo.git(&["worktree", "add", "-q", "-b", "feat/b", "wt2"]);
    let main = repo.path().canonicalize().unwrap();
    let wt2 = main.join("wt2");
    (repo, main, wt2)
}

/// Whether `agent`'s answer refused the call, in the shape recorded live.
fn denied(agent: &str, o: &Out) -> bool {
    match agent {
        "claude-code" => o.code == 2 && !o.stderr.is_empty(),
        "copilot" => o.code == 0 && o.stdout.contains(r#""permissionDecision":"deny""#),
        "agy" => o.code == 0 && o.stdout.contains(r#""decision":"deny""#),
        "opencode" => o.code == 1 && !o.stdout.is_empty(),
        _ => unreachable!(),
    }
}

const AGENTS: &[(&str, &str)] = &[
    ("claude-code", "claude-code/write.json"),
    ("copilot", "copilot/create.json"),
    ("agy", "agy/write_to_file.json"),
    ("opencode", "opencode/write.json"),
];

#[test]
fn each_agent_is_refused_an_edit_in_another_worktree_and_allowed_its_own() {
    let (_repo, main, wt2) = two_worktrees();
    for (agent, rel) in AGENTS {
        // From the main worktree's session, into wt2: refused, naming the worktree.
        let o = pretool(
            &main,
            agent,
            &payload(rel, &main, &wt2.join("src/lib.rs")),
            &[],
        );
        assert!(
            denied(agent, &o),
            "{agent}: {} {} {}",
            o.code,
            o.stdout,
            o.stderr
        );
        assert!(
            format!("{}{}", o.stdout, o.stderr).contains("worktree `wt2`"),
            "{agent}: {}{}",
            o.stdout,
            o.stderr
        );
        // Its own worktree: allowed.
        let own = pretool(
            &main,
            agent,
            &payload(rel, &main, &main.join("src/new.rs")),
            &[],
        );
        assert!(
            !denied(agent, &own) && own.code == 0,
            "{agent}: {} {} {}",
            own.code,
            own.stdout,
            own.stderr
        );
        // From wt2 into the main worktree: refused too.
        let back = pretool(
            &wt2,
            agent,
            &payload(rel, &wt2, &main.join("src/lib.rs")),
            &[],
        );
        assert!(
            denied(agent, &back),
            "{agent}: {}{}",
            back.stdout,
            back.stderr
        );
    }
}

#[test]
fn a_worktree_leased_by_another_session_is_refused_until_released() {
    let (repo, main, _wt2) = two_worktrees();
    let take = repo.run(
        &[
            "lease",
            "take",
            "--agent",
            "copilot",
            "--session",
            "other-session",
            "--branch",
            "work",
        ],
        &[],
    );
    assert_eq!(take.code, 0, "{}", take.stderr);
    let p = payload("claude-code/write.json", &main, &main.join("a.txt"));
    let o = pretool(&main, "claude-code", &p, &[]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(
        o.stderr.contains("leased by copilot session other-session"),
        "{}",
        o.stderr
    );
    assert_eq!(repo.run(&["lease", "release"], &[]).code, 0);
    assert_eq!(pretool(&main, "claude-code", &p, &[]).code, 0);
}

#[test]
fn observe_mode_logs_and_lets_the_edit_through() {
    let (_repo, main, wt2) = two_worktrees();
    let o = pretool(
        &main,
        "claude-code",
        &payload("claude-code/write.json", &main, &wt2.join("x")),
        &["--observe"],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stderr.contains("observe mode): would refuse"),
        "{}",
        o.stderr
    );
    let log = std::fs::read_to_string(main.join(".git/discipline/hook-observe.log")).unwrap();
    assert!(log.contains(r#""event":"pre-tool""#), "{log}");
}

#[test]
fn a_check_that_cannot_be_made_refuses_the_edit() {
    let (_repo, main, _wt2) = two_worktrees();
    let dir = main.join(".git/discipline/leases");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("wt2.json"), "{").unwrap();
    let o = pretool(
        &main,
        "claude-code",
        &payload("claude-code/write.json", &main, &main.join("a.txt")),
        &[],
    );
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(
        o.stderr.contains("could not check where this edit goes"),
        "{}",
        o.stderr
    );
    // A payload that is not JSON is refused as well.
    assert_eq!(pretool(&main, "claude-code", "not json", &[]).code, 2);
    // A shell command is not an edit: this check does not refuse it (shell commands are
    // a later step), even with a lease it cannot read.
    let bash = fixture("claude-code/bash.json").replace("/work/repo", main.to_str().unwrap());
    assert_eq!(pretool(&main, "claude-code", &bash, &[]).code, 0);
}

#[test]
fn an_edit_outside_any_repository_is_not_this_checks_concern() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    let o = pretool(
        &dir,
        "claude-code",
        &payload("claude-code/write.json", &dir, &dir.join("notes.md")),
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
}
