//! CI/CD workflow integrity and rollup sentinel (`ci-integrity`).
//!
//! Enforces:
//! - Rollup jobs (`ci-gate`) depend on every verification job in the workflow (`needs:`).
//! - Rollup jobs cannot drop previously depended-on jobs without an override.
//! - Third-party actions (step `uses:`, including the nested steps of a composite action's
//!   `action.yml`) and remote reusable workflows (job-level `uses:`) are pinned by a
//!   40-character commit SHA (`actions/` and `github/` included, unless a repository lists
//!   them in `first_party_action_prefixes`);
//!   container images (`container:`, `services.*.image`, `uses: docker://`) carry an
//!   `@sha256:` digest.
//! - With `diff_only = true` (the default) only a reference new relative to the base side is
//!   judged; with `diff_only = false` every reference is, pre-existing ones adopted through
//!   the baseline.
//! - A `uses:` matching a `banned_actions` entry is reported in every scanned file of the
//!   head tree, whatever `diff_only` says; no directive lifts it.
//! - Exposure of secrets or a write token (`ci_exposure`): template injection in `run:`,
//!   `secrets: inherit`, persisted checkout credentials in a job that can write, secrets
//!   beside a third-party action, a scheduled workflow reading secrets.
//! - Masked failures (`continue-on-error: true`) and error suppression (`|| true`, `set +e`) are forbidden.
//! - Verification jobs and steps (testing, linting, gates) cannot be deleted without an override;
//!   a step renamed with a similar body is paired with its base form, not reported as deleted.
//! - Compilation / lint flags cannot be dropped (`-D warnings`, `--locked`, `--all-targets`).
//! - Action inputs to `orieg/discipline` cannot be weakened (`disable`, `fail_on_warnings: false`, narrowed `suite`, etc.).
//! - Workflow permissions cannot be widened from `read` to `write` without an override.
//! - `pull_request_target` triggers cannot be introduced without fork protection.
//! - Documented job counts in catalogue documentation stay in sync with workflow definitions.

mod banned;
mod discipline_pin;
mod gitlab_pipeline;
mod job_steps;
mod locate;
mod pins;
mod removed;
mod rollup;
mod step_matching;
#[cfg(test)]
mod test_support;
mod triggers;

pub(crate) use banned::*;
pub use discipline_pin::*;
use gitlab_pipeline::*;
pub use job_steps::*;
pub(crate) use locate::*;
pub(crate) use pins::*;
pub(crate) use removed::*;
pub use rollup::*;
pub(crate) use step_matching::*;
pub(crate) use triggers::*;

// The submodules reach these through `super::`.
use super::{capture_pattern, ci_gitlab};
use crate::guards::{exempt_filter, line_allows, Context, GateOutcome, PathFilter, Severity};
use crate::tokens;
use anyhow::Result;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub const GATE: &str = "ci-integrity";

/// Why a documented job count was not compared, for the report.
const NOT_COMPARED: &str = "the documented job count was not compared with the workflow";

/// Evaluates CI workflow integrity and rollup invariants.
pub fn evaluate_ci_integrity(ctx: &Context) -> Result<GateOutcome> {
    let mut out = GateOutcome::new(GATE);
    let settings = &ctx.config.gates.ci_integrity;

    let filter = exempt_filter(settings)?;

    let workflow_filter = PathFilter::new(&settings.workflows)?;
    let docs = YamlDocs::default();

    // Banned references are judged across the whole tree, whatever `diff_only` says: a
    // reference is banned whether or not this change added it.
    if !settings.banned_actions.is_empty() {
        check_banned(ctx, &docs, &filter, &workflow_filter, &mut out)?;
    }

    let Some(workflow_files) = workflow_files_to_examine(ctx, &filter, &workflow_filter, &mut out)?
    else {
        return Ok(out);
    };

    let mut added_steps = AddedSteps {
        workflow_filter: &workflow_filter,
        filter: &filter,
        docs: &docs,
        steps: None,
    };
    // Whether a workflow examined here has the rollup job the documented count is
    // compared under.
    let mut rollup_seen = false;

    for path in &workflow_files {
        // GitLab pipelines are a different document shape; they have their own diff.
        if super::ci_gitlab::is_gitlab_ci_path(path) {
            evaluate_gitlab_file(ctx, path, &mut out)?;
            continue;
        }
        // A composite action's metadata file carries steps, not jobs: only its nested
        // `uses:` are checked, never the rollup and job rules.
        if is_action_metadata_path(path) {
            evaluate_action_file(ctx, path, &docs, &mut out)?;
            continue;
        }
        if evaluate_workflow_file(ctx, path, &mut added_steps, &mut out)? {
            rollup_seen = true;
        }
    }

    if !rollup_seen
        && settings.documented_job_count_path.is_some()
        && settings.documented_job_count_pattern.is_some()
    {
        // The count is compared under the rollup job only; say so when there was none.
        let why = match &settings.rollup_job {
            Some(rollup) => {
                format!("no workflow examined in this run has the `rollup_job` (`{rollup}`)")
            }
            None => "`rollup_job` is not set".to_string(),
        };
        out.notes.push(format!("{why}; {NOT_COMPARED}"));
    }
    Ok(out)
}

/// The workflow files this run examines: with `diff_only`, the ones the change touches
/// (a GitLab pipeline also when a file it includes changed), else every tracked one.
/// `None` when `diff_only` leaves none, which is noted.
fn workflow_files_to_examine(
    ctx: &Context,
    filter: &PathFilter,
    workflow_filter: &PathFilter,
    out: &mut GateOutcome,
) -> Result<Option<Vec<String>>> {
    let settings = &ctx.config.gates.ci_integrity;
    let workflow_files = if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        let mut files = Vec::new();
        for f in changed {
            if filter.matches(&f.path) || !workflow_filter.matches(&f.path) {
                continue;
            }
            files.push(f.path);
        }
        // A change to a file the pipeline pulls in through `include: local:` is a change
        // to the pipeline: analyse the (unchanged) pipeline file against its base.
        let changed = ctx.git.changed_files()?;
        for pipeline in [".gitlab-ci.yml", ".gitlab-ci.yaml"] {
            if files.iter().any(|p| p == pipeline) || filter.matches(pipeline) {
                continue;
            }
            let Some(head) = ctx.git.head_content(pipeline)? else {
                continue;
            };
            let local = super::ci_gitlab::includes(&head).local;
            if changed
                .iter()
                .any(|f| local.iter().any(|l| *l == f.path || *l == f.old_path))
            {
                files.push(pipeline.to_string());
            }
        }
        if files.is_empty() {
            out.notes
                .push("no workflow files modified in this diff".to_string());
            return Ok(None);
        }
        files
    } else {
        let tracked = ctx.git.tracked_files()?;
        let files: Vec<_> = tracked
            .into_iter()
            .filter(|p| !filter.matches(p) && workflow_filter.matches(p))
            .collect();
        files
    };
    Ok(Some(workflow_files))
}

/// Steps of the jobs this change added, read once and only when a job or workflow
/// was removed.
struct AddedSteps<'a> {
    workflow_filter: &'a PathFilter,
    filter: &'a PathFilter,
    docs: &'a YamlDocs,
    steps: Option<Vec<serde_yaml::Value>>,
}

impl AddedSteps<'_> {
    /// Reads the steps unless they were read already.
    fn load(&mut self, ctx: &Context, notes: &mut Vec<String>) -> Result<()> {
        if self.steps.is_none() {
            self.steps = Some(added_job_steps(
                ctx,
                self.workflow_filter,
                self.filter,
                self.docs,
                notes,
            )?);
        }
        Ok(())
    }

    /// The steps read so far: none before the first `load`.
    fn loaded(&self) -> &[serde_yaml::Value] {
        self.steps.as_deref().unwrap_or(&[])
    }
}

/// One GitHub workflow file under comparison: its head text, its base text when the base
/// side has the file, and each side's document when it parses.
struct WorkflowFile<'a> {
    path: &'a str,
    head_content: &'a str,
    base_content: Option<&'a str>,
    head: Option<&'a serde_yaml::Value>,
    base: Option<&'a serde_yaml::Value>,
}

/// One job of the head workflow, with the base side's job of the same id.
struct JobSite<'a> {
    id: &'a str,
    value: &'a serde_yaml::Value,
    base_job: Option<&'a serde_yaml::Value>,
    line: Option<usize>,
}

/// One step of a head job, with the base step it is compared against and the line it is
/// reported at.
struct StepSite<'a> {
    job: &'a JobSite<'a>,
    step: &'a serde_yaml::Value,
    base_step: Option<&'a serde_yaml::Value>,
    approx_line: Option<usize>,
    /// The step's own lines, when the job's steps are written one item each.
    span: Option<Span>,
}

impl StepSite<'_> {
    /// The line of the step's key `key`, else the line the step is reported at.
    fn key_line(&self, content: &str, key: &str) -> Option<usize> {
        self.span
            .and_then(|s| s.item_key_line(content, key))
            .or(self.approx_line)
    }

    /// The first line of the step that contains `needle`.
    fn text_line(&self, content: &str, needle: &str) -> Option<usize> {
        self.span
            .and_then(|s| s.line_with(content, s.start, needle))
    }
}

/// Evaluates one GitHub workflow file against its base side. Returns whether the file
/// has the rollup job.
fn evaluate_workflow_file(
    ctx: &Context,
    path: &str,
    added_steps: &mut AddedSteps,
    out: &mut GateOutcome,
) -> Result<bool> {
    let settings = &ctx.config.gates.ci_integrity;
    let head_content = match ctx.git.head_content(path)? {
        Some(c) => c,
        None => {
            // Workflow file deleted in head
            report_deleted_workflow(ctx, path, added_steps, out)?;
            return Ok(false);
        }
    };

    let base_content = ctx.git.base_content(path)?;
    let docs = added_steps.docs;
    let head_val = docs.noted(out, path, "head", Some(&head_content));
    let base_val = docs.noted(out, path, "base", base_content.as_deref());
    if head_val.is_some() {
        out.examined += 1;
    } else if base_content.is_some() {
        // A workflow that does not parse runs none of the jobs its base side had, and
        // cannot be shown to be unweakened: a finding, as for a GitLab pipeline. A new
        // file has nothing to be weakened against and keeps the note alone.
        out.push(
            settings.severity,
            &crate::findings::PIPELINE_FILE_UNREADABLE,
            Some(path),
            None,
            format!(
                "`{path}` does not parse as YAML on the head side, so its jobs could not be compared with its base side."
            ),
            "Fix the YAML so the workflow can be checked.",
        );
    }

    let wf = WorkflowFile {
        path,
        head_content: &head_content,
        base_content: base_content.as_deref(),
        head: head_val.as_deref(),
        base: base_val.as_deref(),
    };

    check_discipline_pin(ctx, &wf, out);
    check_workflow_pins(ctx, &wf, out);
    check_workflow_exposures(ctx, &wf, out);

    // 1. Rollup job checks
    let rollup_seen = check_rollup_job(ctx, &wf, out)?;

    if let Some(head_doc) = wf.head {
        // 3. Workflow-level trigger and permission AST checks
        check_triggers_and_permissions(ctx, &wf, head_doc, out);

        // 4. Job and step comparisons against base
        compare_jobs_with_base(ctx, &wf, head_doc, added_steps, out)?;

        // 5. AST Step-by-Step and Job-level inspection
        inspect_jobs(ctx, &wf, head_doc, out);
    }
    Ok(rollup_seen)
}

/// Reports a change of the discipline release the workflow pins: the binary that judges
/// the change is then chosen by the change.
fn check_discipline_pin(ctx: &Context, wf: &WorkflowFile, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    if let (Some(b), Some(h)) = (wf.base, wf.head) {
        let mut head_pins = discipline_pins(h);
        locate_pins(&mut head_pins, head_content);
        let (blocking, notes) = discipline_pin_changes(&discipline_pins(b), &head_pins);
        for n in notes {
            out.notes.push(format!("`{path}`: discipline pin {n}"));
        }
        if !blocking.is_empty() {
            let line = blocking.iter().filter_map(|(_, l)| *l).min();
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::DISCIPLINE_VERSION_CHANGED,
                Some(path.to_string()),
                line,
                format!(
                    "The discipline that judges this change is chosen by the change: {}.",
                    describe_blocking(&blocking)
                ),
                "Keep the discipline pin, or move it to a newer immutable release; excuse with allow-gate-weakening: ci-integrity <reason>.",
                "discipline-version",
            );
        }
    }
}

/// Checks that the remote references the workflow pulls in are pinned.
fn check_workflow_pins(ctx: &Context, wf: &WorkflowFile, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (base_content, base_val) = (wf.base_content, wf.base);
    // Pinning: every remote reference the workflow pulls in (step and job-level
    // `uses:`, `container:`, `services.*.image`, `docker://`), compared against the
    // base side's references.
    if settings.pin_actions {
        if let Some(head_doc) = wf.head {
            let head_refs = workflow_pin_refs(head_doc, head_content);
            let base_refs = base_pin_set(
                base_val.map(|b| workflow_pin_refs(b, base_content.unwrap_or_default())),
            );
            check_pins(ctx, path, head_content, &head_refs, &base_refs, out);
        }
    }
}

/// Reports secrets or a write token the workflow newly exposes.
fn check_workflow_exposures(ctx: &Context, wf: &WorkflowFile, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (base_content, base_val) = (wf.base_content, wf.base);
    // Exposure: secrets or a write token handed to code the workflow does not
    // control (template injection, `secrets: inherit`, persisted credentials, a
    // third-party action beside secrets, a scheduled workflow reading secrets).
    if let Some(head_doc) = wf.head {
        let fp = &settings.first_party_action_prefixes;
        let head_x = super::ci_exposure::workflow_exposures(head_doc, head_content, fp);
        let base_x = base_val
            .map(|b| {
                super::ci_exposure::workflow_exposures(b, base_content.unwrap_or_default(), fp)
            })
            .unwrap_or_default();
        report_exposures(ctx, path, head_content, head_x, &base_x, out);
    }
}

/// The `run:` script of a step without its full-line shell comments: what
/// actually executes. Comments neither make two steps different nor keep a
/// commented-out command alive.
fn executable_run(step: &serde_yaml::Value) -> Option<String> {
    step.get("run").and_then(|r| r.as_str()).map(|run| {
        run.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Steps of the jobs this change added, in every workflow file in head: where a
/// removed job's steps may have moved (a rename, a split, a move to another file).
///
/// A file with a side that does not parse contributes nothing, and `notes` names it: its
/// head side has no jobs to read, and without its base side no job of it can be shown to
/// be one the change added.
fn added_job_steps(
    ctx: &Context,
    globs: &crate::guards::PathFilter,
    filter: &crate::guards::PathFilter,
    docs: &YamlDocs,
    notes: &mut Vec<String>,
) -> Result<Vec<serde_yaml::Value>> {
    let unparsed = |path: &str, side: &str| {
        format!(
            "{path}: the {side} side could not be parsed, so its jobs were not compared as the place a removed job's steps moved to"
        )
    };
    let mut paths = ctx.git.tracked_files()?;
    paths.extend(ctx.git.changed_files()?.into_iter().map(|f| f.path));
    paths.sort();
    paths.dedup();
    let mut steps = Vec::new();
    for p in paths.iter().filter(|p| {
        globs.matches(p) && !filter.matches(p) && !super::ci_gitlab::is_gitlab_ci_path(p)
    }) {
        let Some(head) = ctx.git.head_content(p)? else {
            continue;
        };
        let Ok(head) = docs.load(p, "head", &head) else {
            notes.push(unparsed(p, "head"));
            continue;
        };
        let base = match ctx.git.base_content(p)? {
            None => None,
            Some(b) => match docs.load(p, "base", &b) {
                Ok(b) => Some(b),
                // Every job of the file would read as added.
                Err(_) => {
                    notes.push(unparsed(p, "base"));
                    continue;
                }
            },
        };
        let base_jobs: HashSet<String> = base
            .as_deref()
            .and_then(|b| {
                b.get("jobs").and_then(|j| j.as_mapping()).map(|m| {
                    m.keys()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default();
        if let Some(jobs) = head.get("jobs").and_then(|j| j.as_mapping()) {
            for (k, job) in jobs {
                if k.as_str().is_some_and(|k| base_jobs.contains(k)) {
                    continue;
                }
                if let Some(s) = job.get("steps").and_then(|s| s.as_sequence()) {
                    steps.extend(s.iter().cloned());
                }
            }
        }
    }
    Ok(steps)
}

/// Display label of a step: its name, else its id, else `fallback`.
fn step_label<'a>(step: &'a serde_yaml::Value, fallback: &'a str) -> &'a str {
    step.get("name")
        .and_then(|n| n.as_str())
        .or_else(|| step.get("id").and_then(|i| i.as_str()))
        .unwrap_or(fallback)
}

fn steps_match_identity(a: &serde_yaml::Value, b: &serde_yaml::Value) -> bool {
    let a_id = a.get("id").and_then(|i| i.as_str());
    let b_id = b.get("id").and_then(|i| i.as_str());
    if let (Some(ai), Some(bi)) = (a_id, b_id) {
        return ai == bi;
    }

    let a_name = a.get("name").and_then(|n| n.as_str());
    let b_name = b.get("name").and_then(|n| n.as_str());
    if let (Some(an), Some(bn)) = (a_name, b_name) {
        return an == bn;
    }

    let a_uses = a.get("uses").and_then(|u| u.as_str());
    let b_uses = b.get("uses").and_then(|u| u.as_str());
    if let (Some(au), Some(bu)) = (a_uses, b_uses) {
        let au_action = au.split('@').next().unwrap_or(au);
        let bu_action = bu.split('@').next().unwrap_or(bu);
        if au_action == bu_action {
            return true;
        }
    }

    let a_run = a.get("run").and_then(|r| r.as_str());
    let b_run = b.get("run").and_then(|r| r.as_str());
    if let (Some(ar), Some(br)) = (a_run, b_run) {
        let first_a = ar.lines().next().unwrap_or("").trim();
        let first_b = br.lines().next().unwrap_or("").trim();
        if !first_a.is_empty() && first_a == first_b {
            return true;
        }
    }

    false
}

/// Substrings of a `run:` script that mark a step as verifying something.
const VERIFYING_RUN_MARKERS: &[&str] = &[
    "test",
    "clippy",
    "cargo check",
    "fmt --check",
    "lint",
    "pytest",
    "discipline",
    "audit",
];

/// Substrings of a `uses:` action that mark a step as verifying something.
const VERIFYING_USES_MARKERS: &[&str] = &["discipline", "clippy", "actionlint"];

/// The verification markers a step's executable body (not its name, not its
/// comment lines) carries.
fn verifying_body_markers(step: &serde_yaml::Value) -> HashSet<&'static str> {
    markers_in(step, executable_run(step))
}

fn markers_in(step: &serde_yaml::Value, run: Option<String>) -> HashSet<&'static str> {
    let mut found = HashSet::new();
    if let Some(run) = run {
        let r = run.to_ascii_lowercase();
        found.extend(VERIFYING_RUN_MARKERS.iter().filter(|m| r.contains(*m)));
    }
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        let u = uses.to_ascii_lowercase();
        found.extend(VERIFYING_USES_MARKERS.iter().filter(|m| u.contains(*m)));
    }
    found
}

fn describe_blocking(blocking: &[(String, Option<usize>)]) -> String {
    blocking
        .iter()
        .map(|(d, _)| d.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Actions that report a result rather than check one: they upload, download, or post it.
const REPORTING_ACTIONS: &[&str] = &[
    "actions/upload-artifact",
    "actions/download-artifact",
    "peter-evans/create-or-update-comment",
    "marocchino/sticky-pull-request-comment",
    "thollander/actions-comment-pull-request",
    "mshick/add-pr-comment",
];

/// First words of a step name that says it reports: `Comment the gate result on the PR`,
/// `Upload test logs`, `Show the diff for any failing test`.
const REPORTING_VERBS: &[&str] = &[
    "comment",
    "post",
    "upload",
    "download",
    "publish",
    "report",
    "show",
    "print",
    "summarize",
    "summarise",
    "annotate",
    "notify",
    "render",
];

/// Whether a step only reports: it uses a reporting action (or `github-script` whose
/// script cannot fail the step: no `setFailed`, no `throw`), or its name starts with a
/// reporting verb. Such a step is not a check because its name mentions one.
fn is_reporting_step(step: &serde_yaml::Value) -> bool {
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        let action = uses.split('@').next().unwrap_or(uses).trim();
        if REPORTING_ACTIONS.contains(&action) {
            return true;
        }
        if action == "actions/github-script" {
            let script = step
                .get("with")
                .and_then(|w| w.get("script"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            return !script.contains("setFailed") && !script.contains("throw");
        }
    }
    step.get("name")
        .and_then(|n| n.as_str())
        .and_then(|n| n.split_whitespace().next())
        .is_some_and(|w| REPORTING_VERBS.contains(&w.to_ascii_lowercase().as_str()))
}

fn is_verification_step(step: &serde_yaml::Value) -> bool {
    let raw_run = step.get("run").and_then(|r| r.as_str()).map(str::to_string);
    if !markers_in(step, raw_run).is_empty() {
        return true;
    }
    // A name that mentions a check makes a check only of a step that does not report.
    if is_reporting_step(step) {
        return false;
    }
    if let Some(name) = step.get("name").and_then(|n| n.as_str()) {
        let n = name.to_ascii_lowercase();
        if n.contains("test")
            || n.contains("lint")
            || n.contains("clippy")
            || n.contains("gate")
            || n.contains("check")
            || n.contains("discipline")
        {
            return true;
        }
    }
    false
}

fn is_verification_job(job_id: &str, job: &serde_yaml::Value) -> bool {
    let lower_id = job_id.to_ascii_lowercase();
    if lower_id.contains("test")
        || lower_id.contains("lint")
        || lower_id.contains("gate")
        || lower_id.contains("check")
        || lower_id.contains("clippy")
        || lower_id.contains("audit")
        || lower_id.contains("verify")
        || lower_id.contains("ci")
    {
        return true;
    }
    if let Some(steps) = job.get("steps").and_then(|s| s.as_sequence()) {
        if steps.iter().any(is_verification_step) {
            return true;
        }
    }
    false
}

/// The image name of a `docker://` reference, or `None` for any other `uses:`.
fn docker_image(uses: &str) -> Option<&str> {
    uses.strip_prefix("docker://")
}

/// Whether `path` is an action metadata file (`action.yml` / `action.yaml`) rather than
/// a workflow: named so, and outside every workflow directory.
pub(crate) fn is_action_metadata_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    (name == "action.yml" || name == "action.yaml")
        && !crate::doctor::WORKFLOW_DIRS
            .iter()
            .any(|dir| path.starts_with(&format!("{dir}/")))
}

/// The one place this gate loads the YAML of a GitHub workflow or action file. `Err` is
/// where the parser stopped, as ` (line L, column C)`, or empty when it gave no place.
/// Location only: the parser's message can quote the text near the error.
pub(crate) fn load_yaml(text: &str) -> std::result::Result<serde_yaml::Value, String> {
    #[cfg(test)]
    test_support::record_load(text);
    serde_yaml::from_str::<serde_yaml::Value>(text).map_err(|e| {
        e.location()
            .map(|l| format!(" (line {}, column {})", l.line(), l.column()))
            .unwrap_or_default()
    })
}

/// The note for a side of a CI file that does not parse; `at` is what [`load_yaml`]
/// returned for it.
fn unparsed_side_note(path: &str, side: &str, at: &str) -> String {
    format!(
        "{path}: the {side} side does not parse as YAML{at}; the pin, exposure and rollup checks that read it were skipped"
    )
}

/// Parses one side of a CI file. `None` for an absent side (nothing to say) and, with a
/// note naming the file and the side, for a side that does not parse: the checks that
/// compare that side are skipped, which is reported, never silent.
pub(crate) fn parse_yaml_side(
    out: &mut GateOutcome,
    path: &str,
    side: &str,
    text: Option<&str>,
) -> Option<serde_yaml::Value> {
    match load_yaml(text?) {
        Ok(v) => Some(v),
        Err(at) => {
            out.notes.push(unparsed_side_note(path, side, &at));
            None
        }
    }
}

/// One side of a file as [`YamlDocs`] keeps it: the document, or where the parser stopped.
type LoadedSide = std::result::Result<Rc<serde_yaml::Value>, String>;

/// The GitHub workflow and action files this run has loaded, by path and side (`head` or
/// `base`), so that each side of each file is parsed once however many checks read it:
/// the banned-reference scan, the file's own comparison, and the search for the jobs a
/// removed job's steps moved to. Neither side of the change moves while the gate runs,
/// so the text of a path and side is the same on every read.
#[derive(Default)]
pub(crate) struct YamlDocs {
    sides: RefCell<HashMap<(String, &'static str), LoadedSide>>,
}

impl YamlDocs {
    /// The document of `side` of `path`, parsed from `text` on the first call and kept.
    /// A side that does not parse is kept too. Says nothing: each caller words its own
    /// note for a side it could not read.
    pub(crate) fn load(&self, path: &str, side: &'static str, text: &str) -> LoadedSide {
        let key = (path.to_string(), side);
        if let Some(kept) = self.sides.borrow().get(&key) {
            return kept.clone();
        }
        let loaded = load_yaml(text).map(Rc::new);
        self.sides.borrow_mut().insert(key, loaded.clone());
        loaded
    }

    /// [`parse_yaml_side`] over the kept documents: `None` for an absent side and, with
    /// the same note, for a side that does not parse.
    pub(crate) fn noted(
        &self,
        out: &mut GateOutcome,
        path: &str,
        side: &'static str,
        text: Option<&str>,
    ) -> Option<Rc<serde_yaml::Value>> {
        match self.load(path, side, text?) {
            Ok(v) => Some(v),
            Err(at) => {
                out.notes.push(unparsed_side_note(path, side, &at));
                None
            }
        }
    }
}

/// `ci-integrity` for a composite action's metadata file: pinning of its nested
/// `uses:` only.
fn evaluate_action_file(
    ctx: &Context,
    path: &str,
    docs: &YamlDocs,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.ci_integrity;
    let Some(head) = ctx.git.head_content(path)? else {
        return Ok(());
    };
    let base = ctx.git.base_content(path)?;
    let Ok(doc) = docs.load(path, "head", &head) else {
        out.notes.push(format!(
            "{path}: action metadata does not parse as YAML; its nested `uses:` and `run:` steps were not checked"
        ));
        // An action file that no longer parses checks none of the steps its base side
        // had: a finding, as for a workflow. A new file has nothing to be weakened
        // against and keeps the note alone.
        if base.is_some() {
            out.push(
                settings.severity,
                &crate::findings::PIPELINE_FILE_UNREADABLE,
                Some(path),
                None,
                format!(
                    "`{path}` does not parse as YAML on the head side, so its steps could not be compared with its base side."
                ),
                "Fix the YAML so the action can be checked.",
            );
        }
        return Ok(());
    };
    out.examined += 1;
    let base_doc = docs.noted(out, path, "base", base.as_deref());
    if settings.pin_actions {
        let base_refs = base_pin_set(
            base_doc
                .as_deref()
                .map(|d| action_pin_refs(d, base.as_deref().unwrap_or_default())),
        );
        let refs = action_pin_refs(&doc, &head);
        check_pins(ctx, path, &head, &refs, &base_refs, out);
    }
    let base_x = base_doc
        .as_deref()
        .map(|d| super::ci_exposure::action_exposures(d, base.as_deref().unwrap_or_default()))
        .unwrap_or_default();
    let head_x = super::ci_exposure::action_exposures(&doc, &head);
    report_exposures(ctx, path, &head, head_x, &base_x, out);
    Ok(())
}

/// Reports the exposures of one file (`ci_exposure`). With `diff_only` (the default)
/// only an exposure new relative to the base side is reported; with `diff_only = false`
/// every one is, and the message says whether this change added it.
fn report_exposures(
    ctx: &Context,
    path: &str,
    content: &str,
    head: Vec<super::ci_exposure::Exposure>,
    base: &[super::ci_exposure::Exposure],
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let base_keys: HashSet<(&str, &str)> =
        base.iter().map(|x| (x.kind.code, x.key.as_str())).collect();
    for x in head {
        let pre_existing = base_keys.contains(&(x.kind.code, x.key.as_str()));
        if pre_existing && settings.diff_only {
            continue;
        }
        let origin = if pre_existing {
            "pre-existing: already on the base side"
        } else {
            "added by this change"
        };
        // A third-party action in a job that reads secrets is often there by design (a
        // registry login, a deploy): a warning, never louder than the gate is set to.
        let severity = if x.kind.code == crate::findings::SECRETS_WITH_THIRD_PARTY_ACTION.code
            && settings.severity == Severity::Error
        {
            Severity::Warning
        } else {
            settings.severity
        };
        let before = out.violations.len();
        record_or_excuse(
            ctx,
            Some(content),
            out,
            severity,
            x.kind,
            Some(path.to_string()),
            x.line,
            format!("{} ({origin})", x.message),
            x.remediation,
            &x.subject,
        );
        if x.line.is_none() && out.violations.len() > before {
            out.anchor_last(x.key.replace('\u{1f}', ":"));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_or_excuse(
    ctx: &Context,
    head_content: Option<&str>,
    out: &mut GateOutcome,
    severity: Severity,
    kind: &crate::findings::FindingKind,
    file: Option<String>,
    line: Option<usize>,
    message: String,
    remediation: impl Into<String>,
    subject: &str,
) {
    if let (Some(content), Some(line_no)) = (head_content, line) {
        if let Some(l) = content.lines().nth(line_no.saturating_sub(1)) {
            if line_allows(l, GATE)
                || line_allows(l, subject)
                || line_allows(l, "allow-gate-weakening")
                || line_allows(l, "allow-ci-weakening")
            {
                out.inline_exemptions += 1;
                return;
            }
        }
    }

    let ov = ctx
        .find_override(GATE, kind, tokens::ALLOW_CI_WEAKENING, subject)
        .or_else(|| {
            subject
                .split_once('@')
                .and_then(|(act, _)| ctx.find_override(GATE, kind, tokens::ALLOW_CI_WEAKENING, act))
        })
        .or_else(|| ctx.find_override(GATE, kind, tokens::ALLOW_GATE_WEAKENING, GATE));

    if let Some(record) = ov {
        out.overrides.push(record);
    } else {
        let rem_str = remediation.into();
        out.push(severity, kind, file.as_deref(), line, message, &rem_str);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::ci_integrity::test_support::*;

    #[test]
    fn verification_step_detection_still_reads_the_raw_run_text() {
        // Unchanged by rename pairing: comment text still marks a step as
        // verification, so its deletion stays guarded.
        let s = steps("- name: Step\n  run: |\n    # run the lint pass\n    ./ci.sh\n");
        assert!(is_verification_step(&s[0]));
        assert!(verifying_body_markers(&s[0]).is_empty());
    }

    #[test]
    fn action_metadata_paths_are_told_from_workflows() {
        assert!(is_action_metadata_path("action.yml"));
        assert!(is_action_metadata_path("action.yaml"));
        assert!(is_action_metadata_path(".github/actions/setup/action.yml"));
        assert!(!is_action_metadata_path(".github/workflows/action.yml"));
        assert!(!is_action_metadata_path(".gitea/workflows/action.yaml"));
        assert!(!is_action_metadata_path(".github/workflows/ci.yml"));
        assert!(!is_action_metadata_path("my-action.yml"));
    }

    #[test]
    fn a_side_that_does_not_parse_is_noted_with_its_path_and_side() {
        let mut out = GateOutcome::new(GATE);
        assert!(parse_yaml_side(&mut out, ".github/workflows/ci.yml", "head", None).is_none());
        assert!(out.notes.is_empty(), "an absent side has nothing to say");
        assert!(parse_yaml_side(&mut out, "w.yml", "base", Some("on: push\n")).is_some());
        assert!(out.notes.is_empty());

        assert!(parse_yaml_side(
            &mut out,
            ".github/workflows/ci.yml",
            "head",
            Some("jobs:\n\tbuild: [")
        )
        .is_none());
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].starts_with(".github/workflows/ci.yml: the head side does not parse"));
        assert!(out.notes[0].contains("were skipped"), "{:?}", out.notes);
    }

    #[test]
    fn a_kept_side_is_parsed_once_and_noted_on_every_read_that_notes() {
        let docs = YamlDocs::default();
        let broken = "jobs:\n\tbuild: [";
        let mut out = GateOutcome::new(GATE);
        let (loads, ()) = loads_of(broken, || {
            assert!(docs.load("w.yml", "head", broken).is_err());
            assert!(docs
                .noted(&mut out, "w.yml", "head", Some(broken))
                .is_none());
            assert!(docs
                .noted(&mut out, "w.yml", "head", Some(broken))
                .is_none());
        });
        assert_eq!(loads, 1, "a side that does not parse is kept as well");
        // `load` says nothing; each `noted` read says the same as `parse_yaml_side`.
        let mut plain = GateOutcome::new(GATE);
        assert!(parse_yaml_side(&mut plain, "w.yml", "head", Some(broken)).is_none());
        assert_eq!(out.notes, [plain.notes[0].clone(), plain.notes[0].clone()]);

        // The two sides of one path, and one side of two paths, are different documents.
        let (head, base) = ("jobs: {a: {}}\n", "jobs: {b: {}}\n");
        let head_doc = docs.load("x.yml", "head", head).unwrap();
        let base_doc = docs.load("x.yml", "base", base).unwrap();
        let other = docs.load("y.yml", "head", base).unwrap();
        assert!(head_doc.get("jobs").unwrap().get("a").is_some());
        assert!(base_doc.get("jobs").unwrap().get("b").is_some());
        assert!(other.get("jobs").unwrap().get("b").is_some());
        assert!(docs.noted(&mut out, "x.yml", "base", None).is_none());
        assert_eq!(out.notes.len(), 2, "an absent side has nothing to say");
    }

    /// The findings of the gate over one workflow changed from `base` to `head`, with how
    /// many times each side's text was loaded as YAML.
    fn evaluate_counting_loads(base: &str, head: &str) -> (usize, usize, GateOutcome) {
        let config = crate::config::DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\n[gates.ci-integrity]\nenabled = true\nrollup_job = \"ci-gate\"\nbanned_actions = [\"evil/action\"]\n",
        )
        .unwrap();
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            ".github/workflows/ci.yml",
            base,
            head,
        );
        let ctx = crate::guards::test_support::context(&config, &git);
        let (head_loads, out) = loads_of(head, || evaluate_ci_integrity(&ctx).unwrap());
        let (base_loads, _) = loads_of(base, || evaluate_ci_integrity(&ctx).unwrap());
        (head_loads, base_loads, out)
    }

    #[test]
    fn each_side_of_a_workflow_is_loaded_once_by_every_check_that_reads_it() {
        // One run that reaches every reader of the file: the banned-reference scan, the
        // file's own comparison, the rollup job's dependencies on both sides, and (a
        // verification job was removed) the search for where its steps moved.
        let base = "on: push\njobs:\n  lint:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo clippy\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n  ci-gate:\n    needs: [lint, test]\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n";
        let head = "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n  ci-gate:\n    needs: lint\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n";
        let (head_loads, base_loads, out) = evaluate_counting_loads(base, head);
        assert_eq!((head_loads, base_loads), (1, 1));
        let mut codes: Vec<&str> = out.violations.iter().map(|v| v.code.as_str()).collect();
        codes.sort_unstable();
        assert_eq!(
            codes,
            [
                "ci-integrity/rollup-needs-incomplete",
                "ci-integrity/rollup-needs-removed",
                "ci-integrity/verification-job-removed",
            ],
            "{:?}",
            out.violations
        );
    }
}
