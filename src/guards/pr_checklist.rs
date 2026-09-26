//! Pull request checklist reconciliation sentinel (`pr-checklist`).
//!
//! Reconciles ticked checklist items in PR descriptions against actual diff contents
//! to catch vacuous checkoffs (e.g. claiming tests/docs were added when 0 were modified).
//!
//! A test claim is backed by a changed test file, or by a test function the
//! change adds or extends in any file the language packs analyse: most Rust
//! unit tests live in a `mod tests` inside the source file they cover.

use crate::ast::{default_registry, TestFn};
use crate::gitctx::ChangeKind;
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

    let mut has_tests = changed_paths
        .iter()
        .any(|p| p.contains("test") || p.contains("spec") || p.starts_with("tests/"));
    if !has_tests {
        let added = count_added_test_functions(ctx, &changed, &mut out.notes)?;
        if added > 0 {
            out.notes.push(format!(
                "test claim backed by {added} test function(s) added or extended in changed source files"
            ));
            has_tests = true;
        }
    }
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
            let (kind, desc) = match claim.subject {
                "test" => (
                    &crate::findings::CHECKLIST_CLAIMS_TESTS,
                    "PR checklist claims tests added or extended, but diff adds or extends no test file or test function",
                ),
                "docs" => (
                    &crate::findings::CHECKLIST_CLAIMS_DOCS,
                    "PR checklist claims documentation updated, but diff contains zero documentation files",
                ),
                "bench" => (
                    &crate::findings::CHECKLIST_CLAIMS_BENCHMARKS,
                    "PR checklist claims benchmarks updated, but diff contains zero benchmark files",
                ),
                _ => (
                    &crate::findings::CHECKLIST_CLAIM_UNSUPPORTED,
                    "PR checklist claims unsupported change",
                ),
            };
            out.add_violation(
                ctx.overridable(settings.severity),
                kind,
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

/// Test functions the change adds or extends across every changed file a
/// language pack analyses. A file the pack cannot extract adds nothing and is
/// named in `notes`, so an unreadable file never backs a claim.
fn count_added_test_functions(
    ctx: &Context,
    changed: &[crate::gitctx::ChangedFile],
    notes: &mut Vec<String>,
) -> Result<usize> {
    let registry = default_registry();
    let vocab = crate::guards::agent_diff::assert_vocabulary(ctx.config);
    let mut total = 0;
    for file in changed.iter().filter(|f| f.kind != ChangeKind::Deleted) {
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(facts) => facts.tests,
            Err(e) => {
                notes.push(format!(
                    "{}: could not extract tests ({e}); not counted as test evidence",
                    file.path
                ));
                continue;
            }
        };
        let base = match ctx.git.base_content(&file.old_path)? {
            Some(src) => match registry.find_pack(&file.old_path) {
                Some(base_pack) => match base_pack.extract(&file.old_path, &src, &vocab) {
                    Ok(facts) => facts.tests,
                    Err(e) => {
                        notes.push(format!(
                            "{}: could not extract base tests ({e}); not counted as test evidence",
                            file.old_path
                        ));
                        continue;
                    }
                },
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        total += added_or_extended_tests(&base, &head);
    }
    Ok(total)
}

/// Tests the head side adds or extends relative to the base side of one file.
///
/// A head test with no base test of the same name is added; a same-named test
/// whose effective assertions grew is extended. Renames are netted out: a
/// test renamed without new assertions adds nothing.
pub fn added_or_extended_tests(base: &[TestFn], head: &[TestFn]) -> usize {
    let mut unmatched_base: Vec<&TestFn> = base.iter().collect();
    let mut unmatched_head = 0usize;
    let mut extended = 0usize;
    for h in head {
        match unmatched_base.iter().position(|b| b.name == h.name) {
            Some(i) => {
                let b = unmatched_base.swap_remove(i);
                if h.effective_asserts() > b.effective_asserts() {
                    extended += 1;
                }
            }
            None => unmatched_head += 1,
        }
    }
    unmatched_head.saturating_sub(unmatched_base.len()) + extended
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
    use super::added_or_extended_tests;
    use crate::ast::TestFn;
    use crate::config::{PrChecklistGate, Severity};

    fn t(name: &str, asserts: usize) -> TestFn {
        TestFn {
            name: name.to_string(),
            total_asserts: asserts,
            ..Default::default()
        }
    }

    #[test]
    fn added_or_extended_tests_counts_new_and_grown_tests_only() {
        let base = [t("tests::doubles", 1)];
        // Positive: a new test beside an existing one.
        assert_eq!(
            added_or_extended_tests(&base, &[t("tests::doubles", 1), t("tests::triples", 1)]),
            1
        );
        // Positive: an existing test gains an assertion.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubles", 2)]), 1);
        // Negative: nothing about the tests changed.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubles", 1)]), 0);
        // Negative: a rename is not a new test.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubled", 1)]), 0);
        // Negative: a test removed.
        assert_eq!(added_or_extended_tests(&base, &[]), 0);
        // Positive: a new file's tests are all new.
        assert_eq!(added_or_extended_tests(&[], &[t("a", 0), t("b", 1)]), 2);
    }

    #[test]
    fn test_pr_checklist_defaults() {
        let gate = PrChecklistGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
    }
}
