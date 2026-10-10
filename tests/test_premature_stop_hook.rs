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
        // Codex sends Claude Code's payload (READ: its hook input schema).
        ("codex", "claude-code"),
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
    let agents: Vec<String> = log
        .lines()
        .map(|line| {
            let entry: Value = serde_json::from_str(line).unwrap();
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
fn copilot_and_agy_missing_or_unreadable_transcript_skipped_with_warning() {
    let repo = repo_with("enabled = true\nmode = \"refuse\"\n");

    // Missing transcriptPath in payload
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

    // Non-existent transcriptPath
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
