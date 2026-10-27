//! `deletion-rationale`: tests and benchmarks removed without a stated reason.

use super::{leaf_name, Located, Shadowed};
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
    let check_override =
        |lifts: &crate::findings::FindingKind, subject: &str, alt_subject: Option<&str>| {
            removal_override(settings, directives, lifts, subject, alt_subject)
        };

    for file in changed.iter().filter(|f| f.kind == ChangeKind::Deleted) {
        if !watched.matches(&file.path) || exempt.matches(&file.path) {
            continue;
        }
        out.examined += 1;
        let record = check_override(
            &crate::findings::FILE_DELETED_WITHOUT_RATIONALE,
            &file.path,
            None,
        );
        out.lift_or_push(
            record,
            severity,
            &crate::findings::FILE_DELETED_WITHOUT_RATIONALE,
            (Some(&file.path), None),
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
        let record = check_override(
            &crate::findings::TEST_REMOVED_WITHOUT_RATIONALE,
            leaf_name(r.test),
            Some(r.path),
        );
        let anchored = record.is_none();
        out.lift_or_push(
            record,
            severity,
            &crate::findings::TEST_REMOVED_WITHOUT_RATIONALE,
            (Some(r.path), None),
            format!("Test `{}` was removed from `{}`.", r.test.name, r.path),
            &format!(
                "State why on its own line in the PR body or a commit message: \
                 `removes: {} <reason>`.",
                leaf_name(r.test)
            ),
        );
        if anchored {
            out.anchor_last(r.test.name.clone());
        }
    }
    Ok(out)
}

/// The `removes:` directive that lifts `lifts` for `subject` (or `alt_subject`), if any.
/// `lifts` is the finding the removal would raise (a deleted file, a removed or a
/// shadowed test).
fn removal_override(
    settings: &crate::config::DeletionGate,
    directives: &[crate::tokens::ParsedDirective],
    lifts: &crate::findings::FindingKind,
    subject: &str,
    alt_subject: Option<&str>,
) -> Option<crate::tokens::OverrideRecord> {
    const GATE: &str = "deletion-rationale";
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
                already_in_tree: false,
            })
    };
    rec.filter(|r| !(settings.allow_hidden == Some(false) && r.hidden))
}

/// Base-side tests the change hides behind a later definition of the same name
/// ([`super::shadowed_tests`]): the runner collects the last definition only, so the
/// hidden test no longer runs. Lifted by the same `removes:` line as a removed test.
pub fn report_shadowed_tests(
    shadowed: &[Shadowed],
    settings: &crate::config::DeletionGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
    out: &mut GateOutcome,
) -> Result<()> {
    let exempt = exempt_filter(settings)?;
    let watched = PathFilter::new(&settings.paths)?;
    let severity = if is_staged {
        crate::config::Severity::Warning
    } else {
        settings.severity()
    };
    let kind = &crate::findings::TEST_SHADOWED_BY_REDEFINITION;
    for s in shadowed {
        if !watched.matches(s.path) || exempt.matches(s.path) {
            continue;
        }
        out.examined += 1;
        let name = leaf_name(s.base);
        let record = removal_override(settings, directives, kind, name, Some(s.path));
        out.lift_or_push(
            record,
            severity,
            kind,
            (Some(s.path), Some(s.head.line)),
            format!(
                "Test `{}` at line {} of `{}` is defined again at line {}; the runner collects \
                 the last definition only, so the test at line {} no longer runs.",
                s.base.name, s.head.line, s.path, s.head.by_line, s.head.line
            ),
            &format!(
                "Give one of the two tests another name, or state why on its own line in the \
                 PR body or a commit message: `removes: {name} <reason>`."
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TestFn;

    const BASE: &str =
        "def test_f():\n    assert f(2) == 4\n\n\ndef test_g():\n    assert g(1) == 2\n";

    /// `(code, line)` of each finding.
    type Findings = Vec<(String, Option<usize>)>;

    /// The findings, the number of overrides, and the leaf names of the paired base
    /// tests, for a change to `tests/test_calc.py`.
    fn shadowing(base: &str, head: &str, body: &str) -> (Findings, usize, Vec<String>) {
        let facts = super::super::test_support::change_facts(&[("tests/test_calc.py", base, head)]);
        let (pairs, removed, _) = super::super::match_tests(&facts);
        let directives =
            crate::tokens::parse_directives(body, crate::tokens::OverrideSource::PrBody);
        let settings = crate::config::DeletionGate::default();
        let mut out =
            evaluate_deletion_rationale(&[], &removed, &settings, &directives, false).unwrap();
        report_shadowed_tests(
            &super::super::shadowed_tests(&facts),
            &settings,
            &directives,
            false,
            &mut out,
        )
        .unwrap();
        (
            out.violations
                .iter()
                .map(|v| (v.code.to_string(), v.line))
                .collect(),
            out.overrides.len(),
            pairs
                .iter()
                .map(|p| leaf_name(p.base).to_string())
                .collect(),
        )
    }

    const SHADOWED: &str = "deletion-rationale/test-shadowed-by-redefinition";

    /// A base-side test the head side hides behind a later `def` of its name is reported
    /// once, as shadowed, and is not paired (#827).
    #[test]
    fn a_test_hidden_by_a_redefinition_is_reported() {
        // `test_g` renamed to `test_f`: the first `test_f` no longer runs; `test_g` pairs
        // with the second `test_f` as a rename and is not reported as removed.
        let renamed =
            "def test_f():\n    assert f(2) == 4\n\n\ndef test_f():\n    assert g(1) == 2\n";
        let (found, overrides, paired) = shadowing(BASE, renamed, "");
        assert_eq!(found, vec![(SHADOWED.to_string(), Some(1))]);
        assert_eq!((overrides, paired), (0, vec!["test_g".to_string()]));

        // A new `def test_f` added below the existing one.
        let added = format!("{BASE}\n\ndef test_f():\n    assert f(0) == 0\n");
        let (found, _, paired) = shadowing(BASE, &added, "");
        assert_eq!(found, vec![(SHADOWED.to_string(), Some(1))]);
        assert_eq!(paired, vec!["test_g".to_string()]);

        // Lifted by a `removes:` line naming the hidden test, not by one naming another.
        let (found, overrides, _) =
            shadowing(BASE, renamed, "removes: test_f merged into one test\n");
        assert_eq!((found.len(), overrides), (0, 1));
        let (found, overrides, _) =
            shadowing(BASE, renamed, "removes: test_h merged into one test\n");
        assert_eq!((found.len(), overrides), (1, 0));
    }

    /// Controls: a redefinition already present on the base side hides nothing new, and a
    /// change with no redefinition reports nothing.
    #[test]
    fn a_redefinition_already_on_the_base_side_is_not_reported() {
        let both = format!("{BASE}\n\ndef test_f():\n    assert f(0) == 0\n");
        let edited = both.replace("f(0) == 0", "f(1) == 2");
        let (found, _, paired) = shadowing(&both, &edited, "");
        assert_eq!(found, vec![]);
        // The base side's tests are `test_g` and the second `test_f`, in that order.
        assert_eq!(paired, vec!["test_g".to_string(), "test_f".to_string()]);

        let renamed = BASE.replace("def test_g", "def test_h");
        let (found, _, _) = shadowing(BASE, &renamed, "");
        assert_eq!(found, vec![]);
    }

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
