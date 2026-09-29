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
    /// The command a shell tool runs (Claude Code `Bash`, Copilot `bash`, agy
    /// `run_command`, OpenCode `bash`).
    pub command: Option<String>,
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
            if matches!(call.tool.as_str(), "Bash" | "run_shell_command" | "shell") {
                call.command = s(&v, "/tool_input/command");
            }
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
            if call.tool == "bash" {
                call.command = s(&v, "/toolArgs/command");
            }
        }
        Agent::Agy => {
            call.tool = s(&v, "/toolCall/name").unwrap_or_default();
            let target = s(&v, "/toolCall/args/TargetFile");
            call.edits = target.is_some();
            call.targets.extend(target);
            call.session = s(&v, "/conversationId");
            call.cwd = s(&v, "/workspacePaths/0").map(PathBuf::from);
            if call.tool == "run_command" {
                call.command = s(&v, "/toolCall/args/CommandLine");
                // The command runs in its own `Cwd`.
                call.cwd = s(&v, "/toolCall/args/Cwd").map(PathBuf::from).or(call.cwd);
            }
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
            if call.tool == "bash" {
                call.command = s(&v, "/output/args/command");
            }
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
    /// The branch checked out in this session's worktree (a push without a refspec
    /// pushes it).
    pub branch: Option<&'a str>,
}

/// Decide whether `call` may run.
pub fn judge(call: &ToolCall, cwd: &Path, scene: &Scene) -> Verdict {
    if let Some(cmd) = &call.command {
        return judge_shell(cmd, cwd, scene);
    }
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
    run_with(agent, stdin, observe, false)
}

/// The answer that lets a call through: empty, which every agent's pre-tool contract
/// reads as allow.
fn silent_pass() -> HookOutput {
    HookOutput {
        stdout: String::new(),
        stderr: String::new(),
        code: 0,
    }
}

/// [`run`], and with `if_configured` a silent pass unless the payload's directory (else
/// the working directory) is in a git repository with a `discipline.toml` at its root:
/// the guard of a user-level hook, which runs in every folder the agent opens.
pub fn run_with(agent: Agent, stdin: &str, observe: bool, if_configured: bool) -> HookOutput {
    if if_configured && !crate::hook::configured(&payload_dir(stdin)) {
        return silent_pass();
    }
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
        if !call.edits && call.command.is_none() {
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
            branch: here.branch.as_deref(),
        };
        Ok((judge(&call, &cwd, &scene), Some(here.root)))
    })();
    let (verdict, root) = decided.unwrap_or_else(|e| {
        let v = if call.edits || call.command.is_some() {
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

/// The literal text of a shell word, or `None` when it expands at run time (a variable,
/// a command substitution, a glob): such a word cannot be resolved before the command
/// runs.
fn literal(node: tree_sitter::Node, src: &str) -> Option<String> {
    let text = node.utf8_text(src.as_bytes()).ok()?;
    match node.kind() {
        "word" | "number" => {
            (!text.contains(['*', '?', '[', '~', '$', '`'])).then(|| text.to_string())
        }
        "raw_string" => Some(text.trim_matches('\'').to_string()),
        "string" => {
            let mut out = String::new();
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                if child.kind() != "string_content" {
                    return None;
                }
                out.push_str(child.utf8_text(src.as_bytes()).ok()?);
            }
            Some(out)
        }
        "concatenation" => {
            let mut out = String::new();
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                out.push_str(&literal(child, src)?);
            }
            Some(out)
        }
        _ => None,
    }
}

/// Git subcommands that only read: running them against another worktree changes
/// nothing there.
const GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "show",
    "diff",
    "rev-parse",
    "ls-files",
    "blame",
    "grep",
    "describe",
    "shortlog",
    "cat-file",
    "rev-list",
    "reflog",
    "worktree",
    "remote",
    "config",
];

/// Decide whether a shell command may run: it must not `cd` into another worktree, run
/// git against one (`-C`, `--git-dir`, `--work-tree`) other than to read, redirect output
/// into one, or force-push a branch another worktree leases. A command that does not
/// parse, or whose path in one of those positions expands at run time, is refused.
pub fn judge_shell(cmd: &str, cwd: &Path, scene: &Scene) -> Verdict {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return Verdict::Deny(
            "discipline could not load its shell parser; the command is refused".into(),
        );
    }
    let Some(tree) = parser.parse(cmd, None) else {
        return Verdict::Deny(
            "discipline could not parse this shell command, so it is refused".into(),
        );
    };
    if tree.root_node().has_error() {
        return Verdict::Deny(
            "discipline could not parse this shell command, so it cannot tell where it acts; it is refused".into(),
        );
    }
    let here = scene
        .worktrees
        .all
        .iter()
        .find(|(k, _)| *k == scene.worktrees.here)
        .map(|(k, _)| k.clone());
    // The worktree a path is in when it is not this session's, else None.
    let foreign = |p: &str, dir: &Path| -> Option<(String, PathBuf)> {
        let path = resolve(p, dir);
        let (key, root) = scene.worktrees.owner(&path)?;
        (Some(key) != here.as_ref()).then(|| (key.clone(), root.clone()))
    };
    let dynamic = |what: &str| {
        Verdict::Deny(format!(
            "the {what} in this command expands when it runs, so discipline cannot tell which worktree it acts on; it is refused. Use a literal path"
        ))
    };
    let other_wt = |what: &str, key: &str, root: &Path| {
        Verdict::Deny(format!(
            "this command {what} worktree `{key}` ({}), not this session's worktree `{}`. Each session works only in its own worktree; ask the session working in `{key}`, or use a worktree of your own",
            root.display(),
            scene.worktrees.here
        ))
    };
    // Commands in source order; `cd` moves the directory the later ones run in.
    let mut dir = cwd.to_path_buf();
    let mut stack = vec![tree.root_node()];
    let mut nodes = Vec::new();
    while let Some(n) = stack.pop() {
        nodes.push(n);
        let mut c = n.walk();
        let children: Vec<_> = n.named_children(&mut c).collect();
        stack.extend(children.into_iter().rev());
    }
    for n in nodes {
        match n.kind() {
            "file_redirect" => {
                let is_write = n
                    .utf8_text(cmd.as_bytes())
                    .map(|t| {
                        t.trim_start_matches(char::is_numeric).starts_with('>')
                            || t.starts_with("&>")
                    })
                    .unwrap_or(false);
                if !is_write {
                    continue;
                }
                let Some(dest) = n.child_by_field_name("destination") else {
                    continue;
                };
                let Some(target) = literal(dest, cmd) else {
                    return dynamic("redirect target");
                };
                if target.starts_with("/dev/") {
                    continue;
                }
                if let Some((key, root)) = foreign(&target, &dir) {
                    return other_wt("writes into", &key, &root);
                }
            }
            "command" => {
                let Some(name) = n.child_by_field_name("name") else {
                    continue;
                };
                let Some(name) = name.named_child(0).and_then(|w| literal(w, cmd)) else {
                    continue;
                };
                let mut c = n.walk();
                let args: Vec<tree_sitter::Node> =
                    n.children_by_field_name("argument", &mut c).collect();
                let arg = |i: usize| args.get(i).map(|a| literal(*a, cmd));
                match name.as_str() {
                    "cd" | "pushd" => {
                        let Some(target) = arg(0) else {
                            continue;
                        };
                        let Some(target) = target else {
                            return dynamic("`cd` directory");
                        };
                        if target == "-" {
                            continue;
                        }
                        if let Some((key, root)) = foreign(&target, &dir) {
                            return other_wt("changes into", &key, &root);
                        }
                        dir = resolve(&target, &dir);
                    }
                    "git" => {
                        let mut i = 0;
                        let mut git_dir = dir.clone();
                        let mut elsewhere: Option<(String, PathBuf)> = None;
                        let mut sub = None;
                        while i < args.len() {
                            let Some(a) = arg(i).flatten() else {
                                if sub.is_none() {
                                    return dynamic("git option");
                                }
                                i += 1;
                                continue;
                            };
                            if sub.is_some() {
                                i += 1;
                                continue;
                            }
                            let value = |j: usize| arg(j).flatten();
                            let (opt, val) = match a.split_once('=') {
                                Some((o, v)) if o.starts_with("--") => {
                                    (o.to_string(), Some(v.to_string()))
                                }
                                _ => (a.clone(), None),
                            };
                            match opt.as_str() {
                                "-C" | "--git-dir" | "--work-tree" => {
                                    let v = match val {
                                        Some(v) => v,
                                        None => {
                                            i += 1;
                                            match value(i) {
                                                Some(v) => v,
                                                None => return dynamic("git directory"),
                                            }
                                        }
                                    };
                                    if let Some(f) = foreign(&v, &git_dir) {
                                        elsewhere = Some(f);
                                    }
                                    if opt == "-C" {
                                        git_dir = resolve(&v, &git_dir);
                                    }
                                }
                                o if o.starts_with('-') => {}
                                _ => sub = Some(a.clone()),
                            }
                            i += 1;
                        }
                        let sub = sub.unwrap_or_default();
                        if let Some((key, root)) = elsewhere {
                            if !GIT_READ_ONLY.contains(&sub.as_str()) {
                                return other_wt(&format!("runs `git {sub}` in"), &key, &root);
                            }
                        }
                        if sub == "push" {
                            if let Some(v) = judge_push(&args, cmd, scene) {
                                return v;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Verdict::Allow
}

/// A force push (`-f`, `--force`, `--force-with-lease`, a `+` refspec) of a branch
/// another worktree's live lease claims. Without a refspec the push is of this
/// worktree's branch.
fn judge_push(args: &[tree_sitter::Node], cmd: &str, scene: &Scene) -> Option<Verdict> {
    let words: Vec<Option<String>> = args.iter().map(|a| literal(*a, cmd)).collect();
    let after: Vec<&Option<String>> = words
        .iter()
        .skip_while(|w| w.as_deref() != Some("push"))
        .skip(1)
        .collect();
    let mut force = false;
    let mut positional = Vec::new();
    for w in &after {
        match w.as_deref() {
            None => return Some(Verdict::Deny(
                "a `git push` argument in this command expands when it runs, so discipline cannot tell which branch it pushes; it is refused".into(),
            )),
            Some("-f" | "--force" | "--force-if-includes") => force = true,
            Some(o) if o.starts_with("--force-with-lease") => force = true,
            Some(o) if o.starts_with('-') => {}
            Some(p) => positional.push(p.to_string()),
        }
    }
    let mut branches: Vec<String> = Vec::new();
    for spec in positional.iter().skip(1) {
        let plus = spec.starts_with('+');
        let spec = spec.trim_start_matches('+');
        let dst = spec.rsplit_once(':').map_or(spec, |(_, d)| d);
        let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
        if force || plus {
            branches.push(dst.to_string());
        }
    }
    if positional.len() <= 1 && force {
        branches.extend(scene.branch.map(str::to_string));
    }
    for b in branches {
        if let Some((key, lease)) = scene
            .leases
            .iter()
            .find(|(k, l)| *k != scene.worktrees.here && l.branches.contains(&b))
        {
            return Some(Verdict::Deny(format!(
                "this command force-pushes `{b}`, which worktree `{key}` has leased ({} session {}). Hand the work over, or take the branch with `discipline lease take --branch {b} --steal`",
                lease.agent,
                if lease.session.is_empty() { "-" } else { &lease.session }
            )));
        }
    }
    None
}

/// The session a session-start payload names, and the directory it starts in (the
/// shapes recorded live: `tests/fixtures/pretool/*/session_start.json`). OpenCode's plugin
/// sends `{"input": {"sessionID": ...}, "cwd": ...}` from its `session.created` event.
pub fn parse_session_start(agent: Agent, raw: &str) -> Option<(String, Option<PathBuf>)> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let (session, cwd) = match agent {
        Agent::ClaudeCode | Agent::Codex | Agent::Qwen => (s(&v, "/session_id"), s(&v, "/cwd")),
        Agent::Copilot => (s(&v, "/sessionId"), s(&v, "/cwd")),
        Agent::Agy => (s(&v, "/conversationId"), s(&v, "/workspacePaths/0")),
        Agent::Opencode => (s(&v, "/input/sessionID"), s(&v, "/cwd")),
        Agent::Cursor | Agent::Aider => (None, None),
    };
    Some((session?, cwd.map(PathBuf::from)))
}

/// `hook run --event session-start`: take this worktree's lease for the session, on the
/// branch checked out there, so later tool calls from another session are refused and
/// this one's refresh the heartbeat. It never blocks a session from starting: every
/// outcome passes, and what it could not do is said on stderr.
///
/// - A live lease of another session on this worktree is left alone (said).
/// - A branch another worktree's live lease claims is not taken; the worktree is still
///   leased to this session, without it (said).
pub fn session_start(agent: Agent, stdin: &str) -> HookOutput {
    session_start_with(agent, stdin, false)
}

/// The directory a hook payload names (`cwd`, agy's first workspace path, or the tool
/// call's `Cwd`), else the working directory: where `--if-configured` looks for a
/// `discipline.toml`.
fn payload_dir(stdin: &str) -> PathBuf {
    serde_json::from_str::<serde_json::Value>(stdin)
        .ok()
        .and_then(|v| {
            ["/cwd", "/workspacePaths/0", "/toolCall/args/Cwd"]
                .iter()
                .find_map(|p| s(&v, p))
        })
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// [`session_start`], and with `if_configured` a silent pass outside a repository with a
/// `discipline.toml`, so a user-level hook takes no lease in a repository that has not
/// adopted discipline.
pub fn session_start_with(agent: Agent, stdin: &str, if_configured: bool) -> HookOutput {
    if if_configured && !crate::hook::configured(&payload_dir(stdin)) {
        return silent_pass();
    }
    let pass = |note: String| HookOutput {
        stdout: String::new(),
        stderr: note,
        code: 0,
    };
    let Some((session, cwd)) = parse_session_start(agent, stdin) else {
        return pass(String::new());
    };
    let dir = cwd
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| PathBuf::from("."));
    if crate::gitctx::discover_repository(&dir).is_err() {
        return pass(String::new());
    }
    let result = (|| -> anyhow::Result<String> {
        let (store, here) = crate::lease::open(&dir)?;
        let now = crate::lease::now();
        if let Some((_, held)) = store.list()?.into_iter().find(|(k, _)| *k == here.key) {
            if held.is_live(now) && !held.session.is_empty() && held.session != session {
                return Ok(format!(
                    "discipline: worktree `{}` is leased by {} session {}; this session did not take it, and its edits here will be refused until that lease is released or goes stale\n",
                    here.key, held.agent, held.session
                ));
            }
        }
        let lease = |branches: Vec<String>| Lease {
            agent: agent.id().to_string(),
            session: session.clone(),
            worktree: here.root.display().to_string(),
            branches,
            taken_at: now,
            heartbeat: now,
            ttl_secs: crate::lease::DEFAULT_TTL_SECS,
        };
        let branches: Vec<String> = here.branch.iter().cloned().collect();
        match store.take(&here.key, lease(branches.clone()), now, false) {
            Ok(_) => Ok(String::new()),
            Err(e) if !branches.is_empty() => {
                store.take(&here.key, lease(Vec::new()), now, false)?;
                Ok(format!(
                    "discipline: this session leases worktree `{}` but not its branch: {e:#}\n",
                    here.key
                ))
            }
            Err(e) => Err(e),
        }
    })();
    pass(match result {
        Ok(note) => note,
        Err(e) => format!("discipline: could not take this worktree's lease: {e:#}\n"),
    })
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
    fn each_recorded_session_start_parses_to_its_session_and_directory() {
        let opencode = serde_json::json!({
            "input": { "sessionID": "00000000-0000-0000-0000-000000000000" },
            "cwd": "/work/repo",
        })
        .to_string();
        for (agent, raw) in [
            (Agent::ClaudeCode, fixture("claude-code/session_start.json")),
            (Agent::Copilot, fixture("copilot/session_start.json")),
            (Agent::Agy, fixture("agy/session_start.json")),
            (Agent::Opencode, opencode),
        ] {
            let (session, cwd) =
                parse_session_start(agent, &raw).unwrap_or_else(|| panic!("{agent:?}: no session"));
            assert_eq!(session, "00000000-0000-0000-0000-000000000000", "{agent:?}");
            assert_eq!(cwd, Some(PathBuf::from("/work/repo")), "{agent:?}");
        }
        // Another agent's shape names no session for this one.
        assert_eq!(
            parse_session_start(Agent::Copilot, &fixture("claude-code/session_start.json")),
            None
        );
        assert_eq!(parse_session_start(Agent::ClaudeCode, "not json"), None);
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
            command: None,
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
            branch: None,
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
            branch: None,
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
            branch: None,
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
            branch: None,
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

    #[test]
    fn shell_commands_are_judged_by_what_they_touch() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main");
        let other = main.join("wt2");
        std::fs::create_dir_all(&other).unwrap();
        let wts = scene_with(&[("main", &main), ("wt2", &other)], "main");
        let lease = Lease {
            agent: "copilot".into(),
            session: "s2".into(),
            worktree: "/w".into(),
            branches: vec!["feat/stack".into(), "work".into()],
            taken_at: 0,
            heartbeat: 0,
            ttl_secs: 60,
        };
        let leases = vec![("wt2".to_string(), lease)];
        let scene = Scene {
            worktrees: &wts,
            leases: &leases,
            forbidden: None,
            branch: Some("work"),
        };
        let cwd = main.canonicalize().unwrap();
        let o = other.canonicalize().unwrap();
        let o = o.to_str().unwrap();
        let refused = |c: &str| matches!(judge_shell(c, &cwd, &scene), Verdict::Deny(_));
        for c in [
            "cd wt2",
            &format!("cd '{o}' && ls"),
            "true && (cd wt2/src)",
            "git -C wt2 commit -m x",
            &format!("git -C \"{o}\" reset --hard"),
            "git --git-dir=wt2/.git --work-tree=wt2 checkout -b x",
            "echo hi > wt2/escape.txt",
            "echo hi >> wt2/log",
            "cd src && echo x > ../wt2/y",
            "cd \"$OTHER\"",
            "git -C $DIR commit",
            "echo x > \"$F\"",
            "git push --force origin feat/stack",
            "git push origin +feat/stack",
            "git push -f origin HEAD:refs/heads/feat/stack",
            "git push --force-with-lease",
            "echo ( unbalanced",
        ] {
            assert!(refused(c), "allowed: {c}");
        }
        for c in [
            "ls wt2",
            "cat wt2/README.md",
            "git -C wt2 status",
            "git -C wt2 log --oneline -3",
            "cd src && cargo test",
            "echo hi > own.txt 2>/dev/null",
            "git push origin feat/stack",
            "git push --force origin feat/mine",
            "cd -",
            "grep -r 'cd wt2' .",
        ] {
            assert_eq!(judge_shell(c, &cwd, &scene), Verdict::Allow, "refused: {c}");
        }
    }

    #[test]
    fn shell_payloads_carry_their_command() {
        let bash = parse(Agent::ClaudeCode, &fixture("claude-code/bash.json")).unwrap();
        assert!(bash.command.is_some());
        let cases = [
            (
                Agent::Copilot,
                r#"{"toolName":"bash","toolArgs":{"command":"git -C wt2 status"},"sessionId":"s","cwd":"/r"}"#,
            ),
            (
                Agent::Agy,
                r#"{"toolCall":{"name":"run_command","args":{"CommandLine":"git -C wt2 status","Cwd":"/r/sub"}},"conversationId":"s","workspacePaths":["/r"]}"#,
            ),
            (
                Agent::Opencode,
                r#"{"input":{"tool":"bash","sessionID":"s"},"output":{"args":{"command":"git -C wt2 status"}},"cwd":"/r"}"#,
            ),
        ];
        for (agent, raw) in cases {
            let c = parse(agent, raw).unwrap();
            assert_eq!(c.command.as_deref(), Some("git -C wt2 status"), "{agent:?}");
        }
        let agy = parse(Agent::Agy, cases[1].1).unwrap();
        assert_eq!(
            agy.cwd.as_deref(),
            Some(Path::new("/r/sub")),
            "agy runs the command in its Cwd"
        );
    }
}
