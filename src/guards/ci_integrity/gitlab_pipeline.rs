//! GitLab pipeline files, compared with their base side.

use super::{
    describe_blocking, discipline_pin_changes, discipline_pins, find_line_number, locate_pins,
    record_or_excuse, DisciplinePin,
};
use crate::guards::{Context, GateOutcome};
use anyhow::Result;

/// Diff one GitLab pipeline file against its base side. A side that does not parse is a
/// finding: an unreadable pipeline cannot be shown to be unweakened.
pub(super) fn evaluate_gitlab_file(ctx: &Context, path: &str, out: &mut GateOutcome) -> Result<()> {
    use super::ci_gitlab::verification_jobs;
    let settings = &ctx.config.gates.ci_integrity;
    let base = ctx.git.base_content(path)?;
    let Some(head) = ctx.git.head_content(path)? else {
        let jobs = match base.as_deref().map(verification_jobs) {
            Some(Ok(jobs)) => Some(jobs),
            Some(Err(_)) => {
                out.notes.push(format!(
                    "{path}: the base side of this deleted pipeline could not be parsed, so its verification jobs were not compared"
                ));
                None
            }
            None => None,
        };
        if let Some(jobs) = jobs {
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
    // Pins of the pipeline file itself carry their line; pins of an included file do
    // not (the finding is reported on the pipeline file).
    let pins = |docs: &[String]| -> Vec<DisciplinePin> {
        let mut out = Vec::new();
        for (i, d) in docs.iter().enumerate() {
            let mut file_pins = Vec::new();
            // A document that does not parse ends the read: the deserializer does not
            // advance past it (the diff below reports the file as unreadable).
            for doc in serde_yaml::Deserializer::from_str(d) {
                let Ok(v) = <serde_yaml::Value as serde::Deserialize>::deserialize(doc) else {
                    break;
                };
                file_pins.extend(discipline_pins(&v));
            }
            if i == 0 {
                locate_pins(&mut file_pins, d);
            }
            out.extend(file_pins);
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
            blocking
                .iter()
                .filter_map(|(_, l)| *l)
                .min()
                .or_else(|| find_line_number(&head, "include:")),
            format!(
                "The discipline that judges this change is chosen by the change: {}.",
                describe_blocking(&blocking)
            ),
            "Keep the discipline pin, or move it to a newer immutable release; excuse with allow-gate-weakening: ci-integrity <reason>.",
            "discipline-version",
        );
    }
    match super::ci_gitlab::diff_gitlab_ci_with(&base_docs, &head_docs) {
        Ok(found) => {
            for w in found {
                let line = find_line_number(&head, &format!("{}:", w.job));
                let before = out.violations.len();
                let job = w.job.clone();
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
                // A job that left the pipeline has no line: the job tells it from another.
                if line.is_none() && out.violations.len() > before {
                    out.anchor_last(format!("job:{job}"));
                }
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
