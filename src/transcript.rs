//! Session and conversation transcript readers for coding agents.
//!
//! Provides structured transcript readers for agents whose end-of-turn hook payload
//! points to a transcript file rather than embedding the final assistant message directly
//! (Copilot CLI, agy; docs/ROADMAP.md Phase 17 Step 2b).

use crate::hook::Agent;
use anyhow::{bail, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The outcome of reading an agent's final turn in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnding {
    /// The text of the final assistant message, if any.
    pub message: Option<String>,
    /// Whether the final turn requested or executed a tool call.
    pub tool_call_followed: bool,
}

/// Resolves a transcript path to an existing candidate file.
///
/// If `path` is a file, returns `Some(path)`.
/// If `path` is a directory, searches for known transcript file names for the agent.
/// If `path` with `.jsonl` or `.json` exists, returns that candidate.
pub fn resolve_transcript_file(agent: Agent, path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.is_dir() {
        let candidates: &[&str] = match agent {
            Agent::Copilot => &[
                "events.jsonl",
                "transcript.jsonl",
                "transcript_last_message.json",
            ],
            Agent::Agy => &[
                "transcript_full.jsonl",
                "transcript.jsonl",
                "transcript_last_message.json",
            ],
            _ => &["transcript.jsonl", "events.jsonl"],
        };
        for name in candidates {
            let candidate = path.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let jsonl = path.with_extension("jsonl");
    if jsonl.is_file() {
        return Some(jsonl);
    }
    let json = path.with_extension("json");
    if json.is_file() {
        return Some(json);
    }
    None
}

/// Reads the final turn from a transcript for `agent`.
///
/// Yields the final assistant message and whether a tool call accompanied or followed it.
/// Fails with an error if the transcript path cannot be resolved, read, or parsed.
pub fn read_final_turn(agent: Agent, path: &Path) -> Result<TurnEnding> {
    let file = resolve_transcript_file(agent, path)
        .ok_or_else(|| anyhow::anyhow!("transcript file '{}' does not exist", path.display()))?;
    let content = std::fs::read_to_string(&file)?;
    match agent {
        Agent::Copilot => parse_copilot_transcript(&content),
        Agent::Agy => parse_agy_transcript(&content),
        _ => bail!(
            "transcript reading is not implemented for agent {}",
            agent.id()
        ),
    }
}

/// Parses Copilot CLI transcript text (single JSON object or JSON Lines).
///
/// Extracts the last `assistant.message` event, its content, and whether any
/// tool requests accompanied or followed it.
pub fn parse_copilot_transcript(content: &str) -> Result<TurnEnding> {
    // 1. Try parsing as a single JSON object.
    if let Ok(val) = serde_json::from_str::<Value>(content) {
        if val.get("type").and_then(Value::as_str) == Some("assistant.message") {
            return Ok(copilot_turn_from_value(&val));
        }
    }

    // 2. Iterate JSON Lines from the end.
    for line in content.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            if val.get("type").and_then(Value::as_str) == Some("assistant.message") {
                return Ok(copilot_turn_from_value(&val));
            }
        }
    }

    Ok(TurnEnding {
        message: None,
        tool_call_followed: false,
    })
}

fn copilot_turn_from_value(val: &Value) -> TurnEnding {
    let tool_calls = val
        .pointer("/data/toolRequests")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    let message = val
        .pointer("/data/content")
        .and_then(Value::as_str)
        .map(str::to_string);
    TurnEnding {
        message,
        tool_call_followed: tool_calls,
    }
}

/// Parses agy conversation transcript text (single JSON object or JSON Lines).
///
/// Extracts the last `PLANNER_RESPONSE` step, its content, and whether any
/// tool calls accompanied or followed it.
pub fn parse_agy_transcript(content: &str) -> Result<TurnEnding> {
    // 1. Try parsing as a single JSON object.
    if let Ok(val) = serde_json::from_str::<Value>(content) {
        if val.get("type").and_then(Value::as_str) == Some("PLANNER_RESPONSE") {
            return Ok(agy_turn_from_value(&val));
        }
    }

    // 2. Iterate JSON Lines from the end.
    for line in content.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
            if val.get("type").and_then(Value::as_str) == Some("PLANNER_RESPONSE") {
                return Ok(agy_turn_from_value(&val));
            }
        }
    }

    Ok(TurnEnding {
        message: None,
        tool_call_followed: false,
    })
}

fn agy_turn_from_value(val: &Value) -> TurnEnding {
    let tool_calls = val
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    let message = val
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    TurnEnding {
        message,
        tool_call_followed: tool_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_single_fixture_and_jsonl() {
        let fixture_path = format!(
            "{}/tests/fixtures/stop/copilot/transcript_last_message.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let turn = read_final_turn(Agent::Copilot, Path::new(&fixture_path)).unwrap();
        assert_eq!(turn.message.as_deref(), Some("Now let me run the tests."));
        assert!(!turn.tool_call_followed);

        let jsonl = r#"
{"type":"user.message","data":{"content":"hello"}}
{"type":"assistant.message","data":{"content":"First response","toolRequests":[{"name":"list"}]}}
{"type":"tool.result","data":{"output":"done"}}
{"type":"assistant.message","data":{"content":"Now let me check the result.","toolRequests":[]}}
"#;
        let parsed = parse_copilot_transcript(jsonl).unwrap();
        assert_eq!(
            parsed.message.as_deref(),
            Some("Now let me check the result.")
        );
        assert!(!parsed.tool_call_followed);

        let jsonl_with_tool = r#"
{"type":"user.message","data":{"content":"hello"}}
{"type":"assistant.message","data":{"content":"Now let me run the test.","toolRequests":[{"name":"run_test"}]}}
"#;
        let parsed_tool = parse_copilot_transcript(jsonl_with_tool).unwrap();
        assert!(parsed_tool.tool_call_followed);
    }

    #[test]
    fn agy_single_fixture_and_jsonl() {
        let fixture_msg = format!(
            "{}/tests/fixtures/stop/agy/transcript_last_message.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let turn_msg = read_final_turn(Agent::Agy, Path::new(&fixture_msg)).unwrap();
        assert_eq!(
            turn_msg.message.as_deref(),
            Some("Now let me run the tests.")
        );
        assert!(!turn_msg.tool_call_followed);

        let fixture_tool = format!(
            "{}/tests/fixtures/stop/agy/transcript_tool_call_message.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let turn_tool = read_final_turn(Agent::Agy, Path::new(&fixture_tool)).unwrap();
        assert!(turn_tool.tool_call_followed);

        let jsonl = r#"
{"type":"USER_INPUT","content":"please fix"}
{"type":"PLANNER_RESPONSE","content":"I will inspect files.","tool_calls":[{"name":"view_file"}]}
{"type":"TOOL_OUTPUT","content":"contents"}
{"type":"PLANNER_RESPONSE","content":"Now let me run the tests."}
"#;
        let parsed = parse_agy_transcript(jsonl).unwrap();
        assert_eq!(parsed.message.as_deref(), Some("Now let me run the tests."));
        assert!(!parsed.tool_call_followed);
    }

    #[test]
    fn nonexistent_transcript_returns_err() {
        let err = read_final_turn(Agent::Copilot, Path::new("/path/does/not/exist")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn directory_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let events = temp.path().join("events.jsonl");
        std::fs::write(
            &events,
            r#"{"type":"assistant.message","data":{"content":"Now let me verify.","toolRequests":[]}}"#,
        )
        .unwrap();

        let turn = read_final_turn(Agent::Copilot, temp.path()).unwrap();
        assert_eq!(turn.message.as_deref(), Some("Now let me verify."));
        assert!(!turn.tool_call_followed);

        let temp_agy = tempfile::tempdir().unwrap();
        let transcript = temp_agy.path().join("transcript_full.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"PLANNER_RESPONSE","content":"Now let me commit."}"#,
        )
        .unwrap();

        let turn_agy = read_final_turn(Agent::Agy, temp_agy.path()).unwrap();
        assert_eq!(turn_agy.message.as_deref(), Some("Now let me commit."));
        assert!(!turn_agy.tool_call_followed);
    }
}
