//! Pairing of test helpers across the change: the helpers of the changed files and of the
//! files that call them, each with the checks it holds on either side.

use super::{
    call_leaves, call_names_helper, equality_exits_gained, helper_call_gain, helper_lost,
    holds_no_test, moved_into_looping_helpers, name_similarity, run_unnamed, FileFacts, TestPair,
    RENAME_NAME_SIMILARITY_THRESHOLD,
};
use crate::ast::ParsedFileFacts;

#[derive(Debug, Clone)]
pub struct HelperPair<'a> {
    pub path: &'a str,
    pub base: &'a crate::ast::TestHelperFacts,
    pub head: Option<&'a crate::ast::TestHelperFacts>,
}

/// The helper pairs `assertion-reduction` judges for a change read without the rest of
/// the tree: [`pair_helpers_in_tree`] with no file outside the change.
#[cfg(test)]
pub fn pair_helpers<'a>(files: &'a [FileFacts], pairs: &[TestPair<'a>]) -> Vec<HelperPair<'a>> {
    pair_helpers_in_tree(files, pairs, &[])
}

/// The helper pairs `assertion-reduction` judges: [`match_helpers`], less the functions
/// of a file that holds no test which no test names ([`leave_unreferenced_functions`]),
/// with the helpers of unchanged test-support files the changed tests call
/// ([`outside_helpers`]), and less the helpers a dropping test of their own file already
/// shows.
pub fn pair_helpers_in_tree<'a>(
    files: &'a [FileFacts],
    pairs: &[TestPair<'a>],
    outside: &'a [OutsideFile],
) -> Vec<HelperPair<'a>> {
    let mut helpers = match_helpers(files);
    leave_unreferenced_functions(&mut helpers, files, outside);
    helpers.extend(outside_helpers(pairs, outside));
    leave_helpers_shown_by_tests(&mut helpers, pairs, files, None);
    helpers
}

/// A file the change does not touch, as the head tree has it.
pub struct OutsideFile {
    pub path: String,
    /// `None` when the file could not be parsed: it then stands for no helper, and
    /// counts as naming every function its text holds (`text`).
    pub facts: Option<ParsedFileFacts>,
    /// The file's text, kept only when `facts` is `None`.
    pub text: String,
}

/// Whether `text`, a file that could not be parsed, holds `name` as a whole word.
pub(super) fn text_names(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(word)
            && !text[at + name.len()..].chars().next().is_some_and(word)
    })
}

/// A function of a file under a test directory that holds no test is an assertion helper
/// only when a test names it: `tests/support/gen.rs` going from `.unwrap()` to
/// `.unwrap_or_default()` weakens no test unless a test checks through `gen`. The pair of
/// a function that lost checks and that nothing names is left out.
///
/// What names a function: a call, direct or on a receiver, in a test of a changed file
/// (either side) or of an unchanged test file (`outside`); a call in a helper of a file
/// that holds tests; and a call in a function of a file without tests that is itself
/// named. A call names the function whose name, or last `::` / `.` segment, it ends in,
/// whichever file defines it: imports are not resolved, so a same-named function
/// elsewhere keeps the pair. A function a runner calls unnamed ([`run_unnamed`]) and an
/// unchanged file that names it and cannot be parsed keep the pair too.
fn leave_unreferenced_functions<'a>(
    helpers: &mut Vec<HelperPair<'a>>,
    files: &'a [FileFacts],
    outside: &'a [OutsideFile],
) {
    let in_no_test_file = |hp: &HelperPair| {
        files
            .iter()
            .any(|ff| ff.file.path == hp.path && holds_no_test(ff))
    };
    if !helpers
        .iter()
        .any(|hp| helper_lost(hp) && in_no_test_file(hp))
    {
        return;
    }
    let changed = files
        .iter()
        .flat_map(|ff| [ff.base.as_ref(), ff.head.as_ref()])
        .flatten();
    let all: Vec<&ParsedFileFacts> = changed
        .chain(outside.iter().filter_map(|o| o.facts.as_ref()))
        .collect();
    // The calls of every test, and of every helper of a file that holds tests.
    let mut calls: Vec<&str> = Vec::new();
    for facts in &all {
        for test in &facts.tests {
            let reach = &test.helper_reach;
            let own = test.direct_calls.iter().chain(&reach.receiver_calls);
            calls.extend(own.flat_map(|c| call_leaves(c)));
        }
        if !facts.tests.is_empty() {
            let own = facts.helper_calls.iter().flatten();
            calls.extend(own.flat_map(|c| call_leaves(c)));
        }
    }
    // Then the calls of each function of a file without tests that is named so far.
    let mut named: Vec<&str> = Vec::new();
    while let Some(call) = calls.pop() {
        if named.contains(&call) {
            continue;
        }
        named.push(call);
        for facts in all.iter().filter(|facts| facts.tests.is_empty()) {
            for (at, helper) in facts.test_helpers.iter().enumerate() {
                if crate::ast::helper_leaf(&helper.name) == call {
                    let own = facts.helper_calls.get(at).into_iter().flatten();
                    calls.extend(own.flat_map(|c| call_leaves(c)));
                }
            }
        }
    }
    helpers.retain(|hp| {
        if !helper_lost(hp) || !in_no_test_file(hp) || run_unnamed(hp.path, &hp.base.name) {
            return true;
        }
        let leaf = crate::ast::helper_leaf(&hp.base.name);
        let head_leaf = hp.head.map(|h| crate::ast::helper_leaf(&h.name));
        named.contains(&leaf)
            || head_leaf.is_some_and(|l| named.contains(&l))
            || outside
                .iter()
                .any(|o| o.facts.is_none() && text_names(&o.text, leaf))
    });
}

/// The helpers of unchanged test-support files that the changed tests call: each is
/// paired with itself, as a helper new on the head side is, so a test that starts
/// calling it gets its checks ([`helper_call_gain`]) and one that already called it gets
/// nothing more. A file the head tree does not hold, or that cannot be parsed, gives no
/// pair: a call into it stands for nothing.
fn outside_helpers<'a>(pairs: &[TestPair<'a>], outside: &'a [OutsideFile]) -> Vec<HelperPair<'a>> {
    let mut out = Vec::new();
    for o in outside {
        let Some(facts) = o.facts.as_ref() else {
            continue;
        };
        if !crate::ast::functions::test_support_path(&o.path) {
            continue;
        }
        for helper in HelperSide::of(Some(facts)).tracked {
            let called = pairs.iter().any(|p| {
                [p.base, p.head].into_iter().any(|test| {
                    let reach = &test.helper_reach;
                    let mut calls = test.direct_calls.iter().chain(&reach.receiver_calls);
                    calls.any(|call| {
                        !reach.own_file_calls.contains(call)
                            && call_names_helper(call, &helper.name)
                    })
                })
            });
            if called {
                out.push(HelperPair {
                    path: &o.path,
                    base: helper,
                    head: Some(helper),
                });
            }
        }
    }
    out
}

/// One side of a file's helpers: each helper with its callees' checks counted in
/// (`tracked`), and its own body's checks and calls (`own`, `calls`).
struct HelperSide<'a> {
    tracked: &'a [crate::ast::TestHelperFacts],
    own: &'a [crate::ast::TestHelperFacts],
    calls: &'a [Vec<String>],
}

impl<'a> HelperSide<'a> {
    fn of(facts: Option<&'a crate::ast::ParsedFileFacts>) -> Self {
        let Some(facts) = facts else {
            return HelperSide {
                tracked: &[],
                own: &[],
                calls: &[],
            };
        };
        // Facts built without the resolving pass count each helper's own body.
        let tracked = if facts.tracked_helpers.len() == facts.test_helpers.len() {
            &facts.tracked_helpers[..]
        } else {
            &facts.test_helpers[..]
        };
        HelperSide {
            tracked,
            own: &facts.test_helpers,
            calls: &facts.helper_calls,
        }
    }

    fn calls_of(&self, at: usize) -> Vec<&'a str> {
        let mut calls: Vec<&'a str> = self
            .calls
            .get(at)
            .map(|c| c.iter().map(String::as_str).collect())
            .unwrap_or_default();
        calls.sort_unstable();
        calls
    }
}

fn helper_counts(h: &crate::ast::TestHelperFacts) -> (usize, usize, usize, usize) {
    (
        h.total_asserts,
        h.strong_asserts,
        h.tautologies,
        h.fatal_asserts,
    )
}

pub(super) fn helper_has_checks(h: &crate::ast::TestHelperFacts) -> bool {
    h.effective_asserts() > 0 || h.strong_asserts > 0 || h.fatal_asserts > 0
}

/// Pairs the helpers of one file across the change, steps (1) and (2) of
/// [`match_helpers`]: `(base, head)` positions of each pair, and the base helpers left
/// unpaired. `head_taken` marks the head helpers paired.
fn match_in_file(
    base: &HelperSide,
    head: &HelperSide,
    head_taken: &mut [bool],
) -> (Vec<(usize, usize)>, Vec<usize>) {
    let mut matched: Vec<(usize, usize)> = Vec::new();
    let mut base_left: Vec<usize> = (0..base.tracked.len()).collect();
    // (1) Same name: unchanged checks first, then in the order they are declared.
    for unchanged_only in [true, false] {
        base_left.retain(|&bi| {
            let b = &base.tracked[bi];
            let found = (0..head.tracked.len()).find(|&hi| {
                let h = &head.tracked[hi];
                !head_taken[hi]
                    && h.name == b.name
                    && (!unchanged_only || helper_counts(h) == helper_counts(b))
            });
            match found {
                Some(hi) => {
                    head_taken[hi] = true;
                    matched.push((bi, hi));
                    false
                }
                None => true,
            }
        });
    }
    // (2) Renamed in place: the most similar name first.
    let mut renames: Vec<(usize, usize, f64)> = Vec::new();
    for &bi in &base_left {
        let b = &base.tracked[bi];
        for hi in (0..head.tracked.len()).filter(|&hi| !head_taken[hi]) {
            let h = &head.tracked[hi];
            let sim = name_similarity(
                crate::ast::helper_leaf(&b.name),
                crate::ast::helper_leaf(&h.name),
            );
            let same_body = helper_has_checks(b)
                && helper_counts(b) == helper_counts(h)
                && b.end_line.saturating_sub(b.line) == h.end_line.saturating_sub(h.line)
                && base.calls_of(bi) == head.calls_of(hi);
            if sim >= RENAME_NAME_SIMILARITY_THRESHOLD || same_body {
                renames.push((bi, hi, if same_body { sim + 1.0 } else { sim }));
            }
        }
    }
    renames.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    for (bi, hi, _) in renames {
        if base_left.contains(&bi) && !head_taken[hi] {
            head_taken[hi] = true;
            base_left.retain(|&left| left != bi);
            matched.push((bi, hi));
        }
    }
    (matched, base_left)
}

/// Whether what the pair `(bi, hi)` lost is what a helper it calls lost: its own body
/// and its calls are as they were. That helper is paired and reported on its own.
fn inherited_loss(base: &HelperSide, head: &HelperSide, bi: usize, hi: usize) -> bool {
    helper_counts(&base.tracked[bi]) != helper_counts(&head.tracked[hi])
        && base.own.get(bi).map(helper_counts) == head.own.get(hi).map(helper_counts)
        && base.calls_of(bi) == head.calls_of(hi)
}

/// One file's helpers paired across a change, for a caller that reads the facts itself
/// ([`super::crate_helpers`]): each pair as `(base, head)` positions in the helpers of
/// each side, and the base helpers with no head.
pub(super) struct FileHelperPairs {
    pub matched: Vec<(usize, usize)>,
    pub left: Vec<usize>,
}

/// Pairs the helpers of one file as [`match_helpers`] pairs them within a file.
pub(super) fn pair_file_helpers(
    base: Option<&ParsedFileFacts>,
    head: Option<&ParsedFileFacts>,
) -> FileHelperPairs {
    let (base, head) = (HelperSide::of(base), HelperSide::of(head));
    let mut head_taken = vec![false; head.tracked.len()];
    let (mut matched, left) = match_in_file(&base, &head, &mut head_taken);
    matched.sort_unstable();
    FileHelperPairs { matched, left }
}

/// The helpers of a file, each with the checks of the helpers it calls counted in.
pub(super) fn tracked_helpers_of(facts: &ParsedFileFacts) -> &[crate::ast::TestHelperFacts] {
    HelperSide::of(Some(facts)).tracked
}

/// Pairs each helper of the changed test-support files ([`test_support_path`]) across the
/// change. A file that also holds tests is tracked like one that holds none: tests in
/// other files call its helpers too.
///
/// In order: (1) helpers of one file with the same name, the ones whose checks did not
/// change first, so two same-named methods or overloads pair with themselves; (2) a
/// helper renamed within its file, by a similar name or by the same checks, calls and
/// length; (3) a helper that left its file with one of the same name that is new in
/// another changed file; (4) what is left on the base side is deleted. A helper that is
/// new on the head side is paired with itself: nothing was lost, and a test that starts
/// calling it gets its checks ([`helper_call_gain`]).
///
/// [`test_support_path`]: crate::ast::functions::test_support_path
pub fn match_helpers<'a>(files: &'a [FileFacts]) -> Vec<HelperPair<'a>> {
    let mut pairs = Vec::new();
    let sides: Vec<(HelperSide<'a>, HelperSide<'a>)> = files
        .iter()
        .map(|ff| {
            (
                HelperSide::of(ff.base.as_ref()),
                HelperSide::of(ff.head.as_ref()),
            )
        })
        .collect();
    let tracked_base = |ff: &FileFacts| crate::ast::functions::test_support_path(&ff.file.old_path);
    let tracked_head = |ff: &FileFacts| crate::ast::functions::test_support_path(&ff.file.path);
    // Head helpers already paired, and base helpers of tracked files still unpaired.
    let mut head_taken: Vec<Vec<bool>> = sides
        .iter()
        .map(|(_, head)| vec![false; head.tracked.len()])
        .collect();
    let mut unpaired_base: Vec<(usize, usize)> = Vec::new();

    for (fi, ff) in files.iter().enumerate() {
        let (base, head) = (&sides[fi].0, &sides[fi].1);
        let base_tracked = tracked_base(ff);
        let head_tracked = tracked_head(ff);
        if !base_tracked && !head_tracked {
            continue;
        }
        if base_tracked && head_tracked {
            let (mut matched, base_left) = match_in_file(base, head, &mut head_taken[fi]);
            matched.sort_unstable();
            for (bi, hi) in matched {
                let (b, h) = (&base.tracked[bi], &head.tracked[hi]);
                // Its own body and its calls are as they were: what it lost, a helper it
                // calls lost, and that helper is paired and reported on its own.
                let inherited = inherited_loss(base, head, bi, hi);
                pairs.push(HelperPair {
                    path: &ff.file.path,
                    base: if inherited { h } else { b },
                    head: Some(h),
                });
            }
            unpaired_base.extend(base_left.into_iter().map(|bi| (fi, bi)));
        } else if base_tracked {
            unpaired_base.extend((0..base.tracked.len()).map(|bi| (fi, bi)));
        }
    }

    // (3) Moved to another changed file, where a helper of that name is new.
    for (fi, bi) in unpaired_base {
        let b = &sides[fi].0.tracked[bi];
        if !helper_has_checks(b) {
            continue;
        }
        let moved = (0..files.len())
            .filter(|&other| other != fi && tracked_head(&files[other]))
            .find_map(|other| {
                let head = &sides[other].1;
                (0..head.tracked.len())
                    .find(|&hi| !head_taken[other][hi] && head.tracked[hi].name == b.name)
                    .map(|hi| (other, hi))
            });
        match moved {
            Some((other, hi)) => {
                head_taken[other][hi] = true;
                pairs.push(HelperPair {
                    path: &files[other].file.path,
                    base: b,
                    head: Some(&sides[other].1.tracked[hi]),
                });
            }
            // (4) Deleted.
            None => {
                let path = if tracked_base(&files[fi]) {
                    &files[fi].file.old_path
                } else {
                    &files[fi].file.path
                };
                pairs.push(HelperPair {
                    path,
                    base: b,
                    head: None,
                });
            }
        }
    }

    // New on the head side of a tracked file.
    for (fi, ff) in files.iter().enumerate() {
        if !tracked_head(ff) {
            continue;
        }
        for (hi, h) in sides[fi].1.tracked.iter().enumerate() {
            if !head_taken[fi][hi] {
                pairs.push(HelperPair {
                    path: &ff.file.path,
                    base: h,
                    head: Some(h),
                });
            }
        }
    }
    pairs
}

/// A helper in a file that holds tests is counted in the tests of that file that call
/// it, so when it loses checks those tests lose them too and are reported for it. Such a
/// helper is not reported a second time: its pair is kept, with nothing lost, for every
/// test of the file that drops and reaches it, directly or through the file's helpers.
/// A helper no dropping test of its file reaches is reported on its own; tests in other
/// files may call it.
///
/// A test whose drop may be read as a move is not reported for it, so it shows nothing
/// and does not count here: one that calls more same-file helpers that check in a loop
/// than before ([`moved_into_looping_helpers`]), and one that a helper gives checks to
/// ([`helper_call_gain`]). The helper is then reported itself.
///
/// `credited` are the pairs whose helpers give checks to a test, when they are not
/// `helpers` themselves: the helpers of a crate's test-only modules are judged apart
/// from the pairs that credit a test (`report_weakened_crate_helpers`).
pub(super) fn leave_helpers_shown_by_tests<'a>(
    helpers: &mut Vec<HelperPair<'a>>,
    pairs: &[TestPair<'a>],
    files: &'a [FileFacts],
    credited: Option<&[HelperPair<'a>]>,
) {
    let lost = helper_lost;
    for ff in files {
        let path = ff.file.path.as_str();
        if !helpers.iter().any(|hp| hp.path == path && lost(hp)) {
            continue;
        }
        let mut calls: Vec<&str> = Vec::new();
        for p in pairs.iter().filter(|p| p.path == path) {
            let (b, h) = (p.base, p.head);
            let drops = h.effective_asserts() < b.effective_asserts()
                || h.strong_asserts < b.strong_asserts
                || h.fatal_asserts < b.fatal_asserts;
            // Nor does a test whose drop a helper of another file accounts for.
            let moved = helper_call_gain(b, h, path, credited.unwrap_or(helpers), &[]);
            // Nor one whose only shortfall is equality checks now written by hand.
            let by_hand = h.effective_asserts() >= b.effective_asserts()
                && h.fatal_asserts >= b.fatal_asserts
                && equality_exits_gained(b, h) > 0
                && h.strong_asserts + equality_exits_gained(b, h) >= b.strong_asserts;
            if drops
                && !moved_into_looping_helpers(b, h)
                && !by_hand
                && moved.total == 0
                && moved.strong == 0
            {
                let both = b.direct_calls.iter().chain(&h.direct_calls);
                calls.extend(both.flat_map(|c| c.split('|')));
            }
        }
        // Every helper of the file those calls reach, on either side of the change.
        let mut reached: Vec<&str> = Vec::new();
        while let Some(call) = calls.pop() {
            for facts in [ff.base.as_ref(), ff.head.as_ref()].into_iter().flatten() {
                for (at, helper) in facts.test_helpers.iter().enumerate() {
                    if call_names_helper(call, &helper.name)
                        && !reached.contains(&helper.name.as_str())
                    {
                        reached.push(&helper.name);
                        let own = facts.helper_calls.get(at).into_iter().flatten();
                        calls.extend(own.flat_map(|c| c.split('|')));
                    }
                }
            }
        }
        helpers.retain_mut(|hp| {
            if hp.path != path || !lost(hp) || !reached.contains(&hp.base.name.as_str()) {
                return true;
            }
            match hp.head {
                Some(h) => {
                    hp.base = h;
                    true
                }
                None => false,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::agent_diff::test_support::*;

    type PairedHelper = (String, usize, Option<(String, usize)>);

    /// Each pair as `(base name, base checks, head name and checks)`.
    fn paired(files: &[FileFacts]) -> Vec<PairedHelper> {
        match_helpers(files)
            .iter()
            .map(|hp| {
                (
                    hp.base.name.clone(),
                    hp.base.effective_asserts(),
                    hp.head.map(|h| (h.name.clone(), h.effective_asserts())),
                )
            })
            .collect()
    }
    /// A test file the change leaves as it is, whose test calls `check`: a function of a
    /// file that holds no test is judged only when a test names it.
    const PY_CALLER: &str = "def test_api():\n    check(load())\n";
    const PY_CALLER_FILE: (&str, &str, &str) = ("tests/test_api.py", PY_CALLER, PY_CALLER);

    /// #562: two methods named `check` in one file pair each with itself, so a comment
    /// added to the file loses nothing; one of them losing a check is one drop.
    #[test]
    fn two_helpers_of_one_name_pair_with_themselves() {
        let two = "impl A {\n    pub fn check(&self, v: u32) {\n        assert_eq!(v, 1);\n    }\n}\nimpl B {\n    pub fn check(&self, v: u32) {\n        assert_eq!(v, 2);\n        assert_eq!(v % 2, 0);\n    }\n}\n";
        let commented = format!("// shared\n{two}");
        let facts = change_facts(&[("tests/common/mod.rs", two, &commented)]);
        assert_eq!(
            paired(&facts),
            vec![
                ("check".to_string(), 1, Some(("check".to_string(), 1))),
                ("check".to_string(), 2, Some(("check".to_string(), 2))),
            ]
        );
        let weaker = two.replace("        assert_eq!(v % 2, 0);\n", "");
        let facts = change_facts(&[("tests/common/mod.rs", two, &weaker)]);
        assert_eq!(
            paired(&facts),
            vec![
                ("check".to_string(), 1, Some(("check".to_string(), 1))),
                ("check".to_string(), 2, Some(("check".to_string(), 1))),
            ]
        );
        // The first of the two is deleted: the second is unchanged and pairs with itself.
        let second_only = two.split_once("impl B").map(|(_, b)| format!("impl B{b}"));
        let facts = change_facts(&[("tests/common/mod.rs", two, &second_only.unwrap())]);
        assert_eq!(
            paired(&facts),
            vec![
                ("check".to_string(), 2, Some(("check".to_string(), 2))),
                ("check".to_string(), 1, None),
            ]
        );
    }

    /// #562: a helper renamed in place is paired with its new name, by the name's
    /// similarity or, for an unrelated name, by an unchanged body; what it lost under the
    /// new name is a drop. An unrelated new helper with other checks is no rename.
    #[test]
    fn a_renamed_helper_is_paired_with_its_new_name() {
        let renamed = PY_CHECK_2.replace("def check(", "def check_response(");
        let facts = change_facts(&[("tests/helpers.py", PY_CHECK_2, &renamed)]);
        assert_eq!(
            paired(&facts),
            vec![(
                "check".to_string(),
                2,
                Some(("check_response".to_string(), 2))
            )]
        );
        let weaker = PY_CHECK_1.replace("def check(", "def check_response(");
        let facts = change_facts(&[("tests/helpers.py", PY_CHECK_2, &weaker)]);
        assert_eq!(
            paired(&facts),
            vec![(
                "check".to_string(),
                2,
                Some(("check_response".to_string(), 1))
            )]
        );
        let unrelated = PY_CHECK_2.replace("def check(", "def verify(");
        let facts = change_facts(&[("tests/helpers.py", PY_CHECK_2, &unrelated)]);
        assert_eq!(
            paired(&facts),
            vec![("check".to_string(), 2, Some(("verify".to_string(), 2)))]
        );
        let other =
            "def verify(r):\n    assert r.x == 9\n    assert r.y == 8\n    assert r.z == 7\n";
        let facts = change_facts(&[("tests/helpers.py", PY_CHECK_2, other)]);
        assert_eq!(
            paired(&facts),
            vec![
                ("check".to_string(), 2, None),
                ("verify".to_string(), 3, Some(("verify".to_string(), 3))),
            ]
        );
    }

    /// #562: a helper that leaves its file is paired with one of its name in another
    /// changed file only where that helper is new; an unchanged helper of the same name
    /// there is its own pair, and the first is deleted in its own file.
    #[test]
    fn a_helper_moves_only_to_a_file_where_its_name_is_new() {
        let other =
            "def check(r):\n    assert r.x == 9\n    assert r.y == 8\n    assert r.z == 7\n";
        let facts = change_facts(&[
            (
                "tests/helpers.py",
                PY_CHECK_2,
                "def unrelated():\n    pass\n",
            ),
            (
                "tests/zother/helpers.py",
                other,
                &format!("# shared\n{other}"),
            ),
        ]);
        let pairs = match_helpers(&facts);
        let deleted: Vec<&str> = pairs
            .iter()
            .filter(|hp| hp.head.is_none())
            .map(|hp| hp.path)
            .collect();
        assert_eq!(deleted, vec!["tests/helpers.py"]);

        let facts = change_facts(&[
            (
                "tests/helpers.py",
                PY_CHECK_2,
                "def unrelated():\n    pass\n",
            ),
            (
                "tests/zother/helpers.py",
                "def other(r):\n    pass\n",
                PY_CHECK_2,
            ),
        ]);
        let pairs = match_helpers(&facts);
        assert!(pairs.iter().all(|hp| hp.head.is_some()));
        let moved = pairs.iter().find(|hp| hp.base.name == "check").unwrap();
        assert_eq!(moved.path, "tests/zother/helpers.py");
    }

    /// #562: a file that holds a test is tracked like one that holds none; a file under
    /// `examples/` or `benches/` is not test support and is not tracked.
    #[test]
    fn helpers_are_tracked_beside_tests_and_not_under_examples() {
        let with_test =
            |helper: &str| format!("{helper}\ndef test_unrelated():\n    assert 1 + 1 == 2\n");
        let files = [(
            "tests/helpers.py",
            with_test(PY_CHECK_2),
            with_test(PY_CHECK_1),
        )];
        let files: Vec<(&str, &str, &str)> = files
            .iter()
            .map(|(p, b, h)| (*p, b.as_str(), h.as_str()))
            .collect();
        assert_eq!(
            reduction(&files, ""),
            vec![(HELPER_WEAKENED.to_string(), "tests/helpers.py".to_string())]
        );
        for path in ["examples/helpers.py", "benches/helpers.py"] {
            assert_eq!(reduction(&[(path, PY_CHECK_2, PY_CHECK_1)], ""), Vec::new());
        }
        assert_eq!(
            reduction(
                &[("tests/helpers.py", PY_CHECK_2, PY_CHECK_1), PY_CALLER_FILE],
                ""
            ),
            vec![(HELPER_WEAKENED.to_string(), "tests/helpers.py".to_string())]
        );
    }

    /// #562: a helper counts the helpers it calls. Dropping the call is a drop; moving
    /// checks into a helper it calls is none; and when a called helper loses a check, that
    /// helper alone is reported, not each helper that calls it.
    #[test]
    fn a_helper_counts_the_helpers_it_calls() {
        let whole =
            "def check(r):\n    assert r.a == 1\n    assert r.b == 2\n    assert r.c == 3\n";
        let split = "def check(r):\n    assert r.a == 1\n    check_body(r)\n\ndef check_body(r):\n    assert r.b == 2\n    assert r.c == 3\n";
        let uncalled = split.replace("    check_body(r)\n", "");
        let weaker_body = split.replace("    assert r.c == 3\n", "");
        let path = "tests/helpers.py";
        assert_eq!(reduction(&[(path, whole, split)], ""), Vec::new());
        assert_eq!(
            reduction(&[(path, split, &uncalled), PY_CALLER_FILE], ""),
            vec![(HELPER_WEAKENED.to_string(), path.to_string())]
        );
        let facts = change_facts(&[(path, split, &weaker_body)]);
        let lost: Vec<(String, usize, usize)> = match_helpers(&facts)
            .iter()
            .map(|hp| {
                (
                    hp.base.name.clone(),
                    hp.base.effective_asserts(),
                    hp.head.unwrap().effective_asserts(),
                )
            })
            .collect();
        assert_eq!(
            lost,
            vec![
                ("check".to_string(), 2, 2),
                ("check_body".to_string(), 2, 1)
            ]
        );
    }

    #[test]
    fn test_match_helpers_same_file_and_cross_file() {
        let h1 = crate::ast::TestHelperFacts {
            name: "check_same".to_string(),
            line: 5,
            end_line: 10,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let h2_base = crate::ast::TestHelperFacts {
            name: "check_cross".to_string(),
            line: 15,
            end_line: 20,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let h2_head = crate::ast::TestHelperFacts {
            name: "check_cross".to_string(),
            line: 25,
            end_line: 30,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };

        let file1 = FileFacts {
            file: crate::gitctx::ChangedFile {
                path: "tests/helpers.py".to_string(),
                old_path: "tests/helpers.py".to_string(),
                kind: crate::gitctx::ChangeKind::Modified,
                added_lines: Default::default(),
            },
            base: Some(crate::ast::ParsedFileFacts {
                test_helpers: vec![h1.clone(), h2_base.clone()],
                ..Default::default()
            }),
            head: Some(crate::ast::ParsedFileFacts {
                test_helpers: vec![h1.clone()],
                ..Default::default()
            }),
            newly_added_nul: false,
        };
        let file2 = FileFacts {
            file: crate::gitctx::ChangedFile {
                path: "tests/utils.py".to_string(),
                old_path: "tests/utils.py".to_string(),
                kind: crate::gitctx::ChangeKind::Modified,
                added_lines: Default::default(),
            },
            base: Some(crate::ast::ParsedFileFacts::default()),
            head: Some(crate::ast::ParsedFileFacts {
                test_helpers: vec![h2_head.clone()],
                ..Default::default()
            }),
            newly_added_nul: false,
        };

        let files = [file1, file2];
        let matched = match_helpers(&files);
        assert_eq!(matched.len(), 2);
        let same = matched
            .iter()
            .find(|m| m.base.name == "check_same")
            .unwrap();
        assert_eq!(same.path, "tests/helpers.py");
        assert_eq!(same.head.unwrap().line, 5);

        let cross = matched
            .iter()
            .find(|m| m.base.name == "check_cross")
            .unwrap();
        assert_eq!(cross.path, "tests/utils.py");
        assert_eq!(cross.head.unwrap().line, 25);
    }

    /// #688: match_helpers must not pair a helper leaving a test-support file into a
    /// non-test-support file. Such a move must be treated as deleted (head: None).
    #[test]
    fn test_match_helpers_does_not_pair_into_non_test_support_file() {
        let h_base = crate::ast::TestHelperFacts {
            name: "check".to_string(),
            line: 5,
            end_line: 10,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let h_head = crate::ast::TestHelperFacts {
            name: "check".to_string(),
            line: 15,
            end_line: 20,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            fatal_asserts: 0,
            helper_checks: 0,
            equality_exits: 0,
        };
        let file1 = FileFacts {
            file: crate::gitctx::ChangedFile {
                path: "tests/helpers.py".to_string(),
                old_path: "tests/helpers.py".to_string(),
                kind: crate::gitctx::ChangeKind::Modified,
                added_lines: Default::default(),
            },
            base: Some(crate::ast::ParsedFileFacts {
                test_helpers: vec![h_base.clone()],
                ..Default::default()
            }),
            head: Some(crate::ast::ParsedFileFacts::default()),
            newly_added_nul: false,
        };
        let file2 = FileFacts {
            file: crate::gitctx::ChangedFile {
                path: "src/app.py".to_string(),
                old_path: "src/app.py".to_string(),
                kind: crate::gitctx::ChangeKind::Modified,
                added_lines: Default::default(),
            },
            base: Some(crate::ast::ParsedFileFacts::default()),
            head: Some(crate::ast::ParsedFileFacts {
                test_helpers: vec![h_head.clone()],
                ..Default::default()
            }),
            newly_added_nul: false,
        };

        let files = [file1, file2];
        let matched = match_helpers(&files);
        assert_eq!(matched.len(), 1);
        let pair = &matched[0];
        assert_eq!(pair.path, "tests/helpers.py");
        assert!(
            pair.head.is_none(),
            "expected helper moved to non-test-support file to be treated as deleted (head: None), got Some in {}",
            pair.path
        );
    }
}
