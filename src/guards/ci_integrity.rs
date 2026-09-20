//! CI/CD workflow integrity and rollup sentinel (`ci-integrity`).
//!
//! Enforces:
//! - Rollup jobs (`ci-gate`) depend on every verification job in the workflow (`needs:`).
//! - Rollup jobs cannot drop previously depended-on jobs without an override.
//! - Third-party GitHub actions introduced or modified in diff are pinned by a 40-character commit SHA
//!   (excluding first-party action prefixes like `actions/` and `github/`).
//! - Existing unpinned actions from base ref are grandfathered in and not flagged as new findings.
//! - Masked failures (`continue-on-error: true`) and error suppression (`|| true`, `set +e`) are forbidden.
//! - Verification jobs and steps (testing, linting, gates) cannot be deleted without an override.
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

    for path in &workflow_files {
        let head_content = match ctx.git.head_content(path)? {
            Some(c) => c,
            None => {
                // Workflow file deleted in head
                if let Ok(Some(base_src)) = ctx.git.base_content(path) {
                    if let Ok(base_val) = serde_yaml::from_str::<serde_yaml::Value>(&base_src) {
                        let (base_jobs, _) =
                            parse_workflow_jobs(&base_src, settings.rollup_job.as_deref());
                        let has_verification = base_jobs.iter().any(|j| {
                            let job_val = base_val.get("jobs").and_then(|m| m.get(j));
                            is_verification_job(j, job_val.unwrap_or(&serde_yaml::Value::Null))
                        });
                        if has_verification {
                            record_or_excuse(
                                ctx,
                                None,
                                &mut out,
                                settings.severity,
                                "Deletion of Verification Workflow",
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

        let mut base_actions = HashSet::new();
        if let Some(base_doc) = &base_val {
            if let Some(b_jobs) = base_doc.get("jobs").and_then(|j| j.as_mapping()) {
                for (_, b_job) in b_jobs {
                    if let Some(b_steps) = b_job.get("steps").and_then(|s| s.as_sequence()) {
                        for b_step in b_steps {
                            if let Some(u) = b_step.get("uses").and_then(|u| u.as_str()) {
                                base_actions.insert(u.to_string());
                            }
                        }
                    }
                }
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
                                "Rollup Job Dropped Dependency",
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
                        "Incomplete Rollup Job Needs",
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
                            title: "Documented Job Count File Missing".to_string(),
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
                                            title: "Documented Job Count Mismatch".to_string(),
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
                    "Dangerous pull_request_target Trigger",
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
                        "Workflow Permissions Widened",
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
                        "Workflow timeout-minutes Removed",
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
                            record_or_excuse(
                                ctx,
                                Some(&head_content),
                                &mut out,
                                settings.severity,
                                "Deletion of Verification Job",
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
                                    "Job timeout-minutes Removed",
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

                            for b_step in &base_steps {
                                if is_verification_step(b_step) {
                                    let matched = head_steps
                                        .iter()
                                        .any(|h_step| steps_match_identity(b_step, h_step));
                                    if !matched {
                                        let step_name = b_step
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .or_else(|| b_step.get("id").and_then(|i| i.as_str()))
                                            .unwrap_or("unnamed verification step");
                                        record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            "Deletion of Verification Step",
                                            Some(path.clone()),
                                            find_line_number(&head_content, job_id),
                                            format!("Verification step '{step_name}' in job '{job_id}' was deleted."),
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
                                settings.severity,
                                "continue-on-error Masks Failure",
                                Some(path.clone()),
                                coe_line,
                                format!("Job '{job_id}' carries 'continue-on-error: true', which masks failures in CI."),
                                "Remove continue-on-error or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                                "continue-on-error",
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
                            if if_cond.contains("always()") || if_cond.contains("cancelled()") {
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
                                        "Conditional Masking on Verification Job",
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
                        for step in steps {
                            let step_name = step.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let step_id = step.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            let uses_str = step.get("uses").and_then(|u| u.as_str());
                            let run_str = step.get("run").and_then(|r| r.as_str());

                            // Find matching base step
                            let base_step = base_job_steps.and_then(|b_steps| {
                                b_steps.iter().find(|b| steps_match_identity(b, step))
                            });

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

                            // 5a. Action pinning
                            if settings.pin_actions {
                                if let Some(uses) = uses_str {
                                    let base_had_action = base_actions.contains(uses);

                                    if !base_had_action
                                        && !uses.starts_with("./")
                                        && !uses.starts_with("docker://")
                                    {
                                        if let Some((action, ref_str)) = uses.split_once('@') {
                                            let is_first_party = settings
                                                .first_party_action_prefixes
                                                .iter()
                                                .any(|prefix| action.starts_with(prefix));
                                            if !is_first_party {
                                                let is_sha = ref_str.len() == 40
                                                    && ref_str
                                                        .chars()
                                                        .all(|c| c.is_ascii_hexdigit());
                                                if !is_sha {
                                                    let uses_line = find_line_after(
                                                        &head_content,
                                                        uses,
                                                        approx_line.unwrap_or(1),
                                                    )
                                                    .or(approx_line);
                                                    record_or_excuse(
                                                        ctx,
                                                        Some(&head_content),
                                                        &mut out,
                                                        settings.severity,
                                                        "Unpinned Third-Party Action",
                                                        Some(path.clone()),
                                                        uses_line,
                                                        format!("Third-party action '{action}' is unpinned ('@{ref_str}'). Must be pinned by a 40-character commit SHA."),
                                                        "Pin the action by its immutable 40-character commit SHA, or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                        action,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // 5b. Check orieg/discipline action inputs
                            if let Some(uses) = uses_str {
                                if uses.contains("orieg/discipline")
                                    || uses.contains("discipline")
                                    || uses == "./action.yml"
                                {
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
                                                    "Discipline Action Weakened (disable input)",
                                                    Some(path.clone()),
                                                    approx_line,
                                                    "The 'disable' input on the discipline step was added or widened, bypassing verification gates.".to_string(),
                                                    "Remove 'disable' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                    "disable",
                                                );
                                            }
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
                                                        "Discipline Action Weakened (fail_on_warnings: false)",
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
                                                    "Discipline Action Invalid config_override",
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
                                                        "Discipline Action Suite Changed",
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
                                                        "Discipline Action Directive Sources Widened",
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
                                            "Compiler Flag Dropped (-D warnings)",
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
                                            "Cargo Flag Dropped (--locked)",
                                            Some(path.clone()),
                                            approx_line,
                                            "Step dropped '--locked' from cargo command, permitting unverified dependency updates.".to_string(),
                                            "Restore '--locked' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                            "--locked",
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
                                            "Clippy Flag Dropped (--all-targets)",
                                            Some(path.clone()),
                                            approx_line,
                                            "Step dropped '--all-targets' from clippy command, skipping linting on tests/benchmarks.".to_string(),
                                            "Restore '--all-targets' or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                            "--all-targets",
                                        );
                                    }
                                }
                            }

                            // 5d. Introduction of if: always() or if: failure() on verification step
                            if is_verification_step(step) {
                                if let Some(if_cond) = step.get("if").and_then(|i| i.as_str()) {
                                    let lower_if = if_cond.to_ascii_lowercase();
                                    if lower_if.contains("always()")
                                        || lower_if.contains("failure()")
                                    {
                                        let base_had_it = base_step
                                            .and_then(|b| b.get("if"))
                                            .and_then(|i| i.as_str())
                                            .map(|b| {
                                                let bl = b.to_ascii_lowercase();
                                                bl.contains("always()") || bl.contains("failure()")
                                            })
                                            .unwrap_or(false);
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
                                                "Conditional Masking on Verification Step",
                                                Some(path.clone()),
                                                if_line,
                                                format!("Verification step carries 'if: {if_cond}', masking earlier pipeline failures."),
                                                "Remove conditional masking or excuse with allow-gate-weakening: ci-integrity <reason>.",
                                                "if-always",
                                            );
                                        }
                                    }
                                }
                            }

                            // 5e. continue-on-error
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
                                    record_or_excuse(
                                            ctx,
                                            Some(&head_content),
                                            &mut out,
                                            settings.severity,
                                            "continue-on-error Masks Failure",
                                            Some(path.clone()),
                                            coe_line,
                                            "Step carries 'continue-on-error: true', which masks failures in CI.".to_string(),
                                            "Remove continue-on-error or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                                            "continue-on-error",
                                        );
                                }
                            }

                            // 5f. Error suppression: || true / set +e
                            if settings.forbid_or_true {
                                if let Some(run_cmd) = run_str {
                                    let has_mask = run_cmd
                                        .lines()
                                        .any(|l| l.contains("|| true") || l.contains("set +e"));
                                    if has_mask {
                                        let base_had_mask = base_step
                                            .and_then(|b| b.get("run"))
                                            .and_then(|r| r.as_str())
                                            .map(|b| {
                                                b.lines().any(|l| {
                                                    l.contains("|| true") || l.contains("set +e")
                                                })
                                            })
                                            .unwrap_or(false);
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
                                            record_or_excuse(
                                                ctx,
                                                Some(&head_content),
                                                &mut out,
                                                settings.severity,
                                                "Command Masks Exit Code",
                                                Some(path.clone()),
                                                mask_line,
                                                "Command uses '|| true' or 'set +e' to mask command failure.".to_string(),
                                                "Remove '|| true' or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                                                "or-true",
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

fn is_verification_step(step: &serde_yaml::Value) -> bool {
    if let Some(run) = step.get("run").and_then(|r| r.as_str()) {
        let r = run.to_ascii_lowercase();
        if r.contains("test")
            || r.contains("clippy")
            || r.contains("cargo check")
            || r.contains("fmt --check")
            || r.contains("lint")
            || r.contains("pytest")
            || r.contains("discipline")
            || r.contains("audit")
        {
            return true;
        }
    }
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        let u = uses.to_ascii_lowercase();
        if u.contains("discipline") || u.contains("clippy") || u.contains("actionlint") {
            return true;
        }
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

#[allow(clippy::too_many_arguments)]
fn record_or_excuse(
    ctx: &Context,
    head_content: Option<&str>,
    out: &mut GateOutcome,
    severity: Severity,
    title: &str,
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
        .find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, subject)
        .or_else(|| {
            ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, "ci-integrity")
        })
        .or_else(|| {
            if let Some(ref f) = file {
                ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, f)
            } else {
                None
            }
        });

    if let Some(record) = ov {
        out.overrides.push(record);
    } else {
        let rem_str = remediation.into();
        out.push(severity, title, file.as_deref(), line, message, &rem_str);
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
}
