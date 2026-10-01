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
    /// The login that opened the pull request, when the run's context names it (an
    /// Actions event payload's `pull_request.user.login`, a merged pull request read
    /// from the forge). `None` on a GitLab merge-request pipeline, which names only the
    /// login that started the pipeline: [`PullContext::author_on`] reads it from the forge.
    pub author: Option<String>,
    /// The pull request's head commit. On a `pull_request` event `HEAD` is a merge
    /// commit, so this is read from the payload, never from the checkout.
    pub head_sha: String,
}

/// Read the pull-request fields an Actions-shaped event payload carries.
pub fn pull_context(event: &Value) -> Option<PullContext> {
    let pr = event.get("pull_request")?;
    Some(PullContext {
        number: pr.get("number")?.as_u64()?,
        author: Some(pr.get("user")?.get("login")?.as_str()?.to_string()),
        head_sha: pr.get("head")?.get("sha")?.as_str()?.to_string(),
    })
}

impl PullContext {
    /// The pull request's author: the one the context names, else the one the forge
    /// names ([`forge::pull_author`]). `Err` (tagged `forge`, exit 2) when the forge
    /// cannot say: a check that compares logins with the author never guesses it, and
    /// never takes the pipeline starter for it.
    pub fn author_on(&self, api: &dyn ForgeApi, forge: &Forge) -> Result<String> {
        match &self.author {
            Some(login) => Ok(login.clone()),
            None => forge::pull_author(api, forge, self.number).map_err(|e| {
                tag(
                    Reason::Forge,
                    anyhow!("cannot read the author of {}: {e}", self.describe(forge)),
                )
            }),
        }
    }

    /// `merge request !7` on GitLab, `pull request #7` elsewhere.
    pub fn describe(&self, forge: &Forge) -> String {
        match forge.kind {
            forge::ForgeKind::GitLab => format!("merge request !{}", self.number),
            _ => format!("pull request #{}", self.number),
        }
    }
}

/// Why this run's directive overrides or inline overrides are refused; empty when they stand.
///
/// `Err` means the policy could not be evaluated (no pull-request context, forge
/// unreachable): the caller exits 2, never passes.
pub fn judge(
    cfg: &DirectivesConfig,
    directive_overrides: usize,
    inline_overrides: usize,
    pull: Option<&PullContext>,
    forge: &dyn Fn() -> Result<Forge, String>,
    api: &dyn ForgeApi,
) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    if let Some(max) = cfg.max_inline_overrides {
        if inline_overrides > max {
            failures.push(format!(
                "{inline_overrides} inline override(s) applied; `directives.max_inline_overrides` allows {max}"
            ));
        }
    }
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
        let author = pull.author_on(api, &forge)?;
        if author.trim().is_empty() {
            // Nothing could tell the author's own approval apart from a reviewer's.
            return Err(tag(
                Reason::Forge,
                anyhow!(
                    "`directives.require_approval`: the author of {} is not known, so their own approval cannot be told apart",
                    pull.describe(&forge)
                ),
            ));
        }
        let approvers =
            forge::pull_approvers(api, &forge, pull.number, &pull.head_sha).map_err(|e| {
                tag(
                    Reason::Forge,
                    anyhow!("cannot read reviews of pull request #{}: {e}", pull.number),
                )
            })?;
        let approved = approvers.iter().any(|login| {
            !login.eq_ignore_ascii_case(&author)
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
            author: Some("agent".into()),
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
        assert!(judge(&cfg, 0, 0, None, &forge, &none).unwrap().is_empty());
        assert!(judge(&cfg, 1, 0, None, &forge, &none).unwrap().is_empty());
        let over = judge(&cfg, 2, 0, None, &forge, &none).unwrap();
        assert_eq!(over.len(), 1);
        assert!(over[0].contains("allows 1"), "{over:?}");
    }

    #[test]
    fn the_inline_budget_counts_inline_overrides() {
        let cfg = DirectivesConfig {
            max_inline_overrides: Some(1),
            ..Default::default()
        };
        let none = api(Value::Null);
        assert!(judge(&cfg, 0, 0, None, &forge, &none).unwrap().is_empty());
        assert!(judge(&cfg, 0, 1, None, &forge, &none).unwrap().is_empty());
        let over = judge(&cfg, 0, 2, None, &forge, &none).unwrap();
        assert_eq!(over.len(), 1);
        assert!(
            over[0].contains(
                "2 inline override(s) applied; `directives.max_inline_overrides` allows 1"
            ),
            "{over:?}"
        );
    }

    #[test]
    fn approval_must_come_from_a_listed_reviewer_who_is_not_the_author() {
        let cfg = DirectivesConfig {
            require_approval: true,
            allowed_override_actors: vec!["Lead".into(), "agent".into()],
            ..Default::default()
        };
        let p = pull();
        let ok = judge(&cfg, 1, 0, Some(&p), &forge, &api(approved_by("lead"))).unwrap();
        assert!(ok.is_empty(), "{ok:?}");
        for refused in ["agent", "stranger"] {
            let got = judge(&cfg, 1, 0, Some(&p), &forge, &api(approved_by(refused))).unwrap();
            assert_eq!(got.len(), 1, "{refused}: {got:?}");
        }
        let none = judge(&cfg, 1, 0, Some(&p), &forge, &api(serde_json::json!([]))).unwrap();
        assert_eq!(none.len(), 1);
    }

    /// A context that names no author (a GitLab merge-request pipeline) has it read from
    /// the forge: the author's approval is refused even though the context never named
    /// them, a reviewer's stands, and an author the forge cannot give is exit 2.
    #[test]
    fn an_author_the_context_does_not_name_is_read_from_the_forge() {
        let cfg = DirectivesConfig {
            require_approval: true,
            allowed_override_actors: vec!["lead".into(), "agent".into()],
            ..Default::default()
        };
        let unnamed = PullContext {
            author: None,
            ..pull()
        };
        let with_author = |reviews: Value, author: Value| {
            let mut a = api(reviews);
            a.responses.insert(
                "github:repos/o/r/pulls/7".into(),
                serde_json::json!({"user": author}),
            );
            a
        };
        let agent = serde_json::json!({"login": "agent"});
        let refused = judge(
            &cfg,
            1,
            0,
            Some(&unnamed),
            &forge,
            &with_author(approved_by("agent"), agent.clone()),
        )
        .unwrap();
        assert_eq!(refused.len(), 1, "{refused:?}");
        let stands = judge(
            &cfg,
            1,
            0,
            Some(&unnamed),
            &forge,
            &with_author(approved_by("lead"), agent),
        )
        .unwrap();
        assert!(stands.is_empty(), "{stands:?}");

        let reason = |r: Result<Vec<String>>| crate::could_not_check::classify(&r.unwrap_err()).0;
        // The forge does not answer for the pull request, or names no author.
        assert_eq!(
            reason(judge(
                &cfg,
                1,
                0,
                Some(&unnamed),
                &forge,
                &api(approved_by("lead"))
            )),
            Reason::Forge
        );
        assert_eq!(
            reason(judge(
                &cfg,
                1,
                0,
                Some(&unnamed),
                &forge,
                &with_author(approved_by("lead"), Value::Null)
            )),
            Reason::Forge
        );
        // A context whose author is empty cannot tell the author's approval apart.
        let empty = PullContext {
            author: Some(String::new()),
            ..pull()
        };
        assert_eq!(
            reason(judge(
                &cfg,
                1,
                0,
                Some(&empty),
                &forge,
                &api(approved_by("lead"))
            )),
            Reason::Forge
        );
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
            reason(judge(&cfg, 1, 0, None, &forge, &api(approved_by("lead")))),
            Reason::Configuration
        );
        // Forge cannot be reached.
        assert_eq!(
            reason(judge(&cfg, 1, 0, Some(&p), &forge, &CannedApi::default())),
            Reason::Forge
        );
        // Forge cannot be identified.
        let unknown = || Err("no remote".to_string());
        assert_eq!(
            reason(judge(
                &cfg,
                1,
                0,
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
                0,
                Some(&p),
                &forge,
                &api(approved_by("lead"))
            )),
            Reason::Configuration
        );
        // Without overrides there is nothing to approve and nothing to look up.
        assert!(judge(&cfg, 0, 0, None, &unknown, &CannedApi::default())
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
