//! Per-session hook state (`<common git dir>/discipline/sessions/<session>.json`).
//!
//! Shared state store for session-scoped agent hooks (`[hooks.verified-stop]` in `#700`
//! and `[hooks.tool-loop]` in `#701`). Lives under the repository's common git directory
//! so linked worktrees share the store and no session file is ever tracked by git.
//!
//! Safety and privacy invariants:
//! - Session IDs from untrusted hook payloads are sanitized through [`session_key`]
//!   (ASCII alphanumeric, `-`, `_`, `.`, no leading dots, capped at 96 characters).
//! - Tool arguments are never stored in session files: only SHA-256 digests of canonical
//!   tool inputs and working-tree states are persisted.
//! - Corrupt or unreadable session files recover to an empty [`SessionState`] (never
//!   granting an unearned verification pass and never blocking read-only tools).
//! - Concurrent updates serialize under `<sessions dir>/.lock` (`create_new`, stale lock
//!   recovery after 30 seconds) and write via temporary file + atomic rename.

use crate::report::gitlab::sha256_hex;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default time-to-live of a session state file without updates, in seconds.
pub const DEFAULT_TTL_SECS: i64 = 7200;

const STALE_LOCK_SECS: u64 = 30;
const LOCK_ATTEMPTS: u32 = 200;
const LOCK_RETRY_MS: u64 = 25;
const MAX_KEY_LEN: usize = 96;
const MAX_RECORDS: usize = 64;

/// One witnessed passing verification run on a specific working-tree digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub tree_digest: String,
    pub command_id: String,
    pub exit: i32,
    pub at: i64,
}

/// Per-session state persisted at `<common git dir>/discipline/sessions/<key>.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub session: String,
    pub agent: String,
    pub updated_at: i64,
    #[serde(default)]
    pub records: Vec<VerificationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_call_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tree_digest: Option<String>,
    #[serde(default)]
    pub consecutive_calls: u32,
    #[serde(default)]
    pub stop_refusals: u32,
}

impl SessionState {
    pub fn new(session: &str, agent: &str, now: i64) -> Self {
        Self {
            session: session.to_string(),
            agent: agent.to_string(),
            updated_at: now,
            records: Vec::new(),
            last_call_digest: None,
            last_tree_digest: None,
            consecutive_calls: 0,
            stop_refusals: 0,
        }
    }

    /// Whether this session has a passing verification record (`exit == 0`) for `digest`.
    pub fn has_verified(&self, digest: &str) -> bool {
        self.records
            .iter()
            .any(|r| r.exit == 0 && r.tree_digest == digest)
    }

    /// Record a verification run, keeping the most recent [`MAX_RECORDS`] entries.
    pub fn record_verification(&mut self, record: VerificationRecord) {
        self.records.retain(|r| {
            !(r.tree_digest == record.tree_digest && r.command_id == record.command_id)
        });
        self.records.push(record);
        if self.records.len() > MAX_RECORDS {
            let drop_count = self.records.len() - MAX_RECORDS;
            self.records.drain(0..drop_count);
        }
    }
}

/// Sanitizes an untrusted session ID into a safe filename stem (`<= 96` chars, no leading
/// `.`, only ASCII alphanumeric and `-_.`). Returns `None` when nothing usable remains.
pub fn session_key(raw: &str) -> Option<String> {
    let filtered: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = filtered.trim_start_matches('.').trim_matches('_');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_KEY_LEN).collect())
}

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("session: could not remove lock {}: {e}", self.0.display());
            }
        }
    }
}

/// The `<common git dir>/discipline/sessions` store.
#[derive(Debug, Clone)]
pub struct SessionStore {
    pub dir: PathBuf,
}

impl SessionStore {
    /// Open the session store for the repository containing `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = crate::gitctx::discover_repository(path)?;
        Ok(Self {
            dir: repo.commondir().join("discipline").join("sessions"),
        })
    }

    fn lock(&self) -> Result<Lock> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        let path = self.dir.join(".lock");
        for _ in 0..LOCK_ATTEMPTS {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Lock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > STALE_LOCK_SECS);
                    if stale {
                        match std::fs::remove_file(&path) {
                            Ok(()) => continue,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(e) => {
                                return Err(e).with_context(|| {
                                    format!("cannot remove stale session lock {}", path.display())
                                })
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(LOCK_RETRY_MS));
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot create {}", path.display()))
                }
            }
        }
        bail!(
            "the session lock {} is held by another process; try again",
            path.display()
        )
    }

    pub fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// Opportunistically delete expired session files older than [`DEFAULT_TTL_SECS`].
    fn prune_stale(&self, now: i64) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let expired = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<SessionState>(&raw).ok())
                .is_some_and(|s| now.saturating_sub(s.updated_at) > DEFAULT_TTL_SECS);
            if expired {
                if let Err(e) = std::fs::remove_file(&path) {
                    eprintln!(
                        "discipline: could not remove expired session file {}: {e}",
                        path.display()
                    );
                }
            }
        }
    }

    /// Read a session's state. If the file does not exist or is corrupt JSON, returns a
    /// fresh empty [`SessionState`].
    pub fn load(&self, session: &str, agent: &str, now: i64) -> Option<SessionState> {
        let key = session_key(session)?;
        let path = self.path_for(&key);
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<SessionState>(&raw).ok())
            .filter(|s| now.saturating_sub(s.updated_at) <= DEFAULT_TTL_SECS)
            .unwrap_or_else(|| SessionState::new(session, agent, now));
        Some(state)
    }

    /// Update a session's state atomically under the directory lock.
    pub fn update<F, R>(&self, session: &str, agent: &str, now: i64, f: F) -> Result<Option<R>>
    where
        F: FnOnce(&mut SessionState) -> R,
    {
        let Some(key) = session_key(session) else {
            return Ok(None);
        };
        let _lock = self.lock()?;
        self.prune_stale(now);
        let path = self.path_for(&key);
        let mut state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<SessionState>(&raw).ok())
            .filter(|s| now.saturating_sub(s.updated_at) <= DEFAULT_TTL_SECS)
            .unwrap_or_else(|| SessionState::new(session, agent, now));
        state.session = session.to_string();
        state.agent = agent.to_string();
        state.updated_at = now;
        let out = f(&mut state);
        let tmp = self.dir.join(format!(".{key}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(&state)? + "\n")
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(Some(out))
    }
}

/// Whether `rel` is a documentation-only path excluded when `verify_paths` is empty.
pub fn is_doc_only_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".md")
        || lower.ends_with(".mdx")
        || lower.ends_with(".rst")
        || lower.starts_with("docs/")
}

/// Resolves the base tree to compare the working tree against (the merge base of `HEAD`
/// with `base_ref` or the repository's default branch, falling back to `HEAD`'s tree when
/// no separate base branch exists).
fn resolve_base_tree<'a>(
    repo: &'a git2::Repository,
    base_ref: Option<&str>,
) -> Result<git2::Tree<'a>> {
    let head_commit = repo
        .head()
        .context("cannot resolve HEAD")?
        .peel_to_commit()
        .context("cannot peel HEAD to commit")?;
    let head_id = head_commit.id();
    let named = match base_ref {
        Some(b) if !b.trim().is_empty() => b.to_string(),
        _ if crate::gitctx::environment_names_base() => {
            crate::gitctx::detect_base_ref(None, None, None)
        }
        _ => crate::hook::change_base(repo).unwrap_or_else(|| "origin/main".to_string()),
    };
    let other = match named.strip_prefix("origin/") {
        Some(local) => local.to_string(),
        None => format!("origin/{named}"),
    };
    if let Some(merge_base) = [named, other].iter().find_map(|name| {
        let commit = repo.revparse_single(name).ok()?.peel_to_commit().ok()?;
        repo.merge_base(commit.id(), head_id).ok()
    }) {
        if let Ok(commit) = repo.find_commit(merge_base) {
            if let Ok(tree) = commit.tree() {
                return Ok(tree);
            }
        }
    }
    head_commit.tree().context("cannot read HEAD tree")
}

/// Computes the working-tree digest over changed and untracked in-scope files relative to
/// the base ref. Returns `Ok(None)` when no in-scope files have changed (clean tree or
/// documentation-only change).
pub fn tree_digest(
    dir: &Path,
    base_ref: Option<&str>,
    verify_paths: &[String],
) -> Result<Option<String>> {
    let repo = crate::gitctx::discover_repository(dir)?;
    let workdir = repo
        .workdir()
        .context("bare repository has no working tree")?;
    let base_tree = resolve_base_tree(&repo, base_ref)?;
    let filter = if verify_paths.is_empty() {
        None
    } else {
        Some(crate::guards::PathFilter::new(verify_paths)?)
    };

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(false);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut opts))
        .context("cannot diff working tree against base")?;

    let mut entries: Vec<(String, git2::Delta)> = Vec::new();
    for delta in diff.deltas() {
        let Some(p) = delta.new_file().path().or_else(|| delta.old_file().path()) else {
            continue;
        };
        let rel = p.to_string_lossy().replace('\\', "/");
        let in_scope = match &filter {
            Some(f) => f.matches(&rel),
            None => !is_doc_only_path(&rel),
        };
        if in_scope {
            entries.push((rel, delta.status()));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);

    let mut buf = Vec::new();
    for (rel, status) in entries {
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(format!("{status:?}").as_bytes());
        buf.push(0);
        let full = workdir.join(&rel);
        if let Ok(bytes) = std::fs::read(&full) {
            let file_hash = sha256_hex(&bytes);
            buf.extend_from_slice(file_hash.as_bytes());
        } else {
            buf.extend_from_slice(b"missing");
        }
        buf.push(b'\n');
    }
    Ok(Some(sha256_hex(&buf)))
}

/// Canonicalizes a JSON value with lexicographically sorted object keys (`serde_json::Value`
/// uses `BTreeMap` by default when `preserve_order` is disabled, and we recursively sort
/// to guarantee key-order independence).
pub fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for k in keys {
                let key_json = serde_json::to_string(k).unwrap_or_default();
                let val_json = canonical_json(&map[k]);
                parts.push(format!("{key_json}:{val_json}"));
            }
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Computes the SHA-256 digest of `(tool_name, canonical_json(tool_input))` without
/// retaining any raw argument text.
pub fn tool_input_digest(tool_name: &str, input: &serde_json::Value) -> String {
    let canon = canonical_json(input);
    let mut bytes = Vec::with_capacity(tool_name.len() + 1 + canon.len());
    bytes.extend_from_slice(tool_name.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(canon.as_bytes());
    sha256_hex(&bytes)
}

/// Vouched verification commands from the base configuration.
///
/// Sources:
/// 1. `base_config.hooks.verified_stop.commands`
/// 2. `base_config.gates.command` (`command`, `preset`, `commands`)
/// 3. `base_config.gates.test_floor.test_command`
/// 4. If none of the above are configured on the base ref, defaults to the standard
///    test runner invocations (`cargo test`, `pytest`, `npm test`, `go test ./...`).
pub fn vouched_commands(base: &crate::config::DisciplineConfig) -> Vec<String> {
    let mut out = Vec::new();
    for cmd in &base.hooks.verified_stop.commands {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() && !out.iter().any(|c| c == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    if !out.is_empty() {
        return out;
    }
    if let Some(cmd) = base
        .gates
        .command
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push(cmd.to_string());
    }
    if let Some(preset) = base
        .gates
        .command
        .preset
        .as_deref()
        .and_then(crate::guards::presets::resolve_preset)
        .map(|p| p.default_command.to_string())
    {
        if !out.contains(&preset) {
            out.push(preset);
        }
    }
    for entry in &base.gates.command.commands {
        if let Some(cmd) = entry
            .command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !out.iter().any(|c| c == cmd) {
                out.push(cmd.to_string());
            }
        }
        if let Some(preset) = entry
            .preset
            .as_deref()
            .and_then(crate::guards::presets::resolve_preset)
            .map(|p| p.default_command.to_string())
        {
            if !out.contains(&preset) {
                out.push(preset);
            }
        }
    }
    if let Some(cmd) = base
        .gates
        .test_floor
        .test_command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !out.iter().any(|c| c == cmd) {
            out.push(cmd.to_string());
        }
    }
    if out.is_empty() {
        for default_cmd in ["cargo test", "pytest", "npm test", "go test ./..."] {
            out.push(default_cmd.to_string());
        }
    }
    out
}

/// Whether `cmd` is a single simple shell command with no pipelines, lists (`&&`, `||`, `;`),
/// backgrounding (`&`), redirects, or command substitutions.
fn is_simple_shell_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(tree) = crate::pretool::parse_shell_tree(trimmed) else {
        return false;
    };
    let root = tree.root_node();
    if root.has_error() {
        return false;
    }
    let mut walk = root.walk();
    let children: Vec<_> = root.named_children(&mut walk).collect();
    if children.len() != 1 || children[0].kind() != "command" {
        return false;
    }
    let mut stack = vec![children[0]];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "pipeline"
                | "list"
                | "subshell"
                | "command_substitution"
                | "file_redirect"
                | "heredoc_redirect"
        ) {
            return false;
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            stack.push(child);
        }
    }
    true
}

/// Checks whether a shell command `ran` exactly matches one of the `vouched` verification
/// commands by parsed argv equality (after verifying `ran` is a single simple command).
///
/// This rejects `cargo test --no-run`, `cargo test -- --list`, `cargo test filter_name`,
/// `cargo test || true`, and `true` when `cargo test` is the vouched command.
pub fn matches_vouched_command(ran: &str, vouched: &[String]) -> Option<String> {
    if !is_simple_shell_command(ran) {
        return None;
    }
    let ran_argv = crate::guards::command::split_command_line(ran).ok()?;
    if ran_argv.is_empty() {
        return None;
    }
    for v in vouched {
        let Ok(v_argv) = crate::guards::command::split_command_line(v) else {
            continue;
        };
        if !v_argv.is_empty() && ran_argv == v_argv {
            return Some(v_argv.join(" "));
        }
    }
    None
}

/// Whether command output indicates a vacuous zero-test run (e.g. `0 passed; 0 failed`
/// with no target running at least 1 test, or `no tests ran`).
pub fn output_is_vacuous(output: &str) -> bool {
    static NON_ZERO_PASSED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\b[1-9][0-9]* passed\b").expect("valid regex")
    });
    static ZERO_PASSED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b0 passed;\s*0 failed\b|\bno tests ran\b|\bran 0 tests\b")
            .expect("valid regex")
    });
    if NON_ZERO_PASSED.is_match(output) {
        return false;
    }
    ZERO_PASSED.is_match(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_sanitizes_and_rejects_dot_only_inputs() {
        assert_eq!(session_key(""), None);
        assert_eq!(session_key("."), None);
        assert_eq!(session_key(".."), None);
        assert_eq!(session_key("..."), None);
        assert_eq!(session_key("///"), None);
        assert_eq!(session_key("../etc/passwd"), Some("etc_passwd".to_string()));
        assert_eq!(
            session_key("sess-123_abc.456"),
            Some("sess-123_abc.456".to_string())
        );
        let long = "a".repeat(150);
        assert_eq!(session_key(&long).unwrap().len(), 96);
    }

    #[test]
    fn canonical_json_and_input_digest_are_key_order_independent() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"b": 1, "a": {"y": "hello", "x": [3, 2, 1]}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"a": {"x": [3, 2, 1], "y": "hello"}, "b": 1}"#).unwrap();
        let c: serde_json::Value =
            serde_json::from_str(r#"{"a": {"x": [3, 2, 2], "y": "hello"}, "b": 1}"#).unwrap();
        assert_eq!(tool_input_digest("Read", &a), tool_input_digest("Read", &b));
        assert_ne!(tool_input_digest("Read", &a), tool_input_digest("Read", &c));
        assert_ne!(tool_input_digest("Read", &a), tool_input_digest("Edit", &a));
    }

    #[test]
    fn matches_vouched_command_requires_exact_argv_and_simple_command() {
        let vouched = vec!["cargo test".to_string(), "pytest -q".to_string()];
        assert_eq!(
            matches_vouched_command("cargo test", &vouched),
            Some("cargo test".to_string())
        );
        assert_eq!(
            matches_vouched_command("  cargo   test  ", &vouched),
            Some("cargo test".to_string())
        );
        assert_eq!(
            matches_vouched_command("pytest '-q'", &vouched),
            Some("pytest -q".to_string())
        );
        // Spoof / partial / masked variants must all be rejected:
        assert_eq!(
            matches_vouched_command("cargo test --no-run", &vouched),
            None
        );
        assert_eq!(
            matches_vouched_command("cargo test -- --list", &vouched),
            None
        );
        assert_eq!(
            matches_vouched_command("cargo test nonexistent_filter", &vouched),
            None
        );
        assert_eq!(
            matches_vouched_command("cargo test || true", &vouched),
            None
        );
        assert_eq!(matches_vouched_command("cargo test | cat", &vouched), None);
        assert_eq!(matches_vouched_command("cargo test; true", &vouched), None);
        assert_eq!(matches_vouched_command("true", &vouched), None);
    }

    #[test]
    fn output_is_vacuous_rejects_zero_tests_and_accepts_real_runs() {
        assert!(output_is_vacuous(
            "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"
        ));
        assert!(!output_is_vacuous(
            "running 0 tests\ntest result: ok. 0 passed; 0 failed\nrunning 4 tests\ntest result: ok. 4 passed; 0 failed\n"
        ));
        assert!(output_is_vacuous("==== no tests ran in 0.01s ===="));
    }

    #[test]
    fn corrupt_session_file_recovers_to_empty_state() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore {
            dir: tmp.path().join("sessions"),
        };
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(store.path_for("s1"), "{not valid json").unwrap();

        let loaded = store.load("s1", "claude-code", 100).unwrap();
        assert!(loaded.records.is_empty());
        assert_eq!(loaded.consecutive_calls, 0);

        store
            .update("s1", "claude-code", 105, |s| {
                s.consecutive_calls = 2;
            })
            .unwrap();
        let after = store.load("s1", "claude-code", 106).unwrap();
        assert_eq!(after.consecutive_calls, 2);
    }
}
