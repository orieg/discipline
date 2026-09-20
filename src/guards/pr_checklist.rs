//! Pull request checklist reconciliation sentinel (`pr-checklist`).
//!
//! Reconciles ticked checklist items in PR descriptions against actual diff contents
//! to catch vacuous checkoffs (e.g. claiming tests/docs were added when 0 were modified).

use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_PR_CHECKLIST;
use anyhow::Result;
use regex::Regex;

pub const GATE: &str = "pr-checklist";

pub fn evaluate_pr_checklist(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.pr_checklist;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let Some(pr_body) = &ctx.pr_body else {
        out.notes
            .push("no PR body supplied; PR checklist reconciliation skipped".to_string());
        return Ok(out);
    };

    let changed = ctx.git.changed_files()?;
    let changed_paths: Vec<&str> = changed.iter().map(|f| f.path.as_str()).collect();

    let has_tests = changed_paths
        .iter()
        .any(|p| p.contains("test") || p.contains("spec") || p.starts_with("tests/"));
    let has_docs = changed_paths
        .iter()
        .any(|p| p.ends_with(".md") || p.starts_with("docs/"));
    let has_benches = changed_paths
        .iter()
        .any(|p| p.contains("bench") || p.starts_with("benches/"));

    let box_re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$").expect("static regex");

    let mut checked_count = 0;
    for (idx, line) in pr_body.lines().enumerate() {
        if let Some(caps) = box_re.captures(line) {
            checked_count += 1;
            let item_text = caps.get(1).map_or("", |m| m.as_str()).trim();
            let lower = item_text.to_ascii_lowercase();

            // Test claim verification
            if (lower.contains("test") || lower.contains("tests"))
                && !lower.contains("no test")
                && !lower.contains("n/a")
                && !has_tests
            {
                if let Some(ov) =
                    ctx.find_gate_or_subject_override(GATE, ALLOW_PR_CHECKLIST, "test")
                {
                    out.notes.push(format!(
                        "override applied: `{}: {}` for test checklist claim ({})",
                        ov.directive, ov.reason, ov.source
                    ));
                } else {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        "PR body",
                        idx + 1,
                        "PR checklist claims tests added or extended, but diff contains zero test files",
                        format!("checked item: `{}`; use `discipline:allow(pr-checklist): <reason>` to waive", item_text),
                    );
                }
            }

            // Docs claim verification
            if (lower.contains("doc") || lower.contains("docs") || lower.contains("documentation"))
                && !lower.contains("no doc")
                && !lower.contains("n/a")
                && !has_docs
            {
                if let Some(ov) =
                    ctx.find_gate_or_subject_override(GATE, ALLOW_PR_CHECKLIST, "docs")
                {
                    out.notes.push(format!(
                        "override applied: `{}: {}` for docs checklist claim ({})",
                        ov.directive, ov.reason, ov.source
                    ));
                } else {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        "PR body",
                        idx + 1,
                        "PR checklist claims documentation updated, but diff contains zero documentation files",
                        format!("checked item: `{}`; use `discipline:allow(pr-checklist): <reason>` to waive", item_text),
                    );
                }
            }

            // Benchmark claim verification
            if (lower.contains("bench") || lower.contains("benchmark"))
                && !lower.contains("no bench")
                && !lower.contains("n/a")
                && !has_benches
            {
                if let Some(ov) =
                    ctx.find_gate_or_subject_override(GATE, ALLOW_PR_CHECKLIST, "bench")
                {
                    out.notes.push(format!(
                        "override applied: `{}: {}` for benchmark checklist claim ({})",
                        ov.directive, ov.reason, ov.source
                    ));
                } else {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        "PR body",
                        idx + 1,
                        "PR checklist claims benchmarks updated, but diff contains zero benchmark files",
                        format!("checked item: `{}`; use `discipline:allow(pr-checklist): <reason>` to waive", item_text),
                    );
                }
            }
        }
    }

    out.examined = checked_count;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::config::{PrChecklistGate, Severity};

    #[test]
    fn test_pr_checklist_defaults() {
        let gate = PrChecklistGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
    }
}
