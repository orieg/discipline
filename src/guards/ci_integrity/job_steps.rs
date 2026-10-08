//! Jobs and steps that survive the change: failures that no longer fail the job,
//! conditions that narrow when a step runs, dropped flags and weakened discipline inputs.

use super::{
    child_key_line, is_verification_job, is_verification_step, job_line, pair_steps,
    record_or_excuse, step_label, step_line, step_spans, steps_match_identity, JobSite, StepMatch,
    StepSite, WorkflowFile, GATE,
};
use crate::guards::{line_allows, Context, GateOutcome, Severity};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

/// Inspects each job of the head side, and each of its steps, for what it newly weakens.
pub(super) fn inspect_jobs(
    ctx: &Context,
    wf: &WorkflowFile,
    head_doc: &serde_yaml::Value,
    out: &mut GateOutcome,
) {
    let head_content = wf.head_content;
    let base_val = wf.base;
    if let Some(jobs_map) = head_doc.get("jobs").and_then(|j| j.as_mapping()) {
        let base_jobs_map = base_val
            .and_then(|b| b.get("jobs"))
            .and_then(|j| j.as_mapping());

        for (job_k, job_v) in jobs_map {
            let job_id = job_k.as_str().unwrap_or("");
            let job_line = job_line(head_content, job_id);
            let job = JobSite {
                id: job_id,
                value: job_v,
                base_job: base_jobs_map.and_then(|m| m.get(job_k)),
                line: job_line,
            };
            check_job_continue_on_error(ctx, wf, &job, out);
            check_job_condition(ctx, wf, &job, out);
            inspect_job_steps(ctx, wf, &job, out);
        }
    }
}

/// Reports a job that newly carries `continue-on-error: true`.
fn check_job_continue_on_error(
    ctx: &Context,
    wf: &WorkflowFile,
    job: &JobSite,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (job_id, job_v, job_line) = (job.id, job.value, job.line);
    // Job-level continue-on-error
    if settings.forbid_continue_on_error
        && job_v.get("continue-on-error").and_then(|c| c.as_bool()) == Some(true)
    {
        let base_had_it = job
            .base_job
            .and_then(|b| b.get("continue-on-error"))
            .and_then(|c| c.as_bool())
            == Some(true);
        if !base_had_it {
            let coe_line = job_line
                .and_then(|l| child_key_line(head_content, l, "continue-on-error"))
                .or(job_line);
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                if is_verification_job(job_id, job_v) { settings.severity } else { Severity::Warning },
                &crate::findings::JOB_FAILURE_MASKED_CONTINUE_ON_ERROR,
                Some(path.to_string()),
                coe_line,
                format!("Job '{job_id}' carries 'continue-on-error: true', which masks failures in CI."),
                "Remove continue-on-error or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                job_id,
            );
        }
    }
}

/// Reports a verification job that newly runs under `always()` or `cancelled()` without
/// enforcing the results of the jobs it needs.
fn check_job_condition(ctx: &Context, wf: &WorkflowFile, job: &JobSite, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (job_id, job_v, job_line) = (job.id, job.value, job.line);
    // Job-level if: always() on verification jobs
    if is_verification_job(job_id, job_v) {
        if let Some(if_val) = job_v.get("if") {
            let if_cond = match if_val {
                serde_yaml::Value::String(s) => s.as_str(),
                serde_yaml::Value::Bool(b) => {
                    if *b {
                        "true"
                    } else {
                        "false"
                    }
                }
                _ => "",
            };
            if (if_cond.contains("always()") || if_cond.contains("cancelled()"))
                && !crate::doctor::rollup_enforces(job_v)
            {
                let base_had_always = job
                    .base_job
                    .and_then(|b| b.get("if"))
                    .map(|b| {
                        let b_s = match b {
                            serde_yaml::Value::String(s) => s.as_str(),
                            serde_yaml::Value::Bool(bv) => {
                                if *bv {
                                    "true"
                                } else {
                                    "false"
                                }
                            }
                            _ => "",
                        };
                        b_s.contains("always()") || b_s.contains("cancelled()")
                    })
                    .unwrap_or(false);
                if !base_had_always {
                    let if_line = job_line
                        .and_then(|l| child_key_line(head_content, l, "if"))
                        .or(job_line);
                    record_or_excuse(
                        ctx,
                        Some(head_content),
                        out,
                        settings.severity,
                        &crate::findings::VERIFICATION_JOB_MASKED_BY_CONDITION,
                        Some(path.to_string()),
                        if_line,
                        format!("Verification job '{job_id}' carries 'if: {if_cond}', masking earlier pipeline failures."),
                        "Remove conditional masking or excuse with allow-gate-weakening: ci-integrity <reason>.",
                        "if-always",
                    );
                }
            }
        }
    }
}

/// Inspects each step of a head job that differs from its base form and carries no
/// inline exemption.
fn inspect_job_steps(ctx: &Context, wf: &WorkflowFile, job: &JobSite, out: &mut GateOutcome) {
    let head_content = wf.head_content;
    let job_v = job.value;
    let base_job_steps = job
        .base_job
        .and_then(|j| j.get("steps"))
        .and_then(|s| s.as_sequence());

    if let Some(steps) = job_v.get("steps").and_then(|s| s.as_sequence()) {
        // Head step index -> the base step it was renamed from, so a
        // renamed step is still compared against its base form.
        let mut renamed_from: HashMap<usize, &serde_yaml::Value> = HashMap::new();
        if let Some(b_steps) = base_job_steps {
            for (bi, pair) in pair_steps(b_steps, steps).into_iter().enumerate() {
                if let Some(StepMatch::Renamed { head, .. }) = pair {
                    renamed_from.insert(head, &b_steps[bi]);
                }
            }
        }
        let spans = job
            .line
            .and_then(|l| step_spans(head_content, l, steps.len()));
        for (step_idx, step) in steps.iter().enumerate() {
            let step_name = step.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let step_id = step.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let uses_str = step.get("uses").and_then(|u| u.as_str());
            let run_str = step.get("run").and_then(|r| r.as_str());

            // Find matching base step
            let base_step = base_job_steps
                .and_then(|b_steps| b_steps.iter().find(|b| steps_match_identity(b, step)))
                .or_else(|| renamed_from.get(&step_idx).copied());

            // A line of the step itself; where the steps have no line each, the job's.
            let span = spans.as_ref().map(|s| s[step_idx]);
            let approx_line = span
                .map(|s| step_line(head_content, s, step_name, step_id, uses_str, run_str))
                .or(job.line);

            if let Some(b) = base_step {
                if b == step {
                    continue;
                }
            }

            if let Some(line_no) = approx_line {
                if let Some(line) = head_content.lines().nth(line_no - 1) {
                    if line_allows(line, GATE) {
                        out.inline_exemptions += 1;
                        continue;
                    }
                }
            }

            let site = StepSite {
                job,
                step,
                base_step,
                approx_line,
                span,
            };
            check_discipline_action_inputs(ctx, wf, &site, out);
            check_run_advisory(ctx, wf, &site, out);
            check_dropped_flags(ctx, wf, &site, out);
            check_step_runs_only_on_failure(ctx, wf, &site, out);
            check_step_narrowed(ctx, wf, &site, out);
            check_step_continue_on_error(ctx, wf, &site, out);
            check_step_exit_code_masked(ctx, wf, &site, out);
        }
    }
}

/// Checks the inputs of a step that runs the discipline action against its base form.
fn check_discipline_action_inputs(
    ctx: &Context,
    wf: &WorkflowFile,
    site: &StepSite,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (step, base_step, approx_line) = (site.step, site.base_step, site.approx_line);
    let uses_str = step.get("uses").and_then(|u| u.as_str());
    // 5a. Action pinning is checked for the whole file above
    // (`check_pins`).

    // 5b. Check orieg/discipline action inputs
    if let Some(uses) = uses_str {
        if uses.contains("orieg/discipline")
            || uses.contains("discipline")
            || uses == "./action.yml"
        {
            // policy_from moved off `base` (or the whole `with:` block dropped): the change
            // is judged by its own configuration again
            let policy_from = |w: Option<&serde_yaml::Value>| {
                w.and_then(|w| w.get("policy_from"))
                    .and_then(|p| p.as_str())
                    .map(|p| p.trim().to_ascii_lowercase())
            };
            if policy_from(base_step.and_then(|b| b.get("with"))).as_deref() == Some("base")
                && policy_from(step.get("with")).as_deref() != Some("base")
            {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::DISCIPLINE_ACTION_POLICY_FROM,
                    Some(path.to_string()),
                    approx_line,
                    "The discipline step no longer sets 'policy_from: base'; the change would be judged by its own discipline.toml.".to_string(),
                    "Restore 'policy_from: base' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "policy_from",
                );
            }

            if let Some(with_val) = step.get("with") {
                let base_with_val = base_step.and_then(|b| b.get("with"));
                check_discipline_enforcement_inputs(ctx, wf, site, with_val, base_with_val, out);
                check_discipline_scope_inputs(ctx, wf, site, with_val, base_with_val, out);
            }
        }
    }
}

/// The discipline action inputs that stop a finding from failing the step: `disable`,
/// `advisory`, `fail_on_warnings`.
fn check_discipline_enforcement_inputs(
    ctx: &Context,
    wf: &WorkflowFile,
    site: &StepSite,
    with_val: &serde_yaml::Value,
    base_with_val: Option<&serde_yaml::Value>,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let approx_line = site.approx_line;
    // disable input added or expanded
    if let Some(disable_val) = with_val.get("disable") {
        let base_disable = base_with_val.and_then(|b| b.get("disable"));
        let is_new_or_expanded = match (base_disable, disable_val) {
            (None, _) => true,
            (Some(bv), hv) => bv != hv,
        };
        if is_new_or_expanded {
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::DISCIPLINE_ACTION_DISABLE_INPUT,
                Some(path.to_string()),
                approx_line,
                "The 'disable' input on the discipline step was added or widened, bypassing verification gates.".to_string(),
                "Remove 'disable' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                "disable",
            );
        }
    }

    // advisory switched on: the step reports and exits 0
    if advisory_input_on(Some(with_val)) && !advisory_input_on(base_with_val) {
        record_or_excuse(
            ctx,
            Some(head_content),
            out,
            settings.severity,
            &crate::findings::DISCIPLINE_ACTION_ADVISORY,
            Some(path.to_string()),
            approx_line,
            "The 'advisory' input on the discipline step was switched on; the step exits 0 whatever the gates report.".to_string(),
            "Remove 'advisory' or excuse with allow-gate-weakening: ci-integrity <reason>.",
            "advisory",
        );
    }

    // fail_on_warnings set to false
    if let Some(fow) = with_val.get("fail_on_warnings") {
        if fow.as_bool() == Some(false) {
            let base_fow = base_with_val
                .and_then(|b| b.get("fail_on_warnings"))
                .and_then(|b| b.as_bool())
                .unwrap_or(true);
            if base_fow {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::DISCIPLINE_ACTION_FAIL_ON_WARNINGS_OFF,
                    Some(path.to_string()),
                    approx_line,
                    "fail_on_warnings was set to false, suppressing warning-severity gate failures.".to_string(),
                    "Restore fail_on_warnings: true or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "fail_on_warnings",
                );
            }
        }
    }
}

/// The discipline action inputs that choose what is checked and what lifts a finding:
/// `config_override`, `suite`, `directive_sources`.
fn check_discipline_scope_inputs(
    ctx: &Context,
    wf: &WorkflowFile,
    site: &StepSite,
    with_val: &serde_yaml::Value,
    base_with_val: Option<&serde_yaml::Value>,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let approx_line = site.approx_line;
    // config_override pointing to non-existent or weakened files
    if let Some(cfg_ov) = with_val.get("config_override").and_then(|c| c.as_str()) {
        let ov_file = Path::new(ctx.git.root()).join(cfg_ov);
        if !ov_file.is_file() {
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::DISCIPLINE_ACTION_CONFIG_OVERRIDE_INVALID,
                Some(path.to_string()),
                approx_line,
                format!("config_override points to non-existent file '{cfg_ov}'."),
                "Provide a valid configuration file path.",
                "config_override",
            );
        }
    }

    // suite narrowed
    if let Some(suite_val) = with_val.get("suite").and_then(|s| s.as_str()) {
        let base_suite = base_with_val
            .and_then(|b| b.get("suite"))
            .and_then(|s| s.as_str());
        if let Some(bs) = base_suite {
            if bs != suite_val {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::DISCIPLINE_ACTION_SUITE_CHANGED,
                    Some(path.to_string()),
                    approx_line,
                    format!("discipline suite changed from '{bs}' to '{suite_val}'."),
                    "Restore original suite or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "suite",
                );
            }
        }
    }

    // directive_sources widened
    if let Some(ds_val) = with_val.get("directive_sources") {
        let base_ds = base_with_val.and_then(|b| b.get("directive_sources"));
        if base_ds.is_none() || base_ds != Some(ds_val) {
            let ds_str = serde_yaml::to_string(ds_val).unwrap_or_default();
            if ds_str.contains("commits") {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::DISCIPLINE_ACTION_DIRECTIVE_SOURCES_WIDENED,
                    Some(path.to_string()),
                    approx_line,
                    "directive_sources was widened to accept directives from commit messages.".to_string(),
                    "Keep directive sources restricted, or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "directive_sources",
                );
            }
        }
    }
}

/// Reports a `run:` command that newly passes `--advisory` to discipline.
fn check_run_advisory(ctx: &Context, wf: &WorkflowFile, site: &StepSite, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (base_step, approx_line) = (site.base_step, site.approx_line);
    let run_str = site.step.get("run").and_then(|r| r.as_str());
    // 5b'. `discipline check --advisory` in a run command
    if let Some(head_run) = run_str {
        let base_run = base_step
            .and_then(|b| b.get("run"))
            .and_then(|r| r.as_str())
            .unwrap_or("");
        if run_is_advisory(head_run) && !run_is_advisory(base_run) {
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::DISCIPLINE_RUN_ADVISORY,
                Some(path.to_string()),
                approx_line,
                "A discipline command gained '--advisory'; it exits 0 whatever the gates report."
                    .to_string(),
                "Remove '--advisory' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                "--advisory",
            );
        }
    }
}

/// Reports build, lint and install flags a `run:` command dropped.
fn check_dropped_flags(ctx: &Context, wf: &WorkflowFile, site: &StepSite, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (base_step, approx_line) = (site.base_step, site.approx_line);
    let run_str = site.step.get("run").and_then(|r| r.as_str());
    // 5c. Dropping build/clippy flags from run commands
    if let Some(head_run) = run_str {
        if let Some(base_run) = base_step
            .and_then(|b| b.get("run"))
            .and_then(|r| r.as_str())
        {
            if base_run.contains("-D warnings") && !head_run.contains("-D warnings") {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::COMPILER_DENY_WARNINGS_REMOVED,
                    Some(path.to_string()),
                    approx_line,
                    "Step dropped '-D warnings' from command, allowing compiler/linter warnings to pass.".to_string(),
                    "Restore '-D warnings' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "-D warnings",
                );
            }
            if base_run.contains("--locked") && !head_run.contains("--locked") {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::CARGO_LOCKED_REMOVED,
                    Some(path.to_string()),
                    approx_line,
                    "Step dropped '--locked' from cargo command, permitting unverified dependency updates.".to_string(),
                    "Restore '--locked' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "--locked",
                );
            }
            // Frozen-install flags of other package managers, and the
            // `npm ci` -> `npm install` swap: each lets an install
            // resolve past the lockfile.
            for flag in FROZEN_INSTALL_FLAGS {
                if base_run.contains(flag) && !head_run.contains(flag) {
                    record_or_excuse(
                        ctx,
                        Some(head_content),
                        out,
                        settings.severity,
                        &crate::findings::FROZEN_INSTALL_FLAG_REMOVED,
                        Some(path.to_string()),
                        approx_line,
                        format!("Step dropped '{flag}' from an install command, permitting an install that resolves past the lockfile."),
                        format!("Restore '{flag}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                        flag,
                    );
                }
            }
            if base_run.contains("npm ci")
                && !head_run.contains("npm ci")
                && head_run.contains("npm install")
            {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::INSTALL_COMMAND_WEAKENED,
                    Some(path.to_string()),
                    approx_line,
                    "Step replaced 'npm ci' with 'npm install': the install may rewrite the lockfile instead of honouring it.".to_string(),
                    "Restore 'npm ci' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "npm ci",
                );
            }
            if base_run.contains("--all-targets") && !head_run.contains("--all-targets") {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::CLIPPY_ALL_TARGETS_REMOVED,
                    Some(path.to_string()),
                    approx_line,
                    "Step dropped '--all-targets' from clippy command, skipping linting on tests/benchmarks.".to_string(),
                    "Restore '--all-targets' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "--all-targets",
                );
            }
        }
    }
}

/// Reports an existing verification step that now runs only after a failure.
fn check_step_runs_only_on_failure(
    ctx: &Context,
    wf: &WorkflowFile,
    site: &StepSite,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (step, base_step) = (site.step, site.base_step);
    // 5d. An existing verification step that now runs only after a
    // failure (`if: failure()` without `always()`): it no longer runs
    // on a passing build, so what it checked goes unchecked. A new
    // step that runs on failure (a diagnostic) replaces nothing, and
    // `always()` makes a step run more often, not less.
    if is_verification_step(step) && base_step.is_some() {
        if let Some(if_cond) = step.get("if").and_then(|i| i.as_str()) {
            let failure_only = |c: &str| {
                let l = c.to_ascii_lowercase();
                l.contains("failure()") && !l.contains("always()")
            };
            if failure_only(if_cond) {
                let base_had_it = base_step
                    .and_then(|b| b.get("if"))
                    .and_then(|i| i.as_str())
                    .is_some_and(failure_only);
                if !base_had_it {
                    let if_line = site.key_line(head_content, "if");
                    record_or_excuse(
                        ctx,
                        Some(head_content),
                        out,
                        settings.severity,
                        &crate::findings::VERIFICATION_STEP_MASKED_BY_CONDITION,
                        Some(path.to_string()),
                        if_line,
                        format!("Verification step now carries 'if: {if_cond}': it runs only after an earlier failure, so a passing build no longer runs it."),
                        "Remove conditional masking or excuse with allow-gate-weakening: ci-integrity <reason>.",
                        "if-always",
                    );
                }
            }
        }
    }
}

/// Reports a verification step whose `if:` was added or changed so that it runs on fewer
/// events or conditions.
fn check_step_narrowed(ctx: &Context, wf: &WorkflowFile, site: &StepSite, out: &mut GateOutcome) {
    let path = wf.path;
    let head_content = wf.head_content;
    let (step, base_step) = (site.step, site.base_step);
    let job_id = site.job.id;
    // 5d'. A verification step that newly runs on fewer events or
    // conditions: a step-level `if:` added, or changed. The always() /
    // failure() forms are 5d's; a plain narrowing is a weakening of
    // the same kind as continue-on-error, reported at warning.
    if is_verification_step(step) {
        let head_if = step.get("if").map(if_text);
        let base_if = base_step.and_then(|b| b.get("if")).map(if_text);
        // 5d's failure-only form, or a condition that only widens
        // when the step runs; `always() && <narrowing>` is checked.
        let masking = |t: &str| {
            let l = t.to_ascii_lowercase();
            let bare = l
                .trim()
                .trim_start_matches("${{")
                .trim_end_matches("}}")
                .trim()
                .to_string();
            matches!(bare.as_str(), "always()" | "!cancelled()")
                || (l.contains("failure()") && !l.contains("always()"))
        };
        if let Some(h) = head_if.as_deref().filter(|h| !masking(h)) {
            if base_if.as_deref() != Some(h) {
                let if_line = site.key_line(head_content, "if");
                let what = match &base_if {
                    None => format!("gains 'if: {h}'"),
                    Some(b) => format!("changes 'if: {b}' to 'if: {h}'"),
                };
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    Severity::Warning,
                    &crate::findings::VERIFICATION_STEP_NARROWED,
                    Some(path.to_string()),
                    if_line,
                    format!("Verification step '{}' in job '{job_id}' {what}: it no longer runs on every event or condition it ran on before.", step_label(step, "unnamed step")),
                    "Run the step unconditionally, or record the narrowing with allow-gate-weakening: ci-integrity <reason>. A discipline step restricted to pull_request stops gating pushes to the default branch; the `merged-pr-body` directive source is the alternative when PR-body waivers are the reason.",
                    "if-narrowed",
                );
            }
        }
    }
}

/// Reports a step that newly carries `continue-on-error: true`.
fn check_step_continue_on_error(
    ctx: &Context,
    wf: &WorkflowFile,
    site: &StepSite,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (step, base_step) = (site.step, site.base_step);
    let (job_id, job_v) = (site.job.id, site.job.value);
    // 5e. continue-on-error. In a job that verifies nothing (a summary,
    // a report) no check is masked: a warning.
    let verifies = is_verification_job(job_id, job_v);
    let coe_severity = if verifies {
        settings.severity
    } else {
        Severity::Warning
    };
    if settings.forbid_continue_on_error
        && step.get("continue-on-error").and_then(|c| c.as_bool()) == Some(true)
    {
        let base_had_it = base_step
            .and_then(|b| b.get("continue-on-error"))
            .and_then(|c| c.as_bool())
            == Some(true);
        if !base_had_it {
            let coe_line = site.key_line(head_content, "continue-on-error");
            let step_subject = step
                .get("name")
                .and_then(|n| n.as_str())
                .or_else(|| step.get("id").and_then(|i| i.as_str()))
                .unwrap_or("continue-on-error");
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                coe_severity,
                &crate::findings::STEP_FAILURE_MASKED_CONTINUE_ON_ERROR,
                Some(path.to_string()),
                coe_line,
                if verifies {
                    "Step carries 'continue-on-error: true', which masks failures in CI.".to_string()
                } else {
                    format!("Step carries 'continue-on-error: true' in job '{job_id}', which verifies nothing (a warning: no check is masked).")
                },
                "Remove continue-on-error or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                step_subject,
            );
        }
    }
}

/// Reports a `run:` command that newly masks its exit code (`|| true`, `set +e`).
fn check_step_exit_code_masked(
    ctx: &Context,
    wf: &WorkflowFile,
    site: &StepSite,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (step, base_step, approx_line) = (site.step, site.base_step, site.approx_line);
    let run_str = step.get("run").and_then(|r| r.as_str());
    // 5f. Error suppression: || true / set +e
    if settings.forbid_or_true {
        if let Some(run_cmd) = run_str {
            if masks_exit_code(run_cmd) {
                let base_had_mask = base_step
                    .and_then(|b| b.get("run"))
                    .and_then(|r| r.as_str())
                    .is_some_and(masks_exit_code);
                if !base_had_mask {
                    let mask_line = site
                        .text_line(head_content, "|| true")
                        .or_else(|| site.text_line(head_content, "set +e"))
                        .or(approx_line);
                    let step_subject = step
                        .get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| step.get("id").and_then(|i| i.as_str()))
                        .unwrap_or("or-true");
                    record_or_excuse(
                        ctx,
                        Some(head_content),
                        out,
                        settings.severity,
                        &crate::findings::EXIT_CODE_MASKED,
                        Some(path.to_string()),
                        mask_line,
                        "Command uses '|| true' or 'set +e' to mask command failure.".to_string(),
                        "Remove '|| true' or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                        step_subject,
                    );
                }
            }
        }
    }
}

/// Whether a script that turns `set -e` off checks the status it stops acting on:
/// `$?` is saved into a variable that a later line tests or exits with, or a later
/// line tests or exits with `$?` itself. `set +e; cmd; rc=$?; set -e; [ "$rc" -eq 0 ]`
/// is a checked negative control, not a masked failure.
pub(crate) fn set_e_status_checked(run: &str) -> bool {
    let lines: Vec<&str> = run.lines().collect();
    let Some(start) = lines.iter().position(|l| l.contains("set +e")) else {
        return false;
    };
    let tests = |l: &str| {
        let l = l.trim_start();
        ["if ", "[ ", "[[ ", "test ", "exit ", "case ", "((", "elif "]
            .iter()
            .any(|k| l.starts_with(k) || l.contains(&format!("; {k}")))
    };
    let assign = Regex::new(r#"\b([A-Za-z_][A-Za-z0-9_]*)="?\$\?"?"#).expect("static regex");
    let mut vars: Vec<String> = Vec::new();
    for l in &lines[start..] {
        if tests(l) {
            if l.contains("$?") {
                return true;
            }
            if vars
                .iter()
                .any(|v| l.contains(&format!("${v}")) || l.contains(&format!("${{{v}}}")))
            {
                return true;
            }
        }
        for c in assign.captures_iter(l) {
            vars.push(c[1].to_string());
        }
    }
    false
}

/// Whether a step's script masks a failure: `|| true`, or `set +e` whose status is
/// never checked.
fn masks_exit_code(run: &str) -> bool {
    run.lines().any(|l| l.contains("|| true"))
        || (run.lines().any(|l| l.contains("set +e")) && !set_e_status_checked(run))
}

/// Whether a discipline step's `with:` block switches advisory mode on. YAML `true` and
/// the string "true" both reach the action as the same input.
fn advisory_input_on(with: Option<&serde_yaml::Value>) -> bool {
    match with.and_then(|w| w.get("advisory")) {
        Some(serde_yaml::Value::Bool(b)) => *b,
        Some(serde_yaml::Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Whether a run script invokes discipline with `--advisory`. Comment lines do not count.
pub fn run_is_advisory(run: &str) -> bool {
    run.lines().map(str::trim).any(|l| {
        !l.starts_with('#')
            && (l.contains("discipline check") || l.contains("discipline diff"))
            && l.split_whitespace().any(|w| w == "--advisory")
    })
}

/// The text of a step or job `if:` value (a string, or a bare boolean).
fn if_text(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.trim().to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// Flags that make an install honour its lockfile: yarn / pnpm `--frozen-lockfile`, yarn 2+
/// `--immutable`, pip `--require-hashes`, uv / cargo `--frozen`, poetry `--no-update`.
/// (`--locked` has its own rule above.)
pub const FROZEN_INSTALL_FLAGS: &[&str] = &[
    "--frozen-lockfile",
    "--immutable",
    "--require-hashes",
    "--frozen",
    "--no-update",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisory_is_read_from_the_input_and_the_flag_not_from_prose() {
        let with = |y: &str| serde_yaml::from_str::<serde_yaml::Value>(y).unwrap();
        assert!(advisory_input_on(Some(&with("advisory: true"))));
        assert!(advisory_input_on(Some(&with("advisory: 'True'"))));
        assert!(!advisory_input_on(Some(&with("advisory: false"))));
        assert!(!advisory_input_on(Some(&with("suite: all"))));
        assert!(!advisory_input_on(None));

        assert!(run_is_advisory(
            "set -e\ndiscipline check --advisory --suite all"
        ));
        assert!(run_is_advisory("discipline diff --advisory"));
        assert!(!run_is_advisory("discipline check --suite all"));
        assert!(!run_is_advisory(
            "# discipline check --advisory is forbidden here\ndiscipline check"
        ));
        assert!(!run_is_advisory("echo --advisory"));
        assert!(!run_is_advisory("discipline check --advisory-notes"));
    }
}
