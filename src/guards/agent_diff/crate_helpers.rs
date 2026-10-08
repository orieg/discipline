//! Test helpers in another file of a Rust test's own crate: the checks of each helper a
//! test calls there, counted into the test's [`HelperReach`] on both sides of a change.
//!
//! A helper that leaves a test's file for a module beside it (`#[cfg(test)] mod
//! test_support;`, reached with `use super::test_support::*`) is no longer counted into
//! the test by the pack, which reads one file. The call is resolved here through the
//! crate's `mod` declarations and the `use` items or the path it is written with
//! ([`crate::ast::rust_modules`]), on each side from that side's tree, and the helper
//! stands for the checks it holds there. What the head side holds beyond the base side
//! is what the change moved into such helpers (`helper_call_gain`).
//!
//! [`HelperReach`]: crate::ast::HelperReach

use super::{match_tests, FileFacts};
use crate::ast::{AssertVocabulary, LanguageRegistry, ParsedFileFacts, TestFn, TestHelperFacts};
use anyhow::Result;

/// The files of one side of a change.
pub trait CrateTree {
    /// Every file of the tree.
    fn paths(&self) -> Result<Vec<String>>;
    /// The text of `path`; `None` when the tree does not hold it as a file.
    fn read(&self, path: &str) -> Result<Option<String>>;
}

/// A tree held in memory, as `(path, text)`.
impl CrateTree for Vec<(String, String)> {
    fn paths(&self) -> Result<Vec<String>> {
        Ok(self.iter().map(|(path, _)| path.clone()).collect())
    }

    fn read(&self, path: &str) -> Result<Option<String>> {
        Ok(self
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| text.clone()))
    }
}

/// Counts, into each Rust test whose assertions dropped across the change and into the
/// test it is paired with, the checks of the helpers it calls in other files of its
/// crate (`HelperReach::crate_helpers`). Returns notes on head-side calls that resolved
/// to a function whose checks do not count.
///
/// The calls of one name are read on each side ([`Reading`]) and count only when both
/// sides can be read: on each, the test makes no call of that name, or the name is a
/// helper of its own file (the pack counted it), or every call of that name resolves to
/// a helper that counts. A name that one side cannot read is left on both sides to the
/// pairing of helpers by name, so a helper the base test already called, in a way that
/// is not followed, is not taken for one the head test newly calls.
///
/// A call resolves to a helper that counts when all of these hold:
///
/// - it resolves, by the syntax trees of that side, to one function of another file
///   ([`crate::ast::rust_modules`]);
/// - the function is test code: built under `#[cfg(test)]`, or in a test path. A
///   function built outside tests too is the code under test, whatever it asserts;
/// - its file is not a test-support path: the helpers of those files are paired by
///   `match_helpers` and `outside_helpers`, and are left to them;
/// - the test's calls of that name are as many here as the pack recorded, and the pack
///   holds the function as a helper of its file.
///
/// Each call adds the helper's checks, those of the helpers it calls in its own file
/// counted in, less one for a helper configured in `assert_helper_fns`, which is already
/// one assertion of the test. The name is then one the test's own count accounts for
/// (`HelperReach::own_file_calls`): no helper of that name in a changed test-support
/// file is credited for it as well.
#[cfg(feature = "lang-rust")]
pub fn credit_crate_helpers(
    files: &mut [FileFacts],
    registry: &LanguageRegistry,
    trees: [&dyn CrateTree; 2],
    vocabs: [&AssertVocabulary; 2],
) -> Result<Vec<String>> {
    // `(file, test)` of the base and of the head test of each Rust pair that dropped.
    let mut wanted: Vec<[(usize, usize); 2]> = Vec::new();
    {
        let (pairs, _, _) = match_tests(files);
        for p in &pairs {
            let (b, h) = (p.base, p.head);
            if h.effective_asserts() >= b.effective_asserts()
                && h.strong_asserts >= b.strong_asserts
            {
                continue;
            }
            let locate = |test: &TestFn, head: bool| {
                files.iter().enumerate().find_map(|(fi, ff)| {
                    let facts = if head { &ff.head } else { &ff.base };
                    let tests = &facts.as_ref()?.tests;
                    let ti = tests.iter().position(|t| std::ptr::eq(t, test))?;
                    let pack = registry.find_pack(side_path(ff, head))?;
                    (pack.id() == "rust").then_some((fi, ti))
                })
            };
            if let (Some(base), Some(head)) = (locate(b, false), locate(h, true)) {
                wanted.push([base, head]);
            }
        }
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut notes = Vec::new();
    // What each test gains, read before any test is changed.
    let mut gains: Vec<(usize, bool, usize, NamedCredits)> = Vec::new();
    {
        let side = |head: bool| -> Result<Side> {
            let tree = trees[usize::from(head)];
            Ok(Side {
                modules: crate::ast::rust_modules::CrateModules::new(tree.paths()?, |p: &str| {
                    tree.read(p)
                }),
                tree,
                registry,
                vocab: vocabs[usize::from(head)],
                head,
                helper_files: Vec::new(),
            })
        };
        let mut sides = [side(false)?, side(true)?];
        for pair in &wanted {
            let mut tests = Vec::new();
            for (head, &(fi, ti)) in [false, true].into_iter().zip(pair) {
                let ff = &files[fi];
                let facts = if head { &ff.head } else { &ff.base };
                let Some(test) = facts.as_ref().and_then(|f| f.tests.get(ti)) else {
                    continue;
                };
                let path = side_path(ff, head);
                let sites = sides[usize::from(head)]
                    .modules
                    .test_calls(path, test.line)?;
                tests.push((path, test, sites));
            }
            let [base, head] = &tests[..] else {
                continue;
            };
            let mut leaves: Vec<&str> = Vec::new();
            for site in base.2.iter().chain(&head.2) {
                if site.target.is_some() && !leaves.contains(&site.leaf.as_str()) {
                    leaves.push(&site.leaf);
                }
            }
            let mut per_side: [NamedCredits; 2] = [Vec::new(), Vec::new()];
            for leaf in leaves {
                let b = sides[0].read(base.0, base.1, leaf, &base.2, &mut notes)?;
                let h = sides[1].read(head.0, head.1, leaf, &head.2, &mut notes)?;
                if matches!(b, Reading::Other) || matches!(h, Reading::Other) {
                    continue;
                }
                for (at, reading) in [b, h].into_iter().enumerate() {
                    if let Reading::Helpers(credits) = reading {
                        per_side[at].push((leaf.to_string(), credits));
                    }
                }
            }
            for ((head, &(fi, ti)), credits) in [false, true].into_iter().zip(pair).zip(per_side) {
                gains.push((fi, head, ti, credits));
            }
        }
    }
    for (fi, head, ti, credits) in gains {
        let facts = if head {
            &mut files[fi].head
        } else {
            &mut files[fi].base
        };
        let Some(test) = facts.as_mut().and_then(|f| f.tests.get_mut(ti)) else {
            continue;
        };
        let reach = &mut test.helper_reach;
        for (leaf, credits) in credits {
            reach.own_file_calls.push(leaf);
            for credit in credits {
                let held = &mut reach.crate_helpers;
                held.total += credit.total;
                held.strong += credit.strong;
                held.fatal += credit.fatal;
                held.equality_exits += credit.equality_exits;
                if !held.names.contains(&credit.name) {
                    held.names.push(credit.name);
                }
            }
        }
    }
    notes.sort();
    notes.dedup();
    Ok(notes)
}

/// Without the Rust pack no call is resolved.
#[cfg(not(feature = "lang-rust"))]
pub fn credit_crate_helpers(
    _files: &mut [FileFacts],
    _registry: &LanguageRegistry,
    _trees: [&dyn CrateTree; 2],
    _vocabs: [&AssertVocabulary; 2],
) -> Result<Vec<String>> {
    Ok(Vec::new())
}

/// The path of `ff` on one side of the change.
fn side_path(ff: &FileFacts, head: bool) -> &str {
    if head {
        &ff.file.path
    } else {
        &ff.file.old_path
    }
}

/// What one call to a crate helper adds to the test that makes it.
#[cfg(feature = "lang-rust")]
struct Credit {
    /// The helper and its file, as a note names them.
    name: String,
    total: usize,
    strong: usize,
    fatal: usize,
    equality_exits: usize,
}

/// The credits of a test's calls, by the name called.
#[cfg(feature = "lang-rust")]
type NamedCredits = Vec<(String, Vec<Credit>)>;

/// How one side of a pair reads the calls of one name.
#[cfg(feature = "lang-rust")]
enum Reading {
    /// The test makes no call of that name.
    Absent,
    /// A helper of the test's own file has the name: the pack counted it.
    Own,
    /// Every call resolves to a crate helper that counts: one credit per call.
    Helpers(Vec<Credit>),
    /// Anything else: a call that does not resolve, or resolves to a function that does
    /// not count.
    Other,
}

/// One side of the change: its module tree, and the helper files read from it so far.
#[cfg(feature = "lang-rust")]
struct Side<'a> {
    modules: crate::ast::rust_modules::CrateModules<'a>,
    tree: &'a dyn CrateTree,
    registry: &'a LanguageRegistry,
    vocab: &'a AssertVocabulary,
    head: bool,
    /// `(path, read as test code, facts)`; no facts for a file that could not be read.
    helper_files: Vec<(String, bool, Option<ParsedFileFacts>)>,
}

#[cfg(feature = "lang-rust")]
impl Side<'_> {
    /// Reads the calls named `leaf` of `test`, a test of the file `path` whose call
    /// sites are `sites`.
    fn read(
        &mut self,
        path: &str,
        test: &TestFn,
        leaf: &str,
        sites: &[crate::ast::rust_modules::CallSite],
        notes: &mut Vec<String>,
    ) -> Result<Reading> {
        if test.helper_reach.own_file_calls.iter().any(|c| c == leaf) {
            return Ok(Reading::Own);
        }
        let here: Vec<_> = sites.iter().filter(|s| s.leaf == leaf).collect();
        let recorded = test.direct_calls.iter().filter(|c| *c == leaf).count();
        if here.is_empty() && recorded == 0 {
            return Ok(Reading::Absent);
        }
        if here.len() != recorded {
            return Ok(Reading::Other);
        }
        let configured = self
            .vocab
            .helper_fns
            .iter()
            .any(|name| crate::ast::helper_call_matches(leaf, name));
        let mut credits = Vec::new();
        for site in here {
            let Some(target) = &site.target else {
                return Ok(Reading::Other);
            };
            if target.file == path || crate::ast::functions::test_support_path(&target.file) {
                return Ok(Reading::Other);
            }
            let test_code = target.test_only
                || crate::ast::functions::test_path(&target.file)
                || crate::ast::functions::declared_test_path(&target.file, &self.vocab.test_paths);
            if !test_code {
                // Said on the head side, and only of a function that checks.
                let checks = self
                    .helper(&target.file, target.line, false)?
                    .is_some_and(|h| h.effective_asserts() > 0);
                if self.head && checks {
                    notes.push(format!(
                        "`{}` in `{}` calls `{}` of `{}`, which is built outside tests too (no `#[cfg(test)]` on it or on a module around it): its checks are not counted as the test's",
                        test.name, path, target.name, target.file
                    ));
                }
                return Ok(Reading::Other);
            }
            let Some(helper) = self.helper(&target.file, target.line, target.test_only)? else {
                return Ok(Reading::Other);
            };
            credits.push(Credit {
                name: format!("{}` in `{}", target.name, target.file),
                total: helper
                    .effective_asserts()
                    .saturating_sub(usize::from(configured)),
                strong: helper.strong_asserts,
                fatal: helper.fatal_asserts,
                equality_exits: helper.equality_exits,
            });
        }
        Ok(Reading::Helpers(credits))
    }

    /// The helper whose function starts on `line` of `file`, the checks of the helpers
    /// it calls in that file counted in. `test_only` reads the file as test code, where
    /// `unwrap`, `expect` and `?` are checks. `None` when the file gives no facts or
    /// holds no helper there (a test, a function without a body).
    fn helper(
        &mut self,
        file: &str,
        line: usize,
        test_only: bool,
    ) -> Result<Option<TestHelperFacts>> {
        let known = self
            .helper_files
            .iter()
            .position(|(path, as_test, _)| path == file && *as_test == test_only);
        let at = match known {
            Some(at) => at,
            None => {
                let facts = match (self.registry.find_pack(file), self.tree.read(file)?) {
                    (Some(pack), Some(text)) => {
                        let mut vocab = self.vocab.clone();
                        if test_only {
                            vocab.test_paths.push(globset::escape(file));
                        }
                        super::extract_facts(pack, file, &text, &vocab).ok()
                    }
                    _ => None,
                };
                self.helper_files.push((file.to_string(), test_only, facts));
                self.helper_files.len() - 1
            }
        };
        let Some(facts) = &self.helper_files[at].2 else {
            return Ok(None);
        };
        let Some(index) = facts.test_helpers.iter().position(|h| h.line == line) else {
            return Ok(None);
        };
        let tracked = facts.tracked_helpers.len() == facts.test_helpers.len();
        Ok(Some(if tracked {
            facts.tracked_helpers[index].clone()
        } else {
            facts.test_helpers[index].clone()
        }))
    }
}
