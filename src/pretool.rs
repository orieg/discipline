//! The pre-tool check (`hook run --event pre-tool`): refuse an edit before it runs.
//!
//! Several agents often share one repository, each in its own git worktree. An edit
//! made from one session into another worktree, or into a worktree another live session
//! has leased, is refused before the tool runs (docs/ROADMAP.md, Phase 13 Step 3).
//! The contracts are the ones recorded live in Step 0 (`tests/fixtures/pretool/`):
//!
//! | Agent       | Edit tools                                      | Target field                | Session           | Deny                                                      |
//! |-------------|-------------------------------------------------|-----------------------------|-------------------|-----------------------------------------------------------|
//! | Claude Code | `Write`, `Edit`, `MultiEdit`, `NotebookEdit`    | `tool_input.file_path` / `notebook_path` | `session_id` | exit 2, reason on stderr                                |
//! | Copilot CLI | `create`, `edit`, `str_replace_editor`, `apply_patch` | `toolArgs.path`         | `sessionId`       | exit 0, `{"permissionDecision":"deny", ...}`              |
//! | agy         | a tool whose arguments name a `TargetFile`      | `toolCall.args.TargetFile`  | `conversationId`  | exit 0, `{"decision":"deny","reason": ...}`               |
//! | OpenCode    | `write`, `edit`, `apply_patch`                  | `output.args.filePath`, or the patch's file lines | `input.sessionID` | exit 1, the plugin throws with the reason |
//!
//! Codex and Qwen Code are answered like Claude Code (their documented contract, not
//! observed live). Cursor and Aider have no pre-tool event for edits: this check does
//! not apply to them.
//!
//! The check is cooperative: it stops sessions stepping on each other by mistake. An
//! edit outside the repository (a scratch directory) is not refused. An edit tool whose
//! target cannot be read is refused: a check that cannot see where an edit goes does not
//! let it through.

use crate::hook::{Agent, HookOutput};
use crate::lease::Lease;
use std::path::{Component, Path, PathBuf};

/// One tool call, as the agent's pre-tool payload describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCall {
    pub tool: String,
    /// Whether the tool edits files (as opposed to reading or running).
    pub edits: bool,
    /// The files the edit writes, as given (absolute, or relative to `cwd`).
    pub targets: Vec<String>,
    pub session: Option<String>,
    pub cwd: Option<PathBuf>,
}

fn s(v: &serde_json::Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(|x| x.as_str())
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

/// The files an `apply_patch` body touches (`*** Add File: <path>`, `*** Update File:`,
/// `*** Delete File:`, `*** Move to:`).
pub fn patch_targets(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            [
                "*** Add File:",
                "*** Update File:",
                "*** Delete File:",
                "*** Move to:",
            ]
            .iter()
            .find_map(|p| l.strip_prefix(p))
            .map(|p| p.trim().to_string())
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// Read a pre-tool payload in `agent`'s shape. `None` for an agent without a pre-tool
/// contract or a payload that is not JSON.
pub fn parse(agent: Agent, raw: &str) -> Option<ToolCall> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let mut call = ToolCall::default();
    match agent {
        Agent::ClaudeCode | Agent::Codex | Agent::Qwen => {
            call.tool = s(&v, "/tool_name").unwrap_or_default();
            call.edits = matches!(
                call.tool.as_str(),
                "Write"
                    | "Edit"
                    | "MultiEdit"
                    | "NotebookEdit"
                    | "apply_patch"
                    | "write_file"
                    | "edit"
                    | "replace"
            );
            call.targets.extend(s(&v, "/tool_input/file_path"));
            call.targets.extend(s(&v, "/tool_input/notebook_path"));
            call.targets.extend(s(&v, "/tool_input/path"));
            if let Some(p) = s(&v, "/tool_input/command").filter(|_| call.tool == "apply_patch") {
                call.targets.extend(patch_targets(&p));
            }
            call.session = s(&v, "/session_id");
            call.cwd = s(&v, "/cwd").map(PathBuf::from);
        }
        Agent::Copilot => {
            call.tool = s(&v, "/toolName").unwrap_or_default();
            call.edits = matches!(
                call.tool.as_str(),
                "create" | "edit" | "str_replace_editor" | "apply_patch" | "write"
            );
            call.targets.extend(s(&v, "/toolArgs/path"));
            // Copilot may send the arguments as a JSON string.
            let args = v.get("toolArgs");
            if let Some(text) = args.and_then(|a| a.as_str()) {
                if let Ok(a) = serde_json::from_str::<serde_json::Value>(text) {
                    call.targets.extend(s(&a, "/path"));
                    call.targets.extend(
                        s(&a, "/input")
                            .map(|p| patch_targets(&p))
                            .into_iter()
                            .flatten(),
                    );
                } else {
                    call.targets.extend(patch_targets(text));
                }
            }
            if let Some(p) = s(&v, "/toolArgs/input") {
                call.targets.extend(patch_targets(&p));
            }
            call.session = s(&v, "/sessionId");
            call.cwd = s(&v, "/cwd").map(PathBuf::from);
        }
        Agent::Agy => {
            call.tool = s(&v, "/toolCall/name").unwrap_or_default();
            let target = s(&v, "/toolCall/args/TargetFile");
            call.edits = target.is_some();
            call.targets.extend(target);
            call.session = s(&v, "/conversationId");
            call.cwd = s(&v, "/workspacePaths/0").map(PathBuf::from);
        }
        Agent::Opencode => {
            call.tool = s(&v, "/input/tool").unwrap_or_default();
            call.edits = matches!(call.tool.as_str(), "write" | "edit" | "apply_patch");
            call.targets.extend(s(&v, "/output/args/filePath"));
            if let Some(p) = s(&v, "/output/args/patchText") {
                call.targets.extend(patch_targets(&p));
            }
            call.session = s(&v, "/input/sessionID");
            call.cwd = s(&v, "/cwd").map(PathBuf::from);
        }
        Agent::Cursor | Agent::Aider => return None,
    }
    Some(call)
}

/// `path` made absolute against `cwd`, `.` and `..` resolved lexically, and the longest
/// prefix that exists resolved through symlinks (`/tmp` is `/private/tmp` on macOS), so
/// it compares with canonical worktree roots.
pub fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let mut clean = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other.as_os_str()),
        }
    }
    let mut existing = clean.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return clean,
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for name in rest.iter().rev() {
        out.push(name);
    }
    out
}

/// The repository's worktrees: key (as `lease` names them) and canonical root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktrees {
    pub all: Vec<(String, PathBuf)>,
    /// The key of the worktree the session runs in.
    pub here: String,
}

impl Worktrees {
    /// The worktree `path` is in: the deepest root containing it (a worktree may sit
    /// inside another's directory).
    pub fn owner(&self, path: &Path) -> Option<&(String, PathBuf)> {
        self.all
            .iter()
            .filter(|(_, root)| path.starts_with(root))
            .max_by_key(|(_, root)| root.components().count())
    }
}

/// Every worktree of the repository at `dir` and which one `dir` is in.
pub fn worktrees(dir: &Path) -> anyhow::Result<Worktrees> {
    let repo = crate::gitctx::discover_repository(dir)?;
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let main = git2::Repository::open(repo.commondir())?;
    let mut all = Vec::new();
    if let Some(w) = main.workdir() {
        all.push(("main".to_string(), canon(w)));
    }
    for name in main.worktrees()?.iter().flatten().flatten() {
        if let Ok(wt) = main.find_worktree(name) {
            all.push((crate::lease::key_of(name), canon(wt.path())));
        }
    }
    let (_, here) = crate::lease::open(dir)?;
    Ok(Worktrees {
        all,
        here: here.key,
    })
}

/// What the check decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
}

/// The inputs of [`judge`] that are not the tool call.
pub struct Scene<'a> {
    pub worktrees: &'a Worktrees,
    /// Live leases, by worktree key.
    pub leases: &'a [(String, Lease)],
    /// `scope-confinement`'s `forbidden_paths` when that gate is enabled.
    pub forbidden: Option<&'a crate::guards::PathFilter>,
}

/// Decide whether `call` may run.
pub fn judge(call: &ToolCall, cwd: &Path, scene: &Scene) -> Verdict {
    if !call.edits {
        return Verdict::Allow;
    }
    if call.targets.is_empty() {
        return Verdict::Deny(format!(
            "discipline could not read which file the `{}` call edits, so it cannot tell whether the edit stays in this session's worktree; it is refused",
            call.tool
        ));
    }
    let here = scene
        .worktrees
        .all
        .iter()
        .find(|(k, _)| *k == scene.worktrees.here);
    for t in &call.targets {
        let path = resolve(t, cwd);
        let Some((key, root)) = scene.worktrees.owner(&path) else {
            // Outside the repository (a scratch directory): not this check's concern.
            continue;
        };
        if Some(key) != here.map(|(k, _)| k) {
            return Verdict::Deny(format!(
                "`{}` is in worktree `{key}` ({}), not in this session's worktree `{}`. Each session edits only its own worktree; ask the session working in `{key}` to make this change, or make it in a worktree of your own",
                path.display(),
                root.display(),
                scene.worktrees.here
            ));
        }
        if let Some((_, lease)) = scene.leases.iter().find(|(k, _)| k == key) {
            let other_session = !lease.session.is_empty()
                && call
                    .session
                    .as_deref()
                    .is_some_and(|s| !s.is_empty() && s != lease.session);
            if other_session {
                return Verdict::Deny(format!(
                    "worktree `{key}` is leased by {} session {}; this session ({}) may not edit it. Hand the work over, or take the worktree's lease with `discipline lease take --steal`",
                    lease.agent,
                    lease.session,
                    call.session.as_deref().unwrap_or("-")
                ));
            }
        }
        if let Some(f) = scene.forbidden {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if f.matches(&rel) {
                    return Verdict::Deny(format!(
                        "`{rel}` matches `scope-confinement`'s forbidden_paths; this repository does not let an agent edit it"
                    ));
                }
            }
        }
    }
    Verdict::Allow
}

/// `agent`'s answer: its pass, or its deny carrying `reason` (the shapes recorded live).
pub fn answer(agent: Agent, verdict: &Verdict) -> HookOutput {
    let pass = HookOutput {
        stdout: String::new(),
        stderr: String::new(),
        code: 0,
    };
    let Verdict::Deny(reason) = verdict else {
        return pass;
    };
    match agent {
        Agent::ClaudeCode | Agent::Codex | Agent::Qwen => HookOutput {
            stdout: String::new(),
            stderr: format!("{reason}\n"),
            code: 2,
        },
        Agent::Copilot => HookOutput {
            stdout: format!(
                "{}\n",
                serde_json::json!({"permissionDecision": "deny", "permissionDecisionReason": reason})
            ),
            stderr: String::new(),
            code: 0,
        },
        Agent::Agy => HookOutput {
            stdout: format!(
                "{}\n",
                serde_json::json!({"decision": "deny", "reason": reason})
            ),
            stderr: String::new(),
            code: 0,
        },
        // The plugin throws with the command's output when it exits non-zero.
        Agent::Opencode => HookOutput {
            stdout: format!("{reason}\n"),
            stderr: String::new(),
            code: 1,
        },
        Agent::Cursor | Agent::Aider => pass,
    }
}

/// `hook run --event pre-tool`: parse the payload, judge the call, answer in `agent`'s
/// contract. In `observe` mode a deny is said on stderr and logged, and the call passes.
/// A check that cannot be made (a repository or lease that cannot be read) refuses an
/// edit, never lets it through.
pub fn run(agent: Agent, stdin: &str, observe: bool) -> HookOutput {
    let Some(call) = parse(agent, stdin) else {
        // Cursor and Aider have no pre-tool event; a payload that is not JSON cannot
        // name an edit.
        let verdict = if matches!(agent, Agent::Cursor | Agent::Aider) {
            Verdict::Allow
        } else {
            Verdict::Deny(
                "discipline could not read the pre-tool payload, so the call is refused".into(),
            )
        };
        return finish(agent, &verdict, observe, None);
    };
    let cwd = call
        .cwd
        .clone()
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| PathBuf::from("."));
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let decided = (|| -> anyhow::Result<(Verdict, Option<PathBuf>)> {
        if crate::gitctx::discover_repository(&cwd).is_err() {
            // Not in a repository: there is no worktree to protect.
            return Ok((Verdict::Allow, None));
        }
        let wts = worktrees(&cwd)?;
        let (store, here) = crate::lease::open(&cwd)?;
        let now = crate::lease::now();
        store.touch(&here.key, call.session.as_deref(), now)?;
        if !call.edits {
            return Ok((Verdict::Allow, Some(here.root)));
        }
        let leases: Vec<(String, Lease)> = store
            .list()?
            .into_iter()
            .filter(|(_, l)| l.is_live(now))
            .collect();
        let config = here.root.join("discipline.toml");
        let forbidden = if config.is_file() {
            let c = crate::config::DisciplineConfig::load_from_file(&config)?;
            let s = &c.gates.scope_confinement;
            (s.enabled && !s.forbidden_paths.is_empty())
                .then(|| crate::guards::PathFilter::new(&s.forbidden_paths))
                .transpose()?
        } else {
            None
        };
        let scene = Scene {
            worktrees: &wts,
            leases: &leases,
            forbidden: forbidden.as_ref(),
        };
        Ok((judge(&call, &cwd, &scene), Some(here.root)))
    })();
    let (verdict, root) = decided.unwrap_or_else(|e| {
        let v = if call.edits {
            Verdict::Deny(format!(
                "discipline could not check where this edit goes ({e:#}), so it is refused"
            ))
        } else {
            Verdict::Allow
        };
        (v, None)
    });
    finish(agent, &verdict, observe, root.as_deref())
}

fn finish(agent: Agent, verdict: &Verdict, observe: bool, root: Option<&Path>) -> HookOutput {
    match (verdict, observe) {
        (Verdict::Deny(reason), true) => {
            if let Some(dir) = root.and_then(|r| crate::gitctx::discover_repository(r).ok()) {
                let log = dir.path().join("discipline").join("hook-observe.log");
                let entry = serde_json::json!({
                    "time": crate::lease::now(),
                    "agent": agent.id(),
                    "event": "pre-tool",
                    "verdict": "deny",
                    "reason": reason,
                });
                let written = std::fs::create_dir_all(log.parent().unwrap_or(Path::new(".")))
                    .and_then(|_| {
                        use std::io::Write;
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&log)
                            .and_then(|mut f| writeln!(f, "{entry}"))
                    });
                if let Err(e) = written {
                    return HookOutput {
                        stdout: String::new(),
                        stderr: format!("discipline (observe mode): would refuse: {reason}\n(could not write {}: {e})\n", log.display()),
                        code: 0,
                    };
                }
            }
            HookOutput {
                stdout: String::new(),
                stderr: format!("discipline (observe mode): would refuse: {reason}\n"),
                code: 0,
            }
        }
        _ => answer(agent, verdict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/pretool/{rel}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn each_recorded_payload_parses_to_its_tool_target_and_session() {
        let cases = [
            (
                Agent::ClaudeCode,
                "claude-code/write.json",
                "Write",
                true,
                "/work/repo/a.txt",
            ),
            (
                Agent::Copilot,
                "copilot/create.json",
                "create",
                true,
                "/work/repo/a.txt",
            ),
            (
                Agent::Agy,
                "agy/write_to_file.json",
                "write_to_file",
                true,
                "/work/repo/a.txt",
            ),
            (
                Agent::Opencode,
                "opencode/write.json",
                "write",
                true,
                "/work/repo/a.txt",
            ),
        ];
        for (agent, rel, tool, edits, target) in cases {
            let c = parse(agent, &fixture(rel)).unwrap();
            assert_eq!((c.tool.as_str(), c.edits), (tool, edits), "{rel}");
            assert_eq!(c.targets, vec![target.to_string()], "{rel}");
            assert!(c.session.is_some(), "{rel}");
        }
        let bash = parse(Agent::ClaudeCode, &fixture("claude-code/bash.json")).unwrap();
        assert!(!bash.edits, "a shell command is not an edit tool");
        assert!(parse(Agent::Cursor, "{}").is_none());
    }

    #[test]
    fn apply_patch_targets_are_its_file_lines() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: /w/b.rs\n*** Delete File: c.rs\n*** End Patch\n";
        assert_eq!(patch_targets(patch), vec!["src/a.rs", "/w/b.rs", "c.rs"]);
    }

    fn scene_with(dirs: &[(&str, &Path)], here: &str) -> Worktrees {
        Worktrees {
            all: dirs
                .iter()
                .map(|(k, p)| (k.to_string(), p.canonicalize().unwrap()))
                .collect(),
            here: here.to_string(),
        }
    }

    fn edit(target: &str, session: &str) -> ToolCall {
        ToolCall {
            tool: "Write".into(),
            edits: true,
            targets: vec![target.to_string()],
            session: Some(session.to_string()),
            cwd: None,
        }
    }

    #[test]
    fn an_edit_in_another_worktree_is_refused_and_its_own_is_allowed() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main");
        let nested = main.join(".claude/worktrees/other");
        std::fs::create_dir_all(&nested).unwrap();
        let wts = scene_with(&[("main", &main), ("other", &nested)], "main");
        let scene = Scene {
            worktrees: &wts,
            leases: &[],
            forbidden: None,
        };
        let cwd = main.canonicalize().unwrap();
        assert_eq!(
            judge(&edit("src/lib.rs", "s1"), &cwd, &scene),
            Verdict::Allow
        );
        // The nested worktree is the deepest root: an edit there is another worktree's.
        let other = nested.join("src/lib.rs");
        let v = judge(&edit(other.to_str().unwrap(), "s1"), &cwd, &scene);
        assert!(
            matches!(&v, Verdict::Deny(r) if r.contains("worktree `other`")),
            "{v:?}"
        );
        // So is a relative path that climbs into it.
        let v = judge(&edit(".claude/worktrees/other/x", "s1"), &cwd, &scene);
        assert!(matches!(v, Verdict::Deny(_)));
        // Outside the repository: not refused.
        let outside = d.path().join("scratch/notes.md");
        assert_eq!(
            judge(&edit(outside.to_str().unwrap(), "s1"), &cwd, &scene),
            Verdict::Allow
        );
        // A read, or a shell command, is not an edit.
        let read = ToolCall {
            edits: false,
            ..edit(other.to_str().unwrap(), "s1")
        };
        assert_eq!(judge(&read, &cwd, &scene), Verdict::Allow);
        // An edit whose target cannot be read is refused.
        let blind = ToolCall {
            targets: vec![],
            ..edit("", "s1")
        };
        assert!(matches!(judge(&blind, &cwd, &scene), Verdict::Deny(_)));
    }

    #[test]
    fn a_worktree_leased_by_another_session_is_refused_to_this_one() {
        let d = tempfile::tempdir().unwrap();
        let wts = scene_with(&[("main", d.path())], "main");
        let lease = |session: &str| Lease {
            agent: "copilot".into(),
            session: session.into(),
            worktree: "/w".into(),
            branches: vec![],
            taken_at: 0,
            heartbeat: 0,
            ttl_secs: 60,
        };
        let cwd = d.path().canonicalize().unwrap();
        let leases = vec![("main".to_string(), lease("s-other"))];
        let scene = Scene {
            worktrees: &wts,
            leases: &leases,
            forbidden: None,
        };
        let v = judge(&edit("a.txt", "s-me"), &cwd, &scene);
        assert!(
            matches!(&v, Verdict::Deny(r) if r.contains("leased by copilot session s-other")),
            "{v:?}"
        );
        assert_eq!(
            judge(&edit("a.txt", "s-other"), &cwd, &scene),
            Verdict::Allow
        );
        // A lease that names no session claims no session.
        let leases = vec![("main".to_string(), lease(""))];
        let scene = Scene {
            worktrees: &wts,
            leases: &leases,
            forbidden: None,
        };
        assert_eq!(judge(&edit("a.txt", "s-me"), &cwd, &scene), Verdict::Allow);
    }

    #[test]
    fn forbidden_paths_are_refused_before_the_edit() {
        let d = tempfile::tempdir().unwrap();
        let wts = scene_with(&[("main", d.path())], "main");
        let f = crate::guards::PathFilter::new(&[".github/workflows/**".to_string()]).unwrap();
        let scene = Scene {
            worktrees: &wts,
            leases: &[],
            forbidden: Some(&f),
        };
        let cwd = d.path().canonicalize().unwrap();
        assert!(matches!(
            judge(&edit(".github/workflows/ci.yml", "s"), &cwd, &scene),
            Verdict::Deny(_)
        ));
        assert_eq!(judge(&edit("src/ci.rs", "s"), &cwd, &scene), Verdict::Allow);
    }

    #[test]
    fn each_agent_denies_in_its_recorded_shape() {
        let deny = Verdict::Deny("no".into());
        let c = answer(Agent::ClaudeCode, &deny);
        assert_eq!((c.code, c.stderr.as_str()), (2, "no\n"));
        let p = answer(Agent::Copilot, &deny);
        let j: serde_json::Value = serde_json::from_str(&p.stdout).unwrap();
        assert_eq!(
            (p.code, j["permissionDecision"].as_str()),
            (0, Some("deny"))
        );
        let a = answer(Agent::Agy, &deny);
        let j: serde_json::Value = serde_json::from_str(&a.stdout).unwrap();
        assert_eq!((a.code, j["decision"].as_str()), (0, Some("deny")));
        assert_eq!(answer(Agent::Opencode, &deny).code, 1);
        assert_eq!(answer(Agent::ClaudeCode, &Verdict::Allow).code, 0);
    }
}
