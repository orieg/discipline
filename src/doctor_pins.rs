//! `discipline doctor`: what a 40-character SHA pin does not prove on its own.
//!
//! - **Imposter commits** (`imposter-commit`): GitHub resolves `owner/repo@<sha>` across
//!   the repository's fork network, so a commit that exists only in a fork runs under the
//!   parent's name. Each pinned commit must be reachable from a branch or tag of
//!   `owner/repo` itself: the head of a branch or tag, or an ancestor of the default
//!   branch or of another branch or tag (compare API), within [`COMPARE_BUDGET`].
//! - **Mutable nested refs** (`nested-action-pins`): a pinned action whose own
//!   `action.yml` (or a pinned reusable workflow) at that commit uses a tag or branch
//!   ref, or a Docker image without a digest, still runs code that can change. One level
//!   deep: the actions it calls are not read in turn.
//!
//! Reads go through `crate::forge` (AGENTS.md §3.3). GitHub only: Gitea and Forgejo
//! resolve `uses:` through the instance's configured actions source, and GitLab has no
//! `uses:`. The report counts the references it resolved and those it did not check.

use crate::doctor::{access_hint, Finding, Status};
use crate::forge::{read_all, Forge, ForgeApi, ForgeError, ForgeErrorKind, ForgeKind};
use std::collections::BTreeMap;

/// Finding ids this module reports.
pub const IDS: &[&str] = &["imposter-commit", "nested-action-pins"];

/// Compare requests spent per pinned commit on branches and tags other than the default
/// branch, after the cheap checks (default branch, branch and tag heads).
pub const COMPARE_BUDGET: usize = 25;

/// A `uses:` reference, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uses {
    /// `./path`: the calling repository at the same commit.
    Local,
    /// `docker://image[:tag][@sha256:...]`.
    Docker { image: String, digest: bool },
    /// `owner/repo[/path]@ref`.
    Remote {
        owner: String,
        repo: String,
        path: String,
        git_ref: String,
    },
    /// Anything else (an expression, a malformed value).
    Other(String),
}

/// Whether `r` is a full-length commit SHA.
pub fn is_sha(r: &str) -> bool {
    r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Uses {
    pub fn parse(value: &str) -> Self {
        let v = value.trim();
        if v.starts_with("./") || v == "." {
            return Uses::Local;
        }
        if let Some(image) = v.strip_prefix("docker://") {
            return Uses::Docker {
                image: image.to_string(),
                digest: image.contains("@sha256:"),
            };
        }
        let Some((target, git_ref)) = v.rsplit_once('@') else {
            return Uses::Other(v.to_string());
        };
        let mut parts = target.splitn(3, '/');
        let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
            return Uses::Other(v.to_string());
        };
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        if !ok(owner) || !ok(repo) || git_ref.is_empty() || git_ref.contains("${{") {
            return Uses::Other(v.to_string());
        }
        Uses::Remote {
            owner: owner.to_string(),
            repo: repo.to_string(),
            path: parts
                .next()
                .unwrap_or_default()
                .trim_matches('/')
                .to_string(),
            git_ref: git_ref.to_string(),
        }
    }

    /// Whether the reference names content that cannot change.
    pub fn immutable(&self) -> bool {
        match self {
            Uses::Local => true,
            Uses::Docker { digest, .. } => *digest,
            Uses::Remote { git_ref, .. } => is_sha(git_ref),
            Uses::Other(_) => false,
        }
    }
}

/// Every `uses:` value of a workflow (job-level reusable workflows and steps) or of an
/// action's metadata (composite steps), plus a Docker action's `runs.image`.
pub fn uses_values(doc: &serde_yaml::Value) -> Vec<String> {
    let mut out = Vec::new();
    let steps = |v: Option<&serde_yaml::Value>, out: &mut Vec<String>| {
        for s in v.and_then(|s| s.as_sequence()).into_iter().flatten() {
            if let Some(u) = s.get("uses").and_then(|u| u.as_str()) {
                out.push(u.to_string());
            }
        }
    };
    if let Some(jobs) = doc.get("jobs").and_then(|j| j.as_mapping()) {
        for job in jobs.values() {
            if let Some(u) = job.get("uses").and_then(|u| u.as_str()) {
                out.push(u.to_string());
            }
            steps(job.get("steps"), &mut out);
        }
    }
    if let Some(runs) = doc.get("runs") {
        steps(runs.get("steps"), &mut out);
        if let Some(image) = runs.get("image").and_then(|i| i.as_str()) {
            if image.starts_with("docker://") {
                out.push(image.to_string());
            }
        }
    }
    out
}

/// A pinned remote reference, and where it is used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pins {
    /// `(owner, repo, path, sha)` to the files that use it.
    pub pinned: BTreeMap<(String, String, String, String), Vec<String>>,
    /// Remote references that are not SHA-pinned (not checked here).
    pub unpinned: usize,
}

/// The SHA-pinned remote references of the local workflow and action files.
pub fn collect(files: &[(String, String)]) -> Pins {
    let mut pins = Pins::default();
    for (path, content) in files {
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(content) else {
            continue;
        };
        for u in uses_values(&doc) {
            match Uses::parse(&u) {
                Uses::Remote {
                    owner,
                    repo,
                    path: sub,
                    git_ref,
                } if is_sha(&git_ref) => {
                    let where_ = pins
                        .pinned
                        .entry((owner, repo, sub, git_ref.to_ascii_lowercase()))
                        .or_default();
                    if !where_.contains(path) {
                        where_.push(path.clone());
                    }
                }
                Uses::Remote { .. } | Uses::Other(_) => pins.unpinned += 1,
                Uses::Docker { digest: false, .. } => pins.unpinned += 1,
                _ => {}
            }
        }
    }
    pins
}

/// Whether a pinned commit is reachable from a ref of its own repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// Reachable from this ref (`branch main`, `tag v4.2.0`).
    From(String),
    /// Every branch and tag was checked: none contains the commit.
    Imposter { refs: usize },
    /// Not found within the budget: `compared` of `refs` other refs compared.
    Unconfirmed { compared: usize, refs: usize },
    /// The repository or a list is not visible to this token.
    Hidden(String),
    /// The forge could not be read.
    Down(String),
}

fn classify(e: &ForgeError) -> Reach {
    if matches!(e.kind, ForgeErrorKind::NotFound | ForgeErrorKind::Denied) {
        Reach::Hidden(e.to_string())
    } else {
        Reach::Down(e.to_string())
    }
}

/// A path segment safe for [`crate::forge::check_api_path`]: percent-encoded, with a
/// leading dot encoded too (`.github` → `%2Egithub`).
fn segment(s: &str) -> String {
    let enc = crate::forge::encode_segment(s);
    match enc.strip_prefix('.') {
        Some(rest) => format!("%2E{rest}"),
        None => enc,
    }
}

/// What one repository answered, read once per run.
#[derive(Default)]
pub struct RepoCache {
    default_branch: BTreeMap<String, Result<String, Reach>>,
    refs: BTreeMap<String, Result<RepoRefs, Reach>>,
}

#[derive(Clone)]
struct RepoRefs {
    /// `(name, head sha)`.
    branches: Vec<(String, String)>,
    tags: Vec<(String, String)>,
    /// Both lists were read to the end ([`read_all`] refuses more than its page limit).
    complete: bool,
}

fn refs(list: &[serde_json::Value]) -> Vec<(String, String)> {
    list.iter()
        .filter_map(|b| {
            Some((
                b.get("name")?.as_str()?.to_string(),
                b.pointer("/commit/sha")?.as_str()?.to_ascii_lowercase(),
            ))
        })
        .collect()
}

impl RepoCache {
    fn default_branch(
        &mut self,
        api: &dyn ForgeApi,
        forge: &Forge,
        slug: &str,
    ) -> Result<String, Reach> {
        self.default_branch
            .entry(slug.to_string())
            .or_insert_with(|| {
                let info = api
                    .fetch(forge, &format!("repos/{slug}"))
                    .map_err(|e| classify(&e))?;
                info.get("default_branch")
                    .and_then(|b| b.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| Reach::Down(format!("`repos/{slug}` names no default branch")))
            })
            .clone()
    }

    fn refs(&mut self, api: &dyn ForgeApi, forge: &Forge, slug: &str) -> Result<RepoRefs, Reach> {
        self.refs
            .entry(slug.to_string())
            .or_insert_with(|| {
                let mut complete = true;
                let mut list =
                    |what: &str| match read_all(api, forge, &format!("repos/{slug}/{what}")) {
                        Ok(l) => Ok(refs(&l)),
                        // Too many to read: judged on what the budget reaches, never "imposter".
                        Err(e) if e.kind == ForgeErrorKind::Partial => {
                            complete = false;
                            Ok(Vec::new())
                        }
                        Err(e) => Err(classify(&e)),
                    };
                let branches = list("branches")?;
                let tags = list("tags")?;
                Ok(RepoRefs {
                    branches,
                    tags,
                    complete,
                })
            })
            .clone()
    }
}

/// Whether `sha` is reachable from a branch or tag of `slug` (`owner/repo`): first the
/// default branch (one compare), then the heads of every branch and tag, then compares
/// against the other branches and tags, at most [`COMPARE_BUDGET`].
pub fn reach(
    api: &dyn ForgeApi,
    forge: &Forge,
    cache: &mut RepoCache,
    slug: &str,
    sha: &str,
) -> Reach {
    // `compare/{base}...{sha}` is `behind` or `identical` when `sha` is an ancestor of
    // `base`. A 404 (no common ancestor, unknown commit) is "not from this ref".
    let contains = |base: &str| -> Result<bool, Reach> {
        match api.fetch(
            forge,
            &format!("repos/{slug}/compare/{}...{sha}", segment(base)),
        ) {
            Ok(v) => Ok(matches!(
                v.get("status").and_then(|s| s.as_str()),
                Some("behind" | "identical")
            )),
            Err(e) if e.kind == ForgeErrorKind::NotFound => Ok(false),
            Err(e) => Err(classify(&e)),
        }
    };
    let default = match cache.default_branch(api, forge, slug) {
        Ok(b) => b,
        Err(e) => return e,
    };
    match contains(&default) {
        Ok(true) => return Reach::From(format!("branch {default}")),
        Ok(false) => {}
        Err(e) => return e,
    }
    let refs = match cache.refs(api, forge, slug) {
        Ok(r) => r,
        Err(e) => return e,
    };
    for (kind, list) in [("tag", &refs.tags), ("branch", &refs.branches)] {
        if let Some((name, _)) = list.iter().find(|(_, head)| head == sha) {
            return Reach::From(format!("{kind} {name}"));
        }
    }
    let others: Vec<(&str, &str)> = refs
        .branches
        .iter()
        .filter(|(n, _)| *n != default)
        .map(|(n, _)| ("branch", n.as_str()))
        .chain(refs.tags.iter().map(|(n, _)| ("tag", n.as_str())))
        .collect();
    for (kind, name) in others.iter().take(COMPARE_BUDGET) {
        let base = if *kind == "tag" {
            format!("refs/tags/{name}")
        } else {
            name.to_string()
        };
        match contains(&base) {
            Ok(true) => return Reach::From(format!("{kind} {name}")),
            Ok(false) => {}
            Err(e) => return e,
        }
    }
    let total = refs.branches.len() + refs.tags.len();
    if refs.complete && others.len() <= COMPARE_BUDGET {
        Reach::Imposter { refs: total }
    } else {
        Reach::Unconfirmed {
            compared: others.len().min(COMPARE_BUDGET),
            refs: total,
        }
    }
}

/// The metadata a pinned reference runs, at its pinned commit: the reusable workflow
/// itself, or `action.yml` / `action.yaml` in the action's directory.
pub fn metadata(
    api: &dyn ForgeApi,
    forge: &Forge,
    slug: &str,
    path: &str,
    sha: &str,
) -> Result<Option<String>, Reach> {
    let dir: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(segment)
        .collect();
    let workflow =
        path.contains(".github/workflows/") && (path.ends_with(".yml") || path.ends_with(".yaml"));
    let candidates: Vec<String> = if workflow {
        vec![dir.join("/")]
    } else {
        ["action.yml", "action.yaml"]
            .iter()
            .map(|f| {
                let mut p = dir.clone();
                p.push(f.to_string());
                p.join("/")
            })
            .collect()
    };
    for file in candidates {
        match api.fetch(forge, &format!("repos/{slug}/contents/{file}?ref={sha}")) {
            Ok(v) => {
                let raw: String = v
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(raw)
                    .map_err(|e| {
                        Reach::Down(format!("`{file}` of {slug}@{sha} is not base64: {e}"))
                    })?;
                return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
            }
            Err(e) if e.kind == ForgeErrorKind::NotFound => continue,
            Err(e) => return Err(classify(&e)),
        }
    }
    Ok(None)
}

/// The mutable references in a pinned action's (or reusable workflow's) metadata.
pub fn mutable_nested(content: &str) -> Result<Vec<String>, String> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| format!("does not parse: {e}"))?;
    Ok(uses_values(&doc)
        .into_iter()
        .filter(|u| !Uses::parse(u).immutable())
        .collect())
}

fn slug_of(owner: &str, repo: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{owner}/{repo}")
    } else {
        format!("{owner}/{repo}/{path}")
    }
}

/// `imposter-commit` and `nested-action-pins` for the local workflow and action files.
pub fn findings(api: &dyn ForgeApi, forge: &Forge, files: &[(String, String)]) -> Vec<Finding> {
    if forge.kind != ForgeKind::GitHub {
        let why = match forge.kind {
            ForgeKind::GitLab => "not available on this forge (gitlab has no `uses:`)".to_string(),
            k => format!(
                "not available on this forge ({}: `uses:` resolves through the instance's configured actions source)",
                k.label()
            ),
        };
        return IDS
            .iter()
            .map(|id| Finding::new(id, Status::Info, why.clone()))
            .collect();
    }
    let pins = collect(files);
    let not_checked = if pins.unpinned == 0 {
        String::new()
    } else {
        format!(
            "; {} reference(s) not SHA-pinned were not checked",
            pins.unpinned
        )
    };
    if pins.pinned.is_empty() {
        return IDS
            .iter()
            .map(|id| {
                Finding::new(
                    id,
                    Status::Info,
                    format!("no SHA-pinned actions or reusable workflows to check{not_checked}"),
                )
            })
            .collect();
    }

    let mut cache = RepoCache::default();
    let mut reach_out = Vec::new();
    let mut reachable = 0usize;
    let mut commits: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for ((owner, repo, path, sha), used_in) in &pins.pinned {
        let e = commits
            .entry((owner.clone(), repo.clone(), sha.clone()))
            .or_default();
        e.push(slug_of(owner, repo, path));
        e.extend(used_in.iter().map(|u| format!("in {u}")));
    }
    for ((owner, repo, sha), used) in &commits {
        let slug = format!("{owner}/{repo}");
        let named = format!("`{slug}@{sha}`");
        let mut places: Vec<&str> = used.iter().filter_map(|u| u.strip_prefix("in ")).collect();
        places.sort_unstable();
        places.dedup();
        let places = places.join(", ");
        match reach(api, forge, &mut cache, &slug, sha) {
            Reach::From(_) => reachable += 1,
            Reach::Imposter { refs } => reach_out.push(
                Finding::new(
                    "imposter-commit",
                    Status::Fail,
                    format!("{named} ({places}) is not reachable from any of the {refs} branches and tags of {slug}: a commit that exists only in a fork runs under the parent repository's name"),
                )
                .fix(format!("Re-pin to a commit on a branch or release tag of {slug} (the commit a release tag points to).")),
            ),
            Reach::Unconfirmed { compared, refs } => reach_out.push(
                Finding::new(
                    "imposter-commit",
                    Status::Warn,
                    format!("{named} ({places}) is not on the default branch of {slug} nor the head of any branch or tag, and was not found in the {compared} other refs compared (of {refs}): an imposter commit from a fork, or a commit a rewritten branch no longer contains"),
                )
                .fix(format!("Confirm the commit is on a branch or tag of {slug}, or re-pin to the commit a release tag points to.")),
            ),
            Reach::Hidden(why) => reach_out.push(
                Finding::new(
                    "imposter-commit",
                    Status::Warn,
                    format!("could not check ({why}): whether {named} ({places}) is reachable from a branch or tag of {slug}"),
                )
                .fix("Re-run with a token that can read the action's repository."),
            ),
            Reach::Down(why) => reach_out.push(
                Finding::new(
                    "imposter-commit",
                    Status::Unknown,
                    format!("could not check ({why}): whether {named} ({places}) is reachable from a branch or tag of {slug}"),
                )
                .fix(access_hint(forge.kind, &why)),
            ),
        }
    }
    let total = commits.len();
    let mut out = Vec::new();
    out.push(Finding::new(
        "imposter-commit",
        if reach_out.is_empty() { Status::Pass } else { Status::Info },
        format!("{reachable} of {total} pinned commit(s) reachable from a branch or tag of their own repository{not_checked}"),
    ));
    out.extend(reach_out);

    let mut nested_out = Vec::new();
    let mut clean = 0usize;
    for ((owner, repo, path, sha), used_in) in &pins.pinned {
        let slug = format!("{owner}/{repo}");
        let named = format!("`{}@{sha}`", slug_of(owner, repo, path));
        let places = used_in.join(", ");
        match metadata(api, forge, &slug, path, sha) {
            Ok(Some(content)) => match mutable_nested(&content) {
                Ok(m) if m.is_empty() => clean += 1,
                Ok(m) => nested_out.push(
                    Finding::new(
                        "nested-action-pins",
                        Status::Warn,
                        format!(
                            "{named} ({places}) uses {} at that commit: refs that can move, so the pin does not fix the code that runs (checked one level deep)",
                            m.iter().map(|u| format!("`{u}`")).collect::<Vec<_>>().join(", ")
                        ),
                    )
                    .fix("Pin an action whose own metadata pins its actions by SHA (and images by digest), or ask its maintainers to."),
                ),
                Err(why) => nested_out.push(Finding::new(
                    "nested-action-pins",
                    Status::Warn,
                    format!("could not check {named} ({places}): its metadata {why}"),
                )),
            },
            Ok(None) => nested_out.push(Finding::new(
                "nested-action-pins",
                Status::Warn,
                format!("could not check {named} ({places}): no action.yml or action.yaml at that commit"),
            )),
            Err(Reach::Down(why)) => nested_out.push(
                Finding::new(
                    "nested-action-pins",
                    Status::Unknown,
                    format!("could not check ({why}): the metadata of {named} ({places})"),
                )
                .fix(access_hint(forge.kind, &why)),
            ),
            Err(other) => {
                let why = match other {
                    Reach::Hidden(w) => w,
                    o => format!("{o:?}"),
                };
                nested_out.push(
                    Finding::new(
                        "nested-action-pins",
                        Status::Warn,
                        format!("could not check ({why}): the metadata of {named} ({places})"),
                    )
                    .fix("Re-run with a token that can read the action's repository."),
                )
            }
        }
    }
    out.push(Finding::new(
        "nested-action-pins",
        if nested_out.is_empty() { Status::Pass } else { Status::Info },
        format!(
            "{clean} of {} pinned action(s) and reusable workflow(s) pin every action they use by SHA (and images by digest), read one level deep: the actions they call are not read in turn",
            pins.pinned.len()
        ),
    ));
    out.extend(nested_out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CannedApi;
    use serde_json::json;

    const GOOD: &str = "1111111111111111111111111111111111111111";
    const BAD: &str = "2222222222222222222222222222222222222222";

    fn gh() -> Forge {
        Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }
    }

    fn b64(s: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn put(api: &mut CannedApi, k: &str, v: serde_json::Value) {
        api.responses.insert(format!("github:{k}"), v);
    }

    /// `a/act`: default branch `main` (contains `GOOD`), `branches` more branches and
    /// `tags` tags, none containing `BAD`.
    fn api(branches: usize, tags: usize) -> CannedApi {
        let mut api = CannedApi::default();
        put(&mut api, "repos/a/act", json!({"default_branch": "main"}));
        let mut bl = vec![json!({"name": "main", "commit": {"sha": "a".repeat(40)}})];
        for i in 0..branches {
            bl.push(json!({"name": format!("b{i}"), "commit": {"sha": "b".repeat(40)}}));
        }
        let tl: Vec<_> = (0..tags)
            .map(|i| json!({"name": format!("v{i}"), "commit": {"sha": "c".repeat(40)}}))
            .collect();
        put(
            &mut api,
            "repos/a/act/branches?per_page=100&page=1",
            json!(bl),
        );
        put(&mut api, "repos/a/act/tags?per_page=100&page=1", json!(tl));
        put(
            &mut api,
            &format!("repos/a/act/compare/main...{GOOD}"),
            json!({"status": "behind"}),
        );
        put(
            &mut api,
            &format!("repos/a/act/compare/main...{BAD}"),
            json!({"status": "diverged"}),
        );
        for i in 0..branches {
            put(
                &mut api,
                &format!("repos/a/act/compare/b{i}...{BAD}"),
                json!({"status": "diverged"}),
            );
        }
        for i in 0..tags {
            put(
                &mut api,
                &format!("repos/a/act/compare/refs%2Ftags%2Fv{i}...{BAD}"),
                json!({"status": "ahead"}),
            );
        }
        api
    }

    fn action_yml(api: &mut CannedApi, sha: &str, content: &str) {
        put(
            api,
            &format!("repos/a/act/contents/action.yml?ref={sha}"),
            json!({"encoding": "base64", "content": b64(content)}),
        );
    }

    fn wf(uses: &[&str]) -> Vec<(String, String)> {
        let steps: String = uses
            .iter()
            .map(|u| format!("      - uses: {u}\n"))
            .collect();
        vec![(
            ".github/workflows/ci.yml".into(),
            format!("on: push\njobs:\n  j:\n    runs-on: x\n    steps:\n{steps}"),
        )]
    }

    #[test]
    fn uses_references_are_classified() {
        assert_eq!(Uses::parse("./local"), Uses::Local);
        assert!(Uses::parse("./local").immutable());
        assert!(!Uses::parse("docker://alpine:3").immutable());
        assert!(Uses::parse("docker://alpine@sha256:abc").immutable());
        assert!(Uses::parse(&format!("a/b/sub/dir@{GOOD}")).immutable());
        assert!(!Uses::parse("a/b@v4").immutable());
        assert!(!Uses::parse("a/b@main").immutable());
        // A short SHA is ambiguous: a new commit can share its prefix.
        assert!(!Uses::parse("a/b@abc1234").immutable());
        assert!(!is_sha(&GOOD[..39]));
        assert!(!Uses::parse("a/b@${{ inputs.ref }}").immutable());
        assert_eq!(
            Uses::parse(&format!("a/b/sub/dir@{GOOD}")),
            Uses::Remote {
                owner: "a".into(),
                repo: "b".into(),
                path: "sub/dir".into(),
                git_ref: GOOD.into()
            }
        );
    }

    #[test]
    fn collects_step_job_and_composite_pins() {
        let mut files = wf(&[
            &format!("a/act@{GOOD}"),
            &format!("a/act@{GOOD}"),
            "a/act@v1",
            "./local",
            "docker://alpine:3",
        ]);
        files.push((
            ".github/workflows/call.yml".into(),
            format!("on: push\njobs:\n  call:\n    uses: a/act/.github/workflows/r.yml@{BAD}\n"),
        ));
        files.push((
            "action.yml".into(),
            format!("runs:\n  using: composite\n  steps:\n    - uses: c/d@{GOOD}\n"),
        ));
        let p = collect(&files);
        assert_eq!(p.pinned.len(), 3, "{p:?}");
        assert_eq!(p.unpinned, 2);
        assert!(p.pinned.contains_key(&(
            "a".into(),
            "act".into(),
            ".github/workflows/r.yml".into(),
            BAD.into()
        )));
    }

    #[test]
    fn a_commit_on_the_default_branch_or_a_tag_head_is_reachable() {
        let mut api = api(1, 1);
        let c = "c".repeat(40);
        put(
            &mut api,
            &format!("repos/a/act/compare/main...{c}"),
            json!({"status": "diverged"}),
        );
        let mut cache = RepoCache::default();
        assert_eq!(
            reach(&api, &gh(), &mut cache, "a/act", GOOD),
            Reach::From("branch main".into())
        );
        // Not on the default branch, but the head of a tag.
        assert_eq!(
            reach(&api, &gh(), &mut cache, "a/act", &c),
            Reach::From("tag v0".into())
        );
        // The branch and tag lists are read once per repository.
        let reads = api
            .log()
            .iter()
            .filter(|k| k.ends_with("branches?per_page=100&page=1"))
            .count();
        assert_eq!(reads, 1);
    }

    #[test]
    fn a_commit_on_no_branch_or_tag_is_an_imposter() {
        let mut few = api(2, 2);
        // GitHub answers 404 when the two commits share no ancestor: not from this ref.
        put(
            &mut few,
            &format!("repos/a/act/compare/main...{BAD}"),
            serde_json::Value::Null,
        );
        assert_eq!(
            reach(&few, &gh(), &mut RepoCache::default(), "a/act", BAD),
            Reach::Imposter { refs: 5 }
        );
        // Found in another branch: reachable.
        let mut found = few.clone();
        put(
            &mut found,
            &format!("repos/a/act/compare/b1...{BAD}"),
            json!({"status": "identical"}),
        );
        assert_eq!(
            reach(&found, &gh(), &mut RepoCache::default(), "a/act", BAD),
            Reach::From("branch b1".into())
        );
        // More refs than the budget: not confirmed either way.
        let many = api(COMPARE_BUDGET, 3);
        assert_eq!(
            reach(&many, &gh(), &mut RepoCache::default(), "a/act", BAD),
            Reach::Unconfirmed {
                compared: COMPARE_BUDGET,
                refs: COMPARE_BUDGET + 4
            }
        );
    }

    #[test]
    fn findings_fail_an_imposter_and_warn_on_mutable_nested_refs() {
        let mut api = api(1, 1);
        action_yml(
            &mut api,
            GOOD,
            &format!("runs:\n  using: composite\n  steps:\n    - uses: x/y@v2\n    - uses: x/z@{GOOD}\n    - uses: ./inner\n"),
        );
        action_yml(
            &mut api,
            BAD,
            "runs:\n  using: docker\n  image: docker://alpine:latest\n",
        );
        let f = findings(
            &api,
            &gh(),
            &wf(&[&format!("a/act@{GOOD}"), &format!("a/act@{BAD}"), "u/v@v1"]),
        );
        let of = |id: &str, s: Status| {
            f.iter()
                .filter(|x| x.id == id && x.status == s)
                .map(|x| x.summary.clone())
                .collect::<Vec<_>>()
        };
        let fail = of("imposter-commit", Status::Fail);
        assert_eq!(fail.len(), 1, "{f:?}");
        assert!(fail[0].contains(BAD), "{}", fail[0]);
        let summary = of("imposter-commit", Status::Info);
        assert!(
            summary[0].starts_with("1 of 2 pinned commit(s) reachable")
                && summary[0].contains("1 reference(s) not SHA-pinned were not checked"),
            "{summary:?}"
        );
        let warn = of("nested-action-pins", Status::Warn);
        assert_eq!(warn.len(), 2, "{f:?}");
        assert!(
            warn.iter()
                .any(|w| w.contains("`x/y@v2`") && !w.contains("x/z")),
            "{warn:?}"
        );
        assert!(
            warn.iter().any(|w| w.contains("`docker://alpine:latest`")),
            "{warn:?}"
        );
        assert!(warn[0].contains("one level deep"), "{warn:?}");
    }

    #[test]
    fn clean_pins_pass_and_action_yaml_is_the_fallback() {
        let mut api = api(0, 0);
        put(
            &mut api,
            &format!("repos/a/act/contents/action.yml?ref={GOOD}"),
            serde_json::Value::Null,
        );
        put(
            &mut api,
            &format!("repos/a/act/contents/action.yaml?ref={GOOD}"),
            json!({"content": b64(&format!("runs:\n  using: composite\n  steps:\n    - uses: x/z@{GOOD}\n"))}),
        );
        let f = findings(&api, &gh(), &wf(&[&format!("a/act@{GOOD}")]));
        assert!(f.iter().all(|x| x.status == Status::Pass), "{f:?}");
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn a_reusable_workflow_pin_reads_the_workflow_file() {
        let mut api = api(0, 0);
        put(
            &mut api,
            &format!("repos/a/act/contents/%2Egithub/workflows/r.yml?ref={GOOD}"),
            json!({"content": b64("on: workflow_call\njobs:\n  j:\n    runs-on: x\n    steps:\n      - uses: x/y@main\n")}),
        );
        let files = vec![(
            ".github/workflows/call.yml".to_string(),
            format!("on: push\njobs:\n  call:\n    uses: a/act/.github/workflows/r.yml@{GOOD}\n"),
        )];
        let f = findings(&api, &gh(), &files);
        assert!(
            f.iter().any(|x| x.id == "nested-action-pins"
                && x.status == Status::Warn
                && x.summary.contains("`x/y@main`")),
            "{f:?}"
        );
    }

    #[test]
    fn unreachable_forge_is_unknown_and_other_forges_are_information() {
        let down = json!({"__error": "network access is disabled (DISCIPLINE_NO_NETWORK)"});
        let mut api = CannedApi::default();
        put(&mut api, "repos/a/act", down.clone());
        put(
            &mut api,
            &format!("repos/a/act/contents/action.yml?ref={GOOD}"),
            down,
        );
        let f = findings(&api, &gh(), &wf(&[&format!("a/act@{GOOD}")]));
        assert_eq!(
            f.iter().filter(|x| x.status == Status::Unknown).count(),
            2,
            "{f:?}"
        );
        // A private or missing action repository is not visible: a warning, never a pass.
        let mut hidden = CannedApi::default();
        put(&mut hidden, "repos/a/act", serde_json::Value::Null);
        put(
            &mut hidden,
            &format!("repos/a/act/contents/action.yml?ref={GOOD}"),
            json!({"__status": 403, "__body": {}}),
        );
        let f = findings(&hidden, &gh(), &wf(&[&format!("a/act@{GOOD}")]));
        let problems: Vec<&Finding> = f
            .iter()
            .filter(|x| x.summary.starts_with("could not check"))
            .collect();
        assert_eq!(problems.len(), 2, "{f:?}");
        assert!(problems.iter().all(|x| x.status == Status::Warn), "{f:?}");

        for kind in [ForgeKind::Gitea, ForgeKind::Forgejo, ForgeKind::GitLab] {
            let forge = Forge { kind, ..gh() };
            let none = CannedApi::default();
            let f = findings(&none, &forge, &wf(&[&format!("a/act@{GOOD}")]));
            assert_eq!(f.len(), 2);
            assert!(f.iter().all(|x| x.status == Status::Info
                && x.summary.starts_with("not available on this forge")));
            assert!(none.log().is_empty());
        }
        // Nothing SHA-pinned: nothing to ask.
        let none = CannedApi::default();
        let f = findings(&none, &gh(), &wf(&["a/act@v1"]));
        assert!(f.iter().all(|x| x.status == Status::Info), "{f:?}");
        assert!(none.log().is_empty());
    }
}
