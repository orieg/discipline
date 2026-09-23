//! Agent hooks: run the gates inside a coding agent's edit loop.
//!
//! `discipline hook run --agent <name>` checks the change so far (committed and
//! uncommitted, against the merge base with the default branch) and hands the
//! findings back in the form that agent feeds to its model: the `agent-prompt`
//! report, which names the repair and never the waiver syntax. The check itself is
//! this binary run as a child (`check --format agent-prompt`), so the hook adds no
//! behaviour of its own beyond choosing the base and translating the result.
//!
//! Contracts (from each agent's documentation):
//! * Claude Code and Codex: `PostToolUse` / `Stop` command hooks; exit 2 blocks
//!   and stderr reaches the model. A `Stop` payload with `stop_hook_active` is a
//!   continuation this hook already caused, and is let through so it cannot loop.
//! * Cursor: the `stop` hook; `{"followup_message": ...}` on stdout becomes the
//!   next user message. Cursor caps the loop with `loop_limit`.
//! * Aider: `lint-cmd`; a non-zero exit sends stdout to the model. Aider appends
//!   the edited filenames, which are ignored: the whole change is checked.
//! * GitHub Copilot CLI: `postToolUse` / `agentStop` in `.github/hooks/*.json`. After
//!   an edit, exit 0 with `{"additionalContext": ...}` on stdout is appended to the
//!   tool result the model reads; at the end of a turn `{"decision": "block",
//!   "reason": ...}` forces another turn (`stop_hook_active`, and the CLI's own cap of
//!   eight continuations). Docs: docs.github.com/en/copilot/reference/hooks-reference.
//! * Antigravity CLI (`agy`): `.agents/hooks.json`. Only `Stop` can reach the model:
//!   `{"decision": "continue", "reason": ...}` re-enters the loop with the reason as a
//!   system message. agy documents no loop guard, so this hook counts consecutive
//!   blocks per conversation (under the git directory) and lets the third through.
//!   Docs: antigravity.google/docs/hooks.
//! * Qwen Code: `PostToolUse` / `Stop` in `.qwen/settings.json`, Claude Code's
//!   contract (exit 2, stderr, `stop_hook_active`). Docs:
//!   qwenlm.github.io/qwen-code-docs/en/users/features/hooks.
//! * OpenCode: no command hook; `.opencode/plugins/discipline.js` runs this command
//!   after an edit tool and appends a failure to the tool's output (exit 1, the report
//!   on stdout, as for Aider). Docs: opencode.ai/docs/plugins.
//!
//! `discipline hook install --agent <name>` writes that agent's configuration
//! only where none exists. An existing file is never rewritten: the snippet to
//! add is printed instead.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    /// Claude Code (`.claude/settings.json`, PostToolUse + Stop)
    ClaudeCode,
    /// OpenAI Codex CLI (`.codex/hooks.json`, PostToolUse + Stop)
    Codex,
    /// Cursor (`.cursor/hooks.json`, stop)
    Cursor,
    /// Aider (`.aider.conf.yml`, lint-cmd)
    Aider,
    /// GitHub Copilot CLI (`.github/hooks/discipline.json`, postToolUse + agentStop)
    Copilot,
    /// Antigravity CLI (`.agents/hooks.json`, Stop)
    Agy,
    /// Qwen Code (`.qwen/settings.json`, PostToolUse + Stop)
    Qwen,
    /// OpenCode (`.opencode/plugins/discipline.js`, a plugin after edit tools)
    Opencode,
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Aider => "aider",
            Agent::Copilot => "copilot",
            Agent::Agy => "agy",
            Agent::Qwen => "qwen",
            Agent::Opencode => "opencode",
        }
    }
}

/// What the hook process emits for one check result.
#[derive(Debug, PartialEq, Eq)]
pub struct HookOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: u8,
}

/// Translates a `check` result into the agent's hook contract.
///
/// `check_code` is the child's exit code (0 pass, 1 findings, anything else could
/// not check); `report` is its `agent-prompt` output; `detail` is its stderr. A run
/// that could not check blocks like a finding does (fail-closed): the agent is told
/// the check did not run, never that it passed.
pub fn translate(agent: Agent, check_code: i32, report: &str, detail: &str) -> HookOutput {
    translate_event(agent, Event::Edit, check_code, report, detail)
}

/// The agent event a hook call answers: after an edit, or at the end of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Edit,
    Stop,
}

/// As [`translate`], for an agent whose contract differs between events.
pub fn translate_event(
    agent: Agent,
    event: Event,
    check_code: i32,
    report: &str,
    detail: &str,
) -> HookOutput {
    let message = match check_code {
        0 => {
            return HookOutput {
                stdout: if matches!(agent, Agent::Cursor | Agent::Agy) {
                    "{}\n".to_string()
                } else {
                    String::new()
                },
                stderr: String::new(),
                code: 0,
            }
        }
        1 => report.to_string(),
        _ => format!(
            "discipline could not check this change, so it is not known to be safe. Fix the cause and continue:\n{}\n",
            crate::report::scrub_override_directives(detail.trim())
        ),
    };
    match agent {
        Agent::ClaudeCode | Agent::Codex | Agent::Qwen => HookOutput {
            stdout: String::new(),
            stderr: message,
            code: 2,
        },
        Agent::Copilot => HookOutput {
            stdout: format!(
                "{}\n",
                match event {
                    Event::Edit => serde_json::json!({ "additionalContext": message }),
                    Event::Stop => serde_json::json!({ "decision": "block", "reason": message }),
                }
            ),
            stderr: String::new(),
            code: 0,
        },
        Agent::Agy => HookOutput {
            stdout: format!(
                "{}\n",
                serde_json::json!({ "decision": "continue", "reason": message })
            ),
            stderr: String::new(),
            code: 0,
        },
        Agent::Cursor => HookOutput {
            stdout: format!("{}\n", serde_json::json!({ "followup_message": message })),
            stderr: String::new(),
            code: 0,
        },
        Agent::Aider | Agent::Opencode => HookOutput {
            stdout: message,
            stderr: String::new(),
            code: 1,
        },
    }
}

/// The hook payload's fields this hook reads; any other shape reads as empty.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Payload {
    pub cwd: Option<PathBuf>,
    pub stop_hook_active: bool,
    /// The end of a turn: `hook_event_name: "Stop"` (Claude Code, Codex, Qwen Code),
    /// a `stopReason` (Copilot CLI) or a `terminationReason` (agy).
    pub stop: bool,
    /// agy's conversation, for its loop guard.
    pub conversation: Option<String>,
}

pub fn parse_payload(raw: &str) -> Payload {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Payload::default();
    };
    let event = v
        .get("hook_event_name")
        .or_else(|| v.get("hookEventName"))
        .and_then(|e| e.as_str())
        .unwrap_or("");
    Payload {
        cwd: v
            .get("cwd")
            .and_then(|c| c.as_str())
            // agy names the workspace instead.
            .or_else(|| {
                v.get("workspacePaths")
                    .and_then(|w| w.get(0))
                    .and_then(|c| c.as_str())
            })
            .filter(|c| !c.is_empty())
            .map(PathBuf::from),
        stop_hook_active: v
            .get("stop_hook_active")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
        stop: matches!(event, "Stop" | "agentStop")
            || v.get("stopReason").is_some()
            || v.get("terminationReason").is_some(),
        conversation: v
            .get("conversationId")
            .and_then(|c| c.as_str())
            .map(str::to_string),
    }
}

/// The base the agent's change is measured against: the merge base with the
/// default branch, so a weakening already committed on the branch is seen too.
/// `None` defers to `check`'s own resolution (`DISCIPLINE_BASE_REF`).
pub fn default_base(repo: &git2::Repository) -> Option<String> {
    if std::env::var("DISCIPLINE_BASE_REF").is_ok_and(|b| !b.trim().is_empty()) {
        return None;
    }
    if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Ok(Some(target)) = r.symbolic_target() {
            if let Some(short) = target.strip_prefix("refs/remotes/") {
                return Some(short.to_string());
            }
        }
    }
    for name in ["main", "master"] {
        if repo.find_branch(name, git2::BranchType::Local).is_ok() {
            return Some(name.to_string());
        }
    }
    Some("HEAD".to_string())
}

/// Runs the check for the agent and returns what the hook emits.
pub fn run(agent: Agent, base: Option<String>, stdin: &str) -> Result<HookOutput> {
    let payload = parse_payload(stdin);
    let event = if payload.stop {
        Event::Stop
    } else {
        Event::Edit
    };
    if payload.stop_hook_active {
        return Ok(translate_event(agent, event, 0, "", ""));
    }
    // agy reads nothing a hook returns after a tool call: only its Stop can repair.
    if agent == Agent::Agy && event != Event::Stop {
        return Ok(translate_event(agent, event, 0, "", ""));
    }
    let dir = payload
        .cwd
        .clone()
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| PathBuf::from("."));
    let base = match base {
        Some(b) => Some(b),
        None => crate::gitctx::discover_repository(&dir)
            .ok()
            .and_then(|r| default_base(&r)),
    };
    let base = base.map(CheckSide::Base).unwrap_or(CheckSide::Default);
    let (code, report, detail) = run_check(&dir, &base)?;
    if agent == Agent::Agy {
        return Ok(agy_guarded(
            &dir,
            payload.conversation.as_deref(),
            code,
            &report,
            &detail,
        ));
    }
    Ok(translate_event(agent, event, code, &report, &detail))
}

/// Consecutive `continue` answers agy gets for one conversation before its stop is let
/// through (agy documents no loop guard of its own).
pub const AGY_MAX_CONTINUATIONS: u32 = 3;

/// agy's Stop answer with the loop guard: the count of consecutive blocks for the
/// conversation lives in `<git dir>/discipline/agy-stop-<conversation>`, is reset by a
/// pass, and at [`AGY_MAX_CONTINUATIONS`] the stop is let through and CI gates the
/// change. With no conversation id or git directory, every stop is judged on its own.
fn agy_guarded(
    dir: &Path,
    conversation: Option<&str>,
    code: i32,
    report: &str,
    detail: &str,
) -> HookOutput {
    let counter = conversation
        .map(|c| {
            c.chars()
                .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                .collect::<String>()
        })
        .filter(|c| !c.is_empty())
        .and_then(|c| {
            crate::gitctx::discover_repository(dir)
                .ok()
                .map(|r| r.path().join("discipline").join(format!("agy-stop-{c}")))
        });
    let Some(counter) = counter else {
        return translate_event(Agent::Agy, Event::Stop, code, report, detail);
    };
    if code == 0 {
        let _removed = std::fs::remove_file(&counter);
        return translate_event(Agent::Agy, Event::Stop, 0, "", "");
    }
    let n: u32 = std::fs::read_to_string(&counter)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    if n >= AGY_MAX_CONTINUATIONS {
        let _removed = std::fs::remove_file(&counter);
        return translate_event(Agent::Agy, Event::Stop, 0, "", "");
    }
    let recorded = counter
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|()| std::fs::write(&counter, (n + 1).to_string()));
    if recorded.is_err() {
        // Without a counter there is no guard: judge this stop alone rather than loop.
        return translate_event(Agent::Agy, Event::Stop, 0, "", "");
    }
    translate_event(Agent::Agy, Event::Stop, code, report, detail)
}

/// What an agent-facing check measures the change against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckSide {
    /// `check`'s own resolution (`DISCIPLINE_BASE_REF`, else `main`).
    Default,
    /// The working tree against the merge base with this ref.
    Base(String),
    /// The index against `HEAD`.
    Staged,
}

/// Runs this binary's `check --format agent-prompt` in `dir` and returns its exit
/// code, report and stderr.
///
/// An agent-facing check cannot be talked out of a finding by the change it judges:
/// the base ref's configuration decides (`--policy-from base`), so an agent that edits
/// `discipline.toml` does not switch its own gates off, and no directive is read (the
/// only source left is a PR body, and none is passed), so a waiver in a commit message
/// does not lift a finding here. CI, which reads the reviewed PR body, still can.
pub fn run_check(dir: &Path, side: &CheckSide) -> Result<(i32, String, String)> {
    let exe = std::env::current_exe().context("cannot locate the discipline binary")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.current_dir(dir)
        .args(["check", "--format", "agent-prompt", "--quiet"])
        .args(["--policy-from", "base", "--directive-sources", "pr-body"])
        .env_remove("DISCIPLINE_POLICY_FROM")
        .env_remove("DISCIPLINE_DIRECTIVE_SOURCES")
        .env_remove("PR_BODY")
        .env_remove("PR_TITLE")
        .env_remove("DISCIPLINE_COMMENT");
    match side {
        CheckSide::Default => {}
        CheckSide::Base(b) => {
            cmd.args(["--base", b]);
        }
        CheckSide::Staged => {
            cmd.arg("--staged");
        }
    }
    let out = cmd.output().context("cannot run discipline check")?;
    Ok((
        out.status.code().unwrap_or(2),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// The configuration file an agent reads, relative to the repository root, and the
/// content that wires the hook in.
pub fn config_for(agent: Agent) -> (&'static str, String) {
    let cmd = format!("discipline hook run --agent {}", agent.id());
    match agent {
        Agent::ClaudeCode => (
            ".claude/settings.json",
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PostToolUse": [{
                        "matcher": "Edit|Write|MultiEdit|NotebookEdit",
                        "hooks": [{ "type": "command", "command": cmd }]
                    }],
                    "Stop": [{
                        "hooks": [{ "type": "command", "command": cmd }]
                    }]
                }
            }))
            .unwrap_or_default()
                + "\n",
        ),
        Agent::Codex => (
            ".codex/hooks.json",
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PostToolUse": [{
                        "matcher": "apply_patch|Edit|Write",
                        "hooks": [{ "type": "command", "command": cmd }]
                    }],
                    "Stop": [{
                        "hooks": [{ "type": "command", "command": cmd }]
                    }]
                }
            }))
            .unwrap_or_default()
                + "\n",
        ),
        Agent::Cursor => (
            ".cursor/hooks.json",
            serde_json::to_string_pretty(&serde_json::json!({
                "version": 1,
                "hooks": {
                    "stop": [{ "command": cmd, "loop_limit": 3 }]
                }
            }))
            .unwrap_or_default()
                + "\n",
        ),
        Agent::Aider => (
            ".aider.conf.yml",
            format!("lint-cmd:\n  - \"{cmd}\"\nauto-lint: true\n"),
        ),
        Agent::Copilot => (
            ".github/hooks/discipline.json",
            serde_json::to_string_pretty(&serde_json::json!({
                "version": 1,
                "hooks": {
                    "postToolUse": [{
                        "type": "command",
                        "matcher": "create|edit|str_replace_editor",
                        "bash": cmd,
                        "timeoutSec": 120
                    }],
                    "agentStop": [{ "type": "command", "bash": cmd, "timeoutSec": 120 }]
                }
            }))
            .unwrap_or_default()
                + "\n",
        ),
        Agent::Agy => (
            ".agents/hooks.json",
            serde_json::to_string_pretty(&serde_json::json!({
                "discipline": {
                    "Stop": [{
                        "matcher": "",
                        "hooks": [{ "type": "command", "command": cmd, "timeout": 120 }]
                    }]
                }
            }))
            .unwrap_or_default()
                + "\n",
        ),
        Agent::Qwen => (
            ".qwen/settings.json",
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PostToolUse": [{
                        "matcher": "^(write_file|edit)$",
                        "hooks": [{ "type": "command", "command": cmd, "timeout": 120 }]
                    }],
                    "Stop": [{
                        "hooks": [{ "type": "command", "command": cmd, "timeout": 120 }]
                    }]
                }
            }))
            .unwrap_or_default()
                + "\n",
        ),
        Agent::Opencode => (".opencode/plugins/discipline.js", opencode_plugin(&cmd)),
    }
}

/// The OpenCode plugin: after an edit tool, run the hook and append a failure to the
/// tool's output, which is the text the model reads.
fn opencode_plugin(cmd: &str) -> String {
    format!(
        "// Written by `discipline hook install --agent opencode`.
// After an edit tool, runs the discipline check and, when it fails, appends the report
// to the tool's output so the model reads it and repairs the change.
const EDIT_TOOLS = [\"edit\", \"write\", \"apply_patch\"]

export const Discipline = async ({{ $, directory }}) => ({{
  \"tool.execute.after\": async (input, output) => {{
    if (!EDIT_TOOLS.includes(input.tool)) return
    const r = await $`{cmd}`.cwd(directory).nothrow().quiet()
    if (r.exitCode !== 0) {{
      output.output += \"\\n\\n\" + r.stdout.toString() + r.stderr.toString()
    }}
  }},
}})
"
    )
}

/// What `install` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Written(PathBuf),
    AlreadyPresent(PathBuf),
    /// The file exists without the hook; nothing was written. Carries the snippet.
    Refused(PathBuf, String),
}

/// Writes the agent's configuration under `root` when the file does not exist.
pub fn install(agent: Agent, root: &Path) -> Result<Installed> {
    let (rel, content) = config_for(agent);
    let path = root.join(rel);
    if path.exists() {
        let existing = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if existing.contains(&format!("discipline hook run --agent {}", agent.id())) {
            return Ok(Installed::AlreadyPresent(path));
        }
        return Ok(Installed::Refused(path, content));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(Installed::Written(path))
}

/// The repository root `install` writes under.
pub fn repo_root() -> Result<PathBuf> {
    let repo = crate::gitctx::discover_repository(".")
        .context("not a git repository: agent hooks are installed at the repository root")?;
    match repo.workdir() {
        Some(w) => Ok(w.to_path_buf()),
        None => bail!("bare repositories have no working tree to install hooks into"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findings_block_in_each_agents_contract() {
        let report = "### Issue 1 [assertion-reduction]: x\n";
        let cc = translate(Agent::ClaudeCode, 1, report, "");
        assert_eq!(
            (cc.code, cc.stderr.as_str(), cc.stdout.as_str()),
            (2, report, "")
        );
        assert_eq!(translate(Agent::Codex, 1, report, "").code, 2);
        let cursor = translate(Agent::Cursor, 1, report, "");
        assert_eq!(cursor.code, 0);
        let v: serde_json::Value = serde_json::from_str(&cursor.stdout).unwrap();
        assert_eq!(v["followup_message"], report);
        let aider = translate(Agent::Aider, 1, report, "");
        assert_eq!((aider.code, aider.stdout.as_str()), (1, report));
    }

    #[test]
    fn a_pass_is_silent_and_could_not_check_blocks() {
        for agent in [Agent::ClaudeCode, Agent::Codex, Agent::Aider] {
            assert_eq!(
                translate(agent, 0, "No discipline violations", ""),
                HookOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    code: 0
                }
            );
        }
        assert_eq!(translate(Agent::Cursor, 0, "", "").stdout, "{}\n");
        let broken = translate(Agent::ClaudeCode, 2, "", "config does not parse");
        assert_eq!(broken.code, 2);
        assert!(
            broken.stderr.contains("could not check")
                && broken.stderr.contains("config does not parse")
        );
        assert_eq!(translate(Agent::Aider, 2, "", "x").code, 1);
    }

    #[test]
    fn payload_reads_cwd_and_stop_hook_active_and_tolerates_anything_else() {
        assert_eq!(
            parse_payload(r#"{"cwd":"/r","stop_hook_active":true,"x":1}"#),
            Payload {
                cwd: Some(PathBuf::from("/r")),
                stop_hook_active: true,
                ..Default::default()
            }
        );
        assert_eq!(parse_payload(""), Payload::default());
        assert_eq!(parse_payload("[1]"), Payload::default());
    }

    #[test]
    fn install_writes_once_and_never_rewrites_a_foreign_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = install(Agent::ClaudeCode, dir.path()).unwrap();
        assert!(matches!(first, Installed::Written(_)));
        let again = install(Agent::ClaudeCode, dir.path()).unwrap();
        assert!(matches!(again, Installed::AlreadyPresent(_)));
        let foreign = dir.path().join(".cursor/hooks.json");
        std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        std::fs::write(&foreign, "{\"version\":1}").unwrap();
        let refused = install(Agent::Cursor, dir.path()).unwrap();
        assert!(matches!(refused, Installed::Refused(_, ref s) if s.contains("--agent cursor")));
        assert_eq!(
            std::fs::read_to_string(&foreign).unwrap(),
            "{\"version\":1}"
        );
    }
}
