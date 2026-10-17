//! Each agent's end-of-turn hook payload and the answer that refused the stop, recorded
//! from a live run (docs/ROADMAP.md, Phase 17 Step 0). These tests pin the fields a
//! stop-time check reads (`src/transcript.rs`, Phase 17 Step 2b).

use serde_json::Value;

fn fixture(rel: &str) -> Value {
    let path = format!("{}/tests/fixtures/stop/{rel}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no string at {pointer} in {v:#}"))
}

const ANNOUNCED: &str = "Now let me run the tests.";

/// Claude Code, Qwen Code and Codex CLI hand the hook the final message itself, with the
/// flag that marks a stop a hook already continued; Claude Code and Qwen Code also list
/// the session's background work, while Codex CLI names the turn id.
#[test]
fn claude_code_and_qwen_payloads_carry_the_last_message_and_the_loop_flag() {
    for agent in ["claude-code", "qwen", "codex"] {
        let first = fixture(&format!("{agent}/stop.json"));
        assert_eq!(text(&first, "/hook_event_name"), "Stop", "{agent}");
        assert_eq!(
            text(&first, "/last_assistant_message"),
            ANNOUNCED,
            "{agent}"
        );
        assert_eq!(first["stop_hook_active"], false, "{agent}");
        if agent == "codex" {
            assert!(first.get("background_tasks").is_none(), "{agent}");
            text(&first, "/turn_id");
        } else {
            assert!(first["background_tasks"].is_array(), "{agent}");
        }
        text(&first, "/transcript_path");
        text(&first, "/session_id");

        let continued = fixture(&format!("{agent}/stop_continued.json"));
        assert_eq!(continued["stop_hook_active"], true, "{agent}");
        assert_ne!(
            text(&continued, "/last_assistant_message"),
            ANNOUNCED,
            "{agent}: the continued turn ends on the model's answer to the refusal"
        );

        let refuse = fixture(&format!("{agent}/refuse.json"));
        assert_eq!(refuse["exit_code"], 2, "{agent}");
        text(&refuse, "/stderr");
    }
}

/// Copilot CLI names the transcript, not the message; the message and its tool requests
/// are the transcript's last `assistant.message` event.
#[test]
fn copilot_payload_names_a_transcript_whose_last_message_lists_its_tool_requests() {
    let first = fixture("copilot/stop.json");
    assert_eq!(text(&first, "/stopReason"), "end_turn");
    assert_eq!(first["stop_hook_active"], false);
    assert!(first.get("last_assistant_message").is_none(), "{first:#}");
    text(&first, "/transcriptPath");
    text(&first, "/sessionId");
    assert_eq!(
        fixture("copilot/stop_continued.json")["stop_hook_active"],
        true
    );

    let last = fixture("copilot/transcript_last_message.json");
    assert_eq!(text(&last, "/type"), "assistant.message");
    assert_eq!(text(&last, "/data/content"), ANNOUNCED);
    assert_eq!(last["data"]["toolRequests"], serde_json::json!([]));

    let refuse = fixture("copilot/refuse.json");
    assert_eq!(text(&refuse, "/decision"), "block");
    text(&refuse, "/reason");
}

/// agy names the transcript and counts the executions of the conversation; a model step
/// that called a tool carries `tool_calls`, the final one carries `content` only.
#[test]
fn agy_payload_names_a_transcript_and_counts_executions() {
    let first = fixture("agy/stop.json");
    assert_eq!(text(&first, "/terminationReason"), "NO_TOOL_CALL");
    assert_eq!(first["executionNum"], 0);
    assert!(first.get("last_assistant_message").is_none(), "{first:#}");
    text(&first, "/transcriptPath");
    text(&first, "/conversationId");
    assert_eq!(fixture("agy/stop_continued.json")["executionNum"], 1);

    let last = fixture("agy/transcript_last_message.json");
    assert_eq!(text(&last, "/type"), "PLANNER_RESPONSE");
    assert_eq!(text(&last, "/content"), ANNOUNCED);
    assert!(last.get("tool_calls").is_none(), "{last:#}");
    let called = fixture("agy/transcript_tool_call_message.json");
    assert_eq!(text(&called, "/tool_calls/0/name"), "run_command");

    let refuse = fixture("agy/refuse.json");
    assert_eq!(text(&refuse, "/decision"), "continue");
    text(&refuse, "/reason");
}

/// OpenCode 2.x has no stop hook: a plugin reads the event stream, where a turn ends with
/// the final text, a step whose `finish` is `stop`, then `session.execution.succeeded`.
#[test]
fn opencode_turn_ends_with_text_a_stop_step_and_execution_succeeded() {
    let events = fixture("opencode/turn_end_events.json");
    let types: Vec<&str> = events
        .as_array()
        .expect("an array of events")
        .iter()
        .map(|e| text(e, "/type"))
        .collect();
    assert_eq!(
        types,
        [
            "session.tool.called",
            "session.step.ended",
            "session.text.ended",
            "session.step.ended",
            "session.execution.succeeded"
        ]
    );
    assert_eq!(text(&events[1], "/data/finish"), "tool-calls");
    assert_eq!(text(&events[2], "/data/text"), ANNOUNCED);
    assert_eq!(text(&events[3], "/data/finish"), "stop");
    text(&events[4], "/data/sessionID");

    let reprompt = fixture("opencode/reprompt.json");
    text(&reprompt, "/call/sessionID");
    assert_eq!(
        text(&reprompt, "/call/text"),
        text(&reprompt, "/result/payload/text")
    );
    assert_eq!(text(&reprompt, "/result/type"), "user");
}

/// OpenCode 2.0.28 (RUN 2026-10-16, Phase 17 Step 3): the events the plugin's end-of-turn
/// handler reads, once under the background service (a refused turn, the synthetic
/// message that refused it, the continuation the model answered it with) and once under
/// `opencode run --standalone` (the process shut down after the turn); the result of
/// `session.synthetic`; and the plugin process's arguments in each.
#[test]
fn opencode_2028_turns_under_the_service_and_standalone() {
    let events = fixture("opencode/turn_events_v2.json");
    let kinds = |run: &str| -> Vec<String> {
        events[run]
            .as_array()
            .expect("an array of events")
            .iter()
            .map(|e| text(e, "/type").to_string())
            .collect()
    };
    let service = kinds("service");
    let ends: Vec<usize> = service
        .iter()
        .enumerate()
        .filter(|(_, k)| *k == "session.execution.succeeded")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ends.len(), 2, "{service:?}");
    let first = &events["service"][ends[0] - 2];
    assert_eq!(text(first, "/type"), "session.text.ended");
    assert_eq!(text(first, "/data/text"), ANNOUNCED);
    assert_eq!(
        text(&events["service"][ends[0] - 1], "/data/finish"),
        "stop"
    );
    // The succeeded event names no directory; the text event does.
    assert!(events["service"][ends[0]].get("location").is_none());
    assert_eq!(text(first, "/location/directory"), "/work/repo");
    let enqueued = &events["service"][ends[0] + 1];
    assert_eq!(text(enqueued, "/type"), "session.inbox.enqueued");
    assert_eq!(text(enqueued, "/data/item/type"), "synthetic");
    assert_eq!(service[ends[0] + 2], "session.execution.started");
    assert!(text(&events["service"][ends[1] - 2], "/data/text").starts_with("This repository"));

    let standalone = kinds("standalone");
    assert_eq!(
        &standalone[standalone.len() - 2..],
        ["session.execution.succeeded", "location.shutdown"]
    );

    let synthetic = fixture("opencode/synthetic.json");
    assert_eq!(text(&synthetic, "/result/type"), "synthetic");
    assert_eq!(text(&synthetic, "/result/delivery"), "steer");
    assert_eq!(
        text(&synthetic, "/call/text"),
        text(&synthetic, "/result/payload/text")
    );

    let argv = fixture("opencode/plugin_argv.json");
    let has = |run: &str, arg: &str| {
        argv["plugin_process_argv"][run]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == arg)
    };
    assert!(has("standalone", "--stdio") && !has("service", "--stdio"));
}

#[test]
fn transcript_readers_parse_recorded_stop_fixtures() {
    use discipline::hook::Agent;
    use discipline::transcript::read_final_turn;
    use std::path::Path;

    let copilot_fixture = format!(
        "{}/tests/fixtures/stop/copilot/transcript_last_message.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let c_turn = read_final_turn(Agent::Copilot, Path::new(&copilot_fixture)).unwrap();
    assert_eq!(c_turn.message.as_deref(), Some(ANNOUNCED));
    assert!(!c_turn.tool_call_followed);

    let agy_msg_fixture = format!(
        "{}/tests/fixtures/stop/agy/transcript_last_message.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let a_turn = read_final_turn(Agent::Agy, Path::new(&agy_msg_fixture)).unwrap();
    assert_eq!(a_turn.message.as_deref(), Some(ANNOUNCED));
    assert!(!a_turn.tool_call_followed);

    let agy_tool_fixture = format!(
        "{}/tests/fixtures/stop/agy/transcript_tool_call_message.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let a_tool = read_final_turn(Agent::Agy, Path::new(&agy_tool_fixture)).unwrap();
    assert!(a_tool.tool_call_followed);
}
