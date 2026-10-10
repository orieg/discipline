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

/// Claude Code and Qwen Code hand the hook the final message itself, with the flag that
/// marks a stop a hook already continued and the session's background work.
#[test]
fn claude_code_and_qwen_payloads_carry_the_last_message_and_the_loop_flag() {
    for agent in ["claude-code", "qwen"] {
        let first = fixture(&format!("{agent}/stop.json"));
        assert_eq!(text(&first, "/hook_event_name"), "Stop", "{agent}");
        assert_eq!(
            text(&first, "/last_assistant_message"),
            ANNOUNCED,
            "{agent}"
        );
        assert_eq!(first["stop_hook_active"], false, "{agent}");
        assert!(first["background_tasks"].is_array(), "{agent}");
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
