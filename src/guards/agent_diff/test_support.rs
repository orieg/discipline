//! Fixtures the unit tests of the diff gates share.

use super::{evaluate_assertion_reduction, extract_facts, match_tests, pair_helpers, FileFacts};
use crate::ast::{default_registry, AssertVocabulary, TestFn};
use crate::gitctx::{ChangeKind, ChangedFile};

pub(super) fn helper_facts(name: &str, total: usize, strong: usize) -> crate::ast::TestHelperFacts {
    crate::ast::TestHelperFacts {
        name: name.to_string(),
        line: 5,
        end_line: 10,
        total_asserts: total,
        strong_asserts: strong,
        tautologies: 0,
        fatal_asserts: 0,
        helper_checks: 0,
        equality_exits: 0,
    }
}

pub(super) fn calling_test(total: usize, strong: usize, calls: &[&str]) -> TestFn {
    TestFn {
        name: "test_create".to_string(),
        line: 20,
        total_asserts: total,
        strong_asserts: strong,
        direct_calls: calls.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    }
}

/// The facts of a change given as `(path, base source, head source)`; an empty
/// source is a side on which the file does not exist.
pub(super) fn change_facts(files: &[(&str, &str, &str)]) -> Vec<FileFacts> {
    let registry = default_registry();
    let vocab = AssertVocabulary::default();
    files
        .iter()
        .map(|(path, base, head)| {
            let side = |src: &str| {
                (!src.is_empty()).then(|| {
                    extract_facts(registry.find_pack(path).unwrap(), path, src, &vocab).unwrap()
                })
            };
            FileFacts {
                file: ChangedFile {
                    path: path.to_string(),
                    old_path: path.to_string(),
                    kind: ChangeKind::Modified,
                    added_lines: Default::default(),
                },
                base: side(base),
                head: side(head),
                newly_added_nul: false,
            }
        })
        .collect()
}

/// What `assertion-reduction` reports for such a change: `(code, file)` per finding.
pub(super) fn reduction(files: &[(&str, &str, &str)], body: &str) -> Vec<(String, String)> {
    let facts = change_facts(files);
    let (pairs, _, added) = match_tests(&facts);
    let helpers = pair_helpers(&facts, &pairs);
    let directives = crate::tokens::parse_directives(body, crate::tokens::OverrideSource::PrBody);
    let settings = crate::config::AssertionGate::default();
    evaluate_assertion_reduction(&pairs, &added, &helpers, &settings, &directives, false)
        .unwrap()
        .violations
        .iter()
        .map(|v| (v.code.to_string(), v.file.clone().unwrap_or_default()))
        .collect()
}

pub(super) const HELPER_WEAKENED: &str = "assertion-reduction/test-helper-weakened";
pub(super) const PY_CHECK_2: &str = "def check(r):\n    assert r.a == 1\n    assert r.b == 2\n";
pub(super) const PY_CHECK_1: &str = "def check(r):\n    assert r.a == 1\n";
