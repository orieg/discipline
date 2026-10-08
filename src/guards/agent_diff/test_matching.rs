//! Pairing of the tests of the base side with those of the head side.

use super::{FileFacts, Located, TestPair};
use crate::ast::TestFn;

pub const RENAME_NAME_SIMILARITY_THRESHOLD: f64 = 0.5;

pub fn name_similarity(a: &str, b: &str) -> f64 {
    let a_leaf = a.rsplit("::").next().unwrap_or(a);
    let b_leaf = b.rsplit("::").next().unwrap_or(b);
    if a_leaf == b_leaf {
        return 1.0;
    }
    if a_leaf.is_empty() || b_leaf.is_empty() {
        return 0.0;
    }
    // Character bigram Sorensen-Dice similarity
    let bigrams = |s: &str| -> Vec<(char, char)> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 2 {
            return Vec::new();
        }
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let bg_a = bigrams(a_leaf);
    let bg_b = bigrams(b_leaf);
    let bigram_sim = if bg_a.is_empty() || bg_b.is_empty() {
        if a_leaf == b_leaf {
            1.0
        } else {
            0.0
        }
    } else {
        let mut matches = 0;
        let mut b_used = vec![false; bg_b.len()];
        for ga in &bg_a {
            for (i, gb) in bg_b.iter().enumerate() {
                if !b_used[i] && ga == gb {
                    b_used[i] = true;
                    matches += 1;
                    break;
                }
            }
        }
        (2.0 * matches as f64) / ((bg_a.len() + bg_b.len()) as f64)
    };

    // Word-level token Dice similarity
    let clean_a = a_leaf.strip_prefix("test_").unwrap_or(a_leaf);
    let clean_b = b_leaf.strip_prefix("test_").unwrap_or(b_leaf);
    let w_a: Vec<&str> = clean_a
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let w_b: Vec<&str> = clean_b
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let word_sim = if !w_a.is_empty() && !w_b.is_empty() {
        let mut w_matches = 0;
        let mut wb_used = vec![false; w_b.len()];
        for wa in &w_a {
            for (i, wb) in w_b.iter().enumerate() {
                if !wb_used[i] && wa == wb {
                    wb_used[i] = true;
                    w_matches += 1;
                    break;
                }
            }
        }
        (2.0 * w_matches as f64) / ((w_a.len() + w_b.len()) as f64)
    } else {
        0.0
    };

    bigram_sim.max(word_sim)
}

/// Pair tests by name within a file, then pair the leftovers across files so a
/// test moved to another file is compared instead of reported as removed+new.
pub fn match_tests(files: &[FileFacts]) -> (Vec<TestPair<'_>>, Vec<Located<'_>>, Vec<Located<'_>>) {
    let mut pairs = Vec::new();
    let mut removed: Vec<Located> = Vec::new();
    let mut added: Vec<Located> = Vec::new();

    struct FileUnmatched<'a> {
        path: &'a str,
        file_survives: bool,
        base: Vec<&'a TestFn>,
        head: Vec<(usize, &'a TestFn)>,
    }

    let mut file_unmatched: Vec<FileUnmatched> = Vec::new();

    for ff in files {
        let base_tests: &[TestFn] = ff.base.as_ref().map(|f| &f.tests[..]).unwrap_or(&[]);
        let head_tests: &[TestFn] = ff.head.as_ref().map(|f| &f.tests[..]).unwrap_or(&[]);
        let mut taken = vec![false; head_tests.len()];
        let mut unmatched_base = Vec::new();

        // 1. Exact name match within file
        for b in base_tests {
            let hit = head_tests
                .iter()
                .enumerate()
                .find(|(i, h)| !taken[*i] && h.name == b.name);
            match hit {
                Some((i, h)) => {
                    taken[i] = true;
                    pairs.push(TestPair {
                        path: &ff.file.path,
                        base: b,
                        head: h,
                        forced: false,
                    });
                }
                None => unmatched_base.push(b),
            }
        }

        // 2. Name similarity pairing within file for renames
        let mut unmatched_head: Vec<(usize, &TestFn)> = head_tests
            .iter()
            .enumerate()
            .filter(|(i, _)| !taken[*i])
            .collect();

        let mut still_unmatched_base = Vec::new();
        for b in unmatched_base {
            let best = unmatched_head
                .iter()
                .enumerate()
                .map(|(idx, &(orig_i, h))| (idx, orig_i, h, name_similarity(&b.name, &h.name)))
                .filter(|&(_, _, _, sim)| sim >= RENAME_NAME_SIMILARITY_THRESHOLD)
                .max_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));

            if let Some((idx, orig_i, h, _sim)) = best {
                taken[orig_i] = true;
                unmatched_head.remove(idx);
                pairs.push(TestPair {
                    path: &ff.file.path,
                    base: b,
                    head: h,
                    forced: false,
                });
            } else {
                still_unmatched_base.push(b);
            }
        }

        file_unmatched.push(FileUnmatched {
            path: &ff.file.path,
            file_survives: ff.head.is_some(),
            base: still_unmatched_base,
            head: unmatched_head,
        });
    }

    // 3. Exact leaf name match across files (e.g. test moved to another file)
    fn str_leaf(s: &str) -> &str {
        s.rsplit("::").next().unwrap_or(s)
    }
    for i in 0..file_unmatched.len() {
        let mut b_idx = 0;
        while b_idx < file_unmatched[i].base.len() {
            let b_leaf = str_leaf(&file_unmatched[i].base[b_idx].name);
            let mut matched = false;
            for j in 0..file_unmatched.len() {
                if i == j {
                    continue;
                }
                if let Some(pos) = file_unmatched[j]
                    .head
                    .iter()
                    .position(|(_, h)| str_leaf(&h.name) == b_leaf)
                {
                    let b = file_unmatched[i].base.remove(b_idx);
                    let (_, h) = file_unmatched[j].head.remove(pos);
                    pairs.push(TestPair {
                        path: file_unmatched[j].path,
                        base: b,
                        head: h,
                        forced: false,
                    });
                    matched = true;
                    break;
                }
            }
            if !matched {
                b_idx += 1;
            }
        }
    }

    // 4. Forced 1-to-1 pairing within file for remaining tests
    for fu in &mut file_unmatched {
        while !fu.base.is_empty() && !fu.head.is_empty() {
            let mut best_pair: Option<(usize, usize, f64)> = None;
            for (b_idx, b) in fu.base.iter().enumerate() {
                for (h_idx, &(_, h)) in fu.head.iter().enumerate() {
                    let sim = name_similarity(&b.name, &h.name);
                    match best_pair {
                        None => best_pair = Some((b_idx, h_idx, sim)),
                        Some((_, _, best_sim)) => {
                            if sim > best_sim {
                                best_pair = Some((b_idx, h_idx, sim));
                            }
                        }
                    }
                }
            }
            if let Some((b_idx, h_idx, _)) = best_pair {
                let b = fu.base.remove(b_idx);
                let (_, h) = fu.head.remove(h_idx);
                pairs.push(TestPair {
                    path: fu.path,
                    base: b,
                    head: h,
                    forced: true,
                });
            } else {
                break;
            }
        }

        // Any remaining base tests are genuine surplus removals
        for b in fu.base.drain(..) {
            removed.push(Located {
                path: fu.path,
                file_survives: fu.file_survives,
                test: b,
            });
        }

        // Any remaining head tests are genuine surplus additions
        for (_, h) in fu.head.drain(..) {
            added.push(Located {
                path: fu.path,
                file_survives: true,
                test: h,
            });
        }
    }

    // 5. Compile-time assertion pairing
    for ff in files {
        if let (Some(b_facts), Some(h_facts)) = (&ff.base, &ff.head) {
            if let (Some(b), Some(h)) = (&b_facts.compile_time_test, &h_facts.compile_time_test) {
                if b.total_asserts > 0 || h.total_asserts > 0 {
                    pairs.push(TestPair {
                        path: &ff.file.path,
                        base: b,
                        head: h,
                        forced: false,
                    });
                }
            }
        }
    }

    (pairs, removed, added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::RustFacts;
    use crate::gitctx::{ChangeKind, ChangedFile};

    #[test]
    fn name_similarity_discriminates_renames_from_unrelated_tests() {
        assert_eq!(name_similarity("adds", "adds"), 1.0);
        assert!(name_similarity("adds", "adds_integers") >= RENAME_NAME_SIMILARITY_THRESHOLD);
        assert!(
            name_similarity(
                "ablation_sharded_alloc_counts_per_stripe",
                "alloc_counts_per_stripe"
            ) >= RENAME_NAME_SIMILARITY_THRESHOLD
        );
        assert!(
            name_similarity(
                "str_node_is_just_the_map_core",
                "map_core_is_just_the_str_node"
            ) >= RENAME_NAME_SIMILARITY_THRESHOLD
        );
        assert!(name_similarity("test_alpha", "test_beta") < RENAME_NAME_SIMILARITY_THRESHOLD);
        assert!(name_similarity("adds", "orders") < RENAME_NAME_SIMILARITY_THRESHOLD);
    }

    #[test]
    fn match_tests_force_pairs_unrelated_tests_in_same_file() {
        let t1 = TestFn {
            name: "test_alpha".to_string(),
            line: 10,
            total_asserts: 3,
            strong_asserts: 2,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let t2 = TestFn {
            name: "test_omega".to_string(),
            line: 20,
            total_asserts: 1,
            strong_asserts: 0,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let facts = vec![FileFacts {
            file: ChangedFile {
                path: "tests/foo.rs".into(),
                old_path: "tests/foo.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: std::collections::BTreeSet::new(),
            },
            base: Some(RustFacts {
                tests: vec![t1],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            head: Some(RustFacts {
                tests: vec![t2],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];

        let (pairs, removed, added) = match_tests(&facts);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].forced);
        assert_eq!(pairs[0].base.name, "test_alpha");
        assert_eq!(pairs[0].head.name, "test_omega");
        assert!(removed.is_empty());
        assert!(added.is_empty());
    }

    #[test]
    fn match_tests_surplus_base_goes_to_removed_and_surplus_head_to_added() {
        let b1 = TestFn {
            name: "b1".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let b2 = TestFn {
            name: "b2".to_string(),
            line: 5,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let h1 = TestFn {
            name: "h1".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let facts = vec![FileFacts {
            file: ChangedFile {
                path: "tests/foo.rs".into(),
                old_path: "tests/foo.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: std::collections::BTreeSet::new(),
            },
            base: Some(RustFacts {
                tests: vec![b1, b2],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            head: Some(RustFacts {
                tests: vec![h1],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];

        let (pairs, removed, added) = match_tests(&facts);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].forced);
        assert_eq!(pairs[0].base.name, "b1");
        assert_eq!(pairs[0].head.name, "h1");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].test.name, "b2");
        assert!(added.is_empty());
    }
}
