//! `deletion-rationale`: tests and benchmarks removed without a stated reason.

use super::{leaf_name, Located, Shadowed, TestPair};
use crate::ast::TestFn;
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

/// Removed tests merged into a test named for their container (#911): the change turns
/// `describe('P', ..)` holding `P > a`, `P > b` and `P > c` into one `test('P', ..)`.
/// `match_tests` pairs one of them with the new test and leaves the others removed,
/// though their checks run in it. Such a removed test is not a removal when:
///
/// - the container is a head test of the same file that did not exist on the base side
///   (the head of a pair whose base test is nested under its name, of a forced pair, or
///   an added test), is not ignored, and its name is the removed test's name up to a
///   separator (` > `, `::`, `#`);
/// - the container holds at least as many assertions that are not tautologies, and as
///   many strong ones, as every base test under its name that it took over (the removed
///   ones and the one paired with it), and every expected value they checked, as written
///   (`TestFn::expectations`, by skeleton and literal).
///
/// Returns each such removed test's place in `removed` with its container's name. A
/// merge that drops an assertion, or any other consolidation, is still a removal;
/// `assertion-reduction` judges the paired test's count as before.
pub fn merged_into_container<'a>(
    removed: &[Located<'a>],
    pairs: &[TestPair<'a>],
    added: &[Located<'a>],
) -> Vec<(usize, &'a str)> {
    const SEPARATORS: &[&str] = &[" > ", "::", "#"];
    let under = |child: &str, parent: &str| {
        child
            .strip_prefix(parent)
            .is_some_and(|rest| SEPARATORS.iter().any(|s| rest.starts_with(s)))
    };
    let containers = pairs
        .iter()
        .filter(|p| p.forced || under(&p.base.name, &p.head.name))
        .map(|p| (p.path, p.head))
        .chain(added.iter().map(|a| (a.path, a.test)));
    let mut out = Vec::new();
    for (path, container) in containers {
        if container.ignored {
            continue;
        }
        let members: Vec<usize> = removed
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.file_survives && r.path == path && under(&r.test.name, &container.name)
            })
            .map(|(i, _)| i)
            .collect();
        if members.is_empty() {
            continue;
        }
        let taken_over: Vec<&TestFn> = members
            .iter()
            .map(|&i| removed[i].test)
            .chain(
                pairs
                    .iter()
                    .filter(|p| {
                        p.path == path
                            && std::ptr::eq(p.head, container)
                            && under(&p.base.name, &container.name)
                    })
                    .map(|p| p.base),
            )
            .collect();
        let effective = |f: &TestFn| f.total_asserts.saturating_sub(f.tautologies);
        let (total, strong) = taken_over
            .iter()
            .fold((0, 0), |(t, s), b| (t + effective(b), s + b.strong_asserts));
        // Every expected value they checked is checked, as written, in the container.
        let mut expected: Vec<(&str, &str)> = container
            .expectations
            .iter()
            .map(|e| (e.skeleton.as_str(), e.literal.as_str()))
            .collect();
        let kept = taken_over.iter().flat_map(|b| &b.expectations).all(|e| {
            expected
                .iter()
                .position(|&x| x == (e.skeleton.as_str(), e.literal.as_str()))
                .map(|i| expected.swap_remove(i))
                .is_some()
        });
        if kept && effective(container) >= total && container.strong_asserts >= strong {
            out.extend(members.into_iter().map(|i| (i, container.name.as_str())));
        }
    }
    out
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

    /// Tests merged into one named for their container are not removals (#911): the
    /// shape of `vitejs/vite#23604`, a `describe` turned into a `test` that runs every
    /// nested test's assertions. The controls are each still removals: the merge drops
    /// an assertion (strong, or weak with the strong count kept), the container is
    /// ignored, the container is in another file, and a name that only starts with the
    /// container's.
    #[test]
    fn tests_merged_into_their_container_are_not_removals() {
        let t = |name: &str, total: usize, strong: usize| TestFn {
            name: name.to_string(),
            total_asserts: total,
            strong_asserts: strong,
            ..Default::default()
        };
        let path = "test/p.spec.ts";
        let resolve = t("build > resolveId", 2, 2);
        let load = t("build > load", 2, 2);
        let transform = t("build > transform", 4, 4);
        let merged_ids = |container: &TestFn, container_path: &str, removed: &[&TestFn]| {
            let pairs = [TestPair {
                path,
                base: &resolve,
                head: container,
                forced: false,
            }];
            let pairs: Vec<TestPair> = pairs
                .into_iter()
                .filter(|p| p.path == container_path)
                .collect();
            let added: Vec<Located> = if container_path == path {
                Vec::new()
            } else {
                vec![Located {
                    path: container_path,
                    file_survives: true,
                    test: container,
                }]
            };
            let removed: Vec<Located> = removed
                .iter()
                .map(|r| Located {
                    path,
                    file_survives: true,
                    test: r,
                })
                .collect();
            merged_into_container(&removed, &pairs, &added)
                .into_iter()
                .map(|(i, c)| (removed[i].test.name.clone(), c.to_string()))
                .collect::<Vec<_>>()
        };
        let build = t("build", 8, 8);
        assert_eq!(
            merged_ids(&build, path, &[&load, &transform]),
            vec![
                ("build > load".to_string(), "build".to_string()),
                ("build > transform".to_string(), "build".to_string())
            ]
        );
        // Controls.
        assert!(merged_ids(&t("build", 7, 7), path, &[&load, &transform]).is_empty());
        assert!(merged_ids(&t("build", 8, 7), path, &[&load, &transform]).is_empty());
        // A weak assertion dropped in the merge: the strong count still holds, the total
        // does not.
        let weak_load = t("build > load", 2, 1);
        assert!(merged_ids(&t("build", 7, 7), path, &[&weak_load, &transform]).is_empty());
        // A merged check turned into a tautology does not count.
        let mut tautology = t("build", 8, 8);
        tautology.tautologies = 1;
        assert!(merged_ids(&tautology, path, &[&load, &transform]).is_empty());
        let mut ignored = t("build", 8, 8);
        ignored.ignored = true;
        assert!(merged_ids(&ignored, path, &[&load, &transform]).is_empty());
        assert!(merged_ids(&build, "test/other.spec.ts", &[&load, &transform]).is_empty());
        // A name that only starts with the container's is not under it: `build > loader`
        // is not under an added `build > load`; `build > load > x` is.
        let container = t("build > load", 8, 8);
        let added = [Located {
            path,
            file_survives: true,
            test: &container,
        }];
        let loader = t("build > loader", 2, 2);
        let nested = t("build > load > x", 2, 2);
        let removed = [&loader, &nested].map(|r| Located {
            path,
            file_survives: true,
            test: r,
        });
        let found: Vec<&str> = merged_into_container(&removed, &[], &added)
            .into_iter()
            .map(|(i, _)| removed[i].test.name.as_str())
            .collect();
        assert_eq!(found, vec!["build > load > x"]);
    }
}
