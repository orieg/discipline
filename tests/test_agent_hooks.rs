//! `discipline hook run` / `hook install` through the real binary, in a repository
//! whose agent weakens an assertion.

mod common;

use common::Repo;
use std::io::Write;
use std::process::{Command, Stdio};

struct HookRun {
    code: i32,
    stdout: String,
    stderr: String,
}

fn hook(repo: &Repo, args: &[&str], stdin: &str) -> HookRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_discipline"));
    cmd.args(args)
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("DISCIPLINE_NO_NETWORK", "1");
    for var in common::ISOLATED_ENV_VARS {
        cmd.env_remove(var);
    }
    for (k, _) in std::env::vars() {
        if k.starts_with("DISCIPLINE_") && k != "DISCIPLINE_NO_NETWORK" {
            cmd.env_remove(k);
        }
    }
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

/// `tests/a.rs` with `adds` weakened from two `assert_eq!` to none, uncommitted.
fn weakened(repo: &Repo) {
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
}

const POST_EDIT: &str = r#"{"hook_event_name":"PostToolUse","tool_name":"Edit"}"#;

#[test]
fn claude_code_hook_blocks_a_weakened_test_with_repair_text_and_no_waiver() {
    let repo = Repo::new();
    let clean = hook(&repo, &["hook", "run", "--agent", "claude-code"], POST_EDIT);
    assert_eq!(
        (clean.code, clean.stderr.as_str()),
        (0, ""),
        "{}",
        clean.stdout
    );

    weakened(&repo);
    let run = hook(&repo, &["hook", "run", "--agent", "claude-code"], POST_EDIT);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("[assertion-reduction]"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("Repair:"), "{}", run.stderr);
    assert!(
        !run.stderr.contains("allow-") && !run.stderr.contains("discipline:allow"),
        "waiver syntax reached the agent: {}",
        run.stderr
    );
    assert_eq!(run.stdout, "");

    // A Stop continuation this hook caused is let through, so it cannot loop.
    let again = hook(
        &repo,
        &["hook", "run", "--agent", "claude-code"],
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
    );
    assert_eq!(again.code, 0);
}

#[test]
fn a_committed_weakening_on_the_branch_is_seen() {
    let repo = Repo::new();
    weakened(&repo);
    repo.commit("test: simplify");
    let run = hook(&repo, &["hook", "run", "--agent", "codex"], POST_EDIT);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("[assertion-reduction]"),
        "{}",
        run.stderr
    );
}

#[test]
fn cursor_and_aider_get_their_own_contracts() {
    let repo = Repo::new();
    weakened(&repo);
    let cursor = hook(
        &repo,
        &["hook", "run", "--agent", "cursor"],
        r#"{"status":"completed","loop_count":0}"#,
    );
    assert_eq!(cursor.code, 0);
    let v: serde_json::Value = serde_json::from_str(&cursor.stdout).unwrap();
    assert!(v["followup_message"]
        .as_str()
        .unwrap()
        .contains("[assertion-reduction]"));
    let aider = hook(
        &repo,
        &["hook", "run", "--agent", "aider", "tests/a.rs"],
        "",
    );
    assert_eq!(aider.code, 1, "{}", aider.stderr);
    assert!(
        aider.stdout.contains("[assertion-reduction]"),
        "{}",
        aider.stdout
    );
}

#[test]
fn a_check_that_cannot_run_blocks_rather_than_passes() {
    let repo = Repo::new();
    repo.write("discipline.toml", "[meta\n");
    let run = hook(&repo, &["hook", "run", "--agent", "claude-code"], POST_EDIT);
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(run.stderr.contains("could not check"), "{}", run.stderr);
}

#[test]
fn install_writes_the_agent_config_once_and_leaves_an_existing_file_alone() {
    let repo = Repo::new();
    let first = repo.run(&["hook", "install", "--agent", "claude-code"], &[]);
    assert_eq!(first.code, 0, "{}", first.stderr);
    let written = std::fs::read_to_string(repo.file(".claude/settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        v["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "discipline hook run --agent claude-code"
    );
    assert_eq!(
        v["hooks"]["Stop"][0]["hooks"][0]["command"],
        "discipline hook run --agent claude-code"
    );
    let again = repo.run(&["hook", "install", "--agent", "claude-code"], &[]);
    assert_eq!(again.code, 0);

    repo.write(".aider.conf.yml", "model: x\n");
    let refused = repo.run(&["hook", "install", "--agent", "aider"], &[]);
    assert_eq!(refused.code, 1);
    assert!(refused.stdout.contains("lint-cmd"), "{}", refused.stdout);
    assert_eq!(
        std::fs::read_to_string(repo.file(".aider.conf.yml")).unwrap(),
        "model: x\n"
    );
}

#[test]
fn explain_names_the_rule_the_state_here_and_the_directive() {
    let repo = Repo::new();
    let run = repo.run(&["explain", "assertion-reduction"], &[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout.contains("assertion count / strength"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("Here:        on, error"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("`allow-assertion-drop: <subject> <reason>`"),
        "{}",
        run.stdout
    );

    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[gates.assertion-reduction]\nenabled = false\n",
    );
    let off = repo.run(
        &["explain", "error [assertion-reduction] Assertion Reduction"],
        &[],
    );
    assert!(off.stdout.contains("Here:        off"), "{}", off.stdout);

    let unknown = repo.run(&["explain", "swallow"], &[]);
    assert_eq!(unknown.code, 2);
    assert!(
        unknown.stderr.contains("error-swallowing"),
        "{}",
        unknown.stderr
    );
}

#[test]
fn check_help_lists_every_directive_source() {
    let repo = Repo::new();
    let help = repo.run(&["check", "--help"], &[]);
    assert!(
        help.stdout.contains("pr-body, commits, merged-pr-body"),
        "{}",
        help.stdout
    );
}

/// A change cannot switch off the checks that judge it: the agent-facing check reads
/// the base ref's configuration and no directive.
#[test]
fn an_agent_cannot_silence_its_own_hook() {
    // 1. Disabling the gate in the working tree's discipline.toml.
    let repo = Repo::new();
    weakened(&repo);
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[gates.assertion-reduction]\nenabled = false\n",
    );
    let run = hook(&repo, &["hook", "run", "--agent", "claude-code"], POST_EDIT);
    assert_eq!(
        run.code, 2,
        "the change's own config lifted the finding: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains("[assertion-reduction]"),
        "{}",
        run.stderr
    );

    // 2. A waiver in a commit message.
    let repo = Repo::new();
    weakened(&repo);
    repo.git(&["add", "-A"]);
    repo.git(&[
        "-c",
        "user.email=t@example.invalid",
        "-c",
        "user.name=t",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "-m",
        "test: simplify",
        "-m",
        "allow-assertion-drop: adds the second check moved elsewhere",
    ]);
    let run = hook(&repo, &["hook", "run", "--agent", "claude-code"], POST_EDIT);
    assert_eq!(
        run.code, 2,
        "a commit-message waiver lifted the finding: {}",
        run.stderr
    );
}

#[test]
fn the_shared_hook_file_is_not_scratch_state() {
    let repo = Repo::new();
    let install = repo.run(&["hook", "install", "--agent", "claude-code"], &[]);
    assert_eq!(install.code, 0, "{}", install.stderr);
    repo.commit("chore: agent hook");
    let run = repo.check(&[]);
    assert!(
        run.titles("agent-scratch").is_empty(),
        "{:?}",
        run.violations("agent-scratch")
    );
    // Still an agent-control change a reviewer sees.
    assert!(
        !run.titles("instruction-smuggling").is_empty(),
        "{}",
        run.stdout
    );
}

#[test]
fn copilot_gets_additional_context_after_an_edit_and_a_block_at_stop() {
    let repo = Repo::new();
    weakened(&repo);
    let edit = hook(
        &repo,
        &["hook", "run", "--agent", "copilot"],
        r#"{"sessionId":"s","timestamp":1,"cwd":".","toolName":"edit","toolArgs":{}}"#,
    );
    assert_eq!(edit.code, 0, "{}", edit.stderr);
    let v: serde_json::Value = serde_json::from_str(&edit.stdout).unwrap();
    assert!(v["additionalContext"]
        .as_str()
        .unwrap()
        .contains("[assertion-reduction]"));

    let stop = hook(
        &repo,
        &["hook", "run", "--agent", "copilot"],
        r#"{"sessionId":"s","timestamp":1,"cwd":".","stopReason":"end_turn","stop_hook_active":false}"#,
    );
    let v: serde_json::Value = serde_json::from_str(&stop.stdout).unwrap();
    assert_eq!(v["decision"], "block");
    assert!(v["reason"].as_str().unwrap().contains("Repair:"));

    let again = hook(
        &repo,
        &["hook", "run", "--agent", "copilot"],
        r#"{"stopReason":"end_turn","stop_hook_active":true}"#,
    );
    assert_eq!((again.code, again.stdout.as_str()), (0, ""));
}

#[test]
fn agy_repairs_only_at_stop_and_lets_the_fourth_stop_through() {
    let repo = Repo::new();
    weakened(&repo);
    // A tool event cannot reach agy's model: nothing is checked, `{}` returned.
    let tool = hook(
        &repo,
        &["hook", "run", "--agent", "agy"],
        r#"{"conversationId":"c1","toolCall":{"name":"write_to_file"},"stepIdx":3}"#,
    );
    assert_eq!((tool.code, tool.stdout.trim()), (0, "{}"));

    let stop =
        r#"{"conversationId":"c1","executionNum":1,"terminationReason":"done","fullyIdle":true}"#;
    for _ in 0..3 {
        let r = hook(&repo, &["hook", "run", "--agent", "agy"], stop);
        let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
        assert_eq!(v["decision"], "continue", "{}", r.stdout);
        assert!(v["reason"]
            .as_str()
            .unwrap()
            .contains("[assertion-reduction]"));
    }
    // agy documents no loop guard: the fourth consecutive block is let through.
    let fourth = hook(&repo, &["hook", "run", "--agent", "agy"], stop);
    assert_eq!(fourth.stdout.trim(), "{}", "{}", fourth.stdout);
    // The counter restarts: a later stop is judged again.
    let later = hook(&repo, &["hook", "run", "--agent", "agy"], stop);
    assert!(later.stdout.contains("continue"), "{}", later.stdout);
}

#[test]
fn qwen_follows_claude_codes_contract_and_opencode_aiders() {
    let repo = Repo::new();
    weakened(&repo);
    let qwen = hook(
        &repo,
        &["hook", "run", "--agent", "qwen"],
        r#"{"hook_event_name":"PostToolUse","tool_name":"edit"}"#,
    );
    assert_eq!(qwen.code, 2);
    assert!(
        qwen.stderr.contains("[assertion-reduction]"),
        "{}",
        qwen.stderr
    );
    let qwen_loop = hook(
        &repo,
        &["hook", "run", "--agent", "qwen"],
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
    );
    assert_eq!(qwen_loop.code, 0);

    let opencode = hook(&repo, &["hook", "run", "--agent", "opencode"], "");
    assert_eq!(opencode.code, 1);
    assert!(
        opencode.stdout.contains("[assertion-reduction]"),
        "{}",
        opencode.stdout
    );
}

#[test]
fn install_writes_each_new_agents_file() {
    let repo = Repo::new();
    for (agent, file, needle) in [
        ("copilot", ".github/hooks/discipline.json", "\"agentStop\""),
        ("agy", ".agents/hooks.json", "\"Stop\""),
        ("qwen", ".qwen/settings.json", "\"PostToolUse\""),
        (
            "opencode",
            ".opencode/plugins/discipline.js",
            "tool.execute.after",
        ),
    ] {
        let run = repo.run(&["hook", "install", "--agent", agent], &[]);
        assert_eq!(run.code, 0, "{agent}: {}", run.stderr);
        let text = std::fs::read_to_string(repo.file(file)).unwrap();
        assert!(text.contains(needle), "{agent}: {text}");
        assert!(
            text.contains(&format!("discipline hook run --agent {agent}")),
            "{agent}: {text}"
        );
        if file.ends_with(".json") {
            serde_json::from_str::<serde_json::Value>(&text).unwrap();
        }
    }
}
