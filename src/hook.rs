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
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Aider => "aider",
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
    let message = match check_code {
        0 => {
            return HookOutput {
                stdout: if agent == Agent::Cursor {
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
            detail.trim()
        ),
    };
    match agent {
        Agent::ClaudeCode | Agent::Codex => HookOutput {
            stdout: String::new(),
            stderr: message,
            code: 2,
        },
        Agent::Cursor => HookOutput {
            stdout: format!("{}\n", serde_json::json!({ "followup_message": message })),
            stderr: String::new(),
            code: 0,
        },
        Agent::Aider => HookOutput {
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
}

pub fn parse_payload(raw: &str) -> Payload {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Payload::default();
    };
    Payload {
        cwd: v
            .get("cwd")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(PathBuf::from),
        stop_hook_active: v
            .get("stop_hook_active")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
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
    if payload.stop_hook_active {
        return Ok(translate(agent, 0, "", ""));
    }
    let dir = payload
        .cwd
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
    Ok(translate(agent, code, &report, &detail))
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
/// code, report and stderr. A PR body in the environment is not passed on: an
/// agent-facing check reads the change, not a waiver.
pub fn run_check(dir: &Path, side: &CheckSide) -> Result<(i32, String, String)> {
    let exe = std::env::current_exe().context("cannot locate the discipline binary")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.current_dir(dir)
        .args(["check", "--format", "agent-prompt", "--quiet"])
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
    }
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
                stop_hook_active: true
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
