//! The pre-tool hook: an agent's tool-call JSON, an `apply_patch` body, and the shell
//! command a tool is about to run (parsed with tree-sitter-bash). A payload or command
//! that cannot be read is refused or ignored; it must never panic.
//!
//! The body runs on the deep stack the binary does its work on
//! (`discipline::deep_stack`), as the hook does: a command can nest to the tree depth
//! limit.
#![no_main]

use discipline::hook::Agent;
use discipline::pretool::{self, Scene, Worktrees};
use libfuzzer_sys::fuzz_target;
use std::path::Path;

const AGENTS: &[Agent] = &[
    Agent::ClaudeCode,
    Agent::Codex,
    Agent::Cursor,
    Agent::Aider,
    Agent::Copilot,
    Agent::Agy,
    Agent::Qwen,
    Agent::Opencode,
];

fuzz_target!(|data: &[u8]| {
    discipline::deep_stack::on_deep_stack(|| {
        let Some((&sel, body)) = data.split_first() else {
            return;
        };
        let text = String::from_utf8_lossy(body);
        let agent = AGENTS[sel as usize % AGENTS.len()];
        let _ = pretool::parse(agent, &text);
        let _ = pretool::parse_session_start(agent, &text);
        let _ = pretool::patch_targets(&text);

        let worktrees = Worktrees {
            all: vec![
                ("main".to_string(), "/repo".into()),
                ("other".to_string(), "/repo/.worktrees/other".into()),
            ],
            here: "main".to_string(),
        };
        let scene = Scene {
            worktrees: &worktrees,
            leases: &[],
            forbidden: None,
            branch: Some("main"),
        };
        let _ = pretool::judge_shell(&text, Path::new("/repo"), &scene);
    })
    .expect("the deep stack");
});
