//! Pairing of the steps of a job across the change, so a renamed step is compared with
//! its base form.

use super::{executable_run, steps_match_identity, verifying_body_markers};
use std::collections::HashMap;

/// Minimum run-body similarity (Dice coefficient over whitespace tokens) for a
/// base step with no id/name match to be paired with an unmatched head step as
/// a rename.
pub(crate) const STEP_RENAME_SIMILARITY: f64 = 0.6;

/// How a base step was found in the head workflow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StepMatch {
    /// Matched by id, name, action, or first run line.
    Same(usize),
    /// No identity match; paired by run-body similarity.
    Renamed { head: usize, similarity: f64 },
}

/// Whitespace tokens of what a step executes: its `run:` script (comment
/// lines excluded), or its action (ref stripped) and `with:` inputs. Empty
/// when the step has neither.
fn step_body_tokens(step: &serde_yaml::Value) -> Vec<String> {
    if let Some(run) = executable_run(step) {
        return run.split_whitespace().map(str::to_string).collect();
    }
    let mut tokens = Vec::new();
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        tokens.push(format!("uses:{}", uses.split('@').next().unwrap_or(uses)));
        if let Some(with) = step.get("with").and_then(|w| w.as_mapping()) {
            for (k, v) in with {
                let v = serde_yaml::to_string(v).unwrap_or_default();
                tokens.push(format!("{}={}", k.as_str().unwrap_or(""), v.trim()));
            }
        }
    }
    tokens
}

/// Dice coefficient over the multisets of body tokens of two steps:
/// `2 * |A ∩ B| / (|A| + |B|)`, in `[0, 1]`. A `run:` step and a `uses:`
/// step never share tokens. Two empty bodies score 0: no evidence either way.
pub(crate) fn step_body_similarity(a: &serde_yaml::Value, b: &serde_yaml::Value) -> f64 {
    let ta = step_body_tokens(a);
    let tb = step_body_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in &ta {
        *counts.entry(t.as_str()).or_default() += 1;
    }
    let mut common = 0usize;
    for t in &tb {
        if let Some(c) = counts.get_mut(t.as_str()) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    (2 * common) as f64 / (ta.len() + tb.len()) as f64
}

/// Pairs each base step with the head step it became.
///
/// Pass 1 matches by identity (id, name, action, first run line), as before.
/// Pass 2 takes every base step still unmatched and pairs it with the head
/// step, not matched in pass 1 and not already taken, whose body is most
/// similar, provided the similarity reaches [`STEP_RENAME_SIMILARITY`] and
/// the head step still carries every verification marker (`test`, `clippy`,
/// `lint`, ...) the base step's body carried; ties go to the head step
/// closest in position. A step whose name and body both changed past the
/// threshold, or whose body stopped verifying, stays unpaired, i.e. deleted.
pub(crate) fn pair_steps(
    base: &[serde_yaml::Value],
    head: &[serde_yaml::Value],
) -> Vec<Option<StepMatch>> {
    let mut pairs: Vec<Option<StepMatch>> = base
        .iter()
        .map(|b| {
            head.iter()
                .position(|h| steps_match_identity(b, h))
                .map(StepMatch::Same)
        })
        .collect();
    let mut taken: Vec<bool> = head
        .iter()
        .map(|h| base.iter().any(|b| steps_match_identity(b, h)))
        .collect();

    // Best candidates first, so a strong rename is not pre-empted by a weak one.
    let mut candidates: Vec<(f64, usize, usize, usize)> = Vec::new();
    for (bi, b) in base.iter().enumerate() {
        if pairs[bi].is_some() {
            continue;
        }
        for (hi, h) in head.iter().enumerate() {
            if taken[hi] {
                continue;
            }
            let sim = step_body_similarity(b, h);
            // A rename keeps what the step verifies: `cargo test` renamed and
            // turned into `cargo build` is a deletion however similar the rest.
            if sim >= STEP_RENAME_SIMILARITY
                && verifying_body_markers(b).is_subset(&verifying_body_markers(h))
            {
                candidates.push((sim, bi.abs_diff(hi), bi, hi));
            }
        }
    }
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
    for (sim, _, bi, hi) in candidates {
        if pairs[bi].is_none() && !taken[hi] {
            pairs[bi] = Some(StepMatch::Renamed {
                head: hi,
                similarity: sim,
            });
            taken[hi] = true;
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::ci_integrity::test_support::*;

    #[test]
    fn rename_threshold_is_pinned() {
        assert_eq!(STEP_RENAME_SIMILARITY, 0.6);
        let a = &steps("- run: cargo test --locked")[0];
        let b = &steps("- run: cargo test --locked --workspace")[0];
        let c = &steps("- run: cargo build")[0];
        // 2*3/(3+4) = 0.857 clears the threshold; 2*1/(3+2) = 0.4 does not.
        assert!((step_body_similarity(a, b) - 6.0 / 7.0).abs() < 1e-9);
        assert!((step_body_similarity(a, c) - 0.4).abs() < 1e-9);
        assert_eq!(step_body_similarity(a, a), 1.0);
    }

    #[test]
    fn renamed_step_with_unchanged_body_pairs_as_rename() {
        let base = steps(
            "- uses: actions/checkout@v4\n- name: Check a.sh b.sh\n  run: |\n    ./a.sh\n    ./b.sh\n",
        );
        let head = steps(
            "- uses: actions/checkout@v4\n- name: Check a.sh b.sh c.sh\n  run: |\n    ./a.sh\n    ./b.sh\n    ./c.sh\n",
        );
        let pairs = pair_steps(&base, &head);
        assert_eq!(pairs[0], Some(StepMatch::Same(0)));
        match pairs[1] {
            Some(StepMatch::Renamed {
                head: 1,
                similarity,
            }) => {
                assert!(similarity >= STEP_RENAME_SIMILARITY, "{similarity}")
            }
            other => panic!("expected rename, got {other:?}"),
        }
    }

    #[test]
    fn renamed_and_rewritten_step_is_not_paired() {
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let head = steps("- name: Smoke\n  run: echo ok\n");
        assert_eq!(pair_steps(&base, &head), vec![None]);
    }

    #[test]
    fn removed_step_is_not_paired_with_an_unrelated_survivor() {
        let base = steps(
            "- name: Build\n  run: cargo build --locked\n- name: Run tests\n  run: cargo test --locked\n",
        );
        let head = steps("- name: Build\n  run: cargo build --locked\n");
        assert_eq!(
            pair_steps(&base, &head),
            vec![Some(StepMatch::Same(0)), None]
        );

        // The survivor is similar (2*2/6 = 0.67) and verifies the same thing,
        // but it already matched its own base step: it cannot also be the
        // rename of the removed one.
        let base = steps(
            "- name: Unit tests\n  run: cargo test --lib\n- name: All tests\n  run: cargo test --workspace\n",
        );
        let head = steps("- name: Unit tests\n  run: cargo test --lib\n");
        assert!(step_body_similarity(&base[1], &head[0]) >= STEP_RENAME_SIMILARITY);
        assert_eq!(
            pair_steps(&base, &head),
            vec![Some(StepMatch::Same(0)), None]
        );
    }

    #[test]
    fn rename_pairing_prefers_the_closest_position_on_a_tie() {
        let base = steps("- name: A\n  run: make check\n- name: B\n  run: make lint\n");
        let head = steps("- name: X\n  run: make check\n- name: Y\n  run: make check\n");
        let pairs = pair_steps(&base, &head);
        assert!(
            matches!(pairs[0], Some(StepMatch::Renamed { head: 0, .. })),
            "{pairs:?}"
        );
        // `make lint` vs `make check` is 0.5: below the threshold.
        assert_eq!(pairs[1], None);
    }

    #[test]
    fn rename_that_drops_the_verification_command_is_not_paired() {
        // Similar enough by tokens (2*2/6 = 0.67), but `test` is gone.
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let head = steps("- name: Compile\n  run: cargo build --locked\n");
        assert!(step_body_similarity(&base[0], &head[0]) >= STEP_RENAME_SIMILARITY);
        assert_eq!(pair_steps(&base, &head), vec![None]);
    }

    #[test]
    fn one_head_step_is_the_rename_of_at_most_one_base_step() {
        // Two verification steps collapse into one renamed step: one of them
        // was deleted, and pairing must not hide that.
        let base = steps("- name: Test A\n  run: make test\n- name: Test B\n  run: make test\n");
        let head = steps("- name: Tests\n  run: make test\n");
        let pairs = pair_steps(&base, &head);
        assert!(
            matches!(pairs[0], Some(StepMatch::Renamed { head: 0, .. })),
            "{pairs:?}"
        );
        assert_eq!(pairs[1], None);
    }

    #[test]
    fn shell_comment_lines_do_not_count_toward_similarity() {
        // Documenting a step's body with comments leaves it the same step.
        let base = steps("- name: Self-tests a b\n  run: |\n    python3 a.py --self-test\n    python3 b.py --self-test\n");
        let head = steps("- name: Self-tests a b c\n  run: |\n    python3 a.py --self-test\n    # c.py needs a PMU to collect anything, but its parser and\n    # its rendering rules are all checkable without one.\n    python3 c.py --self-test\n    python3 b.py --self-test\n");
        assert!((step_body_similarity(&base[0], &head[0]) - 0.8).abs() < 1e-9);

        // Commenting the old command out does not keep the step alive.
        let base = steps("- name: Run tests\n  run: cargo test --locked\n");
        let head = steps("- name: Smoke\n  run: |\n    # cargo test --locked\n    echo ok\n");
        assert_eq!(step_body_similarity(&base[0], &head[0]), 0.0);
        assert_eq!(pair_steps(&base, &head), vec![None]);
    }
}
