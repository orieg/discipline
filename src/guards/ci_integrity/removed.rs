//! Verification workflows, jobs and steps the change removes.

use super::{
    find_line_number, is_verification_job, is_verification_step, job_needs, pair_steps,
    record_or_excuse, step_body_similarity, step_label, verifying_body_markers, AddedSteps,
    StepMatch, WorkflowFile, STEP_RENAME_SIMILARITY,
};
use crate::guards::{Context, GateOutcome};
use anyhow::Result;

/// A workflow file the head side no longer has: reported when it held verification jobs,
/// unless their steps are in jobs this change added.
pub(super) fn report_deleted_workflow(
    ctx: &Context,
    path: &str,
    added_steps: &mut AddedSteps,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.ci_integrity;
    if let Some(base_src) = ctx.git.base_content(path)? {
        let docs = added_steps.docs;
        if let Some(base_val) = docs.noted(out, path, "base", Some(&base_src)) {
            let base_jobs: Vec<String> = job_needs(&base_val).into_keys().collect();
            let mut verification: Vec<(&String, &serde_yaml::Value)> = base_jobs
                .iter()
                .filter_map(|j| {
                    let job_val = base_val.get("jobs").and_then(|m| m.get(j))?;
                    is_verification_job(j, job_val).then_some((j, job_val))
                })
                .collect();
            // In name order: the note below lists the jobs, and a hash set's order
            // differs from one process to the next.
            verification.sort_unstable_by_key(|(name, _)| name.as_str());
            if !verification.is_empty() {
                added_steps.load(ctx, &mut out.notes)?;
            }
            let added = added_steps.loaded();
            if !verification.is_empty() && verification.iter().all(|(_, job)| job_moved(job, added))
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
                    out,
                    settings.severity,
                    &crate::findings::VERIFICATION_WORKFLOW_DELETED,
                    Some(path.to_string()),
                    None,
                    format!("Workflow '{path}' containing verification jobs was deleted."),
                    "Restore the deleted workflow or provide an allow-gate-weakening: ci-integrity <reason> directive.",
                    path,
                );
            }
        }
    }
    Ok(())
}

/// Compares the jobs of the head side with the jobs of the base side: verification jobs
/// removed, and what the surviving jobs lost.
pub(super) fn compare_jobs_with_base(
    ctx: &Context,
    wf: &WorkflowFile,
    head_doc: &serde_yaml::Value,
    added_steps: &mut AddedSteps,
    out: &mut GateOutcome,
) -> Result<()> {
    if let Some(base_doc) = wf.base {
        if let (Some(base_jobs_map), Some(head_jobs_map)) = (
            base_doc.get("jobs").and_then(|j| j.as_mapping()),
            head_doc.get("jobs").and_then(|j| j.as_mapping()),
        ) {
            report_removed_jobs(ctx, wf, base_jobs_map, head_jobs_map, added_steps, out)?;
            compare_surviving_jobs(ctx, wf, base_jobs_map, head_jobs_map, out);
        }
    }
    Ok(())
}

/// Reports each verification or rollup job of the base side that the head side no longer
/// has, unless its verification steps are in jobs this change added.
fn report_removed_jobs(
    ctx: &Context,
    wf: &WorkflowFile,
    base_jobs_map: &serde_yaml::Mapping,
    head_jobs_map: &serde_yaml::Mapping,
    added_steps: &mut AddedSteps,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    // Check for deleted verification jobs
    for (job_k, job_v) in base_jobs_map {
        let job_id = job_k.as_str().unwrap_or("");
        let is_rollup = settings.rollup_job.as_deref() == Some(job_id);
        if !head_jobs_map.contains_key(job_k) && (is_verification_job(job_id, job_v) || is_rollup) {
            if !is_rollup {
                added_steps.load(ctx, &mut out.notes)?;
                if job_moved(job_v, added_steps.loaded()) {
                    out.notes.push(format!(
                        "{path}: job '{job_id}' was removed; its verification steps are in jobs this change added, so it is treated as a rename or split"
                    ));
                    continue;
                }
            }
            let before = out.violations.len();
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::VERIFICATION_JOB_REMOVED,
                Some(path.to_string()),
                None,
                format!("Verification job '{job_id}' present in base was deleted."),
                format!("Restore job '{job_id}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                job_id,
            );
            // Several jobs can leave one workflow: the job tells them apart.
            if out.violations.len() > before {
                out.anchor_last(format!("job:{job_id}"));
            }
        }
    }
    Ok(())
}

/// Compares each job both sides have: its `timeout-minutes` and its verification steps.
fn compare_surviving_jobs(
    ctx: &Context,
    wf: &WorkflowFile,
    base_jobs_map: &serde_yaml::Mapping,
    head_jobs_map: &serde_yaml::Mapping,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
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
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::JOB_TIMEOUT_REMOVED,
                    Some(path.to_string()),
                    find_line_number(head_content, job_id),
                    format!("Job '{job_id}' timeout-minutes was removed."),
                    format!("Restore timeout-minutes to job '{job_id}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                    job_id,
                );
            }

            report_removed_steps(ctx, wf, job_id, base_job_v, head_job_v, out);
        }
    }
}

/// Reports each verification step of a base job that no step of the head job matches. A
/// step renamed with a similar run body is noted, not reported.
fn report_removed_steps(
    ctx: &Context,
    wf: &WorkflowFile,
    job_id: &str,
    base_job_v: &serde_yaml::Value,
    head_job_v: &serde_yaml::Value,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
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
                let new_name = step_label(&head_steps[*head], "unnamed step");
                out.notes.push(format!(
                    "{path}: verification step '{step_name}' in job '{job_id}' was renamed to '{new_name}' (run body similarity {similarity:.2} >= rename threshold {STEP_RENAME_SIMILARITY:.2}); treated as a rename, not a deletion"
                ));
            }
            None => {
                let why = deletion_reason(b_step, &head_steps, &pairs);
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::VERIFICATION_STEP_REMOVED,
                    Some(path.to_string()),
                    find_line_number(head_content, job_id),
                    format!("Verification step '{step_name}' in job '{job_id}' was deleted: no step in head matches it by id, name, or run body ({why})."),
                    format!("Restore step '{step_name}' or excuse with allow-gate-weakening: ci-integrity <reason>."),
                    step_name,
                );
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::ci_integrity::test_support::*;

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
}
