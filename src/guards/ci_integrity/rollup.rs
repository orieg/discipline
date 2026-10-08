//! The rollup job: that it needs every job of its workflow, keeps the jobs it needed,
//! and that a documented job count matches the workflow.

use super::{find_line_number, record_or_excuse, WorkflowFile, GATE, NOT_COMPARED};
use crate::guards::{Context, GateOutcome, Violation};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

/// The configuration key of the pattern that reads the documented job count.
pub const JOB_COUNT_PATTERN_KEY: &str = "gates.ci-integrity.documented_job_count_pattern";

/// The configuration key of the document the job count is read from.
pub const JOB_COUNT_PATH_KEY: &str = "gates.ci-integrity.documented_job_count_path";

/// The configuration error of a documented job count with only one of its two keys set:
/// the count is compared only with both, so one alone would be a setting that does
/// nothing. `None` when both are set or neither is.
pub fn half_configured_job_count(
    settings: &crate::config::CiIntegrityGate,
) -> Option<anyhow::Error> {
    let (set, unset) = match (
        &settings.documented_job_count_path,
        &settings.documented_job_count_pattern,
    ) {
        (Some(_), None) => (JOB_COUNT_PATH_KEY, JOB_COUNT_PATTERN_KEY),
        (None, Some(_)) => (JOB_COUNT_PATTERN_KEY, JOB_COUNT_PATH_KEY),
        _ => return None,
    };
    Some(crate::could_not_check::tag(
        crate::could_not_check::Reason::Configuration,
        anyhow::anyhow!(
            "`{set}` is set without `{unset}`: the documented job count is compared only when both are set, so this would compare nothing; set both or remove both"
        ),
    ))
}

/// The job count the head side of `doc_path` states, read from the first capture group of
/// `pattern`. `Ok(None)` when the head side has no such file. `Err` is a note for the
/// report: the document could not be read as text, the pattern does not match it, or what
/// it captured is not a number, so nothing was compared. A pattern that cannot capture is
/// a configuration error.
fn documented_job_count(
    ctx: &Context,
    doc_path: &str,
    pattern: &str,
) -> Result<Option<std::result::Result<usize, String>>> {
    let re = super::capture_pattern(pattern, JOB_COUNT_PATTERN_KEY)?;
    let not_compared = NOT_COMPARED;
    // The head side: the index under `--staged`, else the working tree.
    let Some(bytes) = ctx.git.head_bytes(doc_path)? else {
        return Ok(None);
    };
    let Ok(doc) = String::from_utf8(bytes) else {
        return Ok(Some(Err(format!(
            "`{doc_path}` could not be read as text; {not_compared}"
        ))));
    };
    let Some(captured) = re.captures(&doc).and_then(|caps| caps.get(1)) else {
        return Ok(Some(Err(format!(
            "`documented_job_count_pattern` does not match `{doc_path}`; {not_compared}"
        ))));
    };
    Ok(Some(captured.as_str().parse::<usize>().map_err(|_| {
        format!(
            "`documented_job_count_pattern` captured text in `{doc_path}` that is not a number; {not_compared}"
        )
    })))
}

/// Checks the rollup job: the dependencies it kept, the jobs it must depend on, and the
/// documented job count. Returns whether the workflow has the rollup job.
pub(super) fn check_rollup_job(
    ctx: &Context,
    wf: &WorkflowFile,
    out: &mut GateOutcome,
) -> Result<bool> {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let (jobs, rollup_needs) = parse_workflow_jobs(head_content, settings.rollup_job.as_deref());
    let head_all_needs = parse_all_job_needs(head_content);
    check_rollup_needs_kept(ctx, wf, &jobs, &head_all_needs, out);

    let mut rollup_seen = false;
    if let Some(ref rollup) = settings.rollup_job {
        if jobs.contains(rollup) {
            rollup_seen = true;
            let expected: HashSet<String> = jobs
                .iter()
                .filter(|j| *j != rollup && !settings.excluded_jobs.contains(j))
                .cloned()
                .collect();
            // Sorted: a hash set's order differs from one process to the next, and the
            // names go into the message.
            let mut missing: Vec<String> = expected.difference(&rollup_needs).cloned().collect();
            missing.sort_unstable();

            if !missing.is_empty() {
                record_or_excuse(
                    ctx,
                    Some(head_content),
                    out,
                    settings.severity,
                    &crate::findings::ROLLUP_NEEDS_INCOMPLETE,
                    Some(path.to_string()),
                    find_line_number(head_content, rollup),
                    format!("Rollup job '{rollup}' is missing dependencies on: {missing:?}"),
                    format!("Add the missing jobs to '{rollup}' needs: {missing:?}, or excuse with allow-gate-weakening: ci-integrity <reason>."),
                    rollup.as_str(),
                );
            }

            // 2. Documented job count check if configured
            check_documented_job_count(ctx, jobs.len(), out)?;
        }
    }
    Ok(rollup_seen)
}

/// Reports a gate or rollup job that dropped a dependency its base side had on a job the
/// workflow still has.
fn check_rollup_needs_kept(
    ctx: &Context,
    wf: &WorkflowFile,
    jobs: &HashSet<String>,
    head_all_needs: &HashMap<String, HashSet<String>>,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    if let Some(base_src) = wf.base_content {
        let base_all_needs = parse_all_job_needs(base_src);
        // Jobs and their dropped dependencies in name order, so the findings come in
        // the same order on every run.
        let mut base_jobs: Vec<_> = base_all_needs.iter().collect();
        base_jobs.sort_unstable_by_key(|(name, _)| name.as_str());
        for (job_name, base_needs) in base_jobs {
            let is_gate_or_rollup = settings.rollup_job.as_deref() == Some(job_name.as_str())
                || job_name.contains("gate")
                || job_name.contains("rollup");
            if is_gate_or_rollup && jobs.contains(job_name) {
                let head_needs = head_all_needs.get(job_name).cloned().unwrap_or_default();
                let mut dropped_needs: Vec<String> =
                    base_needs.difference(&head_needs).cloned().collect();
                dropped_needs.sort_unstable();
                for dropped in dropped_needs {
                    if jobs.contains(&dropped) {
                        record_or_excuse(
                            ctx,
                            Some(head_content),
                            out,
                            settings.severity,
                            &crate::findings::ROLLUP_NEEDS_REMOVED,
                            Some(path.to_string()),
                            find_line_number(head_content, job_name),
                            format!("Rollup job '{job_name}' dropped dependency on '{dropped}' present in base."),
                            format!("Restore '{dropped}' to '{job_name}' needs, or excuse with allow-gate-weakening: ci-integrity <reason>."),
                            &dropped,
                        );
                    }
                }
            }
        }
    }
}

/// Compares the documented job count, when one is configured, with the number of jobs
/// in the workflow that has the rollup job.
fn check_documented_job_count(
    ctx: &Context,
    job_count: usize,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.ci_integrity;
    if let (Some(doc_path), Some(doc_pattern)) = (
        &settings.documented_job_count_path,
        &settings.documented_job_count_pattern,
    ) {
        match documented_job_count(ctx, doc_path, doc_pattern)? {
            None => out.violations.push(Violation {
                gate: GATE,
                severity: ctx.overridable(settings.severity),
                code: crate::findings::full_code(GATE, &crate::findings::JOB_COUNT_FILE_MISSING),
                fingerprint: String::new(),
                anchor: None,
                legacy_title: None,
                title: crate::findings::JOB_COUNT_FILE_MISSING.title.to_string(),
                file: Some(doc_path.to_string()),
                line: None,
                message: format!("Documented job count file '{doc_path}' does not exist."),
                remediation: Some(
                    "Restore the documentation catalog or update configuration.".to_string(),
                ),
            }),
            Some(Ok(doc_count)) if doc_count != job_count => {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(
                        GATE,
                        &crate::findings::JOB_COUNT_MISMATCH,
                    ),
                    fingerprint: String::new(),
                    title: crate::findings::JOB_COUNT_MISMATCH.title.to_string(),
                    anchor: None,
                    legacy_title: crate::findings::JOB_COUNT_MISMATCH.was_title(),
                    file: Some(doc_path.to_string()),
                    line: None,
                    message: format!(
                        "Documented job count in '{doc_path}' ({doc_count}) does not match workflow jobs count ({}).",
                        job_count
                    ),
                    remediation: Some(format!(
                        "Update the documented count in '{doc_path}' to {} jobs.",
                        job_count
                    )),
                });
            }
            Some(Ok(_)) => {}
            Some(Err(note)) => out.notes.push(note),
        }
    }
    Ok(())
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
