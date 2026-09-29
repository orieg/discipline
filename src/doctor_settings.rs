//! `discipline doctor`: the repository settings that decide whether a mutable action ref,
//! a moved release tag or a replaced release asset can run.
//!
//! - **Actions policy** (GitHub): `sha_pinning_required`, `allowed_actions`,
//!   `default_workflow_permissions`, `can_approve_pull_request_reviews`.
//! - **Immutable releases** (GitHub).
//! - **Tag protection** of release tags: a tag ruleset that blocks update and deletion
//!   (GitHub), protected tags (Gitea, Forgejo, GitLab).
//! - **Secret scoping** (GitHub): repository-level Actions secrets that the workflows read
//!   only from jobs bound to an environment.
//!
//! Reads go through `crate::forge` (AGENTS.md §3.3). A setting the token cannot see is a
//! warning naming the access it needs, never a pass; a forge that cannot be reached is
//! `unknown` (exit 2). A setting the forge does not have is information. Secrets are
//! reported by name only: the API never returns a value.

use crate::doctor::{access_hint, Finding, Status};
use crate::forge::{gitlab_project_id, read_all, Forge, ForgeApi, ForgeErrorKind, ForgeKind};
use std::collections::{BTreeMap, BTreeSet};

/// Finding ids this module reports, in report order.
pub const IDS: &[&str] = &[
    "actions-sha-pinning",
    "allowed-actions",
    "default-token",
    "actions-approve-prs",
    "immutable-releases",
    "tag-protection",
    "secret-scoping",
    "forge-token",
];

/// The access GitHub needs for the admin-only settings endpoints.
const GITHUB_ADMIN: &str =
    "a token with admin access to the repository (fine-grained: `Administration` read)";

/// One read: the answer, a refusal (the token cannot see it), or no answer.
enum Answer {
    Ok(serde_json::Value),
    /// 403 / 404: not visible to this token (or, for some endpoints, not set).
    Hidden(String),
    /// Unreachable, rate limited, malformed: the check could not run.
    Down(String),
}

fn ask(api: &dyn ForgeApi, forge: &Forge, path: &str) -> Answer {
    match api.fetch(forge, path) {
        Ok(v) => Answer::Ok(v),
        Err(e) if matches!(e.kind, ForgeErrorKind::NotFound | ForgeErrorKind::Denied) => {
            Answer::Hidden(e.to_string())
        }
        Err(e) => Answer::Down(e.to_string()),
    }
}

fn hidden(id: &'static str, what: &str, why: &str, need: &str) -> Finding {
    Finding::new(
        id,
        Status::Warn,
        format!("could not check ({why}): {what} is not visible to this token"),
    )
    .fix(format!("Re-run with {need}."))
}

fn down(id: &'static str, kind: ForgeKind, why: &str) -> Finding {
    Finding::new(id, Status::Unknown, format!("could not check ({why})"))
        .fix(access_hint(kind, why))
}

fn unavailable(id: &'static str, kind: ForgeKind) -> Finding {
    Finding::new(
        id,
        Status::Info,
        format!("not available on this forge ({})", kind.label()),
    )
}

/// Every settings finding for `forge`. `workflows` are the local Actions workflow files
/// (path, content), read for which jobs use which secrets.
pub fn findings(api: &dyn ForgeApi, forge: &Forge, workflows: &[(String, String)]) -> Vec<Finding> {
    match forge.kind {
        ForgeKind::GitHub => github(api, forge, workflows),
        ForgeKind::GitLab => gitlab(api, forge),
        ForgeKind::Gitea | ForgeKind::Forgejo => IDS
            .iter()
            .map(|id| match *id {
                "tag-protection" => protected_tags(api, forge),
                "secret-scoping" => gitea_secret_scoping(api, forge, workflows),
                other => unavailable(other, forge.kind),
            })
            .collect(),
    }
}

fn github(api: &dyn ForgeApi, forge: &Forge, workflows: &[(String, String)]) -> Vec<Finding> {
    let r = &forge.repo;
    // The repository read says whether the forge answers at all, and whether this token
    // is an admin (a 404 from `immutable-releases` then means "off", not "hidden").
    let admin = match api.fetch(forge, &format!("repos/{r}")) {
        Ok(v) => v.pointer("/permissions/admin").and_then(|a| a.as_bool()),
        Err(e) => {
            let why = e.to_string();
            return IDS.iter().map(|id| down(id, forge.kind, &why)).collect();
        }
    };
    let mut out = Vec::new();

    match ask(api, forge, &format!("repos/{r}/actions/permissions")) {
        Answer::Ok(v) => out.extend(actions_policy(&v)),
        Answer::Hidden(why) => {
            for id in ["actions-sha-pinning", "allowed-actions"] {
                out.push(hidden(id, "the Actions policy", &why, GITHUB_ADMIN));
            }
        }
        Answer::Down(why) => {
            for id in ["actions-sha-pinning", "allowed-actions"] {
                out.push(down(id, forge.kind, &why));
            }
        }
    }
    match ask(
        api,
        forge,
        &format!("repos/{r}/actions/permissions/workflow"),
    ) {
        Answer::Ok(v) => out.extend(workflow_token(&v)),
        Answer::Hidden(why) => {
            for id in ["default-token", "actions-approve-prs"] {
                out.push(hidden(
                    id,
                    "the default workflow token setting",
                    &why,
                    GITHUB_ADMIN,
                ));
            }
        }
        Answer::Down(why) => {
            for id in ["default-token", "actions-approve-prs"] {
                out.push(down(id, forge.kind, &why));
            }
        }
    }
    out.push(
        match ask(api, forge, &format!("repos/{r}/immutable-releases")) {
            Answer::Ok(v) => immutable_releases(v.get("enabled").and_then(|e| e.as_bool())),
            // GitHub answers 404 both when the setting is off and when the token may not
            // read it; an admin token tells the two apart.
            Answer::Hidden(_) if admin == Some(true) => immutable_releases(Some(false)),
            Answer::Hidden(why) => hidden(
                "immutable-releases",
                "the immutable-releases setting",
                &why,
                GITHUB_ADMIN,
            ),
            Answer::Down(why) => down("immutable-releases", forge.kind, &why),
        },
    );
    out.push(github_tag_rulesets(api, forge));
    out.push(secret_scoping(api, forge, workflows));
    // A GitHub token's scopes are only in a response header (`X-OAuth-Scopes`, classic
    // tokens); a fine-grained token does not list its permissions.
    out.push(unavailable("forge-token", forge.kind));
    out
}

/// `actions-sha-pinning` and `allowed-actions` from `actions/permissions`.
pub fn actions_policy(v: &serde_json::Value) -> Vec<Finding> {
    if v.get("enabled").and_then(|e| e.as_bool()) == Some(false) {
        return ["actions-sha-pinning", "allowed-actions"]
            .into_iter()
            .map(|id| {
                Finding::new(
                    id,
                    Status::Pass,
                    "GitHub Actions is disabled for this repository",
                )
            })
            .collect();
    }
    let pinning = match v.get("sha_pinning_required").and_then(|p| p.as_bool()) {
        Some(true) => Finding::new(
            "actions-sha-pinning",
            Status::Pass,
            "actions must be pinned to a full-length commit SHA (`sha_pinning_required`)",
        ),
        Some(false) => Finding::new(
            "actions-sha-pinning",
            Status::Warn,
            "actions may run from a tag or branch ref, which can be moved to other code (`sha_pinning_required` is false)",
        )
        .fix("Settings → Actions → General → require actions to be pinned to a full-length commit SHA."),
        None => Finding::new(
            "actions-sha-pinning",
            Status::Warn,
            "could not check (the forge does not report `sha_pinning_required`)",
        )
        .fix("Require full-length commit SHA pins in the Actions settings where the forge offers it."),
    };
    let allowed = match v.get("allowed_actions").and_then(|a| a.as_str()) {
        Some("all") => Finding::new(
            "allowed-actions",
            Status::Warn,
            "any action from any repository may run (`allowed_actions: all`)",
        )
        .fix("Settings → Actions → General → allow selected actions (or local actions only)."),
        Some(other @ ("selected" | "local_only")) => Finding::new(
            "allowed-actions",
            Status::Pass,
            format!("only allowed actions may run (`allowed_actions: {other}`)"),
        ),
        other => Finding::new(
            "allowed-actions",
            Status::Warn,
            format!(
                "could not check (unrecognised `allowed_actions`: {})",
                other.unwrap_or("missing")
            ),
        ),
    };
    vec![pinning, allowed]
}

/// `default-token` and `actions-approve-prs` from `actions/permissions/workflow`.
pub fn workflow_token(v: &serde_json::Value) -> Vec<Finding> {
    let token = match v.get("default_workflow_permissions").and_then(|p| p.as_str()) {
        Some("read") => Finding::new(
            "default-token",
            Status::Pass,
            "the default workflow token is read-only (`default_workflow_permissions: read`)",
        ),
        Some("write") => Finding::new(
            "default-token",
            Status::Warn,
            "a workflow without `permissions:` gets a token that can write (`default_workflow_permissions: write`)",
        )
        .fix("Settings → Actions → General → Workflow permissions → read repository contents and packages."),
        other => Finding::new(
            "default-token",
            Status::Warn,
            format!(
                "could not check (unrecognised `default_workflow_permissions`: {})",
                other.unwrap_or("missing")
            ),
        ),
    };
    let approve = match v.get("can_approve_pull_request_reviews").and_then(|p| p.as_bool()) {
        Some(false) => Finding::new(
            "actions-approve-prs",
            Status::Pass,
            "workflows cannot approve pull requests",
        ),
        Some(true) => Finding::new(
            "actions-approve-prs",
            Status::Warn,
            "a workflow's token can approve pull requests (`can_approve_pull_request_reviews`), so an approval need not come from a person",
        )
        .fix("Settings → Actions → General → clear \"Allow GitHub Actions to create and approve pull requests\"."),
        None => Finding::new(
            "actions-approve-prs",
            Status::Warn,
            "could not check (the forge does not report `can_approve_pull_request_reviews`)",
        ),
    };
    vec![token, approve]
}

/// `immutable-releases` from whether the setting is on (`None`: not reported).
pub fn immutable_releases(enabled: Option<bool>) -> Finding {
    match enabled {
        Some(true) => Finding::new(
            "immutable-releases",
            Status::Pass,
            "releases are immutable: a published release's assets and tag cannot be changed",
        ),
        Some(false) => Finding::new(
            "immutable-releases",
            Status::Warn,
            "releases are not immutable: a published release's assets can be replaced and its tag moved",
        )
        .fix("Settings → General → Releases → enable release immutability."),
        None => Finding::new(
            "immutable-releases",
            Status::Warn,
            "could not check (the forge does not report whether releases are immutable)",
        ),
    }
}

/// Whether a ref pattern list matches `name`. GitHub ruleset patterns are fnmatch-style
/// with `*` not crossing `/` (`~ALL` matches everything); Gitea, Forgejo and GitLab tag
/// patterns let `*` match any character, and Gitea / Forgejo read `/.../` as a regex.
fn pattern_matches(pattern: &str, name: &str, slash_literal: bool) -> bool {
    if pattern == "~ALL" {
        return true;
    }
    if !slash_literal && pattern.len() > 1 && pattern.starts_with('/') && pattern.ends_with('/') {
        return regex::Regex::new(&pattern[1..pattern.len() - 1]).is_ok_and(|re| re.is_match(name));
    }
    globset::GlobBuilder::new(pattern)
        .literal_separator(slash_literal)
        .build()
        .map(|g| g.compile_matcher().is_match(name))
        .unwrap_or(false)
}

/// The tag a release is published under: the latest release's, else the first tag listed.
fn sample_tag(api: &dyn ForgeApi, forge: &Forge) -> Result<Option<String>, String> {
    let (release, tags) = match forge.kind {
        ForgeKind::GitLab => {
            let id = gitlab_project_id(&forge.repo);
            (
                format!("projects/{id}/releases?per_page=1"),
                format!("projects/{id}/repository/tags?per_page=1"),
            )
        }
        ForgeKind::GitHub => (
            format!("repos/{}/releases/latest", forge.repo),
            format!("repos/{}/tags?per_page=1", forge.repo),
        ),
        ForgeKind::Gitea | ForgeKind::Forgejo => (
            format!("repos/{}/releases/latest", forge.repo),
            format!("repos/{}/tags?limit=1", forge.repo),
        ),
    };
    let first = |v: &serde_json::Value, key: &str| {
        let item = if v.is_array() { v.get(0) } else { Some(v) };
        item.and_then(|i| i.get(key))
            .and_then(|t| t.as_str())
            .map(str::to_string)
    };
    for (path, key) in [(release, "tag_name"), (tags, "name")] {
        match ask(api, forge, &path) {
            Answer::Ok(v) => {
                if let Some(t) = first(&v, key) {
                    return Ok(Some(t));
                }
            }
            Answer::Hidden(_) => {}
            Answer::Down(why) => return Err(why),
        }
    }
    Ok(None)
}

/// One GitHub tag ruleset, as far as the check needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRuleset {
    pub name: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub blocks_update: bool,
    pub blocks_deletion: bool,
}

impl TagRuleset {
    /// From the body of `rulesets/{id}`; `None` unless it targets tags and is active.
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        if v.get("target")
            .and_then(|t| t.as_str())
            .is_some_and(|t| t != "tag")
            || v.get("enforcement").and_then(|e| e.as_str()) != Some("active")
        {
            return None;
        }
        let list = |key: &str| -> Vec<String> {
            v.pointer(&format!("/conditions/ref_name/{key}"))
                .and_then(|l| l.as_array())
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        };
        let rules: BTreeSet<&str> = v
            .get("rules")
            .and_then(|r| r.as_array())
            .into_iter()
            .flatten()
            .filter_map(|r| r.get("type").and_then(|t| t.as_str()))
            .collect();
        Some(Self {
            name: v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unnamed")
                .to_string(),
            include: list("include"),
            exclude: list("exclude"),
            blocks_update: rules.contains("update"),
            blocks_deletion: rules.contains("deletion"),
        })
    }

    /// Whether the ruleset applies to `tag` (`None`: to some tag, any at all).
    pub fn covers(&self, tag: Option<&str>) -> bool {
        match tag {
            None => !self.include.is_empty(),
            Some(t) => {
                let r = format!("refs/tags/{t}");
                let hit = |ps: &[String]| {
                    ps.iter()
                        .any(|p| pattern_matches(p, &r, true) || pattern_matches(p, t, true))
                };
                hit(&self.include) && !hit(&self.exclude)
            }
        }
    }
}

/// `tag-protection` on GitHub from the active tag rulesets and the release tag.
pub fn tag_ruleset_finding(rulesets: &[TagRuleset], tag: Option<&str>) -> Finding {
    let applying: Vec<&TagRuleset> = rulesets.iter().filter(|r| r.covers(tag)).collect();
    let what = match tag {
        Some(t) => format!("the release tag `{t}`"),
        None => "tags (no release or tag yet to match)".to_string(),
    };
    let blocking: Vec<String> = applying
        .iter()
        .filter(|r| r.blocks_update && r.blocks_deletion)
        .map(|r| format!("`{}`", r.name))
        .collect();
    if !blocking.is_empty() {
        return Finding::new(
            "tag-protection",
            Status::Pass,
            format!(
                "tag ruleset(s) {} block update and deletion of {what}",
                blocking.join(", ")
            ),
        );
    }
    let fix = "Add an active tag ruleset whose target covers the release tags (for example `refs/tags/v*.*.*`) with the \"Restrict updates\" and \"Restrict deletions\" rules.";
    if applying.is_empty() {
        Finding::new(
            "tag-protection",
            Status::Warn,
            format!("no active tag ruleset covers {what}: a release tag can be moved or deleted"),
        )
        .fix(fix)
    } else {
        let names: Vec<String> = applying.iter().map(|r| format!("`{}`", r.name)).collect();
        Finding::new(
            "tag-protection",
            Status::Warn,
            format!(
                "tag ruleset(s) {} cover {what} but do not block both update and deletion",
                names.join(", ")
            ),
        )
        .fix(fix)
    }
}

fn github_tag_rulesets(api: &dyn ForgeApi, forge: &Forge) -> Finding {
    let r = &forge.repo;
    let need = "a token that can read the repository's rulesets";
    let list = match read_all(
        api,
        forge,
        &format!("repos/{r}/rulesets?targets=tag&includes_parents=true"),
    ) {
        Ok(l) => l,
        Err(e) if matches!(e.kind, ForgeErrorKind::NotFound | ForgeErrorKind::Denied) => {
            return hidden("tag-protection", "the tag rulesets", &e.to_string(), need)
        }
        Err(e) => return down("tag-protection", forge.kind, &e.to_string()),
    };
    let mut rulesets = Vec::new();
    for item in &list {
        if item.get("target").and_then(|t| t.as_str()) != Some("tag")
            || item.get("enforcement").and_then(|e| e.as_str()) != Some("active")
        {
            continue;
        }
        let Some(id) = item.get("id").and_then(|i| i.as_i64()) else {
            continue;
        };
        match ask(api, forge, &format!("repos/{r}/rulesets/{id}")) {
            Answer::Ok(v) => rulesets.extend(TagRuleset::from_json(&v)),
            Answer::Hidden(why) => {
                return hidden("tag-protection", "a tag ruleset's rules", &why, need)
            }
            Answer::Down(why) => return down("tag-protection", forge.kind, &why),
        }
    }
    match sample_tag(api, forge) {
        Ok(tag) => tag_ruleset_finding(&rulesets, tag.as_deref()),
        Err(why) => down("tag-protection", forge.kind, &why),
    }
}

/// `tag-protection` on Gitea, Forgejo and GitLab from the protected-tag patterns.
pub fn protected_tag_finding(kind: ForgeKind, patterns: &[String], tag: Option<&str>) -> Finding {
    let fix = match kind {
        ForgeKind::GitLab => "Settings → Repository → Protected tags → protect a pattern covering the release tags (for example `v*`).",
        _ => "Settings → Tags → add a protected tag pattern covering the release tags (for example `v*`), with an empty or minimal allowlist.",
    };
    let covering: Vec<String> = patterns
        .iter()
        .filter(|p| tag.is_none_or(|t| pattern_matches(p, t, false)))
        .map(|p| format!("`{p}`"))
        .collect();
    match (covering.is_empty(), tag) {
        (false, Some(t)) => Finding::new(
            "tag-protection",
            Status::Pass,
            format!(
                "protected tag pattern(s) {} cover the release tag `{t}`",
                covering.join(", ")
            ),
        ),
        (false, None) => Finding::new(
            "tag-protection",
            Status::Pass,
            format!(
                "protected tag pattern(s) {} (no release or tag yet to match)",
                covering.join(", ")
            ),
        ),
        (true, Some(t)) => Finding::new(
            "tag-protection",
            Status::Warn,
            format!(
                "no protected tag pattern covers the release tag `{t}`: it can be moved or deleted"
            ),
        )
        .fix(fix),
        (true, None) => Finding::new(
            "tag-protection",
            Status::Warn,
            "no protected tags: a release tag can be moved or deleted",
        )
        .fix(fix),
    }
}

fn protected_tags(api: &dyn ForgeApi, forge: &Forge) -> Finding {
    let (path, key, need) = match forge.kind {
        ForgeKind::GitLab => (
            format!("projects/{}/protected_tags", gitlab_project_id(&forge.repo)),
            "name",
            "a token with the Maintainer role",
        ),
        _ => (
            format!("repos/{}/tag_protections", forge.repo),
            "name_pattern",
            "a token with admin access to the repository",
        ),
    };
    let list = match read_all(api, forge, &path) {
        Ok(l) => l,
        Err(e) if matches!(e.kind, ForgeErrorKind::NotFound | ForgeErrorKind::Denied) => {
            return hidden("tag-protection", "the protected tags", &e.to_string(), need)
        }
        Err(e) => return down("tag-protection", forge.kind, &e.to_string()),
    };
    let patterns: Vec<String> = list
        .iter()
        .filter_map(|p| p.get(key).and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    match sample_tag(api, forge) {
        Ok(tag) => protected_tag_finding(forge.kind, &patterns, tag.as_deref()),
        Err(why) => down("tag-protection", forge.kind, &why),
    }
}

/// Which jobs read a secret, and whether each is bound to an environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretUses {
    /// Secret name (upper-cased: GitHub secret names are case-insensitive) to the jobs
    /// reading it: `(workflow job, environment)`.
    pub reads: BTreeMap<String, Vec<(String, Option<String>)>>,
    /// Jobs outside an environment that pass every secret on (`secrets: inherit`).
    pub inherit: Vec<String>,
}

static SECRET_REF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r#"secrets\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]\s*\])"#,
    )
    .expect("static regex")
});

fn secret_names(text: &str) -> BTreeSet<String> {
    SECRET_REF
        .captures_iter(text)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_ascii_uppercase())
        .filter(|n| n != "INHERIT")
        .collect()
}

/// The secrets each workflow job reads. A secret named in the workflow's top-level `env:`
/// counts as read by every job of that workflow.
pub fn secret_uses(workflows: &[(String, String)]) -> SecretUses {
    let mut out = SecretUses::default();
    for (path, content) in workflows {
        let Ok(wf) = serde_yaml::from_str::<serde_yaml::Value>(content) else {
            continue;
        };
        let Some(jobs) = wf.get("jobs").and_then(|j| j.as_mapping()) else {
            continue;
        };
        let shared = wf
            .get("env")
            .and_then(|e| serde_yaml::to_string(e).ok())
            .map(|t| secret_names(&t))
            .unwrap_or_default();
        for (id, job) in jobs {
            let Some(id) = id.as_str() else { continue };
            let label = format!("{path} job `{id}`");
            let env = match job.get("environment") {
                Some(serde_yaml::Value::String(s)) => Some(s.clone()),
                Some(m @ serde_yaml::Value::Mapping(_)) => Some(
                    m.get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("?")
                        .to_string(),
                ),
                _ => None,
            };
            if env.is_none() && job.get("secrets").and_then(|s| s.as_str()) == Some("inherit") {
                out.inherit.push(label.clone());
            }
            let mut names = serde_yaml::to_string(job)
                .map(|t| secret_names(&t))
                .unwrap_or_default();
            names.extend(shared.iter().cloned());
            for n in names {
                out.reads
                    .entry(n)
                    .or_default()
                    .push((label.clone(), env.clone()));
            }
        }
    }
    out
}

/// `secret-scoping`: repository-level secrets read only from environment-bound jobs.
pub fn secret_scoping_finding(repo_secrets: &[String], uses: &SecretUses) -> Finding {
    if repo_secrets.is_empty() {
        return Finding::new(
            "secret-scoping",
            Status::Pass,
            "no repository-level Actions secrets",
        );
    }
    let mut env_only = Vec::new();
    let mut unread = Vec::new();
    for name in repo_secrets {
        match uses.reads.get(&name.to_ascii_uppercase()) {
            None if uses.inherit.is_empty() => unread.push(name.as_str()),
            None => {}
            Some(jobs) if uses.inherit.is_empty() && jobs.iter().all(|(_, e)| e.is_some()) => {
                let mut envs: Vec<&str> = jobs.iter().filter_map(|(_, e)| e.as_deref()).collect();
                envs.sort_unstable();
                envs.dedup();
                env_only.push(format!("`{name}` (environment {})", envs.join(", ")));
            }
            Some(_) => {}
        }
    }
    let unread_note = if unread.is_empty() {
        String::new()
    } else {
        format!(
            "; not read by any workflow here: {}",
            unread
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if env_only.is_empty() {
        Finding::new(
            "secret-scoping",
            Status::Pass,
            format!(
                "{} repository-level secret(s); none is read only from environment-bound jobs{unread_note}",
                repo_secrets.len()
            ),
        )
    } else {
        Finding::new(
            "secret-scoping",
            Status::Warn,
            format!(
                "repository-level secret(s) read only by jobs bound to an environment: {}. A workflow on any branch can read a repository secret; an environment secret reaches only jobs that pass the environment's protection rules{unread_note}",
                env_only.join(", ")
            ),
        )
        .fix("Move each secret to the environment its jobs use (Settings → Environments → <name> → secrets) and delete the repository-level copy.")
    }
}

fn secret_scoping(api: &dyn ForgeApi, forge: &Forge, workflows: &[(String, String)]) -> Finding {
    let need = "a token with admin access to the repository (fine-grained: `Secrets` read)";
    let mut names = Vec::new();
    // The list is an object (`total_count`, `secrets`), so it is paged here.
    for page in 1..=crate::forge::MAX_PAGES {
        let path = format!(
            "repos/{}/actions/secrets?per_page=100&page={page}",
            forge.repo
        );
        let v = match ask(api, forge, &path) {
            Answer::Ok(v) => v,
            Answer::Hidden(why) => {
                return hidden("secret-scoping", "the repository's secret list", &why, need)
            }
            Answer::Down(why) => return down("secret-scoping", forge.kind, &why),
        };
        let total = v.get("total_count").and_then(|t| t.as_u64());
        let batch: Vec<String> = v
            .get("secrets")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect();
        let empty = batch.is_empty();
        names.extend(batch);
        match total {
            Some(t) if names.len() as u64 >= t => {
                return secret_scoping_finding(&names, &secret_uses(workflows))
            }
            _ if empty => break,
            None => return secret_scoping_finding(&names, &secret_uses(workflows)),
            Some(_) => {}
        }
    }
    down(
        "secret-scoping",
        forge.kind,
        "the secret list ended before the count the forge states; refusing to judge a partial list",
    )
}

/// The read permission a GitLab fine-grained token lacks, named in the refusal's reason
/// ("... requires a fine-grained personal access token with the following project
/// permissions: [Variable: Read]."), recorded live in
/// `tests/fixtures/forge_settings/gitlab.json`.
pub fn gitlab_missing_permission(why: &str) -> Option<&str> {
    let start = why.find("permissions: [")? + "permissions: [".len();
    let end = why[start..].find(']')? + start;
    Some(why[start..end].trim()).filter(|p| !p.is_empty())
}

/// What a GitLab token needs to read a setting: the permission the refusal names, else
/// `fallback`.
fn gitlab_need(why: &str, fallback: &str) -> String {
    match gitlab_missing_permission(why) {
        Some(p) => format!(
            "a token that holds `{p}` (a fine-grained token), or `read_api` with {fallback}"
        ),
        None => format!("a token with `read_api` and {fallback}"),
    }
}

/// GitLab's settings: the job token (`default-token`), CI/CD variables
/// (`secret-scoping`), protected tags, and the token `doctor` runs with (`forge-token`).
/// GitLab has no `uses:` actions, no workflow-approval setting and no release
/// immutability: those stay information.
fn gitlab(api: &dyn ForgeApi, forge: &Forge) -> Vec<Finding> {
    let id = gitlab_project_id(&forge.repo);
    let project = match api.fetch(forge, &format!("projects/{id}")) {
        Ok(v) => v,
        Err(e) => {
            let why = e.to_string();
            return IDS.iter().map(|i| down(i, forge.kind, &why)).collect();
        }
    };
    let mut out: Vec<Finding> = ["actions-sha-pinning", "allowed-actions"]
        .iter()
        .map(|i| unavailable(i, forge.kind))
        .collect();
    out.push(
        match ask(api, forge, &format!("projects/{id}/job_token_scope")) {
            Answer::Ok(v) => gitlab_job_token_finding(&v, &project),
            Answer::Hidden(why) => hidden(
                "default-token",
                "the CI job token scope",
                &why,
                &gitlab_need(&why, "the Maintainer role"),
            ),
            Answer::Down(why) => down("default-token", forge.kind, &why),
        },
    );
    for i in ["actions-approve-prs", "immutable-releases"] {
        out.push(unavailable(i, forge.kind));
    }
    out.push(protected_tags(api, forge));
    out.push(
        match read_all(api, forge, &format!("projects/{id}/variables")) {
            Ok(vars) => gitlab_variables_finding(
                &vars,
                project
                    .get("ci_allow_fork_pipelines_to_run_in_parent_project")
                    .and_then(|f| f.as_bool())
                    == Some(true),
            ),
            Err(e) if matches!(e.kind, ForgeErrorKind::NotFound | ForgeErrorKind::Denied) => {
                let why = e.to_string();
                hidden(
                    "secret-scoping",
                    "the project's CI/CD variables",
                    &why,
                    &gitlab_need(&why, "the Maintainer role"),
                )
            }
            Err(e) => down("secret-scoping", forge.kind, &e.to_string()),
        },
    );
    out.push(match ask(api, forge, "personal_access_tokens/self") {
        Answer::Ok(v) => gitlab_token_finding(&v, crate::lease::now()),
        // A CI job token, a deploy token or an OAuth token has no self-description.
        Answer::Hidden(_) => Finding::new(
            "forge-token",
            Status::Info,
            "this token does not describe itself (`personal_access_tokens/self`): a CI job token, a deploy token or an OAuth token",
        ),
        Answer::Down(why) => down("forge-token", forge.kind, &why),
    });
    out
}

/// `default-token` on GitLab: other projects' CI job tokens are kept out by the inbound
/// allowlist (`job_token_scope.inbound_enabled`), and a job token cannot push to the
/// repository (`ci_push_repository_for_job_token_allowed`).
pub fn gitlab_job_token_finding(scope: &serde_json::Value, project: &serde_json::Value) -> Finding {
    let inbound = scope.get("inbound_enabled").and_then(|b| b.as_bool());
    let push = project
        .get("ci_push_repository_for_job_token_allowed")
        .and_then(|b| b.as_bool());
    let mut wrong = Vec::new();
    if inbound != Some(true) {
        wrong.push("a CI job token from any project can call this project's API (the job-token allowlist is off: `inbound_enabled: false`)");
    }
    if push == Some(true) {
        wrong.push("a CI job token can push to this repository (`ci_push_repository_for_job_token_allowed`)");
    }
    if wrong.is_empty() {
        return Finding::new(
            "default-token",
            Status::Pass,
            "only allowlisted projects' CI job tokens reach this project, and a job token cannot push to it",
        );
    }
    Finding::new("default-token", Status::Warn, wrong.join("; "))
        .fix("Settings → CI/CD → Job token permissions: limit access to this project to the allowlist, and do not let job tokens push to the repository.")
}

/// `secret-scoping` on GitLab: a variable marked sensitive (masked or hidden) that is
/// neither protected nor scoped to one environment reaches every pipeline of the
/// project, including merge-request pipelines; when fork pipelines run in the project
/// (`ci_allow_fork_pipelines_to_run_in_parent_project`), a fork's change can read it.
/// Variables that are not masked are configuration, counted but not reported. Names
/// only: a value is never read into a finding.
pub fn gitlab_variables_finding(vars: &[serde_json::Value], forks_run: bool) -> Finding {
    if vars.is_empty() {
        return Finding::new(
            "secret-scoping",
            Status::Pass,
            "no project-level CI/CD variables",
        );
    }
    let flag = |v: &serde_json::Value, k: &str| v.get(k).and_then(|b| b.as_bool()) == Some(true);
    let exposed: Vec<String> = vars
        .iter()
        .filter(|v| (flag(v, "masked") || flag(v, "hidden")) && !flag(v, "protected"))
        .filter(|v| {
            v.get("environment_scope")
                .and_then(|e| e.as_str())
                .unwrap_or("*")
                == "*"
        })
        .filter_map(|v| {
            v.get("key")
                .and_then(|k| k.as_str())
                .map(|k| format!("`{k}`"))
        })
        .collect();
    if exposed.is_empty() {
        return Finding::new(
            "secret-scoping",
            Status::Pass,
            format!(
                "{} project-level CI/CD variable(s); each masked one is protected or scoped to an environment",
                vars.len()
            ),
        );
    }
    let forks = if forks_run {
        "; pipelines for merge requests from forks run in this project (`ci_allow_fork_pipelines_to_run_in_parent_project`), so a fork's change can read them"
    } else {
        ""
    };
    Finding::new(
        "secret-scoping",
        Status::Warn,
        format!(
            "masked CI/CD variable(s) every pipeline of the project can read (not protected, every environment): {}{forks}",
            exposed.join(", ")
        ),
    )
    .fix("Settings → CI/CD → Variables: mark each one Protected, so only pipelines on protected branches and tags get it, or scope it to the environment whose jobs use it.")
}

/// How many days ahead a GitLab token's expiry is reported (`forge-token`).
pub const TOKEN_EXPIRY_WARN_DAYS: i64 = 14;

/// `forge-token` on GitLab: the token `doctor` runs with, from its own description
/// (`personal_access_tokens/self`). `doctor` and the gates only read, so any scope that
/// is not `read_*` is more than they need; a token that never expires, or expires within
/// [`TOKEN_EXPIRY_WARN_DAYS`], is reported too. `now` is Unix seconds.
pub fn gitlab_token_finding(v: &serde_json::Value, now: i64) -> Finding {
    let scopes: Vec<&str> = v
        .get("scopes")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();
    let beyond: Vec<String> = scopes
        .iter()
        .filter(|s| !s.starts_with("read_"))
        .map(|s| format!("`{s}`"))
        .collect();
    let expires = v.get("expires_at").and_then(|e| e.as_str());
    let days_left = expires
        .and_then(|d| crate::ratification::parse_time(&format!("{d}T00:00:00Z")))
        .map(|t| (t - now).div_euclid(86_400));
    let mut wrong = Vec::new();
    if !beyond.is_empty() {
        wrong.push(format!(
            "the token can do more than read: {}; `doctor` and the gates need `read_api` only",
            beyond.join(", ")
        ));
    }
    match (expires, days_left) {
        (None, _) => wrong.push("the token never expires".to_string()),
        (Some(d), Some(n)) if n < TOKEN_EXPIRY_WARN_DAYS => {
            wrong.push(format!("the token expires on {d} ({n} day(s))"))
        }
        _ => {}
    }
    if wrong.is_empty() {
        return Finding::new(
            "forge-token",
            Status::Pass,
            format!(
                "the token reads only ({}) and expires on {}",
                scopes.join(", "),
                expires.unwrap_or("?")
            ),
        );
    }
    Finding::new("forge-token", Status::Warn, wrong.join("; ")).fix(
        "Create a token with `read_api` only (User settings → Access tokens), with an expiry, and run `doctor` and CI with it.",
    )
}

/// The workflows (by path) that run on a pull request: `on:` names `pull_request` or
/// `pull_request_target`, as a string, a list or a mapping key.
pub fn pull_request_workflows(workflows: &[(String, String)]) -> BTreeSet<String> {
    let runs_on_pr = |on: &serde_yaml::Value| {
        let is_pr = |e: &str| matches!(e, "pull_request" | "pull_request_target");
        match on {
            serde_yaml::Value::String(e) => is_pr(e),
            serde_yaml::Value::Sequence(es) => es.iter().filter_map(|e| e.as_str()).any(is_pr),
            serde_yaml::Value::Mapping(m) => m.keys().filter_map(|e| e.as_str()).any(is_pr),
            _ => false,
        }
    };
    workflows
        .iter()
        .filter(|(_, content)| {
            serde_yaml::from_str::<serde_yaml::Value>(content)
                .ok()
                .and_then(|wf| wf.get("on").cloned())
                .is_some_and(|on| runs_on_pr(&on))
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// `secret-scoping` on Gitea and Forgejo, whose Actions secrets have no environment or
/// other scope (recorded: `tests/fixtures/forge_settings/`): every repository and
/// organisation secret is readable by a workflow on any branch of the repository, so
/// that exposure is stated, not warned about. A secret no local workflow reads adds
/// that exposure for nothing, which is a warning. The pull-request workflows that read a
/// secret are named. Names only.
pub fn gitea_secrets_finding(
    kind: ForgeKind,
    repo_secrets: &[String],
    org_secrets: &[String],
    uses: &SecretUses,
    pr_workflows: &BTreeSet<String>,
    org_note: Option<&str>,
) -> Finding {
    let note = org_note.map(|n| format!("; {n}")).unwrap_or_default();
    if repo_secrets.is_empty() && org_secrets.is_empty() {
        return Finding::new(
            "secret-scoping",
            Status::Pass,
            format!("no Actions secrets for this repository or its organisation{note}"),
        );
    }
    let mut all: Vec<&String> = repo_secrets.iter().chain(org_secrets).collect();
    all.sort_unstable_by_key(|n| n.to_ascii_uppercase());
    all.dedup_by_key(|n| n.to_ascii_uppercase());
    let read_by = |n: &str| uses.reads.get(&n.to_ascii_uppercase());
    let unread: Vec<String> = if uses.inherit.is_empty() {
        all.iter()
            .filter(|n| read_by(n).is_none())
            .map(|n| format!("`{n}`"))
            .collect()
    } else {
        Vec::new()
    };
    let on_pr: Vec<String> = all
        .iter()
        .filter_map(|n| {
            let jobs: Vec<&str> = read_by(n)?
                .iter()
                .filter(|(job, _)| {
                    pr_workflows
                        .iter()
                        .any(|w| job.starts_with(&format!("{w} job ")))
                })
                .map(|(job, _)| job.as_str())
                .collect();
            (!jobs.is_empty()).then(|| format!("`{n}` ({})", jobs.join(", ")))
        })
        .collect();
    let mut summary = format!(
        "{} Actions secret(s): {} repository, {} organisation. {} has no environment to scope a secret to, so a workflow on any branch of this repository can read each one",
        all.len(),
        repo_secrets.len(),
        org_secrets.len(),
        kind.label()
    );
    if !on_pr.is_empty() {
        summary.push_str(&format!(
            "; read in workflows that run on pull requests: {}",
            on_pr.join(", ")
        ));
    }
    if unread.is_empty() {
        return Finding::new("secret-scoping", Status::Info, format!("{summary}{note}"));
    }
    Finding::new(
        "secret-scoping",
        Status::Warn,
        format!(
            "{summary}; not read by any workflow here: {}{note}",
            unread.join(", ")
        ),
    )
    .fix("Delete the secrets no workflow reads (Settings → Actions → Secrets, of the repository or the organisation): a workflow on any branch can still read them.")
}

/// The Gitea / Forgejo secret lists (names only) behind [`gitea_secrets_finding`]. An
/// organisation's list is read when the owner is one: a 404 (a user owns the
/// repository) leaves it out, a 403 (the token is not an organisation owner) is said.
fn gitea_secret_scoping(
    api: &dyn ForgeApi,
    forge: &Forge,
    workflows: &[(String, String)],
) -> Finding {
    let names = |list: Vec<serde_json::Value>| -> Vec<String> {
        list.iter()
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect()
    };
    let repo = match read_all(api, forge, &format!("repos/{}/actions/secrets", forge.repo)) {
        Ok(l) => names(l),
        Err(e) if matches!(e.kind, ForgeErrorKind::NotFound | ForgeErrorKind::Denied) => {
            return hidden(
                "secret-scoping",
                "the repository's secret list",
                &e.to_string(),
                "a token of a repository administrator (`write:repository`)",
            )
        }
        Err(e) => return down("secret-scoping", forge.kind, &e.to_string()),
    };
    let owner = forge.repo.split('/').next().unwrap_or_default();
    let (org, org_note) = match read_all(api, forge, &format!("orgs/{owner}/actions/secrets")) {
        Ok(l) => (names(l), None),
        Err(e) if e.kind == ForgeErrorKind::NotFound => (Vec::new(), None),
        Err(e) if e.kind == ForgeErrorKind::Denied => (
            Vec::new(),
            Some("the organisation's secrets are not visible to this token (an organisation owner's token lists them)"),
        ),
        Err(e) => return down("secret-scoping", forge.kind, &e.to_string()),
    };
    gitea_secrets_finding(
        forge.kind,
        &repo,
        &org,
        &secret_uses(workflows),
        &pull_request_workflows(workflows),
        org_note,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CannedApi;
    use serde_json::json;

    fn gh() -> Forge {
        Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }
    }

    fn status_of(f: &[Finding], id: &str) -> Status {
        f.iter()
            .find(|x| x.id == id)
            .unwrap_or_else(|| panic!("no {id} in {f:?}"))
            .status
    }

    /// A GitHub repository with every setting at its safe value; tests replace one answer.
    fn healthy() -> CannedApi {
        let mut api = CannedApi::default();
        let mut put = |k: &str, v: serde_json::Value| {
            api.responses.insert(format!("github:{k}"), v);
        };
        put("repos/o/r", json!({"permissions": {"admin": true}}));
        put(
            "repos/o/r/actions/permissions",
            json!({"enabled": true, "allowed_actions": "selected", "sha_pinning_required": true}),
        );
        put(
            "repos/o/r/actions/permissions/workflow",
            json!({"default_workflow_permissions": "read", "can_approve_pull_request_reviews": false}),
        );
        put("repos/o/r/immutable-releases", json!({"enabled": true}));
        put(
            "repos/o/r/rulesets?targets=tag&includes_parents=true&per_page=100&page=1",
            json!([{"id": 7, "target": "tag", "enforcement": "active"}]),
        );
        put(
            "repos/o/r/rulesets/7",
            json!({"name": "release tags", "target": "tag", "enforcement": "active",
                   "conditions": {"ref_name": {"include": ["refs/tags/v*.*.*"], "exclude": []}},
                   "rules": [{"type": "update"}, {"type": "deletion"}]}),
        );
        put("repos/o/r/releases/latest", json!({"tag_name": "v1.2.3"}));
        put(
            "repos/o/r/actions/secrets?per_page=100&page=1",
            json!({"total_count": 0, "secrets": []}),
        );
        api
    }

    fn set(api: &mut CannedApi, k: &str, v: serde_json::Value) {
        api.responses.insert(format!("github:{k}"), v);
    }

    #[test]
    fn healthy_github_settings_all_pass() {
        let f = findings(&healthy(), &gh(), &[]);
        let ids: Vec<&str> = f.iter().map(|x| x.id).collect();
        assert_eq!(ids, IDS);
        // Every setting passes; the token audit is not available on GitHub.
        assert!(
            f.iter()
                .filter(|x| x.id != "forge-token")
                .all(|x| x.status == Status::Pass),
            "{f:?}"
        );
        let token = f.iter().find(|x| x.id == "forge-token").unwrap();
        assert_eq!(token.status, Status::Info, "{token:?}");
    }

    #[test]
    fn actions_policy_warns_on_each_unsafe_value() {
        let mut api = healthy();
        set(
            &mut api,
            "repos/o/r/actions/permissions",
            json!({"enabled": true, "allowed_actions": "all", "sha_pinning_required": false}),
        );
        set(
            &mut api,
            "repos/o/r/actions/permissions/workflow",
            json!({"default_workflow_permissions": "write", "can_approve_pull_request_reviews": true}),
        );
        let f = findings(&api, &gh(), &[]);
        for id in [
            "actions-sha-pinning",
            "allowed-actions",
            "default-token",
            "actions-approve-prs",
        ] {
            assert_eq!(status_of(&f, id), Status::Warn, "{id}");
        }
        // The other settings are untouched.
        assert_eq!(status_of(&f, "immutable-releases"), Status::Pass);
        // `local_only` is as narrow as `selected`.
        let local =
            actions_policy(&json!({"allowed_actions": "local_only", "sha_pinning_required": true}));
        assert_eq!(status_of(&local, "allowed-actions"), Status::Pass);
    }

    #[test]
    fn admin_only_settings_hidden_from_the_token_warn_not_pass() {
        let mut api = healthy();
        set(
            &mut api,
            "repos/o/r",
            json!({"permissions": {"admin": false}}),
        );
        let forbidden = json!({"__status": 403, "__body": {"message": "Must have admin rights"}});
        set(&mut api, "repos/o/r/actions/permissions", forbidden.clone());
        set(
            &mut api,
            "repos/o/r/actions/permissions/workflow",
            forbidden.clone(),
        );
        set(
            &mut api,
            "repos/o/r/immutable-releases",
            serde_json::Value::Null,
        );
        set(
            &mut api,
            "repos/o/r/actions/secrets?per_page=100&page=1",
            forbidden,
        );
        let f = findings(&api, &gh(), &[]);
        for id in [
            "actions-sha-pinning",
            "allowed-actions",
            "default-token",
            "actions-approve-prs",
            "immutable-releases",
            "secret-scoping",
        ] {
            let x = f.iter().find(|x| x.id == id).unwrap();
            assert_eq!(x.status, Status::Warn, "{id}");
            assert!(
                x.summary.starts_with("could not check ("),
                "{id}: {}",
                x.summary
            );
            assert!(x.remediation.as_deref().unwrap().contains("admin"), "{id}");
        }
    }

    #[test]
    fn immutable_releases_404_is_off_for_an_admin_token() {
        let mut api = healthy();
        set(
            &mut api,
            "repos/o/r/immutable-releases",
            serde_json::Value::Null,
        );
        let f = findings(&api, &gh(), &[]);
        let x = f.iter().find(|x| x.id == "immutable-releases").unwrap();
        assert_eq!(x.status, Status::Warn);
        assert!(
            x.summary.starts_with("releases are not immutable"),
            "{}",
            x.summary
        );
        set(
            &mut api,
            "repos/o/r/immutable-releases",
            json!({"enabled": false}),
        );
        assert_eq!(
            status_of(&findings(&api, &gh(), &[]), "immutable-releases"),
            Status::Warn
        );
    }

    #[test]
    fn an_unreachable_forge_is_unknown_for_every_setting_and_asks_nothing_more() {
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r".into(),
            json!({"__error": "network access is disabled (DISCIPLINE_NO_NETWORK)"}),
        );
        let f = findings(&api, &gh(), &[]);
        assert_eq!(f.len(), IDS.len());
        assert!(f.iter().all(|x| x.status == Status::Unknown), "{f:?}");
        assert_eq!(api.log(), vec!["github:repos/o/r".to_string()]);
    }

    #[test]
    fn tag_ruleset_must_cover_the_release_tag_and_block_update_and_deletion() {
        let rs = |include: &[&str], update: bool, deletion: bool| TagRuleset {
            name: "t".into(),
            include: include.iter().map(|s| s.to_string()).collect(),
            exclude: vec![],
            blocks_update: update,
            blocks_deletion: deletion,
        };
        let ok = rs(&["refs/tags/v*.*.*"], true, true);
        assert_eq!(
            tag_ruleset_finding(std::slice::from_ref(&ok), Some("v1.2.3")).status,
            Status::Pass
        );
        assert_eq!(
            tag_ruleset_finding(&[rs(&["~ALL"], true, true)], Some("v1")).status,
            Status::Pass
        );
        // The pattern does not cover the tag.
        assert_eq!(
            tag_ruleset_finding(std::slice::from_ref(&ok), Some("release-7")).status,
            Status::Warn
        );
        // Covers it, but deletion is allowed.
        assert_eq!(
            tag_ruleset_finding(&[rs(&["refs/tags/v*"], true, false)], Some("v1.0.0")).status,
            Status::Warn
        );
        assert_eq!(
            tag_ruleset_finding(&[], Some("v1.0.0")).status,
            Status::Warn
        );
        // Excluded.
        let mut ex = ok.clone();
        ex.exclude = vec!["refs/tags/v1.*".into()];
        assert_eq!(
            tag_ruleset_finding(&[ex], Some("v1.2.3")).status,
            Status::Warn
        );
        // No release yet: any blocking tag ruleset.
        assert_eq!(tag_ruleset_finding(&[ok], None).status, Status::Pass);
    }

    #[test]
    fn a_ruleset_in_evaluate_mode_or_on_branches_does_not_protect_tags() {
        let mut api = healthy();
        set(
            &mut api,
            "repos/o/r/rulesets/7",
            json!({"name": "release tags", "target": "tag", "enforcement": "evaluate",
                   "conditions": {"ref_name": {"include": ["~ALL"]}},
                   "rules": [{"type": "update"}, {"type": "deletion"}]}),
        );
        assert_eq!(
            status_of(&findings(&api, &gh(), &[]), "tag-protection"),
            Status::Warn
        );
        set(
            &mut api,
            "repos/o/r/rulesets?targets=tag&includes_parents=true&per_page=100&page=1",
            json!([]),
        );
        assert_eq!(
            status_of(&findings(&api, &gh(), &[]), "tag-protection"),
            Status::Warn
        );
    }

    #[test]
    fn protected_tags_on_gitea_forgejo_and_gitlab() {
        let pats = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for kind in [ForgeKind::Gitea, ForgeKind::Forgejo, ForgeKind::GitLab] {
            assert_eq!(
                protected_tag_finding(kind, &pats(&["v*"]), Some("v1.2.3")).status,
                Status::Pass
            );
            assert_eq!(
                protected_tag_finding(kind, &pats(&["release-*"]), Some("v1.2.3")).status,
                Status::Warn
            );
            assert_eq!(protected_tag_finding(kind, &[], None).status, Status::Warn);
        }
        // Gitea / Forgejo read `/.../` as a regular expression.
        assert_eq!(
            protected_tag_finding(
                ForgeKind::Gitea,
                &pats(&[r"/^v\d+\.\d+\.\d+$/"]),
                Some("v1.2.3")
            )
            .status,
            Status::Pass
        );

        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r/tag_protections?limit=50&page=1".into(),
            json!([{"id": 1, "name_pattern": "v*"}]),
        );
        api.responses.insert(
            "gitea:repos/o/r/tag_protections?limit=50&page=2".into(),
            json!([]),
        );
        api.responses.insert(
            "gitea:repos/o/r/releases/latest".into(),
            json!({"tag_name": "v2.0.0"}),
        );
        let forge = Forge {
            kind: ForgeKind::Gitea,
            ..gh()
        };
        let f = findings(&api, &forge, &[]);
        let ids: Vec<&str> = f.iter().map(|x| x.id).collect();
        assert_eq!(ids, IDS);
        assert_eq!(status_of(&f, "tag-protection"), Status::Pass);
        // `secret-scoping` is read on Gitea and Forgejo (its own tests below).
        for id in IDS
            .iter()
            .filter(|i| !matches!(**i, "tag-protection" | "secret-scoping"))
        {
            let x = f.iter().find(|x| x.id == *id).unwrap();
            assert_eq!(x.status, Status::Info, "{id}");
            assert!(x.summary.contains("not available on this forge"), "{id}");
        }
    }

    const DEPLOY: &str = r#"
on: push
env:
  SHARED: ${{ secrets.EVERYWHERE }}
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo "${{ secrets.BUILD_KEY }}"
  deploy:
    runs-on: ubuntu-latest
    environment: production
    steps:
      - run: deploy
        env:
          TOKEN: ${{ secrets.deploy_token }}
          ALSO: ${{ secrets['BUILD_KEY'] }}
  pages:
    runs-on: ubuntu-latest
    environment:
      name: github-pages
    steps:
      - run: echo "${{ secrets.DEPLOY_TOKEN }}"
"#;

    #[test]
    fn a_secret_read_only_from_environment_jobs_warns() {
        let wf = vec![(".github/workflows/d.yml".to_string(), DEPLOY.to_string())];
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let uses = secret_uses(&wf);
        // DEPLOY_TOKEN is read by two environment-bound jobs only (names match without case).
        let f = secret_scoping_finding(&names(&["DEPLOY_TOKEN", "BUILD_KEY", "EVERYWHERE"]), &uses);
        assert_eq!(f.status, Status::Warn);
        assert!(f
            .summary
            .contains("`DEPLOY_TOKEN` (environment github-pages, production)"));
        assert!(!f.summary.contains("BUILD_KEY"));
        assert!(!f.summary.contains("EVERYWHERE"));
        // Read by a job outside an environment: nothing to move.
        let f = secret_scoping_finding(&names(&["BUILD_KEY", "UNUSED"]), &uses);
        assert_eq!(f.status, Status::Pass);
        assert!(f
            .summary
            .contains("not read by any workflow here: `UNUSED`"));
        // `secrets: inherit` outside an environment may pass any secret on.
        let inherit = "on: push\njobs:\n  call:\n    uses: o/r/.github/workflows/x.yml@0123456789012345678901234567890123456789\n    secrets: inherit\n";
        let mut wf2 = wf.clone();
        wf2.push((".github/workflows/c.yml".into(), inherit.into()));
        assert_eq!(
            secret_scoping_finding(&names(&["DEPLOY_TOKEN"]), &secret_uses(&wf2)).status,
            Status::Pass
        );
        assert_eq!(secret_scoping_finding(&[], &uses).status, Status::Pass);
    }

    #[test]
    fn secret_scoping_reads_the_forge_list_by_name() {
        let mut api = healthy();
        set(
            &mut api,
            "repos/o/r/actions/secrets?per_page=100&page=1",
            json!({"total_count": 1, "secrets": [{"name": "DEPLOY_TOKEN"}]}),
        );
        let wf = vec![(".github/workflows/d.yml".to_string(), DEPLOY.to_string())];
        assert_eq!(
            status_of(&findings(&api, &gh(), &wf), "secret-scoping"),
            Status::Warn
        );
        assert_eq!(
            status_of(&findings(&api, &gh(), &[]), "secret-scoping"),
            Status::Pass
        );
    }

    /// The responses recorded from gitlab.com (Phase 14 Step 0).
    fn gitlab_recorded() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/forge_settings/gitlab.json")).unwrap()
    }

    const GITLAB_DENIAL: &str = "Access denied: This operation requires a fine-grained personal access token with the following project permissions: [Variable: Read].";

    #[test]
    fn a_gitlab_denial_names_the_permission_the_token_lacks() {
        assert_eq!(
            gitlab_missing_permission(GITLAB_DENIAL),
            Some("Variable: Read")
        );
        assert_eq!(
            gitlab_missing_permission("gitlab.com answered HTTP 403 403 Forbidden"),
            None
        );
        assert_eq!(gitlab_missing_permission("permissions: []"), None);
        assert!(gitlab_need(GITLAB_DENIAL, "the Maintainer role").contains("`Variable: Read`"));
        assert!(!gitlab_need("403 Forbidden", "the Maintainer role").contains('['));
    }

    #[test]
    fn gitlab_job_token_passes_only_with_the_allowlist_on_and_no_push() {
        let r = gitlab_recorded();
        let (scope, project) = (&r["job_token_scope"], &r["project_ci_settings"]);
        assert_eq!(
            gitlab_job_token_finding(scope, project).status,
            Status::Pass
        );
        let open = json!({"inbound_enabled": false, "outbound_enabled": false});
        let f = gitlab_job_token_finding(&open, project);
        assert_eq!(f.status, Status::Warn);
        assert!(f.summary.contains("inbound_enabled: false"), "{f:?}");
        let mut pushing = project.clone();
        pushing["ci_push_repository_for_job_token_allowed"] = json!(true);
        let f = gitlab_job_token_finding(scope, &pushing);
        assert_eq!(f.status, Status::Warn);
        assert!(f.summary.contains("can push"), "{f:?}");
    }

    #[test]
    fn gitlab_masked_variables_readable_by_every_pipeline_are_reported_by_name() {
        let r = gitlab_recorded();
        let vars = r["variables"].as_array().unwrap().clone();
        // As recorded: the masked one is scoped to `production`, the other two are not
        // masked (configuration).
        assert_eq!(gitlab_variables_finding(&vars, true).status, Status::Pass);
        assert_eq!(gitlab_variables_finding(&[], true).status, Status::Pass);

        let mut widened = vars.clone();
        for v in widened.iter_mut() {
            if v["key"] == "DEPLOY_TOKEN" {
                v["environment_scope"] = json!("*");
            }
        }
        let f = gitlab_variables_finding(&widened, false);
        assert_eq!(f.status, Status::Warn);
        assert!(f.summary.contains("`DEPLOY_TOKEN`"), "{f:?}");
        assert!(
            !f.summary.contains("PLAIN_VAR"),
            "an unmasked variable: {f:?}"
        );
        assert!(!f.summary.contains("fork"), "{f:?}");
        let f = gitlab_variables_finding(&widened, true);
        assert!(f.summary.contains("forks"), "{f:?}");

        // Protected is enough; a hidden variable counts as masked.
        let mut protected = widened.clone();
        for v in protected.iter_mut() {
            v["protected"] = json!(true);
        }
        assert_eq!(
            gitlab_variables_finding(&protected, true).status,
            Status::Pass
        );
        let hidden_var = vec![
            json!({"key": "H", "masked": false, "hidden": true, "protected": false, "environment_scope": "*"}),
        ];
        assert_eq!(
            gitlab_variables_finding(&hidden_var, false).status,
            Status::Warn
        );
    }

    #[test]
    fn a_gitlab_token_that_can_write_or_expires_soon_is_reported() {
        let now = crate::ratification::parse_time("2026-09-29T00:00:00Z").unwrap();
        // The recorded probing token: `api`, write and runner scopes.
        let f = gitlab_token_finding(&gitlab_recorded()["token_self"], now);
        assert_eq!(f.status, Status::Warn);
        for s in ["`api`", "`write_repository`", "`create_runner`"] {
            assert!(f.summary.contains(s), "{s}: {f:?}");
        }
        assert!(!f.summary.contains("`read_api`,"), "{f:?}");

        let read_only = |expires: serde_json::Value| json!({"scopes": ["read_api", "read_repository"], "expires_at": expires});
        assert_eq!(
            gitlab_token_finding(&read_only(json!("2026-12-01")), now).status,
            Status::Pass
        );
        let soon = gitlab_token_finding(&read_only(json!("2026-10-05")), now);
        assert_eq!(soon.status, Status::Warn);
        assert!(soon.summary.contains("2026-10-05 (6 day(s))"), "{soon:?}");
        let never = gitlab_token_finding(&read_only(serde_json::Value::Null), now);
        assert_eq!(never.status, Status::Warn);
        assert!(never.summary.contains("never expires"), "{never:?}");
        // Exactly at the threshold is not reported.
        assert_eq!(
            gitlab_token_finding(&read_only(json!("2026-10-13")), now).status,
            Status::Pass
        );
        assert_eq!(
            gitlab_token_finding(&read_only(json!("2026-10-12")), now).status,
            Status::Warn
        );
    }

    #[test]
    fn gitlab_settings_are_read_and_a_denial_names_the_missing_permission() {
        let r = gitlab_recorded();
        let forge = Forge {
            kind: ForgeKind::GitLab,
            url: "https://gitlab.example.com".into(),
            repo: "o/r".into(),
        };
        let mut api = CannedApi::default();
        let mut put = |k: &str, v: serde_json::Value| {
            api.responses.insert(format!("gitlab:projects/o%2Fr{k}"), v);
        };
        put("", r["project_ci_settings"].clone());
        put("/job_token_scope", r["job_token_scope"].clone());
        put("/variables?per_page=100&page=1", r["variables"].clone());
        put(
            "/protected_tags?per_page=100&page=1",
            r["protected_tags"].clone(),
        );
        put("/releases?per_page=1", json!([{"tag_name": "v1.0.0"}]));
        api.responses.insert(
            "gitlab:personal_access_tokens/self".into(),
            json!({"scopes": ["read_api"], "expires_at": "2099-01-01"}),
        );
        let f = findings(&api, &forge, &[]);
        let ids: Vec<&str> = f.iter().map(|x| x.id).collect();
        assert_eq!(ids, IDS);
        for id in [
            "default-token",
            "tag-protection",
            "secret-scoping",
            "forge-token",
        ] {
            assert_eq!(status_of(&f, id), Status::Pass, "{id}: {f:?}");
        }
        for id in [
            "actions-sha-pinning",
            "allowed-actions",
            "actions-approve-prs",
            "immutable-releases",
        ] {
            assert_eq!(status_of(&f, id), Status::Info, "{id}");
        }

        // A fine-grained token without `Variable: Read`: the fix names it.
        api.responses.insert(
            "gitlab:projects/o%2Fr/variables?per_page=100&page=1".into(),
            json!({"__status": 403, "__body": {"error": "insufficient_granular_scope", "error_description": GITLAB_DENIAL}}),
        );
        let f = findings(&api, &forge, &[]);
        let scoping = f.iter().find(|x| x.id == "secret-scoping").unwrap();
        assert_eq!(scoping.status, Status::Warn, "{scoping:?}");
        assert!(
            scoping
                .remediation
                .as_deref()
                .unwrap_or("")
                .contains("`Variable: Read`"),
            "{scoping:?}"
        );

        // A CI job token cannot describe itself: information, not a failure.
        api.responses.insert(
            "gitlab:personal_access_tokens/self".into(),
            json!({"__status": 401, "__body": {"message": "401 Unauthorized"}}),
        );
        let f = findings(&api, &forge, &[]);
        assert_eq!(status_of(&f, "forge-token"), Status::Info);
    }

    /// The secret lists recorded from Gitea and Forgejo (Phase 14 Step 0).
    fn gitea_recorded(forge: &str) -> serde_json::Value {
        let text = match forge {
            "gitea" => include_str!("../tests/fixtures/forge_settings/gitea.json"),
            _ => include_str!("../tests/fixtures/forge_settings/forgejo.json"),
        };
        serde_json::from_str(text).unwrap()
    }

    const PR_DEPLOY: &str = r#"
on:
  pull_request:
  push:
    branches: [main]
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - run: deploy
        env:
          KEY: ${{ secrets.DEPLOY_KEY }}
"#;
    const PUSH_ONLY: &str = r#"
on: [push]
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - run: publish ${{ secrets.ORG_TOKEN }}
"#;

    #[test]
    fn pull_request_workflows_are_found_in_every_spelling_of_on() {
        let wf = |on: &str| (format!("w-{on}.yml"), format!("on: {on}\njobs: {{}}\n"));
        let set = pull_request_workflows(&[
            wf("pull_request"),
            wf("[push, pull_request_target]"),
            wf("{pull_request: {types: [opened]}}"),
            wf("push"),
            wf("[push, workflow_dispatch]"),
        ]);
        let got: Vec<&str> = set.iter().map(String::as_str).collect();
        assert_eq!(
            got,
            [
                "w-[push, pull_request_target].yml",
                "w-pull_request.yml",
                "w-{pull_request: {types: [opened]}}.yml"
            ]
        );
    }

    #[test]
    fn gitea_secrets_are_stated_and_an_unread_one_is_reported() {
        for forge in ["gitea", "forgejo"] {
            let r = gitea_recorded(forge);
            let list = |k: &str| -> Vec<String> {
                r[k].as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s["name"].as_str().unwrap().to_string())
                    .collect()
            };
            let (repo, org) = (list("repo_secrets"), list("org_secrets"));
            let kind = if forge == "gitea" {
                ForgeKind::Gitea
            } else {
                ForgeKind::Forgejo
            };
            let workflows = vec![
                (
                    ".gitea/workflows/deploy.yml".to_string(),
                    PR_DEPLOY.to_string(),
                ),
                (
                    ".gitea/workflows/publish.yml".to_string(),
                    PUSH_ONLY.to_string(),
                ),
            ];
            let uses = secret_uses(&workflows);
            let prs = pull_request_workflows(&workflows);

            // Both recorded secrets are read: information, naming the pull-request reader.
            let f = gitea_secrets_finding(kind, &repo, &org, &uses, &prs, None);
            assert_eq!(f.status, Status::Info, "{forge}");
            assert!(f.summary.contains("1 repository, 1 organisation"));
            assert!(f.summary.contains("no environment to scope"));
            assert!(f
                .summary
                .contains("`DEPLOY_KEY` (.gitea/workflows/deploy.yml job `deploy`)"));
            assert!(!f.summary.contains("`ORG_TOKEN` ("), "a push-only reader");

            // Drop the publish workflow: `ORG_TOKEN` is read by nothing.
            let only_pr = &workflows[..1];
            let f = gitea_secrets_finding(
                kind,
                &repo,
                &org,
                &secret_uses(only_pr),
                &pull_request_workflows(only_pr),
                None,
            );
            assert_eq!(f.status, Status::Warn);
            assert!(f
                .summary
                .contains("not read by any workflow here: `ORG_TOKEN`"));
            assert!(!f.summary.contains("`DEPLOY_KEY`, "));

            // `secrets: inherit` may pass any secret on: none is called unread.
            let inherit = vec![(
                ".gitea/workflows/call.yml".to_string(),
                "on: push\njobs:\n  call:\n    uses: ./.gitea/workflows/r.yml\n    secrets: inherit\n".to_string(),
            )];
            let f = gitea_secrets_finding(
                kind,
                &repo,
                &org,
                &secret_uses(&inherit),
                &BTreeSet::new(),
                None,
            );
            assert_eq!(f.status, Status::Info);

            assert_eq!(
                gitea_secrets_finding(kind, &[], &[], &uses, &prs, None).status,
                Status::Pass
            );
        }
    }

    #[test]
    fn gitea_secret_lists_are_read_and_a_user_owner_has_no_organisation() {
        let r = gitea_recorded("gitea");
        let forge = Forge {
            kind: ForgeKind::Gitea,
            url: "https://git.example.com".into(),
            repo: "o/r".into(),
        };
        let mut api = CannedApi::default();
        let page = |p: u32| format!("limit=50&page={p}");
        api.responses.insert(
            format!("gitea:repos/o/r/actions/secrets?{}", page(1)),
            r["repo_secrets"].clone(),
        );
        api.responses.insert(
            format!("gitea:repos/o/r/actions/secrets?{}", page(2)),
            json!([]),
        );
        api.responses.insert(
            format!("gitea:orgs/o/actions/secrets?{}", page(1)),
            r["org_secrets"].clone(),
        );
        api.responses.insert(
            format!("gitea:orgs/o/actions/secrets?{}", page(2)),
            json!([]),
        );
        let workflows = vec![(
            ".gitea/workflows/deploy.yml".to_string(),
            PR_DEPLOY.to_string(),
        )];
        let f = gitea_secret_scoping(&api, &forge, &workflows);
        assert_eq!(f.status, Status::Warn);
        assert!(f.summary.contains("1 repository, 1 organisation"));
        assert!(f.summary.contains("`ORG_TOKEN`"));

        // A user owns the repository: no organisation list, nothing said about one.
        api.responses.insert(
            format!("gitea:orgs/o/actions/secrets?{}", page(1)),
            json!({"__status": 404, "__body": {"message": "GetOrgByName"}}),
        );
        let f = gitea_secret_scoping(&api, &forge, &workflows);
        assert_eq!(f.status, Status::Info);
        assert!(f.summary.contains("1 repository, 0 organisation"));
        assert!(!f.summary.contains("not visible"));

        // Not an organisation owner: said, not taken for an empty list.
        api.responses.insert(
            format!("gitea:orgs/o/actions/secrets?{}", page(1)),
            json!({"__status": 403, "__body": {"message": "user should be an owner of the organization"}}),
        );
        let f = gitea_secret_scoping(&api, &forge, &workflows);
        assert!(f.summary.contains("organisation's secrets are not visible"));

        // The repository list refused: a warning naming the access, never a pass.
        api.responses.insert(
            format!("gitea:repos/o/r/actions/secrets?{}", page(1)),
            json!({"__status": 403, "__body": {"message": "token does not have required scope"}}),
        );
        let f = gitea_secret_scoping(&api, &forge, &workflows);
        assert_eq!(f.status, Status::Warn);
        assert!(f.summary.contains("could not check"));
    }
}
