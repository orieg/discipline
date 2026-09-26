//! Run-level limits on directive overrides.
//!
//! A directive in the PR body or a commit body is written by the author of the change it
//! excuses. `fail_on_overrides` refuses every one; the two options here sit between that
//! and accepting them all: a budget, and an approval read from the forge.

use crate::config::DirectivesConfig;
use crate::could_not_check::{tag, Reason};
use crate::forge::{self, Forge, ForgeApi};
use anyhow::{anyhow, Result};
use serde_json::Value;

/// The pull request a run is checking, as the forge's event payload describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullContext {
    pub number: u64,
    pub author: String,
    /// The pull request's head commit. On a `pull_request` event `HEAD` is a merge
    /// commit, so this is read from the payload, never from the checkout.
    pub head_sha: String,
}

/// Read the pull-request fields an Actions-shaped event payload carries.
pub fn pull_context(event: &Value) -> Option<PullContext> {
    let pr = event.get("pull_request")?;
    Some(PullContext {
        number: pr.get("number")?.as_u64()?,
        author: pr.get("user")?.get("login")?.as_str()?.to_string(),
        head_sha: pr.get("head")?.get("sha")?.as_str()?.to_string(),
    })
}

/// Why this run's directive overrides are refused; empty when they stand.
///
/// `Err` means the policy could not be evaluated (no pull-request context, forge
/// unreachable): the caller exits 2, never passes.
pub fn judge(
    cfg: &DirectivesConfig,
    directive_overrides: usize,
    pull: Option<&PullContext>,
    forge: &dyn Fn() -> Result<Forge, String>,
    api: &dyn ForgeApi,
) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    if directive_overrides == 0 {
        return Ok(failures);
    }
    if let Some(max) = cfg.max_overrides {
        if directive_overrides > max {
            failures.push(format!(
                "{directive_overrides} directive override(s) applied; `directives.max_overrides` allows {max}"
            ));
        }
    }
    if cfg.require_approval {
        if cfg.allowed_override_actors.is_empty() {
            return Err(tag(
                Reason::Configuration,
                anyhow!("`directives.require_approval` is set but `allowed_override_actors` names no reviewer"),
            ));
        }
        let pull = pull.ok_or_else(|| {
            tag(
                Reason::Configuration,
                anyhow!(
                    "`directives.require_approval` needs a pull-request event payload \
                     (GITHUB_EVENT_PATH or the Gitea / Forgejo equivalent); none was found"
                ),
            )
        })?;
        let forge =
            forge().map_err(|e| tag(Reason::Forge, anyhow!("cannot identify the forge: {e}")))?;
        let approvers =
            forge::pull_approvers(api, &forge, pull.number, &pull.head_sha).map_err(|e| {
                tag(
                    Reason::Forge,
                    anyhow!("cannot read reviews of pull request #{}: {e}", pull.number),
                )
            })?;
        let approved = approvers.iter().any(|login| {
            !login.eq_ignore_ascii_case(&pull.author)
                && cfg
                    .allowed_override_actors
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(login))
        });
        if !approved {
            failures.push(format!(
                "{directive_overrides} directive override(s) await an approving review of {} by one of: {} \
                 (the author's own approval does not count)",
                &pull.head_sha[..pull.head_sha.len().min(10)],
                cfg.allowed_override_actors.join(", ")
            ));
        }
    }
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{CannedApi, ForgeKind};

    fn forge() -> Result<Forge, String> {
        Ok(Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        })
    }

    fn api(reviews: Value) -> CannedApi {
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/pulls/7/reviews?per_page=100".into(),
            reviews,
        );
        api
    }

    fn pull() -> PullContext {
        PullContext {
            number: 7,
            author: "agent".into(),
            head_sha: "abc123".into(),
        }
    }

    fn approved_by(login: &str) -> Value {
        serde_json::json!([{"user": {"login": login}, "state": "APPROVED", "commit_id": "abc123"}])
    }

    #[test]
    fn the_budget_counts_overrides_and_ignores_a_run_without_any() {
        let cfg = DirectivesConfig {
            max_overrides: Some(1),
            ..Default::default()
        };
        let none = api(Value::Null);
        assert!(judge(&cfg, 0, None, &forge, &none).unwrap().is_empty());
        assert!(judge(&cfg, 1, None, &forge, &none).unwrap().is_empty());
        let over = judge(&cfg, 2, None, &forge, &none).unwrap();
        assert_eq!(over.len(), 1);
        assert!(over[0].contains("allows 1"), "{over:?}");
    }

    #[test]
    fn approval_must_come_from_a_listed_reviewer_who_is_not_the_author() {
        let cfg = DirectivesConfig {
            require_approval: true,
            allowed_override_actors: vec!["Lead".into(), "agent".into()],
            ..Default::default()
        };
        let p = pull();
        let ok = judge(&cfg, 1, Some(&p), &forge, &api(approved_by("lead"))).unwrap();
        assert!(ok.is_empty(), "{ok:?}");
        for refused in ["agent", "stranger"] {
            let got = judge(&cfg, 1, Some(&p), &forge, &api(approved_by(refused))).unwrap();
            assert_eq!(got.len(), 1, "{refused}: {got:?}");
        }
        let none = judge(&cfg, 1, Some(&p), &forge, &api(serde_json::json!([]))).unwrap();
        assert_eq!(none.len(), 1);
    }

    #[test]
    fn an_unevaluable_approval_policy_is_an_error_never_a_pass() {
        let cfg = DirectivesConfig {
            require_approval: true,
            allowed_override_actors: vec!["lead".into()],
            ..Default::default()
        };
        let p = pull();
        let reason = |r: Result<Vec<String>>| crate::could_not_check::classify(&r.unwrap_err()).0;
        use crate::could_not_check::Reason;
        // No pull-request payload.
        assert_eq!(
            reason(judge(&cfg, 1, None, &forge, &api(approved_by("lead")))),
            Reason::Configuration
        );
        // Forge cannot be reached.
        assert_eq!(
            reason(judge(&cfg, 1, Some(&p), &forge, &CannedApi::default())),
            Reason::Forge
        );
        // Forge cannot be identified.
        let unknown = || Err("no remote".to_string());
        assert_eq!(
            reason(judge(
                &cfg,
                1,
                Some(&p),
                &unknown,
                &api(approved_by("lead"))
            )),
            Reason::Forge
        );
        // Nobody could ever approve.
        let empty = DirectivesConfig {
            require_approval: true,
            ..Default::default()
        };
        assert_eq!(
            reason(judge(
                &empty,
                1,
                Some(&p),
                &forge,
                &api(approved_by("lead"))
            )),
            Reason::Configuration
        );
        // Without overrides there is nothing to approve and nothing to look up.
        assert!(judge(&cfg, 0, None, &unknown, &CannedApi::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pull_context_reads_the_event_head_not_the_checkout() {
        let event = serde_json::json!({"pull_request": {
            "number": 7, "user": {"login": "agent"}, "head": {"sha": "abc123"}}});
        assert_eq!(pull_context(&event), Some(pull()));
        assert_eq!(pull_context(&serde_json::json!({"push": {}})), None);
    }
}
