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
    let checked_count = pr_body.lines().filter(|l| box_re.is_match(l)).count();
    out.examined = checked_count;

    let claims = find_unsupported_claims(pr_body, has_tests, has_docs, has_benches);
    for claim in claims {
        if let Some(ov) = ctx.find_override(GATE, ALLOW_PR_CHECKLIST, claim.subject) {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for {} checklist claim ({})",
                ov.directive, ov.reason, claim.subject, ov.source
            ));
        } else {
            let desc = match claim.subject {
                "test" => {
                    "PR checklist claims tests added or extended, but diff contains zero test files"
                }
                "docs" => {
                    "PR checklist claims documentation updated, but diff contains zero documentation files"
                }
                "bench" => {
                    "PR checklist claims benchmarks updated, but diff contains zero benchmark files"
                }
                _ => "PR checklist claims unsupported change",
            };
            out.add_violation(
                ctx.overridable(settings.severity),
                "PR body",
                claim.line,
                desc,
                format!(
                    "checked item: `{}`; use `discipline:allow(pr-checklist): <reason>` to waive",
                    claim.item_text
                ),
            );
        }
    }

    out.examined = checked_count;
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistClaim {
    pub line: usize,
    pub subject: &'static str,
    pub item_text: String,
}

pub fn find_unsupported_claims(
    pr_body: &str,
    has_tests: bool,
    has_docs: bool,
    has_benches: bool,
) -> Vec<ChecklistClaim> {
    let box_re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$").expect("static regex");
    let mut claims = Vec::new();
    for (idx, line) in pr_body.lines().enumerate() {
        if let Some(caps) = box_re.captures(line) {
            let item_text = caps.get(1).map_or("", |m| m.as_str()).trim();
            let lower = item_text.to_ascii_lowercase();

            if (lower.contains("test") || lower.contains("tests"))
                && !lower.contains("no test")
                && !lower.contains("n/a")
                && !has_tests
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "test",
                    item_text: item_text.to_string(),
                });
            }

            if (lower.contains("doc") || lower.contains("docs") || lower.contains("documentation"))
                && !lower.contains("no doc")
                && !lower.contains("n/a")
                && !has_docs
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "docs",
                    item_text: item_text.to_string(),
                });
            }

            if (lower.contains("bench") || lower.contains("benchmark"))
                && !lower.contains("no bench")
                && !lower.contains("n/a")
                && !has_benches
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "bench",
                    item_text: item_text.to_string(),
                });
            }
        }
    }
    claims
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
