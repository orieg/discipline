//! `discipline doctor`: is this repository set up so that discipline can block a merge?
//!
//! Gates judge a diff; they cannot see whether the platform enforces their verdict. A red
//! discipline run on a branch without a required check is advisory. This command reads
//! the two places that decide that and reports each finding with a remediation:
//!
//! - **Local files** (platform-neutral for Actions-style workflows): which workflow jobs
//!   run discipline, which jobs roll them up, the triggers and token permissions of those
//!   workflows, and whether `CODEOWNERS` covers the gate configuration.
//! - **Platform settings** (GitHub, GitLab, Gitea, Forgejo, over HTTPS through `crate::forge`;
//!   AGENTS.md §3.3): the effective branch rules and classic protection of the default
//!   branch — a required check that runs discipline, the up-to-date policy, force-push
//!   and deletion blocking, pull-request requirement and bypass.
//!
//! Exit status follows the gate contract: `0` nothing failed, `1` at least one finding
//! failed (with `--strict`, a warning also fails), `2` a check could not be decided
//! (no network, no access, a forge that cannot be identified). "Could not check" is never
//! reported as healthy.

mod agents;
mod codeowners;
mod gitlab_ci;
mod local_files;
mod protection;
mod protection_findings;
mod run;
#[cfg(test)]
mod test_support;
mod workflows;

pub use agents::*;
pub use codeowners::*;
pub use gitlab_ci::*;
pub use local_files::*;
pub use protection::*;
pub use protection_findings::*;
pub use run::*;
pub use workflows::*;

use crate::forge::{Forge, ForgeApi};
use serde::Serialize;
use std::path::Path;

/// Outcome of one doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    /// Worth fixing; fails only under `--strict`.
    Warn,
    Fail,
    /// Could not be decided.
    Unknown,
    /// Optional setting, reported for completeness; never affects the exit status.
    Info,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
            Status::Unknown => "unknown",
            Status::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub id: &'static str,
    pub status: Status,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl Finding {
    pub(crate) fn new(id: &'static str, status: Status, summary: impl Into<String>) -> Self {
        Self {
            id,
            status,
            summary: summary.into(),
            remediation: None,
        }
    }

    pub(crate) fn fix(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub platform: String,
    /// Web address of the forge the platform checks read, when one was identified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge_url: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub findings: Vec<Finding>,
}

impl Report {
    /// `0` healthy, `1` a failure (or a warning under `strict`), `2` undecided.
    pub fn exit_code(&self, strict: bool) -> u8 {
        let has = |s: Status| self.findings.iter().any(|f| f.status == s);
        let has_strict_warn = self
            .findings
            .iter()
            .any(|f| f.status == Status::Warn && f.id != "hook-mode");
        if has(Status::Unknown) {
            2
        } else if has(Status::Fail) || (strict && has_strict_warn) {
            1
        } else {
            0
        }
    }

    pub fn render_text(&self) -> String {
        // A summary quotes the forge (a branch, a check name, a refusal) and the
        // repository's workflows: each stays on its own line.
        let line = crate::report::text::terminal_line;
        let mut out = format!(
            "platform: {}{}   repository: {}   branch: {}\n\n",
            self.platform,
            self.forge_url
                .as_deref()
                .map(|u| format!(" ({})", line(u)))
                .unwrap_or_default(),
            line(self.repository.as_deref().unwrap_or("-")),
            line(self.branch.as_deref().unwrap_or("-"))
        );
        for f in &self.findings {
            out.push_str(&format!(
                "{:<8} {:<22} {}\n",
                f.status.label(),
                f.id,
                line(&f.summary)
            ));
            if let Some(r) = &f.remediation {
                out.push_str(&format!("{:<8} {:<22} fix: {}\n", "", "", line(r)));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------
// Local analysis
// ---------------------------------------------------------------------------------------

/// A workflow job that runs discipline, and the status contexts that report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisciplineJob {
    pub workflow: String,
    pub job_id: String,
    /// Check-run name of the job itself (`name:` or the job id).
    pub context: String,
    /// Check-run names of rollup jobs that fail when it fails: it is a direct `needs:`,
    /// the rollup runs even after a failure (`if: always()`, `!cancelled()`, `failure()`),
    /// and a step reads the `needs` results.
    pub rollups: Vec<String>,
    /// Jobs that depend on it but would be skipped, or pass, when it fails. A skipped
    /// required check counts as passed, so requiring one of these enforces nothing.
    pub weak_rollups: Vec<String>,
    /// Which branches a `push` event runs this job on: `None` when the workflow has no
    /// push trigger or the job / step is restricted to pull requests, `Some(vec![])` for
    /// every branch, `Some(branches)` for a filtered list (globs as written).
    pub push_branches: Option<Vec<String>>,
    /// Display names of the workflow (`name:` and the file name), which Gitea and Forgejo
    /// put in front of the job name in status contexts.
    pub workflow_names: Vec<String>,
    /// GitLab `allow_failure: true`: the job can fail without failing the pipeline.
    pub allow_failure: bool,
    /// The job's token is granted `pull-requests: read` or `write` (or `read-all` /
    /// `write-all`), by the job's `permissions:` or, when it has none, the workflow's.
    pub reads_pull_requests: bool,
}

/// What the local files say.
#[derive(Debug, Clone, Default)]
pub struct LocalFacts {
    pub jobs: Vec<DisciplineJob>,
    pub findings: Vec<Finding>,
}

/// Stable finding codes emitted by `discipline doctor`.
pub const DOCTOR_FINDINGS: &[&str] = &[
    "actions-approve-prs",
    "actions-sha-pinning",
    "agent-permission",
    "agent-sandbox",
    "allow-failure",
    "allowed-actions",
    "auto-merge",
    "bypass",
    "code-owner-review",
    "codeowners",
    "config",
    "copilot-trust",
    "default-token",
    "deletion",
    "dependency-alerts",
    "deploy-keys",
    "environment-reviewers",
    "force-push",
    "forge-token",
    "forking",
    "hook-mode",
    "immutable-releases",
    "last-push-approval",
    "leases",
    "mutation-testing",
    "non-blocking",
    "org-base-permission",
    "outside-collaborators",
    "platform",
    "pretool-hook",
    "pull-request",
    "push-trigger",
    "ref-guard",
    "required-check",
    "review",
    "secret-scanning",
    "secret-scoping",
    "security-policy",
    "signed-commits",
    "tag-protection",
    "test-report",
    "thread-resolution",
    "token",
    "trigger",
    "two-factor",
    "up-to-date",
    "visibility-change",
    "webhooks",
    "workflow-protection",
    "workflows",
];

fn get(api: &dyn ForgeApi, forge: &Forge, path: &str) -> Result<serde_json::Value, String> {
    api.get(forge, path)?
        .ok_or_else(|| format!("`{path}` was not found"))
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A branch name and a forge's reason that try to write the report.
    const BRANCH_WITH_A_LINE: &str = "main\nfail     branch-protection      forged";
    const SUMMARY_WITH_AN_ESCAPE: &str = "the forge said: no\r\u{1b}[2Jpass\u{7}";

    #[test]
    fn the_text_report_keeps_forge_text_on_its_line() {
        let mut finding =
            Finding::new("branch-protection", Status::Unknown, SUMMARY_WITH_AN_ESCAPE);
        finding.remediation = Some("set a token\nok       everything".into());
        let report = Report {
            platform: "gitea".into(),
            forge_url: Some("https://forge.example\u{1b}[1m".into()),
            repository: Some("o/r\nrepository: other".into()),
            branch: Some(BRANCH_WITH_A_LINE.into()),
            findings: vec![finding],
        };
        let text = report.render_text();
        // The header, a blank line, the finding and its fix: four lines and no more.
        assert_eq!(text.lines().count(), 4, "{text:?}");
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
        assert!(
            text.contains("the forge said: no\u{fffd}\u{fffd}[2Jpass\u{fffd}\n"),
            "{text:?}"
        );
        // A report with nothing to neutralise is written as before.
        let plain = Report {
            platform: "github".into(),
            forge_url: None,
            repository: Some("o/r".into()),
            branch: Some("main".into()),
            findings: vec![Finding::new("ci-gate", Status::Pass, "`ci` is required")],
        };
        assert_eq!(
            plain.render_text(),
            "platform: github   repository: o/r   branch: main\n\npass     ci-gate                `ci` is required\n"
        );
    }

    #[test]
    fn exit_code_follows_the_gate_contract() {
        let report = |s: &[Status]| Report {
            platform: "t".into(),
            forge_url: None,
            repository: None,
            branch: None,
            findings: s.iter().map(|&st| Finding::new("x", st, "")).collect(),
        };
        assert_eq!(report(&[Status::Pass, Status::Info]).exit_code(false), 0);
        assert_eq!(report(&[Status::Pass, Status::Warn]).exit_code(false), 0);
        assert_eq!(report(&[Status::Pass, Status::Warn]).exit_code(true), 1);
        assert_eq!(report(&[Status::Fail]).exit_code(false), 1);
        assert_eq!(report(&[Status::Fail, Status::Unknown]).exit_code(false), 2);
    }
}
