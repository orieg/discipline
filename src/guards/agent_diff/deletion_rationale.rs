//! `deletion-rationale`: tests and benchmarks removed without a stated reason.

use super::{leaf_name, Located};
use crate::config::GateSettings;
use crate::gitctx::{ChangeKind, ChangedFile};
use crate::guards::{exempt_filter, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::Result;

pub fn evaluate_deletion_rationale(
    changed: &[ChangedFile],
    removed: &[Located],
    settings: &crate::config::DeletionGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
) -> Result<GateOutcome> {
    const GATE: &str = "deletion-rationale";
    let exempt = exempt_filter(settings)?;
    let watched = PathFilter::new(&settings.paths)?;
    let severity = if is_staged {
        crate::config::Severity::Warning
    } else {
        settings.severity()
    };
    let mut out = GateOutcome::new(GATE);

    // `lifts`: the finding the deletion would raise (a deleted file or a removed test).
    let check_override = |lifts: &crate::findings::FindingKind,
                          subject: &str,
                          alt_subject: Option<&str>|
     -> Option<crate::tokens::OverrideRecord> {
        let rec = if settings.require_scope {
            tokens::find_override(directives, GATE, lifts, tokens::REMOVES, subject).or_else(|| {
                alt_subject.and_then(|alt| {
                    tokens::find_override(directives, GATE, lifts, tokens::REMOVES, alt)
                })
            })
        } else {
            directives
                .iter()
                .find(|d| {
                    tokens::REMOVES
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(&d.directive))
                })
                .map(|d| crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    code: Some(crate::findings::full_code(GATE, lifts)),
                    subject: subject.to_string(),
                    directive: d.directive.clone(),
                    reason: d.reason.clone(),
                    source: d.source.clone(),
                    hidden: d.hidden,
                })
        };
        if let Some(r) = rec {
            if settings.allow_hidden == Some(false) && r.hidden {
                return None;
            }
            return Some(r);
        }
        None
    };

    for file in changed.iter().filter(|f| f.kind == ChangeKind::Deleted) {
        if !watched.matches(&file.path) || exempt.matches(&file.path) {
            continue;
        }
        out.examined += 1;
        if let Some(record) = check_override(
            &crate::findings::FILE_DELETED_WITHOUT_RATIONALE,
            &file.path,
            None,
        ) {
            out.overrides.push(record);
            continue;
        }
        out.push(
            severity,
            &crate::findings::FILE_DELETED_WITHOUT_RATIONALE,
            Some(&file.path),
            None,
            format!(
                "`{}` was deleted and no `removes:` directive names it.",
                file.path
            ),
            &format!(
                "State why on its own line in the PR body or a commit message: \
                 `removes: {} <reason>` (a directory prefix covers everything under it).",
                file.path
            ),
        );
    }

    // Tests removed from a file that still exists. Because match_tests force-pairs
    // tests 1-to-1 within each file, any tests remaining in `removed` are genuine
    // surplus deletions (removed tests outnumber added tests in that file).
    // Tests inside a deleted file are covered by that file's own rationale.
    for r in removed.iter().filter(|r| r.file_survives) {
        if !watched.matches(r.path) || exempt.matches(r.path) {
            continue;
        }
        out.examined += 1;
        if let Some(record) = check_override(
            &crate::findings::TEST_REMOVED_WITHOUT_RATIONALE,
            leaf_name(r.test),
            Some(r.path),
        ) {
            out.overrides.push(record);
            continue;
        }
        out.push(
            severity,
            &crate::findings::TEST_REMOVED_WITHOUT_RATIONALE,
            Some(r.path),
            None,
            format!("Test `{}` was removed from `{}`.", r.test.name, r.path),
            &format!(
                "State why on its own line in the PR body or a commit message: \
                 `removes: {} <reason>`.",
                leaf_name(r.test)
            ),
        );
        out.anchor_last(r.test.name.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TestFn;

    #[test]
    fn test_deletion_rationale_pure() {
        let settings = crate::config::DeletionGate::default();
        let deleted_files = [ChangedFile {
            path: "src/legacy.rs".into(),
            old_path: "src/legacy.rs".into(),
            kind: ChangeKind::Deleted,
            added_lines: std::collections::BTreeSet::new(),
        }];

        // File deletion without rationale
        let out_unexcused =
            evaluate_deletion_rationale(&deleted_files, &[], &settings, &[], false).unwrap();
        assert_eq!(out_unexcused.violations.len(), 1);

        // File deletion with rationale
        let directives = [crate::tokens::ParsedDirective {
            directive: "removes".to_string(),
            reason: "src/legacy.rs removed in v2".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_excused =
            evaluate_deletion_rationale(&deleted_files, &[], &settings, &directives, false)
                .unwrap();
        assert_eq!(out_excused.violations.len(), 0);
        assert_eq!(out_excused.overrides.len(), 1);

        // Test removal from surviving file without rationale
        let t = TestFn {
            name: "test_old".to_string(),
            line: 5,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let removed_tests = [Located {
            path: "tests/suite.rs",
            file_survives: true,
            test: &t,
        }];
        let out_test_unexcused =
            evaluate_deletion_rationale(&[], &removed_tests, &settings, &[], false).unwrap();
        assert_eq!(out_test_unexcused.violations.len(), 1);

        // Test removal with rationale
        let test_directive = [crate::tokens::ParsedDirective {
            directive: "removes".to_string(),
            reason: "test_old superseded".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_test_excused =
            evaluate_deletion_rationale(&[], &removed_tests, &settings, &test_directive, false)
                .unwrap();
        assert_eq!(out_test_excused.violations.len(), 0);
        assert_eq!(out_test_excused.overrides.len(), 1);
    }
}
