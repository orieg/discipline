//! End to end: `discipline hook run` at the end of a turn whose final message announces
//! an action (`src/turn.rs`, docs/ROADMAP.md Phase 17), driven with the stop payloads
//! recorded from live agents (`tests/fixtures/stop/`).

mod common;

use common::Repo;
use serde_json::Value;
use std::io::Write;
use std::process::Stdio;

struct HookRun {
    code: i32,
    stdout: String,
    stderr: String,
}

fn hook(repo: &Repo, args: &[&str], stdin: &str) -> HookRun {
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
    HookRun {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A recorded stop payload, with `edit` applied to it.
fn recorded(rel: &str, edit: impl FnOnce(&mut Value)) -> String {
    let path = format!("{}/tests/fixtures/stop/{rel}", env!("CARGO_MANIFEST_DIR"));
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut v);
    v.to_string()
}

fn stop(agent_dir: &str) -> String {
    recorded(&format!("{agent_dir}/stop.json"), |_| {})
}

const CODE: &str = "turn/announced-action-not-taken";
const SENTENCE: &str = "Now let me run the tests.";

/// A repository whose base branch carries `[hooks.premature-stop]` with `settings`.
fn repo_with(settings: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        &format!("[meta]\nversion = 1\nname = \"t\"\n\n[hooks.premature-stop]\n{settings}"),
        "chore: enable the end-of-turn check",
    );
    repo
}

fn observe_log(repo: &Repo) -> String {
    std::fs::read_to_string(repo.file(".git/discipline/hook-observe.log")).unwrap_or_default()
}

#[test]
fn off_by_default_and_when_only_the_change_switches_it_on() {
    // No configuration at all.
    let repo = Repo::new();
    let run = hook(&repo, &["--agent", "claude-code"], &stop("claude-code"));
    assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{}", run.stdout);

    // A table present and not enabled.
    let repo = repo_with("mode = \"refuse\"\n");
    let run = hook(&repo, &["--agent", "claude-code"], &stop("claude-code"));
    assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{}", run.stdout);

    // Enabled in the change's own copy only: the base ref's configuration is the policy.
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n\n[hooks.premature-stop]\nenabled = true\nmode = \"refuse\"\n",
    );
    let run = hook(&repo, &["--agent", "claude-code"], &stop("claude-code"));
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert!(!run.stderr.contains(CODE), "{}", run.stderr);
}

#[test]
fn refuse_mode_refuses_the_recorded_stop_once_in_each_agents_shape() {
    for (agent, dir) in [
        ("claude-code", "claude-code"),
        ("qwen", "qwen"),
        ("codex", "codex"),
    ] {
        let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
        let run = hook(&repo, &["--agent", agent], &stop(dir));
        assert_eq!(run.code, 2, "{agent}: {}\n{}", run.stdout, run.stderr);
        assert_eq!(run.stdout, "", "{agent}");
        assert!(
            run.stderr.contains(&format!("[{CODE}]"))
                && run.stderr.contains(&format!("`{SENTENCE}`"))
                && run
                    .stderr
                    .contains("Do it now, or say in one sentence why you cannot."),
            "{agent}: {}",
            run.stderr
        );

        // The recorded continuation: the model answered, and the agent marks the stop.
        let answered = recorded(&format!("{dir}/stop_continued.json"), |_| {});
        let run = hook(&repo, &["--agent", agent], &answered);
        assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{agent}");

        // A continuation that announces again is let through at the agent's loop guard,
        // and said.
        let again = recorded(&format!("{dir}/stop_continued.json"), |v| {
            v["last_assistant_message"] = SENTENCE.into();
        });
        let run = hook(&repo, &["--agent", agent], &again);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        assert!(
            run.stderr.contains("loop guard") && run.stderr.contains(CODE),
            "{agent}: {}",
            run.stderr
        );
        assert!(!run.stderr.contains(SENTENCE), "{agent}: {}", run.stderr);
    }
}

#[test]
fn a_session_at_its_cap_is_let_through() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\nmax_per_session = 1\n");
    let payload = recorded("claude-code/stop.json", |v| {
        v["session_id"] = "session-a".into();
    });
    let first = hook(&repo, &["--agent", "claude-code"], &payload);
    assert_eq!(first.code, 2, "{}", first.stderr);
    assert_eq!(
        std::fs::read_to_string(repo.file(".git/discipline/turn-session-a")).unwrap(),
        "1"
    );
    // The next turn of the same session ends the same way.
    let second = hook(&repo, &["--agent", "claude-code"], &payload);
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert!(
        second.stderr.contains("max_per_session") && second.stderr.contains(CODE),
        "{}",
        second.stderr
    );
    // Another session has its own count.
    let other = recorded("claude-code/stop.json", |v| {
        v["session_id"] = "session-b".into();
    });
    assert_eq!(hook(&repo, &["--agent", "claude-code"], &other).code, 2);

    // A cap of zero never refuses.
    let repo = repo_with("enabled = true\nmode = \"refuse\"\nmax_per_session = 0\n");
    assert_eq!(hook(&repo, &["--agent", "claude-code"], &payload).code, 0);
}

#[test]
fn observe_mode_logs_the_code_and_never_the_message() {
    for (settings, flags) in [
        ("enabled = true\n", &["--agent", "claude-code"][..]),
        (
            "enabled = true\nmode = \"refuse\"\n",
            &["--agent", "claude-code", "--observe"][..],
        ),
    ] {
        let repo = repo_with(settings);
        let run = hook(&repo, flags, &stop("claude-code"));
        assert_eq!(run.code, 0, "{settings}: {}", run.stderr);
        assert!(
            run.stderr.contains("observe mode, not enforced") && run.stderr.contains(CODE),
            "{settings}: {}",
            run.stderr
        );
        let log = observe_log(&repo);
        let entry: Value = serde_json::from_str(log.lines().next().expect("one log line")).unwrap();
        assert_eq!(entry["agent"], "claude-code");
        assert_eq!(entry["event"], "stop");
        assert_eq!(entry["verdict"], "premature-stop");
        assert_eq!(entry["codes"], serde_json::json!([CODE]));
        assert!(
            !log.contains("run the tests") && !run.stderr.contains("run the tests"),
            "the message reached the log or stderr: {log}\n{}",
            run.stderr
        );
        assert!(!repo
            .file(".git/discipline")
            .join("turn-00000000-0000-0000-0000-000000000000")
            .exists());
    }
}

#[test]
fn a_turn_that_handed_over_or_finished_is_not_judged() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    for (why, payload) in [
        (
            "background work is listed",
            recorded("claude-code/stop.json", |v| {
                v["background_tasks"] = serde_json::json!([{"id": "t1"}]);
            }),
        ),
        (
            "a scheduled wake-up is listed",
            recorded("qwen/stop.json", |v| {
                v["crons"] = serde_json::json!([{"id": "c1"}]);
            }),
        ),
        (
            "the message is a question",
            recorded("claude-code/stop.json", |v| {
                v["last_assistant_message"] = "Should I run the tests now?".into();
            }),
        ),
        (
            "the message reports finished work",
            recorded("claude-code/stop.json", |v| {
                v["last_assistant_message"] = "All 12 tests pass.".into();
            }),
        ),
        (
            "the payload has no message",
            recorded("claude-code/stop.json", |v| {
                v.as_object_mut().unwrap().remove("last_assistant_message");
            }),
        ),
    ] {
        let run = hook(&repo, &["--agent", "claude-code"], &payload);
        assert_eq!((run.code, run.stderr.as_str()), (0, ""), "{why}");
    }
    // An agent whose payload carries no message is not judged from another field.
    for (agent, dir) in [("copilot", "copilot"), ("agy", "agy")] {
        let payload = recorded(&format!("{dir}/stop.json"), |v| {
            v["last_assistant_message"] = SENTENCE.into();
        });
        let run = hook(&repo, &["--agent", agent], &payload);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        assert!(
            !run.stdout.contains(CODE) && !run.stderr.contains(CODE),
            "{agent}: {}\n{}",
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn a_tool_call_written_as_text_is_refused_unless_switched_off() {
    let written = recorded("claude-code/stop.json", |v| {
        v["last_assistant_message"] =
            "I have a syntax error.\n</parameter>\n</function>\n</tool_call>".into();
    });
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    let run = hook(&repo, &["--agent", "claude-code"], &written);
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(
        run.stderr.contains("[turn/tool-call-written-as-text]"),
        "{}",
        run.stderr
    );
    let repo = repo_with("enabled = true\nmode = \"refuse\"\ntool_call_as_text = false\n");
    let run = hook(&repo, &["--agent", "claude-code"], &written);
    assert_eq!((run.code, run.stderr.as_str()), (0, ""));
}

#[test]
fn a_change_with_findings_is_answered_by_its_report_not_by_the_turn() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    let run = hook(&repo, &["--agent", "claude-code"], &stop("claude-code"));
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("[assertion-reduction/assertions-reduced]")
            && !run.stderr.contains(CODE),
        "{}",
        run.stderr
    );
}

#[test]
fn refuse_mode_refuses_copilot_transcript_and_handles_continuation() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    let transcript = format!(
        "{}/tests/fixtures/stop/copilot/transcript_last_message.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let payload = recorded("copilot/stop.json", |v| {
        v["transcriptPath"] = transcript.clone().into();
    });
    let run = hook(&repo, &["--agent", "copilot"], &payload);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "");
    let decision: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(decision["decision"], "block");
    let reason = decision["reason"].as_str().unwrap();
    assert!(
        reason.contains(&format!("[{CODE}]"))
            && reason.contains(&format!("`{SENTENCE}`"))
            && reason.contains("Do it now, or say in one sentence why you cannot."),
        "reason: {reason}"
    );

    // Continuation: stop_hook_active = true lets through at loop guard
    let continued = recorded("copilot/stop_continued.json", |v| {
        v["transcriptPath"] = transcript.into();
    });
    let run = hook(&repo, &["--agent", "copilot"], &continued);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr.contains("loop guard") && run.stderr.contains(CODE),
        "stderr: {}",
        run.stderr
    );

    // Directory resolution: passing the directory resolves transcript_last_message.json
    let dir = format!("{}/tests/fixtures/stop/copilot", env!("CARGO_MANIFEST_DIR"));
    let dir_payload = recorded("copilot/stop.json", |v| {
        v["transcriptPath"] = dir.into();
    });
    let dir_run = hook(&repo, &["--agent", "copilot"], &dir_payload);
    assert_eq!(dir_run.code, 0, "{}", dir_run.stderr);
    let dir_decision: Value = serde_json::from_str(&dir_run.stdout).unwrap();
    assert_eq!(dir_decision["decision"], "block");
}

#[test]
fn refuse_mode_refuses_agy_transcript_and_handles_continuation() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    let transcript = format!(
        "{}/tests/fixtures/stop/agy/transcript_last_message.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let payload = recorded("agy/stop.json", |v| {
        v["transcriptPath"] = transcript.clone().into();
    });
    let run = hook(&repo, &["--agent", "agy"], &payload);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "");
    let decision: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(decision["decision"], "continue");
    let reason = decision["reason"].as_str().unwrap();
    assert!(
        reason.contains(&format!("[{CODE}]"))
            && reason.contains(&format!("`{SENTENCE}`"))
            && reason.contains("Do it now, or say in one sentence why you cannot."),
        "reason: {reason}"
    );

    // Continuation: executionNum = 1 lets through at loop guard
    let continued = recorded("agy/stop_continued.json", |v| {
        v["transcriptPath"] = transcript.into();
    });
    let run = hook(&repo, &["--agent", "agy"], &continued);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout.trim(), "{}");
    assert!(
        run.stderr.contains("loop guard") && run.stderr.contains(CODE),
        "stderr: {}",
        run.stderr
    );

    // Directory resolution: passing the directory resolves transcript_last_message.json
    let dir = format!("{}/tests/fixtures/stop/agy", env!("CARGO_MANIFEST_DIR"));
    let dir_payload = recorded("agy/stop.json", |v| {
        v["transcriptPath"] = dir.into();
    });
    let dir_run = hook(&repo, &["--agent", "agy"], &dir_payload);
    assert_eq!(dir_run.code, 0, "{}", dir_run.stderr);
    let dir_decision: Value = serde_json::from_str(&dir_run.stdout).unwrap();
    assert_eq!(dir_decision["decision"], "continue");
}

#[test]
fn observe_mode_logs_copilot_and_agy_premature_stops() {
    let repo = repo_with("enabled = true\nmode = \"observe\"\n");
    for (agent, dir) in [("copilot", "copilot"), ("agy", "agy")] {
        let transcript = format!(
            "{}/tests/fixtures/stop/{dir}/transcript_last_message.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let payload = recorded(&format!("{dir}/stop.json"), |v| {
            v["transcriptPath"] = transcript.into();
        });
        let run = hook(&repo, &["--agent", agent], &payload);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        let expected_stdout = if agent == "agy" { "{}\n" } else { "" };
        assert_eq!(run.stdout, expected_stdout, "{agent}");
        assert!(
            run.stderr.contains("observe mode, not enforced") && run.stderr.contains(CODE),
            "{agent}: {}",
            run.stderr
        );
    }
    let log = observe_log(&repo);
    let entries: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // Each stop also leaves the change check's record (#937), which passed.
    let (checks, judged): (Vec<Value>, Vec<Value>) =
        entries.into_iter().partition(|e| e.get("record").is_some());
    assert_eq!(checks.len(), 2, "{log}");
    for c in &checks {
        assert_eq!(
            (&c["event"], &c["verdict"], &c["decision"]),
            (&"stop".into(), &"pass".into(), &"allowed".into()),
            "{c}"
        );
    }
    let agents: Vec<String> = judged
        .iter()
        .map(|entry| {
            assert_eq!(entry["event"], "stop");
            assert_eq!(entry["verdict"], "premature-stop");
            assert_eq!(entry["codes"], serde_json::json!([CODE]));
            entry["agent"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(agents, vec!["copilot", "agy"]);
}

#[test]
fn copilot_and_agy_tool_call_in_transcript_lets_stop_through() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");

    // Agy with tool_calls in transcript
    let agy_tool_transcript = format!(
        "{}/tests/fixtures/stop/agy/transcript_tool_call_message.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let payload = recorded("agy/stop.json", |v| {
        v["transcriptPath"] = agy_tool_transcript.into();
    });
    let run = hook(&repo, &["--agent", "agy"], &payload);
    assert_eq!(
        (run.code, run.stdout.trim(), run.stderr.as_str()),
        (0, "{}", "")
    );

    // Copilot with toolRequests in transcript
    let copilot_tmp = repo.path().join("copilot_tool_transcript.json");
    std::fs::write(
        &copilot_tmp,
        serde_json::json!({
            "type": "assistant.message",
            "data": {
                "content": SENTENCE,
                "toolRequests": [{"name": "run_command"}]
            }
        })
        .to_string(),
    )
    .unwrap();
    let payload = recorded("copilot/stop.json", |v| {
        v["transcriptPath"] = copilot_tmp.to_str().unwrap().into();
    });
    let run = hook(&repo, &["--agent", "copilot"], &payload);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (0, "", "")
    );
}

#[test]
fn copilot_and_agy_missing_or_unreadable_transcript_refused_in_refuse_mode() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");

    // Missing transcriptPath in payload fails closed
    for (agent, dir) in [("copilot", "copilot"), ("agy", "agy")] {
        let payload = recorded(&format!("{dir}/stop.json"), |v| {
            v.as_object_mut().unwrap().remove("transcriptPath");
        });
        let run = hook(&repo, &["--agent", agent], &payload);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        let expected_decision = if agent == "copilot" {
            "\"decision\":\"block\""
        } else {
            "\"decision\":\"continue\""
        };
        assert!(
            run.stdout.contains(expected_decision),
            "{agent} expected refusal decision in stdout: {}",
            run.stdout
        );
        assert!(
            run.stdout.contains("no transcript path in stop payload"),
            "{agent} expected reason in stdout: {}",
            run.stdout
        );
    }

    // Non-existent transcriptPath fails closed
    for (agent, dir) in [("copilot", "copilot"), ("agy", "agy")] {
        let payload = recorded(&format!("{dir}/stop.json"), |v| {
            v["transcriptPath"] = "/path/that/does/not/exist/transcript.json".into();
        });
        let run = hook(&repo, &["--agent", agent], &payload);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        let expected_decision = if agent == "copilot" {
            "\"decision\":\"block\""
        } else {
            "\"decision\":\"continue\""
        };
        assert!(
            run.stdout.contains(expected_decision),
            "{agent} expected refusal decision in stdout: {}",
            run.stdout
        );
        assert!(
            run.stdout
                .contains("could not read transcript '/path/that/does/not/exist/transcript.json'"),
            "{agent} expected reason in stdout: {}",
            run.stdout
        );
    }
}

#[test]
fn copilot_and_agy_missing_or_unreadable_transcript_skipped_in_observe_mode() {
    let repo = repo_with("enabled = true\nmode = \"observe\"\n");

    for (agent, dir) in [("copilot", "copilot"), ("agy", "agy")] {
        let payload = recorded(&format!("{dir}/stop.json"), |v| {
            v.as_object_mut().unwrap().remove("transcriptPath");
        });
        let run = hook(&repo, &["--agent", agent], &payload);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        let expected_stdout = if agent == "agy" { "{}\n" } else { "" };
        assert_eq!(run.stdout, expected_stdout, "{agent}");
        assert!(
            run.stderr
                .contains("no transcript path in stop payload; premature stop check skipped"),
            "{agent}: {}",
            run.stderr
        );
    }

    // Non-existent transcriptPath in observe mode
    for (agent, dir) in [("copilot", "copilot"), ("agy", "agy")] {
        let payload = recorded(&format!("{dir}/stop.json"), |v| {
            v["transcriptPath"] = "/path/that/does/not/exist/transcript.json".into();
        });
        let run = hook(&repo, &["--agent", agent], &payload);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        let expected_stdout = if agent == "agy" { "{}\n" } else { "" };
        assert_eq!(run.stdout, expected_stdout, "{agent}");
        assert!(
            run.stderr
                .contains("could not read transcript '/path/that/does/not/exist/transcript.json'"),
            "{agent}: {}",
            run.stderr
        );
    }
}

/// The final message of the turn recorded under OpenCode 2.0.28's background service
/// (`tests/fixtures/stop/opencode/turn_events_v2.json`): the last `session.text.ended`
/// before the first `session.execution.succeeded`.
fn opencode_recorded_text() -> String {
    let events: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/stop/opencode/turn_events_v2.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let service = events["service"].as_array().unwrap();
    let end = service
        .iter()
        .position(|e| e["type"] == "session.execution.succeeded")
        .unwrap();
    service[..end]
        .iter()
        .rev()
        .find(|e| e["type"] == "session.text.ended")
        .and_then(|e| e["data"]["text"].as_str())
        .unwrap()
        .to_string()
}

/// What the generated OpenCode plugin sends `hook run --event stop` at the end of a turn
/// (the node-driven tests in `tests/test_agent_hooks.rs` pin that it does).
fn opencode_stop(
    repo: &Repo,
    session: &str,
    text: Option<&str>,
    edit: impl FnOnce(&mut Value),
) -> String {
    let mut v = serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": session,
        "tool_call_followed": false,
        "standalone": false,
        "cwd": repo.path(),
    });
    if let Some(t) = text {
        v["last_assistant_message"] = t.into();
    }
    edit(&mut v);
    v.to_string()
}

const OPENCODE_STOP: [&str; 4] = ["--agent", "opencode", "--event", "stop"];

/// The refusal is `{"decision": "block", "reason"}` with exit 0, which the plugin sends
/// back to the model; the continuation that follows is let through at the marker this hook
/// keeps, since OpenCode marks none; the next turn is judged again, up to the cap.
#[test]
fn opencode_refuses_once_per_turn_as_a_block_answer_up_to_the_cap() {
    let text = opencode_recorded_text();
    assert_eq!(text, SENTENCE);
    let repo = repo_with("enabled = true\nmode = \"refuse\"\nmax_per_session = 2\n");
    let session = "ses_0000000000000000000000000";
    let payload = opencode_stop(&repo, session, Some(&text), |_| {});
    let marker = repo.file(&format!(".git/discipline/opencode-stop-{session}"));

    let first = hook(&repo, &OPENCODE_STOP, &payload);
    assert_eq!(
        (first.code, first.stderr.as_str()),
        (0, ""),
        "{}",
        first.stdout
    );
    let answer: Value = serde_json::from_str(first.stdout.trim()).unwrap();
    assert_eq!(answer["decision"], "block", "{answer}");
    let reason = answer["reason"].as_str().unwrap();
    assert!(
        reason.contains(&format!("[{CODE}]")) && reason.contains(&format!("`{SENTENCE}`")),
        "{reason}"
    );
    assert!(marker.exists());

    // The continuation announces again: let through at the marker, which it removes.
    let continued = hook(&repo, &OPENCODE_STOP, &payload);
    assert_eq!(
        (continued.code, continued.stdout.as_str()),
        (0, ""),
        "{}",
        continued.stderr
    );
    assert!(
        continued.stderr.contains("loop guard") && continued.stderr.contains(CODE),
        "{}",
        continued.stderr
    );
    assert!(!continued.stderr.contains(SENTENCE), "{}", continued.stderr);
    assert!(!marker.exists());

    // The next turn is judged again, and refused: the second of the cap of two.
    let next = hook(&repo, &OPENCODE_STOP, &payload);
    assert!(
        next.stdout.contains("\"block\""),
        "{}\n{}",
        next.stdout,
        next.stderr
    );
    let _ = hook(&repo, &OPENCODE_STOP, &payload);
    // The session is at its cap.
    let capped = hook(&repo, &OPENCODE_STOP, &payload);
    assert_eq!(
        (capped.code, capped.stdout.as_str()),
        (0, ""),
        "{}",
        capped.stderr
    );
    assert!(
        capped.stderr.contains("max_per_session"),
        "{}",
        capped.stderr
    );
}

/// Off unless the base ref enables an end-of-turn check; observe mode logs and never
/// answers with a block; a turn whose final message a tool call followed, or that has
/// no message, is not judged.
#[test]
fn opencode_stop_is_off_by_default_logs_in_observe_mode_and_skips_handed_on_turns() {
    let repo = Repo::new();
    let payload = opencode_stop(&repo, "s-1", Some(SENTENCE), |_| {});
    let run = hook(&repo, &OPENCODE_STOP, &payload);
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (0, "", "")
    );

    for (settings, observe) in [
        ("enabled = true\n", false),
        ("enabled = true\nmode = \"refuse\"\n", true),
    ] {
        let repo = repo_with(settings);
        let mut args = OPENCODE_STOP.to_vec();
        if observe {
            args.push("--observe");
        }
        let run = hook(
            &repo,
            &args,
            &opencode_stop(&repo, "s-1", Some(SENTENCE), |_| {}),
        );
        assert_eq!((run.code, run.stdout.as_str()), (0, ""), "{settings}");
        assert!(
            run.stderr.contains("observe mode, not enforced") && run.stderr.contains(CODE),
            "{}",
            run.stderr
        );
        let entry: Value =
            serde_json::from_str(observe_log(&repo).lines().next().unwrap()).unwrap();
        assert_eq!(
            (entry["agent"].as_str(), entry["verdict"].as_str()),
            (Some("opencode"), Some("premature-stop"))
        );
        assert!(!repo.file(".git/discipline/opencode-stop-s-1").exists());
    }

    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    for (why, payload) in [
        (
            "a tool call followed",
            opencode_stop(&repo, "s-2", Some(SENTENCE), |v| {
                v["tool_call_followed"] = true.into()
            }),
        ),
        ("no message", opencode_stop(&repo, "s-2", None, |_| {})),
        (
            "finished work",
            opencode_stop(&repo, "s-2", Some("All 12 tests pass."), |_| {}),
        ),
    ] {
        let run = hook(&repo, &OPENCODE_STOP, &payload);
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (0, "", ""),
            "{why}"
        );
    }
}

/// Under `opencode run --standalone` the process exits before a continuation runs: the
/// check is skipped and the skip logged, where an end-of-turn check is enabled.
#[test]
fn opencode_standalone_turn_is_skipped_and_logged() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    let payload = opencode_stop(&repo, "s-3", Some(SENTENCE), |v| {
        v["standalone"] = true.into()
    });
    let run = hook(&repo, &OPENCODE_STOP, &payload);
    assert_eq!((run.code, run.stdout.as_str()), (0, ""), "{}", run.stderr);
    assert!(
        run.stderr.contains("skipped") && run.stderr.contains("--standalone"),
        "{}",
        run.stderr
    );
    let entry: Value = serde_json::from_str(observe_log(&repo).lines().next().unwrap()).unwrap();
    assert_eq!(entry["event"], "stop");
    assert_eq!(entry["verdict"], "skipped");
    assert_eq!(entry["reason"], "standalone");
    assert!(!repo.file(".git/discipline/turn-s-3").exists());

    // Nothing enabled: nothing to skip, nothing logged.
    let repo = Repo::new();
    let run = hook(
        &repo,
        &OPENCODE_STOP,
        &opencode_stop(&repo, "s-3", Some(SENTENCE), |v| {
            v["standalone"] = true.into()
        }),
    );
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (0, "", "")
    );
    assert_eq!(observe_log(&repo), "");
}

/// The change check runs first, as for every agent, once an end-of-turn check is
/// enabled: its report is the block. With none enabled the stop does not run it (the
/// plugin already checks after every edit), where Claude Code's stop does.
#[test]
fn opencode_stop_answers_a_change_with_findings_only_when_a_turn_check_is_enabled() {
    let weakened = "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n";
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");
    repo.write("tests/a.rs", weakened);
    let run = hook(
        &repo,
        &OPENCODE_STOP,
        &opencode_stop(&repo, "s-4", Some("All done."), |_| {}),
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    let answer: Value = serde_json::from_str(run.stdout.trim()).unwrap();
    assert_eq!(answer["decision"], "block");
    assert!(
        answer["reason"]
            .as_str()
            .unwrap()
            .contains("[assertion-reduction/assertions-reduced]"),
        "{answer}"
    );

    let repo = repo_with("enabled = false\n");
    repo.write("tests/a.rs", weakened);
    let run = hook(
        &repo,
        &OPENCODE_STOP,
        &opencode_stop(&repo, "s-4", Some("All done."), |_| {}),
    );
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (0, "", "")
    );
    let control = hook(&repo, &["--agent", "claude-code"], &stop("claude-code"));
    assert_eq!(control.code, 2, "{}", control.stderr);
}
