pub mod agent_diff;
pub mod archive_contents;
pub mod archive_formats;
pub mod archive_presets;
pub mod build_flags;
pub mod build_hooks;
pub mod ci_exposure;
pub mod ci_gitlab;
pub mod ci_integrity;
pub mod ci_skip_set;
pub mod citation_metadata;
pub mod claim_registry;
pub mod command;
pub mod commit_provenance;
pub mod config_delta;
pub mod confusables;
#[cfg(test)]
mod confusables_tests;
pub mod dependency;
pub mod error_swallowing;
pub mod harness_tampering;
pub mod hygiene;
pub mod instruction_smuggling;
pub mod integrity;
pub mod issue_link;
pub mod lockfile;
pub mod manifest_sync;
pub mod measured_citations;
pub mod miri;
pub mod msrv;
pub mod perf;
pub mod pr_checklist;
pub mod presets;
pub mod provenance_tags;
pub mod ratified_paths;
pub mod review_threads;
pub mod sandbox_config;
pub mod sanitizers;
pub mod scope_confinement;
pub mod shell_secrets;
pub mod source_maps;
pub mod stub_bodies;
pub mod suppression_delta;
pub mod test_budget;
pub mod test_floor;
pub mod token_formats;
pub mod toolchain_config;
pub mod unsafe_budget;
pub mod version_lockstep;

use crate::cli::SuiteChoice;
pub use crate::config::Severity;
use crate::config::{gate_info, DisciplineConfig, GateSettings, Suite, GATES};
use crate::gitctx::GitCtx;
use anyhow::{anyhow, bail, Context as _, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

/// Whether `path` is how a gate names the pull request body as the place of a finding
/// (`time-estimates` and `pii` write `<PR body>`, `pr-checklist` and `provenance-tags`
/// `PR body`).
pub fn is_pr_body_label(path: &str) -> bool {
    matches!(path, "<PR body>" | "PR body")
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub gate: &'static str,
    /// `gate/code`: the finding's stable identity (`crate::findings`).
    pub code: String,
    /// The version-2 baseline fingerprint (`crate::baseline`): stable across line moves
    /// and title changes. Filled once all gates have run.
    pub fingerprint: String,
    /// The title a fingerprint-version-1 baseline recorded, when it differs from `title`
    /// ([`crate::findings::V1`]). Not reported.
    #[serde(skip)]
    pub legacy_title: Option<String>,
    /// What tells this finding apart from another of its code in the same file when it
    /// has no line: a typed source datum (a test, dependency, key, job or counter name),
    /// never prose. The version-2 fingerprint hashes it in place of the line
    /// ([`crate::baseline`]). Not reported.
    #[serde(skip)]
    pub anchor: Option<String>,
    pub severity: Severity,
    pub title: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub message: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateOutcome {
    pub gate: &'static str,
    pub suite: &'static str,
    pub enabled: bool,
    /// How many items (files, tests, config keys ...) the gate looked at. A
    /// report always shows this, so "0 violations" over "0 examined" is visible.
    pub examined: usize,
    /// Lines skipped through an inline `discipline:allow(<gate>)` marker.
    pub inline_exemptions: usize,
    /// Findings matching the grandfathering baseline (not blocking).
    pub baselined: usize,
    /// Named degradations: what the gate could not verify, and why.
    pub notes: Vec<String>,
    pub violations: Vec<Violation>,
    pub overrides: Vec<crate::tokens::OverrideRecord>,
}

/// The note for a changed file a gate could not read as text (F7): binary content, or a
/// symlink that leaves the repository.
pub fn unread_note(path: &str) -> String {
    format!("skipped `{path}` (binary or unreadable file)")
}

/// Base-anchored classification for one changed file, shared by the gates that
/// read whole-file and per-function test scope (`error-swallowing`,
/// `stub-bodies`): a file that was production code on the base side stays
/// production code through a rename, so a rename cannot move it into test scope.
/// A file that was test code on the base side, and a file the change adds, are
/// classified by their head path.
///
/// The extension also selects the grammar a pack parses with, so a rename that
/// changes the extension or the language is never classified by the base path
/// itself: a production file is classified by its base path carrying the head
/// extension ([`production_path_for`]), which keeps the head grammar.
pub struct BaseAnchored {
    /// Path to pass to `pack.extract` for classification; the reporting path
    /// stays `file.path`.
    pub classify_path: String,
    /// A rename of production code into a path a pack's convention reads as
    /// test code: the caller in `error-swallowing` reports it once as
    /// `test-path-reclassification`; `stub-bodies` skips it (it already judges
    /// the same file's bodies).
    pub reclassified: bool,
    /// A rename across languages or extensions; the caller records this as a
    /// gate note.
    pub language_changed_note: Option<String>,
    /// A rename of production code, extension unchanged, into a path that only
    /// `[tests] paths` reads as test code: it stays production code and no
    /// finding says so, so the caller records this as a gate note.
    pub declared_scope_note: Option<String>,
}

/// A path the head-side pack parses with the grammar of `head_path` and reads
/// as production code, for a file that was production code at `base_path`: the
/// base path with the head extension, or a bare file name with that extension
/// when the head pack's convention reads even that as test code (`Spec.java`
/// to `tests/Spec.kt`). `None` when there is no such path: the head path has
/// no extension, or a `[tests] paths` glob covers every file of the extension.
fn production_path_for(
    base_path: &str,
    head_path: &str,
    head_pack: &dyn crate::ast::LanguagePack,
    registry: &crate::ast::LanguageRegistry,
    declared_test_paths: &[String],
) -> Option<String> {
    let head_ext = crate::ast::extension(head_path)?;
    let with_head_ext = match crate::ast::extension(base_path) {
        Some(base_ext) => format!(
            "{}{head_ext}",
            &base_path[..base_path.len() - base_ext.len()]
        ),
        None => format!("{base_path}.{head_ext}"),
    };
    [with_head_ext, format!("renamed.{head_ext}")]
        .into_iter()
        .find(|candidate| {
            registry
                .find_pack(candidate)
                .is_some_and(|p| p.id() == head_pack.id())
                && !head_pack.is_test_path(candidate)
                && !crate::ast::functions::declared_test_path(candidate, declared_test_paths)
        })
}

pub fn base_anchored_classification(
    file: &crate::gitctx::ChangedFile,
    registry: &crate::ast::LanguageRegistry,
    declared_test_paths: &[String],
) -> BaseAnchored {
    let by_head_path = |language_changed_note: Option<String>| BaseAnchored {
        classify_path: file.path.clone(),
        reclassified: false,
        language_changed_note,
        declared_scope_note: None,
    };
    if file.old_path == file.path {
        return by_head_path(None);
    }
    let by_new_path = || {
        format!(
            "`{}` was renamed from `{}` to another language or extension; classified by its new path",
            file.path, file.old_path
        )
    };
    let Some(head_pack) = registry.find_pack(&file.path) else {
        return by_head_path(Some(by_new_path()));
    };
    let base_pack = registry.find_pack(&file.old_path);
    let same_language = base_pack.is_some_and(|base| base.id() == head_pack.id());
    let same_extension = crate::ast::extension(&file.path) == crate::ast::extension(&file.old_path);
    let declared =
        |path: &str| crate::ast::functions::declared_test_path(path, declared_test_paths);
    // Whether the path moved into test scope does not depend on the grammar: the base
    // path is judged by the pack that read it there, or by the head pack when no pack
    // read it (`notes.txt` to `test_notes.py`).
    let base_by_convention = base_pack.unwrap_or(head_pack).is_test_path(&file.old_path);
    let head_by_convention = head_pack.is_test_path(&file.path);
    if same_language && same_extension {
        // The anchor exists so that a move into test scope cannot silence a production
        // file. A file that was test code on the base side has nothing to keep: it is
        // judged by where it is now, so a move out of test scope makes it production code.
        let base_is_test = base_by_convention || declared(&file.old_path);
        return BaseAnchored {
            classify_path: if base_is_test {
                file.path.clone()
            } else {
                file.old_path.clone()
            },
            reclassified: !base_is_test && head_by_convention,
            language_changed_note: None,
            declared_scope_note: (!base_is_test && !head_by_convention && declared(&file.path))
                .then(|| {
                    format!(
                        "`{}` was renamed from `{}` into a path under `[tests] paths`; it was production code on the base side and is still judged as production code",
                        file.path, file.old_path
                    )
                }),
        };
    }
    if base_by_convention || declared(&file.old_path) {
        return by_head_path(Some(by_new_path()));
    }
    if !head_by_convention && !declared(&file.path) {
        // Production code on both sides: the head path says so itself.
        return by_head_path(Some(by_new_path()));
    }
    let Some(classify_path) = production_path_for(
        &file.old_path,
        &file.path,
        head_pack,
        registry,
        declared_test_paths,
    ) else {
        return by_head_path(Some(by_new_path()));
    };
    BaseAnchored {
        classify_path,
        reclassified: head_by_convention,
        language_changed_note: Some(format!(
            "`{}` was renamed from `{}` to another language or extension; it was production code on the base side and is still judged as production code",
            file.path, file.old_path
        )),
        declared_scope_note: None,
    }
}

/// A production file the change deletes and adds again under a test name: for an added
/// file whose path is test scope by its file NAME only (outside every test directory,
/// not under `[tests] paths`), the deleted file it replaces and the path the pack reads
/// the added source under as production code. `None` for every other file.
///
/// The deleted file is one of the same language pack that was production code on the
/// base side and whose stem is the added file's stem without its test word
/// ([`crate::ast::functions::stems_without_test_word`]): `loader.py` and
/// `test_loader.py`, `Repo.java` and `RepoTest.java`, `util.js` and `util.spec.mjs`.
/// The directory may differ, except for a Go `_test.go` file: a Go package is a
/// directory, so `repo.go` and `repo_test.go` pair in the same directory only.
///
/// `error-swallowing` and `stub-bodies` judge the added file by the returned path when
/// it holds no test that checks anything: git pairs a rename only when enough content
/// is unchanged, so below that threshold there is no base path to anchor to.
pub struct ReplacedProduction {
    /// The deleted production file.
    pub deleted: String,
    /// Path to pass to `pack.extract` so that the source is read as production code.
    pub classify_path: String,
}

pub fn replaced_production_file(
    file: &crate::gitctx::ChangedFile,
    changed: &[crate::gitctx::ChangedFile],
    pack: &dyn crate::ast::LanguagePack,
    registry: &crate::ast::LanguageRegistry,
    declared_test_paths: &[String],
) -> Option<ReplacedProduction> {
    use crate::ast::functions::{file_name, stems_without_test_word};
    let declared =
        |path: &str| crate::ast::functions::declared_test_path(path, declared_test_paths);
    if file.kind != crate::gitctx::ChangeKind::Added
        || !pack.is_test_path(&file.path)
        || declared(&file.path)
    {
        return None;
    }
    let name = file_name(&file.path);
    let directory = &file.path[..file.path.len() - name.len()];
    let neutral = format!("{directory}renamed.{}", crate::ast::extension(&file.path)?);
    let same_pack = |path: &str| {
        registry
            .find_pack(path)
            .is_some_and(|p| p.id() == pack.id())
    };
    if !same_pack(&neutral) || pack.is_test_path(&neutral) || declared(&neutral) {
        return None;
    }
    let same_package_only = crate::ast::functions::go_test_file(&file.path);
    let stems = stems_without_test_word(name);
    let deleted = changed.iter().find(|other| {
        let other_name = file_name(&other.path);
        let other_stem = other_name
            .rsplit_once('.')
            .map_or(other_name, |(stem, _)| stem);
        other.kind == crate::gitctx::ChangeKind::Deleted
            && same_pack(&other.path)
            && !pack.is_test_path(&other.path)
            && !declared(&other.path)
            && stems.iter().any(|stem| stem == other_stem)
            && (!same_package_only
                || other.path[..other.path.len() - other_name.len()] == *directory)
    })?;
    Some(ReplacedProduction {
        deleted: deleted.path.clone(),
        classify_path: neutral,
    })
}

/// The gate note for a file [`replaced_production_file`] names, once a gate judged it as
/// production code.
pub fn replaced_production_note(path: &str, deleted: &str, tests: usize) -> String {
    format!(
        "`{path}` is added under a test file name and {}, and the change deletes the production file `{deleted}` of the same name; it is judged as production code",
        holds_no_checking_test(tests)
    )
}

/// How a note says that a file of `tests` tests holds none that checks anything: a file
/// with tests in it is a real test file to its reader, so the note counts them.
fn holds_no_checking_test(tests: usize) -> String {
    match tests {
        0 => "holds no test".to_string(),
        n => format!("holds {n} test(s), none of which checks anything"),
    }
}

/// A file the change adds that is test scope by a naming heuristic alone, for
/// `error-swallowing` and `stub-bodies` to judge as production code when it holds no
/// test that checks anything ([`holds_a_checking_test`]). `None` for every other file.
///
/// The file is one the change adds, whose path a pack's convention reads as test code,
/// that `[tests] paths` does not declare and that no build tool makes test code by
/// construction ([`crate::ast::functions::test_by_construction`]): a name or a
/// directory cannot say whether `TestDataBuilder.cs`, `app/test_runner.py` or
/// `tests/support/helpers.py` is test code, and the tests it holds can. A file
/// [`replaced_production_file`] pairs is one of these whatever the build tool says.
///
/// A file the base already had is not one: it keeps its classification
/// ([`base_anchored_classification`]).
///
/// `tests` is the number of tests the pack found in the file, for the note.
pub struct TestScopeByNameOnly {
    /// Path to pass to `pack.extract` so that the source is read as production code;
    /// `None` when the pack reads no path of this extension as production code.
    pub classify_path: Option<String>,
    /// The gate note, once a gate judged the file as production code (or could not).
    pub note: String,
}

pub fn test_scope_by_name_only(
    file: &crate::gitctx::ChangedFile,
    changed: &[crate::gitctx::ChangedFile],
    pack: &dyn crate::ast::LanguagePack,
    registry: &crate::ast::LanguageRegistry,
    declared_test_paths: &[String],
    tests: usize,
) -> Option<TestScopeByNameOnly> {
    if let Some(replaced) =
        replaced_production_file(file, changed, pack, registry, declared_test_paths)
    {
        return Some(TestScopeByNameOnly {
            note: replaced_production_note(&file.path, &replaced.deleted, tests),
            classify_path: Some(replaced.classify_path),
        });
    }
    let declared =
        |path: &str| crate::ast::functions::declared_test_path(path, declared_test_paths);
    if file.kind != crate::gitctx::ChangeKind::Added
        || !pack.is_test_path(&file.path)
        || declared(&file.path)
        || crate::ast::functions::test_by_construction(&file.path)
    {
        return None;
    }
    // The file's own directory when the name alone made it test code, else no directory.
    let name = crate::ast::functions::file_name(&file.path);
    let directory = &file.path[..file.path.len() - name.len()];
    let classify_path = crate::ast::extension(&file.path).and_then(|ext| {
        [
            format!("{directory}renamed.{ext}"),
            format!("renamed.{ext}"),
        ]
        .into_iter()
        .find(|candidate| {
            registry
                .find_pack(candidate)
                .is_some_and(|p| p.id() == pack.id())
                && !pack.is_test_path(candidate)
                && !declared(candidate)
        })
    });
    let note = match classify_path {
        Some(_) => format!(
            "`{}` is added in test scope by its name or directory only and {}; it is judged as production code (declare it under `[tests] paths` if it is test code)",
            file.path,
            holds_no_checking_test(tests)
        ),
        None => format!(
            "`{}` is added in test scope by its name or directory only and {}; no path of its extension is production code here, so it is still judged as test code",
            file.path,
            holds_no_checking_test(tests)
        ),
    };
    Some(TestScopeByNameOnly {
        classify_path,
        note,
    })
}

/// [`test_scope_by_name_only`] for a file whose parsed facts hold no test that checks.
pub(crate) fn name_only_scope(
    file: &crate::gitctx::ChangedFile,
    changed: &[crate::gitctx::ChangedFile],
    pack: &dyn crate::ast::LanguagePack,
    registry: &crate::ast::LanguageRegistry,
    declared_test_paths: &[String],
    facts: &crate::ast::ParsedFileFacts,
) -> Option<TestScopeByNameOnly> {
    (!holds_a_checking_test(facts))
        .then(|| {
            test_scope_by_name_only(
                file,
                changed,
                pack,
                registry,
                declared_test_paths,
                facts.tests.len(),
            )
        })
        .flatten()
}

/// The changed files a gate that reads `fact` analyses, each with its language pack
/// (`stub-bodies`, `error-swallowing`): not deleted, not exempt, and in a language that
/// has a pack. A file whose pack does not supply `fact` is listed in `unsupported`
/// instead, for [`unsupported_fact_note`].
pub(crate) fn ast_changes<'r>(
    changed: &[crate::gitctx::ChangedFile],
    exempt: &PathFilter,
    registry: &'r crate::ast::LanguageRegistry,
    fact: crate::ast::Fact,
    unsupported: &mut Vec<String>,
) -> Vec<(crate::gitctx::ChangedFile, &'r dyn crate::ast::LanguagePack)> {
    let mut analysed = Vec::new();
    for file in changed {
        if file.kind == crate::gitctx::ChangeKind::Deleted || exempt.matches(&file.path) {
            continue;
        }
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        if !pack.supplies(fact) {
            unsupported.push(file.path.clone());
            continue;
        }
        analysed.push((file.clone(), pack));
    }
    analysed
}

/// The note that names the files [`ast_changes`] left out because their pack supplies
/// no `what` facts (`function`, `handler`); `None` when it left none out.
pub(crate) fn unsupported_fact_note(unsupported: &[String], what: &str) -> Option<String> {
    if unsupported.is_empty() {
        return None;
    }
    let sample: Vec<&str> = unsupported.iter().take(3).map(String::as_str).collect();
    Some(format!(
        "{} changed file(s) are in a language whose pack supplies no {what} facts and were NOT analysed (e.g. {})",
        unsupported.len(),
        sample.join(", ")
    ))
}

/// Whether a file holds a test that checks something: at least one test the pack found
/// that `vacuous-tests` would not report ([`crate::ast::TestFacts::is_vacuous`]). One
/// empty test is not enough to keep a whole file test code.
pub fn holds_a_checking_test(facts: &crate::ast::ParsedFileFacts) -> bool {
    facts.tests.iter().any(|test| !test.is_vacuous())
}

impl GateOutcome {
    /// Enabled, examined nothing, found nothing, and says why: "not evaluated: ...".
    pub fn is_not_evaluated(&self) -> bool {
        self.enabled
            && self.examined == 0
            && self.violations.is_empty()
            && self.notes.iter().any(|n| n.starts_with("not evaluated"))
    }

    pub fn new(gate: &'static str) -> Self {
        let info = gate_info(gate).expect("gate id registered in config::GATES");
        Self {
            gate,
            suite: info.suite.label(),
            enabled: true,
            examined: 0,
            inline_exemptions: 0,
            baselined: 0,
            notes: Vec::new(),
            violations: Vec::new(),
            overrides: Vec::new(),
        }
    }

    /// `gate/code` for a kind this gate may report ([`crate::findings::full_code`]).
    pub fn code_of(&self, kind: &crate::findings::FindingKind) -> String {
        crate::findings::full_code(self.gate, kind)
    }

    /// Report a finding.
    pub fn push(
        &mut self,
        severity: Severity,
        kind: &crate::findings::FindingKind,
        file: Option<&str>,
        line: Option<usize>,
        message: String,
        remediation: &str,
    ) {
        assert!(
            kind.v1 != crate::findings::V1::Site,
            "`{}` is reported with `push_site`",
            kind.code
        );
        self.record(severity, kind, None, (file, line), message, remediation);
    }

    /// Record the override that lifted a finding, or report the finding ([`Self::push`])
    /// when no directive lifted it. `lifted` is the gate's own lookup for this finding
    /// ([`Context::find_override`]), made with the same `kind`.
    pub fn lift_or_push(
        &mut self,
        lifted: Option<crate::tokens::OverrideRecord>,
        severity: Severity,
        kind: &crate::findings::FindingKind,
        at: (Option<&str>, Option<usize>),
        message: String,
        remediation: &str,
    ) {
        match lifted {
            Some(record) => self.overrides.push(record),
            None => self.push(severity, kind, at.0, at.1, message, remediation),
        }
    }

    /// Anchor the finding just reported ([`Violation::anchor`]): what tells it apart from
    /// another finding of its code in the same file when it has no line.
    pub fn anchor_last(&mut self, anchor: impl Into<String>) {
        if let Some(v) = self.violations.last_mut() {
            v.anchor = Some(anchor.into());
        }
    }

    /// Report a finding whose version-1 title the site builds
    /// ([`crate::findings::V1::Site`]).
    pub fn push_site(
        &mut self,
        severity: Severity,
        kind: &crate::findings::FindingKind,
        legacy_title: String,
        at: (Option<&str>, Option<usize>),
        message: String,
        remediation: &str,
    ) {
        self.record(severity, kind, Some(legacy_title), at, message, remediation);
    }

    fn record(
        &mut self,
        severity: Severity,
        kind: &crate::findings::FindingKind,
        site_title: Option<String>,
        at: (Option<&str>, Option<usize>),
        message: String,
        remediation: &str,
    ) {
        use crate::findings::V1;
        let (file, line) = at;
        let legacy_title = match kind.v1 {
            V1::Same => None,
            V1::Was(t) => Some(t.to_string()),
            V1::Message => Some(message.clone()),
            V1::Site => site_title,
        };
        let code = self.code_of(kind);
        self.violations.push(Violation {
            gate: self.gate,
            code,
            fingerprint: String::new(),
            anchor: None,
            legacy_title,
            severity,
            title: kind.title.to_string(),
            file: file.map(str::to_string),
            line,
            message,
            remediation: Some(remediation.to_string()),
        });
    }

    /// Report a finding at a file and line (a kind whose version-1 title was its message).
    pub fn add_violation(
        &mut self,
        severity: Severity,
        kind: &crate::findings::FindingKind,
        file: impl AsRef<str>,
        line: usize,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) {
        let remediation = remediation.into();
        self.push(
            severity,
            kind,
            Some(file.as_ref()),
            Some(line),
            message.into(),
            &remediation,
        );
    }
}

#[derive(Debug, Serialize)]
pub struct CheckSummary {
    /// The report schema's version ([`crate::output_schema::REPORT_SCHEMA_VERSION`]). A field
    /// is added without changing it; one renamed, removed or retyped changes it.
    pub schema_version: u32,
    pub base: String,
    pub errors: usize,
    pub warnings: usize,
    pub notes: usize,
    pub overrides: usize,
    pub baselined: usize,
    pub outcomes: Vec<GateOutcome>,
    /// Gates the roadmap plans but this binary does not ship. Listed in every
    /// report so their absence is never mistaken for coverage.
    pub planned_gates: Vec<&'static str>,
    /// Run-level refusals that no single gate owns (an exhausted override budget,
    /// overrides awaiting approval). Any entry fails the run.
    /// Each is written as its sentence ([`crate::refusals::PolicyFailure`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub policy_failures: Vec<crate::refusals::PolicyFailure>,
    /// Where each hidden directive that was not read is written. Not in the JSON report,
    /// which carries the gate note naming the directive: this is what the SARIF, JUnit
    /// and GitLab reports build their entry from ([`crate::refusals::project`]).
    #[serde(skip)]
    pub refused_hidden_directives: Vec<crate::tokens::OverrideSource>,
    /// Deprecated configuration keys this run read, one note each. They never fail the run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deprecations: Vec<String>,
    /// Notes about the directive sources as a whole (which merged pull request a pushed
    /// commit came through, a forge lookup that could not be made), once per run. A note
    /// about one directive goes to its gate instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub directive_notes: Vec<String>,
    /// Directives this run read (PR body, commit bodies, merged pull-request bodies) that
    /// lifted no finding. Computed when every suite ran; empty under `--suite`. They
    /// never fail the run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unused_directives: Vec<crate::tokens::UnusedDirective>,
    /// Why the run could not check (exit 2): only then present, and `outcomes` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub could_not_check: Option<crate::could_not_check::CouldNotCheck>,
}

impl CheckSummary {
    pub fn is_success(&self, fail_on_warnings: bool, fail_on_overrides: bool) -> bool {
        self.errors == 0
            && self.policy_failures.is_empty()
            && (!fail_on_warnings || self.warnings == 0)
            && (!fail_on_overrides || self.total_overrides() == 0)
    }

    /// Overrides granted by a directive in the PR body or a commit body. Inline markers
    /// are reviewed as part of the tree and are not counted.
    pub fn directive_overrides(&self) -> usize {
        self.overrides()
            .filter(|o| !matches!(o.source, crate::tokens::OverrideSource::Inline { .. }))
            .count()
    }

    /// Overrides granted by an inline source marker (`// discipline:allow(...)`, `# discipline:allow(...)`).
    pub fn inline_overrides(&self) -> usize {
        self.overrides()
            .filter(|o| matches!(o.source, crate::tokens::OverrideSource::Inline { .. }))
            .count()
    }

    pub fn violations(&self) -> impl Iterator<Item = &Violation> {
        self.outcomes.iter().flat_map(|o| o.violations.iter())
    }

    pub fn overrides(&self) -> impl Iterator<Item = &crate::tokens::OverrideRecord> {
        self.outcomes.iter().flat_map(|o| o.overrides.iter())
    }

    pub fn total_overrides(&self) -> usize {
        self.outcomes.iter().map(|o| o.overrides.len()).sum()
    }

    /// Returns affirmative gate counts and examination tallies:
    /// `(passed_gates, failed_gates, disabled_gates, total_examined)`
    pub fn gate_counts(
        &self,
        fail_on_warnings: bool,
        fail_on_overrides: bool,
    ) -> (usize, usize, usize, usize) {
        let mut passed = 0;
        let mut failed = 0;
        let mut disabled = 0;
        let mut examined = 0;

        for o in &self.outcomes {
            if !o.enabled {
                disabled += 1;
            } else {
                examined += o.examined;
                let has_failure = o.violations.iter().any(|v| match v.severity {
                    Severity::Error => true,
                    Severity::Warning => fail_on_warnings,
                    Severity::Note => false,
                }) || (fail_on_overrides && !o.overrides.is_empty());

                if has_failure {
                    failed += 1;
                } else if !o.is_not_evaluated() {
                    passed += 1;
                }
            }
        }

        (passed, failed, disabled, examined)
    }

    /// Enabled gates that examined nothing because their input was absent (a named
    /// "not evaluated" note). Counted neither as passed nor as failed.
    pub fn not_evaluated_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.is_not_evaluated())
            .count()
    }
}

/// Everything a gate needs.
pub struct Context<'a> {
    pub config: &'a DisciplineConfig,
    /// The change's own configuration when the run is judged by the base ref's
    /// (`--policy-from base`); `None` when `config` already is the change's own.
    pub head_config: Option<&'a DisciplineConfig>,
    pub git: &'a GitCtx,
    /// Repo-relative path of the configuration file (for config-integrity).
    pub config_path: &'a str,
    /// Repo-relative path of the baseline file if grandfathering is active.
    pub baseline_path: Option<&'a str>,
    /// Loaded baseline if grandfathering is active.
    pub baseline: Option<&'a crate::baseline::DisciplineBaseline>,
    pub staged: bool,
    pub pr_title: Option<String>,
    pub pr_body: Option<String>,
    pub directives: Vec<crate::tokens::ParsedDirective>,
    pub directive_notes: Vec<String>,
    pub bench_provenance: Option<String>,
    pub allow_cross_host_bench: bool,
    pub bench_base_file: Option<std::path::PathBuf>,
    pub bench_head_file: Option<std::path::PathBuf>,
    pub test_base_report: Option<std::path::PathBuf>,
    pub test_head_report: Option<std::path::PathBuf>,
    pub test_report: Option<std::path::PathBuf>,
    /// The forge, for the gates that read facts from it (`issue-link` with
    /// `verify_references`). `None` where a run has no forge to ask (a baseline write).
    pub forge: Option<ForgeAccess<'a>>,
}

/// A gate's way to the forge: the API client, how the forge is identified, and the pull
/// request being checked when the run has one.
pub struct ForgeAccess<'a> {
    pub api: &'a dyn crate::forge::ForgeApi,
    pub identify: &'a dyn Fn() -> std::result::Result<crate::forge::Forge, String>,
    pub pull: Option<crate::override_policy::PullContext>,
}

impl Context<'_> {
    /// The pull request a gate that reads pull-request facts judges. `Ok(None)` for a
    /// run that has none to read (a local run, a push); in CI on any other event, a
    /// missing pull request is a configuration error (exit 2), never a pass.
    pub fn pull_request_for(&self, gate: &str) -> Result<Option<&ForgeAccess<'_>>> {
        let Some(access) = self.forge.as_ref().filter(|f| f.pull.is_some()) else {
            let in_ci = [
                "CI",
                "GITHUB_ACTIONS",
                "GITLAB_CI",
                "GITEA_ACTIONS",
                "FORGEJO_ACTIONS",
            ]
            .iter()
            .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty() && v != "false"));
            if in_ci && !crate::gitctx::is_push_event_environment() {
                return Err(crate::could_not_check::tag(
                    crate::could_not_check::Reason::Configuration,
                    anyhow::anyhow!(
                        "`gates.{gate}` needs the pull request being checked (the Actions event payload, or GitLab's CI_MERGE_REQUEST_IID), and this CI run has none"
                    ),
                ));
            }
            return Ok(None);
        };
        Ok(Some(access))
    }
}

/// Set by `discipline replay` on each case's `check`: the configuration under test is
/// newer than the replayed trees. Honoured only when the base is a commit replay built
/// ([`crate::gitctx::GitCtx::base_is_replay_base`]), so setting it in a CI job loosens
/// nothing.
pub const REPLAY_CASE_ENV: &str = "DISCIPLINE_REPLAY_CASE";

/// The end of the note a gate records when [`Context::predates_config`] skips part of
/// it: `<what> skipped: <why> (the configuration is newer)`. `discipline replay` counts
/// these notes per gate (`skipped_by_gate`), so every such note ends with this text.
pub const PREDATES_CONFIG_NOTE: &str = "(the configuration is newer)";

impl Context<'_> {
    /// The base side's configuration text: the file `--config` names when the base tree
    /// has it, else the base `discipline.toml`. A `--config` outside the repository is in
    /// no tree, so only the base `discipline.toml` is read.
    pub fn base_config_text(&self) -> Result<Option<String>> {
        let own = if crate::gitctx::config_in_tree(self.config_path) {
            self.git.base_content(self.config_path)?
        } else {
            None
        };
        Ok(match own {
            Some(s) => Some(s),
            None if self.config_path != "discipline.toml" => {
                self.git.base_content("discipline.toml")?
            }
            None => None,
        })
    }

    /// Under `discipline replay`, a file the configuration names that neither side of the
    /// replayed change has predates the configuration, so the part of a gate that reads it
    /// does not apply to that change. Outside replay (or when either side has the file) a
    /// missing file stays a configuration error.
    pub fn predates_config(&self, path: &str) -> Result<bool> {
        Ok(std::env::var_os(REPLAY_CASE_ENV).is_some()
            && self.git.base_is_replay_base()
            && self.git.base_content(path)?.is_none()
            && self.git.head_content(path)?.is_none())
    }

    /// [`crate::tokens::find_override`] over this run's directives: `lifts` is the finding
    /// the gate would report without the override.
    pub fn find_override(
        &self,
        gate: &str,
        lifts: &crate::findings::FindingKind,
        names: &[&str],
        subject: &str,
    ) -> Option<crate::tokens::OverrideRecord> {
        crate::tokens::find_override(&self.directives, gate, lifts, names, subject)
    }

    /// [`crate::tokens::find_whole_file_override`] over this run's directives: a directive
    /// that names `subject` (a path or a file name) and no line.
    pub fn find_whole_file_override(
        &self,
        gate: &str,
        lifts: &crate::findings::FindingKind,
        names: &[&str],
        subject: &str,
    ) -> Option<crate::tokens::OverrideRecord> {
        crate::tokens::find_whole_file_override(&self.directives, gate, lifts, names, subject)
    }

    /// A finding that an override directive could lift is only a warning in
    /// `--staged` mode without a PR body: a pre-commit hook runs before the
    /// commit message exists, so there is nowhere to put the directive yet.
    /// CI, which sees the message and the PR body, stays authoritative.
    pub fn overridable(&self, severity: Severity) -> Severity {
        if self.staged && self.pr_body.is_none() {
            Severity::Warning
        } else {
            severity
        }
    }
}

/// The suites a `--suite` choice runs.
fn wanted_suites(suite: SuiteChoice) -> Vec<Suite> {
    match suite {
        SuiteChoice::All => vec![
            Suite::AgentGuard,
            Suite::Hygiene,
            Suite::Integrity,
            Suite::Quality,
            Suite::Verification,
            Suite::Bench,
        ],
        SuiteChoice::AgentGuard => vec![Suite::AgentGuard],
        SuiteChoice::Hygiene => vec![Suite::Hygiene],
        SuiteChoice::Integrity => vec![Suite::Integrity],
        SuiteChoice::Quality => vec![Suite::Quality],
        SuiteChoice::Verification => vec![Suite::Verification],
        SuiteChoice::Bench => vec![Suite::Bench],
    }
}

/// Ids of the gates a `--suite` choice runs: the ones whose configured values are checked
/// before any gate runs.
fn selected_gate_ids(suite: SuiteChoice) -> Vec<&'static str> {
    let wanted = wanted_suites(suite);
    GATES
        .iter()
        .filter(|g| g.available && wanted.contains(&g.suite))
        .map(|g| g.id)
        .collect()
}

pub fn run_checks(
    config: &DisciplineConfig,
    suite: SuiteChoice,
    ctx: &Context,
) -> Result<CheckSummary> {
    let wanted = wanted_suites(suite);

    let selected: Vec<_> = GATES
        .iter()
        .filter(|g| g.available && wanted.contains(&g.suite))
        .collect();
    if selected.is_empty() {
        // Asking for a suite with nothing in it must not print "PASSED".
        bail!(
            "suite `{}` has no gates available in this version of discipline \
             (its gates are planned); nothing was checked",
            wanted[0].label()
        );
    }

    if ctx.git.tracked_files()?.is_empty() {
        bail!("git tracks no files here; refusing to report a pass over an empty tree");
    }

    // Compile every configured glob before any gate runs: a gate that returns early would
    // not, and the change's own copy is not otherwise read when the policy comes from the
    // base side.
    let selected_ids: Vec<&'static str> = selected.iter().map(|g| g.id).collect();
    for checked in std::iter::once(config).chain(ctx.head_config) {
        check_configured_globs(checked, &selected_ids)?;
        check_configured_patterns(checked, &selected_ids)?;
        check_configured_pairs(checked, &selected_ids)?;
    }

    let whole_tree = ctx.git.is_whole_tree();

    // Misses recorded by an earlier run on this thread are not this run's.
    crate::tokens::forget_subject_misses();
    let mut outcomes = Vec::new();
    let mut ast_outcomes = None;
    for gate in selected {
        let settings = config
            .gates
            .settings(gate.id)
            .ok_or_else(|| anyhow!("gate `{}` has no settings entry", gate.id))?;
        // The gate that guards the configuration is switched by the base side (F9).
        let held_on = gate.id == "config-integrity"
            && !settings.enabled()
            && integrity::enabled_on_base(ctx)?;
        if !settings.enabled() && !held_on {
            let mut o = GateOutcome::new(gate.id);
            o.enabled = false;
            outcomes.push(o);
            continue;
        }
        if whole_tree && gate.delta_only {
            let mut o = GateOutcome::new(gate.id);
            o.notes.push(
                "not evaluated: this rule describes a change, and a whole-tree baseline has no change to describe"
                    .to_string(),
            );
            outcomes.push(o);
            continue;
        }
        let outcome = match gate.id {
            "agents-md" => hygiene::agents_md(ctx),
            "time-estimates" => hygiene::time_estimates(ctx),
            "pii" => hygiene::pii(ctx),
            "agent-scratch" => hygiene::agent_scratch(ctx),
            "shell-secrets" => shell_secrets::evaluate_shell_secrets(ctx),
            "issue-link" => issue_link::evaluate_issue_link(ctx),
            "ratified-paths" => ratified_paths::evaluate_ratified_paths(ctx),
            "review-threads" => review_threads::evaluate_review_threads(ctx),
            "commit-provenance" => commit_provenance::commit_provenance(ctx),
            "citation-metadata" => citation_metadata::citation_metadata(ctx),
            "config-integrity" => integrity::config_integrity(ctx),
            "toolchain-config" => toolchain_config::toolchain_config(ctx),
            "sandbox-config" => sandbox_config::sandbox_config(ctx),
            "stub-bodies" => stub_bodies::stub_bodies(ctx),
            "harness-tampering" => harness_tampering::harness_tampering(ctx),
            "error-swallowing" => error_swallowing::error_swallowing(ctx),
            "instruction-smuggling" => instruction_smuggling::instruction_smuggling(ctx),
            "build-hooks" => build_hooks::build_hooks(ctx),
            "golden-output" => integrity::golden_output(ctx),
            "bench-regression" => perf::bench_regression(ctx),
            "command" => command::evaluate_command(ctx),
            "dependency-delta" => dependency::evaluate_dependency_delta(ctx),
            "test-budget" => test_budget::evaluate_test_budget(ctx),
            "test-floor" => test_floor::evaluate_test_floor(ctx),
            "ci-integrity" => ci_integrity::evaluate_ci_integrity(ctx),
            "ci-skip-set" => ci_skip_set::evaluate_ci_skip_set(ctx),
            "provenance-tags" => provenance_tags::evaluate_provenance_tags(ctx),
            "archive-contents" => archive_contents::evaluate_archive_contents(ctx),
            "manifest-sync" => manifest_sync::evaluate_manifest_sync(ctx),
            "version-lockstep" => version_lockstep::evaluate_version_lockstep(ctx),
            "scope-confinement" => scope_confinement::evaluate_scope_confinement(ctx),
            "suppression-delta" => suppression_delta::evaluate_suppression_delta(ctx),
            "pr-checklist" => pr_checklist::evaluate_pr_checklist(ctx),
            "unsafe-budget" => unsafe_budget::evaluate_unsafe_budget(ctx),
            "msrv" => msrv::evaluate_msrv(ctx),
            "miri" => miri::evaluate_miri(ctx),
            "sanitizers" => sanitizers::evaluate_sanitizers(ctx),
            "assertion-reduction"
            | "vacuous-tests"
            | "ignored-tests"
            | "unsafe-safety-comment"
            | "deletion-rationale" => {
                if ast_outcomes.is_none() {
                    ast_outcomes = Some(
                        agent_diff::run(ctx)
                            .with_context(|| format!("gate `{}` could not run", gate.id))
                            .map_err(|e| crate::could_not_check::tag_gate(gate.id, e))?,
                    );
                }
                let all: &Vec<GateOutcome> = ast_outcomes.as_ref().expect("just set");
                Ok(all
                    .iter()
                    .find(|o| o.gate == gate.id)
                    .cloned()
                    .expect("agent_diff returns every agent-guard diff gate"))
            }
            other => bail!("gate `{other}` is marked available but has no implementation"),
        }
        .with_context(|| format!("gate `{}` could not run", gate.id))
        .map_err(|e| crate::could_not_check::tag_gate(gate.id, e))?;
        outcomes.push(outcome);
    }

    // A submodule pointer change is skipped by every gate (its content is another
    // repository). Say so where a deletion or an out-of-scope change would be judged.
    let submodules = ctx.git.changed_submodules()?;
    if !submodules.is_empty() {
        let note = format!(
            "not inspected: submodule pointer change(s) at {} (review the submodule's own history)",
            submodules.join(", ")
        );
        for o in outcomes
            .iter_mut()
            .filter(|o| o.enabled && matches!(o.gate, "deletion-rationale" | "scope-confinement"))
        {
            o.notes.push(note.clone());
        }
    }

    let mut run_directive_notes: Vec<String> = Vec::new();
    for note in &ctx.directive_notes {
        // Where a pushed commit came from: about the run's sources, not any gate's
        // directives, so it is reported once for the run. Decided before the words in the
        // note are read for a gate: it carries an author login and can carry a forge's
        // refusal, and neither names a directive (#568).
        if crate::tokens::is_merged_source_note(note) {
            if !run_directive_notes.contains(note) {
                run_directive_notes.push(note.clone());
            }
            continue;
        }
        // A refused hidden directive's note names the directive and nothing it says (#362):
        // it belongs to that directive's gate alone.
        let hidden_gate = note
            .strip_prefix("hidden directive `")
            .and_then(|rest| rest.split_once('`'))
            .and_then(|(name, _)| crate::tokens::spec_for_directive(name))
            .map(|spec| spec.gate);
        let target_gate = if let Some(gate) = hidden_gate {
            gate
        } else if note.contains("removes") || note.contains("deletes") {
            "deletion-rationale"
        } else if note.contains("allow-assertion-drop") {
            "assertion-reduction"
        } else if note.contains("allow-ignore") {
            "ignored-tests"
        } else if note.contains("allow-gate-weakening") {
            "config-integrity"
        } else if note.contains("allow-golden-update") {
            "golden-output"
        } else if note.contains("allow-regression") {
            "bench-regression"
        } else if note.contains("allow-command") {
            "command"
        } else if note.contains("allow-dependency") {
            "dependency-delta"
        } else if note.contains("allow-test-shrink") || note.contains("allow-floor-drop") {
            "test-floor"
        } else if note.contains("allow-test-budget") {
            "test-budget"
        } else if note.contains("allow-ci-weakening")
            || note.contains("allow-unpinned-action")
            || note.contains("allow-ci-change")
        {
            "ci-integrity"
        } else if note.contains("allow-archive-leak") {
            "archive-contents"
        } else if note.contains("allow-manifest-drift") {
            "manifest-sync"
        } else if note.contains("allow-version-mismatch") {
            "version-lockstep"
        } else if note.contains("allow-scope") {
            "scope-confinement"
        } else if note.contains("allow-build-hook") {
            "build-hooks"
        } else if note.contains("allow-commit-provenance") {
            "commit-provenance"
        } else if note.contains("allow-citation-metadata") {
            "citation-metadata"
        } else if note.contains("allow-agent-instructions") {
            "instruction-smuggling"
        } else if note.contains("allow-swallow") {
            "error-swallowing"
        } else if note.contains("allow-stub") {
            "stub-bodies"
        } else if note.contains("allow-harness-tampering") {
            "harness-tampering"
        } else if note.contains("allow-toolchain-weakening") {
            "toolchain-config"
        } else if note.contains("allow-sandbox-widening") {
            "sandbox-config"
        } else if note.contains("allow-suppression") {
            "suppression-delta"
        } else if note.contains("allow-checklist") {
            "pr-checklist"
        } else if note.contains("allow-unsafe") {
            "unsafe-budget"
        } else if note.contains("allow-msrv") {
            "msrv"
        } else if note.contains("allow-miri") {
            "miri"
        } else if note.contains("allow-sanitizers") {
            "sanitizers"
        } else if note.contains("allow-vacuous-test") {
            "vacuous-tests"
        } else if note.contains("allow-nul") || note.contains("allow-corrupt") {
            "assertion-reduction"
        } else {
            ""
        };
        for o in &mut outcomes {
            if target_gate.is_empty() {
                if matches!(
                    o.gate,
                    "deletion-rationale"
                        | "assertion-reduction"
                        | "ignored-tests"
                        | "config-integrity"
                        | "toolchain-config"
                        | "sandbox-config"
                        | "stub-bodies"
                        | "harness-tampering"
                        | "error-swallowing"
                        | "instruction-smuggling"
                        | "commit-provenance"
                        | "citation-metadata"
                        | "build-hooks"
                        | "golden-output"
                        | "bench-regression"
                        | "command"
                        | "dependency-delta"
                        | "test-budget"
                        | "test-floor"
                        | "ci-integrity"
                        | "archive-contents"
                        | "manifest-sync"
                        | "version-lockstep"
                        | "scope-confinement"
                        | "suppression-delta"
                        | "pr-checklist"
                        | "unsafe-budget"
                        | "msrv"
                        | "miri"
                        | "sanitizers"
                ) {
                    o.notes.push(note.clone());
                }
            } else if o.gate == target_gate {
                o.notes.push(note.clone());
            }
        }
    }

    // A directive that names another subject and only mentions this one further on does
    // not cover it (`tokens::find_override`). The gate that still reports a finding says
    // which subject the directive was read with, unless another directive lifted the one
    // it mentions.
    // A gate may try a path and then its file name: one note for the two.
    let mut noted: Vec<crate::tokens::SubjectMiss> = Vec::new();
    for miss in crate::tokens::take_subject_misses() {
        let same_finding = noted.iter().any(|n| {
            n.gate == miss.gate
                && n.directive == miss.directive
                && n.source == miss.source
                && n.subject.ends_with(&format!("/{}", miss.subject))
        });
        if same_finding {
            continue;
        }
        if let Some(o) = outcomes.iter_mut().find(|o| o.gate == miss.gate) {
            let lifted = o.overrides.iter().any(|ov| ov.subject == miss.subject);
            // A gate tries several subjects for one finding (a test's name, then its
            // file): the note is for a finding the gate still reports, one that names
            // the subject.
            let reported = o.violations.iter().any(|v| {
                v.message.contains(&miss.subject)
                    || v.file.as_deref().is_some_and(|f| {
                        f == miss.subject || f.ends_with(&format!("/{}", miss.subject))
                    })
            });
            if reported && !lifted {
                o.notes.push(miss.note());
                noted.push(miss);
            }
        }
    }

    // Deduplicate violations per gate by (file, line, title, message) preserving discovery order
    for o in &mut outcomes {
        let mut seen = std::collections::HashSet::new();
        o.violations.retain(|v| {
            let key = (v.file.clone(), v.line, v.title.clone(), v.message.clone());
            seen.insert(key)
        });
    }

    {
        let mut all: Vec<&mut Violation> = outcomes
            .iter_mut()
            .flat_map(|o| o.violations.iter_mut())
            .collect();
        let reads = crate::gitctx::ReadRecorder::new();
        let head = reads.head(ctx.git);
        // A finding on the pull request body is on a line of the body, not of a file:
        // read there, its fingerprint hashes that line like any other finding's. The
        // body comes first, so a tracked file of the same name never stands in for it.
        crate::baseline::fill_fingerprints(&mut all, |path: &str| {
            match (is_pr_body_label(path), &ctx.pr_body) {
                (true, Some(body)) => Some(body.clone()),
                _ => head(path),
            }
        });
        reads.finish()?;
    }

    // Grandfathered findings baseline matching
    let mut deprecations = ctx.config.deprecations.clone();
    if let Some(baseline) = ctx.baseline {
        crate::baseline::apply_baseline_with_git(ctx.git, baseline, &mut outcomes)?;
        if baseline.version < crate::baseline::FINGERPRINT_VERSION {
            deprecations.push(format!(
                "`{}` uses fingerprint version {}, which keys on finding titles: run `discipline baseline --migrate` in a change of its own to rewrite it to version {}",
                ctx.baseline_path.unwrap_or(crate::baseline::DEFAULT_BASELINE_FILE),
                baseline.version,
                crate::baseline::FINGERPRINT_VERSION
            ));
        }
    }

    let count = |s: Severity| {
        outcomes
            .iter()
            .flat_map(|o| &o.violations)
            .filter(|v| v.severity == s)
            .count()
    };
    let total_overrides = outcomes.iter().map(|o| o.overrides.len()).sum();
    let total_baselined = outcomes.iter().map(|o| o.baselined).sum();
    Ok(CheckSummary {
        schema_version: crate::output_schema::REPORT_SCHEMA_VERSION,
        could_not_check: None,
        base: ctx.git.base_label().to_string(),
        errors: count(Severity::Error),
        warnings: count(Severity::Warning),
        notes: count(Severity::Note),
        overrides: total_overrides,
        baselined: total_baselined,
        planned_gates: GATES
            .iter()
            .filter(|g| !g.available)
            .map(|g| g.id)
            .collect(),
        unused_directives: if matches!(suite, SuiteChoice::All) {
            crate::tokens::unused_directives(
                &ctx.directives,
                outcomes.iter().flat_map(|o| o.overrides.iter()),
            )
        } else {
            Vec::new()
        },
        outcomes,
        policy_failures: Vec::new(),
        refused_hidden_directives: Vec::new(),
        deprecations,
        directive_notes: run_directive_notes,
    })
}

/// Glob set over repo-relative, `/`-separated paths.
pub struct PathFilter(GlobSet);

impl PathFilter {
    pub fn new(globs: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for g in globs {
            b.add(Glob::new(g).with_context(|| format!("invalid glob `{g}` in configuration"))?);
        }
        Ok(Self(b.build()?))
    }

    pub fn matches(&self, path: &str) -> bool {
        self.0.is_match(path)
    }
}

/// Configuration keys whose value is a list of globs, wherever they sit in a gate's table
/// (an entry of `rules`, the `scratch` table) or under `[tests]`.
const GLOB_LIST_KEYS: &[&str] = &[
    "exempt_paths",
    "paths",
    "include",
    "workflows",
    "manifests",
    "allowed_paths",
    "forbidden_paths",
    "protected_paths",
    "never_ratifiable",
    "watched_paths",
    "exclude_paths",
    "corpus_dirs",
    "fuzz_targets",
    "instruction_files",
    "superseded_json_paths",
    "record_paths",
    "required_paths",
    "constant_fallback_paths",
];

/// One step of the way to a configured value: a table key or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

fn key_path(keys: &[&str]) -> Vec<Seg> {
    keys.iter().map(|k| Seg::Key(k.to_string())).collect()
}

/// A configured value that fails the check made before any gate runs.
struct UnusableValue {
    /// The gate whose table holds it; `None` for `[tests]`.
    gate: Option<&'static str>,
    /// Where it sits in the serialised configuration.
    path: Vec<Seg>,
    /// The key as a message names it.
    key: String,
    /// The error the run stops with.
    error: anyhow::Error,
}

/// Every glob list under `value` holding a glob that does not compile: where the list
/// sits, its dotted key, and its first such glob. An entry of an array of tables is named
/// by the array's key, as before; its index is in the path only.
fn invalid_globs(
    value: &toml::Value,
    at: &str,
    path: &mut Vec<Seg>,
    out: &mut Vec<(Vec<Seg>, String, String, globset::Error)>,
) {
    match value {
        toml::Value::Table(table) => {
            for (key, v) in table {
                let here = format!("{at}.{key}");
                path.push(Seg::Key(key.clone()));
                let listed = GLOB_LIST_KEYS.contains(&key.as_str());
                match v.as_array().filter(|_| listed) {
                    Some(globs) => {
                        let bad = globs
                            .iter()
                            .filter_map(toml::Value::as_str)
                            .find_map(|g| Glob::new(g).err().map(|e| (g.to_string(), e)));
                        if let Some((glob, e)) = bad {
                            out.push((path.clone(), here, glob, e));
                        }
                    }
                    None => invalid_globs(v, &here, path, out),
                }
                path.pop();
            }
        }
        toml::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                invalid_globs(v, at, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Every glob list that does not compile in `[tests]` or in the table of an enabled gate
/// among `gate_ids`, in the order the run reports them.
fn unusable_globs(
    config: &DisciplineConfig,
    gate_ids: &[&'static str],
) -> Result<Vec<UnusableValue>> {
    let value = toml::Value::try_from(config)?;
    let message = |glob: &str, key: &str, e: &globset::Error| {
        anyhow!("invalid glob `{glob}` in configuration (`{key}`): {e}")
    };
    let mut out = Vec::new();
    let mut found = Vec::new();
    if let Some(tests) = value.get("tests") {
        invalid_globs(tests, "tests", &mut key_path(&["tests"]), &mut found);
    }
    for (path, key, glob, e) in found.drain(..) {
        let error = crate::could_not_check::tag(
            crate::could_not_check::Reason::Configuration,
            message(&glob, &key, &e),
        );
        out.push(UnusableValue {
            gate: None,
            path,
            key,
            error,
        });
    }
    for id in gate_ids {
        if !config.gates.settings(id).is_some_and(|s| s.enabled()) {
            continue;
        }
        if let Some(gate) = value.get("gates").and_then(|gates| gates.get(*id)) {
            let at = format!("gates.{id}");
            invalid_globs(gate, &at, &mut key_path(&["gates", id]), &mut found);
        }
        for (path, key, glob, e) in found.drain(..) {
            let error = crate::could_not_check::tag_gate(id, message(&glob, &key, &e));
            out.push(UnusableValue {
                gate: Some(id),
                path,
                key,
                error,
            });
        }
    }
    Ok(out)
}

/// Fails on the first glob that does not compile in `[tests] paths` or in the table of an
/// enabled gate among `gate_ids`. A glob in a gate's table is that gate's failure, as it
/// is when the gate compiles it; `[tests] paths` belongs to no gate.
pub fn check_configured_globs(config: &DisciplineConfig, gate_ids: &[&'static str]) -> Result<()> {
    match unusable_globs(config, gate_ids)?.into_iter().next() {
        Some(first) => Err(first.error),
        None => Ok(()),
    }
}

/// Compiles a configured regular expression whose first capture group is read. A pattern
/// that does not compile, or has no group to read, is a configuration error naming `key`:
/// it could only ever extract nothing.
pub fn capture_pattern(pattern: &str, key: &str) -> Result<regex::Regex> {
    let config_error = |msg: String| {
        crate::could_not_check::tag(crate::could_not_check::Reason::Configuration, anyhow!(msg))
    };
    let re = regex::Regex::new(pattern).map_err(|e| {
        config_error(format!(
            "`{key}` pattern `{pattern}` is not a valid regular expression: {e}"
        ))
    })?;
    // Group 0 is the whole match.
    if re.captures_len() < 2 {
        return Err(config_error(format!(
            "`{key}` pattern `{pattern}` has no capture group; the value is read from its first group, for example `(\\d+)`"
        )));
    }
    Ok(re)
}

/// How a configured pattern is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternForm {
    /// A regular expression.
    Regex,
    /// One of the forms of `ratio_satisfied_by` (`interval`, `marker:<word>`,
    /// `artifact:<glob>`, `regex:<pattern>`).
    RatioEvidence,
}

/// Configuration keys whose value is a regular expression or a list of them, wherever
/// they sit in a gate's table (an entry of `rules`, of `groups`, of `commands`). The
/// gate that reads one compiles it when it gets that far; [`unusable_patterns`] compiles
/// every one before any gate runs. The patterns of `command` and `ci-integrity` that
/// carry a rule of their own (a capture group, a message of their own) are checked by
/// name there and are not listed here.
const PATTERN_KEYS: &[(&str, PatternForm)] = &[
    ("allow_patterns", PatternForm::Regex),
    ("extra_patterns", PatternForm::Regex),
    ("extra_secret_patterns", PatternForm::Regex),
    ("forbidden_patterns", PatternForm::Regex),
    ("snapshot_ignore", PatternForm::Regex),
    ("pattern", PatternForm::Regex),
    ("extract_regex", PatternForm::Regex),
    ("regex", PatternForm::Regex),
    ("ratio_satisfied_by", PatternForm::RatioEvidence),
];

/// Compiles one configured value of a [`PATTERN_KEYS`] key. A value that does not compile
/// is a configuration error naming `key`.
fn check_configured_pattern(pattern: &str, key: &str, form: PatternForm) -> Result<()> {
    let checked = match form {
        PatternForm::Regex => regex::Regex::new(pattern)
            .map(|_| ())
            .map_err(|e| anyhow!("`{key}` pattern `{pattern}` is not a valid regex: {e}")),
        PatternForm::RatioEvidence => {
            provenance_tags::interval_evidence_regex(&[pattern.to_string()])
                .map(|_| ())
                .map_err(|e| anyhow!("`{key}` entry does not compile: {e:#}"))
        }
    };
    checked
        .map_err(|e| crate::could_not_check::tag(crate::could_not_check::Reason::Configuration, e))
}

/// Every [`PATTERN_KEYS`] key under `value` holding a pattern that does not compile:
/// where the key sits, its dotted name, and the error of its first such pattern. An entry
/// of an array of tables is named by its `name` when it has one (`commands[api]`), else by
/// its index (`rules[0]`).
fn invalid_patterns(
    value: &toml::Value,
    at: &str,
    path: &mut Vec<Seg>,
    out: &mut Vec<(Vec<Seg>, String, anyhow::Error)>,
) {
    match value {
        toml::Value::Table(table) => {
            for (key, v) in table {
                let here = format!("{at}.{key}");
                path.push(Seg::Key(key.clone()));
                let form = PATTERN_KEYS
                    .iter()
                    .find(|(k, _)| *k == key.as_str())
                    .map(|(_, form)| *form);
                let patterns: Option<Vec<&str>> = match (form, v) {
                    (Some(_), toml::Value::String(one)) => Some(vec![one.as_str()]),
                    (Some(_), toml::Value::Array(items)) => {
                        Some(items.iter().filter_map(toml::Value::as_str).collect())
                    }
                    _ => None,
                };
                match (form, patterns) {
                    (Some(form), Some(patterns)) => {
                        let bad = patterns
                            .iter()
                            .find_map(|p| check_configured_pattern(p, &here, form).err());
                        if let Some(e) = bad {
                            out.push((path.clone(), here, e));
                        }
                    }
                    _ => invalid_patterns(v, &here, path, out),
                }
                path.pop();
            }
        }
        toml::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let entry = match v.get("name").and_then(toml::Value::as_str) {
                    Some(name) => format!("{at}[{name}]"),
                    None => format!("{at}[{i}]"),
                };
                path.push(Seg::Index(i));
                invalid_patterns(v, &entry, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Every configured value, other than a glob list, that an enabled gate among `gate_ids`
/// could not use, in the order the run reports them: the capture patterns of
/// `ci-integrity` and `command`, the output patterns of `command`, the shapes of the
/// values `miri` and `sanitizers` place in a command, a `sanitizers` canary under a
/// sanitizer that cannot produce its diagnostic, the globs among `bench-regression`'s
/// `exempt_arms` (a list of names and globs, so not one of [`GLOB_LIST_KEYS`]), and then
/// every other configured regular expression ([`PATTERN_KEYS`]) of every gate. Each error
/// is the gate's, with the reason `configuration`.
fn unusable_patterns(
    config: &DisciplineConfig,
    gate_ids: &[&'static str],
) -> Result<Vec<UnusableValue>> {
    let on =
        |id: &str| gate_ids.contains(&id) && config.gates.settings(id).is_some_and(|s| s.enabled());
    let mut out = Vec::new();
    let mut push = |gate: &'static str, path: Vec<Seg>, key: String, e: anyhow::Error| {
        out.push(UnusableValue {
            gate: Some(gate),
            path,
            key,
            error: crate::could_not_check::tag_gate(gate, e),
        });
    };
    if on(ci_integrity::GATE) {
        let gate = ci_integrity::GATE;
        let settings = &config.gates.ci_integrity;
        let at = |key: &str| key_path(&["gates", gate, key]);
        if let Some(p) = &settings.documented_job_count_pattern {
            if let Err(e) = capture_pattern(p, ci_integrity::JOB_COUNT_PATTERN_KEY) {
                push(
                    gate,
                    at("documented_job_count_pattern"),
                    ci_integrity::JOB_COUNT_PATTERN_KEY.to_string(),
                    e,
                );
            }
        }
    }
    if on(command::GATE) {
        let gate = command::GATE;
        let settings = &config.gates.command;
        let at = |entry: Option<usize>, key: &str| {
            let mut path = key_path(&["gates", gate]);
            if let Some(i) = entry {
                path.push(Seg::Key("commands".to_string()));
                path.push(Seg::Index(i));
            }
            path.push(Seg::Key(key.to_string()));
            path
        };
        let tables = || {
            std::iter::once((None, None)).chain(
                settings
                    .commands
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (Some(i), Some(c))),
            )
        };
        for (index, entry) in tables() {
            let name = entry.map(|c| c.name.as_str());
            let pattern = entry.map_or(&settings.count_pattern, |c| &c.count_pattern);
            let key = command::count_pattern_key(name);
            if let Some(Err(e)) = pattern.as_ref().map(|p| capture_pattern(p, &key)) {
                push(gate, at(index, "count_pattern"), key, e);
            }
        }
        for (index, entry) in tables() {
            let name = entry.map(|c| c.name.as_str());
            let forbid = entry.map_or(&settings.forbid_output, |c| &c.forbid_output);
            let single = [
                (
                    "zero_items_pattern",
                    entry.map_or(&settings.zero_items_pattern, |c| &c.zero_items_pattern),
                ),
                (
                    "canary_expected_diagnostic",
                    entry.map_or(&settings.canary_expected_diagnostic, |c| {
                        &c.canary_expected_diagnostic
                    }),
                ),
            ];
            let listed = forbid.iter().map(|p| ("forbid_output", p));
            let mut reported = Vec::new();
            for (key, pattern) in listed.chain(
                single
                    .iter()
                    .filter_map(|(k, p)| p.as_ref().map(|p| (*k, p))),
            ) {
                // One entry per key: a repair replaces the whole value.
                if reported.contains(&key) {
                    continue;
                }
                let dotted = command::entry_key(name, key);
                if let Err(e) = command::output_pattern(pattern, &dotted) {
                    reported.push(key);
                    push(gate, at(index, key), dotted, e);
                }
            }
        }
    }
    // Values placed among the arguments of a built-in command: each has one shape.
    if on(miri::GATE) {
        let args = &config.gates.miri.args;
        if let Some(e) = args.iter().find_map(|a| miri::check_arg(a).err()) {
            push(
                miri::GATE,
                key_path(&["gates", miri::GATE, "args"]),
                miri::ARGS_KEY.to_string(),
                e,
            );
        }
    }
    if on(sanitizers::GATE) {
        if let Err(e) = sanitizers::check_sanitizer_name(&config.gates.sanitizers.sanitizer) {
            push(
                sanitizers::GATE,
                key_path(&["gates", sanitizers::GATE, "sanitizer"]),
                sanitizers::SANITIZER_KEY.to_string(),
                e,
            );
        } else if let Err(e) = sanitizers::check_canary_sanitizer(&config.gates.sanitizers) {
            push(
                sanitizers::GATE,
                key_path(&["gates", sanitizers::GATE, "canary"]),
                sanitizers::CANARY_KEY.to_string(),
                e,
            );
        }
    }
    if on(perf::GATE) {
        if let Err(e) = perf::ArmExemptions::new(&config.gates.bench_regression.exempt_arms) {
            push(
                perf::GATE,
                key_path(&["gates", perf::GATE, "exempt_arms"]),
                format!("gates.{}.exempt_arms", perf::GATE),
                crate::could_not_check::tag(crate::could_not_check::Reason::Configuration, e),
            );
        }
    }
    // Every other configured regular expression, of every enabled gate.
    let value = toml::Value::try_from(config)?;
    let mut found = Vec::new();
    for id in gate_ids {
        if !on(id) {
            continue;
        }
        if let Some(gate) = value.get("gates").and_then(|gates| gates.get(*id)) {
            let at = format!("gates.{id}");
            invalid_patterns(gate, &at, &mut key_path(&["gates", id]), &mut found);
        }
        for (path, key, e) in found.drain(..) {
            push(id, path, key, e);
        }
    }
    Ok(out)
}

/// Settings of an enabled gate among `gate_ids` that only work together and are set apart:
/// a documented job count with one of its two keys. Both keys are named, and both are
/// what a repair replaces.
fn unusable_pairs(config: &DisciplineConfig, gate_ids: &[&'static str]) -> Vec<UnusableValue> {
    let gate = ci_integrity::GATE;
    let settings = &config.gates.ci_integrity;
    let mut out = Vec::new();
    if !gate_ids.contains(&gate) || !settings.enabled {
        return out;
    }
    for (key, dotted) in [
        (
            "documented_job_count_path",
            ci_integrity::JOB_COUNT_PATH_KEY,
        ),
        (
            "documented_job_count_pattern",
            ci_integrity::JOB_COUNT_PATTERN_KEY,
        ),
    ] {
        if let Some(e) = ci_integrity::half_configured_job_count(settings) {
            out.push(UnusableValue {
                gate: Some(gate),
                path: key_path(&["gates", gate, key]),
                key: dotted.to_string(),
                error: crate::could_not_check::tag_gate(gate, e),
            });
        }
    }
    out
}

/// Fails on a setting [`unusable_pairs`] finds: one that, set without its partner, would
/// do nothing and say nothing.
pub fn check_configured_pairs(config: &DisciplineConfig, gate_ids: &[&'static str]) -> Result<()> {
    match unusable_pairs(config, gate_ids).into_iter().next() {
        Some(first) => Err(first.error),
        None => Ok(()),
    }
}

/// Fails on the first configured value [`unusable_patterns`] finds.
pub fn check_configured_patterns(
    config: &DisciplineConfig,
    gate_ids: &[&'static str],
) -> Result<()> {
    match unusable_patterns(config, gate_ids)?.into_iter().next() {
        Some(first) => Err(first.error),
        None => Ok(()),
    }
}

/// A key of the base ref's configuration that this run reads from the change's copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadValue {
    /// The gate whose table holds the key; `None` for `[tests]`.
    pub gate: Option<&'static str>,
    /// What the report says about it.
    pub note: String,
}

fn value_at<'v>(value: &'v toml::Value, path: &[Seg]) -> Option<&'v toml::Value> {
    path.iter().try_fold(value, |v, seg| match seg {
        Seg::Key(k) => v.get(k.as_str()),
        Seg::Index(i) => v.get(*i),
    })
}

/// Puts `new` at `path` in `value`, or removes the key there when `new` is `None`. `false`
/// when the path does not lead to a table key that can hold it.
fn replace_at(value: &mut toml::Value, path: &[Seg], new: Option<toml::Value>) -> bool {
    let Some((Seg::Key(last), parents)) = path.split_last() else {
        return false;
    };
    let mut at = value;
    for seg in parents {
        let next = match seg {
            Seg::Key(k) => at.get_mut(k.as_str()),
            Seg::Index(i) => at.get_mut(*i),
        };
        let Some(next) = next else {
            return false;
        };
        at = next;
    }
    let Some(table) = at.as_table_mut() else {
        return false;
    };
    match new {
        Some(v) => {
            table.insert(last.clone(), v);
        }
        None => {
            table.remove(last);
        }
    }
    true
}

/// `--policy-from base` with a base configuration whose only fault is a glob or pattern
/// that does not compile: the configuration this run is judged by, when the change repairs
/// it (#600).
///
/// The base copy is the one in force, so without this the change that repairs it stops
/// like every other (exit 2) and the repair can never be checked. When the change's own
/// copy passes the up-front check, each key that fails it in the base copy is read from
/// the change's copy, and nothing else is: the result is the base copy with those keys
/// replaced. `None`, which leaves the run to stop on the base copy's error, when the base
/// copy has nothing to repair, when the change's copy fails the check too, when a key
/// cannot be replaced (the change has no table at that place), or when the result still
/// fails the check (the change left the value as it was, or switched the gate off
/// instead of repairing it).
pub fn base_policy_repaired_by_head(
    base: &DisciplineConfig,
    head: &DisciplineConfig,
    suite: SuiteChoice,
) -> Result<Option<(DisciplineConfig, Vec<HeadValue>)>> {
    let ids = selected_gate_ids(suite);
    let unusable = |config: &DisciplineConfig| -> Result<Vec<UnusableValue>> {
        let mut found = unusable_globs(config, &ids)?;
        found.extend(unusable_patterns(config, &ids)?);
        found.extend(unusable_pairs(config, &ids));
        Ok(found)
    };
    let broken = unusable(base)?;
    if broken.is_empty() || !unusable(head)?.is_empty() {
        return Ok(None);
    }
    let mut value = toml::Value::try_from(base)?;
    let head_value = toml::Value::try_from(head)?;
    let mut taken: Vec<HeadValue> = Vec::new();
    for b in &broken {
        // The table the key sits in must exist in the change's copy; the key itself may be
        // unset there, which unsets it here.
        let parent = &b.path[..b.path.len() - 1];
        if value_at(&head_value, parent).is_none_or(|p| !p.is_table()) {
            return Ok(None);
        }
        if !replace_at(&mut value, &b.path, value_at(&head_value, &b.path).cloned()) {
            return Ok(None);
        }
        let note = format!(
            "`{}` does not compile in the base ref's configuration ({:#}); this change's own value for it is in force for this run, and every other setting is the base ref's",
            b.key, b.error
        );
        if !taken.iter().any(|t| t.note == note) {
            taken.push(HeadValue { gate: b.gate, note });
        }
    }
    let Ok(mut repaired) = value.clone().try_into::<DisciplineConfig>() else {
        return Ok(None);
    };
    repaired.deprecations = base.deprecations.clone();
    // The rebuilt configuration must be the base copy with those keys replaced and nothing
    // else: a setting the round trip changed would be one the base ref never chose.
    if toml::Value::try_from(&repaired)? != value || !unusable(&repaired)?.is_empty() {
        return Ok(None);
    }
    Ok(Some((repaired, taken)))
}

/// Records, on the gate that owns each key and on `config-integrity`, that the key was
/// read from the change's copy ([`base_policy_repaired_by_head`]).
pub fn note_head_values(summary: &mut CheckSummary, taken: &[HeadValue]) {
    for t in taken {
        let note = format!("base policy: {}", t.note);
        for outcome in &mut summary.outcomes {
            let owner = t.gate == Some(outcome.gate);
            if outcome.enabled && (owner || outcome.gate == "config-integrity") {
                outcome.notes.push(note.clone());
            }
        }
    }
}

pub fn exempt_filter(settings: &dyn GateSettings) -> Result<PathFilter> {
    PathFilter::new(settings.exempt_paths())
}

/// `discipline:allow(gate-a, gate-b)` anywhere on a line exempts that line.
/// For backwards compatibility with documentation lints, `docs-lint: allow` exempts `time-estimates` and `pii` only.
pub fn line_allows(line: &str, gate: &str) -> bool {
    if (gate == "time-estimates" || gate == "pii")
        && (line.contains("docs-lint: allow") || line.contains("docs-lint:allow"))
    {
        return true;
    }
    const MARKER: &str = "discipline:allow(";
    let Some(start) = line.find(MARKER) else {
        return false;
    };
    let rest = &line[start + MARKER.len()..];
    let Some(end) = rest.find(')') else {
        return false;
    };
    rest[..end].split(',').any(|g| g.trim() == gate)
}

/// Signatures that mean **the tool never ran**, as opposed to the tool running
/// and finding a violation.
///
/// Fail-closed contract (docs/ARCHITECTURE.md §3, F1/F2): "could not check" is
/// exit 2 and must never be reported as "violation found". A missing rustup
/// component or a nightly-only flag on a stable toolchain is an environment
/// fault; reporting it as detected undefined behaviour or a detected data race
/// inverts the meaning of the result and trains readers to distrust the gate.
const TOOLCHAIN_UNAVAILABLE_SIGNATURES: &[&str] = &[
    "is not available for the", // rustup: component missing for this toolchain
    "only accepted on the nightly", // -Z flag used on a stable channel
    "no such subcommand",       // cargo subcommand not installed
    "is not installed",         // rustup: toolchain not installed
    "command not found",
    "not recognized as an internal or external command",
    "error: rustup could not",
    "requires a nightly",
    "requires nightly",
];

/// Returns the offending diagnostic line when `stdout`/`stderr` show that the
/// tool could not run at all. Callers turn this into an `Err` (exit 2) instead
/// of a violation.
pub fn toolchain_unavailable(stdout: &str, stderr: &str) -> Option<String> {
    for stream in [stderr, stdout] {
        for line in stream.lines() {
            let lower = line.to_lowercase();
            if TOOLCHAIN_UNAVAILABLE_SIGNATURES
                .iter()
                .any(|sig| lower.contains(sig))
            {
                return Some(line.trim().to_string());
            }
        }
    }
    None
}

/// A [`Context`] for unit tests that call a gate function directly.
#[cfg(test)]
pub(crate) mod test_support {
    use super::Context;
    use crate::config::DisciplineConfig;
    use crate::gitctx::GitCtx;

    /// A local run over `git` under `config`: no pull request, no directives, no forge.
    pub(crate) fn context<'a>(config: &'a DisciplineConfig, git: &'a GitCtx) -> Context<'a> {
        Context {
            config,
            head_config: None,
            git,
            config_path: "discipline.toml",
            baseline_path: None,
            baseline: None,
            staged: false,
            pr_title: None,
            pr_body: None,
            directives: Vec::new(),
            directive_notes: Vec::new(),
            bench_provenance: None,
            allow_cross_host_bench: false,
            bench_base_file: None,
            bench_head_file: None,
            test_base_report: None,
            test_head_report: None,
            test_report: None,
            forge: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gates a whole-tree run does not evaluate, by `GateInfo::delta_only`: the set is
    /// written out here so that a flag flipped in `config::GATES` fails this test.
    #[test]
    fn the_delta_only_gates_are_the_ones_whose_rule_describes_a_change() {
        let delta_only: Vec<&str> = GATES
            .iter()
            .filter(|g| g.delta_only)
            .map(|g| g.id)
            .collect();
        let mut expected = vec![
            "assertion-reduction",
            "ignored-tests",
            "deletion-rationale",
            "config-integrity",
            "toolchain-config",
            "build-hooks",
            "ci-integrity",
            "ci-skip-set",
            "golden-output",
            "dependency-delta",
            "test-budget",
            "test-floor",
            "suppression-delta",
            "error-swallowing",
            "stub-bodies",
            "harness-tampering",
            "scope-confinement",
            "ratified-paths",
            "review-threads",
            "commit-provenance",
            "bench-regression",
        ];
        let mut found = delta_only.clone();
        expected.sort_unstable();
        found.sort_unstable();
        assert_eq!(found, expected);
        assert!(
            GATES.iter().all(|g| g.available || !g.delta_only),
            "a planned gate never runs, so it carries no whole-tree rule"
        );
    }

    /// Every string-list property of the configuration schema, by name, with whether
    /// its description calls it a glob.
    fn schema_string_lists(v: &serde_json::Value, out: &mut Vec<(String, bool)>) {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
                    for (name, prop) in props {
                        let is_list = prop.get("$ref").and_then(|r| r.as_str())
                            == Some("#/$defs/StringListOrReset");
                        if is_list {
                            let glob = prop
                                .get("description")
                                .and_then(|d| d.as_str())
                                .is_some_and(|d| d.to_lowercase().contains("glob"));
                            out.push((name.clone(), glob));
                        }
                    }
                }
                map.values().for_each(|c| schema_string_lists(c, out));
            }
            serde_json::Value::Array(items) => {
                items.iter().for_each(|c| schema_string_lists(c, out))
            }
            _ => {}
        }
    }

    /// String lists of the schema whose entries are not globs, by what they are. Every
    /// string list is here or in `GLOB_LIST_KEYS`.
    const NOT_GLOB_LISTS: &[&str] = &[
        // Names: functions, macros, jobs, packages, logins, repositories, trailers, units.
        "agent_logins",
        "allow_dependencies",
        "allowed_override_actors",
        "allowed_users",
        "approved_predicates",
        "assert_helper_fns",
        "closing_keywords",
        "deny_dependencies",
        "deterministic_units",
        "excluded_jobs",
        "exempt_authors",
        "extra_assert_macros",
        "function_macros",
        "functions",
        "hostname_denylist",
        "macros",
        "pending_issue_repos",
        "ratification_repos",
        "ratifiers",
        "reference_repos",
        "required_trailers",
        "sources",
        "unconditional_jobs",
        // Text matched as a substring or a prefix.
        "agent_markers",
        "allowed_suppressions",
        "first_party_action_prefixes",
        "mock_assert_fns",
        "mock_setup_fns",
        "placeholders",
        // Regular expressions, compiled by `check_configured_patterns` (`PATTERN_KEYS`,
        // and `forbid_output` by name).
        "allow_patterns",
        "extra_patterns",
        "extra_secret_patterns",
        "forbid_output",
        "forbidden_patterns",
        "snapshot_ignore",
        // Command-line arguments.
        "args",
        // Literal files or directories.
        "citation_source_paths",
        "required_suites",
        // Names and globs mixed, and prefixed forms (`artifact:<glob>`,
        // `regex:<pattern>`); both compiled by `check_configured_patterns`.
        "exempt_arms",
        "ratio_satisfied_by",
    ];

    /// The lists above whose description says "glob" although the list is not a plain
    /// glob list.
    const GLOBS_AMONG_OTHER_FORMS: &[&str] = &["exempt_arms", "ratio_satisfied_by"];

    /// `GLOB_LIST_KEYS` is written by hand (#567). This ties it to the schema: every
    /// string list the schema declares is classified, as a glob list compiled before any
    /// gate runs or, by name, as something else. A list added to the schema is in neither
    /// and fails here, whatever its description says (#600: `workflows` and `corpus_dirs`
    /// are described as "patterns", and a check on the word "glob" did not see them).
    #[test]
    fn every_string_list_of_the_schema_is_a_glob_list_compiled_up_front_or_named_otherwise() {
        let mut lists = Vec::new();
        schema_string_lists(&crate::schema::generate_schema(), &mut lists);
        assert!(
            lists.len() > GLOB_LIST_KEYS.len(),
            "the schema walk found too little"
        );
        for (name, says_glob) in &lists {
            let name = name.as_str();
            let compiled = GLOB_LIST_KEYS.contains(&name);
            assert!(
                compiled || NOT_GLOB_LISTS.contains(&name),
                "`{name}` is a string list in the schema and is not classified: add it to \
                 GLOB_LIST_KEYS if its entries are globs (they are then compiled before any \
                 gate runs), else to NOT_GLOB_LISTS under what its entries are"
            );
            if *says_glob {
                assert!(
                    compiled || GLOBS_AMONG_OTHER_FORMS.contains(&name),
                    "the schema describes `{name}` as globs; add it to GLOB_LIST_KEYS"
                );
            }
        }
        for key in GLOB_LIST_KEYS.iter().chain(NOT_GLOB_LISTS) {
            assert!(
                lists.iter().any(|(name, _)| name == key),
                "`{key}` is classified here and is no string list in the schema"
            );
        }
        for key in NOT_GLOB_LISTS {
            assert!(!GLOB_LIST_KEYS.contains(key), "`{key}` is listed twice");
        }
        for key in GLOBS_AMONG_OTHER_FORMS {
            assert!(
                NOT_GLOB_LISTS.contains(key),
                "`{key}` is not in NOT_GLOB_LISTS"
            );
        }
    }

    /// A glob list is compiled wherever it sits: every key of `GLOB_LIST_KEYS`, set to a
    /// glob that does not compile in a gate's table, is found.
    #[test]
    fn every_glob_list_key_is_compiled_wherever_it_sits() {
        for key in GLOB_LIST_KEYS {
            let mut gate = toml::Table::new();
            gate.insert(
                key.to_string(),
                toml::Value::Array(vec![toml::Value::String("[".to_string())]),
            );
            let value = toml::Value::Table(gate);
            let mut found = Vec::new();
            invalid_globs(
                &value,
                "gates.x",
                &mut key_path(&["gates", "x"]),
                &mut found,
            );
            assert_eq!(found.len(), 1, "{key}");
            assert_eq!(found[0].1, format!("gates.x.{key}"));
            let mut path = key_path(&["gates", "x"]);
            path.push(Seg::Key(key.to_string()));
            assert_eq!(found[0].0, path);
        }
        // Control: a list that is not a glob list is not compiled as one.
        let mut gate = toml::Table::new();
        gate.insert(
            "extra_patterns".to_string(),
            toml::Value::Array(vec![toml::Value::String("[".to_string())]),
        );
        let mut found = Vec::new();
        invalid_globs(
            &toml::Value::Table(gate),
            "gates.x",
            &mut key_path(&["gates", "x"]),
            &mut found,
        );
        assert!(found.is_empty());
    }

    /// The pattern keys [`unusable_patterns`] checks by name, under a rule of their own
    /// (a capture group, the message of an output pattern), and not through
    /// [`PATTERN_KEYS`].
    const PATTERN_KEYS_CHECKED_BY_NAME: &[&str] = &[
        "canary_expected_diagnostic",
        "count_pattern",
        "documented_job_count_pattern",
        "forbid_output",
        "zero_items_pattern",
    ];

    /// String values of the schema that are not regular expressions, by what they are.
    /// Every string value is here, or is a pattern key compiled before any gate runs.
    const NOT_PATTERN_SCALARS: &[&str] = &[
        // Paths of files, and one glob.
        "archive_path",
        "base_file",
        "base_report",
        "constant_file",
        "deny_file",
        "documented_job_count_path",
        "head_file",
        "head_report",
        "manifest",
        "path",
        "ratio_baseline",
        "snapshot",
        "superseded_registry",
        "test_report",
        "workflow",
        // Command lines.
        "canary_command",
        "command",
        "test_command",
        // Names: jobs, steps, constants, presets, sanitizers, groups, actions, record keys.
        "change_job",
        "constant_name",
        "guard",
        "job",
        "name",
        "preset",
        "record_commit_key",
        "rollup_job",
        "sanitizer",
        "uses",
        // Text compared or printed as written.
        "description",
        "marker",
        "pinned_version",
        "provenance",
        "reason",
        "review_trailer",
    ];

    /// How many string lists hold neither globs nor patterns (`NOT_GLOB_LISTS` without
    /// the pattern keys). A list added there changes this count, which is where its
    /// author says whether its entries are regular expressions.
    const LISTS_OF_NEITHER_GLOBS_NOR_PATTERNS: usize = 33;

    /// Every string and string-list property of the configuration schema:
    /// `(name, is a list, description)`.
    fn schema_strings(v: &serde_json::Value, out: &mut Vec<(String, bool, String)>) {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
                    for (name, prop) in props {
                        let is_list = prop.get("$ref").and_then(|r| r.as_str())
                            == Some("#/$defs/StringListOrReset");
                        let is_string = match prop.get("type") {
                            Some(serde_json::Value::String(t)) => t == "string",
                            Some(serde_json::Value::Array(ts)) => {
                                ts.iter().any(|t| t == "string")
                                    && ts.iter().all(|t| t == "string" || t == "null")
                            }
                            _ => false,
                        };
                        // A closed set of words is not free text.
                        if is_list || (is_string && prop.get("enum").is_none()) {
                            let description = prop
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or_default()
                                .to_lowercase();
                            out.push((name.clone(), is_list, description));
                        }
                    }
                }
                map.values().for_each(|c| schema_strings(c, out));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|c| schema_strings(c, out)),
            _ => {}
        }
    }

    fn is_pattern_key(name: &str) -> bool {
        PATTERN_KEYS.iter().any(|(k, _)| *k == name) || PATTERN_KEYS_CHECKED_BY_NAME.contains(&name)
    }

    /// `PATTERN_KEYS` is written by hand. This ties it to the schema: every string value
    /// the schema declares is a pattern key, compiled before any gate runs, or is named as
    /// something else. A string added to the schema is in neither and fails here, and so
    /// does one whose description calls it a regular expression while it is not compiled.
    #[test]
    fn every_string_of_the_schema_is_a_pattern_compiled_up_front_or_named_otherwise() {
        let mut strings = Vec::new();
        schema_strings(&crate::schema::generate_schema(), &mut strings);
        assert!(
            strings.len() > PATTERN_KEYS.len() + NOT_PATTERN_SCALARS.len(),
            "the schema walk found too little"
        );
        for (name, is_list, description) in &strings {
            let name = name.as_str();
            let pattern = is_pattern_key(name);
            if *is_list {
                assert!(
                    pattern || GLOB_LIST_KEYS.contains(&name) || NOT_GLOB_LISTS.contains(&name),
                    "`{name}` is a string list in the schema and is not classified"
                );
                assert!(
                    !(pattern && GLOB_LIST_KEYS.contains(&name)),
                    "`{name}` is a glob list and a pattern key"
                );
            } else {
                assert!(
                    pattern != NOT_PATTERN_SCALARS.contains(&name),
                    "`{name}` is a string in the schema and must be in exactly one of \
                     PATTERN_KEYS (it is then compiled before any gate runs), the keys \
                     `unusable_patterns` checks by name, or NOT_PATTERN_SCALARS under what \
                     it is"
                );
            }
            let says_regex = ["regex", "regular expression"]
                .iter()
                .any(|w| description.contains(w));
            assert!(
                !says_regex || pattern,
                "the schema describes `{name}` as a regular expression; add it to PATTERN_KEYS"
            );
        }
        let declared = |key: &str| strings.iter().any(|(name, _, _)| name == key);
        for key in PATTERN_KEYS
            .iter()
            .map(|(k, _)| *k)
            .chain(PATTERN_KEYS_CHECKED_BY_NAME.iter().copied())
            .chain(NOT_PATTERN_SCALARS.iter().copied())
        {
            assert!(
                declared(key),
                "`{key}` is classified here and is no string in the schema"
            );
        }
        for key in PATTERN_KEYS_CHECKED_BY_NAME {
            assert!(
                !PATTERN_KEYS.iter().any(|(k, _)| k == key),
                "`{key}` is listed twice"
            );
        }
        let neither = NOT_GLOB_LISTS.iter().filter(|k| !is_pattern_key(k)).count();
        assert_eq!(
            neither, LISTS_OF_NEITHER_GLOBS_NOR_PATTERNS,
            "a string list was added to or taken from NOT_GLOB_LISTS: if its entries are \
             regular expressions, add it to PATTERN_KEYS so that they are compiled before \
             any gate runs; then update LISTS_OF_NEITHER_GLOBS_NOR_PATTERNS"
        );
    }

    /// A pattern key is compiled wherever it sits: every key of `PATTERN_KEYS`, set to a
    /// value that does not compile, as one value or in a list, in a gate's table or in an
    /// entry of an array of tables, is found and named.
    #[test]
    fn every_pattern_key_is_compiled_wherever_it_sits() {
        let found_in = |gate: toml::Table| {
            let mut found = Vec::new();
            invalid_patterns(
                &toml::Value::Table(gate),
                "gates.x",
                &mut key_path(&["gates", "x"]),
                &mut found,
            );
            found
        };
        let text = |s: &str| toml::Value::String(s.to_string());
        for (key, form) in PATTERN_KEYS {
            let bad = match form {
                PatternForm::Regex => "(a",
                PatternForm::RatioEvidence => "regex:(a",
            };
            let good = match form {
                PatternForm::Regex => "(a)",
                PatternForm::RatioEvidence => "regex:(a)",
            };
            for value in [text(bad), toml::Value::Array(vec![text(good), text(bad)])] {
                let mut gate = toml::Table::new();
                gate.insert(key.to_string(), value.clone());
                let found = found_in(gate);
                assert_eq!(found.len(), 1, "{key}");
                assert_eq!(found[0].1, format!("gates.x.{key}"));
                let mut path = key_path(&["gates", "x"]);
                path.push(Seg::Key(key.to_string()));
                assert_eq!(found[0].0, path);
                let shown = format!("{:#}", found[0].2);
                assert!(shown.contains(&format!("`gates.x.{key}`")), "{shown}");
                let (reason, _) = crate::could_not_check::classify(&found[0].2);
                assert_eq!(reason, crate::could_not_check::Reason::Configuration);

                // In an entry of an array of tables: named by `name`, else by index.
                let mut unnamed = toml::Table::new();
                unnamed.insert(key.to_string(), value.clone());
                let mut named = unnamed.clone();
                named.insert("name".to_string(), text("api"));
                // `name` is no pattern key, so the entry's name is never compiled.
                assert!(!is_pattern_key("name"));
                let mut gate = toml::Table::new();
                gate.insert(
                    "entries".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Table(unnamed),
                        toml::Value::Table(named),
                    ]),
                );
                let found = found_in(gate);
                let keys: Vec<&str> = found.iter().map(|f| f.1.as_str()).collect();
                assert_eq!(
                    keys,
                    [
                        format!("gates.x.entries[0].{key}"),
                        format!("gates.x.entries[api].{key}")
                    ]
                );
                let mut path = key_path(&["gates", "x", "entries"]);
                path.push(Seg::Index(1));
                path.push(Seg::Key(key.to_string()));
                assert_eq!(found[1].0, path);
            }
            // Controls: a value that compiles, and an empty list.
            for value in [text(good), toml::Value::Array(Vec::new())] {
                let mut gate = toml::Table::new();
                gate.insert(key.to_string(), value);
                assert!(found_in(gate).is_empty(), "{key}");
            }
        }
        // Control: a key that is not a pattern key is not compiled as one.
        let mut gate = toml::Table::new();
        gate.insert("exempt_paths".to_string(), text("(a"));
        gate.insert("command".to_string(), text("(a"));
        assert!(found_in(gate).is_empty());
        // The other forms of `ratio_satisfied_by` are read as what they are, and an
        // unknown form is refused.
        for (entry, ok) in [
            ("interval", true),
            ("marker:(a", true),
            ("artifact:results/(a*", true),
            ("regex:(a", false),
            ("pattern:x", false),
        ] {
            let mut gate = toml::Table::new();
            gate.insert(
                "ratio_satisfied_by".to_string(),
                toml::Value::Array(vec![text(entry)]),
            );
            assert_eq!(found_in(gate).is_empty(), ok, "{entry}");
        }
    }

    /// Through the check the run makes: the pattern keys of real gate tables, and the keys
    /// checked by name, stop it with the gate, the key and the reason `configuration`;
    /// the same table with a pattern that compiles, or with the gate off, does not.
    #[test]
    fn a_pattern_of_any_gate_that_does_not_compile_fails_the_check_made_up_front() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "time-estimates",
                "[gates.time-estimates]\nenabled = true\nallow_patterns = ['(a']\n",
                "gates.time-estimates.allow_patterns",
            ),
            (
                "pii",
                "[gates.pii]\nenabled = true\nextra_patterns = ['(a']\n",
                "gates.pii.extra_patterns",
            ),
            (
                "shell-secrets",
                "[gates.shell-secrets]\nenabled = true\nextra_secret_patterns = ['(a']\n",
                "gates.shell-secrets.extra_secret_patterns",
            ),
            (
                "issue-link",
                "[gates.issue-link]\nenabled = true\npattern = '(a'\n",
                "gates.issue-link.pattern",
            ),
            (
                "archive-contents",
                "[gates.archive-contents]\nenabled = true\narchive_path = \"d/*.tgz\"\nforbidden_patterns = ['(a']\n",
                "gates.archive-contents.forbidden_patterns",
            ),
            (
                "manifest-sync",
                "[gates.manifest-sync]\nenabled = true\n[[gates.manifest-sync.rules]]\nmanifest = \"a.toml\"\nextract_regex = '(a'\nwatched_paths = [\"a/**\"]\n",
                "gates.manifest-sync.rules[0].extract_regex",
            ),
            (
                "version-lockstep",
                "[gates.version-lockstep]\nenabled = true\n[[gates.version-lockstep.groups]]\nname = \"g\"\n[[gates.version-lockstep.groups.sources]]\npath = \"a\"\nregex = '(a'\n",
                "gates.version-lockstep.groups[g].sources[0].regex",
            ),
            (
                "provenance-tags",
                "[gates.provenance-tags]\nenabled = true\nratio_satisfied_by = ['regex:(a']\n",
                "gates.provenance-tags.ratio_satisfied_by",
            ),
            (
                "command",
                "[gates.command]\nenabled = true\ncommand = \"true\"\nsnapshot_ignore = ['(a']\n",
                "gates.command.snapshot_ignore",
            ),
            (
                "command",
                "[gates.command]\nenabled = true\n[[gates.command.commands]]\nname = \"api\"\ncommand = \"true\"\nsnapshot_ignore = ['(a']\n",
                "gates.command.commands[api].snapshot_ignore",
            ),
            // Checked by name.
            (
                "command",
                "[gates.command]\nenabled = true\ncommand = \"true\"\ncount_pattern = '(a'\n",
                "gates.command.count_pattern",
            ),
            (
                "command",
                "[gates.command]\nenabled = true\ncommand = \"true\"\nzero_items_pattern = '(a'\n",
                "gates.command.zero_items_pattern",
            ),
            (
                "command",
                "[gates.command]\nenabled = true\ncommand = \"true\"\ncanary_expected_diagnostic = '(a'\n",
                "gates.command.canary_expected_diagnostic",
            ),
            (
                "command",
                "[gates.command]\nenabled = true\ncommand = \"true\"\nforbid_output = ['(a']\n",
                "gates.command.forbid_output",
            ),
            (
                "ci-integrity",
                "[gates.ci-integrity]\nenabled = true\ndocumented_job_count_path = \"d.md\"\ndocumented_job_count_pattern = '(a'\n",
                "gates.ci-integrity.documented_job_count_pattern",
            ),
        ];
        let ids: Vec<&'static str> = GATES.iter().map(|g| g.id).collect();
        for (gate, body, key) in cases {
            let e = check_configured_patterns(&policy(body), &ids).unwrap_err();
            let (reason, named) = crate::could_not_check::classify(&e);
            assert_eq!(
                reason,
                crate::could_not_check::Reason::Configuration,
                "{key}"
            );
            assert_eq!(named.as_deref(), Some(*gate), "{key}");
            let shown = format!("{e:#}");
            assert!(shown.contains(&format!("`{key}`")), "{shown}");
            // Controls.
            let good = policy(&body.replace("(a", "(a)"));
            assert!(check_configured_patterns(&good, &ids).is_ok(), "{key}");
            let off = policy(&body.replace("enabled = true", "enabled = false"));
            assert!(check_configured_patterns(&off, &ids).is_ok(), "{key}");
            // A gate the run did not select is not read.
            let others: Vec<&'static str> = ids.iter().copied().filter(|i| i != gate).collect();
            assert!(
                check_configured_patterns(&policy(body), &others).is_ok(),
                "{key}"
            );
        }
    }

    const CANARY_ADDRESS: &str =
        "[gates.sanitizers]\nenabled = true\nsanitizer = \"address\"\ncanary = true\n";
    const CANARY_THREAD: &str =
        "[gates.sanitizers]\nenabled = true\nsanitizer = \"thread\"\ncanary = true\n";
    const NO_CANARY_ADDRESS: &str =
        "[gates.sanitizers]\nenabled = true\nsanitizer = \"address\"\ncanary = false\n";

    #[test]
    fn a_canary_under_a_sanitizer_other_than_thread_fails_the_check_made_up_front() {
        let ids = ["sanitizers"];
        let e = check_configured_patterns(&policy(CANARY_ADDRESS), &ids).unwrap_err();
        let (reason, gate) = crate::could_not_check::classify(&e);
        assert_eq!(reason, crate::could_not_check::Reason::Configuration);
        assert_eq!(gate.as_deref(), Some("sanitizers"));
        let shown = format!("{e:#}");
        assert!(shown.contains("`gates.sanitizers.canary`"), "{shown}");
        assert!(
            shown.contains("`gates.sanitizers.sanitizer = \"address\"`"),
            "{shown}"
        );
        // Controls.
        for ok in [CANARY_THREAD, NO_CANARY_ADDRESS] {
            assert!(check_configured_patterns(&policy(ok), &ids).is_ok(), "{ok}");
        }
        let off = CANARY_ADDRESS.replace("enabled = true", "enabled = false");
        assert!(check_configured_patterns(&policy(&off), &ids).is_ok());
        // A name of the wrong shape is reported first, as before.
        let misshapen = CANARY_ADDRESS.replace("address", "Address");
        let e = check_configured_patterns(&policy(&misshapen), &ids).unwrap_err();
        assert!(format!("{e:#}").contains("is not a sanitizer name"));
        // Under `--policy-from base` a change that drops the canary repairs it.
        let (config, taken) = repaired(CANARY_ADDRESS, NO_CANARY_ADDRESS).expect("a repair");
        assert!(!config.gates.sanitizers.canary);
        assert!(taken[0].note.contains("`gates.sanitizers.canary`"));
    }

    const POLICY_HEAD: &str = "[meta]\nversion = 1\nname = \"t\"\n";
    const BAD_SCOPE: &str =
        "[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/[a-z\"]\n";
    const GOOD_SCOPE: &str =
        "[gates.scope-confinement]\nenabled = true\nforbidden_paths = [\"secrets/**\", \"keys/[a-z]*\"]\n";
    const RULE_BAD: &str = "[gates.manifest-sync]\nenabled = true\n[[gates.manifest-sync.rules]]\nmanifest = \"a.toml\"\nextract_regex = \"x\"\nwatched_paths = [\"a/**\"]\n[[gates.manifest-sync.rules]]\nmanifest = \"b.toml\"\nextract_regex = \"x\"\nwatched_paths = [\"src/[a-z\"]\n";
    const RULE_GOOD: &str = "[gates.manifest-sync]\nenabled = true\n[[gates.manifest-sync.rules]]\nmanifest = \"a.toml\"\nextract_regex = \"x\"\nwatched_paths = [\"a/**\"]\n[[gates.manifest-sync.rules]]\nmanifest = \"b.toml\"\nextract_regex = \"x\"\nwatched_paths = [\"src/[a-z]*\"]\n";
    const RULE_ONE_LEFT: &str = "[gates.manifest-sync]\nenabled = true\n[[gates.manifest-sync.rules]]\nmanifest = \"a.toml\"\nextract_regex = \"x\"\nwatched_paths = [\"a/**\"]\n";
    const COUNT_HALF: &str = "[gates.ci-integrity]\ndocumented_job_count_path = \"docs/ci.md\"\n";
    const COUNT_NONE: &str = "[gates.ci-integrity]\nrollup_job = \"gate\"\n";
    const COMMAND_BAD: &str = "[gates.command]\nenabled = true\ncommand = \"true\"\nmin_count = 4\nforbid_output = ['ok', '(a']\n";
    const COMMAND_GOOD: &str = "[gates.command]\nenabled = true\ncommand = \"other\"\nmin_count = 1\nforbid_output = ['ok', 'a+']\n";

    fn policy(body: &str) -> DisciplineConfig {
        DisciplineConfig::from_toml_str(&format!("{POLICY_HEAD}{body}")).unwrap()
    }

    fn repaired(base: &str, head: &str) -> Option<(DisciplineConfig, Vec<HeadValue>)> {
        base_policy_repaired_by_head(&policy(base), &policy(head), SuiteChoice::All).unwrap()
    }

    /// The serialised form is what the repair edits, so a configuration must come back
    /// from it unchanged: the defaults, and one with every kind of table set.
    #[test]
    fn a_configuration_survives_the_round_trip_the_repair_makes() {
        for body in ["", GOOD_SCOPE, RULE_GOOD, COMMAND_GOOD, COUNT_NONE] {
            let config = policy(body);
            let value = toml::Value::try_from(&config).unwrap();
            let back: DisciplineConfig = value.clone().try_into().unwrap();
            assert_eq!(toml::Value::try_from(&back).unwrap(), value, "{body}");
        }
    }

    #[test]
    fn the_repair_replaces_only_the_keys_that_do_not_compile() {
        let head = format!("{GOOD_SCOPE}[gates.time-estimates]\nenabled = false\n[gates.pii]\nseverity = \"warning\"\n");
        let (config, taken) = repaired(BAD_SCOPE, &head).expect("a repair");
        assert_eq!(
            config.gates.scope_confinement.forbidden_paths,
            ["secrets/**", "keys/[a-z]*"]
        );
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].gate, Some("scope-confinement"));
        assert!(taken[0]
            .note
            .contains("`gates.scope-confinement.forbidden_paths`"));
        // Everything else is the base copy: serialised, the result differs from it in
        // that one key.
        let mut expected = toml::Value::try_from(policy(BAD_SCOPE)).unwrap();
        expected["gates"]["scope-confinement"]["forbidden_paths"] =
            toml::Value::try_from(["secrets/**", "keys/[a-z]*"]).unwrap();
        assert_eq!(toml::Value::try_from(&config).unwrap(), expected);
        assert!(config.gates.time_estimates.enabled);
    }

    #[test]
    fn the_repair_reaches_an_entry_of_an_array_of_tables_and_one_key_of_a_command() {
        let (config, taken) = repaired(RULE_BAD, RULE_GOOD).expect("a repair");
        assert_eq!(
            config.gates.manifest_sync.rules[1].watched_paths,
            ["src/[a-z]*"]
        );
        assert_eq!(taken[0].gate, Some("manifest-sync"));
        // The same key of the other entry is untouched, and so is every other key of the
        // command table the change also edited.
        assert_eq!(config.gates.manifest_sync.rules[0].watched_paths, ["a/**"]);
        let (config, taken) = repaired(COMMAND_BAD, COMMAND_GOOD).expect("a repair");
        assert_eq!(config.gates.command.forbid_output, ["ok", "a+"]);
        assert_eq!(config.gates.command.command.as_deref(), Some("true"));
        assert_eq!(config.gates.command.min_count, Some(4));
        assert!(taken[0].note.contains("`gates.command.forbid_output`"));
    }

    /// A half-configured pair is repaired by setting the other key or removing the one.
    #[test]
    fn the_repair_covers_a_half_configured_job_count() {
        let (config, taken) = repaired(COUNT_HALF, COUNT_NONE).expect("a repair");
        assert_eq!(config.gates.ci_integrity.documented_job_count_path, None);
        // Nothing else of the change's table came along.
        assert_eq!(
            config.gates.ci_integrity.rollup_job,
            policy(COUNT_HALF).gates.ci_integrity.rollup_job
        );
        assert_eq!(taken.len(), 2);
    }

    #[test]
    fn nothing_is_repaired_when_the_change_does_not_repair_it() {
        // Nothing to repair.
        assert!(repaired(GOOD_SCOPE, GOOD_SCOPE).is_none());
        // The change leaves the value, or breaks its own copy elsewhere.
        assert!(repaired(BAD_SCOPE, BAD_SCOPE).is_none());
        assert!(repaired(BAD_SCOPE, &format!("{GOOD_SCOPE}{COUNT_HALF}")).is_none());
        // The change switches the gate off: its copy passes, the value is still bad.
        let off = BAD_SCOPE.replace("enabled = true", "enabled = false");
        assert!(repaired(BAD_SCOPE, &off).is_none());
        // The change dropped the table the value sits in.
        assert!(repaired(RULE_BAD, RULE_ONE_LEFT).is_none());
    }

    #[test]
    fn the_checks_made_before_any_gate_runs_name_the_key() {
        let ids = ["ci-integrity", "command"];
        let shown = |body: &str| {
            let config = policy(body);
            let e = check_configured_patterns(&config, &ids)
                .and_then(|()| check_configured_pairs(&config, &ids))
                .unwrap_err();
            let (reason, gate) = crate::could_not_check::classify(&e);
            assert_eq!(reason, crate::could_not_check::Reason::Configuration);
            (format!("{e:#}"), gate)
        };
        let (text, gate) = shown(COMMAND_BAD);
        assert!(text.contains("`gates.command.forbid_output`"), "{text}");
        assert!(text.contains("never as literal text"), "{text}");
        assert_eq!(gate.as_deref(), Some("command"));
        let (text, gate) = shown(COUNT_HALF);
        assert!(text.contains("`gates.ci-integrity.documented_job_count_path`"));
        assert!(text.contains("`gates.ci-integrity.documented_job_count_pattern`"));
        assert_eq!(gate.as_deref(), Some("ci-integrity"));
        // Controls.
        let good = policy(&format!("{COMMAND_GOOD}{COUNT_NONE}"));
        assert!(check_configured_patterns(&good, &ids).is_ok());
        assert!(check_configured_pairs(&good, &ids).is_ok());
        let off = policy(&COUNT_HALF.replace(
            "[gates.ci-integrity]\n",
            "[gates.ci-integrity]\nenabled = false\n",
        ));
        assert!(check_configured_pairs(&off, &ids).is_ok());
    }

    #[test]
    fn a_capture_pattern_must_compile_and_have_a_group() {
        let shown = |p: &str| format!("{:#}", capture_pattern(p, "gates.x.key").unwrap_err());
        assert!(shown("(a").contains("`gates.x.key`"));
        assert!(shown("(a").contains("not a valid regular expression"));
        assert!(shown(r"\d+ jobs").contains("no capture group"));
        // A non-capturing group reads nothing either.
        assert!(shown(r"(?:\d+) jobs").contains("no capture group"));
        let (reason, _) = crate::could_not_check::classify(&capture_pattern("x", "k").unwrap_err());
        assert_eq!(reason, crate::could_not_check::Reason::Configuration);
        let re = capture_pattern(r"(\d+) jobs", "gates.x.key").unwrap();
        assert_eq!(&re.captures("7 jobs").unwrap()[1], "7");
        // A named group is a group.
        assert!(capture_pattern(r"(?P<n>\d+) jobs", "gates.x.key").is_ok());
    }

    /// Base-anchored classification: a file that was production code on the base
    /// side is judged by its base path, so a same-language rename into test scope
    /// is flagged and stays production; a file that was test code is judged by its
    /// head path; a cross-language rename keeps head behaviour with a note.
    #[test]
    fn base_anchored_classification_pins_renames_to_the_base_path() {
        use crate::gitctx::{ChangeKind, ChangedFile};
        let reg = crate::ast::default_registry();
        let changed = |old: &str, new: &str, kind: ChangeKind| ChangedFile {
            path: new.to_string(),
            old_path: old.to_string(),
            kind,
            added_lines: std::collections::BTreeSet::new(),
        };

        // Added file: head path, no flag, no note.
        let added = base_anchored_classification(
            &changed(
                "src/main/java/TestNew.java",
                "src/main/java/TestNew.java",
                ChangeKind::Added,
            ),
            &reg,
            &[],
        );
        assert_eq!(added.classify_path, "src/main/java/TestNew.java");
        assert!(!added.reclassified);
        assert!(added.language_changed_note.is_none());

        // Same-language rename out of test scope into it: base path, flagged.
        let renamed = base_anchored_classification(
            &changed(
                "src/main/java/Repo.java",
                "src/main/java/TestRepo.java",
                ChangeKind::Renamed,
            ),
            &reg,
            &[],
        );
        assert_eq!(renamed.classify_path, "src/main/java/Repo.java");
        assert!(renamed.reclassified);
        assert!(renamed.language_changed_note.is_none());

        // Same-language rename inside test scope: silent.
        let inside = base_anchored_classification(
            &changed("tests/a_test.py", "tests/b_test.py", ChangeKind::Renamed),
            &reg,
            &[],
        );
        // Test code on the base side has no production classification to keep.
        assert_eq!(inside.classify_path, "tests/b_test.py");
        assert!(!inside.reclassified);
        let out = base_anchored_classification(
            &changed(
                "src/main/java/TestRepo.java",
                "src/main/java/Repo.java",
                ChangeKind::Renamed,
            ),
            &reg,
            &[],
        );
        // Out of test scope: production code from now on (#517).
        assert_eq!(out.classify_path, "src/main/java/Repo.java");
        assert!(!out.reclassified);
        // The same for a path that is test scope only by a declared glob.
        let declared = base_anchored_classification(
            &changed("qa/a.py", "src/a.py", ChangeKind::Renamed),
            &reg,
            &["qa/**".to_string()],
        );
        assert_eq!(declared.classify_path, "src/a.py");
        let undeclared = base_anchored_classification(
            &changed("qa/a.py", "src/a.py", ChangeKind::Renamed),
            &reg,
            &[],
        );
        assert_eq!(undeclared.classify_path, "qa/a.py");

        // Same-language rename inside production code: base path, silent.
        let moved = base_anchored_classification(
            &changed("pkg/a.go", "pkg/b.go", ChangeKind::Renamed),
            &reg,
            &[],
        );
        assert_eq!(moved.classify_path, "pkg/a.go");
        assert!(!moved.reclassified);

        // Cross-language rename: head path with a note.
        let crossed = base_anchored_classification(
            &changed("src/a.c", "src/a.cpp", ChangeKind::Renamed),
            &reg,
            &[],
        );
        assert_eq!(crossed.classify_path, "src/a.cpp");
        assert!(!crossed.reclassified);
        assert!(crossed
            .language_changed_note
            .is_some_and(|n| { n.contains("src/a.cpp") && n.contains("src/a.c") }));
    }

    /// Base anchoring needs an unchanged extension: the extension also picks the
    /// grammar a pack parses with, so `.js` -> `.ts` (one pack) or `.c` -> `.h`
    /// is classified by the head path with a note, and an equal extension is
    /// anchored to the base path.
    #[test]
    fn base_anchoring_needs_an_unchanged_extension_within_one_pack() {
        use crate::gitctx::{ChangeKind, ChangedFile};
        let reg = crate::ast::default_registry();
        let renamed = |old: &str, new: &str| ChangedFile {
            path: new.to_string(),
            old_path: old.to_string(),
            kind: ChangeKind::Renamed,
            added_lines: std::collections::BTreeSet::new(),
        };
        for (old, new) in [
            ("web/a.js", "web/a.ts"),
            ("web/a.ts", "web/a.tsx"),
            ("src/a.c", "src/a.h"),
            ("pkg/a.py", "pkg/a.pyi"),
            ("web/a.JS", "web/a.js"),
        ] {
            let got = base_anchored_classification(&renamed(old, new), &reg, &[]);
            assert_eq!(got.classify_path, new, "{old} -> {new}");
            assert!(!got.reclassified, "{old} -> {new}");
            assert!(got.language_changed_note.is_some(), "{old} -> {new}");
        }
        let same = base_anchored_classification(&renamed("web/a.ts", "web/test_a.ts"), &reg, &[]);
        assert_eq!(same.classify_path, "web/a.ts");
        assert!(same.language_changed_note.is_none());
    }

    /// #565: whether a rename moved production code into test scope does not depend
    /// on the grammar. Across extensions and packs the file is classified by a path
    /// the head pack parses with the head grammar and reads as production code.
    #[test]
    fn a_production_file_stays_production_through_an_extension_or_language_change() {
        use crate::gitctx::{ChangeKind, ChangedFile};
        let reg = crate::ast::default_registry();
        let renamed = |old: &str, new: &str| ChangedFile {
            path: new.to_string(),
            old_path: old.to_string(),
            kind: ChangeKind::Renamed,
            added_lines: std::collections::BTreeSet::new(),
        };
        for (old, new, classify) in [
            ("src/Repo.cc", "src/RepoTest.cpp", "src/Repo.cpp"),
            ("src/Repo.kt", "src/RepoTest.kts", "src/Repo.kts"),
            ("web/api.js", "tests/api.mjs", "web/api.mjs"),
            ("web/api.ts", "web/__tests__/api.tsx", "web/api.tsx"),
            (
                "src/main/java/Repo.java",
                "src/main/java/RepoTest.kt",
                "src/main/java/Repo.kt",
            ),
            // No pack reads the base path: the head pack judges it.
            ("docs/service.txt", "app/test_service.py", "docs/service.py"),
            ("docs/LICENSE", "app/test_license.py", "docs/LICENSE.py"),
            // The base name is a test name for the head pack only.
            (
                "src/main/java/RepoSpec.java",
                "src/main/java/RepoSpec.kt",
                "renamed.kt",
            ),
        ] {
            let got = base_anchored_classification(&renamed(old, new), &reg, &[]);
            assert_eq!(got.classify_path, classify, "{old} -> {new}");
            assert!(got.reclassified, "{old} -> {new}");
            assert!(
                got.language_changed_note.is_some_and(|n| n.contains(old)
                    && n.contains(new)
                    && n.contains("still judged as production code")),
                "{old} -> {new}"
            );
            let head = reg.find_pack(new).expect("head pack");
            assert!(
                reg.find_pack(classify)
                    .is_some_and(|p| p.id() == head.id() && !p.is_test_path(classify)),
                "{classify}"
            );
        }
        // Negative controls: test code on the base side is classified by the head path.
        for (old, new) in [
            ("tests/util.js", "tests/util.mjs"),
            ("tests/util.js", "web/util.mjs"),
            ("src/test/java/RepoTest.java", "src/test/kotlin/RepoTest.kt"),
            ("spec/notes.txt", "spec/notes_spec.rb"),
        ] {
            let got = base_anchored_classification(&renamed(old, new), &reg, &[]);
            assert_eq!(got.classify_path, new, "{old} -> {new}");
            assert!(!got.reclassified, "{old} -> {new}");
            assert!(
                got.language_changed_note
                    .is_some_and(|n| n.contains("classified by its new path")),
                "{old} -> {new}"
            );
        }
    }

    /// A path only `[tests] paths` reads as test code: production code renamed into
    /// it stays production code with a note and no finding, with and without an
    /// extension change; a glob that covers every file of the head extension leaves
    /// no production path, so the head path decides.
    #[test]
    fn a_rename_into_a_declared_test_path_is_noted() {
        use crate::gitctx::{ChangeKind, ChangedFile};
        let reg = crate::ast::default_registry();
        let renamed = |old: &str, new: &str| ChangedFile {
            path: new.to_string(),
            old_path: old.to_string(),
            kind: ChangeKind::Renamed,
            added_lines: std::collections::BTreeSet::new(),
        };
        let qa = ["qa/**".to_string()];
        let same = base_anchored_classification(&renamed("app/a.py", "qa/a.py"), &reg, &qa);
        assert_eq!(same.classify_path, "app/a.py");
        assert!(!same.reclassified);
        assert!(same
            .declared_scope_note
            .is_some_and(|n| n.contains("qa/a.py") && n.contains("[tests] paths")));
        let crossed = base_anchored_classification(&renamed("web/a.js", "qa/a.mjs"), &reg, &qa);
        assert_eq!(crossed.classify_path, "web/a.mjs");
        assert!(!crossed.reclassified);
        assert!(crossed
            .language_changed_note
            .is_some_and(|n| n.contains("still judged as production code")));
        // Negative controls: no glob, no note; a rename inside production code, no note.
        let plain = base_anchored_classification(&renamed("app/a.py", "qa/a.py"), &reg, &[]);
        assert!(plain.declared_scope_note.is_none());
        let inside = base_anchored_classification(&renamed("qa/a.py", "qa/b.py"), &reg, &qa);
        assert!(inside.declared_scope_note.is_none());
        assert_eq!(inside.classify_path, "qa/b.py");
        let every = ["**/*.mjs".to_string()];
        let all = base_anchored_classification(&renamed("web/a.js", "web/a.mjs"), &reg, &every);
        assert_eq!(all.classify_path, "web/a.mjs");
        assert!(!all.reclassified);
    }

    #[test]
    fn inline_marker_is_scoped_to_the_named_gate() {
        assert!(line_allows("x <!-- discipline:allow(pii) -->", "pii"));
        assert!(line_allows(
            "x // discipline:allow(time-estimates, pii)",
            "pii"
        ));
        assert!(line_allows(
            "planned for 2 weeks docs-lint: allow",
            "time-estimates"
        ));
        assert!(line_allows(
            "connect to 192.168.1.20 # docs-lint: allow",
            "pii"
        ));
        assert!(!line_allows(
            "#[allow(dead_code)] // docs-lint: allow",
            "suppression-delta"
        ));
        assert!(!line_allows(
            "x <!-- discipline:allow(time-estimates) -->",
            "pii"
        ));
        assert!(!line_allows("discipline:allow(pii", "pii"));
        assert!(!line_allows("we allow pii here", "pii"));
    }

    #[test]
    fn path_filter_matches_globs_and_rejects_bad_ones() {
        let f = PathFilter::new(&["docs/archive/**".into(), "*.lock".into()]).unwrap();
        assert!(f.matches("docs/archive/2020/x.md"));
        assert!(f.matches("Cargo.lock"));
        assert!(!f.matches("docs/GATES.md"));
        assert!(PathFilter::new(&["[".into()]).is_err());
    }

    #[test]
    fn toolchain_unavailable_distinguishes_environment_faults_from_findings() {
        // Positive controls: the tool never ran.
        for bad in [
            "error: the 'miri' component which provides the command 'cargo-miri' is not available for the 'stable-aarch64-apple-darwin' toolchain",
            "error: the `-Z` flag is only accepted on the nightly channel of Cargo, but this is the `stable` channel",
            "error: no such subcommand: `miri`",
            "error: toolchain 'nightly-x86_64-unknown-linux-gnu' is not installed",
            "cargo: command not found",
        ] {
            assert!(
                toolchain_unavailable("", bad).is_some(),
                "missed environment fault: {bad}"
            );
        }

        // Negative controls: the tool RAN and found something. These must stay
        // violations, or the fix would silently disarm both gates.
        for real in [
            "error: Undefined Behavior: attempting a read access using <untagged> at alloc1[0x0]",
            "ERROR: AddressSanitizer: heap-use-after-free on address 0x602000000010",
            "WARNING: ThreadSanitizer: data race (pid=1234)",
            "test result: FAILED. 3 passed; 1 failed",
            "assertion `left == right` failed",
        ] {
            assert!(
                toolchain_unavailable("", real).is_none(),
                "environment fault falsely claimed for a real finding: {real}"
            );
        }

        // stdout is inspected too, and the offending line is returned.
        let hit = toolchain_unavailable("error: no such subcommand: `miri`", "").unwrap();
        assert!(hit.contains("no such subcommand"), "got: {hit}");
    }
}
