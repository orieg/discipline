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
    // A shell command is judged too, so a lease it cannot read refuses it as well.
    let bash = fixture("claude-code/bash.json").replace("/work/repo", main.to_str().unwrap());
    assert_eq!(pretool(&main, "claude-code", &bash, &[]).code, 2);
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

/// The recorded shell payload of `agent`, its command replaced by `command`.
fn shell(agent: &str, cwd: &Path, command: &str) -> String {
    let (rel, recorded) = match agent {
        "claude-code" => ("claude-code/bash.json", "echo hi > b.txt"),
        "copilot" => ("copilot/bash.json", "git -C wt2 status"),
        "agy" => ("agy/run_command.json", "git -C wt2 status"),
        "opencode" => ("opencode/bash.json", "git -C wt2 status"),
        _ => unreachable!(),
    };
    let text = fixture(rel);
    assert!(text.contains(recorded), "{rel}");
    text.replace(recorded, command)
        .replace("/work/repo", cwd.to_str().unwrap())
}

#[test]
fn each_agent_is_refused_a_shell_command_that_acts_on_another_worktree() {
    let (_repo, main, _wt2) = two_worktrees();
    for (agent, _) in AGENTS {
        for refused in [
            "git -C wt2 commit -m x",
            "echo hi > wt2/escape.txt",
            "cd wt2",
        ] {
            let o = pretool(&main, agent, &shell(agent, &main, refused), &[]);
            assert!(
                denied(agent, &o),
                "{agent} `{refused}`: {} {} {}",
                o.code,
                o.stdout,
                o.stderr
            );
            assert!(
                format!("{}{}", o.stdout, o.stderr).contains("worktree `wt2`"),
                "{agent} `{refused}`"
            );
        }
        for allowed in ["git -C wt2 status", "echo hi > own.txt", "ls wt2"] {
            let o = pretool(&main, agent, &shell(agent, &main, allowed), &[]);
            assert!(
                !denied(agent, &o) && o.code == 0,
                "{agent} `{allowed}`: {} {} {}",
                o.code,
                o.stdout,
                o.stderr
            );
        }
    }
}

#[test]
fn a_force_push_of_a_branch_another_worktree_leased_is_refused() {
    let (repo, main, wt2) = two_worktrees();
    repo.git(&["branch", "feat/stack"]);
    let take = repo.run_in_dir(
        "wt2",
        &[
            "lease",
            "take",
            "--agent",
            "copilot",
            "--session",
            "s2",
            "--branch",
            "feat/stack",
        ],
        &[],
    );
    assert_eq!(take.code, 0, "{}", take.stderr);
    let _ = wt2;
    let o = pretool(
        &main,
        "claude-code",
        &shell("claude-code", &main, "git push --force origin feat/stack"),
        &[],
    );
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(
        o.stderr.contains("worktree `wt2` has leased"),
        "{}",
        o.stderr
    );
    let plain = pretool(
        &main,
        "claude-code",
        &shell("claude-code", &main, "git push origin feat/stack"),
        &[],
    );
    assert_eq!(plain.code, 0, "{}", plain.stderr);
}

/// `discipline hook run --agent <agent> --event session-start` in `dir`, `payload` on stdin.
fn session_start(dir: &Path, agent: &str, payload: &str) -> Out {
    session_start_args(dir, agent, payload, &[])
}

/// [`session_start`] with `extra` arguments.
fn session_start_args(dir: &Path, agent: &str, payload: &str, extra: &[&str]) -> Out {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"));
    cmd.args(["hook", "run", "--agent", agent, "--event", "session-start"])
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

/// A session-start payload for `agent` naming `session`, started in `cwd`. OpenCode's
/// is the shape its generated plugin sends.
fn start_payload(agent: &str, cwd: &Path, session: &str) -> String {
    let rel = match agent {
        "opencode" => {
            return serde_json::json!({ "input": { "sessionID": session }, "cwd": cwd })
                .to_string();
        }
        a => format!("{a}/session_start.json"),
    };
    fixture(&rel)
        .replace("/work/repo", cwd.to_str().unwrap())
        .replace("00000000-0000-0000-0000-000000000000", session)
}

/// The recorded edit payload `rel`, editing `target` in `cwd` as `session`.
fn edit_as(rel: &str, cwd: &Path, target: &Path, session: &str) -> String {
    payload(rel, cwd, target).replace("00000000-0000-0000-0000-000000000000", session)
}

fn lease_list(repo: &Repo) -> String {
    let o = repo.run(&["lease", "list", "--json"], &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    o.stdout
}

#[test]
fn each_agent_session_start_leases_its_worktree_and_branch() {
    for (agent, rel) in AGENTS {
        let (repo, _main, wt2) = two_worktrees();
        let o = session_start(&wt2, agent, &start_payload(agent, &wt2, "sess-a"));
        assert_eq!(o.code, 0, "{agent}: {}", o.stderr);
        assert!(o.stdout.is_empty(), "{agent}: {}", o.stdout);
        let leases = lease_list(&repo);
        assert!(leases.contains("\"sess-a\""), "{agent}: {leases}");
        assert!(leases.contains("\"feat/b\""), "{agent}: {leases}");
        assert!(
            leases.contains(&format!("\"{agent}\"")),
            "{agent}: {leases}"
        );

        // The same session edits there; another session is refused.
        let target = wt2.join("a.txt");
        let own = pretool(&wt2, agent, &edit_as(rel, &wt2, &target, "sess-a"), &[]);
        assert!(
            !denied(agent, &own),
            "{agent}: {} {}",
            own.stdout,
            own.stderr
        );
        let other = pretool(&wt2, agent, &edit_as(rel, &wt2, &target, "sess-b"), &[]);
        assert!(
            denied(agent, &other),
            "{agent}: {} {}",
            other.stdout,
            other.stderr
        );
    }
}

#[test]
fn a_second_session_leaves_a_live_lease_alone_and_says_so() {
    let (repo, main, _wt2) = two_worktrees();
    assert_eq!(
        session_start(
            &main,
            "claude-code",
            &start_payload("claude-code", &main, "sess-a")
        )
        .code,
        0
    );
    let o = session_start(&main, "copilot", &start_payload("copilot", &main, "sess-b"));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stderr.contains("leased by claude-code session sess-a"),
        "{}",
        o.stderr
    );
    let leases = lease_list(&repo);
    assert!(
        leases.contains("\"sess-a\"") && !leases.contains("\"sess-b\""),
        "{leases}"
    );

    // The same session starting again (a resume) keeps its lease without a note.
    let again = session_start(
        &main,
        "claude-code",
        &start_payload("claude-code", &main, "sess-a"),
    );
    assert_eq!(again.code, 0);
    assert!(again.stderr.is_empty(), "{}", again.stderr);
}

#[test]
fn a_stale_lease_is_taken_over_by_a_new_session() {
    let (repo, main, _wt2) = two_worktrees();
    let take = repo.run(
        &[
            "lease",
            "take",
            "--agent",
            "copilot",
            "--session",
            "old",
            "--ttl",
            "1",
        ],
        &[],
    );
    assert_eq!(take.code, 0, "{}", take.stderr);
    // Age the heartbeat past its time-to-live instead of waiting for it.
    let dir = repo.path().join(".git/discipline/leases");
    for f in std::fs::read_dir(&dir).unwrap() {
        let p = f.unwrap().path();
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        v["heartbeat"] = serde_json::json!(0);
        std::fs::write(&p, v.to_string()).unwrap();
    }
    let o = session_start(
        &main,
        "claude-code",
        &start_payload("claude-code", &main, "new"),
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let leases = lease_list(&repo);
    assert!(
        leases.contains("\"new\"") && !leases.contains("\"old\""),
        "{leases}"
    );
}

#[test]
fn a_branch_leased_in_another_worktree_is_left_out_of_the_new_lease() {
    let (repo, main, wt2) = two_worktrees();
    // wt2's session claims the main worktree's branch too.
    let branch = String::from_utf8(
        std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&main)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let take = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"))
        .args([
            "lease",
            "take",
            "--agent",
            "agy",
            "--session",
            "s-wt2",
            "--branch",
            &branch,
        ])
        .current_dir(&wt2)
        .output()
        .unwrap();
    assert!(
        take.status.success(),
        "{}",
        String::from_utf8_lossy(&take.stderr)
    );

    let o = session_start(
        &main,
        "claude-code",
        &start_payload("claude-code", &main, "s-main"),
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stderr.contains("but not its branch"), "{}", o.stderr);
    let leases = lease_list(&repo);
    assert!(leases.contains("\"s-main\""), "{leases}");
}

#[test]
fn a_session_start_outside_a_repository_or_unparsed_passes_silently() {
    let dir = tempfile::tempdir().unwrap();
    let o = session_start(
        dir.path(),
        "claude-code",
        &start_payload("claude-code", dir.path(), "s"),
    );
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
    let o = session_start(dir.path(), "claude-code", "not json");
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
}

/// With `--if-configured` (the user-level Copilot hook), the pre-tool check and the
/// session-start lease pass silently in a repository without a `discipline.toml`, and
/// act as without it in one that has it.
#[test]
fn if_configured_acts_only_in_a_repository_with_discipline_toml() {
    let (repo, main, wt2) = two_worktrees();
    let edit = payload("copilot/create.json", &main, &wt2.join("escape.txt"));
    let start = start_payload("copilot", &main, "sess-a");
    let guarded = ["--if-configured"];

    let o = pretool(&main, "copilot", &edit, &guarded);
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
    let o = session_start_args(&main, "copilot", &start, &guarded);
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""));
    assert!(!lease_list(&repo).contains("sess-a"));
    // A payload that cannot be read outside a configured repository passes too.
    let o = pretool(&main, "copilot", "not json", &guarded);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""));

    std::fs::write(main.join("discipline.toml"), "").unwrap();
    let o = pretool(&main, "copilot", &edit, &guarded);
    assert!(denied("copilot", &o), "{} {}", o.stdout, o.stderr);
    // The payload's directory decides, not the hook's own working directory.
    let elsewhere = tempfile::tempdir().unwrap();
    let o = pretool(elsewhere.path(), "copilot", &edit, &guarded);
    assert!(denied("copilot", &o), "{} {}", o.stdout, o.stderr);
    let o = session_start_args(&main, "copilot", &start, &guarded);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(lease_list(&repo).contains("sess-a"));
    let o = pretool(&main, "copilot", "not json", &guarded);
    assert!(denied("copilot", &o), "{} {}", o.stdout, o.stderr);
}
