//! CI/CD workflow integrity and rollup sentinel (`ci-integrity`).
//!
//! Enforces:
//! - Rollup jobs (`ci-gate`) depend on every verification job in the workflow (`needs:`).
//! - Rollup jobs cannot drop previously depended-on jobs without an override.
//! - Third-party actions (step `uses:`, including the nested steps of a composite action's
//!   `action.yml`) and remote reusable workflows (job-level `uses:`) are pinned by a
//!   40-character commit SHA (excluding first-party prefixes like `actions/` and `github/`);
//!   container images (`container:`, `services.*.image`, `uses: docker://`) carry an
//!   `@sha256:` digest.
//! - With `diff_only = true` (the default) only a reference new relative to the base side is
//!   judged; with `diff_only = false` every reference is, pre-existing ones adopted through
//!   the baseline.
//! - Masked failures (`continue-on-error: true`) and error suppression (`|| true`, `set +e`) are forbidden.
//! - Verification jobs and steps (testing, linting, gates) cannot be deleted without an override;
//!   a step renamed with a similar body is paired with its base form, not reported as deleted.
//! - Compilation / lint flags cannot be dropped (`-D warnings`, `--locked`, `--all-targets`).
//! - Action inputs to `orieg/discipline` cannot be weakened (`disable`, `fail_on_warnings: false`, narrowed `suite`, etc.).
//! - Workflow permissions cannot be widened from `read` to `write` without an override.
//! - `pull_request_target` triggers cannot be introduced without fork protection.
//! - Documented job counts in catalogue documentation stay in sync with workflow definitions.

use crate::guards::{exempt_filter, line_allows, Context, GateOutcome, Severity, Violation};
use crate::tokens;
use anyhow::{Context as _, Result};
use globset::GlobSetBuilder;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const GATE: &str = "ci-integrity";

/// Evaluates CI workflow integrity and rollup invariants.
pub fn evaluate_ci_integrity(ctx: &Context) -> Result<GateOutcome> {
    let mut out = GateOutcome::new(GATE);
    let settings = &ctx.config.gates.ci_integrity;
    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let filter = exempt_filter(settings)?;

    let mut glob_builder = GlobSetBuilder::new();
    for pattern in &settings.workflows {
        let glob = globset::Glob::new(pattern)
            .with_context(|| format!("Invalid workflow glob: '{pattern}'"))?;
        glob_builder.add(glob);
    }
    let workflow_globs = glob_builder.build()?;

    let (workflow_files, added_lines_map) = if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        let mut files = Vec::new();
        let mut line_map = std::collections::HashMap::new();
        for f in changed {
            if filter.matches(&f.path) || !workflow_globs.is_match(&f.path) {
                continue;
            }
            files.push(f.path.clone());
            line_map.insert(f.path, f.added_lines);
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
            return Ok(out);
        }
        (files, Some(line_map))
    } else {
        let tracked = ctx.git.tracked_files()?;
        let files: Vec<_> = tracked
            .into_iter()
            .filter(|p| !filter.matches(p) && workflow_globs.is_match(p))
            .collect();
        (files, None)
    };

    // Steps of the jobs this change added, read once and only when a job or workflow
    // was removed.
    let mut added_steps: Option<Vec<serde_yaml::Value>> = None;

    for path in &workflow_files {
        // GitLab pipelines are a different document shape; they have their own diff.
        if super::ci_gitlab::is_gitlab_ci_path(path) {
            evaluate_gitlab_file(ctx, path, &mut out)?;
            continue;
        }
        // A composite action's metadata file carries steps, not jobs: only its nested
        // `uses:` are checked, never the rollup and job rules.
        if is_action_metadata_path(path) {
            evaluate_action_file(ctx, path, &mut out)?;
            continue;
        }
        let head_content = match ctx.git.head_content(path)? {
            Some(c) => c,
            None => {
                // Workflow file deleted in head
                if let Ok(Some(base_src)) = ctx.git.base_content(path) {
                    if let Ok(base_val) = serde_yaml::from_str::<serde_yaml::Value>(&base_src) {
                        let (base_jobs, _) =
                            parse_workflow_jobs(&base_src, settings.rollup_job.as_deref());
                        let verification: Vec<(&String, &serde_yaml::Value)> = base_jobs
                            .iter()
                            .filter_map(|j| {
                                let job_val = base_val.get("jobs").and_then(|m| m.get(j))?;
                                is_verification_job(j, job_val).then_some((j, job_val))
                            })
                            .collect();
                        if added_steps.is_none() && !verification.is_empty() {
                            added_steps = Some(added_job_steps(ctx, &workflow_globs, &filter)?);
                        }
                        let added = added_steps.as_deref().unwrap_or(&[]);
                        if !verification.is_empty()
                            && verification.iter().all(|(_, job)| job_moved(job, added))
                        {
                            out.notes.push(format!(
                                "{path}: workflow deleted; the verification steps of its jobs ({}) are in jobs this change added, so it is treated as a move",
                                verification
                                    .iter()
                                    .map(|(j, _)| j.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        } else if !verification.is_empty() {
                            record_or_excuse(
                                ctx,
                                None,
                                &mut out,
                                settings.severity,
                                &crate::findings::VERIFICATION_WORKFLOW_DELETED,
                                Some(path.clone()),
                                None,
                                format!("Workflow '{path}' containing verification jobs was deleted."),
                                "Restore the deleted workflow or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                                path,
                            );
                        }
                    }
                }
                continue;
            }
        };

        out.examined += 1;
        let base_content = ctx.git.base_content(path).unwrap_or(None);
        let head_val = serde_yaml::from_str::<serde_yaml::Value>(&head_content).ok();
        let base_val = base_content
            .as_deref()
            .and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(s).ok());

        if let (Some(b), Some(h)) = (&base_val, &head_val) {
            let (blocking, notes) =
                discipline_pin_changes(&discipline_pins(b), &discipline_pins(h));
            for n in notes {
                out.notes.push(format!("`{path}`: discipline pin {n}"));
            }
            if !blocking.is_empty() {
                let line = find_line_number(&head_content, "discipline");
                record_or_excuse(
                    ctx,
                    Some(&head_content),
                    &mut out,
                    settings.severity,
                    &crate::findings::DISCIPLINE_VERSION_CHANGED,
                    Some(path.clone()),
                    line,
                    format!(
                        "The discipline that judges this change is chosen by the change: {}.",
                        blocking.join("; ")
                    ),
                    "Keep the discipline pin, or move it to a newer immutable release; excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "discipline-version",
                );
            }
        }

        // Pinning: every remote reference the workflow pulls in (step and job-level
        // `uses:`, `container:`, `services.*.image`, `docker://`), compared against the
        // base side's references.
        if settings.pin_actions {
            if let Some(head_doc) = &head_val {
                let head_refs = workflow_pin_refs(head_doc, &head_content);
                let base_refs =
                    base_pin_set(base_val.as_ref().map(|b| {
                        workflow_pin_refs(b, base_content.as_deref().unwrap_or_default())
                    }));
                check_pins(ctx, path, &head_content, &head_refs, &base_refs, &mut out);
            }
        }

        let _added_lines = added_lines_map.as_ref().and_then(|m| m.get(path));

        // 1. Rollup job checks
        let (jobs, rollup_needs) =
            parse_workflow_jobs(&head_content, settings.rollup_job.as_deref());
        let head_all_needs = parse_all_job_needs(&head_content);
        if let Some(ref base_src) = base_content {
            let base_all_needs = parse_all_job_needs(base_src);
            for (job_name, base_needs) in &base_all_needs {
                let is_gate_or_rollup = settings.rollup_job.as_deref() == Some(job_name.as_str())
                    || job_name.contains("gate")
                    || job_name.contains("rollup");
                if is_gate_or_rollup && jobs.contains(job_name) {
                    let head_needs = head_all_needs.get(job_name).cloned().unwrap_or_default();
                    let dropped_needs: Vec<String> =
                        base_needs.difference(&head_needs).cloned().collect();
                    for dropped in dropped_needs {
                        if jobs.contains(&dropped) {
                            record_or_excuse(
                                ctx,
                                Some(&head_content),
                                &mut out,
                                settings.severity,
                                &crate::findings::ROLLUP_NEEDS_REMOVED,
                                Some(path.clone()),
                                find_line_number(&head_content, job_name),
                                format!("Rollup job '{job_name}' dropped dependency on '{dropped}' present in base."),
                                format!("Restore '{dropped}' to '{job_name}' needs, or excuse with allow-gate-weakening: ci-integrity <reason>."),
                                &dropped,
                            );
                        }
                    }
                }
            }
        }

        if let Some(ref rollup) = settings.rollup_job {
            if jobs.contains(rollup) {
                let expected: HashSet<String> = jobs
                    .iter()
                    .filter(|j| *j != rollup && !settings.excluded_jobs.contains(j))
                    .cloned()
                    .collect();
                let missing: Vec<String> = expected.difference(&rollup_needs).cloned().collect();

                if !missing.is_empty() {
                    record_or_excuse(
                        ctx,
                        Some(&head_content),
                        &mut out,
                        settings.severity,
                        &crate::findings::ROLLUP_NEEDS_INCOMPLETE,
                        Some(path.clone()),
                        find_line_number(&head_content, rollup),
                        format!("Rollup job '{rollup}' is missing dependencies on: {missing:?}"),
                        format!("Add the missing jobs to '{rollup}' needs: {missing:?}, or excuse with allow-gate-weakening: ci-integrity <reason>."),
                        rollup.as_str(),
                    );
                }

                // 2. Documented job count check if configured
                if let (Some(doc_path), Some(doc_pattern)) = (
                    &settings.documented_job_count_path,
                    &settings.documented_job_count_pattern,
                ) {
                    let doc_full = Path::new(ctx.git.root()).join(doc_path);
                    if !doc_full.is_file() {
                        out.violations.push(Violation {
                            gate: GATE,
                            severity: ctx.overridable(settings.severity),
                            code: crate::findings::full_code(
                                GATE,
                                &crate::findings::JOB_COUNT_FILE_MISSING,
                            ),
                            fingerprint: String::new(),
                            anchor: None,
                            legacy_title: None,
                            title: crate::findings::JOB_COUNT_FILE_MISSING.title.to_string(),
                            file: Some(doc_path.clone()),
                            line: None,
                            message: format!(
                                "Documented job count file '{doc_path}' does not exist."
                            ),
                            remediation: Some(
                                "Restore the documentation catalog or update configuration."
                                    .to_string(),
                            ),
                        });
                    } else if let Ok(doc_src) = std::fs::read_to_string(&doc_full) {
                        if let Ok(re) = Regex::new(doc_pattern) {
                            if let Some(caps) = re.captures(&doc_src) {
                                if let Ok(doc_count) = caps[1].parse::<usize>() {
                                    if doc_count != jobs.len() {
                                        out.violations.push(Violation {
                                            gate: GATE,
                                            severity: ctx.overridable(settings.severity),
                                            code: crate::findings::full_code(GATE, &crate::findings::JOB_COUNT_MISMATCH),
                                            fingerprint: String::new(),
                                            title: crate::findings::JOB_COUNT_MISMATCH.title.to_string(),
                                            anchor: None,
                                            legacy_title: crate::findings::JOB_COUNT_MISMATCH.was_title(),
                                            file: Some(doc_path.clone()),
                                            line: None,
                                            message: format!(
                                                "Documented job count in '{doc_path}' ({doc_count}) does not match workflow jobs count ({}).",
                                                jobs.len()
                                            ),
                                            remediation: Some(format!(
                                                "Update the documented count in '{doc_path}' to {} jobs.",
                                                jobs.len()
                                            )),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Workflow-level trigger and permission AST checks
        if let Some(head_doc) = &head_val {
            // Check pull_request_target
            let head_has_pr_target = workflow_has_trigger(head_doc, "pull_request_target");
            let base_has_pr_target = base_val
                .as_ref()
                .map(|b| workflow_has_trigger(b, "pull_request_target"))
                .unwrap_or(false);

            if head_has_pr_target && !base_has_pr_target {
                let line_no = find_line_number(&head_content, "pull_request_target");
                record_or_excuse(
                    ctx,
                    Some(&head_content),
                    &mut out,
                    settings.severity,
                    &crate::findings::PULL_REQUEST_TARGET_TRIGGER,
                    Some(path.clone()),
                    line_no,
                    "Workflow introduces 'pull_request_target' trigger, which executes with repository write access and secrets.".to_string(),
                    "Use 'pull_request' instead, or excuse with allow-gate-weakening: ci-integrity <reason>.",
                    "pull_request_target",
                );
            }

            // Check permissions widening
            if let Some(base_doc) = &base_val {
                let head_perm_level = workflow_permission_level(head_doc);
                let base_perm_level = workflow_permission_level(base_doc);
                if head_perm_level > base_perm_level {
                    let line_no = find_line_number(&head_content, "permissions:");
                    record_or_excuse(
                        ctx,
                        Some(&head_content),
                        &mut out,
                        settings.severity,
                        &crate::findings::WORKFLOW_PERMISSIONS_WIDENED,
                        Some(path.clone()),
                        line_no,
                        "Workflow permissions were widened from base ref (e.g. gained write privileges).".to_string(),
                        "Keep permissions minimal (e.g. read-all or specific read scopes), or excuse with allow-gate-weakening: ci-integrity <reason>.",
                        "permissions",
                    );
                }
            }

            // Check workflow-level timeout-minutes
            if let Some(base_doc) = &base_val {
                if base_doc.get("timeout-minutes").is_some()
                    && head_doc.get("timeout-minutes").is_none()
                {
                    record_or_excuse(
                        ctx,
                        Some(&head_content),
                        &mut out,
                        settings.severity,
                        &crate::findings::WORKFLOW_TIMEOUT_REMOVED,
                        Some(path.clone()),
                        None,
                        "Workflow-level 'timeout-minutes' was removed.".to_string(),
                        "Restore timeout-minutes or excuse with allow-gate-weakening: ci-integrity <reason>.",
                        "timeout-minutes",
                    );
                }
            }

            // 4. Job and step comparisons against base
            if let Some(base_doc) = &base_val {
                if let (Some(base_jobs_map), Some(head_jobs_map)) = (
                    base_doc.get("jobs").and_then(|j| j.as_mapping()),
                    head_doc.get("jobs").and_then(|j| j.as_mapping()),
                ) {
                    // Check for deleted verification jobs
                    for (job_k, job_v) in base_jobs_map {
                        let job_id = job_k.as_str().unwrap_or("");
                        if !head_jobs_map.contains_key(job_k) && is_verification_job(job_id, job_v)
                        {
                            if added_steps.is_none() {
                                added_steps = Some(added_job_steps(ctx, &workflow_globs, &filter)?);
                            }
                            if job_moved(job_v, added_steps.as_deref().unwrap_or(&[])) {
                                out.notes.push(format!(
                                    "{path}: job '{job_id}' was removed; its verification steps are in jobs this change added, so it is treated as a rename or split"
                                ));
                                continue;
                            }
                            record_or_excuse(
                                ctx,
                                Some(&head_content),
                                &mut out,
                                settings.severity,
                                &crate::findings::VERIFICATION_JOB_REMOVED,
                                Some(path.clone()),
                                None,
                                format!("Verification job '{job_id}' present in base was deleted."),
                                format!("Restore job '{job_id}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                                job_id,
                            );
                        }
                    }

                    // Compare surviving jobs
                    for (job_k, head_job_v) in head_jobs_map {
                        let job_id = job_k.as_str().unwrap_or("");
                        if let Some(base_job_v) = base_jobs_map.get(job_k) {
                            // Check job-level timeout-minutes
                            if base_job_v.get("timeout-minutes").is_some()
                                && head_job_v.get("timeout-minutes").is_none()
                            {
                                record_or_excuse(
                                    ctx,
                                    Some(&head_content),
                                    &mut out,
                                    settings.severity,
                                    &crate::findings::JOB_TIMEOUT_REMOVED,
                                    Some(path.clone()),
                                    find_line_number(&head_content, job_id),
                                    format!("Job '{job_id}' timeout-minutes was removed."),
                                    format!("Restore timeout-minutes to job '{job_id}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                                    job_id,
                                );
                            }

                            // Check deleted verification steps in this job
                            let base_steps = base_job_v
                                .get("steps")
                                .and_then(|s| s.as_sequence())
                                .cloned()
                                .unwrap_or_default();
                            let head_steps = head_job_v
                                .get("steps")
                                .and_then(|s| s.as_sequence())
                                .cloned()
                                .unwrap_or_default();

                            let pairs = pair_steps(&base_steps, &head_steps);
                            for (b_step, pair) in base_steps.iter().zip(&pairs) {
                                if !is_verification_step(b_step) {
                                    continue;
                                }
                                let step_name = step_label(b_step, "unnamed verification step");
                                match pair {
                                    Some(StepMatch::Same(_)) => {}
                                    Some(StepMatch::Renamed { head, similarity }) => {
                                        let new_name =
                                            step_label(&head_steps[*head], "unnamed step");
                                        out.notes.push(format!(
                                            "{path}: verification step '{step_name}' in job '{job_id}' was renamed to '{new_name}' (run body similarity {similarity:.2} >= rename threshold {STEP_RENAME_SIMILARITY:.2}); treated as a rename, not a deletion"
                                        ));
                                    }
                                    None => {
                                        let why = deletion_reason(b_step, &head_steps, &pairs);
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            &crate::findings::VERIFICATION_STEP_REMOVED,
                                            Some(path.clone()),
                                            find_line_number(&head_content, job_id),
                                            format!("Verification step '{step_name}' in job '{job_id}' was deleted: no step in head matches it by id, name, or run body ({why})."),
                                            format!("Restore step '{step_name}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                                            step_name,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 5. AST Step-by-Step and Job-level inspection
        if let Some(head_doc) = &head_val {
            if let Some(jobs_map) = head_doc.get("jobs").and_then(|j| j.as_mapping()) {
                let base_jobs_map = base_val
                    .as_ref()
                    .and_then(|b| b.get("jobs"))
                    .and_then(|j| j.as_mapping());

                for (job_k, job_v) in jobs_map {
                    let job_id = job_k.as_str().unwrap_or("");
                    let job_line = find_line_number(&head_content, &format!("{job_id}:"))
                        .or_else(|| find_line_number(&head_content, job_id));

                    // Job-level continue-on-error
                    if settings.forbid_continue_on_error
                        && job_v.get("continue-on-error").and_then(|c| c.as_bool()) == Some(true)
                    {
                        let base_had_it = base_jobs_map
                            .and_then(|m| m.get(job_k))
                            .and_then(|b| b.get("continue-on-error"))
                            .and_then(|c| c.as_bool())
                            == Some(true);
                        if !base_had_it {
                            let coe_line = find_line_after(
                                &head_content,
                                "continue-on-error",
                                job_line.unwrap_or(1),
                            )
                            .or(job_line);
                            record_or_excuse(
                                ctx,
                                Some(&head_content),
                                &mut out,
                                if is_verification_job(job_id, job_v) { settings.severity } else { Severity::Warning },
                                &crate::findings::JOB_FAILURE_MASKED_CONTINUE_ON_ERROR,
                                Some(path.clone()),
                                coe_line,
                                format!("Job '{job_id}' carries 'continue-on-error: true', which masks failures in CI."),
                                "Remove continue-on-error or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                                job_id,
                            );
                        }
                    }

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
                                let base_had_always = base_jobs_map
                                    .and_then(|m| m.get(job_k))
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
                                    let if_line = find_line_after(
                                        &head_content,
                                        "if:",
                                        job_line.unwrap_or(1),
                                    )
                                    .or(job_line);
                                    record_or_excuse(
                                        ctx,
                                        Some(&head_content),
                                        &mut out,
                                        settings.severity,
                                        &crate::findings::VERIFICATION_JOB_MASKED_BY_CONDITION,
                                        Some(path.clone()),
                                        if_line,
                                        format!("Verification job '{job_id}' carries 'if: {if_cond}', masking earlier pipeline failures."),
                                        "Remove conditional masking or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                        "if-always",
                                    );
                                }
                            }
                        }
                    }

                    let base_job_steps = base_jobs_map
                        .and_then(|m| m.get(job_k))
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
                        for (step_idx, step) in steps.iter().enumerate() {
                            let step_name = step.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let step_id = step.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            let uses_str = step.get("uses").and_then(|u| u.as_str());
                            let run_str = step.get("run").and_then(|r| r.as_str());

                            // Find matching base step
                            let base_step = base_job_steps
                                .and_then(|b_steps| {
                                    b_steps.iter().find(|b| steps_match_identity(b, step))
                                })
                                .or_else(|| renamed_from.get(&step_idx).copied());

                            let approx_line = find_step_line(
                                &head_content,
                                step_name,
                                step_id,
                                uses_str,
                                run_str,
                            );

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
                                    if policy_from(base_step.and_then(|b| b.get("with"))).as_deref()
                                        == Some("base")
                                        && policy_from(step.get("with")).as_deref() != Some("base")
                                    {
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            &crate::findings::DISCIPLINE_ACTION_POLICY_FROM,
                                            Some(path.clone()),
                                            approx_line,
                                            "The discipline step no longer sets 'policy_from: base'; the change would be judged by its own discipline.toml.".to_string(),
                                            "Restore 'policy_from: base' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                            "policy_from",
                                        );
                                    }

                                    if let Some(with_val) = step.get("with") {
                                        let base_with_val = base_step.and_then(|b| b.get("with"));

                                        // disable input added or expanded
                                        if let Some(disable_val) = with_val.get("disable") {
                                            let base_disable =
                                                base_with_val.and_then(|b| b.get("disable"));
                                            let is_new_or_expanded =
                                                match (base_disable, disable_val) {
                                                    (None, _) => true,
                                                    (Some(bv), hv) => bv != hv,
                                                };
                                            if is_new_or_expanded {
                                                record_or_excuse(
                                                    ctx,
                                                    Some(&head_content),
                                                    &mut out,
                                                    settings.severity,
                                                    &crate::findings::DISCIPLINE_ACTION_DISABLE_INPUT,
                                                    Some(path.clone()),
                                                    approx_line,
                                                    "The 'disable' input on the discipline step was added or widened, bypassing verification gates.".to_string(),
                                                    "Remove 'disable' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                    "disable",
                                                );
                                            }
                                        }

                                        // advisory switched on: the step reports and exits 0
                                        if advisory_input_on(Some(with_val))
                                            && !advisory_input_on(base_with_val)
                                        {
                                            record_or_excuse(
                                                ctx,
                                                Some(&head_content),
                                                &mut out,
                                                settings.severity,
                                                &crate::findings::DISCIPLINE_ACTION_ADVISORY,
                                                Some(path.clone()),
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
                                                        Some(&head_content),
                                                        &mut out,
                                                        settings.severity,
                                                        &crate::findings::DISCIPLINE_ACTION_FAIL_ON_WARNINGS_OFF,
                                                        Some(path.clone()),
                                                        approx_line,
                                                        "fail_on_warnings was set to false, suppressing warning-severity gate failures.".to_string(),
                                                        "Restore fail_on_warnings: true or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                        "fail_on_warnings",
                                                    );
                                                }
                                            }
                                        }

                                        // config_override pointing to non-existent or weakened files
                                        if let Some(cfg_ov) =
                                            with_val.get("config_override").and_then(|c| c.as_str())
                                        {
                                            let ov_file = Path::new(ctx.git.root()).join(cfg_ov);
                                            if !ov_file.is_file() {
                                                record_or_excuse(
                                                    ctx,
                                                    Some(&head_content),
                                                    &mut out,
                                                    settings.severity,
                                                    &crate::findings::DISCIPLINE_ACTION_CONFIG_OVERRIDE_INVALID,
                                                    Some(path.clone()),
                                                    approx_line,
                                                    format!("config_override points to non-existent file '{cfg_ov}'."),
                                                    "Provide a valid configuration file path.",
                                                    "config_override",
                                                );
                                            }
                                        }

                                        // suite narrowed
                                        if let Some(suite_val) =
                                            with_val.get("suite").and_then(|s| s.as_str())
                                        {
                                            let base_suite = base_with_val
                                                .and_then(|b| b.get("suite"))
                                                .and_then(|s| s.as_str());
                                            if let Some(bs) = base_suite {
                                                if bs != suite_val {
                                                    record_or_excuse(
                                                        ctx,
                                                        Some(&head_content),
                                                        &mut out,
                                                        settings.severity,
                                                        &crate::findings::DISCIPLINE_ACTION_SUITE_CHANGED,
                                                        Some(path.clone()),
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
                                            let base_ds = base_with_val
                                                .and_then(|b| b.get("directive_sources"));
                                            if base_ds.is_none() || base_ds != Some(ds_val) {
                                                let ds_str = serde_yaml::to_string(ds_val)
                                                    .unwrap_or_default();
                                                if ds_str.contains("commits") {
                                                    record_or_excuse(
                                                        ctx,
                                                        Some(&head_content),
                                                        &mut out,
                                                        settings.severity,
                                                        &crate::findings::DISCIPLINE_ACTION_DIRECTIVE_SOURCES_WIDENED,
                                                        Some(path.clone()),
                                                        approx_line,
                                                        "directive_sources was widened to accept directives from commit messages.".to_string(),
                                                        "Keep directive sources restricted, or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                        "directive_sources",
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // 5b'. `discipline check --advisory` in a run command
                            if let Some(head_run) = run_str {
                                let base_run = base_step
                                    .and_then(|b| b.get("run"))
                                    .and_then(|r| r.as_str())
                                    .unwrap_or("");
                                if run_is_advisory(head_run) && !run_is_advisory(base_run) {
                                    record_or_excuse(
                                        ctx,
                                        Some(&head_content),
                                        &mut out,
                                        settings.severity,
                                        &crate::findings::DISCIPLINE_RUN_ADVISORY,
                                        Some(path.clone()),
                                        approx_line,
                                        "A discipline command gained '--advisory'; it exits 0 whatever the gates report.".to_string(),
                                        "Remove '--advisory' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                        "--advisory",
                                    );
                                }
                            }

                            // 5c. Dropping build/clippy flags from run commands
                            if let Some(head_run) = run_str {
                                if let Some(base_run) = base_step
                                    .and_then(|b| b.get("run"))
                                    .and_then(|r| r.as_str())
                                {
                                    if base_run.contains("-D warnings")
                                        && !head_run.contains("-D warnings")
                                    {
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            &crate::findings::COMPILER_DENY_WARNINGS_REMOVED,
                                            Some(path.clone()),
                                            approx_line,
                                            "Step dropped '-D warnings' from command, allowing compiler/linter warnings to pass.".to_string(),
                                            "Restore '-D warnings' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                            "-D warnings",
                                        );
                                    }
                                    if base_run.contains("--locked")
                                        && !head_run.contains("--locked")
                                    {
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            &crate::findings::CARGO_LOCKED_REMOVED,
                                            Some(path.clone()),
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
                                                Some(&head_content),
                                                &mut out,
                                                settings.severity,
                                                &crate::findings::FROZEN_INSTALL_FLAG_REMOVED,
                                                Some(path.clone()),
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
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            &crate::findings::INSTALL_COMMAND_WEAKENED,
                                            Some(path.clone()),
                                            approx_line,
                                            "Step replaced 'npm ci' with 'npm install': the install may rewrite the lockfile instead of honouring it.".to_string(),
                                            "Restore 'npm ci' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                            "npm ci",
                                        );
                                    }
                                    if base_run.contains("--all-targets")
                                        && !head_run.contains("--all-targets")
                                    {
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            &crate::findings::CLIPPY_ALL_TARGETS_REMOVED,
                                            Some(path.clone()),
                                            approx_line,
                                            "Step dropped '--all-targets' from clippy command, skipping linting on tests/benchmarks.".to_string(),
                                            "Restore '--all-targets' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                            "--all-targets",
                                        );
                                    }
                                }
                            }

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
                                            let if_line = find_line_after(
                                                &head_content,
                                                "if:",
                                                approx_line.unwrap_or(1),
                                            )
                                            .or(approx_line);
                                            record_or_excuse(
                                                ctx,
                                                Some(&head_content),
                                                &mut out,
                                                settings.severity,
                                                &crate::findings::VERIFICATION_STEP_MASKED_BY_CONDITION,
                                                Some(path.clone()),
                                                if_line,
                                                format!("Verification step now carries 'if: {if_cond}': it runs only after an earlier failure, so a passing build no longer runs it."),
                                                "Remove conditional masking or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                "if-always",
                                            );
                                        }
                                    }
                                }
                            }

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
                                        let if_line = find_line_after(
                                            &head_content,
                                            "if:",
                                            approx_line.unwrap_or(1),
                                        )
                                        .or(approx_line);
                                        let what = match &base_if {
                                            None => format!("gains 'if: {h}'"),
                                            Some(b) => format!("changes 'if: {b}' to 'if: {h}'"),
                                        };
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            Severity::Warning,
                                            &crate::findings::VERIFICATION_STEP_NARROWED,
                                            Some(path.clone()),
                                            if_line,
                                            format!("Verification step '{}' in job '{job_id}' {what}: it no longer runs on every event or condition it ran on before.", step_label(step, "unnamed step")),
                                            "Run the step unconditionally, or record the narrowing with allow-gate-weakening: ci-integrity <reason>. A discipline step restricted to pull_request stops gating pushes to the default branch; the `merged-pr-body` directive source is the alternative when PR-body waivers are the reason.",
                                            "if-narrowed",
                                        );
                                    }
                                }
                            }

                            // 5e. continue-on-error. In a job that verifies nothing (a summary,
                            // a report) no check is masked: a warning.
                            let verifies = is_verification_job(job_id, job_v);
                            let coe_severity = if verifies {
                                settings.severity
                            } else {
                                Severity::Warning
                            };
                            if settings.forbid_continue_on_error
                                && step.get("continue-on-error").and_then(|c| c.as_bool())
                                    == Some(true)
                            {
                                let base_had_it = base_step
                                    .and_then(|b| b.get("continue-on-error"))
                                    .and_then(|c| c.as_bool())
                                    == Some(true);
                                if !base_had_it {
                                    let coe_line = find_line_after(
                                        &head_content,
                                        "continue-on-error",
                                        approx_line.unwrap_or(1),
                                    )
                                    .or(approx_line);
                                    let step_subject = step
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .or_else(|| step.get("id").and_then(|i| i.as_str()))
                                        .unwrap_or("continue-on-error");
                                    record_or_excuse(
                                        ctx,
                                        Some(&head_content),
                                        &mut out,
                                        coe_severity,
                                        &crate::findings::STEP_FAILURE_MASKED_CONTINUE_ON_ERROR,
                                        Some(path.clone()),
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

                            // 5f. Error suppression: || true / set +e
                            if settings.forbid_or_true {
                                if let Some(run_cmd) = run_str {
                                    if masks_exit_code(run_cmd) {
                                        let base_had_mask = base_step
                                            .and_then(|b| b.get("run"))
                                            .and_then(|r| r.as_str())
                                            .is_some_and(masks_exit_code);
                                        if !base_had_mask {
                                            let mask_line = find_line_after(
                                                &head_content,
                                                "|| true",
                                                approx_line.unwrap_or(1),
                                            )
                                            .or_else(|| {
                                                find_line_after(
                                                    &head_content,
                                                    "set +e",
                                                    approx_line.unwrap_or(1),
                                                )
                                            })
                                            .or(approx_line);
                                            let step_subject = step
                                                .get("name")
                                                .and_then(|n| n.as_str())
                                                .or_else(|| step.get("id").and_then(|i| i.as_str()))
                                                .unwrap_or("or-true");
                                            record_or_excuse(
                                                ctx,
                                                Some(&head_content),
                                                &mut out,
                                                settings.severity,
                                                &crate::findings::EXIT_CODE_MASKED,
                                                Some(path.clone()),
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
                    }
                }
            }
        }
    }

    Ok(out)
}

/// Minimum run-body similarity (Dice coefficient over whitespace tokens) for a
/// base step with no id/name match to be paired with an unmatched head step as
/// a rename.
pub(crate) const STEP_RENAME_SIMILARITY: f64 = 0.6;

/// How a base step was found in the head workflow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StepMatch {
    /// Matched by id, name, action, or first run line.
    Same(usize),
    /// No identity match; paired by run-body similarity.
    Renamed { head: usize, similarity: f64 },
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

/// Whitespace tokens of what a step executes: its `run:` script (comment
/// lines excluded), or its action (ref stripped) and `with:` inputs. Empty
/// when the step has neither.
fn step_body_tokens(step: &serde_yaml::Value) -> Vec<String> {
    if let Some(run) = executable_run(step) {
        return run.split_whitespace().map(str::to_string).collect();
    }
    let mut tokens = Vec::new();
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        tokens.push(format!("uses:{}", uses.split('@').next().unwrap_or(uses)));
        if let Some(with) = step.get("with").and_then(|w| w.as_mapping()) {
            for (k, v) in with {
                let v = serde_yaml::to_string(v).unwrap_or_default();
                tokens.push(format!("{}={}", k.as_str().unwrap_or(""), v.trim()));
            }
        }
    }
    tokens
}

/// Dice coefficient over the multisets of body tokens of two steps:
/// `2 * |A ∩ B| / (|A| + |B|)`, in `[0, 1]`. A `run:` step and a `uses:`
/// step never share tokens. Two empty bodies score 0: no evidence either way.
pub(crate) fn step_body_similarity(a: &serde_yaml::Value, b: &serde_yaml::Value) -> f64 {
    let ta = step_body_tokens(a);
    let tb = step_body_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in &ta {
        *counts.entry(t.as_str()).or_default() += 1;
    }
    let mut common = 0usize;
    for t in &tb {
        if let Some(c) = counts.get_mut(t.as_str()) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    (2 * common) as f64 / (ta.len() + tb.len()) as f64
}

/// Pairs each base step with the head step it became.
///
/// Pass 1 matches by identity (id, name, action, first run line), as before.
/// Pass 2 takes every base step still unmatched and pairs it with the head
/// step, not matched in pass 1 and not already taken, whose body is most
/// similar, provided the similarity reaches [`STEP_RENAME_SIMILARITY`] and
/// the head step still carries every verification marker (`test`, `clippy`,
/// `lint`, ...) the base step's body carried; ties go to the head step
/// closest in position. A step whose name and body both changed past the
/// threshold, or whose body stopped verifying, stays unpaired, i.e. deleted.
pub(crate) fn pair_steps(
    base: &[serde_yaml::Value],
    head: &[serde_yaml::Value],
) -> Vec<Option<StepMatch>> {
    let mut pairs: Vec<Option<StepMatch>> = base
        .iter()
        .map(|b| {
            head.iter()
                .position(|h| steps_match_identity(b, h))
                .map(StepMatch::Same)
        })
        .collect();
    let mut taken: Vec<bool> = head
        .iter()
        .map(|h| base.iter().any(|b| steps_match_identity(b, h)))
        .collect();

    // Best candidates first, so a strong rename is not pre-empted by a weak one.
    let mut candidates: Vec<(f64, usize, usize, usize)> = Vec::new();
    for (bi, b) in base.iter().enumerate() {
        if pairs[bi].is_some() {
            continue;
        }
        for (hi, h) in head.iter().enumerate() {
            if taken[hi] {
                continue;
            }
            let sim = step_body_similarity(b, h);
            // A rename keeps what the step verifies: `cargo test` renamed and
            // turned into `cargo build` is a deletion however similar the rest.
            if sim >= STEP_RENAME_SIMILARITY
                && verifying_body_markers(b).is_subset(&verifying_body_markers(h))
            {
                candidates.push((sim, bi.abs_diff(hi), bi, hi));
            }
        }
    }
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
    for (sim, _, bi, hi) in candidates {
        if pairs[bi].is_none() && !taken[hi] {
            pairs[bi] = Some(StepMatch::Renamed {
                head: hi,
                similarity: sim,
            });
            taken[hi] = true;
        }
    }
    pairs
}

/// Why no head step was accepted as the rename of `b_step`: the closest
/// unmatched head step and the rule that refused it.
fn deletion_reason(
    b_step: &serde_yaml::Value,
    head_steps: &[serde_yaml::Value],
    pairs: &[Option<StepMatch>],
) -> String {
    let claimed = |hi: usize| {
        pairs.iter().any(|p| match p {
            Some(StepMatch::Same(h)) => *h == hi,
            Some(StepMatch::Renamed { head, .. }) => *head == hi,
            None => false,
        })
    };
    let closest = head_steps
        .iter()
        .enumerate()
        .filter(|(hi, _)| !claimed(*hi))
        .map(|(_, h)| (step_body_similarity(b_step, h), h))
        .max_by(|x, y| x.0.total_cmp(&y.0));
    match closest {
        Some((sim, h)) if sim >= STEP_RENAME_SIMILARITY => {
            let kept = verifying_body_markers(h);
            let mut lost: Vec<&str> = verifying_body_markers(b_step)
                .into_iter()
                .filter(|m| !kept.contains(m))
                .collect();
            lost.sort_unstable();
            let lost: Vec<String> = lost.iter().map(|m| format!("`{m}`")).collect();
            format!(
                "the closest unmatched head step, '{}', has run body similarity {sim:.2}, but no longer carries {} from the deleted step's body, so it is not a rename",
                step_label(h, "unnamed step"),
                lost.join(", ")
            )
        }
        Some((sim, h)) => format!(
            "the closest unmatched head step, '{}', has run body similarity {sim:.2}, below the rename threshold {STEP_RENAME_SIMILARITY:.2}",
            step_label(h, "unnamed step")
        ),
        None => "no unmatched head step remains to be a rename of it".to_string(),
    }
}

/// Steps of the jobs this change added, in every workflow file in head: where a
/// removed job's steps may have moved (a rename, a split, a move to another file).
fn added_job_steps(
    ctx: &Context,
    globs: &globset::GlobSet,
    filter: &crate::guards::PathFilter,
) -> Result<Vec<serde_yaml::Value>> {
    let mut paths = ctx.git.tracked_files()?;
    paths.extend(ctx.git.changed_files()?.into_iter().map(|f| f.path));
    paths.sort();
    paths.dedup();
    let mut steps = Vec::new();
    for p in paths.iter().filter(|p| {
        globs.is_match(p) && !filter.matches(p) && !super::ci_gitlab::is_gitlab_ci_path(p)
    }) {
        let Some(head) = ctx.git.head_content(p)? else {
            continue;
        };
        let Ok(head) = serde_yaml::from_str::<serde_yaml::Value>(&head) else {
            continue;
        };
        let base_jobs: HashSet<String> = ctx
            .git
            .base_content(p)
            .ok()
            .flatten()
            .and_then(|b| serde_yaml::from_str::<serde_yaml::Value>(&b).ok())
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

/// Whether every verification step of a removed job (every step, when none is a
/// verification step) reappears in a job this change added: the job was renamed,
/// split, or moved, not deleted. Steps pair by body alone (similarity at least
/// [`STEP_RENAME_SIMILARITY`], keeping every verification marker), never by name,
/// and each added step takes one removed step, so an added job that reuses the
/// names over emptied bodies does not count.
pub(crate) fn job_moved(job: &serde_yaml::Value, added: &[serde_yaml::Value]) -> bool {
    let steps: Vec<&serde_yaml::Value> = job
        .get("steps")
        .and_then(|s| s.as_sequence())
        .map(|s| s.iter().collect())
        .unwrap_or_default();
    let verifying: Vec<&serde_yaml::Value> = steps
        .iter()
        .copied()
        .filter(|s| is_verification_step(s))
        .collect();
    let wanted = if verifying.is_empty() {
        steps
    } else {
        verifying
    };
    if wanted.is_empty() {
        return false;
    }
    let mut taken = vec![false; added.len()];
    wanted.iter().all(|w| {
        let markers = verifying_body_markers(w);
        let best = added
            .iter()
            .enumerate()
            .filter(|(i, a)| !taken[*i] && markers.is_subset(&verifying_body_markers(a)))
            .map(|(i, a)| (step_body_similarity(w, a), i))
            .filter(|(sim, _)| *sim >= STEP_RENAME_SIMILARITY)
            .max_by(|x, y| x.0.total_cmp(&y.0));
        match best {
            Some((_, i)) => {
                taken[i] = true;
                true
            }
            None => false,
        }
    })
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

/// Diff one GitLab pipeline file against its base side. A side that does not parse is a
/// finding: an unreadable pipeline cannot be shown to be unweakened.
fn evaluate_gitlab_file(ctx: &Context, path: &str, out: &mut GateOutcome) -> Result<()> {
    use super::ci_gitlab::verification_jobs;
    let settings = &ctx.config.gates.ci_integrity;
    let base = ctx.git.base_content(path)?;
    let Some(head) = ctx.git.head_content(path)? else {
        if let Some(jobs) = base.as_deref().and_then(|b| verification_jobs(b).ok()) {
            if !jobs.is_empty() {
                record_or_excuse(
                    ctx,
                    None,
                    out,
                    settings.severity,
                    &crate::findings::VERIFICATION_WORKFLOW_DELETED,
                    Some(path.to_string()),
                    None,
                    format!(
                        "Pipeline '{path}' defining verification job(s) {} was deleted.",
                        jobs.join(", ")
                    ),
                    "Restore the pipeline or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                    path,
                );
            }
        }
        return Ok(());
    };
    out.examined += 1;
    // Local includes are followed on each side; other include kinds are named.
    let head_inc = super::ci_gitlab::includes(&head);
    let mut head_docs = vec![head.clone()];
    for local in &head_inc.local {
        match ctx.git.head_content(local)? {
            Some(c) => head_docs.push(c),
            None => out.notes.push(format!(
                "`{path}`: included file `{local}` is not in the tree; not read"
            )),
        }
    }
    for nf in &head_inc.not_followed {
        out.notes.push(format!(
            "`{path}`: include `{nf}` is not read (only local includes are followed)"
        ));
    }
    // A new pipeline file has nothing to be weakened against.
    let Some(base) = base else {
        return Ok(());
    };
    let mut base_docs = vec![base.clone()];
    for local in &super::ci_gitlab::includes(&base).local {
        if let Some(c) = ctx.git.base_content(local)? {
            base_docs.push(c);
        }
    }
    let pins = |docs: &[String]| -> Vec<DisciplinePin> {
        let mut out = Vec::new();
        for d in docs {
            // A document that does not parse ends the read: the deserializer does not
            // advance past it (the diff below reports the file as unreadable).
            for doc in serde_yaml::Deserializer::from_str(d) {
                let Ok(v) = <serde_yaml::Value as serde::Deserialize>::deserialize(doc) else {
                    break;
                };
                out.extend(discipline_pins(&v));
            }
        }
        out
    };
    let (blocking, notes) = discipline_pin_changes(&pins(&base_docs), &pins(&head_docs));
    for n in notes {
        out.notes.push(format!("`{path}`: discipline pin {n}"));
    }
    if !blocking.is_empty() {
        record_or_excuse(
            ctx,
            Some(&head),
            out,
            settings.severity,
            &crate::findings::DISCIPLINE_VERSION_CHANGED,
            Some(path.to_string()),
            find_line_number(&head, "discipline"),
            format!(
                "The discipline that judges this change is chosen by the change: {}.",
                blocking.join("; ")
            ),
            "Keep the discipline pin, or move it to a newer immutable release; excuse with allow-gate-weakening: ci-integrity <reason>.",
            "discipline-version",
        );
    }
    match super::ci_gitlab::diff_gitlab_ci_with(&base_docs, &head_docs) {
        Ok(found) => {
            for w in found {
                let line = find_line_number(&head, &format!("{}:", w.job));
                record_or_excuse(
                    ctx,
                    Some(&head),
                    out,
                    settings.severity,
                    w.kind,
                    Some(path.to_string()),
                    line,
                    w.message,
                    format!(
                        "Revert it, or excuse with allow-gate-weakening: ci-integrity <reason> naming `{}`.",
                        w.subject
                    ),
                    &w.subject,
                );
            }
        }
        Err(e) => out.push(
            settings.severity,
            &crate::findings::PIPELINE_FILE_UNREADABLE,
            Some(path),
            None,
            format!("`{path}` could not be compared with its base side ({e})."),
            "Fix the YAML so the pipeline can be checked.",
        ),
    }
    Ok(())
}

/// A value that chooses which discipline binary a pipeline runs: the action's `uses:`
/// (with its ref), its `version`, `binary` and `download_url` inputs, a job container or
/// image, and GitLab includes of the template or component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisciplinePin {
    pub what: &'static str,
    pub value: String,
}

fn names_discipline(s: &str) -> bool {
    s.to_ascii_lowercase().contains("discipline")
}

/// The discipline pins of one workflow or pipeline document, in document order.
pub fn discipline_pins(doc: &serde_yaml::Value) -> Vec<DisciplinePin> {
    let mut pins = Vec::new();
    let mut push = |what: &'static str, v: &str| {
        pins.push(DisciplinePin {
            what,
            value: v.trim().to_string(),
        })
    };
    let image_of = |v: &serde_yaml::Value| -> Option<String> {
        v.as_str()
            .or_else(|| v.get("image").and_then(|i| i.as_str()))
            .or_else(|| v.get("name").and_then(|i| i.as_str()))
            .map(str::to_string)
    };
    // GitLab includes.
    if let Some(inc) = doc.get("include") {
        let items: Vec<&serde_yaml::Value> = match inc {
            serde_yaml::Value::Sequence(s) => s.iter().collect(),
            other => vec![other],
        };
        for item in items {
            for k in ["remote", "component"] {
                if let Some(v) = item.get(k).and_then(|v| v.as_str()) {
                    if names_discipline(v) {
                        push("include", v);
                    }
                }
            }
            if let Some(p) = item.get("project").and_then(|v| v.as_str()) {
                if names_discipline(p) {
                    let r = item.get("ref").and_then(|r| r.as_str()).unwrap_or("");
                    push("include", &format!("{p}@{r}"));
                }
            }
            if let Some(img) = item.pointer_image() {
                push("image", &img);
            }
        }
    }
    let Some(map) = doc.as_mapping() else {
        return pins;
    };
    // Actions jobs live under `jobs:`; GitLab jobs are top-level keys.
    let jobs: Vec<&serde_yaml::Value> = match doc.get("jobs").and_then(|j| j.as_mapping()) {
        Some(j) => j.values().collect(),
        None => map.values().filter(|v| v.is_mapping()).collect(),
    };
    for job in jobs {
        for key in ["container", "image"] {
            if let Some(img) = job.get(key).and_then(image_of) {
                if names_discipline(&img) {
                    push("image", &img);
                }
            }
        }
        for step in job
            .get("steps")
            .and_then(|s| s.as_sequence())
            .into_iter()
            .flatten()
        {
            let Some(uses) = step.get("uses").and_then(|u| u.as_str()) else {
                continue;
            };
            if !(names_discipline(uses) || uses == "./action.yml" || uses == "./") {
                continue;
            }
            push("uses", uses);
            for input in ["version", "binary", "download_url"] {
                if let Some(v) = step.get("with").and_then(|w| w.get(input)) {
                    let v = match v {
                        serde_yaml::Value::String(s) => s.clone(),
                        other => serde_yaml::to_string(other).unwrap_or_default(),
                    };
                    push(
                        match input {
                            "version" => "version",
                            "binary" => "binary",
                            _ => "download_url",
                        },
                        &v,
                    );
                }
            }
        }
    }
    pins
}

trait PointerImage {
    fn pointer_image(&self) -> Option<String>;
}

impl PointerImage for serde_yaml::Value {
    /// `include: [{ ..., inputs: { image: ... } }]`, when it names discipline.
    fn pointer_image(&self) -> Option<String> {
        self.get("inputs")
            .and_then(|i| i.get("image"))
            .and_then(|i| i.as_str())
            .filter(|s| names_discipline(s))
            .map(str::to_string)
    }
}

/// The version part of a pin: an action or component ref (`@v0.14.4`), an image tag
/// (`:v0.14.4`, a digest dropped), a release segment of a template URL, or the value.
fn pin_version(p: &DisciplinePin) -> String {
    let v = p.value.as_str();
    match p.what {
        "uses" => v.rsplit_once('@').map(|(_, r)| r).unwrap_or("").to_string(),
        "image" => {
            let v = v.split('@').next().unwrap_or(v);
            let last = v.rsplit('/').next().unwrap_or(v);
            last.rsplit_once(':')
                .map(|(_, t)| t)
                .unwrap_or("latest")
                .to_string()
        }
        "include" => {
            if let Some((_, r)) = v.rsplit_once('@') {
                return r.to_string();
            }
            v.split('/')
                .find(|seg| release(seg).is_some())
                .unwrap_or("")
                .to_string()
        }
        _ => v.to_string(),
    }
}

/// `v1.2.3` / `1.2.3` as numbers, with how many parts were given (a `v0` tag moves).
fn release(v: &str) -> Option<(Vec<u64>, usize)> {
    let t = v.trim().trim_start_matches(['v', 'V']);
    let parts: Option<Vec<u64>> = t.split('.').map(|p| p.parse().ok()).collect();
    let parts = parts.filter(|p| !p.is_empty() && p.len() <= 3)?;
    let n = parts.len();
    Some((parts, n))
}

fn is_digest(v: &str) -> bool {
    (v.len() == 40 && v.chars().all(|c| c.is_ascii_hexdigit())) || v.starts_with("sha256:")
}

/// A ref another push can move: a branch, `latest`, a major or minor tag (`v0`), none.
fn is_mutable(v: &str) -> bool {
    match release(v) {
        Some((_, n)) => n < 3,
        None => !is_digest(v),
    }
}

/// How a pin changed between base and head: `Some(true)` blocks (a downgrade, a move to
/// a ref that can move, a new binary source), `Some(false)` is a note (an upgrade, a
/// digest bump), `None` is no change.
pub fn pin_change(what: &str, base: Option<&DisciplinePin>, head: &DisciplinePin) -> Option<bool> {
    if base.is_some_and(|b| b.value == head.value) {
        return None;
    }
    if what == "binary" || what == "download_url" {
        return Some(true);
    }
    let base = base?;
    let (b, h) = (pin_version(base), pin_version(head));
    if b == h {
        // Same version, another location or digest: not a version move.
        return Some(false);
    }
    if is_mutable(&h) && !is_mutable(&b) {
        return Some(true);
    }
    match (release(&b), release(&h)) {
        (Some((bv, 3)), Some((hv, 3))) => Some(hv < bv),
        _ => Some(false),
    }
}

/// Compare the discipline pins of a file's base and head: blocking changes and notes.
pub fn discipline_pin_changes(
    base: &[DisciplinePin],
    head: &[DisciplinePin],
) -> (Vec<String>, Vec<String>) {
    let (mut blocking, mut notes) = (Vec::new(), Vec::new());
    for what in [
        "uses",
        "version",
        "binary",
        "download_url",
        "image",
        "include",
    ] {
        let b: Vec<&DisciplinePin> = base.iter().filter(|p| p.what == what).collect();
        let h: Vec<&DisciplinePin> = head.iter().filter(|p| p.what == what).collect();
        for (i, hp) in h.iter().enumerate() {
            let bp = b.get(i).copied();
            let describe = || match bp {
                Some(bp) => format!("`{what}` changed from `{}` to `{}`", bp.value, hp.value),
                None => format!("`{what}` set to `{}`", hp.value),
            };
            match pin_change(what, bp, hp) {
                Some(true) => blocking.push(describe()),
                Some(false) => notes.push(describe()),
                None => {}
            }
        }
    }
    (blocking, notes)
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

fn workflow_has_trigger(val: &serde_yaml::Value, trigger: &str) -> bool {
    if let Some(on_val) = val.get("on") {
        if let Some(s) = on_val.as_str() {
            return s == trigger;
        }
        if let Some(seq) = on_val.as_sequence() {
            return seq.iter().any(|item| item.as_str() == Some(trigger));
        }
        if on_val.get(trigger).is_some() {
            return true;
        }
    }
    false
}

fn workflow_permission_level(val: &serde_yaml::Value) -> u8 {
    if let Some(perm) = val.get("permissions") {
        if let Some(s) = perm.as_str() {
            if s == "write-all" {
                return 2;
            }
            if s == "read-all" {
                return 1;
            }
        }
        if let Some(map) = perm.as_mapping() {
            if map.is_empty() {
                return 0;
            }
            let has_write = map.values().any(|v| v.as_str() == Some("write"));
            if has_write {
                return 2;
            }
            return 1;
        }
    } else {
        // Omitting permissions defaults to permissive / repository-default (which includes write access)
        return 2;
    }
    0
}

fn find_line_number(content: &str, needle: &str) -> Option<usize> {
    for (idx, line) in content.lines().enumerate() {
        if line.contains(needle) {
            return Some(idx + 1);
        }
    }
    None
}

fn find_line_after(content: &str, needle: &str, start_line: usize) -> Option<usize> {
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no >= start_line && line.contains(needle) {
            return Some(line_no);
        }
    }
    None
}

fn find_step_line(
    content: &str,
    name: &str,
    id: &str,
    uses: Option<&str>,
    run: Option<&str>,
) -> Option<usize> {
    if !name.is_empty() {
        if let Some(l) = find_line_number(content, name) {
            return Some(l);
        }
    }
    if !id.is_empty() {
        if let Some(l) = find_line_number(content, &format!("id: {id}")) {
            return Some(l);
        }
    }
    if let Some(u) = uses {
        if let Some(l) = find_line_number(content, u) {
            return Some(l);
        }
    }
    if let Some(r) = run {
        let first = r.lines().next().unwrap_or("").trim();
        if !first.is_empty() {
            if let Some(l) = find_line_number(content, first) {
                return Some(l);
            }
        }
    }
    None
}

/// What a pinned reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PinKind {
    /// A step's `uses:` action (`owner/repo@ref`), in a workflow or a composite action.
    Action,
    /// A job-level `uses:` calling a reusable workflow (`owner/repo/.github/workflows/x.yml@ref`).
    ReusableWorkflow,
    /// A container image: `container:`, `services.<name>.image` or `uses: docker://`.
    Image,
}

/// One remote reference a workflow or composite action pulls in, and where it is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PinRef {
    pub kind: PinKind,
    /// The reference as written (`docker://alpine:latest`, `node:20`, `owner/repo@v1`).
    pub value: String,
    /// Where it sits, for the message (`job 'build' container`).
    pub site: String,
    /// 1-based line of the reference in the file, when found.
    pub line: Option<usize>,
    /// The step's own line (its name or id), whose inline exemption also covers the
    /// reference, as it always has for step-level `uses:`.
    pub step_line: Option<usize>,
}

/// How a reference fares against the pinning rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinVerdict {
    /// Pinned, local, first-party, or not a remote reference: nothing to report.
    Pinned,
    /// A mutable reference: an action or reusable workflow not at a 40-hex commit SHA, or
    /// an image without an `@sha256:<64 hex>` digest.
    Unpinned,
    /// An expression (`${{ matrix.image }}`) the gate cannot resolve: a note, never a pass.
    Expression,
}

/// The image name of a `docker://` reference, or `None` for any other `uses:`.
fn docker_image(uses: &str) -> Option<&str> {
    uses.strip_prefix("docker://")
}

/// Whether an image reference carries an immutable `@sha256:<64 hex>` digest.
pub(crate) fn image_has_digest(image: &str) -> bool {
    image.rsplit_once("@sha256:").is_some_and(|(name, digest)| {
        !name.is_empty() && digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit())
    })
}

/// Whether a git ref is a full 40-character commit SHA.
fn is_commit_sha(r: &str) -> bool {
    r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit())
}

/// Judges one reference against the pinning rule. `first_party` prefixes exempt an
/// action or a reusable workflow; local (`./`) references are exempt.
pub(crate) fn pin_verdict(kind: PinKind, value: &str, first_party: &[String]) -> PinVerdict {
    let value = value.trim();
    if value.is_empty() {
        return PinVerdict::Pinned;
    }
    if value.contains("${{") {
        return PinVerdict::Expression;
    }
    match kind {
        PinKind::Image => {
            if image_has_digest(docker_image(value).unwrap_or(value)) {
                PinVerdict::Pinned
            } else {
                PinVerdict::Unpinned
            }
        }
        PinKind::Action | PinKind::ReusableWorkflow => {
            if value.starts_with("./") {
                return PinVerdict::Pinned;
            }
            let Some((target, r)) = value.split_once('@') else {
                return PinVerdict::Pinned;
            };
            if first_party.iter().any(|p| target.starts_with(p.as_str())) || is_commit_sha(r) {
                PinVerdict::Pinned
            } else {
                PinVerdict::Unpinned
            }
        }
    }
}

/// The line of a YAML mapping key (`key:` at the start of a line, optionally quoted)
/// at or after `start`.
fn find_key_line(content: &str, key: &str, start: usize) -> Option<usize> {
    let spellings = [key.to_string(), format!("'{key}'"), format!("\"{key}\"")];
    content.lines().enumerate().find_map(|(idx, line)| {
        let t = line.trim_start();
        let hit = spellings.iter().any(|k| {
            t.strip_prefix(k.as_str())
                .and_then(|r| r.strip_prefix(':'))
                .is_some_and(|r| r.is_empty() || r.starts_with([' ', '\t']))
        });
        (idx + 1 >= start && hit).then_some(idx + 1)
    })
}

/// A step's `uses:`, as an action or, for `docker://`, an image. `cursor` is the line
/// the search starts from; it moves past each reference found, so a reference repeated
/// in later steps is located at its own line.
fn step_ref(
    step: &serde_yaml::Value,
    site: String,
    content: &str,
    cursor: &mut usize,
) -> Option<PinRef> {
    let uses = step.get("uses").and_then(|u| u.as_str())?;
    let name = step.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let id = step.get("id").and_then(|i| i.as_str()).unwrap_or("");
    let run = step.get("run").and_then(|r| r.as_str());
    let line = find_line_after(content, uses, *cursor);
    if let Some(l) = line {
        *cursor = l + 1;
    }
    Some(PinRef {
        kind: if docker_image(uses).is_some() {
            PinKind::Image
        } else {
            PinKind::Action
        },
        value: uses.to_string(),
        site,
        line,
        step_line: find_step_line(content, name, id, Some(uses), run),
    })
}

/// Every remote reference of a workflow: step and job-level `uses:`, `container:`
/// (string or `image:`) and `services.<name>.image`.
pub(crate) fn workflow_pin_refs(doc: &serde_yaml::Value, content: &str) -> Vec<PinRef> {
    let mut refs = Vec::new();
    let Some(jobs) = doc.get("jobs").and_then(|j| j.as_mapping()) else {
        return refs;
    };
    let jobs_line = find_key_line(content, "jobs", 1).unwrap_or(1);
    for (job_k, job) in jobs {
        let job_id = job_k.as_str().unwrap_or("");
        let job_line = find_key_line(content, job_id, jobs_line).unwrap_or(jobs_line);
        let at = |value: &str| find_line_after(content, value, job_line);
        if let Some(uses) = job.get("uses").and_then(|u| u.as_str()) {
            refs.push(PinRef {
                kind: PinKind::ReusableWorkflow,
                value: uses.to_string(),
                site: format!("job '{job_id}' reusable workflow"),
                line: at(uses),
                step_line: None,
            });
        }
        let container = match job.get("container") {
            Some(serde_yaml::Value::String(s)) => Some(s.as_str()),
            Some(m) => m.get("image").and_then(|i| i.as_str()),
            None => None,
        };
        if let Some(image) = container {
            refs.push(PinRef {
                kind: PinKind::Image,
                value: image.to_string(),
                site: format!("job '{job_id}' container"),
                line: at(image),
                step_line: None,
            });
        }
        if let Some(services) = job.get("services").and_then(|s| s.as_mapping()) {
            for (svc_k, svc) in services {
                let svc_id = svc_k.as_str().unwrap_or("");
                if let Some(image) = svc.get("image").and_then(|i| i.as_str()) {
                    refs.push(PinRef {
                        kind: PinKind::Image,
                        value: image.to_string(),
                        site: format!("job '{job_id}' service '{svc_id}'"),
                        line: at(image),
                        step_line: None,
                    });
                }
            }
        }
        if let Some(steps) = job.get("steps").and_then(|s| s.as_sequence()) {
            let mut cursor = job_line;
            for step in steps {
                let site = format!("job '{job_id}' step");
                if let Some(r) = step_ref(step, site, content, &mut cursor) {
                    refs.push(r);
                }
            }
        }
    }
    refs
}

/// The nested `runs.steps[*].uses` of a composite action's metadata file.
pub(crate) fn action_pin_refs(doc: &serde_yaml::Value, content: &str) -> Vec<PinRef> {
    let Some(steps) = doc
        .get("runs")
        .and_then(|r| r.get("steps"))
        .and_then(|s| s.as_sequence())
    else {
        return Vec::new();
    };
    let mut cursor = find_key_line(content, "runs", 1).unwrap_or(1);
    steps
        .iter()
        .filter_map(|step| step_ref(step, "composite step".to_string(), content, &mut cursor))
        .collect()
}

/// The base side's references, for telling an added reference from a pre-existing one.
fn base_pin_set(refs: Option<Vec<PinRef>>) -> HashSet<(PinKind, String)> {
    refs.unwrap_or_default()
        .into_iter()
        .map(|r| (r.kind, r.value))
        .collect()
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

/// Reports the unpinned references of one file. With `diff_only` (the default) only a
/// reference new relative to the base side is judged; with `diff_only = false` every
/// reference is, and the message says whether this change added it or it was already
/// there (existing ones are adopted through the baseline).
fn check_pins(
    ctx: &Context,
    path: &str,
    content: &str,
    refs: &[PinRef],
    base: &HashSet<(PinKind, String)>,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    for r in refs {
        let pre_existing = base.contains(&(r.kind, r.value.clone()));
        if pre_existing && settings.diff_only {
            continue;
        }
        let verdict = pin_verdict(r.kind, &r.value, &settings.first_party_action_prefixes);
        if verdict == PinVerdict::Pinned {
            continue;
        }
        if r.step_line
            .and_then(|l| content.lines().nth(l.saturating_sub(1)))
            .is_some_and(|l| line_allows(l, GATE))
        {
            continue;
        }
        let at = r.line.map(|l| format!(":{l}")).unwrap_or_default();
        if verdict == PinVerdict::Expression {
            out.notes.push(format!(
                "{path}{at}: {} reference '{}' is an expression; its pin cannot be checked (not a pass)",
                r.site, r.value
            ));
            continue;
        }
        let origin = if pre_existing {
            "pre-existing: already on the base side"
        } else {
            "added by this change"
        };
        let (kind, message, remediation) = match r.kind {
            PinKind::Image => (
                &crate::findings::UNPINNED_CONTAINER_IMAGE,
                format!(
                    "Container image '{}' ({}) has no digest ({origin}). Must be pinned by '@sha256:<64-hex digest>'.",
                    r.value, r.site
                ),
                "Pin the image by its immutable digest (`image:tag@sha256:<digest>`), or excuse with allow-gate-weakening: ci-integrity <reason>.",
            ),
            PinKind::Action | PinKind::ReusableWorkflow => {
                let (target, rf) = r.value.split_once('@').unwrap_or((r.value.as_str(), ""));
                let what = if r.kind == PinKind::Action {
                    "Third-party action"
                } else {
                    "Reusable workflow"
                };
                (
                    &crate::findings::UNPINNED_ACTION,
                    format!(
                        "{what} '{target}' ({}) is unpinned ('@{rf}', {origin}). Must be pinned by a 40-character commit SHA.",
                        r.site
                    ),
                    "Pin the reference by its immutable 40-character commit SHA, or excuse with allow-gate-weakening: ci-integrity <reason>.",
                )
            }
        };
        let before = out.violations.len();
        record_or_excuse(
            ctx,
            Some(content),
            out,
            settings.severity,
            kind,
            Some(path.to_string()),
            r.line,
            message,
            remediation,
            &r.value,
        );
        if r.line.is_none() && out.violations.len() > before {
            out.anchor_last(format!("{}:{}", r.site, r.value));
        }
    }
}

/// `ci-integrity` for a composite action's metadata file: pinning of its nested
/// `uses:` only.
fn evaluate_action_file(ctx: &Context, path: &str, out: &mut GateOutcome) -> Result<()> {
    let settings = &ctx.config.gates.ci_integrity;
    let Some(head) = ctx.git.head_content(path)? else {
        return Ok(());
    };
    out.examined += 1;
    if !settings.pin_actions {
        return Ok(());
    }
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&head) else {
        out.notes.push(format!(
            "{path}: action metadata does not parse as YAML; its nested `uses:` were not checked"
        ));
        return Ok(());
    };
    let base = ctx.git.base_content(path).unwrap_or(None);
    let base_refs = base_pin_set(base.as_deref().and_then(|b| {
        serde_yaml::from_str::<serde_yaml::Value>(b)
            .ok()
            .map(|d| action_pin_refs(&d, b))
    }));
    let refs = action_pin_refs(&doc, &head);
    check_pins(ctx, path, &head, &refs, &base_refs, out);
    Ok(())
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
        .find_override(GATE, tokens::ALLOW_CI_WEAKENING, subject)
        .or_else(|| {
            subject
                .split_once('@')
                .and_then(|(act, _)| ctx.find_override(GATE, tokens::ALLOW_CI_WEAKENING, act))
        })
        .or_else(|| ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, GATE));

    if let Some(record) = ov {
        out.overrides.push(record);
    } else {
        let rem_str = remediation.into();
        out.push(severity, kind, file.as_deref(), line, message, &rem_str);
    }
}

/// Parses all job IDs and their needed job IDs from GitHub Actions workflow YAML.
pub fn parse_all_job_needs(content: &str) -> HashMap<String, HashSet<String>> {
    let mut map = HashMap::new();
    if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(content) {
        if let Some(jobs_map) = val.get("jobs").and_then(|j| j.as_mapping()) {
            for (k, v) in jobs_map {
                if let Some(job_name) = k.as_str() {
                    let mut needs = HashSet::new();
                    if let Some(needs_val) = v.get("needs") {
                        if let Some(seq) = needs_val.as_sequence() {
                            for item in seq {
                                if let Some(s) = item.as_str() {
                                    needs.insert(s.to_string());
                                }
                            }
                        } else if let Some(s) = needs_val.as_str() {
                            needs.insert(s.to_string());
                        }
                    }
                    map.insert(job_name.to_string(), needs);
                }
            }
        }
    }
    map
}

/// Parses job IDs and the rollup job's needed job IDs from GitHub Actions workflow YAML.
pub fn parse_workflow_jobs(
    content: &str,
    rollup_name: Option<&str>,
) -> (HashSet<String>, HashSet<String>) {
    let mut jobs = HashSet::new();
    let mut rollup_needs = HashSet::new();

    if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(content) {
        if let Some(jobs_map) = val.get("jobs").and_then(|j| j.as_mapping()) {
            for (k, v) in jobs_map {
                if let Some(job_name) = k.as_str() {
                    jobs.insert(job_name.to_string());
                    if rollup_name == Some(job_name) {
                        if let Some(needs_val) = v.get("needs") {
                            if let Some(seq) = needs_val.as_sequence() {
                                for item in seq {
                                    if let Some(s) = item.as_str() {
                                        rollup_needs.insert(s.to_string());
                                    }
                                }
                            } else if let Some(s) = needs_val.as_str() {
                                rollup_needs.insert(s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    (jobs, rollup_needs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(yaml: &str) -> Vec<DisciplinePin> {
        discipline_pins(&serde_yaml::from_str::<serde_yaml::Value>(yaml).unwrap())
    }

    #[test]
    fn discipline_pins_are_read_from_every_place_a_pipeline_names_the_binary() {
        let actions = pins(
            "jobs:\n  gate:\n    container:\n      image: ghcr.io/orieg/discipline:v0.14.4@sha256:abc\n    steps:\n      - uses: actions/checkout@v4\n      - uses: orieg/discipline@v0.14.4\n        with:\n          version: v0.14.4\n          binary: ./bin/discipline\n",
        );
        let whats: Vec<&str> = actions.iter().map(|p| p.what).collect();
        assert_eq!(whats, vec!["image", "uses", "version", "binary"]);
        let gitlab = pins(
            "include:\n  - remote: 'https://raw.githubusercontent.com/orieg/discipline/v0.14.4/templates/discipline.gitlab-ci.yml'\n  - project: orieg/discipline\n    ref: v0.14.4\n    file: templates/discipline.gitlab-ci.yml\ngate:\n  image: ghcr.io/orieg/discipline:v0.14.4\n  script: [discipline check]\n",
        );
        let whats: Vec<&str> = gitlab.iter().map(|p| p.what).collect();
        assert_eq!(whats, vec!["include", "include", "image"]);
        assert_eq!(pin_version(&gitlab[0]), "v0.14.4");
        assert_eq!(pin_version(&gitlab[1]), "v0.14.4");
        assert_eq!(pin_version(&gitlab[2]), "v0.14.4");
        assert!(pins("jobs:\n  t:\n    steps:\n      - uses: actions/checkout@v4\n").is_empty());
    }

    #[test]
    fn a_downgrade_or_a_movable_ref_blocks_and_an_upgrade_is_a_note() {
        let pin = |what: &'static str, value: &str| DisciplinePin {
            what,
            value: value.into(),
        };
        let uses = |r: &str| pin("uses", &format!("orieg/discipline@{r}"));
        let change = |b: &str, h: &str| pin_change("uses", Some(&uses(b)), &uses(h));
        assert_eq!(change("v0.14.4", "v0.14.4"), None);
        assert_eq!(change("v0.14.4", "v0.13.0"), Some(true), "downgrade");
        assert_eq!(change("v0.14.4", "main"), Some(true), "a branch moves");
        assert_eq!(change("v0.14.4", "v0"), Some(true), "a major tag moves");
        assert_eq!(change("v0.14.4", "v0.15.0"), Some(false), "upgrade");
        assert_eq!(
            change("v0", "v0.14.4"),
            Some(false),
            "to an immutable release"
        );
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(change("v0.14.4", sha), Some(false), "to a commit pin");
        assert_eq!(change(sha, "main"), Some(true));
        let image = |t: &str| pin("image", &format!("ghcr.io/orieg/discipline:{t}"));
        assert_eq!(
            pin_change("image", Some(&image("v0.14.4")), &image("v0.12.0")),
            Some(true)
        );
        assert_eq!(
            pin_change("image", Some(&image("v0.14.4")), &image("latest")),
            Some(true)
        );
        // A new binary source always blocks; a removed input is not a pin change here.
        assert_eq!(
            pin_change("binary", None, &pin("binary", "./bin/discipline")),
            Some(true)
        );
        let (blocking, notes) = discipline_pin_changes(
            &[uses("v0.14.4")],
            &[
                uses("v0.15.0"),
                pin("download_url", "https://example.com/dl"),
            ],
        );
        assert_eq!(
            (blocking.len(), notes.len()),
            (1, 1),
            "{blocking:?} {notes:?}"
        );
    }

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

    #[test]
    fn parses_workflow_jobs_and_inline_needs() {
        let yml = r#"
name: CI
jobs:
  lint:
    runs-on: ubuntu-latest
  test:
    runs-on: ubuntu-latest
  ci-gate:
    needs: [lint, test]
    runs-on: ubuntu-latest
"#;
        let (jobs, needs) = parse_workflow_jobs(yml, Some("ci-gate"));
        assert_eq!(jobs.len(), 3);
        assert!(jobs.contains("lint"));
        assert!(jobs.contains("test"));
        assert!(jobs.contains("ci-gate"));
        assert_eq!(needs.len(), 2);
        assert!(needs.contains("lint"));
        assert!(needs.contains("test"));
        assert!(!jobs.is_empty());
        assert!(!needs.is_empty());
        assert!(!jobs.contains("deploy"));
        assert!(!needs.contains("deploy"));
        assert!(!needs.contains("ci-gate"));
    }

    #[test]
    fn parses_workflow_jobs_and_multiline_needs() {
        let yml = r#"
jobs:
  detect-changes:
    runs-on: ubuntu-latest
  lint:
    runs-on: ubuntu-latest
  ci-gate:
    needs:
      - detect-changes
      - lint
"#;
        let (jobs, needs) = parse_workflow_jobs(yml, Some("ci-gate"));
        assert_eq!(jobs.len(), 3);
        assert_eq!(needs.len(), 2);
        assert!(jobs.contains("detect-changes"));
        assert!(jobs.contains("lint"));
        assert!(jobs.contains("ci-gate"));
        assert!(!jobs.is_empty());
        assert!(!needs.is_empty());
        assert!(!jobs.contains("deploy"));
        assert!(!needs.contains("deploy"));
    }

    fn steps(yaml: &str) -> Vec<serde_yaml::Value> {
        serde_yaml::from_str::<serde_yaml::Value>(yaml)
            .unwrap()
            .as_sequence()
            .unwrap()
            .clone()
    }

    #[test]
    fn rename_threshold_is_pinned() {
        assert_eq!(STEP_RENAME_SIMILARITY, 0.6);
        let a = &steps("- run: cargo test --locked")[0];
        let b = &steps("- run: cargo test --locked --workspace")[0];
        let c = &steps("- run: cargo build")[0];
        // 2*3/(3+4) = 0.857 clears the threshold; 2*1/(3+2) = 0.4 does not.
        assert!((step_body_similarity(a, b) - 6.0 / 7.0).abs() < 1e-9);
        assert!((step_body_similarity(a, c) - 0.4).abs() < 1e-9);
        assert_eq!(step_body_similarity(a, a), 1.0);
    }

    #[test]
    fn renamed_step_with_unchanged_body_pairs_as_rename() {
        let base = steps(
            "- uses: actions/checkout@v4\n- name: Check a.sh b.sh\n  run: |\n    ./a.sh\n    ./b.sh\n",
        );
        let head = steps(
            "- uses: actions/checkout@v4\n- name: Check a.sh b.sh c.sh\n  run: |\n    ./a.sh\n    ./b.sh\n    ./c.sh\n",
        );
        let pairs = pair_steps(&base, &head);
        assert_eq!(pairs[0], Some(StepMatch::Same(0)));
        match pairs[1] {
            Some(StepMatch::Renamed {
                head: 1,
                similarity,
            }) => {
                assert!(similarity >= STEP_RENAME_SIMILARITY, "{similarity}")
            }
            other => panic!("expected rename, got {other:?}"),
        }
    }

    #[test]
    fn renamed_and_rewritten_step_is_not_paired() {
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let head = steps("- name: Smoke\n  run: echo ok\n");
        assert_eq!(pair_steps(&base, &head), vec![None]);
    }

    #[test]
    fn removed_step_is_not_paired_with_an_unrelated_survivor() {
        let base = steps(
            "- name: Build\n  run: cargo build --locked\n- name: Run tests\n  run: cargo test --locked\n",
        );
        let head = steps("- name: Build\n  run: cargo build --locked\n");
        assert_eq!(
            pair_steps(&base, &head),
            vec![Some(StepMatch::Same(0)), None]
        );

        // The survivor is similar (2*2/6 = 0.67) and verifies the same thing,
        // but it already matched its own base step: it cannot also be the
        // rename of the removed one.
        let base = steps(
            "- name: Unit tests\n  run: cargo test --lib\n- name: All tests\n  run: cargo test --workspace\n",
        );
        let head = steps("- name: Unit tests\n  run: cargo test --lib\n");
        assert!(step_body_similarity(&base[1], &head[0]) >= STEP_RENAME_SIMILARITY);
        assert_eq!(
            pair_steps(&base, &head),
            vec![Some(StepMatch::Same(0)), None]
        );
    }

    #[test]
    fn rename_pairing_prefers_the_closest_position_on_a_tie() {
        let base = steps("- name: A\n  run: make check\n- name: B\n  run: make lint\n");
        let head = steps("- name: X\n  run: make check\n- name: Y\n  run: make check\n");
        let pairs = pair_steps(&base, &head);
        assert!(
            matches!(pairs[0], Some(StepMatch::Renamed { head: 0, .. })),
            "{pairs:?}"
        );
        // `make lint` vs `make check` is 0.5: below the threshold.
        assert_eq!(pairs[1], None);
    }

    #[test]
    fn rename_that_drops_the_verification_command_is_not_paired() {
        // Similar enough by tokens (2*2/6 = 0.67), but `test` is gone.
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let head = steps("- name: Compile\n  run: cargo build --locked\n");
        assert!(step_body_similarity(&base[0], &head[0]) >= STEP_RENAME_SIMILARITY);
        assert_eq!(pair_steps(&base, &head), vec![None]);
    }

    #[test]
    fn one_head_step_is_the_rename_of_at_most_one_base_step() {
        // Two verification steps collapse into one renamed step: one of them
        // was deleted, and pairing must not hide that.
        let base = steps("- name: Test A\n  run: make test\n- name: Test B\n  run: make test\n");
        let head = steps("- name: Tests\n  run: make test\n");
        let pairs = pair_steps(&base, &head);
        assert!(
            matches!(pairs[0], Some(StepMatch::Renamed { head: 0, .. })),
            "{pairs:?}"
        );
        assert_eq!(pairs[1], None);
    }

    #[test]
    fn shell_comment_lines_do_not_count_toward_similarity() {
        // Documenting a step's body with comments leaves it the same step.
        let base = steps("- name: Self-tests a b\n  run: |\n    python3 a.py --self-test\n    python3 b.py --self-test\n");
        let head = steps("- name: Self-tests a b c\n  run: |\n    python3 a.py --self-test\n    # c.py needs a PMU to collect anything, but its parser and\n    # its rendering rules are all checkable without one.\n    python3 c.py --self-test\n    python3 b.py --self-test\n");
        assert!((step_body_similarity(&base[0], &head[0]) - 0.8).abs() < 1e-9);

        // Commenting the old command out does not keep the step alive.
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let head = steps("- name: Smoke\n  run: |\n    # cargo test --locked\n    echo ok\n");
        assert_eq!(step_body_similarity(&base[0], &head[0]), 0.0);
        assert_eq!(pair_steps(&base, &head), vec![None]);
    }

    #[test]
    fn deletion_reason_names_the_rule_that_refused_the_rename() {
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let swapped = steps("- name: Compile\n  run: cargo build --locked\n");
        let reason = deletion_reason(&base[0], &swapped, &pair_steps(&base, &swapped));
        assert!(
            reason.contains("'Compile'")
                && reason.contains("0.67")
                && reason.contains("no longer carries `test`"),
            "{reason}"
        );
        assert!(!reason.contains("below the rename threshold"), "{reason}");

        let rewritten = steps("- name: Smoke\n  run: echo ok\n");
        let reason = deletion_reason(&base[0], &rewritten, &pair_steps(&base, &rewritten));
        assert!(
            reason.contains("'Smoke'") && reason.contains("0.00, below the rename threshold 0.60"),
            "{reason}"
        );

        let reason = deletion_reason(&base[0], &[], &pair_steps(&base, &[]));
        assert!(reason.contains("no unmatched head step"), "{reason}");
    }

    #[test]
    fn verification_step_detection_still_reads_the_raw_run_text() {
        // Unchanged by rename pairing: comment text still marks a step as
        // verification, so its deletion stays guarded.
        let s = steps("- name: Step\n  run: |\n    # run the lint pass\n    ./ci.sh\n");
        assert!(is_verification_step(&s[0]));
        assert!(verifying_body_markers(&s[0]).is_empty());
    }

    const SHA: &str = "b4ffde65f46336ab88eb53be808477a3936bae11";
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn first_party() -> Vec<String> {
        vec!["actions/".to_string(), "github/".to_string()]
    }

    #[test]
    fn reusable_workflow_needs_a_commit_sha_unless_local_or_first_party() {
        let v = |r: &str| pin_verdict(PinKind::ReusableWorkflow, r, &first_party());
        assert_eq!(
            v("evil/reusable/.github/workflows/x.yml@main"),
            PinVerdict::Unpinned
        );
        assert_eq!(
            v("evil/reusable/.github/workflows/x.yml@v1.2.3"),
            PinVerdict::Unpinned
        );
        assert_eq!(
            v(&format!("evil/reusable/.github/workflows/x.yml@{SHA}")),
            PinVerdict::Pinned
        );
        assert_eq!(v("./.github/workflows/local.yml"), PinVerdict::Pinned);
        assert_eq!(
            v("actions/reusable/.github/workflows/x.yml@main"),
            PinVerdict::Pinned
        );
        // A SHA one character short is not a SHA.
        assert_eq!(
            v(&format!("evil/r/.github/workflows/x.yml@{}", &SHA[1..])),
            PinVerdict::Unpinned
        );
    }

    #[test]
    fn container_image_needs_a_sha256_digest() {
        let v = |r: &str| pin_verdict(PinKind::Image, r, &first_party());
        assert_eq!(v("node:latest"), PinVerdict::Unpinned);
        assert_eq!(v("node"), PinVerdict::Unpinned);
        assert_eq!(v("docker://alpine:latest"), PinVerdict::Unpinned);
        assert_eq!(v(&format!("node:20@sha256:{DIGEST}")), PinVerdict::Pinned);
        assert_eq!(
            v(&format!("docker://alpine@sha256:{DIGEST}")),
            PinVerdict::Pinned
        );
        assert_eq!(
            v(&format!("ghcr.io/o/i:1@sha256:{}", &DIGEST[1..])),
            PinVerdict::Unpinned
        );
        assert_eq!(v("node@sha256:not-a-digest"), PinVerdict::Unpinned);
        // A first-party prefix does not exempt an image.
        assert_eq!(v("actions/runner:latest"), PinVerdict::Unpinned);
        assert_eq!(v("${{ matrix.image }}"), PinVerdict::Expression);
        assert_eq!(v("node:${{ matrix.v }}"), PinVerdict::Expression);
    }

    #[test]
    fn workflow_refs_cover_steps_jobs_containers_services_and_docker_steps() {
        let wf = format!(
            "on: push\njobs:\n  triage:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions-cool/issues-helper@v2.2.1\n  r:\n    uses: evil/reusable/.github/workflows/x.yml@main\n  build:\n    runs-on: ubuntu-latest\n    container:\n      image: node:latest\n    services:\n      db:\n        image: postgres@sha256:{DIGEST}\n    steps:\n      - uses: docker://alpine:latest\n      - uses: docker://alpine:latest\n"
        );
        let doc: serde_yaml::Value = serde_yaml::from_str(&wf).unwrap();
        let refs = workflow_pin_refs(&doc, &wf);
        let got: Vec<(PinKind, &str, Option<usize>)> = refs
            .iter()
            .map(|r| (r.kind, r.value.as_str(), r.line))
            .collect();
        let db = format!("postgres@sha256:{DIGEST}");
        assert_eq!(
            got,
            vec![
                (
                    PinKind::Action,
                    "actions-cool/issues-helper@v2.2.1",
                    Some(6)
                ),
                (
                    PinKind::ReusableWorkflow,
                    "evil/reusable/.github/workflows/x.yml@main",
                    Some(8)
                ),
                (PinKind::Image, "node:latest", Some(12)),
                (PinKind::Image, db.as_str(), Some(15)),
                // A repeated reference is located at its own line.
                (PinKind::Image, "docker://alpine:latest", Some(17)),
                (PinKind::Image, "docker://alpine:latest", Some(18)),
            ]
        );
        // The string form of `container:`.
        let wf = "jobs:\n  b:\n    container: node:20\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(wf).unwrap();
        let refs = workflow_pin_refs(&doc, wf);
        assert_eq!(refs.len(), 1);
        assert_eq!((refs[0].kind, refs[0].line), (PinKind::Image, Some(3)));
        assert_eq!(refs[0].value, "node:20");
        // No jobs: nothing.
        let doc: serde_yaml::Value = serde_yaml::from_str("name: x\n").unwrap();
        assert!(workflow_pin_refs(&doc, "name: x\n").is_empty());
    }

    #[test]
    fn composite_action_refs_are_its_nested_step_uses() {
        let action = "name: setup\nruns:\n  using: composite\n  steps:\n    - uses: evil/thing@v1\n    - run: echo hi\n      shell: bash\n    - uses: docker://alpine:3\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(action).unwrap();
        let got: Vec<(PinKind, String, Option<usize>)> = action_pin_refs(&doc, action)
            .into_iter()
            .map(|r| (r.kind, r.value, r.line))
            .collect();
        assert_eq!(
            got,
            vec![
                (PinKind::Action, "evil/thing@v1".to_string(), Some(5)),
                (PinKind::Image, "docker://alpine:3".to_string(), Some(8)),
            ]
        );
        let docker = "runs:\n  using: docker\n  image: Dockerfile\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(docker).unwrap();
        assert!(action_pin_refs(&doc, docker).is_empty());
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
    fn key_lines_match_whole_keys_only() {
        let c = "jobs:\n  rr:\n    runs-on: x\n  r:\n    uses: a/b@v1\n  'q':\n";
        assert_eq!(find_key_line(c, "r", 1), Some(4));
        assert_eq!(find_key_line(c, "q", 1), Some(6));
        assert_eq!(find_key_line(c, "jobs", 1), Some(1));
        assert_eq!(find_key_line(c, "rr", 3), None);
    }
}
