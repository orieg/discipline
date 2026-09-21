//! CI/CD workflow integrity and rollup sentinel (`ci-integrity`).
//!
//! Enforces:
//! - Rollup jobs (`ci-gate`) depend on every verification job in the workflow (`needs:`).
//! - Rollup jobs cannot drop previously depended-on jobs without an override.
//! - Third-party GitHub actions introduced or modified in diff are pinned by a 40-character commit SHA
//!   (excluding first-party action prefixes like `actions/` and `github/`).
//! - Existing unpinned actions from base ref are grandfathered in and not flagged as new findings.
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
        // GitLab pipelines are a different document shape; they have their own diff.
        if super::ci_gitlab::is_gitlab_ci_path(path) {
            evaluate_gitlab_file(ctx, path, &mut out)?;
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
                                            "Deletion of Verification Step",
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
                                settings.severity,
                                "continue-on-error Masks Failure",
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
                                                        uses,
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
                                            "Discipline Action Weakened (policy_from)",
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
                                                    "Discipline Action Weakened (disable input)",
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
                                                "Discipline Action Weakened (advisory: true)",
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
                                        "Discipline Run Weakened (--advisory)",
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
                                    let step_subject = step
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .or_else(|| step.get("id").and_then(|i| i.as_str()))
                                        .unwrap_or("continue-on-error");
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
                                        step_subject,
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
                                                "Command Masks Exit Code",
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
    use super::ci_gitlab::{diff_gitlab_ci, verification_jobs};
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
                    "Deletion of Verification Workflow",
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
    out.notes.push(format!(
        "`{path}`: pipelines pulled in through `include:` and changes to `rules:` are not read"
    ));
    // A new pipeline file has nothing to be weakened against.
    let Some(base) = base else {
        return Ok(());
    };
    match diff_gitlab_ci(&base, &head) {
        Ok(found) => {
            for w in found {
                let line = find_line_number(&head, &format!("{}:", w.job));
                record_or_excuse(
                    ctx,
                    Some(&head),
                    out,
                    settings.severity,
                    w.title,
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
            "Pipeline File Unreadable",
            Some(path),
            None,
            format!("`{path}` could not be compared with its base side ({e})."),
            "Fix the YAML so the pipeline can be checked.",
        ),
    }
    Ok(())
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

fn is_verification_step(step: &serde_yaml::Value) -> bool {
    let raw_run = step.get("run").and_then(|r| r.as_str()).map(str::to_string);
    if !markers_in(step, raw_run).is_empty() {
        return true;
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
}
