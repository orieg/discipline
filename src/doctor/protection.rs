//! The branch rules each forge reports for the default branch, read into one
//! `Protection`.

use super::get;
use crate::forge::{gitlab_project_id, Forge, ForgeApi, ForgeKind};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------------------
// Platform settings
// ---------------------------------------------------------------------------------------

/// Effective protection of one branch, normalised across forges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Protection {
    /// Whether any protection rule covers the branch.
    pub protected: bool,
    pub required_contexts: BTreeSet<String>,
    /// Required contexts are glob patterns (Gitea, Forgejo) rather than exact names.
    pub context_patterns: bool,
    /// GitLab: merges require a successful pipeline (`None` when not visible).
    pub pipeline_must_succeed: Option<Option<bool>>,
    pub strict: bool,
    pub force_push_blocked: bool,
    pub deletion_blocked: bool,
    pub pull_request_required: bool,
    /// Approving reviews a pull request needs (`None` when not visible).
    pub required_approvals: Option<u64>,
    pub code_owner_review: bool,
    pub last_push_approval: bool,
    pub dismiss_stale_reviews: bool,
    pub signatures_required: bool,
    /// Who can bypass the rules (`None` when not visible to this token).
    pub bypass: Option<Vec<String>>,
    /// Whether classic protection also binds administrators (`None` when unknown or unused).
    pub classic_enforce_admins: Option<bool>,
    /// Finding ids whose settings this token could not read.
    pub hidden: BTreeSet<&'static str>,
    /// Merges need every review conversation resolved (GitHub, GitLab). `None` when not
    /// visible, or on a forge without such a rule (Gitea, Forgejo).
    pub thread_resolution_required: Option<bool>,
    /// Gitea / Forgejo: the rule's protected file patterns, which a pull request cannot
    /// change (`None` when not visible).
    pub protected_file_patterns: Option<Vec<String>>,
}

/// The repository's default branch.
/// Which merge methods the repository allows; `None` when the API does not say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeMethods {
    pub squash: Option<bool>,
    pub rebase: Option<bool>,
}

impl MergeMethods {
    /// A squash or rebase merge builds the commit message without the pull request's body.
    pub fn drops_pr_body(&self) -> Option<bool> {
        match (self.squash, self.rebase) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        }
    }
}

/// Read the allowed merge methods from the repository (GitHub `allow_squash_merge` /
/// `allow_rebase_merge`; Gitea and Forgejo `allow_squash_merge` / `allow_rebase` /
/// `allow_rebase_explicit`; GitLab `squash_option` and `merge_method`).
pub fn merge_methods(api: &dyn ForgeApi, forge: &Forge) -> Result<MergeMethods, String> {
    let path = match forge.kind {
        ForgeKind::GitLab => format!("projects/{}", gitlab_project_id(&forge.repo)),
        _ => format!("repos/{}", forge.repo),
    };
    let repo = get(api, forge, &path)?;
    let flag = |k: &str| repo.get(k).and_then(|v| v.as_bool());
    Ok(match forge.kind {
        ForgeKind::GitHub => MergeMethods {
            squash: flag("allow_squash_merge"),
            rebase: flag("allow_rebase_merge"),
        },
        ForgeKind::Gitea | ForgeKind::Forgejo => MergeMethods {
            squash: flag("allow_squash_merge"),
            rebase: match (flag("allow_rebase"), flag("allow_rebase_explicit")) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(false) || b.unwrap_or(false)),
            },
        },
        ForgeKind::GitLab => MergeMethods {
            squash: repo
                .get("squash_option")
                .and_then(|v| v.as_str())
                .map(|o| o != "never"),
            rebase: repo
                .get("merge_method")
                .and_then(|v| v.as_str())
                .map(|m| m == "rebase_merge" || m == "ff"),
        },
    })
}

pub fn default_branch(api: &dyn ForgeApi, forge: &Forge) -> Result<String, String> {
    let path = match forge.kind {
        ForgeKind::GitLab => format!("projects/{}", gitlab_project_id(&forge.repo)),
        _ => format!("repos/{}", forge.repo),
    };
    get(api, forge, &path)?
        .get("default_branch")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "the repository reports no default branch".to_string())
}

/// Read the effective protection of `branch` on GitHub: rulesets merged with classic
/// branch protection.
pub fn github_protection(
    api: &dyn ForgeApi,
    forge: &Forge,
    branch: &str,
) -> Result<Protection, String> {
    let repo = &forge.repo;
    let mut p = Protection {
        thread_resolution_required: Some(false),
        ..Protection::default()
    };
    let rules = get(api, forge, &format!("repos/{repo}/rules/branches/{branch}"))?;
    let rules = rules
        .as_array()
        .ok_or("rules endpoint did not return a list")?;
    let mut ruleset_ids = BTreeSet::new();
    for rule in rules {
        p.protected = true;
        if let Some(id) = rule.get("ruleset_id").and_then(|v| v.as_i64()) {
            ruleset_ids.insert(id);
        }
        let params = rule.get("parameters");
        match rule
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
        {
            "required_status_checks" => {
                if params
                    .and_then(|p| p.get("strict_required_status_checks_policy"))
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    p.strict = true;
                }
                for c in params
                    .and_then(|p| p.get("required_status_checks"))
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    if let Some(ctx) = c.get("context").and_then(|v| v.as_str()) {
                        p.required_contexts.insert(ctx.to_string());
                    }
                }
            }
            "non_fast_forward" => p.force_push_blocked = true,
            "deletion" => p.deletion_blocked = true,
            "pull_request" => {
                p.pull_request_required = true;
                let flag = |k: &str| {
                    params
                        .and_then(|p| p.get(k))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                };
                if let Some(n) = params
                    .and_then(|p| p.get("required_approving_review_count"))
                    .and_then(|v| v.as_u64())
                {
                    p.required_approvals = Some(p.required_approvals.unwrap_or(0).max(n));
                }
                p.code_owner_review |= flag("require_code_owner_review");
                p.last_push_approval |= flag("require_last_push_approval");
                p.dismiss_stale_reviews |= flag("dismiss_stale_reviews_on_push");
                if flag("required_review_thread_resolution") {
                    p.thread_resolution_required = Some(true);
                }
            }
            "required_signatures" => p.signatures_required = true,
            _ => {}
        }
    }

    let mut bypass = Some(Vec::new());
    for id in &ruleset_ids {
        match api.get(forge, &format!("repos/{repo}/rulesets/{id}")) {
            Ok(Some(rs)) => match rs.get("bypass_actors").and_then(|b| b.as_array()) {
                Some(actors) => {
                    if let Some(list) = bypass.as_mut() {
                        for a in actors {
                            let kind = a
                                .get("actor_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("actor");
                            let mode = a
                                .get("bypass_mode")
                                .and_then(|v| v.as_str())
                                .unwrap_or("always");
                            list.push(format!("{kind} ({mode})"));
                        }
                    }
                }
                None => bypass = None,
            },
            _ => bypass = None,
        }
    }
    p.bypass = bypass;

    // Classic branch protection, when enabled, adds to the rulesets.
    let summary = get(api, forge, &format!("repos/{repo}/branches/{branch}"))?;
    let classic_on = summary
        .get("protection")
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        == Some(true);
    if classic_on {
        p.protected = true;
        match api.get(forge, &format!("repos/{repo}/branches/{branch}/protection")) {
            Ok(Some(full)) => {
                let rsc = full.get("required_status_checks");
                if rsc.and_then(|r| r.get("strict")).and_then(|v| v.as_bool()) == Some(true) {
                    p.strict = true;
                }
                for c in rsc
                    .and_then(|r| r.get("contexts"))
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    if let Some(s) = c.as_str() {
                        p.required_contexts.insert(s.to_string());
                    }
                }
                let enabled = |key: &str| {
                    full.get(key)
                        .and_then(|v| v.get("enabled"))
                        .and_then(|v| v.as_bool())
                };
                if enabled("allow_force_pushes") == Some(false) {
                    p.force_push_blocked = true;
                }
                if enabled("allow_deletions") == Some(false) {
                    p.deletion_blocked = true;
                }
                if let Some(reviews) = full.get("required_pull_request_reviews") {
                    p.pull_request_required = true;
                    let flag = |k: &str| reviews.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some(n) = reviews
                        .get("required_approving_review_count")
                        .and_then(|v| v.as_u64())
                    {
                        p.required_approvals = Some(p.required_approvals.unwrap_or(0).max(n));
                    }
                    p.code_owner_review |= flag("require_code_owner_reviews");
                    p.last_push_approval |= flag("require_last_push_approval");
                    p.dismiss_stale_reviews |= flag("dismiss_stale_reviews");
                }
                if enabled("required_signatures") == Some(true) {
                    p.signatures_required = true;
                }
                p.classic_enforce_admins = enabled("enforce_admins");
                if enabled("required_conversation_resolution") == Some(true) {
                    p.thread_resolution_required = Some(true);
                }
            }
            other => {
                // The summary still lists the required contexts to non-admins.
                for c in summary
                    .pointer("/protection/required_status_checks/contexts")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    if let Some(s) = c.as_str() {
                        p.required_contexts.insert(s.to_string());
                    }
                }
                let why = match other {
                    Err(e) => e,
                    _ => "not found".to_string(),
                };
                return Err(format!(
                    "classic branch protection is on but its settings are not readable ({why}); an admin token is needed"
                ));
            }
        }
    }
    Ok(p)
}

/// Read the protection of `branch` on Gitea or Forgejo. The rule list needs a repository
/// admin token; without one, the public branch summary gives the required checks and the
/// rest is reported as not visible.
pub fn gitea_protection(
    api: &dyn ForgeApi,
    forge: &Forge,
    branch: &str,
) -> Result<Protection, String> {
    let repo = &forge.repo;
    let mut p = Protection {
        context_patterns: true,
        ..Protection::default()
    };
    let rules = api.get(forge, &format!("repos/{repo}/branch_protections"));
    let rule = match &rules {
        Ok(Some(serde_json::Value::Array(list))) => {
            let matches = |r: &&serde_json::Value| {
                let name = r
                    .get("rule_name")
                    .or_else(|| r.get("branch_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                // Gitea and Forgejo match rule names with `/` as a separator: `*` does
                // not cover `release/1.0`.
                name == branch || crate::doctor_settings::glob_matches(name, branch, true)
            };
            let exact = list.iter().find(|r| {
                r.get("rule_name")
                    .or_else(|| r.get("branch_name"))
                    .and_then(|v| v.as_str())
                    == Some(branch)
            });
            Some(exact.or_else(|| list.iter().find(matches)).cloned())
        }
        _ => None,
    };
    match rule {
        Some(None) => Ok(p), // readable, and no rule covers the branch
        Some(Some(r)) => {
            p.protected = true;
            let flag = |k: &str| r.get(k).and_then(|v| v.as_bool());
            if flag("enable_status_check") == Some(true) {
                for c in r
                    .get("status_check_contexts")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    if let Some(s) = c.as_str() {
                        p.required_contexts.insert(s.to_string());
                    }
                }
            }
            p.strict = flag("block_on_outdated_branch") == Some(true);
            // Forgejo has no force-push setting and always refuses force pushes to a
            // protected branch; Gitea 1.23+ can allow them (`enable_force_push`).
            p.force_push_blocked = flag("enable_force_push") != Some(true);
            // Neither forge deletes a protected branch.
            p.deletion_blocked = true;
            p.pull_request_required = flag("enable_push") == Some(false);
            p.signatures_required = flag("require_signed_commits") == Some(true);
            // Review rules. Gitea and Forgejo request reviews from CODEOWNERS on their
            // own; `block_on_official_review_requests` is what makes those requests
            // blocking, so it stands for a code-owner review requirement.
            // `dismiss_stale_approvals` is the last-push rule.
            p.required_approvals = r.get("required_approvals").and_then(|v| v.as_u64());
            p.code_owner_review |= flag("block_on_official_review_requests") == Some(true);
            p.dismiss_stale_reviews |= flag("dismiss_stale_approvals") == Some(true);
            // Gitea: `block_admin_merge_override`; Forgejo: `apply_to_admins`.
            let admins_bound = flag("block_admin_merge_override")
                .or_else(|| flag("apply_to_admins"))
                .unwrap_or(false);
            let mut bypass = Vec::new();
            if !admins_bound {
                bypass.push("repository administrators".to_string());
            }
            if flag("enable_merge_whitelist") == Some(true) {
                bypass.push("merge allowlist".to_string());
            }
            p.bypass = Some(bypass);
            p.protected_file_patterns = Some(
                r.get("protected_file_patterns")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
            Ok(p)
        }
        None => {
            // No admin access: fall back to the branch summary.
            let summary =
                get(api, forge, &format!("repos/{repo}/branches/{branch}")).map_err(|e| {
                    match &rules {
                        Err(re) => format!("{re}; {e}"),
                        _ => e,
                    }
                })?;
            p.protected = summary.get("protected").and_then(|v| v.as_bool()) == Some(true);
            if summary.get("enable_status_check").and_then(|v| v.as_bool()) == Some(true) {
                for c in summary
                    .get("status_check_contexts")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    if let Some(s) = c.as_str() {
                        p.required_contexts.insert(s.to_string());
                    }
                }
            }
            p.deletion_blocked = p.protected;
            p.pull_request_required =
                summary.get("user_can_push").and_then(|v| v.as_bool()) == Some(false);
            if forge.kind == ForgeKind::Forgejo {
                p.force_push_blocked = p.protected;
            } else {
                p.hidden.insert("force-push");
            }
            p.required_approvals = summary.get("required_approvals").and_then(|v| v.as_u64());
            p.hidden.extend([
                "up-to-date",
                "bypass",
                "code-owner-review",
                "last-push-approval",
                "workflow-protection",
            ]);
            p.bypass = Some(Vec::new());
            Ok(p)
        }
    }
}

/// Read the protection of `branch` on GitLab.
pub fn gitlab_protection(
    api: &dyn ForgeApi,
    forge: &Forge,
    branch: &str,
) -> Result<Protection, String> {
    let id = gitlab_project_id(&forge.repo);
    let project = get(api, forge, &format!("projects/{id}"))?;
    let mut p = Protection {
        pipeline_must_succeed: Some(
            project
                .get("only_allow_merge_if_pipeline_succeeds")
                .and_then(|v| v.as_bool()),
        ),
        thread_resolution_required: project
            .get("only_allow_merge_if_all_discussions_are_resolved")
            .and_then(|v| v.as_bool()),
        ..Protection::default()
    };
    match project.get("merge_method").and_then(|v| v.as_str()) {
        Some("ff") | Some("rebase_merge") => p.strict = true,
        Some(_) => {
            p.strict = project
                .get("merge_pipelines_enabled")
                .and_then(|v| v.as_bool())
                == Some(true)
        }
        None => {
            p.hidden.insert("up-to-date");
        }
    }
    let enc_branch = branch.replace('%', "%25").replace('/', "%2F");
    if let Some(pb) = api.get(
        forge,
        &format!("projects/{id}/protected_branches/{enc_branch}"),
    )? {
        p.protected = true;
        p.force_push_blocked = pb.get("allow_force_push").and_then(|v| v.as_bool()) != Some(true);
        p.deletion_blocked = true;
        p.pull_request_required = pb
            .get("push_access_levels")
            .and_then(|v| v.as_array())
            .is_some_and(|levels| {
                !levels.is_empty()
                    && levels
                        .iter()
                        .all(|l| l.get("access_level").and_then(|v| v.as_i64()) == Some(0))
            });
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::test_support::*;

    #[test]
    fn merge_methods_per_forge() {
        use crate::forge::CannedApi;
        let f = |kind: ForgeKind| Forge {
            kind,
            url: "https://x".into(),
            repo: "o/r".into(),
        };
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r".into(),
            serde_json::json!({"allow_squash_merge": true, "allow_rebase_merge": false}),
        );
        api.responses.insert(
            "gitea:repos/o/r".into(),
            serde_json::json!({"allow_squash_merge": false, "allow_rebase": false, "allow_rebase_explicit": false}),
        );
        api.responses.insert(
            "gitlab:projects/o%2Fr".into(),
            serde_json::json!({"squash_option": "never", "merge_method": "rebase_merge"}),
        );
        api.responses.insert(
            "forgejo:repos/o/r".into(),
            serde_json::json!({"default_branch": "main"}),
        );
        assert_eq!(
            merge_methods(&api, &f(ForgeKind::GitHub))
                .unwrap()
                .drops_pr_body(),
            Some(true)
        );
        assert_eq!(
            merge_methods(&api, &f(ForgeKind::Gitea))
                .unwrap()
                .drops_pr_body(),
            Some(false)
        );
        assert_eq!(
            merge_methods(&api, &f(ForgeKind::GitLab))
                .unwrap()
                .drops_pr_body(),
            Some(true)
        );
        assert_eq!(
            merge_methods(&api, &f(ForgeKind::Forgejo))
                .unwrap()
                .drops_pr_body(),
            None
        );
    }

    #[test]
    fn github_thread_resolution_is_read_from_rulesets_and_classic_protection() {
        let f = forge(ForgeKind::GitHub);
        // Neither: readable, and off.
        let none = github(full_rules(), serde_json::json!([]));
        assert_eq!(
            github_protection(&none, &f, "main")
                .unwrap()
                .thread_resolution_required,
            Some(false)
        );
        // A ruleset's pull_request rule.
        let mut rules = full_rules();
        rules[3]["parameters"]["required_review_thread_resolution"] = serde_json::json!(true);
        let ruleset = github(rules, serde_json::json!([]));
        assert_eq!(
            github_protection(&ruleset, &f, "main")
                .unwrap()
                .thread_resolution_required,
            Some(true)
        );
        // Classic protection.
        let mut classic = github(serde_json::json!([]), serde_json::json!([]));
        classic.responses.insert(
            "github:repos/o/r/branches/main".into(),
            serde_json::json!({"protected": true, "protection": {"enabled": true}}),
        );
        classic.responses.insert(
            "github:repos/o/r/branches/main/protection".into(),
            serde_json::json!({"required_conversation_resolution": {"enabled": true}}),
        );
        assert_eq!(
            github_protection(&classic, &f, "main")
                .unwrap()
                .thread_resolution_required,
            Some(true)
        );
    }
}
