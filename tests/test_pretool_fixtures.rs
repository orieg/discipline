//! Phase 13 Step 0: each agent's pre-tool hook payload and deny answer, recorded from a
//! live run (docs/ROADMAP.md, Phase 13). These tests pin the fields the pre-tool check
//! (Step 3) reads, so a re-recorded fixture that moves one fails here first.

use serde_json::Value;

fn fixture(rel: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/pretool/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no string at {pointer} in {v:#}"))
}

/// (fixture, tool-name pointer, tool name, target pointer, session pointer, cwd pointer)
const PAYLOADS: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "claude-code/write.json",
        "/tool_name",
        "Write",
        "/tool_input/file_path",
        "/session_id",
        "/cwd",
    ),
    (
        "claude-code/bash.json",
        "/tool_name",
        "Bash",
        "/tool_input/command",
        "/session_id",
        "/cwd",
    ),
    (
        "copilot/create.json",
        "/toolName",
        "create",
        "/toolArgs/path",
        "/sessionId",
        "/cwd",
    ),
    (
        "agy/write_to_file.json",
        "/toolCall/name",
        "write_to_file",
        "/toolCall/args/TargetFile",
        "/conversationId",
        "/workspacePaths/0",
    ),
    (
        "opencode/write.json",
        "/input/tool",
        "write",
        "/output/args/filePath",
        "/input/sessionID",
        "",
    ),
];

#[test]
fn each_recorded_payload_carries_the_tool_its_target_and_the_session() {
    for (rel, tool_ptr, tool, target_ptr, session_ptr, cwd_ptr) in PAYLOADS {
        let v = fixture(rel);
        assert_eq!(text(&v, tool_ptr), *tool, "{rel}");
        let target = text(&v, target_ptr);
        assert!(!target.is_empty(), "{rel}: empty target");
        assert!(!text(&v, session_ptr).is_empty(), "{rel}: no session id");
        if !cwd_ptr.is_empty() {
            assert!(
                text(&v, cwd_ptr).starts_with('/'),
                "{rel}: cwd not absolute"
            );
        }
        // Every edit target was recorded as an absolute path, which is what lets the
        // check tell one worktree from another without guessing a base directory.
        if !target_ptr.ends_with("command") {
            assert!(
                target.starts_with('/'),
                "{rel}: target {target} is not absolute"
            );
        }
    }
}

#[test]
fn each_recorded_deny_blocked_the_call() {
    for rel in [
        "claude-code/deny.json",
        "copilot/deny.json",
        "agy/deny.json",
        "opencode/deny.json",
    ] {
        let v = fixture(rel);
        assert!(text(&v, "/observed").starts_with("RUN "), "{rel}");
        let answers = v["answers"]
            .as_array()
            .unwrap_or_else(|| panic!("{rel}: no answers"));
        assert!(!answers.is_empty(), "{rel}");
        for a in answers {
            assert!(
                a["result"].as_str().unwrap_or_default().contains("blocked"),
                "{rel}: {a}"
            );
        }
    }
}

#[test]
fn no_fixture_names_a_local_machine() {
    let dir = format!("{}/tests/fixtures/pretool", env!("CARGO_MANIFEST_DIR"));
    for agent in std::fs::read_dir(&dir).unwrap() {
        for f in std::fs::read_dir(agent.unwrap().path()).unwrap() {
            let p = f.unwrap().path();
            let s = std::fs::read_to_string(&p).unwrap();
            for leak in ["/Users/", "/private/tmp", "/var/folders"] {
                assert!(!s.contains(leak), "{} names {leak}", p.display());
            }
        }
    }
}
