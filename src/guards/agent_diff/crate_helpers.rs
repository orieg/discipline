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
//! The same resolution says which functions of a crate's test-only modules are helpers
//! at all: one that holds checks and that a test's call resolves to. Such a helper that
//! loses checks is found here whatever changed around it ([`weakened_crate_helpers`]).
//!
//! [`HelperReach`]: crate::ast::HelperReach

#[cfg(feature = "lang-rust")]
use super::{
    call_names_helper, helper_has_checks, pair_file_helpers, text_names, tracked_helpers_of,
};
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
        let mut sides = [
            Side::of(trees[0], registry, vocabs[0], false)?,
            Side::of(trees[1], registry, vocabs[1], true)?,
        ];
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
    /// What each function read by [`Side::callers`] calls: `(file, line)` of the
    /// function, then of each function of the crate one of its calls resolves to.
    calls: std::collections::HashMap<(String, usize), Vec<(String, usize)>>,
}

#[cfg(feature = "lang-rust")]
impl<'a> Side<'a> {
    fn of(
        tree: &'a dyn CrateTree,
        registry: &'a LanguageRegistry,
        vocab: &'a AssertVocabulary,
        head: bool,
    ) -> Result<Self> {
        Ok(Side {
            modules: crate::ast::rust_modules::CrateModules::new(tree.paths()?, |p: &str| {
                tree.read(p)
            }),
            tree,
            registry,
            vocab,
            head,
            helper_files: Vec::new(),
            calls: std::collections::HashMap::new(),
        })
    }

    /// `(file, line)` of each function of the crate that a call of the function on
    /// `line` of `path` resolves to.
    fn resolved_calls(&mut self, path: &str, line: usize) -> Result<&[(String, usize)]> {
        let key = (path.to_string(), line);
        if !self.calls.contains_key(&key) {
            let sites = self.modules.test_calls(path, line)?;
            let targets = sites.into_iter().filter_map(|site| site.target);
            let targets = targets.map(|t| (t.file, t.line)).collect();
            self.calls.insert(key.clone(), targets);
        }
        Ok(&self.calls[&key])
    }

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
            if !self.is_test_code(target) {
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
        let at = self.facts_at(file, test_only)?;
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

    /// Where `helper_files` holds the facts of `file`, read once. `test_only` reads the
    /// file as test code.
    fn facts_at(&mut self, file: &str, test_only: bool) -> Result<usize> {
        let known = self
            .helper_files
            .iter()
            .position(|(path, as_test, _)| path == file && *as_test == test_only);
        if let Some(at) = known {
            return Ok(at);
        }
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
        Ok(self.helper_files.len() - 1)
    }

    /// Which helpers of `file`, by position in the facts at `at`, are test code: all of
    /// them in a test path, and otherwise those on a line built for tests only.
    fn test_code_helpers(&mut self, file: &str, at: usize) -> Result<Vec<bool>> {
        let lines: Vec<usize> = match &self.helper_files[at].2 {
            Some(facts) => facts.test_helpers.iter().map(|h| h.line).collect(),
            None => Vec::new(),
        };
        let whole = crate::ast::functions::test_path(file)
            || crate::ast::functions::declared_test_path(file, &self.vocab.test_paths);
        let mut flags = Vec::new();
        for line in lines {
            flags.push(whole || self.modules.test_only_line(file, line)?);
        }
        Ok(flags)
    }

    /// The helper whose function starts on `line` of `file`, as
    /// [`weakened_crate_helpers`] judges it ([`helper_with_reach`]). `test_only` reads
    /// the file as test code. `None` when the file gives no facts or holds no helper
    /// there.
    fn judged_helper(
        &mut self,
        file: &str,
        line: usize,
        test_only: bool,
    ) -> Result<Option<TestHelperFacts>> {
        let at = self.facts_at(file, test_only)?;
        let flags = self.test_code_helpers(file, at)?;
        let Some(facts) = &self.helper_files[at].2 else {
            return Ok(None);
        };
        let index = facts.test_helpers.iter().position(|h| h.line == line);
        Ok(index.map(|index| helper_with_reach(facts, &flags, index)))
    }

    /// Whether a function `target` is test code whose checks stand for a test's: built
    /// under `#[cfg(test)]`, or in a test path.
    fn is_test_code(&self, target: &crate::ast::rust_modules::CrateFn) -> bool {
        target.test_only
            || crate::ast::functions::test_path(&target.file)
            || crate::ast::functions::declared_test_path(&target.file, &self.vocab.test_paths)
    }

    /// The functions whose calls resolve to the function on `line` of `file`, named
    /// `name`: the tests among them, and the tests that reach it through functions that
    /// are not tests, up to [`crate::ast::HELPER_DEPTH`] calls from the test.
    ///
    /// Every function of every Rust file whose text holds the name of the function
    /// looked for is read, and a call counts only when it resolves to that function
    /// ([`crate::ast::rust_modules`]): a function of the same name elsewhere is not it.
    /// A file that reaches the function under another name alone, given by a re-export
    /// in a third file, is not read.
    fn callers(&mut self, file: &str, line: usize, name: &str) -> Result<Callers> {
        let mut found = Callers::default();
        let mut seen = vec![(file.to_string(), line)];
        let mut queue = vec![(file.to_string(), line, name.to_string(), 1usize)];
        while let Some((file, line, name, depth)) = queue.pop() {
            for path in self.modules.files() {
                let Some(text) = self.modules.file_text(&path)? else {
                    continue;
                };
                if !text_names(&text, &name) {
                    continue;
                }
                for f in self.modules.module_fns(&path)? {
                    if f.file == file && f.line == line {
                        continue;
                    }
                    let calls = self.resolved_calls(&path, f.line)?;
                    if !calls.iter().any(|(to, at)| *to == file && *at == line) {
                        continue;
                    }
                    if depth == 1 {
                        found.direct.push((path.clone(), f.name.clone()));
                    }
                    let at = self.facts_at(&path, false)?;
                    let test = self.helper_files[at].2.as_ref().and_then(|facts| {
                        let test = facts.tests.iter().find(|t| t.line == f.line);
                        test.map(|t| t.name.clone())
                    });
                    match test {
                        Some(test) => found.tests.push(test),
                        None => {
                            let key = (path.clone(), f.line);
                            if depth < crate::ast::HELPER_DEPTH && !seen.contains(&key) {
                                seen.push(key);
                                queue.push((path.clone(), f.line, f.name.clone(), depth + 1));
                            }
                        }
                    }
                }
            }
        }
        found.tests.sort();
        found.tests.dedup();
        Ok(found)
    }

    /// The checks `helper`, a function of `file`, reaches through its calls to test
    /// helpers of other files of the crate, each call resolved: effective, strong and
    /// fatal. A call whose name a helper of `file` has is the pack's, counted there.
    fn reach_elsewhere(
        &mut self,
        file: &str,
        helper: &TestHelperFacts,
    ) -> Result<(usize, usize, usize)> {
        let at = self.facts_at(file, true)?;
        let own: Vec<String> = match &self.helper_files[at].2 {
            Some(facts) => {
                let names = facts.test_helpers.iter();
                names
                    .map(|h| crate::ast::helper_leaf(&h.name).to_string())
                    .collect()
            }
            None => Vec::new(),
        };
        let mut reach = (0, 0, 0);
        for site in self.modules.test_calls(file, helper.line)? {
            let Some(target) = site.target else {
                continue;
            };
            if target.file == file || own.contains(&site.leaf) || !self.is_test_code(&target) {
                continue;
            }
            if let Some(called) = self.judged_helper(&target.file, target.line, target.test_only)? {
                reach.0 += called.effective_asserts();
                reach.1 += called.strong_asserts;
                reach.2 += called.fatal_asserts;
            }
        }
        Ok(reach)
    }

    /// The one helper the calls of `callers`, functions of this side named as on the
    /// other, resolve to under the name `name`, with its file. `None` when such a call
    /// does not resolve, when two resolve to two functions, when none is left, or when
    /// the function is not test code.
    fn resolved_under(
        &mut self,
        callers: &[(String, String)],
        name: &str,
    ) -> Result<Option<(String, TestHelperFacts)>> {
        let mut targets: Vec<crate::ast::rust_modules::CrateFn> = Vec::new();
        for (path, caller) in callers {
            let fns = self.modules.module_fns(path)?;
            let mut named = fns.iter().filter(|f| f.name == *caller);
            let (Some(f), None) = (named.next(), named.next()) else {
                continue;
            };
            for site in self.modules.test_calls(path, f.line)? {
                match site.target {
                    Some(target) if target.name == name => {
                        if !targets.contains(&target) {
                            targets.push(target);
                        }
                    }
                    None if site.leaf == name => return Ok(None),
                    _ => {}
                }
            }
        }
        let [target] = &targets[..] else {
            return Ok(None);
        };
        if !self.is_test_code(target) {
            return Ok(None);
        }
        let helper = self.judged_helper(&target.file, target.line, target.test_only)?;
        Ok(helper.map(|h| (target.file.clone(), h)))
    }
}

/// What calls a function of a crate ([`Side::callers`]).
#[cfg(feature = "lang-rust")]
#[derive(Default)]
struct Callers {
    /// The tests that reach it, by name.
    tests: Vec<String>,
    /// `(file, name)` of each function that calls it itself.
    direct: Vec<(String, String)>,
}

/// A test helper of a test-only module of a Rust crate that holds fewer checks after
/// the change than before it, or is gone ([`weakened_crate_helpers`]).
pub struct WeakenedCrateHelper {
    /// The helper's file on the head side; the file it was in when it is gone.
    pub path: String,
    pub base: TestHelperFacts,
    /// `None` when the helper is gone.
    pub head: Option<TestHelperFacts>,
    /// The tests whose calls resolve to the helper, directly or through other helpers.
    pub callers: Vec<String>,
}

/// The helper at `at` of `facts` with the checks of the helpers of its file that it
/// calls counted in, up to [`crate::ast::HELPER_DEPTH`] calls down, a helper already on
/// the way counted once. Only a helper that is test code (`test_code`, by position)
/// counts: a function of the file that is built outside tests too is the code under
/// test, and what it holds is not the helper's. A call that could name several counts
/// the one with the fewest checks.
#[cfg(feature = "lang-rust")]
fn helper_with_reach(facts: &ParsedFileFacts, test_code: &[bool], at: usize) -> TestHelperFacts {
    fn reach(
        facts: &ParsedFileFacts,
        test_code: &[bool],
        at: usize,
        way: &mut Vec<usize>,
    ) -> (usize, usize, usize) {
        let own = &facts.test_helpers[at];
        let mut held = (
            own.effective_asserts(),
            own.strong_asserts,
            own.fatal_asserts,
        );
        if way.len() + 1 >= crate::ast::HELPER_DEPTH {
            return held;
        }
        way.push(at);
        let calls = facts.helper_calls.get(at).into_iter().flatten();
        for call in calls.flat_map(|c| c.split('|')) {
            let named = (0..facts.test_helpers.len()).filter(|&i| {
                let is_code = test_code.get(i).copied().unwrap_or(false);
                is_code && !way.contains(&i) && call_names_helper(call, &facts.test_helpers[i].name)
            });
            let fewest = named.min_by_key(|&i| {
                let h = &facts.test_helpers[i];
                (h.effective_asserts(), h.strong_asserts)
            });
            if let Some(i) = fewest {
                let more = reach(facts, test_code, i, way);
                held = (held.0 + more.0, held.1 + more.1, held.2 + more.2);
            }
        }
        way.pop();
        held
    }
    let held = reach(facts, test_code, at, &mut Vec::new());
    let mut helper = facts.test_helpers[at].clone();
    helper.total_asserts = held.0 + helper.tautologies;
    helper.strong_asserts = held.1;
    helper.fatal_asserts = held.2;
    helper
}

#[cfg(feature = "lang-rust")]
fn lost(base: &TestHelperFacts, head: Option<&TestHelperFacts>) -> bool {
    head.is_none_or(|h| {
        h.effective_asserts() < base.effective_asserts()
            || h.strong_asserts < base.strong_asserts
            || h.fatal_asserts < base.fatal_asserts
    })
}

/// Whether a test of the file `facts` reaches its helper `name` by the names the pack
/// follows: directly, on a receiver, or through the file's other helpers. The pack
/// counted the helper's checks into such a test.
#[cfg(feature = "lang-rust")]
fn reached_by_own_tests(facts: &ParsedFileFacts, name: &str) -> bool {
    let mut calls: Vec<&str> = Vec::new();
    for test in &facts.tests {
        let own = test.direct_calls.iter();
        let own = own.chain(&test.helper_reach.receiver_calls);
        calls.extend(own.flat_map(|c| c.split('|')));
    }
    let mut seen: Vec<usize> = Vec::new();
    while let Some(call) = calls.pop() {
        for (at, helper) in facts.test_helpers.iter().enumerate() {
            if !call_names_helper(call, &helper.name) || seen.contains(&at) {
                continue;
            }
            if helper.name == name {
                return true;
            }
            seen.push(at);
            let own = facts.helper_calls.get(at).into_iter().flatten();
            calls.extend(own.flat_map(|c| c.split('|')));
        }
    }
    false
}

/// The test helpers of the test-only modules of a Rust crate that the change weakened:
/// what `Test Helper Function Weakened` reports for a file that is not a test-support
/// path (`src/test_support.rs` declared by `#[cfg(test)] mod test_support;`, an inline
/// `#[cfg(test)] mod` of any file).
///
/// A function of a changed Rust file is such a helper when all of these hold:
///
/// - on the base side it is built for tests only, as [`crate::ast::rust_modules`] reads
///   it: the function, an inline module around it, its file or the declaration of a
///   module around it carries `#[cfg(test)]`. A function built outside tests too is the
///   code under test;
/// - it holds checks, its file read as test code, the checks of the test-only helpers
///   of its file that it calls counted in ([`helper_with_reach`]);
/// - the call of a test resolves to it, on the head side or the base side, directly or
///   through functions that are not tests ([`Side::callers`]). A test-only function no
///   test calls is not a helper, and neither is one that only shares the name of a
///   function the tests call;
/// - its file is not a test-support path: `match_helpers` pairs the helpers of those.
///
/// The helper is paired with the function of its file that `match_helpers` would pair
/// it with (the same name, or renamed in place). One that left its file is paired with
/// the function the calls of its callers resolve to on the head side, then with a
/// helper of its name that is new in another changed file, and is gone otherwise. A
/// helper that left its file and that a test of that file reached is not judged: the
/// pack counted its checks into that test, which shows what was lost
/// ([`credit_crate_helpers`]).
///
/// What a pair lost is not a loss when a helper it calls lost it (that helper is judged
/// itself), or when the helper now calls test helpers of other modules that hold as
/// much ([`Side::reach_elsewhere`]).
#[cfg(feature = "lang-rust")]
pub fn weakened_crate_helpers(
    files: &[FileFacts],
    registry: &LanguageRegistry,
    trees: [&dyn CrateTree; 2],
    vocabs: [&AssertVocabulary; 2],
) -> Result<Vec<WeakenedCrateHelper>> {
    let support = crate::ast::functions::test_support_path;
    let rust = |path: &str| registry.find_pack(path).is_some_and(|p| p.id() == "rust");
    let candidates: Vec<&FileFacts> = files
        .iter()
        .filter(|ff| {
            let (old, new) = (&ff.file.old_path, &ff.file.path);
            ff.base.is_some() && rust(old) && !support(old) && !support(new)
        })
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut sides = [
        Side::of(trees[0], registry, vocabs[0], false)?,
        Side::of(trees[1], registry, vocabs[1], true)?,
    ];
    let mut out = Vec::new();
    for ff in candidates {
        let (old, new) = (ff.file.old_path.as_str(), ff.file.path.as_str());
        let test_only: Vec<usize> = sides[0]
            .modules
            .module_fns(old)?
            .into_iter()
            .filter(|f| f.test_only)
            .map(|f| f.line)
            .collect();
        if test_only.is_empty() {
            continue;
        }
        let base_at = sides[0].facts_at(old, true)?;
        let head_at = match ff.head {
            Some(_) => Some(sides[1].facts_at(new, true)?),
            None => None,
        };
        let base_code = sides[0].test_code_helpers(old, base_at)?;
        let head_code = match head_at {
            Some(at) => sides[1].test_code_helpers(new, at)?,
            None => Vec::new(),
        };
        // The helpers that lost something by their own file's reading, and those gone.
        let mut stayed: Vec<(TestHelperFacts, TestHelperFacts)> = Vec::new();
        // Those whose loss is a called helper's, each with the calls of its body.
        let mut inherited: Vec<(TestHelperFacts, TestHelperFacts, Vec<String>)> = Vec::new();
        let mut left: Vec<TestHelperFacts> = Vec::new();
        {
            let Some(base) = sides[0].helper_files[base_at].2.as_ref() else {
                continue;
            };
            let head = head_at.and_then(|at| sides[1].helper_files[at].2.as_ref());
            let paired = pair_file_helpers(Some(base), head);
            let is_helper =
                |b: &TestHelperFacts| test_only.contains(&b.line) && helper_has_checks(b);
            let counts = |h: &TestHelperFacts| {
                let effective = h.effective_asserts();
                (effective, h.strong_asserts, h.fatal_asserts)
            };
            let calls = |facts: &ParsedFileFacts, at: usize| {
                let mut calls = facts.helper_calls.get(at).cloned().unwrap_or_default();
                calls.sort_unstable();
                calls
            };
            for (bi, hi) in paired.matched {
                let Some(head) = head else {
                    continue;
                };
                let b = helper_with_reach(base, &base_code, bi);
                let h = helper_with_reach(head, &head_code, hi);
                if !is_helper(&b) || !lost(&b, Some(&h)) {
                    continue;
                }
                // Its own body and its calls are as they were: a helper it calls lost.
                let from_callee = counts(&base.test_helpers[bi]) == counts(&head.test_helpers[hi])
                    && calls(base, bi) == calls(head, hi);
                if from_callee {
                    inherited.push((b, h, calls(base, bi)));
                } else {
                    stayed.push((b, h));
                }
            }
            for bi in paired.left {
                let b = helper_with_reach(base, &base_code, bi);
                let shown = ff.base.as_ref();
                if is_helper(&b) && !shown.is_some_and(|own| reached_by_own_tests(own, &b.name)) {
                    left.push(b);
                }
            }
        }
        // What a helper lost through a helper it calls is that helper's loss, judged
        // above. A helper that calls none of those judged (the loss is a method's, or a
        // function's that is built outside tests too) is judged itself.
        let mut judged: Vec<String> = stayed.iter().map(|(b, _)| b.name.clone()).collect();
        loop {
            let through = inherited.iter().position(|(_, _, calls)| {
                let mut calls = calls.iter().flat_map(|c| c.split('|'));
                calls.any(|call| judged.iter().any(|name| call_names_helper(call, name)))
            });
            match through {
                Some(at) => judged.push(inherited.remove(at).0.name),
                None => break,
            }
        }
        stayed.extend(inherited.into_iter().map(|(b, h, _)| (b, h)));
        for (b, mut h) in stayed {
            let before = sides[0].reach_elsewhere(old, &b)?;
            let after = sides[1].reach_elsewhere(new, &h)?;
            h.total_asserts += after.0.saturating_sub(before.0);
            h.strong_asserts += after.1.saturating_sub(before.1);
            h.fatal_asserts += after.2.saturating_sub(before.2);
            if !lost(&b, Some(&h)) {
                continue;
            }
            let mut callers = sides[1].callers(new, h.line, &h.name)?.tests;
            if callers.is_empty() {
                callers = sides[0].callers(old, b.line, &b.name)?.tests;
            }
            if !callers.is_empty() {
                out.push(WeakenedCrateHelper {
                    path: new.to_string(),
                    base: b,
                    head: Some(h),
                    callers,
                });
            }
        }
        for b in left {
            let callers = sides[0].callers(old, b.line, &b.name)?;
            if callers.tests.is_empty() {
                continue;
            }
            // The callers as the head side names them.
            let mut now: Vec<(String, String)> = Vec::new();
            for (path, name) in &callers.direct {
                match files.iter().find(|f| f.file.old_path == *path) {
                    Some(f) if f.head.is_none() => {}
                    Some(f) => now.push((f.file.path.clone(), name.clone())),
                    None => now.push((path.clone(), name.clone())),
                }
            }
            let resolved = sides[1].resolved_under(&now, &b.name)?;
            let (path, head) = match resolved {
                Some((path, h)) => (path, Some(h)),
                None => {
                    // A helper of that name that is new in another changed file.
                    let named = |facts: Option<&'_ ParsedFileFacts>| {
                        let helpers = facts.map(tracked_helpers_of).unwrap_or_default();
                        helpers.iter().find(|h| h.name == b.name).cloned()
                    };
                    let new_elsewhere = files.iter().find_map(|other| {
                        let elsewhere =
                            !std::ptr::eq(other, ff) && named(other.base.as_ref()).is_none();
                        let found = elsewhere.then(|| named(other.head.as_ref())).flatten();
                        found.map(|h| (other.file.path.clone(), h))
                    });
                    match new_elsewhere {
                        Some((path, h)) => {
                            // Read as the resolution reads it: as test code when the
                            // function is built for tests only.
                            let as_test = sides[1].modules.test_only_line(&path, h.line)?;
                            let read = sides[1].judged_helper(&path, h.line, as_test)?;
                            (path, Some(read.unwrap_or(h)))
                        }
                        None => (new.to_string(), None),
                    }
                }
            };
            if lost(&b, head.as_ref()) {
                out.push(WeakenedCrateHelper {
                    path,
                    base: b,
                    head,
                    callers: callers.tests,
                });
            }
        }
    }
    Ok(out)
}

/// Without the Rust pack no call is resolved, and no function is such a helper.
#[cfg(not(feature = "lang-rust"))]
pub fn weakened_crate_helpers(
    _files: &[FileFacts],
    _registry: &LanguageRegistry,
    _trees: [&dyn CrateTree; 2],
    _vocabs: [&AssertVocabulary; 2],
) -> Result<Vec<WeakenedCrateHelper>> {
    Ok(Vec::new())
}
