//! Agent-guard gates that reason about the *change*: base vs head facts from
//! the tree-sitter extractor, plus deleted files.

mod assertion_reduction;
mod crate_helpers;
mod deletion_rationale;
mod helper_pairing;
mod ignored_tests;
mod parse_errors;
mod test_matching;
#[cfg(test)]
mod test_support;
mod unsafe_safety_comment;
mod vacuous_tests;
mod vocabulary;

pub use assertion_reduction::*;
pub use crate_helpers::*;
pub use deletion_rationale::*;
pub use helper_pairing::*;
pub use ignored_tests::*;
pub(crate) use parse_errors::*;
pub use test_matching::*;
pub use unsafe_safety_comment::*;
pub use vacuous_tests::*;
pub(crate) use vocabulary::*;

use super::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::ast::{
    default_registry, is_unsupported_source_in, AssertVocabulary, ParsedFileFacts, TestFn,
};
use crate::gitctx::{ChangeKind, ChangedFile};
use anyhow::Result;

pub struct FileFacts {
    pub file: ChangedFile,
    pub base: Option<ParsedFileFacts>,
    pub head: Option<ParsedFileFacts>,
    pub newly_added_nul: bool,
}

pub struct TestPair<'a> {
    pub path: &'a str,
    pub base: &'a TestFn,
    pub head: &'a TestFn,
    pub forced: bool,
}

pub struct Located<'a> {
    pub path: &'a str,
    /// The file still exists on the head side.
    pub file_survives: bool,
    pub test: &'a TestFn,
}

/// Runs every diff-based agent-guard gate and returns one outcome per gate.
/// Disabled gates are filtered by the caller; computing them is cheap.
pub fn run(ctx: &Context) -> Result<Vec<GateOutcome>> {
    let gates = &ctx.config.gates;
    let base_vocab = assert_vocabulary_for_base(ctx)?;
    let head_vocab = assert_vocabulary_for_head(ctx)?;

    let registry = default_registry();
    let changed = ctx.git.changed_files()?;
    let mut analyzed_files = Vec::new();
    let mut packages = GoPackages::default();
    for file in changed
        .iter()
        .filter(|f| registry.is_supported(&f.path) || registry.is_supported(&f.old_path))
    {
        let base_bytes = ctx.git.base_bytes(&file.old_path)?;
        let base_had_nul = base_bytes.as_ref().is_some_and(|b| b.contains(&0));
        let base = match base_bytes {
            Some(bytes) => {
                if let Some(pack) = registry.find_pack(&file.old_path) {
                    let src = String::from_utf8_lossy(&bytes);
                    let mut facts = extract_facts(pack, &file.old_path, &src, &base_vocab)?;
                    packages.resolve(ctx, pack, false, &file.old_path, &src, &mut facts)?;
                    Some(facts)
                } else {
                    None
                }
            }
            None => None,
        };
        let (head, newly_added_nul) = match file.kind {
            ChangeKind::Deleted => (None, false),
            _ => match ctx.git.head_bytes(&file.path)? {
                Some(bytes) => {
                    let has_nul = bytes.contains(&0);
                    let newly_added = has_nul && !base_had_nul;
                    if let Some(pack) = registry.find_pack(&file.path) {
                        let src = String::from_utf8_lossy(&bytes);
                        let mut facts = extract_facts(pack, &file.path, &src, &head_vocab)?;
                        packages.resolve(ctx, pack, true, &file.path, &src, &mut facts)?;
                        (Some(facts), newly_added)
                    } else {
                        (None, newly_added)
                    }
                }
                None => (None, false),
            },
        };
        analyzed_files.push(FileFacts {
            file: file.clone(),
            base,
            head,
            newly_added_nul,
        });
    }

    let crate_helper_notes = if gates.assertion_reduction.enabled {
        credit_crate_helpers(
            &mut analyzed_files,
            &registry,
            [&GitTree { ctx, head: false }, &GitTree { ctx, head: true }],
            [&base_vocab, &head_vocab],
        )?
    } else {
        Vec::new()
    };
    let weakened_crate = if gates.assertion_reduction.enabled {
        weakened_crate_helpers(
            &analyzed_files,
            &registry,
            [&GitTree { ctx, head: false }, &GitTree { ctx, head: true }],
            [&base_vocab, &head_vocab],
        )?
    } else {
        Vec::new()
    };
    let (pairs, removed, added) = match_tests(&analyzed_files);
    let outside = read_outside_files(ctx, &registry, &analyzed_files, &pairs, &head_vocab)?;
    let helpers = pair_helpers_in_tree(&analyzed_files, &pairs, &outside);

    let is_staged = ctx.staged && ctx.pr_body.is_none();
    let mut ast_gates = vec![
        evaluate_assertion_reduction(
            &pairs,
            &added,
            &helpers,
            &gates.assertion_reduction,
            &ctx.directives,
            is_staged,
        )?,
        evaluate_vacuous_tests(&added, &gates.vacuous_tests, &ctx.directives)?,
        evaluate_ignored_tests(
            &pairs,
            &added,
            &gates.ignored_tests,
            &ctx.directives,
            is_staged,
        )?,
        evaluate_unsafe_safety_comment(&analyzed_files, &gates.unsafe_safety_comment)?,
    ];

    report_weakened_crate_helpers(
        &weakened_crate,
        &pairs,
        &analyzed_files,
        &helpers,
        &ReductionInputs {
            settings: &gates.assertion_reduction,
            directives: &ctx.directives,
            is_staged,
        },
        &mut ast_gates[0],
    )?;
    ast_gates[0].notes.extend(crate_helper_notes);

    let first_enabled = [
        ("assertion-reduction", gates.assertion_reduction.enabled),
        ("vacuous-tests", gates.vacuous_tests.enabled),
        ("ignored-tests", gates.ignored_tests.enabled),
        ("unsafe-safety-comment", gates.unsafe_safety_comment.enabled),
    ]
    .into_iter()
    .find(|(_, on)| *on)
    .map(|(id, _)| id);

    if let Some(target) = first_enabled {
        if let Some(outcome) = ast_gates.iter_mut().find(|o| o.gate == target) {
            let target_settings = ctx.config.gates.settings(target);
            let sev = target_settings.map_or(crate::config::Severity::Error, |s| s.severity());
            let exempt = target_settings
                .map(exempt_filter)
                .transpose()?
                .unwrap_or_else(|| PathFilter::new(&[]).unwrap());
            report_parse_errors(&analyzed_files, sev, outcome, &exempt);
            report_newly_added_nul_bytes(
                &analyzed_files,
                sev,
                outcome,
                &ctx.directives,
                is_staged,
                &exempt,
            );
        }
    }

    fn format_unsupported_breakdown(paths: &[&str]) -> String {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for p in paths {
            let ext = p.rsplit('.').next().unwrap_or("");
            *counts.entry(ext).or_default() += 1;
        }
        let c_count = counts.remove("c").unwrap_or(0);
        let h_count = counts.remove("h").unwrap_or(0);
        let ch_count = c_count + h_count;

        let mut cpp_count = 0;
        for k in ["cpp", "cc", "cxx", "hpp", "hh"] {
            cpp_count += counts.remove(k).unwrap_or(0);
        }

        let mut groups: Vec<(String, usize)> = Vec::new();
        if ch_count > 0 {
            groups.push((".c/.h".to_string(), ch_count));
        }
        if cpp_count > 0 {
            groups.push((".cpp/.hpp".to_string(), cpp_count));
        }
        for (ext, count) in counts {
            groups.push((format!(".{ext}"), count));
        }
        groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        groups
            .into_iter()
            .map(|(ext, count)| format!("{count} {ext}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    // Named degradation: source files in a language with no extractor were
    // not analysed. Saying so keeps "0 violations" from reading as coverage.
    let unseen: Vec<&str> = changed
        .iter()
        .filter(|f| f.kind != ChangeKind::Deleted && is_unsupported_source_in(&f.path, &registry))
        .map(|f| f.path.as_str())
        .collect();
    if !unseen.is_empty() {
        let breakdown = format_unsupported_breakdown(&unseen);
        let sample = unseen
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let note = format!(
            "{} changed source file(s) are in a language with no extractor yet ({breakdown}) and were NOT \
             analysed by this gate (e.g. {sample})",
            unseen.len()
        );
        for gate in &mut ast_gates {
            gate.notes.push(note.clone());
        }
    }

    ast_gates.push(evaluate_deletion_rationale(
        &changed,
        &removed,
        &gates.deletion_rationale,
        &ctx.directives,
        is_staged,
    )?);
    Ok(ast_gates)
}

/// One side of the change as the repository holds it: the base tree, or the head side
/// (the working tree, or the index for a staged check).
struct GitTree<'a> {
    ctx: &'a Context<'a>,
    head: bool,
}

impl CrateTree for GitTree<'_> {
    fn paths(&self) -> Result<Vec<String>> {
        if !self.head {
            return self.ctx.git.base_tracked_files();
        }
        // A file the change adds may not be in the index yet.
        let mut paths = self.ctx.git.tracked_files()?;
        for file in self.ctx.git.changed_files()? {
            if file.kind != ChangeKind::Deleted && !paths.contains(&file.path) {
                paths.push(file.path);
            }
        }
        Ok(paths)
    }

    fn read(&self, path: &str) -> Result<Option<String>> {
        let bytes = if self.head {
            self.ctx.git.head_bytes(path)?
        } else {
            self.ctx.git.base_bytes(path)?
        };
        Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
    }
}

/// The other Go files of each package directory a changed test file stands in, per side
/// of the change, read once: a case table or row type declared in another file of the
/// package is resolved there (`crate::ast::go::resolve_package_cases`).
#[derive(Default)]
struct GoPackages {
    read: Vec<GoDirectory>,
    tracked: [Option<Vec<String>>; 2],
}

/// The Go files of one directory on one side of the change.
struct GoDirectory {
    head: bool,
    dir: String,
    /// `(path, source)` of each.
    files: Vec<(String, String)>,
}

impl GoPackages {
    #[cfg(feature = "lang-go")]
    fn resolve(
        &mut self,
        ctx: &Context,
        pack: &dyn crate::ast::LanguagePack,
        head: bool,
        path: &str,
        src: &str,
        facts: &mut ParsedFileFacts,
    ) -> Result<()> {
        if pack.id() != "go" || facts.tests.is_empty() {
            return Ok(());
        }
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
        if !self.read.iter().any(|d| d.head == head && d.dir == dir) {
            let tracked = &mut self.tracked[usize::from(head)];
            if tracked.is_none() {
                *tracked = Some(if head {
                    ctx.git.tracked_files()?
                } else {
                    ctx.git.base_tracked_files()?
                });
            }
            let mut files = Vec::new();
            for other in tracked.iter().flatten() {
                let in_dir = other.rsplit_once('/').map_or("", |(dir, _)| dir) == dir;
                if !in_dir || !other.ends_with(".go") {
                    continue;
                }
                let bytes = if head {
                    ctx.git.head_bytes(other)?
                } else {
                    ctx.git.base_bytes(other)?
                };
                if let Some(bytes) = bytes {
                    files.push((other.clone(), String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
            self.read.push(GoDirectory {
                head,
                dir: dir.clone(),
                files,
            });
        }
        let siblings: Vec<(String, String)> = self
            .read
            .iter()
            .filter(|d| d.head == head && d.dir == dir)
            .flat_map(|d| d.files.iter())
            .filter(|(p, _)| p != path)
            .cloned()
            .collect();
        crate::ast::go::resolve_package_cases(facts, path, src, &siblings)
    }

    #[cfg(not(feature = "lang-go"))]
    fn resolve(
        &mut self,
        _ctx: &Context,
        _pack: &dyn crate::ast::LanguagePack,
        _head: bool,
        _path: &str,
        _src: &str,
        _facts: &mut ParsedFileFacts,
    ) -> Result<()> {
        Ok(())
    }
}

/// One side of a changed file as the test gates read it: the pack's facts, with the
/// checks of the helpers each helper calls counted into it.
pub fn extract_facts(
    pack: &dyn crate::ast::LanguagePack,
    path: &str,
    src: &str,
    vocab: &AssertVocabulary,
) -> Result<ParsedFileFacts> {
    let mut facts = pack.extract(path, src, vocab)?;
    facts.resolve_tracked_helpers();
    facts.resolve_helper_reach(&vocab.helper_fns);
    Ok(facts)
}

fn helper_lost(hp: &HelperPair) -> bool {
    match hp.head {
        None => true,
        Some(h) => {
            h.effective_asserts() < hp.base.effective_asserts()
                || h.strong_asserts < hp.base.strong_asserts
                || h.fatal_asserts < hp.base.fatal_asserts
        }
    }
}

fn holds_no_test(ff: &FileFacts) -> bool {
    [ff.base.as_ref(), ff.head.as_ref()]
        .into_iter()
        .flatten()
        .all(|facts| facts.tests.is_empty())
}

/// The last `::` / `.` segment of each callee a call names (`a|b` names two).
fn call_leaves(call: &str) -> impl Iterator<Item = &str> {
    call.split('|').map(crate::ast::helper_leaf)
}

/// Functions a runner calls without a test naming them: a change to one is judged as
/// before, whatever names it.
const RUN_UNNAMED: &[&str] = &[
    "setUp",
    "tearDown",
    "setUpClass",
    "tearDownClass",
    "setup_method",
    "teardown_method",
    "setup_class",
    "teardown_class",
    "setup_module",
    "teardown_module",
    "setup",
    "teardown",
    "SetUp",
    "TearDown",
    "SetUpTest",
    "TearDownTest",
    "SetupTest",
    "TearDownSuite",
    "SetupSuite",
    "SetUpSuite",
    "TestMain",
    "beforeEach",
    "afterEach",
    "beforeAll",
    "afterAll",
    "before",
    "after",
];

/// Whether a function of a file that holds no test is judged whatever names it: one a
/// runner calls by its name alone, and every function of a pytest `conftest.py`, whose
/// fixtures a test receives as parameters.
fn run_unnamed(path: &str, helper: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file == "conftest.py" || RUN_UNNAMED.contains(&crate::ast::helper_leaf(helper))
}

/// The unchanged files `assertion-reduction` reads beside the change, from the head
/// tree, and only when the change calls for it:
///
/// - a test-support file ([`test_support_path`]) holding a helper that a test whose
///   assertions dropped calls and its own file does not define, so the helper stands
///   for the checks it holds ([`outside_helpers`]);
/// - a test file or test-support file naming a function that lost checks in a changed
///   file holding no test, so that function is known to be called by a test
///   ([`leave_unreferenced_functions`]).
///
/// A file is parsed only when its text holds one of the names looked for, and is of the
/// language of the file the name came from.
///
/// [`test_support_path`]: crate::ast::functions::test_support_path
fn read_outside_files(
    ctx: &Context,
    registry: &crate::ast::LanguageRegistry,
    files: &[FileFacts],
    pairs: &[TestPair],
    vocab: &AssertVocabulary,
) -> Result<Vec<OutsideFile>> {
    let pack_id = |path: &str| registry.find_pack(path).map(|p| p.id());
    // (pack, name) of the helpers looked for, and of the functions whose callers are.
    let mut helpers_wanted: Vec<(&str, &str)> = Vec::new();
    for p in pairs {
        let (b, h) = (p.base, p.head);
        if h.effective_asserts() >= b.effective_asserts() && h.strong_asserts >= b.strong_asserts {
            continue;
        }
        let Some(id) = pack_id(p.path) else { continue };
        let reach = &h.helper_reach;
        for call in h.direct_calls.iter().chain(&reach.receiver_calls) {
            if !reach.own_file_calls.contains(call) {
                helpers_wanted.extend(call_leaves(call).map(|leaf| (id, leaf)));
            }
        }
    }
    let mut callers_wanted: Vec<(&str, &str)> = Vec::new();
    for hp in match_helpers(files) {
        let no_test = files
            .iter()
            .any(|ff| ff.file.path == hp.path && holds_no_test(ff));
        if helper_lost(&hp) && no_test && !run_unnamed(hp.path, &hp.base.name) {
            if let Some(id) = pack_id(hp.path) {
                callers_wanted.push((id, crate::ast::helper_leaf(&hp.base.name)));
            }
        }
    }
    // A test may name such a function through another function of its file.
    for ff in files.iter().filter(|ff| holds_no_test(ff)) {
        let Some(id) = pack_id(&ff.file.path) else {
            continue;
        };
        loop {
            let mut grew = false;
            for facts in [ff.base.as_ref(), ff.head.as_ref()].into_iter().flatten() {
                for (at, helper) in facts.test_helpers.iter().enumerate() {
                    let leaf = crate::ast::helper_leaf(&helper.name);
                    let calls = facts.helper_calls.get(at).into_iter().flatten();
                    let reaches = calls
                        .flat_map(|c| call_leaves(c))
                        .any(|c| callers_wanted.contains(&(id, c)));
                    if reaches && !callers_wanted.contains(&(id, leaf)) {
                        callers_wanted.push((id, leaf));
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
    }
    if helpers_wanted.is_empty() && callers_wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for path in ctx.git.tracked_files()? {
        if files
            .iter()
            .any(|ff| ff.file.path == path || ff.file.old_path == path)
        {
            continue;
        }
        let Some(pack) = registry.find_pack(&path) else {
            continue;
        };
        let support = crate::ast::functions::test_support_path(&path);
        let holds = |wanted: &[(&str, &str)], text: &str| {
            wanted
                .iter()
                .any(|(id, name)| *id == pack.id() && !name.is_empty() && text.contains(name))
        };
        let for_helpers = support && helpers_wanted.iter().any(|(id, _)| *id == pack.id());
        let for_callers = (support || pack.is_test_path(&path))
            && callers_wanted.iter().any(|(id, _)| *id == pack.id());
        if !for_helpers && !for_callers {
            continue;
        }
        let Some(bytes) = ctx.git.head_bytes(&path)? else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        if !(for_helpers && holds(&helpers_wanted, &text)
            || for_callers && holds(&callers_wanted, &text))
        {
            continue;
        }
        match extract_facts(pack, &path, &text, vocab) {
            Ok(facts) => out.push(OutsideFile {
                path,
                facts: Some(facts),
                text: String::new(),
            }),
            Err(_) => out.push(OutsideFile {
                path,
                facts: None,
                text: text.into_owned(),
            }),
        }
    }
    Ok(out)
}

/// What a test's calls to paired helpers add to it across a change.
struct HelperCallGain {
    total: usize,
    strong: usize,
    fatal: usize,
    /// Equality checks written by hand in helpers of other files
    /// (`TestHelperFacts::equality_exits`), per call as the counts above.
    equality: usize,
    names: Vec<String>,
}

fn call_names_helper(call: &str, helper: &str) -> bool {
    crate::ast::helper_call_matches(call, helper)
        || crate::ast::helper_call_matches(call, crate::ast::helper_leaf(helper))
}

/// A drop in a test that calls more same-file helpers that fail than before, where more
/// of those calls check in a loop (`crate::ast::helper_loops`): one check that runs once
/// per element stands for many inline assertions, so the count is not compared. A helper
/// that checks in a straight line is held to what it has ([`helper_call_gain`]).
fn moved_into_looping_helpers(base: &TestFn, head: &TestFn) -> bool {
    head.helper_checks > base.helper_checks && head.helper_reach.looped > base.helper_reach.looped
}

/// Equality checks written by hand that the change added to the same-file helpers the
/// test calls: failure exits guarded by an equality comparison
/// (`HelperReach::equality_exits`).
fn equality_exits_gained(base: &TestFn, head: &TestFn) -> usize {
    let (base, head) = (&base.helper_reach, &head.helper_reach);
    head.equality_exits.saturating_sub(base.equality_exits)
}

/// The checks the helper calls of `head` account for beyond those of `base`, which the
/// tests' own counts do not hold.
///
/// **A helper of the test's own file** is counted into the test by the pack as far as
/// the pack follows it. What it holds beyond that is in `TestFn::helper_reach`: the
/// checks of a method called through a type or an object (`Checks.check(r)`), which
/// stands for the least-checking same-file method of that name, and the checks a helper
/// reaches more calls down than the pack follows, `HELPER_DEPTH` calls from the test.
/// The head's less the base's is what the change moved there.
///
/// **A helper in another changed test-support file** counts through its pair. A call
/// that names a helper of the test's own file (`HelperReach::own_file_calls`) resolves
/// there and is given nothing by a helper of the same name elsewhere; a call to a helper
/// in a file outside the change names no pair and adds nothing. The drop such a call
/// would explain stays reported.
///
/// Each remaining call, direct or on a receiver, is resolved to the pair whose helper it
/// names ([`crate::ast::helper_call_matches`], on the helper's name or its last `::` /
/// `.` segment): a head call by the head name, a base call by the base name, which
/// differ for a renamed helper. When several helpers have that name, the one with the
/// fewest checks counts. A pair contributes its head count for every head call site less
/// its base count for every base call site: the head count for a helper the test newly
/// calls, the count the helper gained for one the base test already called. A call to a
/// helper listed in `configured` (`assert_helper_fns`) is already one assertion of the
/// test and counts one less.
///
/// **A helper in another file of the test's crate**, resolved through the crate's
/// modules (`HelperReach::crate_helpers`, Rust), counts on both sides: the head's less
/// the base's is added, and a shortfall, the checks of a call the head test no longer
/// makes or of a helper that lost some, is taken from what the helpers above gave.
fn helper_call_gain<'a>(
    base: &TestFn,
    head: &TestFn,
    test_path: &str,
    helpers: &[HelperPair<'a>],
    configured: &[String],
) -> HelperCallGain {
    let paired: Vec<(
        &'a crate::ast::TestHelperFacts,
        &'a crate::ast::TestHelperFacts,
        bool,
    )> = helpers
        .iter()
        .filter_map(|hp| {
            hp.head
                .map(|head_helper| (hp.base, head_helper, hp.path == test_path))
        })
        .collect();
    let sites = |test: &TestFn, head_side: bool| -> Vec<(usize, usize)> {
        let mut per_pair = vec![(0usize, 0usize); paired.len()];
        let reach = &test.helper_reach;
        for call in test.direct_calls.iter().chain(&reach.receiver_calls) {
            if reach.own_file_calls.contains(call) {
                continue;
            }
            let named: Vec<usize> = (0..paired.len())
                .filter(|&i| {
                    let (base_helper, head_helper, _) = paired[i];
                    let helper = if head_side { head_helper } else { base_helper };
                    call_names_helper(call, &helper.name)
                })
                .collect();
            if named.iter().any(|&i| paired[i].2) {
                continue;
            }
            let fewest = named.into_iter().min_by_key(|&i| {
                let helper = paired[i].1;
                (helper.effective_asserts(), helper.strong_asserts)
            });
            if let Some(i) = fewest {
                per_pair[i].0 += 1;
                if configured
                    .iter()
                    .any(|name| crate::ast::helper_call_matches(call, name))
                {
                    per_pair[i].1 += 1;
                }
            }
        }
        per_pair
    };
    let (base_sites, head_sites) = (sites(base, false), sites(head, true));
    let (own_base, own_head) = (&base.helper_reach, &head.helper_reach);
    let mut gain = HelperCallGain {
        total: own_head.total.saturating_sub(own_base.total),
        strong: own_head.strong.saturating_sub(own_base.strong),
        fatal: own_head.fatal.saturating_sub(own_base.fatal),
        equality: 0,
        names: Vec::new(),
    };
    if gain.total > 0 || gain.strong > 0 {
        gain.names.extend(own_head.names.iter().cloned());
    }
    for (i, &(base_helper, head_helper, _)) in paired.iter().enumerate() {
        let (base_calls, base_counted) = base_sites[i];
        let (head_calls, head_counted) = head_sites[i];
        let total = (head_calls * head_helper.effective_asserts())
            .saturating_sub(head_counted)
            .saturating_sub(
                (base_calls * base_helper.effective_asserts()).saturating_sub(base_counted),
            );
        let strong = (head_calls * head_helper.strong_asserts)
            .saturating_sub(base_calls * base_helper.strong_asserts);
        let fatal = (head_calls * head_helper.fatal_asserts)
            .saturating_sub(base_calls * base_helper.fatal_asserts);
        gain.equality += (head_calls * head_helper.equality_exits)
            .saturating_sub(base_calls * base_helper.equality_exits);
        if total > 0 || strong > 0 {
            gain.total += total;
            gain.strong += strong;
            gain.fatal += fatal;
            gain.names.push(head_helper.name.clone());
        }
    }
    // Helpers in other files of the test's crate, counted on both sides: what the head
    // side holds less than the base side is lost, and is taken from the rest.
    let (crate_base, crate_head) = (&own_base.crate_helpers, &own_head.crate_helpers);
    if crate_head.total > crate_base.total || crate_head.strong > crate_base.strong {
        gain.names.extend(crate_head.names.iter().cloned());
    }
    gain.total = (gain.total + crate_head.total).saturating_sub(crate_base.total);
    gain.strong = (gain.strong + crate_head.strong).saturating_sub(crate_base.strong);
    gain.fatal = (gain.fatal + crate_head.fatal).saturating_sub(crate_base.fatal);
    gain.equality =
        (gain.equality + crate_head.equality_exits).saturating_sub(crate_base.equality_exits);
    gain
}

pub(crate) fn leaf_name(test: &TestFn) -> &str {
    let s = test.name.rsplit("::").next().unwrap_or(&test.name);
    let s = s.rsplit('#').next().unwrap_or(s);
    s.rsplit(" > ").next().unwrap_or(s)
}

/// What the self-comparison findings do and do not read, stated in each message.
const SELF_COMPARISON_SCOPE: &str = "Exact form only: the two operands are the same tokens; no alias or value-flow analysis, so two names for one value are not seen.";

/// The note a gate carries, once, when it reports a self-comparison.
const SELF_COMPARISON_NOTE: &str = "self-comparison findings read the exact form only (an equality assertion whose two operands are the same tokens, with no call in them and outside a macro definition); no alias/value-flow analysis: two names bound to one value, and an assertion whose operands never reach the code under test, are not reported";

fn note_self_comparison_scope(out: &mut GateOutcome) {
    if !out.notes.iter().any(|n| n == SELF_COMPARISON_NOTE) {
        out.notes.push(SELF_COMPARISON_NOTE.to_string());
    }
}

/// The lines of `found`, in order, as `3, 7`.
fn lines_of(found: &[&crate::ast::self_comparison::SelfComparison]) -> String {
    found
        .iter()
        .map(|s| s.line.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::agent_diff::test_support::*;

    /// #562: of two helpers a call can name, the one with fewer checks counts; and a
    /// renamed helper the test already called under its old name adds only what it gained.
    #[test]
    fn a_call_counts_the_least_helper_it_names_and_a_rename_adds_nothing() {
        let (small, large) = (
            helper_facts("A::check", 1, 1),
            helper_facts("B::check", 3, 3),
        );
        let helpers = [
            HelperPair {
                path: "tests/helpers.py",
                base: &large,
                head: Some(&large),
            },
            HelperPair {
                path: "tests/helpers.py",
                base: &small,
                head: Some(&small),
            },
        ];
        let gain = helper_call_gain(
            &calling_test(3, 3, &[]),
            &calling_test(0, 0, &["check"]),
            "tests/test_api.py",
            &helpers,
            &[],
        );
        assert_eq!(gain.total, 1);

        let (old, new) = (
            helper_facts("check", 3, 3),
            helper_facts("check_response", 3, 3),
        );
        let helpers = [HelperPair {
            path: "tests/helpers.py",
            base: &old,
            head: Some(&new),
        }];
        let gain = helper_call_gain(
            &calling_test(3, 3, &["check"]),
            &calling_test(0, 0, &["check_response"]),
            "tests/test_api.py",
            &helpers,
            &[],
        );
        assert_eq!((gain.total, gain.strong), (0, 0));
    }

    /// #595: a call that names a helper of the test's own file is given nothing by a
    /// helper of the same name in another changed file.
    #[test]
    fn a_call_to_a_same_file_helper_is_not_credited_by_another_files_helper() {
        let b = calling_test(3, 3, &[]);
        let mut h = calling_test(0, 0, &["check"]);
        let other = helper_facts("check", 3, 3);
        let helpers = [HelperPair {
            path: "tests/common/mod.rs",
            base: &other,
            head: Some(&other),
        }];
        // Control: no helper of that name in the calling file.
        let gain = helper_call_gain(&b, &h, "src/lib.rs", &helpers, &[]);
        assert_eq!((gain.total, gain.strong), (3, 3));
        h.helper_reach.own_file_calls = vec!["check".to_string()];
        let gain = helper_call_gain(&b, &h, "src/lib.rs", &helpers, &[]);
        assert_eq!((gain.total, gain.strong), (0, 0));
        // A receiver call names a helper of another changed file like a direct one.
        let mut h = calling_test(0, 0, &[]);
        h.helper_reach.receiver_calls = vec!["check".to_string()];
        let gain = helper_call_gain(&b, &h, "src/lib.rs", &helpers, &[]);
        assert_eq!((gain.total, gain.strong), (3, 3));
    }
}
