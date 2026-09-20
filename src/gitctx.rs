//! Git access. Every function here is three-state: a value, an empty value, or
//! an `Err` meaning "could not determine". Callers never turn the third state
//! into an empty diff — an unresolvable base ref in a shallow CI clone once
//! read as "no changes, PASS".

use anyhow::{anyhow, bail, Context, Result};
use git2::{
    Delta, DiffFindOptions, DiffOptions, Oid, Repository, Tree, TreeWalkMode, TreeWalkResult,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    /// Path on the base side (differs from `path` for renames).
    pub old_path: String,
    pub kind: ChangeKind,
    /// 1-based line numbers added on the head side.
    pub added_lines: BTreeSet<usize>,
}

impl ChangedFile {
    pub fn is_deleted(&self) -> bool {
        matches!(self.kind, ChangeKind::Deleted)
    }
}

pub struct GitCtx {
    repo: Repository,
    /// Tree the change is measured against; `None` = empty tree (first commit).
    base: Option<Oid>,
    base_label: String,
    staged: bool,
}

/// Detect the target base git ref for diff inspection.
///
/// Precedence:
/// 1. Explicit CLI argument (`--base <ref>`)
/// 2. Explicit CLI argument (`--commit <sha>` -> `<sha>~1`)
/// 3. Explicit CLI argument (`--commit-range <before>..<after>` -> `<before>`)
/// 4. `DISCIPLINE_BASE_REF` environment variable
/// 5. Push event before SHA: `GITHUB_EVENT_BEFORE`, `FORGEJO_EVENT_BEFORE`, `GITEA_EVENT_BEFORE`, `CI_COMMIT_BEFORE_SHA`
/// 6. GitLab Merge Request Target Branch: `CI_MERGE_REQUEST_TARGET_BRANCH_NAME` (`origin/<branch>`)
/// 7. GitLab Merge Request Diff Base SHA: `CI_MERGE_REQUEST_DIFF_BASE_SHA`
/// 8. Forgejo Pull Request Base Branch: `FORGEJO_BASE_REF` (`origin/<branch>`)
/// 9. Gitea Pull Request Base Branch: `GITEA_BASE_REF` (`origin/<branch>`)
/// 10. GitHub Pull Request Base Branch: `GITHUB_BASE_REF` (`origin/<branch>`)
/// 11. GitLab Default Branch: `CI_DEFAULT_BRANCH` (`origin/<branch>`)
/// 12. Default fallback: `"origin/main"`
pub fn detect_base_ref(
    explicit_base: Option<&str>,
    commit: Option<&str>,
    commit_range: Option<&str>,
) -> String {
    detect_base_ref_with_env(explicit_base, commit, commit_range, |k| {
        std::env::var(k).ok()
    })
}

pub fn is_push_event_environment() -> bool {
    is_push_event_environment_with_env(|k| std::env::var(k).ok())
}

pub fn is_push_event_environment_with_env<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    if get_env("GITHUB_EVENT_NAME").as_deref() == Some("push")
        || get_env("CI_PIPELINE_SOURCE").as_deref() == Some("push")
        || get_env("FORGEJO_EVENT_NAME").as_deref() == Some("push")
        || get_env("GITEA_EVENT_NAME").as_deref() == Some("push")
    {
        return true;
    }
    for var in &[
        "GITHUB_EVENT_BEFORE",
        "FORGEJO_EVENT_BEFORE",
        "GITEA_EVENT_BEFORE",
        "CI_COMMIT_BEFORE_SHA",
    ] {
        if let Some(before) = get_env(var) {
            let trimmed = before.trim();
            if !trimmed.is_empty() {
                return true;
            }
        }
    }
    false
}

pub fn detect_base_ref_with_env<F>(
    explicit_base: Option<&str>,
    commit: Option<&str>,
    commit_range: Option<&str>,
    get_env: F,
) -> String
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(b) = explicit_base.filter(|s| !s.trim().is_empty()) {
        return b.to_string();
    }
    if let Some(c) = commit.filter(|s| !s.trim().is_empty()) {
        let trimmed = c.trim();
        return format!("{trimmed}~1");
    }
    if let Some(r) = commit_range.filter(|s| !s.trim().is_empty()) {
        let trimmed = r.trim();
        if let Some((before, _)) = trimmed.split_once("...") {
            if !before.is_empty() {
                return before.to_string();
            }
        } else if let Some((before, _)) = trimmed.split_once("..") {
            if !before.is_empty() {
                return before.to_string();
            }
        } else {
            return trimmed.to_string();
        }
    }
    if let Some(b) = get_env("DISCIPLINE_BASE_REF") {
        if !b.trim().is_empty() {
            return b.trim().to_string();
        }
    }

    // Push event detection (GitHub Actions, Forgejo, Gitea, GitLab CI)
    if is_push_event_environment_with_env(&get_env) {
        for var in &[
            "GITHUB_EVENT_BEFORE",
            "FORGEJO_EVENT_BEFORE",
            "GITEA_EVENT_BEFORE",
            "CI_COMMIT_BEFORE_SHA",
        ] {
            if let Some(before) = get_env(var) {
                let trimmed = before.trim();
                if !trimmed.is_empty() {
                    if trimmed.chars().all(|c| c == '0') {
                        return "HEAD~1".to_string();
                    } else if trimmed.len() >= 7 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                        return trimmed.to_string();
                    }
                }
            }
        }
        return "HEAD~1".to_string();
    }

    if let Some(target_branch) = get_env("CI_MERGE_REQUEST_TARGET_BRANCH_NAME") {
        if !target_branch.trim().is_empty() {
            let trimmed = target_branch.trim();
            if trimmed.starts_with("origin/") {
                return trimmed.to_string();
            } else {
                return format!("origin/{}", trimmed);
            }
        }
    }
    if let Some(diff_base) = get_env("CI_MERGE_REQUEST_DIFF_BASE_SHA") {
        if !diff_base.trim().is_empty() {
            return diff_base.trim().to_string();
        }
    }
    for var in &["FORGEJO_BASE_REF", "GITEA_BASE_REF", "GITHUB_BASE_REF"] {
        if let Some(base_branch) = get_env(var) {
            if !base_branch.trim().is_empty() {
                let trimmed = base_branch.trim();
                if trimmed.starts_with("origin/") {
                    return trimmed.to_string();
                } else {
                    return format!("origin/{}", trimmed);
                }
            }
        }
    }
    if let Some(default_branch) = get_env("CI_DEFAULT_BRANCH") {
        if !default_branch.trim().is_empty() {
            let trimmed = default_branch.trim();
            if trimmed.starts_with("origin/") {
                return trimmed.to_string();
            } else {
                return format!("origin/{}", trimmed);
            }
        }
    }
    "origin/main".to_string()
}

/// Discovers a git repository starting from the given path.
///
/// If `DISCIPLINE_TRUST_WORKSPACE` is set to `"1"` or `"true"`, libgit2's owner validation
/// is disabled to support containerized CI environments (such as Docker, Gitea, Forgejo, or
/// Kubernetes) where mounted workspaces are owned by a different UID than the container's
/// unprivileged user (UID 10001).
///
/// If discovery fails due to libgit2 owner validation (`code=Owner (-36)`), a clear,
/// actionable diagnostic is returned naming the exact remediations instead of a generic
/// "not inside a git repository".
pub fn discover_repository(path: impl AsRef<std::path::Path>) -> Result<Repository> {
    let trust_workspace = std::env::var("DISCIPLINE_TRUST_WORKSPACE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if trust_workspace {
        // SAFETY: Disabling owner validation in libgit2 is safe during single-threaded
        // repository discovery and allows volume-mounted repositories across differing
        // UIDs in containerized CI environments (such as Docker, Gitea, Forgejo, or Kubernetes)
        // when explicitly opted-in via DISCIPLINE_TRUST_WORKSPACE.
        unsafe {
            let _ = git2::opts::set_verify_owner_validation(false);
        }
    }

    match Repository::discover(path.as_ref()) {
        Ok(repo) => Ok(repo),
        Err(err) => {
            let is_owner_error = err.code() == git2::ErrorCode::Owner
                || err.raw_code() == -36
                || err.message().contains("not owned by current user");
            if is_owner_error {
                bail!(
                    "repository at '{}' is not owned by current user (libgit2 owner validation rejected access; code=Owner (-36)).\n\
                    To resolve this:\n\
                      1. Add the path to git's safe directory: git config --global --add safe.directory '{}' (or '*' in ephemeral environments).\n\
                      2. Or run the container matching the host UID/GID: --user \"$(id -u):$(id -g)\".\n\
                      3. Or opt in to trust the workspace: --trust-workspace (or pass DISCIPLINE_TRUST_WORKSPACE=1).",
                    path.as_ref().display(),
                    path.as_ref().display()
                );
            }
            Err(err).context("not inside a git repository: discipline measures a change, so it needs git history")
        }
    }
}

impl GitCtx {
    /// `staged = true` inspects the index against `HEAD` (pre-commit hook).
    /// Otherwise the working tree is measured against the merge base of
    /// `base_ref` and `HEAD`.
    pub fn open(base_ref: &str, staged: bool) -> Result<Self> {
        let repo = discover_repository(".")?;
        if repo.is_bare() {
            bail!("bare repositories are not supported");
        }

        let head: Option<Oid> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.id());
        let (base, base_label) = if staged {
            match head {
                Some(id) => (Some(id), "HEAD (staged changes)".to_string()),
                None => (None, "empty tree (first commit)".to_string()),
            }
        } else {
            let head = head.ok_or_else(|| {
                anyhow!("repository has no commits; use --staged for the first commit")
            })?;
            let mut candidates = vec![base_ref.to_string()];
            if let Some(stripped) = base_ref.strip_prefix("origin/") {
                candidates.push(stripped.to_string());
            } else {
                candidates.push(format!("origin/{base_ref}"));
            }
            let resolve_base = |candidates: &[String]| -> Option<(Oid, Oid)> {
                let base_commit = candidates.iter().find_map(|name| {
                    Some(repo.revparse_single(name).ok()?.peel_to_commit().ok()?.id())
                })?;
                let merge_base = repo.merge_base(base_commit, head).ok()?;
                Some((base_commit, merge_base))
            };

            let (_base_commit, merge_base) = match resolve_base(&candidates) {
                Some(pair) => pair,
                None => {
                    deepen_git_history(&candidates, base_ref, &repo);
                    let base_commit = candidates
                        .iter()
                        .find_map(|name| {
                            Some(repo.revparse_single(name).ok()?.peel_to_commit().ok()?.id())
                        })
                        .ok_or_else(|| {
                            anyhow!(
                                "base ref `{base_ref}` does not resolve. In CI, check out with \
                                 `fetch-depth: 0` or fetch the base branch first. Refusing to \
                                 treat an unknown base as an empty diff."
                            )
                        })?;
                    let merge_base = repo.merge_base(base_commit, head).map_err(|e| {
                        anyhow!(
                            "no merge base between `{base_ref}` and HEAD ({e}); the clone is \
                             probably shallow — fetch full history"
                        )
                    })?;
                    (base_commit, merge_base)
                }
            };
            (
                Some(merge_base),
                format!("{base_ref} (merge base {:.10})", merge_base.to_string()),
            )
        };

        Ok(Self {
            repo,
            base,
            base_label,
            staged,
        })
    }

    pub fn base_label(&self) -> &str {
        &self.base_label
    }

    pub fn has_base(&self) -> bool {
        self.base.is_some()
    }

    pub fn root(&self) -> &std::path::Path {
        self.repo
            .workdir()
            .expect("repo is not bare, validated in open()")
    }

    fn base_tree(&self) -> Result<Option<Tree<'_>>> {
        match self.base {
            Some(oid) => Ok(Some(self.repo.find_commit(oid)?.tree()?)),
            None => Ok(None),
        }
    }

    pub fn changed_files(&self) -> Result<Vec<ChangedFile>> {
        let tree = self.base_tree()?;
        let mut opts = DiffOptions::new();
        opts.context_lines(0);
        let mut diff = if self.staged {
            self.repo
                .diff_tree_to_index(tree.as_ref(), None, Some(&mut opts))?
        } else {
            self.repo
                .diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut opts))?
        };
        // Without rename detection a moved test file reads as a deletion.
        diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;

        let mut files: BTreeMap<String, ChangedFile> = BTreeMap::new();
        for delta in diff.deltas() {
            let kind = match delta.status() {
                Delta::Added | Delta::Untracked | Delta::Copied => ChangeKind::Added,
                Delta::Modified | Delta::Typechange => ChangeKind::Modified,
                Delta::Deleted => ChangeKind::Deleted,
                Delta::Renamed => ChangeKind::Renamed,
                _ => continue,
            };
            let path_of =
                |f: git2::DiffFile| f.path().map(|p| p.to_string_lossy().replace('\\', "/"));
            let new_path = path_of(delta.new_file());
            let old_path = path_of(delta.old_file());
            let path = match kind {
                ChangeKind::Deleted => old_path.clone(),
                _ => new_path.clone(),
            }
            .ok_or_else(|| anyhow!("diff delta without a path"))?;
            files.insert(
                path.clone(),
                ChangedFile {
                    old_path: old_path.unwrap_or_else(|| path.clone()),
                    path,
                    kind,
                    added_lines: BTreeSet::new(),
                },
            );
        }

        diff.foreach(
            &mut |_, _| true,
            None,
            None,
            Some(&mut |delta, _hunk, line| {
                if line.origin() == '+' {
                    if let (Some(p), Some(n)) = (delta.new_file().path(), line.new_lineno()) {
                        let key = p.to_string_lossy().replace('\\', "/");
                        if let Some(f) = files.get_mut(&key) {
                            f.added_lines.insert(n as usize);
                        }
                    }
                }
                true
            }),
        )?;

        Ok(files.into_values().collect())
    }

    pub fn normalize_repo_path<'a>(&self, path: &'a str) -> &'a str {
        let p = path.trim_start_matches("./");
        if let Some(root_str) = self.root().to_str() {
            let root_clean = root_str.trim_end_matches('/');
            if let Some(stripped) = p.strip_prefix(root_clean) {
                return stripped.trim_start_matches('/');
            }
        }
        p
    }

    /// Raw bytes of `path` on the base side; `None` when it did not exist there.
    pub fn base_bytes(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(tree) = self.base_tree()? else {
            return Ok(None);
        };
        let rel_path = self.normalize_repo_path(path);
        match tree.get_path(std::path::Path::new(rel_path)) {
            Ok(entry) => {
                let blob = self.repo.find_blob(entry.id())?;
                Ok(Some(blob.content().to_vec()))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Content of `path` on the base side; `None` when it did not exist there or is binary.
    pub fn base_content(&self, path: &str) -> Result<Option<String>> {
        let Some(bytes) = self.base_bytes(path)? else {
            return Ok(None);
        };
        if is_binary_file(path, &bytes) {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Raw bytes of `path` on the head side (index when staged, else worktree).
    pub fn head_bytes(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let rel_path = self.normalize_repo_path(path);
        if self.staged {
            let index = self.repo.index()?;
            let Some(entry) = index.get_path(std::path::Path::new(rel_path), 0) else {
                return Ok(None);
            };
            Ok(Some(self.repo.find_blob(entry.id)?.content().to_vec()))
        } else {
            let root = self
                .repo
                .workdir()
                .ok_or_else(|| anyhow!("repository has no working tree"))?;
            let full_path = root.join(rel_path);
            if !full_path.exists() {
                return Ok(None);
            }
            if let Ok(canon) = full_path.canonicalize() {
                let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
                if !canon.starts_with(&root_canon) {
                    // Path escapes repository root (symlink traversal)
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
            Ok(Some(
                std::fs::read(&full_path).with_context(|| format!("failed to read `{path}`"))?,
            ))
        }
    }

    /// Content of `path` on the head side (index when staged, else worktree).
    /// `None` for binary content (executable, image, archive, or known binary format).
    pub fn head_content(&self, path: &str) -> Result<Option<String>> {
        let Some(bytes) = self.head_bytes(path)? else {
            return Ok(None);
        };
        if is_binary_file(path, &bytes) {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Tracked regular files. Symlinks are skipped so `CLAUDE.md -> AGENTS.md`
    /// is not scanned (and reported) twice.
    pub fn tracked_files(&self) -> Result<Vec<String>> {
        const MODE_SYMLINK: u32 = 0o120000;
        const MODE_GITLINK: u32 = 0o160000;
        let index = self.repo.index()?;
        let root = self.repo.workdir().map(|p| p.to_path_buf());
        let mut out = Vec::new();
        for entry in index.iter() {
            if entry.mode == MODE_SYMLINK || entry.mode == MODE_GITLINK {
                continue;
            }
            let path = String::from_utf8_lossy(&entry.path).replace('\\', "/");
            // Deleted in the worktree but not yet staged: nothing to scan.
            if !self.staged {
                if let Some(root) = &root {
                    if !root.join(&path).is_file() {
                        continue;
                    }
                }
            }
            out.push(path);
        }
        Ok(out)
    }

    /// Tracked regular files in the base ref tree. Symlinks are skipped.
    pub fn base_tracked_files(&self) -> Result<Vec<String>> {
        const MODE_SYMLINK: i32 = 0o120000;
        const MODE_GITLINK: i32 = 0o160000;
        let Some(tree) = self.base_tree()? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        tree.walk(TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob) {
                let mode = entry.filemode();
                if mode != MODE_SYMLINK && mode != MODE_GITLINK {
                    if let Ok(name) = entry.name() {
                        let path = format!("{}{}", root, name).replace('\\', "/");
                        out.push(path);
                    }
                }
            }
            TreeWalkResult::Ok
        })?;
        Ok(out)
    }

    /// Every tracked path including symlinks, for existence checks.
    pub fn is_tracked(&self, path: &str) -> Result<bool> {
        Ok(self
            .repo
            .index()?
            .get_path(std::path::Path::new(path), 0)
            .is_some())
    }

    pub fn is_symlink(&self, path: &str) -> Result<bool> {
        Ok(self
            .repo
            .index()?
            .get_path(std::path::Path::new(path), 0)
            .map(|e| e.mode == 0o120000)
            .unwrap_or(false))
    }

    /// Commits between the base and `HEAD` as `(short_oid, message)` (empty when staged).
    pub fn commits(&self) -> Result<Vec<(String, String)>> {
        let (Some(base), false) = (self.base, self.staged) else {
            return Ok(Vec::new());
        };
        let mut walk = self.repo.revwalk()?;
        walk.push_head()?;
        walk.hide(base)?;
        let mut out = Vec::new();
        for oid in walk {
            let oid = oid?;
            let commit = self.repo.find_commit(oid)?;
            let id_str = format!("{oid}");
            let short = id_str.chars().take(7).collect::<String>();
            let msg = String::from_utf8_lossy(commit.message_bytes()).into_owned();
            out.push((short, msg));
        }
        Ok(out)
    }

    /// Messages of the commits between the base and `HEAD` (empty when staged).
    pub fn commit_messages(&self) -> Result<Vec<String>> {
        Ok(self.commits()?.into_iter().map(|(_, m)| m).collect())
    }

    /// Returns the HEAD commit subject (`%s`), if available.
    pub fn head_commit_subject(&self) -> Result<String> {
        let head = self.repo.head()?.peel_to_commit()?;
        let msg = String::from_utf8_lossy(head.message_bytes());
        let subject = msg.lines().next().unwrap_or("").trim().to_string();
        Ok(subject)
    }

    /// Returns the HEAD commit body (`%b`), if available.
    pub fn head_commit_body(&self) -> Result<String> {
        let head = self.repo.head()?.peel_to_commit()?;
        let msg = String::from_utf8_lossy(head.message_bytes());
        let mut lines = msg.lines();
        lines.next(); // Skip subject line
        let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        Ok(body)
    }

    /// Returns the HEAD commit author name (`%an`), if available.
    pub fn head_commit_author(&self) -> Result<String> {
        let head = self.repo.head()?.peel_to_commit()?;
        let author = head.author();
        let name = author.name().unwrap_or("").to_string();
        Ok(name)
    }
}

fn is_ci_environment() -> bool {
    std::env::var("CI").is_ok()
        || std::env::var("GITHUB_ACTIONS").is_ok()
        || std::env::var("GITLAB_CI").is_ok()
        || std::env::var("GITEA_ACTIONS").is_ok()
        || std::env::var("FORGEJO_ACTIONS").is_ok()
}

fn deepen_git_history(candidates: &[String], base_ref: &str, repo: &Repository) {
    if !is_ci_environment() {
        // Do not make unsolicited network requests in local developer environments
        return;
    }

    let root = match repo.workdir() {
        Some(r) => r,
        None => return,
    };
    // repo.path() resolves the real gitdir even in worktrees where .git is a gitdir reference file
    let is_shallow = repo.path().join("shallow").exists();

    let run_fetch = |args: &[&str]| {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        let _ = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                let start = Instant::now();
                let timeout = Duration::from_secs(30);
                loop {
                    match child.try_wait()? {
                        Some(status) => return Ok(status),
                        None => {
                            if start.elapsed() >= timeout {
                                let _ = child.kill();
                                let _ = child.wait();
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
                child.wait()
            });
    };

    // 1. If base ref is a commit SHA (>=7 hex characters), attempt targeted fetch
    let is_sha = base_ref.len() >= 7 && base_ref.chars().all(|c| c.is_ascii_hexdigit());
    if is_sha {
        run_fetch(&["fetch", "--no-tags", "--depth=100", "origin", base_ref]);
    }

    // 2. Try candidate branch names if they don't contain revision selectors (~, ^)
    for cand in candidates {
        let ref_name = cand.strip_prefix("origin/").unwrap_or(cand);
        if !is_sha && !ref_name.contains('~') && !ref_name.contains('^') {
            run_fetch(&["fetch", "--no-tags", "--depth=100", "origin", ref_name]);
        }
    }

    // 3. If repository is shallow, attempt unshallowing
    if is_shallow {
        run_fetch(&["fetch", "--no-tags", "--unshallow", "origin"]);
    }
}

pub fn is_binary_file(path: &str, bytes: &[u8]) -> bool {
    // 1. Check known binary magic bytes
    if bytes.starts_with(b"\x7fELF") // ELF
        || bytes.starts_with(b"\xfe\xed\xfa\xce") // Mach-O 32-bit
        || bytes.starts_with(b"\xfe\xed\xfa\xcf") // Mach-O 64-bit
        || bytes.starts_with(b"\xce\xfa\xed\xfe") // Mach-O 32-bit rev
        || bytes.starts_with(b"\xcf\xfa\xed\xfe") // Mach-O 64-bit rev
        || bytes.starts_with(b"\xca\xfe\xba\xbe") // Mach-O fat / Java class
        || bytes.starts_with(b"MZ") // Windows PE / DOS
        || bytes.starts_with(b"\0asm") // WebAssembly
        || bytes.starts_with(b"%PDF-") // PDF
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n") // PNG
        || bytes.starts_with(b"\xff\xd8\xff") // JPEG
        || bytes.starts_with(b"GIF87a") // GIF
        || bytes.starts_with(b"GIF89a") // GIF
        || (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP") // WebP
        || bytes.starts_with(b"PK\x03\x04") // Zip / Jar
        || bytes.starts_with(b"PK\x05\x06") // Empty Zip
        || bytes.starts_with(b"PK\x07\x08") // Spanned Zip
        || bytes.starts_with(b"\x1f\x8b") // Gzip
        || bytes.starts_with(b"BZh") // Bzip2
        || bytes.starts_with(b"\xfd7zXZ\x00") // XZ
        || bytes.starts_with(b"7z\xbc\xaf\x27\x1c") // 7z
        || (bytes.len() >= 262 && &bytes[257..262] == b"ustar") // Tar
        || bytes.starts_with(b"SQLite format 3\0") // SQLite
        || bytes.starts_with(b"PAR1") // Parquet
        || bytes.starts_with(b"ARROW1")
    // Arrow
    {
        return true;
    }

    // 2. Check file extension
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    const KNOWN_TEXT_EXTS: &[&str] = &[
        "md",
        "markdown",
        "mdown",
        "mkd",
        "mkdn",
        "txt",
        "rs",
        "toml",
        "json",
        "yaml",
        "yml",
        "c",
        "h",
        "cpp",
        "cc",
        "cxx",
        "hpp",
        "hh",
        "hxx",
        "py",
        "pyi",
        "sh",
        "bash",
        "zsh",
        "html",
        "htm",
        "xml",
        "svg",
        "css",
        "scss",
        "sass",
        "js",
        "mjs",
        "cjs",
        "jsx",
        "ts",
        "tsx",
        "php",
        "phpt",
        "rb",
        "go",
        "java",
        "kt",
        "kts",
        "swift",
        "scala",
        "sql",
        "lua",
        "pl",
        "pm",
        "r",
        "diff",
        "patch",
        "ini",
        "cfg",
        "conf",
        "env",
        "gitignore",
        "gitattributes",
        "editorconfig",
    ];

    if KNOWN_TEXT_EXTS.contains(&ext.as_str()) {
        return false;
    }

    const KNOWN_BINARY_EXTS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "tiff", "tif", "mp4", "mp3", "wav",
        "ogg", "avi", "mov", "webm", "flac", "aac", "m4a", "pdf", "doc", "docx", "ppt", "pptx",
        "xls", "xlsx", "odt", "epub", "exe", "dll", "so", "dylib", "a", "o", "obj", "class", "pyc",
        "wasm", "bin", "zip", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "7z", "rar", "zst",
        "parquet", "arrow", "feather", "h5", "hdf5", "npy", "npz", "pkl", "pickle", "joblib",
        "mat",
    ];

    if KNOWN_BINARY_EXTS.contains(&ext.as_str()) {
        return true;
    }

    // 3. For files with unknown or no extension: if the first 1024 bytes contain NUL, treat as binary.
    let check_len = bytes.len().min(1024);
    bytes[..check_len].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_base_ref_explicit() {
        assert_eq!(
            detect_base_ref_with_env(Some("my-branch"), None, None, |_| None),
            "my-branch"
        );
    }

    #[test]
    fn test_detect_base_ref_commit_and_range() {
        assert_eq!(
            detect_base_ref_with_env(None, Some("1a2b3c4d5e"), None, |_| None),
            "1a2b3c4d5e~1"
        );
        assert_eq!(
            detect_base_ref_with_env(None, None, Some("v0.1.0..v0.2.0"), |_| None),
            "v0.1.0"
        );
        assert_eq!(
            detect_base_ref_with_env(None, None, Some("origin/main...feature"), |_| None),
            "origin/main"
        );
    }

    #[test]
    fn test_detect_base_ref_push_event() {
        let lookup = |k: &str| {
            if k == "GITHUB_EVENT_NAME" {
                Some("push".to_string())
            } else if k == "GITHUB_EVENT_BEFORE" {
                Some("abcdef1234567890abcdef1234567890abcdef12".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "abcdef1234567890abcdef1234567890abcdef12"
        );

        // Zero SHA on branch creation falls back to HEAD~1
        let lookup_zero = |k: &str| {
            if k == "GITHUB_EVENT_NAME" {
                Some("push".to_string())
            } else if k == "GITHUB_EVENT_BEFORE" {
                Some("0000000000000000000000000000000000000000".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup_zero),
            "HEAD~1"
        );
    }

    #[test]
    fn test_detect_base_ref_discipline_base_ref() {
        let lookup = |k: &str| {
            if k == "DISCIPLINE_BASE_REF" {
                Some("upstream/dev".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "upstream/dev"
        );
    }

    #[test]
    fn test_detect_base_ref_gitlab_target_branch() {
        let lookup = |k: &str| {
            if k == "CI_MERGE_REQUEST_TARGET_BRANCH_NAME" {
                Some("feature/pr-123".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "origin/feature/pr-123"
        );
    }

    #[test]
    fn test_detect_base_ref_gitlab_diff_base_sha() {
        let lookup = |k: &str| {
            if k == "CI_MERGE_REQUEST_DIFF_BASE_SHA" {
                Some("1234567890abcdef1234567890abcdef12345678".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "1234567890abcdef1234567890abcdef12345678"
        );
    }

    #[test]
    fn test_detect_base_ref_gitlab_default_branch() {
        let lookup = |k: &str| {
            if k == "CI_DEFAULT_BRANCH" {
                Some("master".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "origin/master"
        );
    }

    #[test]
    fn test_detect_base_ref_forgejo_base_ref() {
        let lookup = |k: &str| {
            if k == "FORGEJO_BASE_REF" {
                Some("main".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "origin/main"
        );
    }

    #[test]
    fn test_detect_base_ref_gitea_base_ref() {
        let lookup = |k: &str| {
            if k == "GITEA_BASE_REF" {
                Some("origin/develop".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "origin/develop"
        );
    }

    #[test]
    fn test_detect_base_ref_github_base_ref() {
        let lookup = |k: &str| {
            if k == "GITHUB_BASE_REF" {
                Some("release/v1.0".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "origin/release/v1.0"
        );
    }

    #[test]
    fn test_detect_base_ref_fallback() {
        assert_eq!(
            detect_base_ref_with_env(None, None, None, |_| None),
            "origin/main"
        );
    }
}
