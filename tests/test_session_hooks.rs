//! End-to-end tests for session-scoped agent hooks:
//! - `[hooks.verified-stop]` (`#700`): dirty working tree requires a passing verification
//!   record for the current tree digest before ending the turn.
//! - `[hooks.tool-loop]` (`#701`): consecutive identical tool calls without any working-tree
//!   change in between are refused after exceeding `max_identical_calls` /
//!   `max_identical_shell_calls`.

mod common;

use common::{Repo, Run};
use serde_json::{json, Value};
use std::io::Write;
use std::process::Stdio;

fn hook(repo: &Repo, args: &[&str], stdin: &str) -> Run {
    let mut cmd = common::discipline_cmd(repo.path());
    cmd.args(["hook", "run"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn recorded_stop(rel: &str, edit: impl FnOnce(&mut Value)) -> String {
    let path = format!("{}/tests/fixtures/stop/{rel}", env!("CARGO_MANIFEST_DIR"));
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut v);
    v.to_string()
}

fn recorded_pretool(rel: &str, edit: impl FnOnce(&mut Value)) -> String {
    let path = format!(
        "{}/tests/fixtures/pretool/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut v);
    v.to_string()
}

fn observe_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.file(".git/discipline/hook-observe.log")).unwrap_or_default()
}

#[test]
fn verified_stop_blocks_dirty_tree_until_vouched_command_passes_and_invalidates_on_edit() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[hooks.verified-stop]\nenabled = true\nmode = \"refuse\"\ncommands = [\"cargo test\"]\n",
        "chore: enable verified-stop",
    );
    repo.commit_base(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        "feat: init",
    );

    let stop_payload = recorded_stop("claude-code/stop.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-700".into();
        v["last_assistant_message"] = "All done.".into();
    });

    // 1. Clean tree passes.
    let run = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(run.code, 0, "clean tree must pass: {}", run.stderr);

    // 2. Docs-only change passes.
    repo.write("README.md", "# Updated docs\n");
    let run = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(run.code, 0, "docs-only change must pass: {}", run.stderr);

    // 3. Source change without verification is refused.
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn double(x: i32) -> i32 { x * 2 }\n",
    );
    let run = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(run.code, 2, "unverified dirty tree must be refused");
    assert!(
        run.stderr.contains("[turn/unverified-dirty-tree]") && run.stderr.contains("`cargo test`"),
        "unexpected stderr: {}",
        run.stderr
    );

    // 4. Spoof / partial / failing / vacuous commands do not satisfy verification.
    for (idx, (bad_cmd, exit_code, stdout)) in [
        ("cargo test --no-run", 0, "Executable unittests"),
        ("cargo test -- --list", 0, "1 test"),
        (
            "cargo test some_filter",
            0,
            "running 1 test\ntest result: ok. 1 passed; 0 failed\n",
        ),
        ("true", 0, ""),
        (
            "cargo test || true",
            0,
            "running 1 test\ntest result: ok. 1 passed; 0 failed\n",
        ),
        (
            "cargo test",
            1,
            "running 1 test\ntest result: FAILED. 0 passed; 1 failed\n",
        ),
        (
            "cargo test",
            0,
            "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let sess = format!("sess-700-check-{idx}");
        let post_payload = json!({
            "cwd": repo.path().display().to_string(),
            "session_id": sess,
            "tool_name": "Bash",
            "tool_input": { "command": bad_cmd },
            "tool_response": {
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": ""
            }
        })
        .to_string();
        let w = hook(
            &repo,
            &["--agent", "claude-code", "--event", "post-tool"],
            &post_payload,
        );
        assert_eq!(w.code, 0);

        let check_stop = recorded_stop("claude-code/stop.json", |v| {
            v["cwd"] = repo.path().display().to_string().into();
            v["session_id"] = sess.into();
            v["last_assistant_message"] = "All done.".into();
        });
        let r = hook(&repo, &["--agent", "claude-code"], &check_stop);
        assert_eq!(
            r.code, 2,
            "command `{bad_cmd}` (exit {exit_code}) must not satisfy verified-stop"
        );
    }

    // 5. Witnessing a passing `cargo test` on the current tree allows Stop to pass.
    let good_post = json!({
        "cwd": repo.path().display().to_string(),
        "session_id": "sess-700",
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" },
        "tool_response": {
            "exit_code": 0,
            "stdout": "running 2 tests\ntest result: ok. 2 passed; 0 failed\n",
            "stderr": ""
        }
    })
    .to_string();
    let w = hook(
        &repo,
        &["--agent", "claude-code", "--event", "post-tool"],
        &good_post,
    );
    assert_eq!(w.code, 0);

    let run = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(
        run.code, 0,
        "stop must pass after witnessed verification: {}",
        run.stderr
    );

    // 6. Modifying the working tree (e.g. via a shell edit) changes the tree digest and
    // invalidates the previous verification record.
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn triple(x: i32) -> i32 { x * 3 }\n",
    );
    let run = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(
        run.code, 2,
        "edit after verification must invalidate prior record"
    );
    assert!(run.stderr.contains("[turn/unverified-dirty-tree]"));
}

#[test]
fn verified_stop_observe_mode_corrupt_session_recovery_and_loop_guards() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[hooks.verified-stop]\nenabled = true\nmode = \"refuse\"\nmax_per_session = 2\ncommands = [\"cargo test\"]\n",
        "chore: enable verified-stop",
    );
    repo.commit_base("src/lib.rs", "pub fn f() -> i32 { 1 }\n", "feat: init");
    repo.write("src/lib.rs", "pub fn f() -> i32 { 2 }\n");

    let stop_payload = recorded_stop("claude-code/stop.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-cap".into();
        v["last_assistant_message"] = "Done.".into();
    });

    // Observe flag logs without blocking.
    let obs = hook(
        &repo,
        &["--agent", "claude-code", "--observe"],
        &stop_payload,
    );
    assert_eq!(obs.code, 0);
    assert!(
        obs.stderr
            .contains("would block: no passing verification for this tree"),
        "{}",
        obs.stderr
    );
    let log = observe_log(&repo);
    assert!(
        log.contains("\"verdict\":\"unverified-dirty-tree\""),
        "{log}"
    );

    // Corrupt session file does not grant a pass.
    let sessions_dir = repo.file(".git/discipline/sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::write(sessions_dir.join("sess-cap.json"), "{corrupt json").unwrap();

    // Refusals 1 and 2 are blocked; refusal 3 hits max_per_session = 2 and is let through.
    let r1 = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r1.code, 2);
    let r2 = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r2.code, 2);
    let r3 = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(
        r3.code, 0,
        "3rd stop must be let through at max_per_session"
    );
    assert!(
        r3.stderr.contains("hooks.verified-stop.max_per_session"),
        "{}",
        r3.stderr
    );

    // `stop_hook_active: true` is let through immediately.
    let continued = recorded_stop("claude-code/stop_continued.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-fresh".into();
        v["last_assistant_message"] = "Done.".into();
    });
    let rc = hook(&repo, &["--agent", "claude-code"], &continued);
    assert_eq!(rc.code, 0);
    assert!(rc.stderr.contains("loop guard"), "{}", rc.stderr);
}

#[test]
fn tool_loop_allows_up_to_cap_then_refuses_and_resets_on_input_or_tree_change() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[hooks.tool-loop]\nenabled = true\nmode = \"refuse\"\nmax_identical_calls = 3\nmax_identical_shell_calls = 5\n",
        "chore: enable tool-loop",
    );
    repo.commit_base("a.txt", "hello\n", "feat: add a.txt");

    let read_call = recorded_pretool("claude-code/read.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-loop".into();
        v["tool_input"] = json!({
            "file_path": repo.file("a.txt").display().to_string(),
            "offset": 0
        });
    });
    // Same logical input with reversed key order.
    let read_call_reordered = recorded_pretool("claude-code/read.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-loop".into();
        v["tool_input"] = json!({
            "offset": 0,
            "file_path": repo.file("a.txt").display().to_string()
        });
    });

    // Calls 1, 2, 3 are allowed.
    for i in 1..=3 {
        let payload = if i == 2 {
            &read_call_reordered
        } else {
            &read_call
        };
        let r = hook(
            &repo,
            &["--agent", "claude-code", "--event", "pre-tool"],
            payload,
        );
        assert_eq!(r.code, 0, "call {i} must be allowed: {}", r.stderr);
    }

    // Call 4 with identical input and unchanged tree is refused.
    let r4 = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool"],
        &read_call,
    );
    assert_eq!(r4.code, 2, "4th identical call must be refused");
    assert!(
        r4.stderr.contains("discipline [hooks.tool-loop]: `Read`: the same call was made 4 times in a row with no change in between; it is refused"),
        "unexpected stderr: {}",
        r4.stderr
    );

    // Changing the working tree resets the consecutive counter.
    repo.write("a.txt", "hello world\n");
    let after_edit = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool"],
        &read_call,
    );
    assert_eq!(
        after_edit.code, 0,
        "tree change must reset consecutive call counter: {}",
        after_edit.stderr
    );

    // Changing the tool arguments also resets the counter.
    for _ in 0..2 {
        let r = hook(
            &repo,
            &["--agent", "claude-code", "--event", "pre-tool"],
            &read_call,
        );
        assert_eq!(r.code, 0);
    }
    let different_arg = recorded_pretool("claude-code/read.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-loop".into();
        v["tool_input"] = json!({
            "file_path": repo.file("a.txt").display().to_string(),
            "offset": 10
        });
    });
    let r_diff = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool"],
        &different_arg,
    );
    assert_eq!(r_diff.code, 0, "different argument must reset counter");
}

#[test]
fn tool_loop_shell_cap_secret_non_retention_and_read_fail_open() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[hooks.tool-loop]\nenabled = true\nmode = \"refuse\"\nmax_identical_calls = 2\nmax_identical_shell_calls = 4\n",
        "chore: enable tool-loop",
    );

    let secret = "SUPER_SECRET_TOKEN_VALUE_999";
    let shell_call = json!({
        "cwd": repo.path().display().to_string(),
        "session_id": "sess-shell",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("echo {secret}")
        }
    })
    .to_string();

    // Shell calls use max_identical_shell_calls (4), so calls 1..=4 pass and call 5 is refused.
    for i in 1..=4 {
        let r = hook(
            &repo,
            &["--agent", "claude-code", "--event", "pre-tool"],
            &shell_call,
        );
        assert_eq!(r.code, 0, "shell call {i} must pass: {}", r.stderr);
    }
    let r5 = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool"],
        &shell_call,
    );
    assert_eq!(r5.code, 2, "5th identical shell call must be refused");
    assert!(!r5.stderr.contains(secret), "stderr leaked secret");

    // Observe mode also never writes raw arguments to hook-observe.log.
    let r6_obs = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool", "--observe"],
        &shell_call,
    );
    assert_eq!(r6_obs.code, 0);
    let session_json =
        std::fs::read_to_string(repo.file(".git/discipline/sessions/sess-shell.json")).unwrap();
    assert!(
        !session_json.contains(secret),
        "session file must never contain raw tool arguments: {session_json}"
    );
    let obs_log = observe_log(&repo);
    assert!(
        !obs_log.contains(secret),
        "hook-observe.log must never contain raw tool arguments: {obs_log}"
    );

    // A truncated/unparseable read tool payload fails open, while an unparseable edit/unknown payload is refused.
    let truncated_read = r#"{"tool_name":"Read","tool_input":{"file_path":"/tmp/x"#;
    let r_read = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool"],
        truncated_read,
    );
    assert_eq!(
        r_read.code, 0,
        "truncated read payload must fail open: {}",
        r_read.stderr
    );

    let truncated_write = r#"{"tool_name":"Write","tool_input":{"file_path":"/tmp/x"#;
    let r_write = hook(
        &repo,
        &["--agent", "claude-code", "--event", "pre-tool"],
        truncated_write,
    );
    assert_eq!(r_write.code, 2, "truncated write payload must fail closed");
}

/// A repository whose base ref enables `[hooks.verified-stop]` in refuse mode with
/// `toml_extra` appended to its table, and an unverified change to `src/lib.rs`.
fn verified_stop_repo(toml_extra: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        &format!("[meta]\nversion = 1\nname = \"t\"\n\n[hooks.verified-stop]\nenabled = true\nmode = \"refuse\"\n{toml_extra}"),
        "chore: enable verified-stop",
    );
    repo.commit_base("src/lib.rs", "pub fn f() -> i32 { 1 }\n", "feat: init");
    repo.write("src/lib.rs", "pub fn f() -> i32 { 2 }\n");
    repo
}

/// Audit C1: the post-tool outcome is read from the shapes the agents send. Codex
/// 0.162.1 sends `tool_response` as a string opening `Exit code: N` (recorded; see the
/// PostToolUse payload in `src/hook.rs`'s `payload_reads_the_final_message_...` test),
/// and Qwen Code puts its shell result in `tool_response.llmContent` with an
/// `Exit Code: N` line (its `run_shell_command` format). A failing run in either shape,
/// and a payload that carries no exit code at all, never count as a passing verification.
#[test]
fn verified_stop_reads_the_codex_and_qwen_shell_outcomes() {
    let repo = verified_stop_repo("commands = [\"cargo test\"]\n");
    let cwd = repo.path().display().to_string();
    let failing = "running 2 tests\ntest tests::a ... ok\ntest tests::b ... FAILED\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n";
    let passing = "running 2 tests\ntest result: ok. 2 passed; 0 failed; 0 ignored\n";
    let codex = |session: &str, response: Value| {
        json!({
            "cwd": cwd, "hook_event_name": "PostToolUse", "model": "m",
            "permission_mode": "bypassPermissions", "session_id": session,
            "tool_input": { "command": "cargo test" }, "tool_name": "Bash",
            "tool_response": response, "tool_use_id": "u-1", "transcript_path": "/t",
            "turn_id": "t-1"
        })
        .to_string()
    };
    let qwen = |session: &str, code: &str, out: &str| {
        json!({
            "cwd": cwd, "hook_event_name": "PostToolUse", "permission_mode": "yolo",
            "session_id": session, "tool_name": "run_shell_command",
            "tool_input": { "command": "cargo test", "description": "Run the tests" },
            "tool_response": { "llmContent": format!(
                "Command: cargo test\nDirectory: (root)\nOutput: {out}\nError: (none)\nExit Code: {code}\nSignal: (none)\nBackground PIDs: (none)\nProcess Group PGID: 4242"
            ) },
            "tool_use_id": "u-2", "transcript_path": "/t"
        })
        .to_string()
    };
    let cases: Vec<(&str, &str, &str, String, i32)> = vec![
        (
            "codex fails",
            "codex",
            "codex/stop.json",
            codex(
                "c-fail",
                Value::from(format!(
                    "Exit code: 101\nWall time: 2.1 seconds\nOutput:\n{failing}"
                )),
            ),
            2,
        ),
        (
            "codex fails, its output prints a passing exit code",
            "codex",
            "codex/stop.json",
            codex(
                "c-fake",
                Value::from(format!(
                    "Exit code: 101\nWall time: 2.1 seconds\nOutput:\nExit code: 0\n{passing}"
                )),
            ),
            2,
        ),
        (
            "codex carries no exit code",
            "codex",
            "codex/stop.json",
            codex("c-unknown", json!({ "stdout": passing })),
            2,
        ),
        (
            "codex passes",
            "codex",
            "codex/stop.json",
            codex(
                "c-pass",
                Value::from(format!(
                    "Exit code: 0\nWall time: 2.1 seconds\nOutput:\n{passing}"
                )),
            ),
            0,
        ),
        (
            "qwen fails",
            "qwen",
            "qwen/stop.json",
            qwen("q-fail", "101", failing),
            2,
        ),
        (
            "qwen passes",
            "qwen",
            "qwen/stop.json",
            qwen("q-pass", "0", passing),
            0,
        ),
        (
            "opencode 2.x fails",
            "opencode",
            "opencode/turn_end_events.json",
            recorded_pretool("opencode/shell_post_fail_v2.json", |v| {
                v["cwd"] = cwd.clone().into();
                v["input"]["sessionID"] = "oc-fail".into();
            }),
            2,
        ),
        (
            "opencode 2.x passes",
            "opencode",
            "opencode/turn_end_events.json",
            recorded_pretool("opencode/shell_post_pass_v2.json", |v| {
                v["cwd"] = cwd.clone().into();
                v["input"]["sessionID"] = "oc-pass".into();
            }),
            0,
        ),
        (
            "agy post-tool carries no exit code on pass",
            "agy",
            "agy/stop.json",
            recorded_pretool("agy/run_command_post_pass.json", |v| {
                v["workspacePaths"] = json!([cwd]);
                v["conversationId"] = "agy-pass".into();
            }),
            2,
        ),
        (
            "agy post-tool carries no exit code on fail",
            "agy",
            "agy/stop.json",
            recorded_pretool("agy/run_command_post_fail.json", |v| {
                v["workspacePaths"] = json!([cwd]);
                v["conversationId"] = "agy-fail".into();
            }),
            2,
        ),
    ];
    for (what, agent, stop, post, want) in cases {
        let post_val = serde_json::from_str::<Value>(&post).unwrap();
        let session = post_val["session_id"]
            .as_str()
            .or_else(|| post_val.pointer("/input/sessionID").and_then(Value::as_str))
            .or_else(|| post_val["conversationId"].as_str())
            .unwrap()
            .to_string();
        let w = hook(&repo, &["--agent", agent, "--event", "post-tool"], &post);
        assert_eq!(w.code, 0, "{what}: post-tool never blocks: {}", w.stderr);
        let stop_payload = match agent {
            "opencode" => json!({
                "cwd": cwd,
                "session_id": session,
                "last_assistant_message": "All done."
            })
            .to_string(),
            "agy" => recorded_stop(stop, |v| {
                v["workspacePaths"] = json!([cwd]);
                v["conversationId"] = session.clone().into();
                v["last_assistant_message"] = "All done.".into();
            }),
            _ => recorded_stop(stop, |v| {
                v["cwd"] = cwd.clone().into();
                v["session_id"] = session.clone().into();
                v["last_assistant_message"] = "All done.".into();
            }),
        };
        let r = if agent == "opencode" {
            hook(&repo, &["--agent", agent, "--event", "stop"], &stop_payload)
        } else {
            hook(&repo, &["--agent", agent], &stop_payload)
        };
        let failed = if agent == "opencode" {
            r.stdout.contains("\"decision\":\"block\"")
        } else if agent == "agy" {
            r.stdout.contains("\"decision\":\"continue\"")
        } else {
            r.code == 2
        };
        assert_eq!(
            if failed { 2 } else { 0 },
            want,
            "{what}: code={} stdout={} stderr={}",
            r.code,
            r.stdout,
            r.stderr
        );
        if want == 2 {
            let msg = format!("{}{}", r.stdout, r.stderr);
            assert!(
                msg.contains("[turn/unverified-dirty-tree]"),
                "{what}: {msg}"
            );
        }
    }
}

/// Audit M2: in refuse mode a stop whose refusal cannot be counted (the session lock is
/// held, here by a file the agent could create itself, or the payload names no session)
/// is refused, not let through. The agent's own loop guard still bounds it, and observe
/// mode only logs.
#[test]
fn verified_stop_refuses_a_stop_whose_refusal_cannot_be_recorded() {
    let repo = verified_stop_repo("commands = [\"cargo test\"]\n");
    let cwd = repo.path().display().to_string();
    let sessions = repo.file(".git/discipline/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join(".lock"), "").unwrap();

    let held = recorded_stop("claude-code/stop.json", |v| {
        v["cwd"] = cwd.clone().into();
        v["session_id"] = "sess-lock".into();
        v["last_assistant_message"] = "All done.".into();
    });
    let r = hook(&repo, &["--agent", "claude-code"], &held);
    assert_eq!(
        r.code, 2,
        "a held session lock must not let the stop through: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("[turn/unverified-dirty-tree]")
            && r.stderr.contains("could not be recorded"),
        "{}",
        r.stderr
    );

    let continued = recorded_stop("claude-code/stop_continued.json", |v| {
        v["cwd"] = cwd.clone().into();
        v["session_id"] = "sess-lock".into();
        v["last_assistant_message"] = "All done.".into();
    });
    let rc = hook(&repo, &["--agent", "claude-code"], &continued);
    assert_eq!(rc.code, 0, "the loop guard still bounds it: {}", rc.stderr);
    assert!(rc.stderr.contains("loop guard"), "{}", rc.stderr);

    let obs = hook(&repo, &["--agent", "claude-code", "--observe"], &held);
    assert_eq!(obs.code, 0, "observe mode only logs: {}", obs.stderr);
    std::fs::remove_file(sessions.join(".lock")).unwrap();

    let anonymous = recorded_stop("claude-code/stop.json", |v| {
        v["cwd"] = cwd.clone().into();
        v.as_object_mut().unwrap().remove("session_id");
        v["last_assistant_message"] = "All done.".into();
    });
    let r = hook(&repo, &["--agent", "claude-code"], &anonymous);
    assert_eq!(
        r.code, 2,
        "a stop with no session id must be refused: {}",
        r.stderr
    );
    assert!(r.stderr.contains("no session id"), "{}", r.stderr);
}

/// Audit (verified-stop defaults): with `run_at_stop = true` and no command configured,
/// only the default runners whose ecosystem marker exists are run. A repository with
/// none of them is refused with a message that says so, rather than with `cargo test`.
#[test]
fn verified_stop_run_at_stop_without_a_command_or_a_marker_names_the_gap() {
    let repo = verified_stop_repo("run_at_stop = true\n");
    let stop_payload = recorded_stop("claude-code/stop.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-defaults".into();
        v["last_assistant_message"] = "All done.".into();
    });
    let r = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(
        r.stderr.contains("no verification command applies"),
        "{}",
        r.stderr
    );
}

#[test]
fn stop_and_post_tool_hook_compact_digest_elides_repeated_warnings_until_changed_or_verbose() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[gates.ignored-tests]\nseverity = \"warning\"\n",
        "chore: configure ignored-tests as warning",
    );
    repo.commit_base(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        "feat: init",
    );
    repo.commit_base(
        "tests/t1.rs",
        "#[test]\nfn skipped_alpha() {\n    assert_eq!(1, 1);\n}\n",
        "test: t1",
    );
    repo.commit_base(
        "tests/t2.rs",
        "#[test]\nfn skipped_beta() {\n    assert_eq!(2, 2);\n}\n",
        "test: t2",
    );

    // Introduce 1 blocking error (`unsafe-safety-comment`) and 1 non-blocking warning (`ignored-tests`).
    repo.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { unsafe { a + b } }\n",
    );
    repo.write(
        "tests/t1.rs",
        "#[test]\n#[ignore]\nfn skipped_alpha() {\n    assert_eq!(1, 1);\n}\n",
    );

    let stop_payload = recorded_stop("claude-code/stop.json", |v| {
        v["cwd"] = repo.path().display().to_string().into();
        v["session_id"] = "sess-818".into();
        v["last_assistant_message"] = "Working on it.".into();
    });

    // 1. First firing in `sess-818` prints both the blocking error and the warning in full,
    // and writes the full report to `.git/discipline/sessions/sess-818.report.txt`.
    let r1 = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r1.code, 2, "{}", r1.stderr);
    assert!(
        r1.stderr
            .contains("[unsafe-safety-comment/safety-comment-missing]"),
        "missing blocking error in first report: {}",
        r1.stderr
    );
    assert!(
        r1.stderr.contains("[ignored-tests/") && r1.stderr.contains("tests/t1.rs"),
        "first firing in session must print warning in full: {}",
        r1.stderr
    );
    let report_path = repo.file(".git/discipline/sessions/sess-818.report.txt");
    assert!(
        report_path.is_file(),
        "expected full report file at {}",
        report_path.display()
    );

    // 2. Second firing in `sess-818` with unchanged findings: blocking error stays in full,
    // unchanged warning is elided into a compact digest with a pointer to the report file.
    let r2 = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r2.code, 2, "{}", r2.stderr);
    assert!(
        r2.stderr.contains(
            "Summary: 2 blocking error(s), 1 warning(s) (0 new or changed, 1 unchanged from earlier in this session)."
        ),
        "expected compact summary header on second firing: {}",
        r2.stderr
    );
    assert!(
        r2.stderr
            .contains("### Issue 1 [unsafe-safety-comment/safety-comment-missing]"),
        "blocking error must still print in full on second firing: {}",
        r2.stderr
    );
    assert!(
        !r2.stderr.contains("tests/t1.rs"),
        "unchanged warning body must be elided on second firing: {}",
        r2.stderr
    );
    assert!(
        r2.stderr
            .contains("1 unchanged non-blocking warning(s) from earlier in this session elided (ignored-tests/")
            && r2.stderr.contains("sess-818.report.txt"),
        "expected digest line and report file pointer on second firing: {}",
        r2.stderr
    );

    // 3. Introducing a second warning prints the new warning in full while keeping the first elided.
    repo.write(
        "tests/t2.rs",
        "#[test]\n#[ignore]\nfn skipped_beta() {\n    assert_eq!(2, 2);\n}\n",
    );
    let r3 = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r3.code, 2, "{}", r3.stderr);
    assert!(
        r3.stderr.contains(
            "Summary: 2 blocking error(s), 2 warning(s) (1 new or changed, 1 unchanged from earlier in this session)."
        ),
        "expected 1 new + 1 unchanged summary on third firing: {}",
        r3.stderr
    );
    assert!(
        r3.stderr.contains("tests/t2.rs") && !r3.stderr.contains("tests/t1.rs"),
        "new warning must print in full while unchanged warning stays elided: {}",
        r3.stderr
    );

    // 4. Shifting the line number of an existing warning (inserting lines above it) keeps it elided.
    repo.write(
        "tests/t1.rs",
        "// header line 1\n// header line 2\n#[test]\n#[ignore]\nfn skipped_alpha() {\n    assert_eq!(1, 1);\n}\n",
    );
    let r_shifted = hook(&repo, &["--agent", "claude-code"], &stop_payload);
    assert_eq!(r_shifted.code, 2, "{}", r_shifted.stderr);
    assert!(
        r_shifted.stderr.contains(
            "Summary: 2 blocking error(s), 2 warning(s) (0 new or changed, 2 unchanged from earlier in this session)."
        ) && !r_shifted.stderr.contains("tests/t1.rs") && !r_shifted.stderr.contains("tests/t2.rs"),
        "line-shifted warnings must remain elided: {}",
        r_shifted.stderr
    );

    // 5. `--verbose` prints all warnings in full even when unchanged.
    let r4 = hook(
        &repo,
        &["--agent", "claude-code", "--verbose"],
        &stop_payload,
    );
    assert_eq!(r4.code, 2, "{}", r4.stderr);
    assert!(
        r4.stderr.contains("tests/t1.rs") && r4.stderr.contains("tests/t2.rs"),
        "--verbose must print all warnings in full: {}",
        r4.stderr
    );
}
