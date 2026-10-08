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
//! | OpenCode    | `write`, `edit`, `apply_patch`                  | `output.args.filePath` or `.path`, or the patch's file lines | `input.sessionID` | exit 1, the plugin throws with the reason |
//! | Qwen Code   | `write_file`, `edit`, `replace`                 | `tool_input.file_path`      | `session_id`      | exit 2, reason on stderr (Claude Code's contract) |
//! | Codex       | `apply_patch`, `Edit`, `Write`                  | the patch's file lines (`tool_input.command`) | `session_id` | exit 2, reason on stderr (Claude Code's contract) |
//!
//! Cursor and Aider have no pre-tool event for edits: this check does not apply to them.
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
    /// `run_command`, OpenCode `bash`, `shell` on 2.x).
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
            // OpenCode 2.x names the target `path`; `filePath` is its legacy spelling.
            call.targets.extend(s(&v, "/output/args/path"));
            if let Some(p) = s(&v, "/output/args/patchText") {
                call.targets.extend(patch_targets(&p));
            }
            call.session = s(&v, "/input/sessionID");
            call.cwd = s(&v, "/cwd").map(PathBuf::from);
            // OpenCode 2.x names its shell tool `shell`; 1.x named it `bash`.
            if matches!(call.tool.as_str(), "bash" | "shell") {
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

/// Text from the payload, the repository or the lease store, quoted in a refusal
/// ([`crate::lease::quote`]): a path, a command word, a branch or a session id.
fn q(text: impl std::fmt::Display) -> String {
    crate::lease::quote(text)
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
            "discipline could not read which file the {} call edits, so it cannot tell whether the edit stays in this session's worktree; it is refused",
            q(&call.tool)
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
                "{} is in worktree {key} ({}), not in this session's worktree {}. Each session edits only its own worktree; ask the session working in {key} to make this change, or make it in a worktree of your own",
                q(path.display()),
                q(root.display()),
                q(&scene.worktrees.here),
                key = q(key)
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
                    "worktree {} is leased by {} session {}; this session ({}) may not edit it. Hand the work over, or take the worktree's lease with `discipline lease take --steal`",
                    q(key),
                    q(&lease.agent),
                    q(&lease.session),
                    q(call.session.as_deref().unwrap_or("-"))
                ));
            }
        }
        if let Some(f) = scene.forbidden {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if f.matches(&rel) {
                    return Verdict::Deny(format!(
                        "{} matches `scope-confinement`'s forbidden_paths; this repository does not let an agent edit it",
                        q(&rel)
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
#[cfg(test)]
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

/// `run`, and with `if_configured` a silent pass unless the payload's directory (else
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
                "discipline could not check where this edit goes ({}), so it is refused",
                q(format!("{e:#}"))
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
                        stderr: format!(
                            "discipline (observe mode): would refuse: {reason}\n(could not write {}: {})\n",
                            q(log.display()),
                            q(&e)
                        ),
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

/// The shell parser, reachable only through an ASCII copy of the command.
///
/// tree-sitter-bash 0.25.1 hands the character it is looking at, a Unicode code point,
/// to `isdigit` in its brace-range scan (`src/scanner.c`, after `{` and after
/// `{<digits>..`). `isdigit` is defined for `unsigned char` values and `EOF` only, and
/// on glibc it indexes a table: a code point above 255 reads outside it (#618). So the
/// parser is never given a character above 127. It parses a copy of the command in
/// which every byte of a non-ASCII character is a placeholder; the copy has the
/// command's length and every byte offset in it is the command's, so a node's text is
/// read from the command by the node's byte range.
///
/// The compiler holds this: `AsciiParseText` has a private field, so outside this
/// module the only way to make one is `ascii_copy_with_unchanged_offsets`, and
/// `parse`, the only function that names the grammar, takes nothing else.
mod shell_tree {
    /// What stands for each byte of a non-ASCII character in the parse copy. It is a
    /// control character because the grammar reads one the way it reads a non-ASCII
    /// character: as part of a word, and as neither a letter (which would make `é=1` an
    /// assignment and `$é` a variable), a digit (which would make `{é..3}` a brace
    /// range), whitespace, a quote nor an operator.
    pub(super) const PLACEHOLDER: u8 = 0x01;

    /// A command with every non-ASCII byte replaced: ASCII, and as long as the command.
    pub(super) struct AsciiParseText(String);

    impl AsciiParseText {
        #[cfg(test)]
        pub(super) fn as_str(&self) -> &str {
            &self.0
        }
    }

    /// `cmd` with each non-ASCII character replaced by as many placeholder bytes as its
    /// UTF-8 encoding has, so the result is ASCII and no byte offset moves.
    pub(super) fn ascii_copy_with_unchanged_offsets(cmd: &str) -> AsciiParseText {
        AsciiParseText(
            cmd.bytes()
                .map(|b| char::from(if b.is_ascii() { b } else { PLACEHOLDER }))
                .collect(),
        )
    }

    /// The syntax tree of the copy, or why there is none (the reason a caller refuses
    /// the command with).
    pub(super) fn parse(text: &AsciiParseText) -> Result<tree_sitter::Tree, &'static str> {
        debug_assert!(
            text.0.is_ascii(),
            "the bash scanner must not be given a character above 127"
        );
        let mut parser = tree_sitter::Parser::new();
        if parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .is_err()
        {
            return Err("discipline could not load its shell parser; the command is refused");
        }
        // Within the step budget every parse has: a command the parser does not finish
        // has no tree, and is refused like one it cannot read.
        crate::ast::source_text::parse_within_budget(&mut parser, text.0.as_bytes())
            .map_err(|_| "discipline could not parse this shell command, so it is refused")
    }
}

/// Whether `node` starts or ends inside a character of `cmd`. The tree comes from the
/// ASCII copy, where a character of several bytes is several placeholders: a node that
/// took only some of them has no text in the command, so the command is refused.
fn splits_a_character(node: tree_sitter::Node, cmd: &str) -> bool {
    !cmd.is_char_boundary(node.start_byte()) || !cmd.is_char_boundary(node.end_byte())
}

/// Whether `node` is a here-document's delimiter with a non-ASCII character in it. In
/// the copy two different delimiters of the same byte length are the same placeholders,
/// so the parser could end a here-document at a line the shell reads as its body. The
/// grammar keeps one byte of each character of a delimiter and compares it with a whole
/// character, so where `char` is signed it never matched such a delimiter and the
/// command did not parse; it stays refused.
fn is_non_ascii_heredoc_delimiter(node: tree_sitter::Node, cmd: &str) -> bool {
    node.kind() == "heredoc_start" && !node.utf8_text(cmd.as_bytes()).is_ok_and(|t| t.is_ascii())
}

/// Decide whether a shell command may run: it must not `cd` into another worktree, run
/// git against one (`-C`, `--git-dir`, `--work-tree`) other than to read, redirect output
/// into one, or force-push a branch another worktree leases. A command that does not
/// parse, or whose path in one of those positions expands at run time, is refused. The
/// parser reads an ASCII copy of the command (`shell_tree`); every word judged here is
/// the command's own text.
pub fn judge_shell(cmd: &str, cwd: &Path, scene: &Scene) -> Verdict {
    // The tree's byte ranges are the command's own (the copy changes no offset), so the
    // text of every node below is read from `cmd`, never from the copy.
    let tree = match shell_tree::parse(&shell_tree::ascii_copy_with_unchanged_offsets(cmd)) {
        Ok(tree) => tree,
        Err(reason) => return Verdict::Deny(reason.into()),
    };
    let unparsed = || {
        Verdict::Deny(
            "discipline could not parse this shell command, so it cannot tell where it acts; it is refused".into(),
        )
    };
    if tree.root_node().has_error() {
        return unparsed();
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
            "this command {what} worktree {key} ({}), not this session's worktree {}. Each session works only in its own worktree; ask the session working in {key}, or use a worktree of your own",
            q(root.display()),
            q(&scene.worktrees.here),
            key = q(key)
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
    if nodes
        .iter()
        .any(|n| splits_a_character(*n, cmd) || is_non_ascii_heredoc_delimiter(*n, cmd))
    {
        return unparsed();
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
                                return other_wt(
                                    &format!("runs {} in", q(format!("git {sub}"))),
                                    &key,
                                    &root,
                                );
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
            // The command to run names the branch only when the name is one plain word.
            let take = crate::lease::take_command(&b);
            return Some(Verdict::Deny(format!(
                "this command force-pushes {}, which worktree {} has leased ({} session {}). Hand the work over, or take the branch with {take}",
                q(&b),
                q(key),
                q(&lease.agent),
                q(if lease.session.is_empty() { "-" } else { &lease.session })
            )));
        }
    }
    None
}

/// The session a session-start payload names, and the directory it starts in (the
/// shapes recorded live: `tests/fixtures/pretool/*/session_start.json`). OpenCode's plugin
/// sends `{"input": {"sessionID": ...}, "cwd": ...}` from its `session.created` event (on
/// 2.x, from that or `session.execution.started`, whichever names the session first).
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
#[cfg(test)]
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

/// `session_start`, and with `if_configured` a silent pass outside a repository with a
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
                    "discipline: worktree {} is leased by {} session {}; this session did not take it, and its edits here will be refused until that lease is released or goes stale\n",
                    q(&here.key),
                    q(&held.agent),
                    q(&held.session)
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
                    "discipline: this session leases worktree {} but not its branch: {}\n",
                    q(&here.key),
                    q(format!("{e:#}"))
                ))
            }
            Err(e) => Err(e),
        }
    })();
    pass(match result {
        Ok(note) => note,
        Err(e) => format!(
            "discipline: could not take this worktree's lease: {}\n",
            q(format!("{e:#}"))
        ),
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
            matches!(&v, Verdict::Deny(r) if r.contains("leased by `copilot` session `s-other`")),
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
        let shell_v2 = fixture("opencode/shell_v2.json");
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
            // OpenCode 2.x names its shell tool `shell` (recorded: opencode/shell_v2.json).
            (Agent::Opencode, shell_v2.as_str()),
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

    fn shell_placeholder() -> u8 {
        shell_tree::PLACEHOLDER
    }

    /// The input of the fuzz run that found #618 (the `crash_wide_char_after_brace` seed
    /// of the `pretool_payload` target, which `tests/test_pretool.rs` reads): the
    /// target's selector byte, then `{`, U+8E753, four bytes that are not UTF-8, and
    /// `me \n}\n`.
    const CRASH_618: &[u8] = b"\x05{\xf2\x8e\x9d\x93\x9a\xa0\x91\x9eme \n}\n";

    /// Strings with non-ASCII characters where the grammar's scanner looks at them.
    fn non_ascii_commands() -> Vec<String> {
        let mut all: Vec<String> = [
            // Directly after `{`: the first `isdigit` of the brace-range scan.
            "echo {\u{8e753}",
            "{\u{8e753}",
            "echo {\u{100}}",
            // After `{<digits>..`, with or without digits: the second.
            "echo {..\u{8e753}",
            "echo {1..\u{8e753}",
            "echo {1..\u{8e753}}",
            "echo {12..\u{663}}",
            "{1..\u{65e5}",
            // Elsewhere.
            "echo fo\u{e9}",
            "e\u{301}cho a\u{300}\u{301}\u{302}",
            "echo '\u{fffd}' \"\u{fffd}\u{fffd}\" \u{fffd}",
            "cat <<EOF\n\u{1f389}\nEOF\n",
            "# \u{65e5}\u{672c}\u{8a9e}\nls",
            "\u{e9}",
            "\u{a0}\u{3000}\u{2028}\u{ff5b}1..3}",
        ]
        .iter()
        .map(|c| c.to_string())
        .collect();
        all.push(String::from_utf8_lossy(&CRASH_618[1..]).into_owned());
        all.push("\u{10ffff}".repeat(20_000));
        all.push(format!(
            "echo {{{}",
            "\u{e9}\u{65e5}\u{1f389}".repeat(5_000)
        ));
        all
    }

    #[test]
    fn the_parse_copy_is_ascii_and_moves_no_byte_offset() {
        for ascii in ["", "echo {1..3} > 'a b' && cd \"$X\"\n", "\x01\t\x7f"] {
            assert_eq!(
                shell_tree::ascii_copy_with_unchanged_offsets(ascii).as_str(),
                ascii
            );
        }
        let p = shell_placeholder();
        assert!(
            p.is_ascii_control() && !p.is_ascii_whitespace(),
            "a letter, a digit, whitespace, a quote or an operator would change the tree"
        );
        for cmd in non_ascii_commands() {
            let copy = shell_tree::ascii_copy_with_unchanged_offsets(&cmd);
            let copy = copy.as_str().as_bytes();
            assert!(copy.is_ascii(), "{cmd:?}");
            assert_eq!(copy.len(), cmd.len(), "{cmd:?}");
            for (at, ch) in cmd.char_indices() {
                let bytes = &copy[at..at + ch.len_utf8()];
                if ch.is_ascii() {
                    assert_eq!(bytes, [ch as u8], "{cmd:?} at {at}");
                } else {
                    assert!(bytes.iter().all(|b| *b == p), "{cmd:?} at {at}");
                }
            }
        }
    }

    /// The out-of-bounds read of #618 cannot be seen here: it faults under glibc's
    /// `isdigit` only. What is asserted is what prevents it (the text the parser gets
    /// is ASCII: `shell_tree::parse` has a `debug_assert!` that these calls go through)
    /// and that each input is still judged.
    #[test]
    fn the_crash_input_of_618_and_both_scanner_sites_are_judged_from_an_ascii_copy() {
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
        // The calls of the `pretool_payload` fuzz target, on its crash input.
        let text = String::from_utf8_lossy(&CRASH_618[1..]);
        assert_eq!(text, "{\u{8e753}\u{fffd}\u{fffd}\u{fffd}\u{fffd}me \n}\n");
        assert_eq!(parse(Agent::Copilot, &text), None);
        assert_eq!(parse_session_start(Agent::Copilot, &text), None);
        assert_eq!(patch_targets(&text), Vec::<String>::new());
        // `{<word>` and `}` are two words to the grammar, as they were before the copy.
        assert_eq!(
            judge_shell(&text, Path::new("/repo"), &scene),
            Verdict::Allow
        );
        for (cmd, allowed) in [
            ("echo {\u{8e753}", true),
            ("echo {\u{100}} > /repo/own.txt", true),
            ("echo {1..\u{8e753}", true),
            ("echo {..\u{8e753}", true),
            ("echo {1..\u{8e753}} {1..3}", true),
            ("echo {\u{8e753} > /repo/.worktrees/other/x", false),
            ("echo {1..\u{8e753} > /repo/.worktrees/other/x", false),
            ("cd /repo/.worktrees/other/{1..\u{8e753}}", false),
        ] {
            let copy = shell_tree::ascii_copy_with_unchanged_offsets(cmd);
            assert!(copy.as_str().is_ascii(), "{cmd}");
            assert!(shell_tree::parse(&copy).is_ok(), "{cmd}");
            let v = judge_shell(cmd, Path::new("/repo"), &scene);
            assert_eq!(v == Verdict::Allow, allowed, "{cmd}: {v:?}");
        }
        for cmd in non_ascii_commands() {
            // Judged without a panic, whatever the verdict.
            let v = judge_shell(&cmd, Path::new("/repo"), &scene);
            assert!(matches!(v, Verdict::Allow | Verdict::Deny(_)));
        }
    }

    /// A command whose parse is cut at its budget is refused with the reason of a
    /// command that does not parse. The budget is counted in parser steps, so a command
    /// long enough to take one step is cut with a budget of none, and read with its own.
    #[test]
    fn a_shell_command_whose_parse_is_cut_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        let wts = scene_with(&[("main", &main)], "main");
        let scene = Scene {
            worktrees: &wts,
            leases: &[],
            forbidden: None,
            branch: None,
        };
        let cwd = main.canonicalize().unwrap();
        let long = vec!["echo one two three"; 200].join(" && ");
        assert_eq!(judge_shell(&long, &cwd, &scene), Verdict::Allow);
        let cut = crate::ast::source_text::with_step_budget(0, || judge_shell(&long, &cwd, &scene));
        assert_eq!(
            cut,
            Verdict::Deny("discipline could not parse this shell command, so it is refused".into())
        );
        // The budget of the next command is its own again.
        assert_eq!(judge_shell(&long, &cwd, &scene), Verdict::Allow);
    }

    /// A command whose tree nests past the depth every parse is held to has no tree, and
    /// is refused with the reason of a command that does not parse; the deepest one read
    /// is judged as any other command. The shell reader's own walk keeps a list of the
    /// nodes left to read, so it is the bound that refuses here, not the stack.
    #[test]
    fn a_shell_command_nested_past_the_depth_limit_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        let wts = scene_with(&[("main", &main)], "main");
        let scene = Scene {
            worktrees: &wts,
            leases: &[],
            forbidden: None,
            branch: None,
        };
        let cwd = main.canonicalize().unwrap();
        // `subshells` nested subshells around one command: a tree of four levels (the
        // program, the command, its name, the word) and one for each subshell.
        let nested =
            |subshells: usize| format!("{}true{}", "( ".repeat(subshells), " )".repeat(subshells));
        let limit = crate::ast::source_text::TREE_DEPTH_LIMIT;
        let refused =
            Verdict::Deny("discipline could not parse this shell command, so it is refused".into());
        // On the deep stack, as the hook judges every command.
        crate::deep_stack::on_deep_stack(|| {
            assert_eq!(judge_shell(&nested(1), &cwd, &scene), Verdict::Allow);
            assert_eq!(
                judge_shell(&nested(limit - 4), &cwd, &scene),
                Verdict::Allow
            );
            assert_eq!(judge_shell(&nested(limit - 3), &cwd, &scene), refused);
            assert_eq!(judge_shell(&nested(20_000), &cwd, &scene), refused);
            // Command substitutions nest the same way, two levels each.
            let substituted =
                |n: usize| format!("echo {}true{}", "$(echo ".repeat(n), ")".repeat(n));
            assert_eq!(judge_shell(&substituted(20), &cwd, &scene), Verdict::Allow);
            assert_eq!(judge_shell(&substituted(20_000), &cwd, &scene), refused);
            // The command after a refused one is read on its own.
            assert_eq!(judge_shell("true", &cwd, &scene), Verdict::Allow);
        })
        .unwrap();
    }

    /// `shell_tree::parse` takes an `AsciiParseText`, whose field is private to that
    /// module, so the compiler refuses any other text. What it cannot refuse is a
    /// second parser built somewhere else: the grammar and the parser type are named
    /// once in `src/`, inside `shell_tree`.
    #[test]
    fn the_bash_grammar_is_named_only_inside_the_guarded_parser() {
        let grammar = ["tree_sitter", "_bash"].concat();
        let parser = ["Parser", "::new"].concat();
        let mut named = Vec::new();
        let mut dirs = vec![PathBuf::from(format!("{}/src", env!("CARGO_MANIFEST_DIR")))];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    named.extend(
                        text.match_indices(&grammar)
                            .map(|(at, _)| (path.clone(), text[..at].lines().count())),
                    );
                }
            }
        }
        assert_eq!(named.len(), 1, "{named:?}");
        let here = std::fs::read_to_string(&named[0].0).unwrap();
        assert!(named[0].0.ends_with("src/pretool.rs"), "{named:?}");
        let module = here
            .split_once("\nmod shell_tree {\n")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map(|(body, _)| body)
            .expect("the module");
        assert!(module.contains(&grammar));
        assert!(module.contains("fn parse(text: &AsciiParseText)"));
        assert!(module.contains("pub(super) struct AsciiParseText(String);"));
        // The only parser this file builds is that one.
        assert_eq!(here.matches(&parser).count(), 1);
        assert_eq!(module.matches(&parser).count(), 1);
    }

    #[test]
    fn a_node_that_splits_a_character_has_no_text_to_judge() {
        let copy = shell_tree::ascii_copy_with_unchanged_offsets("echo ab");
        let tree = shell_tree::parse(&copy).unwrap();
        let word = tree
            .root_node()
            .descendant_for_byte_range(5, 7)
            .expect("the argument");
        assert_eq!((word.kind(), word.byte_range()), ("word", 5..7));
        assert!(!splits_a_character(word, "echo ab"));
        assert!(!splits_a_character(word, "echo \u{e9}"));
        // A command whose character starts or ends inside the node.
        assert!(splits_a_character(word, "ech \u{e9}b"));
        assert!(splits_a_character(word, "echo a\u{e9}"));
        let heredoc = "cat <<\u{e9}\n\u{e9}\n";
        let tree =
            shell_tree::parse(&shell_tree::ascii_copy_with_unchanged_offsets(heredoc)).unwrap();
        let start = tree.root_node().descendant_for_byte_range(6, 8).unwrap();
        assert_eq!(start.kind(), "heredoc_start");
        assert!(is_non_ascii_heredoc_delimiter(start, heredoc));
        assert!(!is_non_ascii_heredoc_delimiter(start, "cat <<AB\nAB\n"));
        assert!(!is_non_ascii_heredoc_delimiter(tree.root_node(), heredoc));
    }

    /// Two worktrees whose names are not ASCII, and a branch name that is not either.
    /// `wt\u{e9}` and `wt\u{f1}` have the same length in bytes, as do the two branches.
    #[test]
    fn shell_commands_with_non_ascii_words_are_judged_by_their_own_text() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main");
        let other = main.join("wt\u{e9}");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(main.join("s\u{e9}par\u{e9}")).unwrap();
        let wts = scene_with(&[("main", &main), ("wt\u{e9}", &other)], "main");
        let lease = Lease {
            agent: "copilot".into(),
            session: "s2".into(),
            worktree: "/w".into(),
            branches: vec!["feat/\u{e9}t\u{e9}".into()],
            taken_at: 0,
            heartbeat: 0,
            ttl_secs: 60,
        };
        let leases = vec![("wt\u{e9}".to_string(), lease)];
        let scene = Scene {
            worktrees: &wts,
            leases: &leases,
            forbidden: None,
            branch: Some("work"),
        };
        let cwd = main.canonicalize().unwrap();
        let o = other.canonicalize().unwrap();
        let o = o.to_str().unwrap();
        // Refused, and for the reason given: the worktree or the branch is named as the
        // command wrote it.
        for (c, reason) in [
            (
                "cd wt\u{e9}".to_string(),
                "changes into worktree `wt\u{e9}`",
            ),
            (
                format!("cd '{o}' && ls"),
                "changes into worktree `wt\u{e9}`",
            ),
            (
                "true && (cd \"wt\u{e9}/\u{65e5}\u{672c}\")".to_string(),
                "changes into worktree `wt\u{e9}`",
            ),
            (
                "cd s\u{e9}par\u{e9} && cd ../wt\u{e9}".to_string(),
                "changes into worktree `wt\u{e9}`",
            ),
            (
                "git -C wt\u{e9} commit -m \u{1f389}".to_string(),
                "runs `git commit` in worktree `wt\u{e9}`",
            ),
            (
                format!("git -C \"{o}\" reset --hard"),
                "runs `git reset` in worktree `wt\u{e9}`",
            ),
            (
                "git --git-dir=wt\u{e9}/.git --work-tree=wt\u{e9} checkout -b x".to_string(),
                "runs `git checkout` in worktree `wt\u{e9}`",
            ),
            (
                "echo h\u{e9} > wt\u{e9}/\u{e9}chapp\u{e9}.txt".to_string(),
                "writes into worktree `wt\u{e9}`",
            ),
            (
                "echo hi >> 'wt\u{e9}/journal \u{1f389}'".to_string(),
                "writes into worktree `wt\u{e9}`",
            ),
            (
                "cd s\u{e9}par\u{e9} && echo x > ../wt\u{e9}/y".to_string(),
                "writes into worktree `wt\u{e9}`",
            ),
            (
                "git push --force origin feat/\u{e9}t\u{e9}".to_string(),
                "force-pushes `feat/\u{e9}t\u{e9}`, which worktree `wt\u{e9}` has leased",
            ),
            (
                "git push origin +feat/\u{e9}t\u{e9}".to_string(),
                "force-pushes `feat/\u{e9}t\u{e9}`",
            ),
            (
                "git push -f origin HEAD:refs/heads/feat/\u{e9}t\u{e9}".to_string(),
                "force-pushes `feat/\u{e9}t\u{e9}`",
            ),
            (
                "echo \u{e9} > \"$F\u{e9}\"".to_string(),
                "redirect target in this command expands when it runs",
            ),
            (
                "echo \u{e9} ( unbalanced".to_string(),
                "could not parse this shell command",
            ),
        ] {
            let v = judge_shell(&c, &cwd, &scene);
            assert!(
                matches!(&v, Verdict::Deny(r) if r.contains(reason)),
                "`{c}`: {v:?}"
            );
            assert!(
                !format!("{v:?}").contains(char::from(shell_placeholder())),
                "`{c}` reports a placeholder: {v:?}"
            );
        }
        for c in [
            "ls wt\u{e9}",
            "cat wt\u{e9}/LISEZ-MOI.md",
            "git -C wt\u{e9} status",
            "git -C wt\u{e9} log --oneline -3",
            "cd s\u{e9}par\u{e9} && cargo test",
            // The same length in bytes as the other worktree's name, and not it.
            "cd wt\u{f1}",
            "echo hi > wt\u{f1}/own.txt",
            "git -C wt\u{f1} commit -m x",
            "echo h\u{e9}llo > own-\u{e9}.txt 2>/dev/null",
            "echo '\u{65e5}\u{672c}\u{8a9e}' >> \"notes \u{1f389}.md\"",
            "git push origin feat/\u{e9}t\u{e9}",
            // The same length in bytes as the leased branch, and not it.
            "git push --force origin feat/\u{e8}t\u{e8}",
            "grep -r 'cd wt\u{e9}' .",
            "echo \u{fffd} # cd wt\u{e9}",
            "\u{e9}=1 ls",
        ] {
            assert_eq!(judge_shell(c, &cwd, &scene), Verdict::Allow, "refused: {c}");
        }
    }

    /// The parser sees `\u{e9}` and `\u{f1}` as the same two placeholders. Were the
    /// delimiter matched in the copy, the here-document below would end at the `\u{f1}`
    /// line, and the `cd` the shell runs would be read as the inside of a string.
    #[test]
    fn a_here_document_with_a_non_ascii_delimiter_stays_refused() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main");
        let other = main.join("wt2");
        std::fs::create_dir_all(&other).unwrap();
        let wts = scene_with(&[("main", &main), ("wt2", &other)], "main");
        let scene = Scene {
            worktrees: &wts,
            leases: &[],
            forbidden: None,
            branch: None,
        };
        let cwd = main.canonicalize().unwrap();
        for c in [
            "cat <<\u{e9}\nbody\n\u{e9}\n",
            "cat <<-'\u{65e5}'\n\tbody\n\t\u{65e5}\n",
            "cat <<\u{e9}\n\u{f1}\necho \"\n\u{e9}\ncd wt2 #\"\n",
        ] {
            let v = judge_shell(c, &cwd, &scene);
            assert!(
                matches!(&v, Verdict::Deny(r) if r.contains("could not parse this shell command")),
                "{c:?}: {v:?}"
            );
        }
        // An ASCII delimiter with a non-ASCII body is read as before: the body is not a
        // command, and what follows the here-document is.
        assert_eq!(
            judge_shell("cat <<EOF\ncd wt2 \u{e9}\nEOF\nls \u{e9}\n", &cwd, &scene),
            Verdict::Allow
        );
        let v = judge_shell("cat <<EOF\n\u{e9}\nEOF\ncd wt2\n", &cwd, &scene);
        assert!(
            matches!(&v, Verdict::Deny(r) if r.contains("changes into worktree `wt2`")),
            "{v:?}"
        );
    }

    /// The edit checks do not go through the shell parser; their targets with
    /// non-ASCII names are judged, and reported, as written.
    #[test]
    fn edits_with_non_ascii_targets_are_judged_and_reported_as_written() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("main");
        let other = main.join("wt\u{e9}");
        std::fs::create_dir_all(&other).unwrap();
        let wts = scene_with(&[("main", &main), ("wt\u{e9}", &other)], "main");
        let f = crate::guards::PathFilter::new(&["d\u{e9}ploiement/**".to_string()]).unwrap();
        let lease = Lease {
            agent: "copilot".into(),
            session: "s-other".into(),
            worktree: "/w".into(),
            branches: vec![],
            taken_at: 0,
            heartbeat: 0,
            ttl_secs: 60,
        };
        let cwd = main.canonicalize().unwrap();
        let scene = Scene {
            worktrees: &wts,
            leases: &[],
            forbidden: Some(&f),
            branch: None,
        };
        let v = judge(&edit("wt\u{e9}/r\u{e9}sum\u{e9}.txt", "s"), &cwd, &scene);
        assert!(
            matches!(&v, Verdict::Deny(r) if r.contains("r\u{e9}sum\u{e9}.txt` is in worktree `wt\u{e9}`")),
            "{v:?}"
        );
        let v = judge(&edit("d\u{e9}ploiement/\u{65e5}.yml", "s"), &cwd, &scene);
        assert!(
            matches!(&v, Verdict::Deny(r) if r.contains("`d\u{e9}ploiement/\u{65e5}.yml` matches")),
            "{v:?}"
        );
        assert_eq!(
            judge(&edit("d\u{e8}ploiement/\u{65e5}.yml", "s"), &cwd, &scene),
            Verdict::Allow
        );
        let leases = vec![("main".to_string(), lease)];
        let leased = Scene {
            worktrees: &wts,
            leases: &leases,
            forbidden: None,
            branch: None,
        };
        let v = judge(&edit("r\u{e9}sum\u{e9}.txt", "s-me"), &cwd, &leased);
        assert!(
            matches!(&v, Verdict::Deny(r) if r.contains("leased by `copilot` session `s-other`")),
            "{v:?}"
        );
        assert_eq!(
            judge(&edit("r\u{e9}sum\u{e9}.txt", "s-other"), &cwd, &leased),
            Verdict::Allow
        );
    }
}
