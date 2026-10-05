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

/// One commit of the range under review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_email: String,
    pub message: String,
    pub parent_count: usize,
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

/// Author and committer of the base commit `discipline replay` builds for each case.
pub const REPLAY_BASE_EMAIL: &str = "replay@discipline.invalid";
/// Message of that base commit.
pub const REPLAY_BASE_MESSAGE: &str = "replay base";

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
///    then the event payload's `before`, read only for a push (the event is named `push`, or
///    nothing names it and the payload holds no `pull_request`)
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

/// Whether the environment names the base (steps 4 to 11 of [`detect_base_ref`]): when it
/// does not, a run with no `--base` falls to the literal `origin/main`.
pub fn environment_names_base() -> bool {
    named_base_ref_with_env(None, None, None, |k| std::env::var(k).ok()).is_some()
}

/// The head a `--commit X` or `--commit-range A..B` names: `X`, or `B` (`None` when the
/// range leaves it out, which means the checkout).
pub fn named_head(commit: Option<&str>, commit_range: Option<&str>) -> Option<String> {
    if let Some(c) = commit.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(c.to_string());
    }
    let r = commit_range.map(str::trim).filter(|s| !s.is_empty())?;
    let after = r
        .split_once("...")
        .or_else(|| r.split_once(".."))
        .map(|(_, a)| a.trim())?;
    (!after.is_empty()).then(|| after.to_string())
}

/// The change is read from the working tree, so the head a flag names must be the commit
/// checked out; otherwise the run would judge a different change than the one asked for.
pub fn verify_named_head(name: &str) -> Result<()> {
    let repo = discover_repository(".")?;
    let named = repo
        .revparse_single(name)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| anyhow!("`{name}` does not resolve to a commit: {}", e.message()))?
        .id();
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id());
    if head != Some(named) {
        let at = head.map_or("no commit".to_string(), |h| h.to_string()[..10].to_string());
        bail!(
            "`{name}` is {}, but the checkout is at {at}: discipline reads the change from the working tree, so check out `{name}` first (or pass `--base <before>` with it checked out)",
            &named.to_string()[..10]
        );
    }
    Ok(())
}

/// Whether a configuration path names a file in the repository's tree. `--config` outside
/// the repository stays absolute: no side of the change holds it, and git rejects the
/// path, so callers look it up only when this holds.
pub fn config_in_tree(path: &str) -> bool {
    let p = std::path::Path::new(path);
    // `../candidate.toml` stays relative but escapes the repository all the same.
    !p.is_absolute()
        && !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

pub fn is_push_event_environment() -> bool {
    is_push_event_environment_with_env(|k| std::env::var(k).ok())
}

/// The variables naming the CI event payload, in the one order every consumer uses: the
/// forge-specific variable wins over the GitHub-compatible one a runner also sets.
pub const EVENT_PATH_VARS: [&str; 3] = [
    "FORGEJO_EVENT_PATH",
    "GITEA_EVENT_PATH",
    "GITHUB_EVENT_PATH",
];

/// The CI event payload, from the first of [`EVENT_PATH_VARS`] that is set and non-empty.
/// That variable alone decides; a later one is never tried, so no two consumers read
/// different files. `Ok(None)` is no variable set. A variable that is set but names a
/// file that cannot be read, or that is not JSON, is an error: the CI environment handed
/// the run a payload it cannot use, which is not the same as handing it none.
///
/// The error names the variable and what failed. It carries neither the path nor any of
/// the file's content.
pub fn try_event_payload_with_env<F>(get_env: F) -> Result<Option<serde_json::Value>>
where
    F: Fn(&str) -> Option<String>,
{
    let Some((var, path)) = EVENT_PATH_VARS.iter().find_map(|var| {
        get_env(var)
            .filter(|v| !v.trim().is_empty())
            .map(|v| (*var, v))
    }) else {
        return Ok(None);
    };
    use crate::could_not_check::{tag, Reason};
    let content = std::fs::read_to_string(path).map_err(|e| {
        tag(
            Reason::Configuration,
            anyhow!(
                "`{var}` is set, but the event payload file it names cannot be read ({}); \
                 no other payload variable is tried",
                e.kind()
            ),
        )
    })?;
    let json = serde_json::from_str(&content).map_err(|e| {
        tag(
            Reason::Configuration,
            anyhow!(
                "`{var}` is set, but the event payload file it names is not valid JSON \
                 (line {}, column {}); no other payload variable is tried",
                e.line(),
                e.column()
            ),
        )
    })?;
    Ok(Some(json))
}

/// [`try_event_payload_with_env`] over the process environment. `check` and `baseline`
/// call it before anything reads the payload, so an unusable one stops the run (exit 2).
pub fn try_event_payload() -> Result<Option<serde_json::Value>> {
    try_event_payload_with_env(|k| std::env::var(k).ok())
}

/// The payload for a reader that cannot fail: an unusable file reads as no payload. Sound
/// only after [`try_event_payload`] has passed in the same run.
pub fn event_payload_with_env<F>(get_env: F) -> Option<serde_json::Value>
where
    F: Fn(&str) -> Option<String>,
{
    try_event_payload_with_env(get_env).ok().flatten()
}

pub fn event_payload() -> Option<serde_json::Value> {
    event_payload_with_env(|k| std::env::var(k).ok())
}

/// A push event's `before` as a base ref: all zeros means no previous commit (`HEAD~1`), a
/// hex SHA of at least seven digits is itself, and empty or anything else is `None`.
pub fn normalize_before(before: &str) -> Option<String> {
    let trimmed = before.trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.chars().all(|c| c == '0') {
        Some("HEAD~1".to_string())
    } else if trimmed.len() >= 7 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// The variables naming the CI event, one per platform.
const EVENT_NAME_VARS: [&str; 4] = [
    "GITHUB_EVENT_NAME",
    "CI_PIPELINE_SOURCE",
    "FORGEJO_EVENT_NAME",
    "GITEA_EVENT_NAME",
];

/// What the environment names the event as: `Some(true)` when any of [`EVENT_NAME_VARS`]
/// says `push`, `Some(false)` when one names another event and none says `push`, `None`
/// when none is set.
fn named_event_is_push<F>(get_env: &F) -> Option<bool>
where
    F: Fn(&str) -> Option<String>,
{
    let names: Vec<String> = EVENT_NAME_VARS
        .iter()
        .filter_map(|var| get_env(var))
        .collect();
    if names.iter().any(|n| n == "push") {
        Some(true)
    } else if names.iter().any(|n| !n.trim().is_empty()) {
        Some(false)
    } else {
        None
    }
}

/// The payload's `before` when it is a push's: the one reading both the push test and the
/// base share. `before` alone does not make a push: a pull request event carries it too
/// (GitHub's `synchronize` puts the previous head there), and taking it as the base would
/// examine the latest push only instead of the pull request. So it counts when the
/// environment names the event `push`; or when nothing names the event and the payload
/// holds no `pull_request`. An event named anything else is never a push by its payload.
fn push_before_from_payload<F>(get_env: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let named_push = named_event_is_push(get_env);
    if named_push == Some(false) {
        return None;
    }
    let payload = event_payload_with_env(get_env)?;
    if named_push.is_none() && payload.get("pull_request").is_some_and(|p| !p.is_null()) {
        return None;
    }
    payload
        .get("before")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn is_push_event_environment_with_env<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    if named_event_is_push(&get_env) == Some(true) {
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
    push_before_from_payload(&get_env).is_some()
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
    named_base_ref_with_env(explicit_base, commit, commit_range, get_env)
        .unwrap_or_else(|| "origin/main".to_string())
}

/// The base an argument or the environment names: every step of [`detect_base_ref`] but
/// the last. `None` when nothing names one.
pub fn named_base_ref_with_env<F>(
    explicit_base: Option<&str>,
    commit: Option<&str>,
    commit_range: Option<&str>,
    get_env: F,
) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(b) = explicit_base.filter(|s| !s.trim().is_empty()) {
        return Some(b.to_string());
    }
    if let Some(c) = commit.filter(|s| !s.trim().is_empty()) {
        let trimmed = c.trim();
        return Some(format!("{trimmed}~1"));
    }
    if let Some(r) = commit_range.filter(|s| !s.trim().is_empty()) {
        let trimmed = r.trim();
        if let Some((before, _)) = trimmed.split_once("...") {
            if !before.is_empty() {
                return Some(before.to_string());
            }
        } else if let Some((before, _)) = trimmed.split_once("..") {
            if !before.is_empty() {
                return Some(before.to_string());
            }
        } else {
            return Some(trimmed.to_string());
        }
    }
    if let Some(b) = get_env("DISCIPLINE_BASE_REF") {
        if !b.trim().is_empty() {
            return Some(b.trim().to_string());
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
            if let Some(base) = get_env(var).and_then(|b| normalize_before(&b)) {
                return Some(base);
            }
        }
        if let Some(base) = push_before_from_payload(&get_env).and_then(|b| normalize_before(&b)) {
            return Some(base);
        }
        return Some("HEAD~1".to_string());
    }

    if let Some(target_branch) = get_env("CI_MERGE_REQUEST_TARGET_BRANCH_NAME") {
        if !target_branch.trim().is_empty() {
            let trimmed = target_branch.trim();
            if trimmed.starts_with("origin/") {
                return Some(trimmed.to_string());
            } else {
                return Some(format!("origin/{}", trimmed));
            }
        }
    }
    if let Some(diff_base) = get_env("CI_MERGE_REQUEST_DIFF_BASE_SHA") {
        if !diff_base.trim().is_empty() {
            return Some(diff_base.trim().to_string());
        }
    }
    for var in &["FORGEJO_BASE_REF", "GITEA_BASE_REF", "GITHUB_BASE_REF"] {
        if let Some(base_branch) = get_env(var) {
            if !base_branch.trim().is_empty() {
                let trimmed = base_branch.trim();
                if trimmed.starts_with("origin/") {
                    return Some(trimmed.to_string());
                } else {
                    return Some(format!("origin/{}", trimmed));
                }
            }
        }
    }
    if let Some(default_branch) = get_env("CI_DEFAULT_BRANCH") {
        if !default_branch.trim().is_empty() {
            let trimmed = default_branch.trim();
            if trimmed.starts_with("origin/") {
                return Some(trimmed.to_string());
            } else {
                return Some(format!("origin/{}", trimmed));
            }
        }
    }
    None
}

/// Discovers a git repository starting from the given path.
///
/// If `DISCIPLINE_TRUST_WORKSPACE` is set to `"1"` or `"true"`, libgit2's owner validation
/// is disabled to support containerized CI environments (such as Docker, Gitea, Forgejo, or
/// Kubernetes) where mounted workspaces are owned by a different UID than the container's
/// unprivileged user (UID 10001).
///
/// Formats a repository discovery error, providing ownership-specific diagnostics
/// and actionable remediations when libgit2 owner validation fails (`code=Owner (-36)`).
pub fn format_discover_error(err: &git2::Error, path: &std::path::Path) -> anyhow::Error {
    let is_owner_error = err.code() == git2::ErrorCode::Owner
        || err.raw_code() == -36
        || err.message().contains("not owned by current user");
    if is_owner_error {
        anyhow::anyhow!(
            "repository at '{}' is not owned by current user (libgit2 owner validation rejected access; code=Owner (-36)).\n\
            To resolve this:\n\
              1. Add the path to git's safe directory: git config --global --add safe.directory '{}' (or '*' in ephemeral environments).\n\
              2. Or run the container matching the host UID/GID: --user \"$(id -u):$(id -g)\".\n\
              3. Or opt in to trust the workspace: --trust-workspace (or pass DISCIPLINE_TRUST_WORKSPACE=1).",
            path.display(),
            path.display()
        )
    } else {
        anyhow::anyhow!("{err}").context(
            "not inside a git repository: discipline measures a change, so it needs git history",
        )
    }
}

/// The two sides of one file: each `None` when the file does not exist (or is binary)
/// there. A failed read is never represented here; see `GitCtx::sides`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sides {
    pub base: Option<String>,
    pub head: Option<String>,
}

/// For a reader that cannot return `Result` (a closure handed to a parser or to the
/// baseline code): keeps the first read error instead of dropping it, so the caller
/// ends with `finish()?` and the gate is "could not run", not a pass over a missing file.
#[derive(Default)]
#[must_use = "a recorder that is never `finish()`ed drops the read error it kept"]
pub struct ReadRecorder {
    first: std::cell::RefCell<Option<anyhow::Error>>,
}

impl ReadRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Passes a successful read through (`None` for an absent file) and keeps a failed one.
    pub fn keep(&self, read: Result<Option<String>>) -> Option<String> {
        match read {
            Ok(content) => content,
            Err(e) => {
                self.first.borrow_mut().get_or_insert(e);
                None
            }
        }
    }

    pub fn head<'a>(&'a self, git: &'a GitCtx) -> impl Fn(&str) -> Option<String> + 'a {
        move |p| self.keep(git.head_content(p))
    }

    pub fn base<'a>(&'a self, git: &'a GitCtx) -> impl Fn(&str) -> Option<String> + 'a {
        move |p| self.keep(git.base_content(p))
    }

    /// The first read error seen, if any.
    pub fn finish(&self) -> Result<()> {
        match self.first.borrow_mut().take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

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
        Err(err) => Err(format_discover_error(&err, path.as_ref())),
    }
}

impl GitCtx {
    /// `staged = true` inspects the index against `HEAD` (pre-commit hook).
    /// Otherwise the working tree is measured against the merge base of
    /// `base_ref` and `HEAD`.
    /// Open the repository with the **empty tree** as the comparison base, so
    /// every tracked file reads as added.
    ///
    /// This is the population a brownfield adopter needs: the findings that
    /// already exist, rather than the findings a change introduced. On a clean
    /// branch the ordinary base is `HEAD`, the diff is empty, and diff-scoped
    /// gates legitimately report nothing — which leaves a consumer with no way
    /// to grandfather existing debt.
    ///
    /// Not a checking mode: a gate run this way reports the whole repository,
    /// so it is used by `discipline baseline --whole-tree` only.
    pub fn open_whole_tree() -> Result<Self> {
        let repo = discover_repository(".")?;
        if repo.is_bare() {
            bail!("bare repositories are not supported");
        }
        if repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .is_none()
        {
            bail!("repository has no commits; nothing to baseline");
        }
        Ok(Self {
            repo,
            base: None,
            base_label: "empty tree (whole-tree baseline)".to_string(),
            staged: false,
        })
    }

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
                anyhow!("repository has no commits; check the first commit with `discipline check --staged`")
            })?;
            let mut candidates = vec![base_ref.to_string()];
            if let Some(stripped) = base_ref.strip_prefix("origin/") {
                candidates.push(stripped.to_string());
            } else {
                candidates.push(format!("origin/{base_ref}"));
            }
            // The candidates after these two are guesses at the default branch. A guess that
            // already holds HEAD compares the change with itself: `origin/HEAD` is the change's
            // own branch after `git clone --branch <change>`, an empty diff every gate passes.
            let named = candidates.len();
            let holds_head = |i: usize, commit: Oid| {
                i >= named && repo.merge_base(commit, head).is_ok_and(|m| m == head)
            };
            if base_ref == "origin/main" || base_ref == "main" {
                for fallback in &[
                    "refs/remotes/origin/HEAD",
                    "origin/master",
                    "master",
                    "origin/trunk",
                    "trunk",
                ] {
                    if !candidates.contains(&fallback.to_string()) {
                        candidates.push(fallback.to_string());
                    }
                }
            }
            let resolve_base = |candidates: &[String]| -> Option<(String, Oid, Oid)> {
                for (i, name) in candidates.iter().enumerate() {
                    if let Ok(obj) = repo.revparse_single(name) {
                        if let Ok(commit) = obj.peel_to_commit() {
                            let base_commit = commit.id();
                            if holds_head(i, base_commit) {
                                continue;
                            }
                            if let Ok(merge_base) = repo.merge_base(base_commit, head) {
                                return Some((name.clone(), base_commit, merge_base));
                            }
                        }
                    }
                }
                None
            };

            let (resolved_name, _base_commit, merge_base) = match resolve_base(&candidates) {
                Some(triple) => triple,
                None => {
                    let fetch_errors = deepen_git_history(&candidates, base_ref, &repo);
                    let fetch_detail = if fetch_errors.is_empty() {
                        String::new()
                    } else {
                        format!("\ngit fetch diagnostics:\n  {}", fetch_errors.join("\n  "))
                    };
                    let (found_name, base_commit) = candidates
                        .iter()
                        .enumerate()
                        .find_map(|(i, name)| {
                            let c = repo.revparse_single(name).ok()?.peel_to_commit().ok()?.id();
                            (!holds_head(i, c)).then(|| (name.clone(), c))
                        })
                        .ok_or_else(|| {
                            anyhow!(
                                "base ref `{base_ref}` does not resolve. If working locally on a non-main branch, pass `--base <branch>` (e.g. `--base master` or `--base HEAD~1`). In CI, check out with \
                                 `fetch-depth: 0` or fetch the base branch first. Refusing to \
                                 treat an unknown base as an empty diff.{fetch_detail}"
                            )
                        })?;
                    let merge_base = repo.merge_base(base_commit, head).map_err(|e| {
                        anyhow!(
                            "no merge base between `{base_ref}` and HEAD ({e}); the clone is \
                             probably shallow — fetch full history.{fetch_detail}"
                        )
                    })?;
                    (found_name, base_commit, merge_base)
                }
            };
            let label_prefix = if resolved_name == base_ref {
                base_ref.to_string()
            } else {
                format!("{base_ref} ({resolved_name})")
            };
            (
                Some(merge_base),
                format!("{label_prefix} (merge base {:.10})", merge_base.to_string()),
            )
        };

        Ok(Self {
            repo,
            base,
            base_label,
            staged,
        })
    }

    /// A whole-tree run diffs against the empty tree: every file is "added". A gate whose
    /// rule describes a change has nothing to describe there.
    pub fn is_whole_tree(&self) -> bool {
        self.base.is_none() && !self.staged
    }

    pub fn base_label(&self) -> &str {
        &self.base_label
    }

    pub fn has_base(&self) -> bool {
        self.base.is_some()
    }

    pub fn base_oid(&self) -> Option<Oid> {
        self.base
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

    /// Submodule pointers (gitlinks) the change adds, moves, bumps or removes. They are
    /// left out of [`GitCtx::changed_files`]: no gate can read a submodule's content.
    pub fn changed_submodules(&self) -> Result<Vec<String>> {
        let tree = self.base_tree()?;
        let mut opts = DiffOptions::new();
        opts.context_lines(0);
        let diff = if self.staged {
            self.repo
                .diff_tree_to_index(tree.as_ref(), None, Some(&mut opts))?
        } else {
            self.repo
                .diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut opts))?
        };
        let mut out = BTreeSet::new();
        for delta in diff.deltas() {
            for f in [delta.new_file(), delta.old_file()] {
                if f.mode() == git2::FileMode::Commit {
                    if let Some(p) = f.path() {
                        out.insert(p.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    pub fn changed_files(&self) -> Result<Vec<ChangedFile>> {
        let tree = self.base_tree()?;
        let mut opts = DiffOptions::new();
        opts.context_lines(0);
        opts.force_text(true);
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
            // A gitlink (mode 160000, a submodule pointer) is a directory on
            // disk, not a file. Enumerating it hands every file-reading gate a
            // path that fails with "Is a directory", aborting the whole run, so
            // any change bumping a submodule pointer turned the gate red.
            if delta.new_file().mode() == git2::FileMode::Commit
                || delta.old_file().mode() == git2::FileMode::Commit
            {
                continue;
            }
            // A symlink (`CLAUDE.md -> AGENTS.md`) is its target, which is enumerated on
            // its own; `tracked_files` skips symlinks for the same reason.
            if delta.new_file().mode() == git2::FileMode::Link
                || delta.old_file().mode() == git2::FileMode::Link
            {
                continue;
            }
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
            // A directory or a gitlink (submodule) is not a file here, the same as an
            // absent path; only a blob that cannot be read is an error.
            Ok(entry) if entry.kind() != Some(git2::ObjectType::Blob) => Ok(None),
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
        let Some(bytes) = self
            .base_bytes(path)
            .with_context(|| format!("failed to read `{path}` on the base side"))?
        else {
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
            // A gitlink (submodule pointer) is not a file.
            if entry.mode == 0o160000 {
                return Ok(None);
            }
            Ok(Some(self.repo.find_blob(entry.id)?.content().to_vec()))
        } else {
            let root = self
                .repo
                .workdir()
                .ok_or_else(|| anyhow!("repository has no working tree"))?;
            let full_path = root.join(rel_path);
            if !full_path.exists() || full_path.is_dir() {
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
        let Some(bytes) = self
            .head_bytes(path)
            .with_context(|| format!("failed to read `{path}` on the head side"))?
        else {
            return Ok(None);
        };
        if is_binary_file(path, &bytes) {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Both sides of a file (`None` where it does not exist there). A read that fails is
    /// an `Err` naming the path, never an absent side.
    pub fn sides(&self, base_path: &str, head_path: &str) -> Result<Sides> {
        Ok(Sides {
            base: self.base_content(base_path)?,
            head: self.head_content(head_path)?,
        })
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

    /// URL of the remote `name`, if it exists and has one.
    pub fn remote_url(&self, name: &str) -> Option<String> {
        self.repo
            .find_remote(name)
            .ok()
            .and_then(|r| r.url().ok().map(str::to_string))
    }

    /// Whether the base is a commit `discipline replay` built: parentless, authored and
    /// committed as [`REPLAY_BASE_EMAIL`], with the message [`REPLAY_BASE_MESSAGE`]. A
    /// pull request's merge base on a real branch never has that shape.
    pub fn base_is_replay_base(&self) -> bool {
        let Some(c) = self.base.and_then(|b| self.repo.find_commit(b).ok()) else {
            return false;
        };
        c.parent_count() == 0
            && c.author().email().ok() == Some(REPLAY_BASE_EMAIL)
            && c.committer().email().ok() == Some(REPLAY_BASE_EMAIL)
            && c.message().ok() == Some(REPLAY_BASE_MESSAGE)
    }

    /// Full hex id of the `HEAD` commit, if there is one.
    pub fn head_oid(&self) -> Option<String> {
        self.repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.id().to_string())
    }

    /// Committer time (seconds since the epoch) of the newest commit that changed any of
    /// `paths` (repo-relative files or directories).
    ///
    /// With `branch_only`, only commits between the base and `HEAD` are considered and
    /// `Ok(None)` means the branch changes none of `paths`; otherwise the whole history
    /// reachable from `HEAD` is walked and `Ok(None)` means no commit ever touched them.
    /// A commit changes a path when the path's tree entry differs from every parent's
    /// (a merge that takes one side unchanged does not count, as in `git log -- <path>`).
    pub fn newest_commit_time_touching(
        &self,
        paths: &[String],
        branch_only: bool,
    ) -> Result<Option<i64>> {
        if branch_only && (self.base.is_none() || self.staged) {
            bail!("no base commit to bound the branch's changes");
        }
        let entry_id = |tree: &Tree<'_>, path: &str| -> Option<Oid> {
            let trimmed = path.trim_matches('/');
            tree.get_path(std::path::Path::new(trimmed))
                .ok()
                .map(|e| e.id())
        };
        let mut walk = self.repo.revwalk()?;
        walk.set_sorting(git2::Sort::TIME)?;
        walk.push_head()?;
        if branch_only {
            if let Some(base) = self.base {
                walk.hide(base)?;
            }
        }
        let mut newest: Option<i64> = None;
        for oid in walk {
            let commit = self.repo.find_commit(oid?)?;
            let tree = commit.tree()?;
            let parents: Vec<Tree<'_>> = commit
                .parents()
                .map(|p| p.tree())
                .collect::<std::result::Result<_, _>>()?;
            let touched = paths.iter().any(|path| {
                let here = entry_id(&tree, path);
                if parents.is_empty() {
                    here.is_some()
                } else {
                    parents.iter().all(|pt| entry_id(pt, path) != here)
                }
            });
            if touched {
                let t = commit.committer().when().seconds();
                newest = Some(newest.map_or(t, |n: i64| n.max(t)));
            }
        }
        Ok(newest)
    }

    /// Committer time of the newest commit on the base's first-parent history that
    /// changed `path`: on a branch whose changes arrive through a forge's merge, squash or
    /// rebase button, the time the forge wrote the commit that brought the change in.
    /// `Ok(None)` when no commit there ever changed it, or when there is no base commit.
    pub fn last_change_on_base(&self, path: &str) -> Result<Option<i64>> {
        let Some(base) = self.base else {
            return Ok(None);
        };
        let entry_id = |tree: &Tree<'_>| -> Option<Oid> {
            tree.get_path(std::path::Path::new(path.trim_matches('/')))
                .ok()
                .map(|e| e.id())
        };
        let mut walk = self.repo.revwalk()?;
        walk.simplify_first_parent()?;
        walk.push(base)?;
        for oid in walk {
            let commit = self.repo.find_commit(oid?)?;
            let here = entry_id(&commit.tree()?);
            let before = match commit.parent(0) {
                Ok(p) => entry_id(&p.tree()?),
                Err(_) => None,
            };
            if here != before {
                return Ok(Some(commit.committer().when().seconds()));
            }
        }
        Ok(None)
    }

    /// Commits between the base and `HEAD` as `(short_oid, message)` (empty when staged).
    /// The full object id of a commit named by an abbreviated id or any revision.
    pub fn full_oid(&self, rev: &str) -> Result<String> {
        let obj = self
            .repo
            .revparse_single(rev)
            .with_context(|| format!("cannot resolve `{rev}`"))?;
        Ok(obj.peel_to_commit()?.id().to_string())
    }

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

    /// The commits between the base and `HEAD` with their authorship (empty when staged).
    pub fn commit_details(&self) -> Result<Vec<CommitDetail>> {
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
            let author = commit.author();
            let committer = commit.committer();
            out.push(CommitDetail {
                sha: format!("{oid}"),
                author_name: author.name().unwrap_or("").to_string(),
                author_email: author.email().unwrap_or("").to_string(),
                committer_email: committer.email().unwrap_or("").to_string(),
                message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
                parent_count: commit.parent_count(),
            });
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

/// Transports the base fetch may use. `ext::` and `fd::` run a command as the transport, and
/// `git://` can go through `core.gitProxy`.
const FETCH_TRANSPORTS: &[&str] = &["https", "http", "ssh", "file"];

/// `-c` arguments for the base fetch. `git` reads the repository's own `.git/config`, which a
/// hostile repository controls, and several keys there name a command: hooks, the file-system
/// monitor, a transport. A fetch needs none of them, so they are overridden (`-c` wins over
/// every configuration file). The upload-pack program is not among them: git reads
/// `remote.<name>.uploadpack` first value first, so the fetch passes `--upload-pack` instead. The runner legitimately sets three
/// other command keys (`core.sshCommand`, `credential.helper`, `core.askPass`), so only a
/// value the repository's own configuration sets is replaced, by the global or system value
/// (#364). `entries` is every configuration entry in priority order: name, value, and whether
/// it comes from the repository (local or worktree scope).
fn fetch_overrides_from(entries: &[(String, String, bool)]) -> Vec<String> {
    let mut out: Vec<String> = [
        "core.fsmonitor=false",
        "core.hooksPath=/dev/null",
        "fetch.recurseSubmodules=false",
        "gc.auto=0",
        "maintenance.auto=false",
        "protocol.allow=never",
        "protocol.ext.allow=never",
        "protocol.fd.allow=never",
        "protocol.git.allow=never",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    out.extend(
        FETCH_TRANSPORTS
            .iter()
            .map(|t| format!("protocol.{t}.allow=always")),
    );
    let global = |name: &str| {
        entries
            .iter()
            .filter(|(n, _, l)| !*l && n == name)
            .map(|(_, v, _)| v.clone())
            .collect::<Vec<_>>()
    };
    let mut seen: Vec<&str> = Vec::new();
    for (name, _, is_local) in entries {
        if !*is_local || seen.contains(&name.as_str()) {
            continue;
        }
        seen.push(name);
        let lower = name.to_ascii_lowercase();
        // A transport the repository allows for itself.
        if let Some(t) = lower
            .strip_prefix("protocol.")
            .and_then(|r| r.strip_suffix(".allow"))
        {
            if !FETCH_TRANSPORTS.contains(&t) {
                out.push(format!("{name}=never"));
            }
        } else if lower.starts_with("credential.") && lower.ends_with(".helper") {
            // Multi-valued: an empty value clears the list, then the runner's own helpers.
            out.push(format!("{name}="));
            out.extend(global(name).into_iter().map(|v| format!("{name}={v}")));
        } else if lower == "core.sshcommand" || lower == "core.askpass" {
            let fallback = if lower == "core.sshcommand" {
                "ssh"
            } else {
                ""
            };
            let value = global(name).pop().unwrap_or_else(|| fallback.to_string());
            out.push(format!("{name}={value}"));
        }
    }
    out
}

/// [`fetch_overrides_from`] over the repository's configuration. When it cannot be read, the
/// three runner keys are overridden in every scope.
fn fetch_overrides(repo: &Repository) -> Vec<String> {
    let entries: Option<Vec<(String, String, bool)>> = repo.config().ok().and_then(|config| {
        let mut out = Vec::new();
        let mut iter = config.entries(None).ok()?;
        while let Some(entry) = iter.next() {
            let entry = entry.ok()?;
            let is_local = matches!(
                entry.level(),
                git2::ConfigLevel::Local | git2::ConfigLevel::Worktree | git2::ConfigLevel::App
            );
            out.push((
                entry.name().ok()?.to_string(),
                entry.value().unwrap_or("").to_string(),
                is_local,
            ));
        }
        Some(out)
    });
    let strict = || {
        vec![
            ("credential.helper".to_string(), String::new(), true),
            ("core.sshCommand".to_string(), String::new(), true),
            ("core.askPass".to_string(), String::new(), true),
        ]
    };
    fetch_overrides_from(&entries.unwrap_or_else(strict))
}

fn deepen_git_history(candidates: &[String], base_ref: &str, repo: &Repository) -> Vec<String> {
    let mut fetch_errors = Vec::new();
    if !is_ci_environment() {
        // Do not make unsolicited network requests in local developer environments
        return fetch_errors;
    }
    // `DISCIPLINE_NO_NETWORK=1` keeps every request off the network, this fetch included.
    if std::env::var("DISCIPLINE_NO_NETWORK").is_ok_and(|v| v == "1") {
        fetch_errors.push(
            "git fetch skipped: DISCIPLINE_NO_NETWORK=1 keeps every request off the network"
                .to_string(),
        );
        return fetch_errors;
    }

    let root = match repo.workdir() {
        Some(r) => r,
        None => return fetch_errors,
    };
    // repo.path() resolves the real gitdir even in worktrees where .git is a gitdir reference file
    let is_shallow = repo.path().join("shallow").exists();

    let overrides = fetch_overrides(repo);
    let mut run_fetch = |args: &[&str]| {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        let mut cmd = std::process::Command::new("git");
        for o in &overrides {
            cmd.arg("-c").arg(o);
        }
        // `--upload-pack` on the command line: `-c` cannot outrank the repository's value.
        let (sub, rest) = args.split_first().expect("a git subcommand");
        cmd.current_dir(root)
            .arg(sub)
            .arg("--upload-pack=git-upload-pack")
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let Ok(mut child) = cmd.spawn() else {
            return;
        };

        let start = Instant::now();
        let timeout = Duration::from_secs(30);
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        let _ = child.kill();
                        timed_out = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        let output = child.wait_with_output();
        if timed_out {
            fetch_errors.push(format!("git {}: timed out after 30s", args.join(" ")));
        } else if let Ok(out) = output {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if !stderr.is_empty() {
                    fetch_errors.push(format!("git {}: {stderr}", args.join(" ")));
                }
            }
        }
    };

    // 1. If base ref is a commit SHA (>=7 hex characters), attempt targeted fetch
    let is_sha = base_ref.len() >= 7 && base_ref.chars().all(|c| c.is_ascii_hexdigit());
    if is_sha {
        run_fetch(&[
            "fetch",
            "--no-tags",
            "--depth=100",
            "origin",
            "--",
            base_ref,
        ]);
    }

    // 2. Try candidate branch names if they don't contain revision selectors (~, ^) or start with a dash
    for cand in candidates {
        let ref_name = cand.strip_prefix("origin/").unwrap_or(cand);
        if !is_sha
            && !ref_name.contains('~')
            && !ref_name.contains('^')
            && !ref_name.starts_with('-')
        {
            run_fetch(&[
                "fetch",
                "--no-tags",
                "--depth=100",
                "origin",
                "--",
                ref_name,
            ]);
        }
    }

    // 3. If repository is shallow, attempt unshallowing
    if is_shallow {
        run_fetch(&["fetch", "--no-tags", "--unshallow", "origin"]);
    }

    fetch_errors
}

pub fn is_binary_file(path: &str, bytes: &[u8]) -> bool {
    // 1. A known text extension is text, whatever its first bytes: `MZ = 0` is valid Python
    //    and the DOS/PE header, and a file read as binary is skipped by every gate that reads
    //    whole files.
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
        // Language-pack extensions not listed above: a gate that reads them must not lose
        // one to a NUL byte.
        "cs",
        "cts",
        "mts",
        "dart",
        "inc",
        "m",
        "mm",
        "phtml",
        "rake",
        "sc",
        "gemspec",
    ];

    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if KNOWN_TEXT_EXTS.contains(&ext.as_str())
        || matches!(name, "Gemfile" | "Rakefile" | "Dockerfile" | "Makefile")
    {
        return false;
    }

    // 2. Magic numbers with a non-printable byte cannot open a text file. Printable ones
    //    (`MZ`, `%PDF-`, `GIF89a`, `BZh`, `PAR1`, `ARROW1`) are left to the NUL check below,
    //    which every real file of those formats meets in its first kilobyte.
    if bytes.starts_with(b"\x7fELF") // ELF
        || bytes.starts_with(b"\xfe\xed\xfa\xce") // Mach-O 32-bit
        || bytes.starts_with(b"\xfe\xed\xfa\xcf") // Mach-O 64-bit
        || bytes.starts_with(b"\xce\xfa\xed\xfe") // Mach-O 32-bit rev
        || bytes.starts_with(b"\xcf\xfa\xed\xfe") // Mach-O 64-bit rev
        || bytes.starts_with(b"\xca\xfe\xba\xbe") // Mach-O fat / Java class
        || bytes.starts_with(b"\0asm") // WebAssembly
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n") // PNG
        || bytes.starts_with(b"\xff\xd8\xff") // JPEG
        || (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP") // WebP
        || bytes.starts_with(b"PK\x03\x04") // Zip / Jar
        || bytes.starts_with(b"PK\x05\x06") // Empty Zip
        || bytes.starts_with(b"PK\x07\x08") // Spanned Zip
        || bytes.starts_with(b"\x1f\x8b") // Gzip
        || bytes.starts_with(b"\xfd7zXZ\x00") // XZ
        || bytes.starts_with(b"7z\xbc\xaf\x27\x1c") // 7z
        || (bytes.len() >= 262 && &bytes[257..262] == b"ustar") // Tar
        || bytes.starts_with(b"SQLite format 3\0")
    // SQLite
    {
        return true;
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

    #[test]
    fn a_known_text_extension_is_text_and_printable_magic_needs_a_nul() {
        // Text that starts with a printable magic number.
        assert!(!is_binary_file("tests/test_calc.py", b"MZ = 0\n"));
        assert!(!is_binary_file("docs/plan.md", b"MZ ships later\n"));
        assert!(!is_binary_file("notes", b"MZ = 0\n"));
        assert!(!is_binary_file("NOTES", b"BZh is a prefix\n"));
        assert!(!is_binary_file("data", b"%PDF-like text\n"));
        // A known text extension stays text even with a non-printable magic number.
        assert!(!is_binary_file("src/a.py", b"\x7fELF\x02\x01"));
        // Language-pack sources and build files stay text even with a NUL byte.
        for path in [
            "src/Svc.cs",
            "lib/a.dart",
            "src/a.m",
            "Dockerfile",
            "Gemfile",
        ] {
            assert!(!is_binary_file(path, b"x\0y"), "{path}");
        }
        // Real files of these formats.
        assert!(is_binary_file("tool", b"MZ\x90\x00\x03\x00"));
        assert!(is_binary_file("tool", b"\x7fELF\x02\x01\x01\x00"));
        assert!(is_binary_file("image", b"\x89PNG\r\n\x1a\nIHDR"));
        assert!(is_binary_file("a.pdf", b"%PDF-1.7\n"));
        assert!(is_binary_file("blob", b"plain then \0"));
    }

    #[test]
    fn the_base_fetch_overrides_repository_commands_and_keeps_the_runners() {
        let e = |n: &str, v: &str, local: bool| (n.to_string(), v.to_string(), local);
        let fixed = fetch_overrides_from(&[]);
        for want in [
            "core.fsmonitor=false",
            "core.hooksPath=/dev/null",
            "protocol.allow=never",
            "protocol.ext.allow=never",
            "protocol.https.allow=always",
        ] {
            assert!(fixed.contains(&want.to_string()), "{want}: {fixed:?}");
        }
        // Nothing the repository sets: the runner's own values are left alone.
        let runner = [
            e("credential.helper", "!gh auth git-credential", false),
            e("core.sshCommand", "ssh -i key", false),
        ];
        assert_eq!(fetch_overrides_from(&runner), fixed);
        // The repository's helper is cleared and the runner's put back, in order.
        let mut both = runner.to_vec();
        both.push(e("credential.helper", "!touch pwn", true));
        both.push(e(
            "credential.https://example.com.helper",
            "!touch pwn",
            true,
        ));
        both.push(e("core.sshCommand", "touch pwn; ssh", true));
        both.push(e("core.askPass", "touch pwn", true));
        both.push(e("protocol.ext.allow", "always", true));
        both.push(e("protocol.file.allow", "always", true));
        let got = fetch_overrides_from(&both);
        let extra: Vec<&str> = got[fixed.len()..].iter().map(String::as_str).collect();
        assert_eq!(
            extra,
            [
                "credential.helper=",
                "credential.helper=!gh auth git-credential",
                "credential.https://example.com.helper=",
                "core.sshCommand=ssh -i key",
                "core.askPass=",
                "protocol.ext.allow=never",
            ]
        );
        // With no runner value, ssh falls back to plain `ssh`.
        let only = fetch_overrides_from(&[e("core.sshCommand", "touch pwn; ssh", true)]);
        assert_eq!(only.last().unwrap(), "core.sshCommand=ssh");
    }

    use super::*;

    #[test]
    fn the_named_head_is_the_commit_or_the_range_end() {
        assert_eq!(named_head(Some("abc"), None).as_deref(), Some("abc"));
        assert_eq!(named_head(None, Some("a..b")).as_deref(), Some("b"));
        assert_eq!(named_head(None, Some("a...b")).as_deref(), Some("b"));
        assert_eq!(named_head(None, Some("a..")), None);
        assert_eq!(named_head(None, Some("a")), None);
        assert_eq!(named_head(None, None), None);
    }

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

    fn payload_files(
        vars: &[(&'static str, &str)],
    ) -> (
        Vec<tempfile::NamedTempFile>,
        std::collections::HashMap<&'static str, String>,
    ) {
        let mut files = Vec::new();
        let mut env = std::collections::HashMap::new();
        for (var, json) in vars {
            let f = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(f.path(), json).unwrap();
            env.insert(*var, f.path().to_string_lossy().to_string());
            files.push(f);
        }
        (files, env)
    }

    #[test]
    fn event_payload_order_is_forgejo_then_gitea_then_github() {
        let all = [
            ("GITHUB_EVENT_PATH", r#"{"who":"github"}"#),
            ("GITEA_EVENT_PATH", r#"{"who":"gitea"}"#),
            ("FORGEJO_EVENT_PATH", r#"{"who":"forgejo"}"#),
        ];
        let who = |vars: &[(&'static str, &str)]| {
            let (_files, env) = payload_files(vars);
            event_payload_with_env(|k| env.get(k).cloned())
                .map(|v| v["who"].as_str().unwrap().to_string())
        };
        assert_eq!(who(&all).as_deref(), Some("forgejo"));
        assert_eq!(who(&all[..2]).as_deref(), Some("gitea"));
        assert_eq!(who(&all[..1]).as_deref(), Some("github"));
        assert_eq!(who(&[]), None);
    }

    #[test]
    fn event_payload_does_not_fall_through_a_broken_first_choice() {
        let (_files, mut env) = payload_files(&[("GITHUB_EVENT_PATH", r#"{"who":"github"}"#)]);
        env.insert("FORGEJO_EVENT_PATH", "/nonexistent/event.json".to_string());
        assert_eq!(event_payload_with_env(|k| env.get(k).cloned()), None);
        let (_f2, mut env2) = payload_files(&[
            ("GITHUB_EVENT_PATH", r#"{"who":"github"}"#),
            ("FORGEJO_EVENT_PATH", "not json"),
        ]);
        env2.insert("GITEA_EVENT_PATH", String::new());
        assert_eq!(event_payload_with_env(|k| env2.get(k).cloned()), None);
    }

    #[test]
    fn try_event_payload_is_none_when_no_variable_is_set_or_all_are_empty() {
        assert_eq!(try_event_payload_with_env(|_| None).unwrap(), None);
        let empty = |k: &str| EVENT_PATH_VARS.contains(&k).then(|| "  ".to_string());
        assert_eq!(try_event_payload_with_env(empty).unwrap(), None);
    }

    #[test]
    fn try_event_payload_returns_the_first_set_file_whatever_it_holds() {
        let (_files, mut env) = payload_files(&[
            ("GITHUB_EVENT_PATH", r#"{"who":"github"}"#),
            ("GITEA_EVENT_PATH", "[]"),
        ]);
        // An empty earlier variable is not set; the next one decides.
        env.insert("FORGEJO_EVENT_PATH", String::new());
        assert_eq!(
            try_event_payload_with_env(|k| env.get(k).cloned()).unwrap(),
            Some(serde_json::json!([]))
        );
    }

    #[test]
    fn try_event_payload_errs_on_an_unreadable_first_choice_and_names_its_variable() {
        let (_files, mut env) = payload_files(&[("GITHUB_EVENT_PATH", r#"{"who":"github"}"#)]);
        env.insert("FORGEJO_EVENT_PATH", "/nonexistent/event.json".to_string());
        let err = try_event_payload_with_env(|k| env.get(k).cloned()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("`FORGEJO_EVENT_PATH`"), "{msg}");
        assert!(msg.contains("cannot be read"), "{msg}");
        assert!(!msg.contains("/nonexistent"), "{msg}");
        assert!(!msg.contains("GITHUB_EVENT_PATH"), "{msg}");
        // The infallible reader still sees no payload, and still does not fall through.
        assert_eq!(event_payload_with_env(|k| env.get(k).cloned()), None);
    }

    #[test]
    fn try_event_payload_errs_on_a_file_that_is_not_json_without_quoting_it() {
        let (_files, env) = payload_files(&[
            ("GITHUB_EVENT_PATH", r#"{"who":"github"}"#),
            ("GITEA_EVENT_PATH", r#"{"token": SENTINEL-not-for-output"#),
        ]);
        let err = try_event_payload_with_env(|k| env.get(k).cloned()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("`GITEA_EVENT_PATH`"), "{msg}");
        assert!(msg.contains("not valid JSON"), "{msg}");
        assert!(!msg.contains("SENTINEL"), "{msg}");
        assert!(!msg.contains(env["GITEA_EVENT_PATH"].as_str()), "{msg}");
        assert_eq!(event_payload_with_env(|k| env.get(k).cloned()), None);
    }

    /// A payload carrying `before` makes the run a push with no event name set.
    #[test]
    fn a_payload_with_before_alone_is_a_push() {
        let (_files, env) = payload_files(&[("GITHUB_EVENT_PATH", r#"{"before":"abc1234"}"#)]);
        assert!(is_push_event_environment_with_env(|k| env.get(k).cloned()));
        let (_f2, env2) = payload_files(&[("GITHUB_EVENT_PATH", r#"{"after":"abc1234"}"#)]);
        assert!(!is_push_event_environment_with_env(|k| env2
            .get(k)
            .cloned()));
    }

    #[test]
    fn normalize_before_table() {
        let sha = "fedcba9876543210fedcba9876543210fedcba98";
        let cases: [(&str, Option<&str>); 7] = [
            ("0000000000000000000000000000000000000000", Some("HEAD~1")),
            ("0", Some("HEAD~1")),
            ("", None),
            ("   ", None),
            (sha, Some(sha)),
            (" fedcba9 ", Some("fedcba9")),
            ("abc", None),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_before(input).as_deref(), want, "input {input:?}");
        }
    }

    const PREVIOUS_HEAD: &str = "1111111111111111111111111111111111111111";

    /// GitHub's `pull_request` event, action `synchronize`: `before` and `after` sit
    /// beside the `pull_request` object.
    fn synchronize_payload() -> String {
        format!(
            r#"{{"action":"synchronize","number":7,"before":"{PREVIOUS_HEAD}","after":"2222222222222222222222222222222222222222","pull_request":{{"number":7,"base":{{"ref":"main"}}}}}}"#
        )
    }

    fn push_payload() -> String {
        format!(r#"{{"ref":"refs/heads/work","before":"{PREVIOUS_HEAD}"}}"#)
    }

    /// `(is a push, named base)` for one payload under `GITHUB_EVENT_PATH` plus `vars`.
    fn push_and_base(payload: &str, vars: &[(&'static str, &str)]) -> (bool, Option<String>) {
        let (_files, mut env) = payload_files(&[("GITHUB_EVENT_PATH", payload)]);
        for (k, v) in vars {
            env.insert(*k, v.to_string());
        }
        (
            is_push_event_environment_with_env(|k| env.get(k).cloned()),
            named_base_ref_with_env(None, None, None, |k| env.get(k).cloned()),
        )
    }

    #[test]
    fn a_synchronize_payload_on_a_pull_request_event_is_not_a_push() {
        for name in [
            "GITHUB_EVENT_NAME",
            "GITEA_EVENT_NAME",
            "FORGEJO_EVENT_NAME",
        ] {
            for event in ["pull_request", "pull_request_target"] {
                let (push, base) = push_and_base(
                    &synchronize_payload(),
                    &[(name, event), ("GITHUB_BASE_REF", "main")],
                );
                assert!(!push, "{name}={event}");
                assert_eq!(base.as_deref(), Some("origin/main"), "{name}={event}");
            }
        }
    }

    /// The `pull_request` object alone decides when nothing names the event.
    #[test]
    fn a_payload_with_a_pull_request_and_no_event_name_is_not_a_push() {
        let (push, base) = push_and_base(&synchronize_payload(), &[("GITHUB_BASE_REF", "main")]);
        assert!(!push);
        assert_eq!(base.as_deref(), Some("origin/main"));
        // With no base variable either, nothing names a base: never the previous head.
        assert_eq!(push_and_base(&synchronize_payload(), &[]), (false, None));
    }

    /// The event name alone decides when the payload has `before` and no `pull_request`.
    #[test]
    fn a_named_event_other_than_push_does_not_read_before_from_the_payload() {
        for event in ["pull_request", "merge_group", "workflow_dispatch"] {
            let (push, base) = push_and_base(
                &push_payload(),
                &[("GITHUB_EVENT_NAME", event), ("GITHUB_BASE_REF", "main")],
            );
            assert!(!push, "{event}");
            assert_eq!(base.as_deref(), Some("origin/main"), "{event}");
        }
    }

    /// Control: a push keeps its `before`, named or not, and an empty event name is not a
    /// name.
    #[test]
    fn a_push_payload_still_gives_its_before() {
        for vars in [
            &[("GITHUB_EVENT_NAME", "push")][..],
            &[("GITEA_EVENT_NAME", "push")][..],
            &[("FORGEJO_EVENT_NAME", "push")][..],
            &[("GITHUB_EVENT_NAME", "push"), ("GITHUB_BASE_REF", "main")][..],
            &[("GITHUB_BASE_REF", "main")][..],
            &[("GITHUB_EVENT_NAME", " ")][..],
            &[][..],
        ] {
            assert_eq!(
                push_and_base(&push_payload(), vars),
                (true, Some(PREVIOUS_HEAD.to_string())),
                "{vars:?}"
            );
        }
    }

    /// Control: a `before` variable is a push whatever the payload holds.
    #[test]
    fn a_before_variable_is_still_a_push_beside_a_pull_request_payload() {
        let sha = "3333333333333333333333333333333333333333";
        assert_eq!(
            push_and_base(&synchronize_payload(), &[("GITHUB_EVENT_BEFORE", sha)]),
            (true, Some(sha.to_string()))
        );
    }

    #[test]
    fn base_ref_reads_the_forge_payload_when_event_paths_conflict() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let c = "c".repeat(40);
        let json = |sha: &str| format!(r#"{{"before": "{sha}"}}"#);
        let (_files, env) = payload_files(&[
            ("GITHUB_EVENT_PATH", &json(&a)),
            ("GITEA_EVENT_PATH", &json(&b)),
            ("FORGEJO_EVENT_PATH", &json(&c)),
        ]);
        let base = |drop: &[&str]| {
            detect_base_ref_with_env(None, None, None, |k| {
                if k == "GITHUB_EVENT_NAME" {
                    Some("push".into())
                } else if drop.contains(&k) {
                    None
                } else {
                    env.get(k).cloned()
                }
            })
        };
        assert_eq!(base(&[]), c);
        assert_eq!(base(&["FORGEJO_EVENT_PATH"]), b);
        assert_eq!(base(&["FORGEJO_EVENT_PATH", "GITEA_EVENT_PATH"]), a);
    }

    #[test]
    fn test_detect_base_ref_push_event_with_github_event_path() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            temp.path(),
            r#"{"before": "fedcba9876543210fedcba9876543210fedcba98", "after": "11111111"}"#,
        )
        .unwrap();

        let event_path = temp.path().to_string_lossy().to_string();
        let lookup = |k: &str| {
            if k == "GITHUB_EVENT_NAME" {
                Some("push".to_string())
            } else if k == "GITHUB_EVENT_PATH" {
                Some(event_path.clone())
            } else {
                None
            }
        };

        assert_eq!(
            detect_base_ref_with_env(None, None, None, lookup),
            "fedcba9876543210fedcba9876543210fedcba98"
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

    /// #530: the environment-named base is told apart from the `origin/main` fallback.
    #[test]
    fn a_base_named_by_nothing_is_none_and_the_fallback_is_origin_main() {
        assert_eq!(named_base_ref_with_env(None, None, None, |_| None), None);
        assert_eq!(
            named_base_ref_with_env(None, None, None, |k| {
                (k == "DISCIPLINE_BASE_REF").then(|| "origin/main".to_string())
            }),
            Some("origin/main".to_string()),
            "a variable that names the fallback's ref still names the base"
        );
        assert_eq!(
            named_base_ref_with_env(None, None, None, |k| {
                (k == "GITHUB_BASE_REF").then(|| "develop".to_string())
            }),
            Some("origin/develop".to_string())
        );
        assert_eq!(
            named_base_ref_with_env(Some("x"), None, None, |_| None),
            Some("x".to_string())
        );
        assert_eq!(
            detect_base_ref_with_env(None, None, None, |k| {
                (k == "DISCIPLINE_BASE_REF").then(|| "  ".to_string())
            }),
            "origin/main"
        );
    }

    #[test]
    fn test_detect_base_ref_fallback() {
        assert_eq!(
            detect_base_ref_with_env(None, None, None, |_| None),
            "origin/main"
        );
    }

    #[test]
    fn test_format_discover_error_owner_code() {
        let err = git2::Error::new(
            git2::ErrorCode::Owner,
            git2::ErrorClass::Config,
            "repository path '/workspace' is not owned by current user",
        );
        let path = std::path::Path::new("/workspace");
        let formatted = format_discover_error(&err, path).to_string();

        assert!(
            formatted.contains("code=Owner (-36)"),
            "diagnostic must name code=Owner (-36): {formatted}"
        );
        assert!(
            formatted.contains("safe.directory"),
            "diagnostic must recommend safe.directory remediation: {formatted}"
        );
        assert!(
            formatted.contains("--user"),
            "diagnostic must recommend matching --user remediation: {formatted}"
        );
        assert!(
            formatted.contains("DISCIPLINE_TRUST_WORKSPACE")
                && formatted.contains("--trust-workspace"),
            "diagnostic must recommend --trust-workspace / DISCIPLINE_TRUST_WORKSPACE: {formatted}"
        );
    }

    #[test]
    fn test_format_discover_error_owner_message() {
        let err = git2::Error::from_str(
            "fatal: repository path '/workspace' is not owned by current user",
        );
        let path = std::path::Path::new("/workspace");
        let formatted = format_discover_error(&err, path).to_string();

        assert!(
            formatted.contains("code=Owner (-36)"),
            "message containing 'not owned by current user' must trigger ownership diagnostic: {formatted}"
        );
    }

    #[test]
    fn test_format_discover_error_not_git_repo() {
        let err = git2::Error::new(
            git2::ErrorCode::NotFound,
            git2::ErrorClass::Repository,
            "could not find repository",
        );
        let path = std::path::Path::new("/tmp/not-a-repo");
        let formatted = format_discover_error(&err, path).to_string();

        assert!(
            formatted.contains("not inside a git repository"),
            "non-owner error must report generic not inside git repository context: {formatted}"
        );
        assert!(
            !formatted.contains("code=Owner (-36)"),
            "non-owner error must not claim code=Owner: {formatted}"
        );
    }

    #[cfg(unix)]
    /// A repository with one commit holding `a.txt`, opened with that commit as base and
    /// the working tree as head.
    fn repo_with_base_file() -> (tempfile::TempDir, GitCtx, Oid) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("a.txt"), "base\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        let blob = tree.get_name("a.txt").unwrap().id();
        drop(tree);
        let git = GitCtx {
            repo,
            base: Some(commit),
            base_label: "base".to_string(),
            staged: false,
        };
        (dir, git, blob)
    }

    #[cfg(unix)]
    fn remove_loose_object(dir: &std::path::Path, oid: Oid) {
        let hex = oid.to_string();
        let obj = dir.join(".git/objects").join(&hex[..2]).join(&hex[2..]);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&obj, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::remove_file(obj).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sides_tell_an_absent_file_from_a_failed_read() {
        let (dir, git, blob) = repo_with_base_file();
        std::fs::write(dir.path().join("a.txt"), "head\n").unwrap();

        let both = git.sides("a.txt", "a.txt").unwrap();
        assert_eq!(both.base.as_deref(), Some("base\n"));
        assert_eq!(both.head.as_deref(), Some("head\n"));
        // Absent is Ok(None) on each side, not an error.
        let absent = git.sides("nope.txt", "nope.txt").unwrap();
        assert_eq!(absent, Sides::default());

        remove_loose_object(dir.path(), blob);
        let err = git.sides("a.txt", "a.txt").unwrap_err();
        let shown = format!("{err:#}");
        assert!(shown.contains("`a.txt` on the base side"), "{shown}");
        // The head side still reads: only the base blob is gone.
        assert_eq!(
            git.head_content("a.txt").unwrap().as_deref(),
            Some("head\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_recorded_read_failure_is_surfaced_not_dropped() {
        let (dir, git, blob) = repo_with_base_file();
        let reads = ReadRecorder::new();
        let base = reads.base(&git);
        assert_eq!(base("a.txt").as_deref(), Some("base\n"));
        assert_eq!(base("nope.txt"), None);
        reads.finish().unwrap();

        remove_loose_object(dir.path(), blob);
        assert_eq!(base("a.txt"), None);
        let err = reads.finish().unwrap_err();
        assert!(format!("{err:#}").contains("`a.txt` on the base side"));
        // Reported once: the recorder is drained.
        reads.finish().unwrap();
    }

    #[test]
    fn a_tree_or_a_gitlink_is_not_a_file_on_either_side() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("somedir")).unwrap();
        std::fs::write(dir.path().join("somedir/in.txt"), "x\n").unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_path(std::path::Path::new("somedir/in.txt"))
            .unwrap();
        let id = repo.blob(b"x\n").unwrap();
        let entry = git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o160000,
            uid: 0,
            gid: 0,
            file_size: 0,
            id,
            flags: 0,
            flags_extended: 0,
            path: b"vendor/sub".to_vec(),
        };
        index.add(&entry).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.invalid").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        drop(tree);
        std::fs::create_dir_all(dir.path().join("vendor/sub")).unwrap();
        for staged in [false, true] {
            let git = GitCtx {
                repo: Repository::open(dir.path()).unwrap(),
                base: Some(commit),
                base_label: "base".to_string(),
                staged,
            };
            for path in ["somedir", "vendor/sub"] {
                assert_eq!(git.base_content(path).unwrap(), None, "{path} base");
                assert_eq!(
                    git.head_content(path).unwrap(),
                    None,
                    "{path} head, staged={staged}"
                );
            }
            assert_eq!(
                git.base_content("somedir/in.txt").unwrap().as_deref(),
                Some("x\n")
            );
        }
    }
}
