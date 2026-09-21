//! `discipline doctor`: is this repository set up so that discipline can block a merge?
//!
//! Gates judge a diff; they cannot see whether the platform enforces their verdict. A red
//! discipline run on a branch without a required check is advisory. This command reads
//! the two places that decide that and reports each finding with a remediation:
//!
//! - **Local files** (platform-neutral for Actions-style workflows): which workflow jobs
//!   run discipline, which jobs roll them up, the triggers and token permissions of those
//!   workflows, and whether `CODEOWNERS` covers the gate configuration.
//! - **Platform settings** (GitHub, through `gh api`; the binary has no network stack,
//!   AGENTS.md §3.3): the effective branch rules and classic protection of the default
//!   branch — a required check that runs discipline, the up-to-date policy, force-push
//!   and deletion blocking, pull-request requirement and bypass.
//!
//! Exit status follows the gate contract: `0` nothing failed, `1` at least one finding
//! failed (with `--strict`, a warning also fails), `2` a check could not be decided
//! (`gh` missing or unauthenticated, an unsupported platform). "Could not check" is never
//! reported as healthy.

use crate::guards::perf::citation::CitationInstruments;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
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
    fn new(id: &'static str, status: Status, summary: impl Into<String>) -> Self {
        Self {
            id,
            status,
            summary: summary.into(),
            remediation: None,
        }
    }

    fn fix(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub platform: String,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub findings: Vec<Finding>,
}

impl Report {
    /// `0` healthy, `1` a failure (or a warning under `strict`), `2` undecided.
    pub fn exit_code(&self, strict: bool) -> u8 {
        let has = |s: Status| self.findings.iter().any(|f| f.status == s);
        if has(Status::Unknown) {
            2
        } else if has(Status::Fail) || (strict && has(Status::Warn)) {
            1
        } else {
            0
        }
    }

    pub fn render_text(&self) -> String {
        let mut out = format!(
            "platform: {}   repository: {}   branch: {}\n\n",
            self.platform,
            self.repository.as_deref().unwrap_or("-"),
            self.branch.as_deref().unwrap_or("-")
        );
        for f in &self.findings {
            out.push_str(&format!(
                "{:<8} {:<22} {}\n",
                f.status.label(),
                f.id,
                f.summary
            ));
            if let Some(r) = &f.remediation {
                out.push_str(&format!("{:<8} {:<22} fix: {r}\n", "", ""));
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
    /// Check-run names of jobs that transitively `needs:` it (rollups).
    pub rollups: Vec<String>,
}

/// What the local files say.
#[derive(Debug, Clone, Default)]
pub struct LocalFacts {
    pub jobs: Vec<DisciplineJob>,
    pub findings: Vec<Finding>,
}

/// Workflow directories read for each Actions-style platform.
pub const WORKFLOW_DIRS: &[&str] = &[
    ".github/workflows",
    ".gitea/workflows",
    ".forgejo/workflows",
];

/// `CODEOWNERS` locations, in the order GitHub resolves them.
pub const CODEOWNERS_PATHS: &[&str] = &[".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

/// Whether a workflow step runs discipline.
fn step_runs_discipline(step: &serde_yaml::Value, self_action: bool) -> bool {
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        let uses = uses.trim();
        if uses.starts_with("orieg/discipline@")
            || uses.starts_with("docker://ghcr.io/orieg/discipline")
            || (self_action && (uses == "./" || uses == "."))
        {
            return true;
        }
    }
    step.get("run").and_then(|r| r.as_str()).is_some_and(|run| {
        run.lines().any(|l| {
            let l = l.trim_start();
            !l.starts_with('#') && (l.contains("discipline check") || l.contains("discipline diff"))
        })
    })
}

fn job_context(job_id: &str, job: &serde_yaml::Value) -> String {
    job.get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.contains("${{"))
        .unwrap_or(job_id)
        .to_string()
}

fn job_needs(job: &serde_yaml::Value) -> Vec<String> {
    match job.get("needs") {
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// `on:` of a workflow as a map from event name to its configuration.
fn triggers(wf: &serde_yaml::Value) -> BTreeMap<String, serde_yaml::Value> {
    // YAML 1.1 reads a bare `on` key as boolean true.
    let on = wf
        .get("on")
        .or_else(|| {
            wf.as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::Bool(true)))
        })
        .cloned()
        .unwrap_or(serde_yaml::Value::Null);
    let mut out = BTreeMap::new();
    match on {
        serde_yaml::Value::String(s) => {
            out.insert(s, serde_yaml::Value::Null);
        }
        serde_yaml::Value::Sequence(seq) => {
            for v in seq {
                if let Some(s) = v.as_str() {
                    out.insert(s.to_string(), serde_yaml::Value::Null);
                }
            }
        }
        serde_yaml::Value::Mapping(m) => {
            for (k, v) in m {
                if let Some(s) = k.as_str() {
                    out.insert(s.to_string(), v);
                }
            }
        }
        _ => {}
    }
    out
}

/// Token permission of a job: 0 none, 1 read-only, 2 write (or unset, which inherits the
/// repository default and may be write).
fn permission_level(value: Option<&serde_yaml::Value>) -> Option<u8> {
    let v = value?;
    Some(match v {
        serde_yaml::Value::String(s) if s == "write-all" => 2,
        serde_yaml::Value::String(s) if s == "read-all" => 1,
        serde_yaml::Value::Mapping(m) if m.is_empty() => 0,
        serde_yaml::Value::Mapping(m) => {
            if m.values().any(|v| v.as_str() == Some("write")) {
                2
            } else {
                1
            }
        }
        _ => 2,
    })
}

/// Analyse workflow files: `(path, content)` pairs. `self_action` is true when the
/// repository is discipline itself (a `uses: ./` step runs it).
pub fn analyse_workflows(files: &[(String, String)], self_action: bool) -> LocalFacts {
    let mut facts = LocalFacts::default();
    let mut unparsable = Vec::new();
    let mut pr_workflows = 0usize;
    for (path, content) in files {
        let wf: serde_yaml::Value = match serde_yaml::from_str(content) {
            Ok(v) => v,
            Err(_) => {
                unparsable.push(path.clone());
                continue;
            }
        };
        let Some(jobs) = wf.get("jobs").and_then(|j| j.as_mapping()) else {
            continue;
        };
        let jobs: Vec<(String, &serde_yaml::Value)> = jobs
            .iter()
            .filter_map(|(k, v)| k.as_str().map(|k| (k.to_string(), v)))
            .collect();
        let runs: BTreeSet<String> = jobs
            .iter()
            .filter(|(_, job)| {
                job.get("steps")
                    .and_then(|s| s.as_sequence())
                    .is_some_and(|steps| steps.iter().any(|s| step_runs_discipline(s, self_action)))
            })
            .map(|(id, _)| id.clone())
            .collect();
        if runs.is_empty() {
            continue;
        }

        // Transitive dependents of each discipline job.
        let needs: BTreeMap<&str, Vec<String>> = jobs
            .iter()
            .map(|(id, job)| (id.as_str(), job_needs(job)))
            .collect();
        let depends_on = |from: &str, target: &str| -> bool {
            let mut stack = vec![from.to_string()];
            let mut seen = BTreeSet::new();
            while let Some(j) = stack.pop() {
                if !seen.insert(j.clone()) {
                    continue;
                }
                for n in needs.get(j.as_str()).into_iter().flatten() {
                    if n == target {
                        return true;
                    }
                    stack.push(n.clone());
                }
            }
            false
        };

        let events = triggers(&wf);
        let wf_perm = permission_level(wf.get("permissions"));
        // Worst token level across this workflow's discipline jobs (None = inherited).
        let mut worst: Option<Option<u8>> = None;
        for (id, job) in &jobs {
            if !runs.contains(id) {
                continue;
            }
            let rollups = jobs
                .iter()
                .filter(|(other, _)| other != id && depends_on(other, id))
                .map(|(other, j)| job_context(other, j))
                .collect();
            facts.jobs.push(DisciplineJob {
                workflow: path.clone(),
                job_id: id.clone(),
                context: job_context(id, job),
                rollups,
            });
            let level = permission_level(job.get("permissions")).or(wf_perm);
            let rank = |l: Option<u8>| l.unwrap_or(3);
            if worst.is_none_or(|w| rank(level) > rank(w)) {
                worst = Some(level);
            }
        }

        match events.get("pull_request") {
            Some(cfg) => {
                pr_workflows += 1;
                let types: Vec<&str> = cfg
                    .get("types")
                    .and_then(|t| t.as_sequence())
                    .map(|s| s.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                facts.findings.push(if types.contains(&"edited") {
                    Finding::new(
                        "trigger",
                        Status::Pass,
                        format!("{path} runs on pull requests, including description edits"),
                    )
                } else {
                    Finding::new(
                        "trigger",
                        Status::Warn,
                        format!("{path} does not re-run when a PR description changes"),
                    )
                    .fix("Use `pull_request: types: [opened, synchronize, reopened, edited]` so adding or removing a directive re-runs the gate.")
                });
            }
            None if events.contains_key("pull_request_target") => {
                pr_workflows += 1;
                facts.findings.push(
                    Finding::new(
                        "trigger",
                        Status::Warn,
                        format!("{path} runs discipline on `pull_request_target`, which executes with a write token"),
                    )
                    .fix("Run discipline on `pull_request`; it needs only `contents: read`."),
                );
            }
            None => {}
        }

        if let Some(level) = worst {
            facts.findings.push(match level {
                Some(0 | 1) => Finding::new(
                    "token",
                    Status::Pass,
                    format!("{path}: discipline jobs run with a read-only token"),
                ),
                _ => Finding::new(
                    "token",
                    Status::Warn,
                    format!(
                        "{path}: {}",
                        if level.is_none() {
                            "discipline jobs inherit the repository's default token permissions"
                        } else {
                            "a discipline job runs with a write token"
                        }
                    ),
                )
                .fix("Set `permissions: contents: read` on the workflow or job."),
            });
        }
    }
    if !facts.jobs.is_empty() && pr_workflows == 0 {
        facts.findings.push(
            Finding::new(
                "trigger",
                Status::Warn,
                "no workflow runs discipline on pull requests",
            )
            .fix("Add a `pull_request` trigger so the gate runs before merge."),
        );
    }
    for path in unparsable {
        facts.findings.push(
            Finding::new(
                "workflows",
                Status::Unknown,
                format!("{path} is not valid YAML"),
            )
            .fix("Fix the workflow file; doctor cannot tell whether it runs discipline."),
        );
    }
    if facts.jobs.is_empty() {
        facts.findings.insert(
            0,
            Finding::new("workflows", Status::Fail, "no workflow job runs discipline").fix(
                "Add a job with `uses: orieg/discipline@v0` (see docs/guides/ci-platforms.md).",
            ),
        );
    } else {
        let names: Vec<String> = facts
            .jobs
            .iter()
            .map(|j| format!("{} ({})", j.context, j.workflow))
            .collect();
        facts.findings.insert(
            0,
            Finding::new(
                "workflows",
                Status::Pass,
                format!("discipline runs in: {}", names.join(", ")),
            ),
        );
    }
    facts
}

/// One `CODEOWNERS` rule.
#[derive(Debug, Clone)]
struct OwnerRule {
    matcher: globset::GlobMatcher,
    owned: bool,
}

fn owner_rules(content: &str) -> Vec<OwnerRule> {
    let mut rules = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next() else {
            continue;
        };
        let owned = parts.next().is_some_and(|o| !o.starts_with('#'));
        // gitignore-style: a leading `/` anchors; a pattern without an inner `/` matches
        // at any depth; a trailing `/` (or a directory) owns everything below it.
        let anchored = pattern.starts_with('/');
        let trimmed = pattern.trim_start_matches('/');
        let dir = trimmed.ends_with('/');
        let body = trimmed.trim_end_matches('/');
        let mut glob = if anchored || body.contains('/') {
            body.to_string()
        } else {
            format!("**/{body}")
        };
        if dir || !body.contains('*') {
            // A literal may name a directory; own it and everything under it.
            glob = format!("{{{glob},{glob}/**}}");
        }
        if let Ok(g) = globset::GlobBuilder::new(&glob)
            .literal_separator(true)
            .build()
        {
            rules.push(OwnerRule {
                matcher: g.compile_matcher(),
                owned,
            });
        }
    }
    rules
}

/// Whether `path` has an owner under `CODEOWNERS` content (the last matching rule wins).
pub fn codeowners_covers(content: &str, path: &str) -> bool {
    owner_rules(content)
        .iter()
        .rev()
        .find(|r| r.matcher.is_match(path))
        .is_some_and(|r| r.owned)
}

/// Check `CODEOWNERS` against the files that configure the gate.
pub fn codeowners_finding(codeowners: Option<(&str, &str)>, targets: &[String]) -> Finding {
    let Some((path, content)) = codeowners else {
        return Finding::new("codeowners", Status::Warn, "no CODEOWNERS file").fix(format!(
            "Add .github/CODEOWNERS owning {} so weakening them needs a named reviewer.",
            targets.join(", ")
        ));
    };
    let missing: Vec<&String> = targets
        .iter()
        .filter(|t| !codeowners_covers(content, t))
        .collect();
    if missing.is_empty() {
        Finding::new(
            "codeowners",
            Status::Pass,
            format!("{path} owns {}", targets.join(", ")),
        )
    } else {
        Finding::new(
            "codeowners",
            Status::Warn,
            format!(
                "{path} has no owner for {}",
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .fix("Add CODEOWNERS rules for the gate configuration and workflow files.")
    }
}

// ---------------------------------------------------------------------------------------
// Platform: GitHub
// ---------------------------------------------------------------------------------------

/// Effective protection of one branch, merged from rulesets and classic protection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Protection {
    pub required_contexts: BTreeSet<String>,
    pub strict: bool,
    pub force_push_blocked: bool,
    pub deletion_blocked: bool,
    pub pull_request_required: bool,
    pub signatures_required: bool,
    /// Bypass actors per ruleset (`None` when not visible to this token).
    pub bypass: Option<Vec<String>>,
    /// Whether classic protection also binds administrators (`None` when unknown or unused).
    pub classic_enforce_admins: Option<bool>,
}

/// Read the effective protection of `branch` on GitHub. `Err` names what could not be read.
pub fn github_protection(
    gh: &dyn CitationInstruments,
    repo: &str,
    branch: &str,
) -> Result<Protection, String> {
    let mut p = Protection::default();
    let rules = gh.gh_api(&format!("repos/{repo}/rules/branches/{branch}"))?;
    let rules = rules
        .as_array()
        .ok_or("rules endpoint did not return a list")?;
    let mut ruleset_ids = BTreeSet::new();
    for rule in rules {
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
            "pull_request" => p.pull_request_required = true,
            "required_signatures" => p.signatures_required = true,
            _ => {}
        }
    }

    let mut bypass = Some(Vec::new());
    for id in &ruleset_ids {
        match gh.gh_api(&format!("repos/{repo}/rulesets/{id}")) {
            Ok(rs) => match rs.get("bypass_actors").and_then(|b| b.as_array()) {
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
            Err(_) => bypass = None,
        }
    }
    p.bypass = bypass;

    // Classic branch protection, when enabled, adds to the rulesets.
    let summary = gh.gh_api(&format!("repos/{repo}/branches/{branch}"))?;
    let classic_on = summary
        .get("protection")
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        == Some(true);
    if classic_on {
        match gh.gh_api(&format!("repos/{repo}/branches/{branch}/protection")) {
            Ok(full) => {
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
                if full.get("required_pull_request_reviews").is_some() {
                    p.pull_request_required = true;
                }
                if enabled("required_signatures") == Some(true) {
                    p.signatures_required = true;
                }
                p.classic_enforce_admins = enabled("enforce_admins");
            }
            Err(e) => {
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
                return Err(format!(
                    "classic branch protection is on but its settings are not readable ({e}); an admin token is needed"
                ));
            }
        }
    }
    Ok(p)
}

/// Findings for a branch's protection against the jobs that run discipline.
pub fn protection_findings(p: &Protection, jobs: &[DisciplineJob]) -> Vec<Finding> {
    let mut out = Vec::new();
    let candidates: BTreeSet<&str> = jobs
        .iter()
        .flat_map(|j| {
            std::iter::once(j.context.as_str()).chain(j.rollups.iter().map(|s| s.as_str()))
        })
        .collect();
    let enforcing: Vec<&String> = p
        .required_contexts
        .iter()
        .filter(|c| candidates.contains(c.as_str()))
        .collect();
    if p.required_contexts.is_empty() {
        out.push(
            Finding::new(
                "required-check",
                Status::Fail,
                "no status check is required before merge",
            )
            .fix("Require the rollup job (or the discipline job) as a status check on the branch."),
        );
    } else if enforcing.is_empty() {
        let expected: Vec<&str> = candidates.iter().copied().collect();
        out.push(
            Finding::new(
                "required-check",
                Status::Fail,
                format!(
                    "required checks ({}) do not include a job that runs discipline",
                    p.required_contexts
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .fix(if expected.is_empty() {
                "Add a discipline job, then require it or a rollup that needs it.".to_string()
            } else {
                format!("Require one of: {}.", expected.join(", "))
            }),
        );
    } else {
        out.push(Finding::new(
            "required-check",
            Status::Pass,
            format!(
                "merge requires {}, which runs discipline",
                enforcing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    out.push(if p.strict {
        Finding::new(
            "up-to-date",
            Status::Pass,
            "branches must be up to date before merge",
        )
    } else {
        Finding::new(
            "up-to-date",
            Status::Warn,
            "a branch can merge without being up to date",
        )
        .fix("Enable the up-to-date (strict) policy so the gate judges the combined result.")
    });
    out.push(if p.force_push_blocked {
        Finding::new("force-push", Status::Pass, "force pushes are blocked")
    } else {
        Finding::new("force-push", Status::Fail, "force pushes are allowed").fix(
            "Block force pushes (ruleset `non_fast_forward`); a rewritten base voids the ratchets.",
        )
    });
    out.push(if p.deletion_blocked {
        Finding::new("deletion", Status::Pass, "branch deletion is blocked")
    } else {
        Finding::new("deletion", Status::Warn, "the branch can be deleted")
            .fix("Block deletion (ruleset `deletion`).")
    });
    out.push(if p.pull_request_required {
        Finding::new("pull-request", Status::Pass, "changes must arrive through a pull request")
    } else {
        Finding::new(
            "pull-request",
            Status::Warn,
            "direct pushes are allowed when the required checks pass",
        )
        .fix("Require pull requests (ruleset `pull_request`); directives are read from the PR description.")
    });
    match (&p.bypass, p.classic_enforce_admins) {
        (None, _) => out.push(
            Finding::new(
                "bypass",
                Status::Warn,
                "ruleset bypass list is not visible to this token",
            )
            .fix("Re-run with an admin token to confirm nobody can bypass the rules."),
        ),
        (Some(list), _) if !list.is_empty() => out.push(
            Finding::new(
                "bypass",
                Status::Warn,
                format!("rules can be bypassed by: {}", list.join(", ")),
            )
            .fix("Empty the bypass list; fix a stuck check instead of overriding it."),
        ),
        (Some(_), Some(false)) => out.push(
            Finding::new(
                "bypass",
                Status::Warn,
                "classic protection does not apply to administrators",
            )
            .fix("Enable `enforce_admins` or move the rules into a ruleset without bypass."),
        ),
        (Some(_), _) => out.push(Finding::new("bypass", Status::Pass, "no bypass actors")),
    }
    out.push(Finding::new(
        "signed-commits",
        Status::Info,
        if p.signatures_required {
            "signed commits are required"
        } else {
            "signed commits are not required (optional)"
        },
    ));
    out
}

// ---------------------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------------------

/// Which forge hosts the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    GitHub,
    Other,
}

/// Classify a remote URL.
pub fn detect_platform(remote_url: Option<&str>) -> Platform {
    match remote_url {
        Some(u) if u.contains("github.com") => Platform::GitHub,
        _ => Platform::Other,
    }
}

/// Inputs to one doctor run, gathered by the caller so the logic is testable offline.
pub struct DoctorInput<'a> {
    pub root: &'a Path,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub platform: Platform,
    pub local_only: bool,
    pub gh: &'a dyn CitationInstruments,
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

/// Run every check.
pub fn run(input: &DoctorInput) -> Report {
    let root = input.root;
    let mut files = Vec::new();
    for dir in WORKFLOW_DIRS {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut names: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
            .collect();
        names.sort();
        for n in names {
            let rel = format!("{dir}/{n}");
            if let Some(content) = read(root, &rel) {
                files.push((rel, content));
            }
        }
    }
    let self_action = read(root, "action.yml").is_some_and(|a| {
        a.lines()
            .any(|l| l.trim_start().starts_with("name:") && l.contains("Discipline"))
    });
    let local = analyse_workflows(&files, self_action);
    let mut findings = local.findings.clone();

    let mut targets = vec!["discipline.toml".to_string()];
    if root.join("discipline-baseline.toml").exists() {
        targets.push("discipline-baseline.toml".to_string());
    }
    let mut wf_targets: Vec<String> = local.jobs.iter().map(|j| j.workflow.clone()).collect();
    wf_targets.dedup();
    targets.extend(wf_targets);
    let codeowners = CODEOWNERS_PATHS
        .iter()
        .find_map(|p| read(root, p).map(|c| (p.to_string(), c)));
    findings.push(codeowners_finding(
        codeowners.as_ref().map(|(p, c)| (p.as_str(), c.as_str())),
        &targets,
    ));
    findings.push(if root.join("discipline.toml").exists() {
        Finding::new("config", Status::Pass, "discipline.toml is present")
    } else {
        Finding::new(
            "config",
            Status::Info,
            "no discipline.toml: the built-in defaults apply",
        )
        .fix("Run `discipline init` to pin the configuration in the repository.")
    });

    let mut platform_name = "local".to_string();
    let mut branch = input.branch.clone();
    if !input.local_only {
        match (input.platform, &input.repository) {
            (Platform::GitHub, Some(repo)) => {
                platform_name = "github".to_string();
                if branch.is_none() {
                    branch = input
                        .gh
                        .gh_api(&format!("repos/{repo}"))
                        .ok()
                        .and_then(|v| v.get("default_branch")?.as_str().map(str::to_string));
                }
                match &branch {
                    None => findings.push(
                        Finding::new(
                            "platform",
                            Status::Unknown,
                            format!("could not read the default branch of {repo} with `gh`"),
                        )
                        .fix("Install and authenticate `gh` (or set DISCIPLINE_GH), or pass --branch; --local-only skips platform checks."),
                    ),
                    Some(b) => match github_protection(input.gh, repo, b) {
                        Ok(p) => findings.extend(protection_findings(&p, &local.jobs)),
                        Err(e) => findings.push(
                            Finding::new(
                                "platform",
                                Status::Unknown,
                                format!("could not read the protection of `{b}`: {e}"),
                            )
                            .fix("Authenticate `gh` with access to the repository, or use --local-only."),
                        ),
                    },
                }
            }
            (Platform::GitHub, None) => findings.push(
                Finding::new("platform", Status::Unknown, "no GitHub repository to query")
                    .fix("Pass --repo OWNER/NAME or set an `origin` remote on github.com."),
            ),
            (Platform::Other, _) => {
                platform_name = "unsupported".to_string();
                findings.push(
                    Finding::new(
                        "platform",
                        Status::Unknown,
                        "platform checks support GitHub only; this remote is not on github.com",
                    )
                    .fix("Use --local-only, and check the protected-branch settings by hand (docs/guides/ci-platforms.md#8-repository-protection)."),
                );
            }
        }
    }

    Report {
        platform: platform_name,
        repository: input.repository.clone(),
        branch,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::perf::citation::{CannedInstruments, Unavailable};

    const WF: &str = r#"
name: CI
on:
  pull_request:
    types: [opened, synchronize, reopened, edited]
permissions:
  contents: read
jobs:
  lint:
    runs-on: ubuntu-latest
    steps: [{ run: cargo clippy }]
  gate:
    name: Discipline
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: orieg/discipline@v0
  report:
    needs: gate
    runs-on: ubuntu-latest
    steps: [{ run: echo done }]
  ci-gate:
    needs: [lint, report]
    runs-on: ubuntu-latest
    steps: [{ run: "true" }]
"#;

    fn wf(content: &str) -> Vec<(String, String)> {
        vec![(".github/workflows/ci.yml".to_string(), content.to_string())]
    }

    #[test]
    fn finds_discipline_jobs_and_transitive_rollups() {
        let facts = analyse_workflows(&wf(WF), false);
        assert_eq!(facts.jobs.len(), 1);
        let j = &facts.jobs[0];
        assert_eq!(j.job_id, "gate");
        assert_eq!(j.context, "Discipline");
        assert_eq!(j.rollups, vec!["report".to_string(), "ci-gate".to_string()]);
        let status = |id: &str| facts.findings.iter().find(|f| f.id == id).unwrap().status;
        assert_eq!(status("workflows"), Status::Pass);
        assert_eq!(status("trigger"), Status::Pass);
        assert_eq!(status("token"), Status::Pass);
    }

    #[test]
    fn run_steps_and_self_action_count_but_comments_do_not() {
        let run = "on: pull_request\njobs:\n  a:\n    steps:\n      - run: |\n          # discipline check is disabled\n          echo hi\n  b:\n    steps:\n      - run: ./bin/discipline check --base origin/main\n  c:\n    steps:\n      - uses: ./\n";
        let ids: Vec<String> = analyse_workflows(&wf(run), false)
            .jobs
            .into_iter()
            .map(|j| j.job_id)
            .collect();
        assert_eq!(ids, vec!["b"]);
        let ids: Vec<String> = analyse_workflows(&wf(run), true)
            .jobs
            .into_iter()
            .map(|j| j.job_id)
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
    }

    #[test]
    fn missing_edited_trigger_write_token_and_no_job_are_reported() {
        let loose = WF
            .replace("    types: [opened, synchronize, reopened, edited]\n", "")
            .replace("permissions:\n  contents: read\n", "");
        let facts = analyse_workflows(&wf(&loose), false);
        let status = |id: &str| facts.findings.iter().find(|f| f.id == id).unwrap().status;
        assert_eq!(status("trigger"), Status::Warn);
        assert_eq!(status("token"), Status::Warn);

        // An explicit `types:` list without `edited` is the same gap.
        let typed = WF.replace("reopened, edited]", "reopened]");
        let facts = analyse_workflows(&wf(&typed), false);
        let trigger = facts.findings.iter().find(|f| f.id == "trigger").unwrap();
        assert_eq!(trigger.status, Status::Warn);

        let none = analyse_workflows(
            &wf("on: push\njobs:\n  a:\n    steps: [{run: make}]\n"),
            false,
        );
        assert!(none.jobs.is_empty());
        assert_eq!(none.findings[0].status, Status::Fail);

        let broken = analyse_workflows(&wf("jobs: [unclosed"), false);
        assert!(broken.findings.iter().any(|f| f.status == Status::Unknown));
    }

    #[test]
    fn codeowners_last_match_wins_and_directories_are_owned() {
        let co = "* @team\n/.github/workflows/ @ci\ndiscipline.toml @gate\n/docs/\n";
        assert!(codeowners_covers(co, "discipline.toml"));
        assert!(codeowners_covers(co, ".github/workflows/ci.yml"));
        assert!(codeowners_covers(co, "src/lib.rs"));
        // A later rule without owners removes ownership.
        assert!(!codeowners_covers(co, "docs/index.md"));
        assert!(!codeowners_covers("/src/ @x\n", "discipline.toml"));
        // Unanchored names match at any depth; anchored ones only at the root.
        assert!(codeowners_covers(
            "discipline.toml @x\n",
            "sub/discipline.toml"
        ));
        assert!(!codeowners_covers(
            "/discipline.toml @x\n",
            "sub/discipline.toml"
        ));

        let targets = vec![
            "discipline.toml".to_string(),
            ".github/workflows/ci.yml".to_string(),
        ];
        assert_eq!(codeowners_finding(None, &targets).status, Status::Warn);
        assert_eq!(
            codeowners_finding(Some(("CODEOWNERS", "/discipline.toml @x\n")), &targets).status,
            Status::Warn
        );
        assert_eq!(
            codeowners_finding(Some(("CODEOWNERS", co)), &targets).status,
            Status::Pass
        );
    }

    fn github(rules: serde_json::Value, bypass: serde_json::Value) -> CannedInstruments {
        let mut c = CannedInstruments::default();
        c.responses
            .insert("repos/o/r/rules/branches/main".into(), rules);
        c.responses.insert(
            "repos/o/r/rulesets/7".into(),
            serde_json::json!({ "bypass_actors": bypass }),
        );
        c.responses.insert(
            "repos/o/r/branches/main".into(),
            serde_json::json!({"protected": true, "protection": {"enabled": false}}),
        );
        c.responses.insert(
            "repos/o/r".into(),
            serde_json::json!({"default_branch": "main"}),
        );
        c
    }

    fn full_rules() -> serde_json::Value {
        serde_json::json!([
          {"type": "required_status_checks", "ruleset_id": 7, "parameters": {
             "strict_required_status_checks_policy": true,
             "required_status_checks": [{"context": "ci-gate"}]}},
          {"type": "non_fast_forward", "ruleset_id": 7},
          {"type": "deletion", "ruleset_id": 7},
          {"type": "pull_request", "ruleset_id": 7, "parameters": {}}
        ])
    }

    #[test]
    fn github_rules_that_enforce_the_rollup_pass() {
        let gh = github(full_rules(), serde_json::json!([]));
        let p = github_protection(&gh, "o/r", "main").unwrap();
        assert!(p.strict && p.force_push_blocked && p.deletion_blocked && p.pull_request_required);
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(&p, &jobs);
        assert!(
            f.iter()
                .all(|f| matches!(f.status, Status::Pass | Status::Info)),
            "{f:?}"
        );
    }

    #[test]
    fn required_check_that_does_not_run_discipline_fails() {
        let rules = serde_json::json!([
          {"type": "required_status_checks", "ruleset_id": 7, "parameters": {
             "strict_required_status_checks_policy": false,
             "required_status_checks": [{"context": "lint"}]}}
        ]);
        let gh = github(
            rules,
            serde_json::json!([{"actor_type": "RepositoryRole", "bypass_mode": "always"}]),
        );
        let p = github_protection(&gh, "o/r", "main").unwrap();
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(&p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap();
        assert_eq!(get("required-check").status, Status::Fail);
        assert!(get("required-check")
            .remediation
            .as_deref()
            .unwrap()
            .contains("ci-gate"));
        assert_eq!(get("up-to-date").status, Status::Warn);
        assert_eq!(get("force-push").status, Status::Fail);
        assert_eq!(get("deletion").status, Status::Warn);
        assert_eq!(get("bypass").status, Status::Warn);

        let none = protection_findings(&Protection::default(), &jobs);
        assert_eq!(none[0].status, Status::Fail);
        assert!(none[0].summary.contains("no status check"));
    }

    #[test]
    fn classic_protection_is_merged_and_unreadable_classic_is_an_error() {
        let mut gh = github(serde_json::json!([]), serde_json::json!([]));
        gh.responses.insert(
            "repos/o/r/branches/main".into(),
            serde_json::json!({"protected": true, "protection": {"enabled": true,
              "required_status_checks": {"contexts": ["Discipline"]}}}),
        );
        assert!(github_protection(&gh, "o/r", "main").is_err());
        gh.responses.insert(
            "repos/o/r/branches/main/protection".into(),
            serde_json::json!({
              "required_status_checks": {"strict": true, "contexts": ["Discipline"]},
              "allow_force_pushes": {"enabled": false},
              "allow_deletions": {"enabled": false},
              "enforce_admins": {"enabled": false}}),
        );
        let p = github_protection(&gh, "o/r", "main").unwrap();
        assert!(p.required_contexts.contains("Discipline") && p.strict && p.force_push_blocked);
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(&p, &jobs);
        assert_eq!(
            f.iter().find(|x| x.id == "required-check").unwrap().status,
            Status::Pass
        );
        assert_eq!(
            f.iter().find(|x| x.id == "bypass").unwrap().status,
            Status::Warn
        );
    }

    #[test]
    fn exit_code_follows_the_gate_contract() {
        let report = |s: &[Status]| Report {
            platform: "t".into(),
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

    #[test]
    fn unreachable_platform_is_unknown_not_healthy() {
        let dir = std::env::temp_dir().join(format!("discipline-doctor-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".github/workflows")).unwrap();
        std::fs::write(dir.join(".github/workflows/ci.yml"), WF).unwrap();
        let input = DoctorInput {
            root: &dir,
            repository: Some("o/r".into()),
            branch: None,
            platform: Platform::GitHub,
            local_only: false,
            gh: &Unavailable,
        };
        let r = run(&input);
        assert_eq!(r.exit_code(false), 2, "{r:?}");
        let local = run(&DoctorInput {
            local_only: true,
            ..input
        });
        assert!(local.findings.iter().all(|f| f.id != "platform"));
        let other = run(&DoctorInput {
            platform: Platform::Other,
            local_only: false,
            root: &dir,
            repository: None,
            branch: None,
            gh: &Unavailable,
        });
        assert_eq!(other.exit_code(false), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
