//! CI/CD workflow integrity and rollup sentinel (`ci-integrity`).
//!
//! Enforces:
//! - Rollup jobs (`ci-gate`) depend on every verification job in the workflow (`needs:`).
//! - Third-party GitHub actions are pinned by a 40-character commit SHA.
//! - Masked failures (`continue-on-error: true`) and error suppression (`|| true`, `set +e`) are forbidden.
//! - Documented job counts in catalogue documentation (e.g. `docs/CI.md`) stay in sync with workflow definitions.

use crate::guards::{exempt_filter, line_allows, Context, GateOutcome, Violation};
use crate::tokens;
use anyhow::{Context as _, Result};
use globset::GlobSetBuilder;
use regex::Regex;
use std::collections::HashSet;
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

    let action_re = Regex::new(r#"uses:\s*['"]?([a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+)@([^'"\s#]+)"#)?;
    let continue_err_re = Regex::new(r"continue-on-error:\s*true\b")?;
    let or_true_re = Regex::new(r"(?:\|\s*true\b|set\s+\+e\b)")?;

    for path in &workflow_files {
        let full = Path::new(ctx.git.root()).join(path);
        if !full.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&full)
            .with_context(|| format!("Failed to read workflow file '{path}'"))?;
        out.examined += 1;

        let added_lines = added_lines_map.as_ref().and_then(|m| m.get(path));

        // 1. Rollup job checks
        let (jobs, rollup_needs) = parse_workflow_jobs(&content, settings.rollup_job.as_deref());
        if let Some(ref rollup) = settings.rollup_job {
            if jobs.contains(rollup) {
                let expected: HashSet<String> = jobs
                    .iter()
                    .filter(|j| *j != rollup && !settings.excluded_jobs.contains(j))
                    .cloned()
                    .collect();
                let missing: Vec<String> = expected.difference(&rollup_needs).cloned().collect();

                if !missing.is_empty() {
                    let subject = rollup.as_str();
                    if let Some(ov) = ctx
                        .find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, subject)
                        .or_else(|| {
                            ctx.find_gate_or_subject_override(
                                GATE,
                                tokens::ALLOW_CI_WEAKENING,
                                path,
                            )
                        })
                    {
                        out.overrides.push(ov);
                    } else {
                        out.violations.push(Violation {
                            gate: GATE,
                            severity: ctx.overridable(settings.severity),
                            title: "Incomplete Rollup Job Needs".to_string(),
                            file: Some(path.clone()),
                            line: None,
                            message: format!(
                                "Rollup job '{rollup}' is missing dependencies on: {missing:?}"
                            ),
                            remediation: Some(format!(
                                "Add the missing jobs to '{rollup}' needs: {missing:?}, or excuse with allow-ci-weakening."
                            )),
                        });
                    }
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

        // 3. Line-by-line checks
        for (idx, line) in content.lines().enumerate() {
            let line_no = idx + 1;
            if let Some(added) = added_lines {
                if !added.contains(&line_no) {
                    continue;
                }
            }

            if line_allows(line, GATE) {
                continue;
            }

            // Action pinning
            if settings.pin_actions {
                if let Some(caps) = action_re.captures(line) {
                    let action = &caps[1];
                    let ref_str = &caps[2];
                    if !action.starts_with("./") && !action.starts_with("docker://") {
                        let is_sha =
                            ref_str.len() == 40 && ref_str.chars().all(|c| c.is_ascii_hexdigit());
                        if !is_sha {
                            if let Some(ov) = ctx
                                .find_gate_or_subject_override(
                                    GATE,
                                    tokens::ALLOW_CI_WEAKENING,
                                    action,
                                )
                                .or_else(|| {
                                    ctx.find_gate_or_subject_override(
                                        GATE,
                                        tokens::ALLOW_CI_WEAKENING,
                                        "unpinned",
                                    )
                                })
                            {
                                out.overrides.push(ov);
                            } else {
                                out.violations.push(Violation {
                                    gate: GATE,
                                    severity: ctx.overridable(settings.severity),
                                    title: "Unpinned Third-Party Action".to_string(),
                                    file: Some(path.clone()),
                                    line: Some(line_no),
                                    message: format!(
                                        "Third-party action '{action}' is unpinned ('@{ref_str}'). Must be pinned by a 40-character commit SHA."
                                    ),
                                    remediation: Some(
                                        "Pin the action by its immutable 40-character commit SHA.".to_string(),
                                    ),
                                });
                            }
                        }
                    }
                }
            }

            // continue-on-error
            if settings.forbid_continue_on_error && continue_err_re.is_match(line) {
                if let Some(ov) = ctx
                    .find_gate_or_subject_override(
                        GATE,
                        tokens::ALLOW_CI_WEAKENING,
                        "continue-on-error",
                    )
                    .or_else(|| {
                        ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, path)
                    })
                {
                    out.overrides.push(ov);
                } else {
                    out.violations.push(Violation {
                        gate: GATE,
                        severity: ctx.overridable(settings.severity),
                        title: "continue-on-error Masks Failure".to_string(),
                        file: Some(path.clone()),
                        line: Some(line_no),
                        message:
                            "Step carries 'continue-on-error: true', which masks failures in CI."
                                .to_string(),
                        remediation: Some(
                            "Remove continue-on-error or provide an allow-ci-weakening directive."
                                .to_string(),
                        ),
                    });
                }
            }

            // Error suppression
            if settings.forbid_or_true && or_true_re.is_match(line) {
                if let Some(ov) = ctx
                    .find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, "or-true")
                    .or_else(|| {
                        ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_CI_WEAKENING, path)
                    })
                {
                    out.overrides.push(ov);
                } else {
                    out.violations.push(Violation {
                        gate: GATE,
                        severity: ctx.overridable(settings.severity),
                        title: "Command Masks Exit Code".to_string(),
                        file: Some(path.clone()),
                        line: Some(line_no),
                        message: "Command uses '|| true' or 'set +e' to mask command failure."
                            .to_string(),
                        remediation: Some(
                            "Remove '|| true' or provide an allow-ci-weakening directive."
                                .to_string(),
                        ),
                    });
                }
            }
        }
    }

    Ok(out)
}

/// Parses job IDs and the rollup job's needed job IDs from GitHub Actions workflow YAML.
pub fn parse_workflow_jobs(
    content: &str,
    rollup_name: Option<&str>,
) -> (HashSet<String>, HashSet<String>) {
    let mut jobs = HashSet::new();
    let mut rollup_needs = HashSet::new();
    let mut in_jobs = false;
    let mut current_job: Option<String> = None;
    let mut in_needs_list = false;

    let job_decl_re = Regex::new(r"^  ([a-zA-Z0-9_-]+):\s*(?:#.*)?$").unwrap();
    let needs_inline_re = Regex::new(r"^    needs:\s*\[([^\]]+)\]").unwrap();
    let needs_single_re = Regex::new(r"^    needs:\s*([a-zA-Z0-9_-]+)\s*(?:#.*)?$").unwrap();
    let needs_list_start_re = Regex::new(r"^    needs:\s*(?:#.*)?$").unwrap();
    let needs_item_re = Regex::new(r"^[ \t]+-\s*([a-zA-Z0-9_-]+)").unwrap();

    for line in content.lines() {
        if line.starts_with("jobs:") {
            in_jobs = true;
            current_job = None;
            in_needs_list = false;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if !line.starts_with(' ') && !line.trim().is_empty() && !line.trim_start().starts_with('#')
        {
            break;
        }

        if let Some(caps) = job_decl_re.captures(line) {
            let job = caps[1].to_string();
            jobs.insert(job.clone());
            current_job = Some(job);
            in_needs_list = false;
            continue;
        }

        let is_rollup = current_job.as_deref() == rollup_name;
        if is_rollup {
            if let Some(caps) = needs_inline_re.captures(line) {
                for item in caps[1].split(',') {
                    let trimmed = item.trim().trim_matches(|c| c == '\'' || c == '"');
                    if !trimmed.is_empty() {
                        rollup_needs.insert(trimmed.to_string());
                    }
                }
                in_needs_list = false;
                continue;
            }
            if let Some(caps) = needs_single_re.captures(line) {
                rollup_needs.insert(caps[1].to_string());
                in_needs_list = false;
                continue;
            }
            if needs_list_start_re.is_match(line) {
                in_needs_list = true;
                continue;
            }
            if in_needs_list {
                if let Some(caps) = needs_item_re.captures(line) {
                    rollup_needs.insert(caps[1].to_string());
                } else if line.starts_with("    ")
                    && !line.trim().is_empty()
                    && !line.trim().starts_with('#')
                {
                    in_needs_list = false;
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
        assert!(needs.contains("detect-changes"));
        assert!(needs.contains("lint"));
    }
}
