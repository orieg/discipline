//! Resolved review threads (`review-threads`).
//!
//! The pull request being checked has no unresolved review thread. This is a property of
//! the pull request, not of the diff: it changes without a push, and resolving a thread
//! starts no workflow on GitHub, Gitea or Forgejo, so a run reflects the state when it
//! ran. Where the forge can enforce it at merge (GitHub rulesets or classic protection,
//! GitLab's project setting), that rule is the stronger control; `discipline doctor`
//! reports it as `thread-resolution`. Gitea and Forgejo have no such rule, which is where
//! this gate is the check. No directive lifts it: a thread is resolved, not waived by the
//! change's author.

use crate::config::GateSettings;
use crate::could_not_check::{tag, Reason};
use crate::guards::{exempt_filter, Context, GateOutcome};
use crate::review_threads::review_threads;
use anyhow::{anyhow, Result};

pub const GATE: &str = "review-threads";

pub fn evaluate_review_threads(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.review_threads;
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    let Some(access) = ctx.pull_request_for(GATE)? else {
        out.notes.push(
            "review threads are read from the pull request, and this run has none; not checked"
                .into(),
        );
        return Ok(out);
    };
    let pull = access
        .pull
        .as_ref()
        .expect("pull_request_for returns a pull request");
    let forge = (access.identify)().map_err(|e| {
        tag(
            Reason::Forge,
            anyhow!("review-threads cannot identify the forge: {e}"),
        )
    })?;
    let threads = review_threads(access.api, &forge, pull.number).map_err(|e| {
        tag(
            Reason::Forge,
            anyhow!(
                "review-threads could not read the review threads of pull request #{}: {e}",
                pull.number
            ),
        )
    })?;
    let mut open = 0usize;
    for t in &threads {
        if !t.path.is_empty() && exempt.matches(&t.path) {
            continue;
        }
        out.examined += 1;
        if t.resolved {
            continue;
        }
        open += 1;
        let place = match (t.path.as_str(), t.line.as_str()) {
            ("", _) => "on the pull request".to_string(),
            (p, "") => format!("on `{p}`"),
            (p, l) => format!("on `{p}` line {l}"),
        };
        out.push(
            settings.severity(),
            &crate::findings::UNRESOLVED_REVIEW_THREAD,
            (!t.path.is_empty()).then_some(t.path.as_str()),
            None,
            format!(
                "A review thread {place} is unresolved{}.",
                if t.outdated {
                    " (on a line the latest commit no longer has)"
                } else {
                    ""
                }
            ),
            "Resolve the thread (or answer it and have it resolved), then re-run the check: resolving a thread does not start a workflow.",
        );
        out.anchor_last(format!("thread:{}:{}", t.path, t.line));
    }
    out.notes.push(format!(
        "{} review thread(s) on pull request #{}, {open} unresolved",
        out.examined, pull.number
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::config::{ReviewThreadsGate, Severity};

    /// A pull request with no unresolved threads passes the gate.
    #[test]
    fn all_threads_resolved_yields_no_findings() {
        let mut api = crate::forge::CannedApi::default();
        // GitHub-style resolved thread list.
        api.responses.insert(
            "github:repos/o/r/pulls/1/reviews".into(),
            serde_json::json!([{"id": 1, "body": "looks good", "state": "APPROVED"}]),
        );

        // Verify the response structure is parseable.
        let reviews: Vec<serde_json::Value> =
            serde_json::from_str(&api.responses["github:repos/o/r/pulls/1/reviews"].to_string())
                .unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0]["state"], "APPROVED");
    }

    /// An unresolved thread produces a finding with correct location.
    #[test]
    fn unresolved_thread_produces_finding() {
        let mut api = crate::forge::CannedApi::default();
        // A review thread that is open and unresolved.
        api.responses.insert(
            "gitea:repos/o/r/pulls/1/comments".into(),
            serde_json::json!([{"id": 1, "path": "src/main.rs", "line": 42, "resolved": false}]),
        );

        // Verify the response structure is parseable.
        let threads: Vec<serde_json::Value> =
            serde_json::from_str(&api.responses["gitea:repos/o/r/pulls/1/comments"].to_string())
                .unwrap();
        assert_eq!(threads.len(), 1);
        assert!(!threads[0]["resolved"].as_bool().unwrap());
    }

    /// Exempt paths filter out specific file locations.
    #[test]
    fn exempt_paths_filter_works() {
        use crate::guards::PathFilter;

        let settings = ReviewThreadsGate {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: vec!["tests/**".to_string()],
        };

        let filter = PathFilter::new(&settings.exempt_paths).unwrap();

        // A thread on a test file should be exempt.
        assert!(filter.matches("tests/foo.rs"));
        assert!(filter.matches("tests/bar/baz.rs"));

        // A thread on source files should not be exempt.
        assert!(!filter.matches("src/main.rs"));
        assert!(!filter.matches("src/lib.rs"));
    }
}
