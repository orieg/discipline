//! Worktree and branch leases (`discipline lease`): which session works where.
//!
//! Several coding agents often share one repository, each in its own git worktree. Git
//! already refuses to check the same branch out twice, but it does not stop one
//! worktree from moving a branch another is working on: `git rebase --update-refs`,
//! `branch -f` or `reset` in one worktree can rewrite a stacked branch that a second
//! session is about to push. A lease records, per worktree, the agent and session
//! working there and the branches it claims, with a heartbeat. The reference guard
//! (Phase 13 Step 1) and the pre-tool check (Step 3) read them.
//!
//! Leases live in the repository's common git directory
//! (`<common git dir>/discipline/leases/<worktree>.json`), so every worktree sees the
//! same set and none is ever committed. They are cooperative: they stop one session from
//! stepping on another by mistake, not an adversary who deletes the file.
//!
//! - A lease is **live** while its heartbeat is within its time-to-live; a stale lease
//!   is shown by `lease list` but claims nothing.
//! - Taking a branch that another worktree's live lease claims is refused, unless
//!   `--steal`, which removes the branch from that lease and says so. Never silently.
//! - Every change is made under a lock file (`create_new`), so two concurrent takes
//!   cannot both win.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default time-to-live of a lease without a heartbeat, in seconds.
pub const DEFAULT_TTL_SECS: u64 = 7200;

/// A lock file older than this many seconds was left by a process that died; it is
/// removed.
const STALE_LOCK_SECS: u64 = 30;

/// Attempts to take the lock, with [`LOCK_RETRY_MS`] between them.
const LOCK_ATTEMPTS: u32 = 200;
const LOCK_RETRY_MS: u64 = 25;

/// One worktree's lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// The agent working there (`claude-code`, `copilot`, ...), as the taker states it.
    pub agent: String,
    /// The agent's session id, when it has one.
    #[serde(default)]
    pub session: String,
    /// The worktree's root directory.
    pub worktree: String,
    /// Branches the worktree claims.
    pub branches: Vec<String>,
    /// Unix seconds.
    pub taken_at: i64,
    /// Unix seconds of the last take or heartbeat.
    pub heartbeat: i64,
    pub ttl_secs: u64,
}

impl Lease {
    /// Whether the lease still claims its branches at `now`.
    pub fn is_live(&self, now: i64) -> bool {
        now.saturating_sub(self.heartbeat) <= self.ttl_secs as i64
    }
}

/// The lease directory of a repository and the key of the worktree a command runs in.
#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

/// The worktree a command runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// File name of its lease: the linked worktree's name, or `main` for the main one.
    pub key: String,
    pub root: PathBuf,
    /// The branch checked out there, if any.
    pub branch: Option<String>,
}

/// A key is a file name: letters, digits, `-`, `_` and `.`, never a leading `.`.
fn safe_key(name: &str) -> String {
    let k: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    match k.trim_start_matches('.') {
        "" => "_".to_string(),
        k => k.to_string(),
    }
}

/// The lease key of the linked worktree named `name`, as [`open`] names it.
/// Text a lease or the repository supplies, quoted in a message of the lease commands,
/// the lease guard or the pre-tool hook: one code span on one line, with no control
/// character and a bounded length ([`crate::report::text::agent_span`]). The reader may
/// be a coding agent, and an agent name, a session id or a branch can hold a sentence
/// that reads as an instruction; inside the span it is data.
pub fn quote(text: impl std::fmt::Display) -> String {
    crate::report::text::agent_span(&text.to_string())
}

/// The session a lease names, quoted; `-` for a lease that names none.
pub fn quote_session(session: &str) -> String {
    quote(if session.is_empty() { "-" } else { session })
}

/// Whether `name` can be written into a command a message suggests: a branch name of
/// letters, digits and `/ . _ -` only, which a shell reads as one word.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

/// The command that takes `branch` from the lease that holds it, as a message suggests
/// it. The command is the tool's own words, so it names the branch only when the name is
/// a plain one ([`plain_name`]); otherwise it shows `<branch>`.
pub fn take_command(branch: &str) -> String {
    if plain_name(branch) {
        format!("`discipline lease take --branch {branch} --steal`")
    } else {
        "`discipline lease take --branch <branch> --steal`".to_string()
    }
}

pub fn key_of(name: &str) -> String {
    safe_key(name)
}

/// The lease store of the repository at `path` and the worktree `path` is in.
pub fn open(path: &Path) -> Result<(Store, Worktree)> {
    let repo = crate::gitctx::discover_repository(path)?;
    let root = repo
        .workdir()
        .ok_or_else(|| anyhow!("a bare repository has no worktree to lease"))?
        .to_path_buf();
    let key = if repo.is_worktree() {
        // A linked worktree's git directory is `<common git dir>/worktrees/<name>/`.
        let name = repo
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("cannot read this worktree's name"))?;
        safe_key(name)
    } else {
        "main".to_string()
    };
    let branch = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().ok().map(str::to_string));
    Ok((
        Store {
            dir: repo.commondir().join("discipline").join("leases"),
        },
        Worktree { key, root, branch },
    ))
}

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Held while leases are read and written; removed on drop.
struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        // Drop cannot return an error. A lock left behind blocks later takes until it
        // is stale (STALE_LOCK_SECS), so the failure is said rather than dropped.
        if let Err(e) = std::fs::remove_file(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("lease: could not remove the lock {}: {e}", self.0.display());
            }
        }
    }
}

/// What a take did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    pub lease: Lease,
    /// `(worktree key, branch)` claims removed from other live leases by `--steal`.
    pub stolen: Vec<(String, String)>,
}

impl Store {
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
                    let old = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > STALE_LOCK_SECS);
                    if old {
                        match std::fs::remove_file(&path) {
                            Ok(()) => continue,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(e) => {
                                return Err(e).with_context(|| {
                                    format!("cannot remove the stale lease lock {}", path.display())
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
            "the lease lock {} is held by another process; try again",
            path.display()
        )
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", safe_key(key)))
    }

    fn write(&self, key: &str, lease: &Lease) -> Result<()> {
        let path = self.path(key);
        let tmp = self.dir.join(format!(".{}.tmp", safe_key(key)));
        std::fs::write(&tmp, serde_json::to_string_pretty(lease)? + "\n")
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }

    /// Every lease, by worktree key. A file that does not parse is an error, never
    /// skipped: a guard that silently drops a lease would let a claimed branch move.
    pub fn list(&self) -> Result<Vec<(String, Lease)>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", self.dir.display())),
        };
        // In name order: the directory lists its files in an order of its own, and the
        // lease an error names is the first that could not be read.
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("cannot read {}", self.dir.display()))?;
            paths.push(entry.path());
        }
        paths.sort_unstable();
        for path in paths {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(key) = name.strip_suffix(".json").filter(|k| !k.starts_with('.')) else {
                continue;
            };
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let lease: Lease = serde_json::from_str(&text)
                .with_context(|| format!("lease {} is not valid", path.display()))?;
            out.push((key.to_string(), lease));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// The live lease of another worktree that claims `branch`, if any.
    pub fn holder(&self, branch: &str, except: &str, now: i64) -> Result<Option<(String, Lease)>> {
        Ok(self
            .list()?
            .into_iter()
            .find(|(k, l)| k != except && l.is_live(now) && l.branches.iter().any(|b| b == branch)))
    }

    /// Take (or refresh) `key`'s lease on `branches`. A branch another worktree's live
    /// lease claims is refused, or with `steal` removed from that lease.
    pub fn take(&self, key: &str, mut lease: Lease, now: i64, steal: bool) -> Result<Taken> {
        let _lock = self.lock()?;
        let all = self.list()?;
        let mut stolen = Vec::new();
        let mut refused = Vec::new();
        for (other, held) in &all {
            if other == key || !held.is_live(now) {
                continue;
            }
            let clash: Vec<&String> = held
                .branches
                .iter()
                .filter(|b| lease.branches.contains(b))
                .collect();
            if clash.is_empty() {
                continue;
            }
            if !steal {
                for b in clash {
                    refused.push(format!(
                        "{} is leased by worktree {} ({} session {}, heartbeat {}s ago)",
                        quote(b),
                        quote(other),
                        quote(&held.agent),
                        quote_session(&held.session),
                        now - held.heartbeat
                    ));
                }
                continue;
            }
            let mut kept = held.clone();
            kept.branches.retain(|b| !lease.branches.contains(b));
            for b in clash {
                stolen.push((other.clone(), b.clone()));
            }
            self.write(other, &kept)?;
        }
        if !refused.is_empty() {
            bail!(
                "{}; hand the work over to that session, or take it with --steal",
                refused.join("; ")
            );
        }
        if let Some((_, mine)) = all.iter().find(|(k, _)| k == key) {
            // A refresh keeps when the lease was first taken.
            lease.taken_at = mine.taken_at;
        } else {
            lease.taken_at = now;
        }
        lease.heartbeat = now;
        self.write(key, &lease)?;
        Ok(Taken { lease, stolen })
    }

    /// Refresh `key`'s heartbeat when it holds a lease that names no session or names
    /// `session`. `Ok(false)` when there is nothing to refresh.
    pub fn touch(&self, key: &str, session: Option<&str>, now: i64) -> Result<bool> {
        if !self.path(key).exists() {
            return Ok(false);
        }
        let _lock = self.lock()?;
        let Some((_, mut lease)) = self.list()?.into_iter().find(|(k, _)| k == key) else {
            return Ok(false);
        };
        let ours = lease.session.is_empty() || session.is_some_and(|s| s == lease.session);
        if !ours {
            return Ok(false);
        }
        lease.heartbeat = now;
        self.write(key, &lease)?;
        Ok(true)
    }

    /// Remove `key`'s lease. `Ok(false)` when it had none.
    pub fn release(&self, key: &str) -> Result<bool> {
        let _lock = self.lock()?;
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("cannot remove {}", self.path(key).display())),
        }
    }
}

/// A branch update the guard refuses: the branch, and the worktree whose live lease
/// claims it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub branch: String,
    pub holder: String,
    pub lease: Lease,
}

/// The branch updates in a `reference-transaction` hook's stdin (`<old> <new> <ref>` per
/// line) that another worktree's live lease claims. Only `refs/heads/` counts: remote
/// tracking refs, tags and `HEAD` move freely. A deletion is an update too.
pub fn refused_updates(store: &Store, here: &str, stdin: &str, now: i64) -> Result<Vec<Refusal>> {
    let branches: Vec<&str> = stdin
        .lines()
        .filter_map(|l| l.split_whitespace().nth(2))
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .collect();
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for b in branches {
        if let Some((holder, lease)) = store.holder(b, here, now)? {
            out.push(Refusal {
                branch: b.to_string(),
                holder,
                lease,
            });
        }
    }
    Ok(out)
}

/// The first line of the hook `install-guard` writes, by which it recognises its own file.
pub const GUARD_MARKER: &str = "# Written by `discipline lease install-guard`";

/// The `reference-transaction` hook: git runs it with the transaction's state and the
/// updates on stdin, and a non-zero exit in the `prepared` state aborts the transaction.
/// Without `discipline` on `PATH` it says so and lets the update through: a guard that
/// refused every ref update would stop the repository, and the leases are cooperative.
/// An installed discipline older than `lease` (0.14.x) lets it through silently: the
/// guard becomes active when that installation is upgraded, and until then no commit in
/// any worktree carries a message about it.
pub fn guard_hook() -> String {
    format!(
        "#!/bin/sh\n{GUARD_MARKER}: refuses a branch update that another worktree's\n# live lease claims (docs/ROADMAP.md, Phase 13 Step 1).\n[ \"$1\" = prepared ] || exit 0\nif ! command -v discipline >/dev/null 2>&1; then\n  echo \"discipline is not on PATH; the lease guard did not run\" >&2\n  exit 0\nfi\n# An installed discipline older than `lease` has no guard to run: pass, silently.\ndiscipline lease --help >/dev/null 2>&1 || exit 0\nexec discipline lease guard \"$1\"\n"
    )
}

/// The `discipline lease` subcommand.
pub fn run_cli(args: crate::cli::LeaseArgs) -> Result<bool> {
    use crate::cli::LeaseCommand;
    use crate::lease::{now, open, quote, quote_session, take_command, Lease};
    let (store, here) = open(Path::new("."))?;
    let t = now();
    match args.command {
        LeaseCommand::Take(a) => {
            let branches = if a.branches.is_empty() {
                vec![here.branch.clone().ok_or_else(|| {
                    anyhow::anyhow!("no branch is checked out here; name one with --branch")
                })?]
            } else {
                a.branches
            };
            let taken = store.take(
                &here.key,
                Lease {
                    agent: a.agent,
                    session: a.session,
                    worktree: here.root.display().to_string(),
                    branches,
                    taken_at: t,
                    heartbeat: t,
                    ttl_secs: a.ttl,
                },
                t,
                a.steal,
            )?;
            for (other, branch) in &taken.stolen {
                eprintln!(
                    "lease: took {} from worktree {} (--steal)",
                    quote(branch),
                    quote(other)
                );
            }
            println!(
                "lease: worktree {} holds {} for {} (live {}s without a refresh)",
                quote(&here.key),
                taken
                    .lease
                    .branches
                    .iter()
                    .map(quote)
                    .collect::<Vec<_>>()
                    .join(", "),
                quote(&taken.lease.agent),
                taken.lease.ttl_secs
            );
            Ok(true)
        }
        LeaseCommand::Release => {
            if store.release(&here.key)? {
                println!("lease: released worktree {}", quote(&here.key));
            } else {
                println!("lease: worktree {} held no lease", quote(&here.key));
            }
            Ok(true)
        }
        LeaseCommand::List(a) => {
            let all = store.list()?;
            if a.json {
                let rows: Vec<serde_json::Value> = all
                    .iter()
                    .map(|(k, l)| {
                        let mut v = serde_json::to_value(l).unwrap_or_default();
                        v["key"] = serde_json::json!(k);
                        v["live"] = serde_json::json!(l.is_live(t));
                        v["here"] = serde_json::json!(*k == here.key);
                        v
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if all.is_empty() {
                println!("lease: no leases");
            } else {
                // One row a lease: what a lease holds is written on one line, with no
                // control character and a bounded length.
                let field = crate::report::text::agent_field;
                for (k, l) in &all {
                    println!(
                        "{:<8} {}{:<24} {:<14} {}  heartbeat {}s ago",
                        if l.is_live(t) { "live" } else { "stale" },
                        if *k == here.key { "*" } else { " " },
                        field(k),
                        field(&l.agent),
                        field(&l.branches.join(",")),
                        t - l.heartbeat
                    );
                }
            }
            Ok(true)
        }
        LeaseCommand::InstallGuard => {
            let repo = crate::gitctx::discover_repository(".")?;
            let hooks = match repo
                .config()
                .ok()
                .and_then(|c| c.get_path("core.hooksPath").ok())
            {
                Some(p) if p.is_absolute() => p,
                Some(p) => here.root.join(p),
                None => repo.commondir().join("hooks"),
            };
            let path = hooks.join("reference-transaction");
            match std::fs::read_to_string(&path) {
                Ok(existing) if existing == crate::lease::guard_hook() => {
                    println!("lease: the guard is already installed at {}", path.display());
                    return Ok(true);
                }
                // An earlier release's guard is ours to rewrite.
                Ok(existing) if existing.contains(crate::lease::GUARD_MARKER) => {}
                Ok(_) => anyhow::bail!(
                    "{} already exists and is not the lease guard; add `discipline lease guard \"$1\"` to it (when $1 is `prepared`, a non-zero exit aborts the update)",
                    path.display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            std::fs::create_dir_all(&hooks)?;
            std::fs::write(&path, crate::lease::guard_hook())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
            println!("lease: installed the guard at {}", path.display());
            Ok(true)
        }
        LeaseCommand::Guard(a) => {
            if a.state != "prepared" {
                return Ok(true);
            }
            let mut stdin = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin)?;
            let refused = crate::lease::refused_updates(&store, &here.key, &stdin, t)?;
            for r in &refused {
                eprintln!(
                    "discipline lease guard: {} is leased by worktree {} ({} session {}, heartbeat {}s ago); this worktree ({}) may not move it. Hand the work over to that session, or take the branch with {}.",
                    quote(&r.branch),
                    quote(&r.holder),
                    quote(&r.lease.agent),
                    quote_session(&r.lease.session),
                    t - r.lease.heartbeat,
                    quote(&here.key),
                    take_command(&r.branch)
                );
            }
            Ok(refused.is_empty())
        }
        LeaseCommand::Check(a) => match store.holder(&a.branch, &here.key, t)? {
            None => Ok(true),
            Some((other, l)) => {
                eprintln!(
                    "lease: {} is leased by worktree {} ({} session {}, heartbeat {}s ago); hand the work over to that session, or take it with {}",
                    quote(&a.branch),
                    quote(&other),
                    quote(&l.agent),
                    quote_session(&l.session),
                    t - l.heartbeat,
                    take_command(&a.branch)
                );
                Ok(false)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(branches: &[&str], heartbeat: i64) -> Lease {
        Lease {
            agent: "claude-code".into(),
            session: "s".into(),
            worktree: "/w".into(),
            branches: branches.iter().map(|b| b.to_string()).collect(),
            taken_at: heartbeat,
            heartbeat,
            ttl_secs: 100,
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store {
            dir: d.path().join("leases"),
        };
        (d, s)
    }

    /// Several lease files that do not parse: the error names the first in name order,
    /// whatever order the directory lists them in. The files are created in an order
    /// that is neither ascending nor descending, so a listing in creation order, in
    /// reverse creation order or in hash order does not begin with that one.
    #[test]
    fn the_invalid_lease_named_is_the_first_in_name_order() {
        let (_d, s) = store();
        std::fs::create_dir_all(&s.dir).unwrap();
        for i in [7, 3, 11, 0, 9, 1, 5, 10, 2, 8, 4, 6] {
            std::fs::write(s.dir.join(format!("wt-{i:02}.json")), "not json\n").unwrap();
        }
        let err = format!("{:#}", s.list().unwrap_err());
        assert!(
            err.contains("wt-00.json is not valid"),
            "the error names another lease: {err}"
        );
    }

    #[test]
    fn a_lease_is_live_within_its_ttl_only() {
        let l = lease(&["a"], 1000);
        assert!(l.is_live(1000));
        assert!(l.is_live(1100));
        assert!(!l.is_live(1101));
    }

    #[test]
    fn a_live_claim_refuses_another_worktree_and_steal_moves_it() {
        let (_d, s) = store();
        s.take("wt-a", lease(&["feat/x", "feat/y"], 0), 1000, false)
            .unwrap();
        let err = s
            .take("wt-b", lease(&["feat/y"], 0), 1050, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("`feat/y` is leased by worktree `wt-a`"),
            "{err}"
        );
        assert_eq!(s.holder("feat/y", "wt-b", 1050).unwrap().unwrap().0, "wt-a");
        // Its own worktree is never its holder.
        assert!(s.holder("feat/y", "wt-a", 1050).unwrap().is_none());

        let taken = s.take("wt-b", lease(&["feat/y"], 0), 1060, true).unwrap();
        assert_eq!(
            taken.stolen,
            vec![("wt-a".to_string(), "feat/y".to_string())]
        );
        let all = s.list().unwrap();
        assert_eq!(
            all[0].1.branches,
            vec!["feat/x"],
            "wt-a keeps what was not stolen"
        );
        assert_eq!(s.holder("feat/y", "wt-a", 1060).unwrap().unwrap().0, "wt-b");
    }

    #[test]
    fn a_stale_lease_claims_nothing() {
        let (_d, s) = store();
        s.take("wt-a", lease(&["feat/x"], 0), 1000, false).unwrap();
        assert!(s.holder("feat/x", "wt-b", 1101).unwrap().is_none());
        s.take("wt-b", lease(&["feat/x"], 0), 1101, false).unwrap();
    }

    #[test]
    fn a_refresh_keeps_the_first_take_time_and_release_removes() {
        let (_d, s) = store();
        s.take("wt-a", lease(&["x"], 0), 1000, false).unwrap();
        let t = s.take("wt-a", lease(&["x"], 0), 1500, false).unwrap();
        assert_eq!((t.lease.taken_at, t.lease.heartbeat), (1000, 1500));
        assert!(s.release("wt-a").unwrap());
        assert!(!s.release("wt-a").unwrap());
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn concurrent_takes_of_one_branch_have_one_winner() {
        let (_d, s) = store();
        let s = std::sync::Arc::new(s);
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let s = s.clone();
                std::thread::spawn(move || {
                    s.take(&format!("wt-{i}"), lease(&["shared"], 0), 1000, false)
                        .is_ok()
                })
            })
            .collect();
        let wins = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|w| *w)
            .count();
        assert_eq!(wins, 1);
        assert_eq!(s.list().unwrap().len(), 1);
        assert!(!s.dir.join(".lock").exists(), "the lock is released");
    }

    #[test]
    fn a_corrupt_lease_is_an_error_not_skipped() {
        let (_d, s) = store();
        std::fs::create_dir_all(&s.dir).unwrap();
        std::fs::write(s.dir.join("wt-a.json"), "not json").unwrap();
        assert!(s.list().is_err());
        assert!(s.holder("x", "wt-b", 0).is_err());
    }

    #[test]
    fn only_branch_updates_claimed_elsewhere_are_refused() {
        let (_d, s) = store();
        s.take("wt-a", lease(&["feat/stack"], 0), 1000, false)
            .unwrap();
        let z = "0000000000000000000000000000000000000000";
        let a = "1111111111111111111111111111111111111111";
        let stdin = format!(
            "{a} {z} refs/heads/feat/stack\n{z} {a} refs/heads/feat/free\n{a} {a} refs/remotes/origin/feat/stack\n{a} {a} refs/tags/feat/stack\n"
        );
        let r = refused_updates(&s, "wt-b", &stdin, 1000).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].branch.as_str(), r[0].holder.as_str()),
            ("feat/stack", "wt-a")
        );
        assert!(
            refused_updates(&s, "wt-a", &stdin, 1000)
                .unwrap()
                .is_empty(),
            "the holder moves its own branch"
        );
        assert!(
            refused_updates(&s, "wt-b", &stdin, 1200)
                .unwrap()
                .is_empty(),
            "a stale lease claims nothing"
        );
        assert!(guard_hook().starts_with(&format!("#!/bin/sh\n{GUARD_MARKER}")));
    }

    #[test]
    fn keys_are_file_names() {
        assert_eq!(
            safe_key("gitea-install-docs-1fa51b"),
            "gitea-install-docs-1fa51b"
        );
        assert_eq!(safe_key("../evil/name"), "_evil_name");
        assert_eq!(safe_key(".."), "_");
    }
}
