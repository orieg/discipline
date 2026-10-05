//! Override directive parser shared by every gate that accepts an escape hatch.
//!
//! Grammar rules:
//!   - a directive must **begin its own line** (optionally inside `<!-- -->`);
//!     a mention mid-sentence, in a table cell, or in a code span never arms it
//!   - lines inside fenced code blocks are ignored; a fence closes only on a line of the
//!     same character at least as long, so a shorter fence quoted inside it stays code
//!   - a line indented four or more columns (a tab counts as four) is an indented code
//!     block and is ignored; up to three spaces of indentation is allowed
//!   - the reason must be non-empty and must not be a template placeholder
//!   - an override is **scoped**: it only covers a subject that its reason names
//!
//! The defect this prevents: a PR that *described* the override
//! mechanism in a markdown table silently approved every regression in the run.

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "detail")]
pub enum OverrideSource {
    PrBody,
    Commit(String),
    Inline {
        file: String,
        line: usize,
    },
    /// The body of the merged pull request a pushed commit arrived through.
    MergedPrBody(u64),
}

impl std::fmt::Display for OverrideSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverrideSource::PrBody => write!(f, "PR body"),
            OverrideSource::Commit(sha) => write!(f, "commit {sha}"),
            OverrideSource::MergedPrBody(n) => write!(f, "merged pull request #{n} body"),
            OverrideSource::Inline { file, line } => write!(f, "inline {file}:{line}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideRecord {
    pub gate: String,
    /// The finding this override lifted (`gate/code`, `src/findings.rs`): the code the
    /// run would have reported without the directive. Absent for an inline marker that
    /// lifts no registered finding, and in reports written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub subject: String,
    pub directive: String,
    pub reason: String,
    pub source: OverrideSource,
    pub hidden: bool,
}

/// Whether a directive note is about the merged pull requests a push was resolved to
/// (`merged-pr-body: ...` from the lookup, `N directive(s) read from merged pull request
/// #N`): a note about the run's sources, reported once rather than on every gate.
pub fn is_merged_source_note(note: &str) -> bool {
    note.starts_with("merged-pr-body: ")
        || note.contains(" directive(s) read from merged pull request #")
}

/// A directive a run read that lifted no finding. Its reason is not carried: it is free
/// text the change's author wrote, and can echo secret material or a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnusedDirective {
    pub directive: String,
    pub source: OverrideSource,
    pub hidden: bool,
}

/// The directives in `directives` that no applied override came from, in order. An
/// override records the directive, reason and source it was granted from, so a
/// directive matches one when all three are equal (the name case-insensitively).
pub fn unused_directives<'a>(
    directives: &[ParsedDirective],
    applied: impl IntoIterator<Item = &'a OverrideRecord>,
) -> Vec<UnusedDirective> {
    let applied: Vec<&OverrideRecord> = applied.into_iter().collect();
    directives
        .iter()
        .filter(|d| {
            !applied.iter().any(|o| {
                o.directive.eq_ignore_ascii_case(&d.directive)
                    && o.reason == d.reason
                    && o.source == d.source
            })
        })
        .map(|d| UnusedDirective {
            directive: d.directive.clone(),
            source: d.source.clone(),
            hidden: d.hidden,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedDirective {
    pub directive: String,
    pub reason: String,
    pub source: OverrideSource,
    pub hidden: bool,
}

impl ParsedDirective {
    pub fn covers(&self, subject: &str) -> bool {
        reason_names(&self.reason, subject)
    }

    pub fn names_subject(&self, subject: &str) -> bool {
        reason_names_subject(&self.reason, subject)
    }
}

/// The override one of `directives` grants `gate` for `subject`: a directive named in
/// `names` whose reason names the subject. `lifts` is the finding the gate would report
/// without it; the record carries its code.
pub fn find_override(
    directives: &[ParsedDirective],
    gate: &str,
    lifts: &crate::findings::FindingKind,
    names: &[&str],
    subject: &str,
) -> Option<OverrideRecord> {
    let trimmed = subject.trim();
    if trimmed.is_empty() {
        return None;
    }
    for d in directives {
        if names.iter().any(|n| n.eq_ignore_ascii_case(&d.directive)) && d.covers(trimmed) {
            return Some(OverrideRecord {
                gate: gate.to_string(),
                code: Some(crate::findings::full_code(gate, lifts)),
                subject: trimmed.to_string(),
                directive: d.directive.clone(),
                reason: d.reason.clone(),
                source: d.source.clone(),
                hidden: d.hidden,
            });
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveSubjectKind {
    /// Test function name, suite path, or count ratchet (e.g. `test_foo`, `tests/e2e.rs`, `test-floor`)
    TestName,
    /// Third-party action reference (e.g. `actions/checkout`)
    ActionRef,
    /// Workflow job or step identifier (e.g. `security`, `test`, `step_name`)
    WorkflowJobOrStep,
    /// File path or directory prefix (e.g. `src/lib.rs`, `tests/golden/api.json`)
    FilePath,
    /// Specific rule name or diagnostic identifier (e.g. `dead_code`, `noqa`, `type: ignore`, `pull_request_target`)
    RuleName,
    /// Benchmark arm, file stem, or benchmark function name
    BenchmarkArm,
    /// Command line or subcommand invocation
    CommandName,
    /// Dependency package or crate name
    DependencyName,
    /// PR checklist section or item identifier
    ChecklistItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectiveSpec {
    pub canonical: &'static str,
    pub deprecated: Option<&'static str>,
    pub gate: &'static str,
    pub subject_kind: DirectiveSubjectKind,
    pub subject_doc: &'static str,
}

pub static DIRECTIVE_SPECS: &[DirectiveSpec] = &[
    DirectiveSpec {
        canonical: "removes",
        deprecated: Some("deletes"),
        gate: "deletion-rationale",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "File path, directory prefix, or test function name",
    },
    DirectiveSpec {
        canonical: "allow-assertion-drop",
        deprecated: None,
        gate: "assertion-reduction",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test function name, file path, or directory prefix",
    },
    DirectiveSpec {
        canonical: "allow-case-drop",
        deprecated: None,
        gate: "assertion-reduction",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test function name, file path, or directory prefix",
    },
    DirectiveSpec {
        canonical: "allow-ignore",
        deprecated: None,
        gate: "ignored-tests",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test function name",
    },
    DirectiveSpec {
        canonical: "allow-gate-weakening",
        deprecated: None,
        gate: "config-integrity",
        subject_kind: DirectiveSubjectKind::RuleName,
        subject_doc: "Gate id",
    },
    DirectiveSpec {
        canonical: "allow-toolchain-weakening",
        deprecated: None,
        gate: "toolchain-config",
        subject_kind: DirectiveSubjectKind::RuleName,
        subject_doc:
            "Option key path (e.g. `compilerOptions.strict`), its last segment, a build-file flag (`-Wno-error`) or what carries it (`CFLAGS`), or the file path",
    },
    DirectiveSpec {
        canonical: "allow-sandbox-widening",
        deprecated: None,
        gate: "sandbox-config",
        subject_kind: DirectiveSubjectKind::RuleName,
        subject_doc:
            "Setting key path (e.g. `permissions.defaultMode`), its last segment, or the file path",
    },
    DirectiveSpec {
        canonical: "allow-stub",
        deprecated: None,
        gate: "stub-bodies",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Function name, or the file path",
    },
    DirectiveSpec {
        canonical: "allow-swallow",
        deprecated: None,
        gate: "error-swallowing",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "File path, or `path:line` of the handler",
    },
    DirectiveSpec {
        canonical: "allow-agent-instructions",
        deprecated: None,
        gate: "instruction-smuggling",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "File path, or `path:line`",
    },
    DirectiveSpec {
        canonical: "allow-citation-metadata",
        deprecated: None,
        gate: "citation-metadata",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "`CITATION.cff` or `.zenodo.json`",
    },
    DirectiveSpec {
        canonical: "allow-commit-provenance",
        deprecated: None,
        gate: "commit-provenance",
        subject_kind: DirectiveSubjectKind::RuleName,
        subject_doc: "Commit SHA (7 or 40 characters)",
    },
    DirectiveSpec {
        canonical: "allow-build-hook",
        deprecated: None,
        gate: "build-hooks",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Hook name (`postinstall`) or file path",
    },
    DirectiveSpec {
        canonical: "allow-golden-update",
        deprecated: None,
        gate: "golden-output",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Snapshot/fixture file path or directory prefix",
    },
    DirectiveSpec {
        canonical: "allow-regression",
        deprecated: None,
        gate: "bench-regression",
        subject_kind: DirectiveSubjectKind::BenchmarkArm,
        subject_doc: "Benchmark name, file stem, or arm, plus non-empty rationale",
    },
    DirectiveSpec {
        canonical: "allow-command",
        deprecated: None,
        gate: "command",
        subject_kind: DirectiveSubjectKind::CommandName,
        subject_doc: "Subcommand or command line invocation, plus non-empty rationale",
    },
    DirectiveSpec {
        canonical: "allow-behavior-change",
        deprecated: None,
        gate: "command",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test identifier or function name, plus non-empty rationale",
    },
    DirectiveSpec {
        canonical: "allow-dependency",
        deprecated: None,
        gate: "dependency-delta",
        subject_kind: DirectiveSubjectKind::DependencyName,
        subject_doc: "Dependency package name or manifest path",
    },
    DirectiveSpec {
        canonical: "allow-test-shrink",
        deprecated: Some("allow-floor-drop"),
        gate: "test-floor",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test count delta, budget parameter, or suite name",
    },
    DirectiveSpec {
        canonical: "allow-ci-weakening",
        deprecated: Some("allow-unpinned-action"),
        gate: "ci-integrity",
        subject_kind: DirectiveSubjectKind::ActionRef,
        subject_doc: "Workflow path, job id, or security check rationale",
    },
    DirectiveSpec {
        canonical: "allow-nul",
        deprecated: Some("allow-nul-byte"),
        // `assertion-reduction/nul-byte-added`: the finding's code names this gate.
        gate: "assertion-reduction",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Corrupt or NUL-byte fixture file path",
    },
    DirectiveSpec {
        canonical: "secrets-argv-ok",
        deprecated: None,
        gate: "shell-secrets",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Shell script path or CLI command line",
    },
    DirectiveSpec {
        canonical: "no-issue",
        deprecated: Some("discipline:no-issue"),
        gate: "issue-link",
        subject_kind: DirectiveSubjectKind::RuleName,
        subject_doc: "PR or commit justification for omitted tracking issue",
    },
    DirectiveSpec {
        canonical: "allow-provenance",
        deprecated: Some("allow-unpaired-figures"),
        gate: "provenance-tags",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Unmeasured figure, claim, or doc file path",
    },
    DirectiveSpec {
        canonical: "allow-archive-leak",
        deprecated: None,
        gate: "archive-contents",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Archive file path or leaked entry name",
    },
    DirectiveSpec {
        canonical: "allow-manifest-drift",
        deprecated: None,
        gate: "manifest-sync",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Manifest path or package field name",
    },
    DirectiveSpec {
        canonical: "allow-version-mismatch",
        deprecated: None,
        gate: "version-lockstep",
        subject_kind: DirectiveSubjectKind::DependencyName,
        subject_doc: "Mismatched crate name or manifest path",
    },
    DirectiveSpec {
        canonical: "allow-scope",
        deprecated: Some("allow-scope-confinement"),
        gate: "scope-confinement",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Out-of-scope file path or module prefix",
    },
    DirectiveSpec {
        canonical: "allow-suppression",
        deprecated: Some("allow-suppression-delta"),
        gate: "suppression-delta",
        subject_kind: DirectiveSubjectKind::RuleName,
        subject_doc:
            "Specific suppression rule (`dead_code`, `noqa`, `type: ignore`) and/or file path",
    },
    DirectiveSpec {
        canonical: "allow-pr-checklist",
        deprecated: Some("allow-checklist"),
        gate: "pr-checklist",
        subject_kind: DirectiveSubjectKind::ChecklistItem,
        subject_doc: "PR checklist item text or section",
    },
    DirectiveSpec {
        canonical: "allow-unsafe",
        deprecated: Some("allow-unsafe-budget"),
        gate: "unsafe-budget",
        subject_kind: DirectiveSubjectKind::FilePath,
        subject_doc: "Rust file path, function name, or module",
    },
    DirectiveSpec {
        canonical: "allow-msrv",
        deprecated: None,
        gate: "msrv",
        subject_kind: DirectiveSubjectKind::DependencyName,
        subject_doc: "Crate name or MSRV error diagnostic",
    },
    DirectiveSpec {
        canonical: "allow-miri",
        deprecated: None,
        gate: "miri",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test name or unsupported Miri operation",
    },
    DirectiveSpec {
        canonical: "allow-sanitizers",
        deprecated: None,
        gate: "sanitizers",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Test or binary name with memory check rationale",
    },
    DirectiveSpec {
        canonical: "allow-vacuous-test",
        deprecated: None,
        gate: "vacuous-tests",
        subject_kind: DirectiveSubjectKind::TestName,
        subject_doc: "Name of the new test the vacuous-tests finding is on",
    },
];

/// The 45 named directives recognized by discipline (35 canonical + 10 deprecated aliases).
pub const KNOWN_DIRECTIVES: &[&str] = &[
    // 35 Canonical
    "removes",
    "allow-assertion-drop",
    "allow-case-drop",
    "allow-ignore",
    "allow-gate-weakening",
    "allow-toolchain-weakening",
    "allow-sandbox-widening",
    "allow-stub",
    "allow-swallow",
    "allow-agent-instructions",
    "allow-commit-provenance",
    "allow-citation-metadata",
    "allow-build-hook",
    "allow-golden-update",
    "allow-regression",
    "allow-command",
    "allow-behavior-change",
    "allow-dependency",
    "allow-test-shrink",
    "allow-ci-weakening",
    "allow-nul",
    "secrets-argv-ok",
    "no-issue",
    "allow-provenance",
    "allow-archive-leak",
    "allow-manifest-drift",
    "allow-version-mismatch",
    "allow-scope",
    "allow-suppression",
    "allow-pr-checklist",
    "allow-unsafe",
    "allow-msrv",
    "allow-miri",
    "allow-sanitizers",
    "allow-vacuous-test",
    // 10 Deprecated aliases
    "deletes",
    "allow-floor-drop",
    "allow-unpinned-action",
    "allow-nul-byte",
    "discipline:no-issue",
    "allow-unpaired-figures",
    "allow-scope-confinement",
    "allow-suppression-delta",
    "allow-checklist",
    "allow-unsafe-budget",
];

pub const REMOVES: &[&str] = &[
    "removes",
    "deletes",
    "discipline:allow(deletion-rationale)",
    "allow(deletion-rationale)",
];
pub const ALLOW_ASSERTION_DROP: &[&str] = &[
    "allow-assertion-drop",
    "discipline:allow(assertion-reduction)",
    "allow(assertion-reduction)",
];
pub const ALLOW_CASE_DROP: &[&str] = &["allow-case-drop"];
pub const ALLOW_IGNORE: &[&str] = &[
    "allow-ignore",
    "discipline:allow(ignored-tests)",
    "allow(ignored-tests)",
];
pub const ALLOW_GATE_WEAKENING: &[&str] = &[
    "allow-gate-weakening",
    "discipline:allow(config-integrity)",
    "allow(config-integrity)",
];
pub const ALLOW_TOOLCHAIN_WEAKENING: &[&str] = &[
    "allow-toolchain-weakening",
    "discipline:allow(toolchain-config)",
    "allow(toolchain-config)",
];
pub const ALLOW_SANDBOX_WIDENING: &[&str] = &[
    "allow-sandbox-widening",
    "discipline:allow(sandbox-config)",
    "allow(sandbox-config)",
];
pub const ALLOW_STUB: &[&str] = &[
    "allow-stub",
    "discipline:allow(stub-bodies)",
    "allow(stub-bodies)",
];
pub const ALLOW_SWALLOW: &[&str] = &[
    "allow-swallow",
    "discipline:allow(error-swallowing)",
    "allow(error-swallowing)",
];
pub const ALLOW_SMUGGLING: &[&str] = &[
    "allow-agent-instructions",
    "discipline:allow(instruction-smuggling)",
    "allow(instruction-smuggling)",
];
pub const ALLOW_CITATION_METADATA: &[&str] = &[
    "allow-citation-metadata",
    "discipline:allow(citation-metadata)",
    "allow(citation-metadata)",
];
pub const ALLOW_COMMIT_PROVENANCE: &[&str] = &[
    "allow-commit-provenance",
    "discipline:allow(commit-provenance)",
    "allow(commit-provenance)",
];
pub const ALLOW_BUILD_HOOK: &[&str] = &[
    "allow-build-hook",
    "discipline:allow(build-hooks)",
    "allow(build-hooks)",
];
pub const ALLOW_GOLDEN_UPDATE: &[&str] = &[
    "allow-golden-update",
    "discipline:allow(golden-output)",
    "allow(golden-output)",
];
pub const ALLOW_NUL: &[&str] = &[
    "allow-nul",
    "allow-nul-byte",
    "discipline:allow(vacuous-tests)",
    "allow(vacuous-tests)",
];

pub const ALLOW_REGRESSION: &[&str] = &[
    "allow-regression",
    "discipline:allow(bench-regression)",
    "allow(bench-regression)",
];

pub const ALLOW_COMMAND: &[&str] = &[
    "allow-command",
    "discipline:allow(command)",
    "allow(command)",
];

pub const ALLOW_BEHAVIOR_CHANGE: &[&str] = &[
    "allow-behavior-change",
    "discipline:allow(behavior-change)",
    "allow(behavior-change)",
];

pub const ALLOW_DEPENDENCY: &[&str] = &[
    "allow-dependency",
    "discipline:allow(dependency-delta)",
    "allow(dependency-delta)",
];

pub const ALLOW_TEST_SHRINK: &[&str] = &[
    "allow-test-shrink",
    "allow-floor-drop",
    "discipline:allow(test-budget)",
    "allow(test-budget)",
    "discipline:allow(test-floor)",
    "allow(test-floor)",
];

pub const ALLOW_CI_WEAKENING: &[&str] = &[
    "allow-ci-weakening",
    "allow-unpinned-action",
    "discipline:allow(ci-integrity)",
    "allow(ci-integrity)",
];

pub const SECRETS_ARGV_OK: &[&str] = &[
    "secrets-argv-ok",
    "discipline:allow(shell-secrets)",
    "allow(shell-secrets)",
];

pub const NO_ISSUE: &[&str] = &[
    "no-issue",
    "discipline:no-issue",
    "discipline:allow(issue-link)",
    "allow(issue-link)",
];

pub const ALLOW_PROVENANCE: &[&str] = &[
    "allow-provenance",
    "allow-unpaired-figures",
    "discipline:allow(provenance-tags)",
    "allow(provenance-tags)",
];

pub const ALLOW_ARCHIVE_LEAK: &[&str] = &[
    "allow-archive-leak",
    "discipline:allow(archive-contents)",
    "allow(archive-contents)",
];

pub const ALLOW_MANIFEST_DRIFT: &[&str] = &[
    "allow-manifest-drift",
    "discipline:allow(manifest-sync)",
    "allow(manifest-sync)",
];

pub const ALLOW_VERSION_MISMATCH: &[&str] = &[
    "allow-version-mismatch",
    "discipline:allow(version-lockstep)",
    "allow(version-lockstep)",
];

pub const ALLOW_SCOPE: &[&str] = &[
    "allow-scope",
    "allow-scope-confinement",
    "discipline:allow(scope-confinement)",
    "allow(scope-confinement)",
];

pub const ALLOW_SUPPRESSION: &[&str] = &[
    "allow-suppression",
    "allow-suppression-delta",
    "discipline:allow(suppression-delta)",
    "allow(suppression-delta)",
];

pub const ALLOW_PR_CHECKLIST: &[&str] = &[
    "allow-pr-checklist",
    "allow-checklist",
    "discipline:allow(pr-checklist)",
    "allow(pr-checklist)",
];

pub const ALLOW_UNSAFE: &[&str] = &[
    "allow-unsafe",
    "allow-unsafe-budget",
    "discipline:allow(unsafe-budget)",
    "allow(unsafe-budget)",
];

pub const ALLOW_MSRV: &[&str] = &["allow-msrv", "discipline:allow(msrv)", "allow(msrv)"];

pub const ALLOW_MIRI: &[&str] = &["allow-miri", "discipline:allow(miri)", "allow(miri)"];

pub const ALLOW_SANITIZERS: &[&str] = &[
    "allow-sanitizers",
    "discipline:allow(sanitizers)",
    "allow(sanitizers)",
];

/// Lifts a `vacuous-tests` finding on one new test. The namespaced forms are shared with
/// [`ALLOW_NUL`], whose subject is a file path, never a test name.
pub const ALLOW_VACUOUS_TEST: &[&str] = &[
    "allow-vacuous-test",
    "discipline:allow(vacuous-tests)",
    "allow(vacuous-tests)",
];

/// Returns the slice of aliases accepted for a given directive name.
pub fn names_for_directive(name: &str) -> &'static [&'static str] {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "removes" | "deletes" => REMOVES,
        "allow-assertion-drop" => ALLOW_ASSERTION_DROP,
        "allow-case-drop" => ALLOW_CASE_DROP,
        "allow-ignore" => ALLOW_IGNORE,
        "allow-gate-weakening" => ALLOW_GATE_WEAKENING,
        "allow-golden-update" => ALLOW_GOLDEN_UPDATE,
        "allow-toolchain-weakening" => ALLOW_TOOLCHAIN_WEAKENING,
        "allow-sandbox-widening" => ALLOW_SANDBOX_WIDENING,
        "allow-stub" => ALLOW_STUB,
        "allow-swallow" => ALLOW_SWALLOW,
        "allow-agent-instructions" => ALLOW_SMUGGLING,
        "allow-commit-provenance" => ALLOW_COMMIT_PROVENANCE,
        "allow-citation-metadata" => ALLOW_CITATION_METADATA,
        "allow-build-hook" => ALLOW_BUILD_HOOK,
        "allow-regression" => ALLOW_REGRESSION,
        "allow-command" => ALLOW_COMMAND,
        "allow-behavior-change" => ALLOW_BEHAVIOR_CHANGE,
        "allow-dependency" => ALLOW_DEPENDENCY,
        "allow-test-shrink" | "allow-floor-drop" => ALLOW_TEST_SHRINK,
        "allow-ci-weakening" | "allow-unpinned-action" => ALLOW_CI_WEAKENING,
        "allow-nul" | "allow-nul-byte" => ALLOW_NUL,
        "secrets-argv-ok" => SECRETS_ARGV_OK,
        "no-issue" | "discipline:no-issue" => NO_ISSUE,
        "allow-provenance" | "allow-unpaired-figures" => ALLOW_PROVENANCE,
        "allow-archive-leak" => ALLOW_ARCHIVE_LEAK,
        "allow-manifest-drift" => ALLOW_MANIFEST_DRIFT,
        "allow-version-mismatch" => ALLOW_VERSION_MISMATCH,
        "allow-scope" | "allow-scope-confinement" => ALLOW_SCOPE,
        "allow-suppression" | "allow-suppression-delta" => ALLOW_SUPPRESSION,
        "allow-pr-checklist" | "allow-checklist" => ALLOW_PR_CHECKLIST,
        "allow-unsafe" | "allow-unsafe-budget" => ALLOW_UNSAFE,
        "allow-msrv" => ALLOW_MSRV,
        "allow-miri" => ALLOW_MIRI,
        "allow-sanitizers" => ALLOW_SANITIZERS,
        "allow-vacuous-test" => ALLOW_VACUOUS_TEST,
        _ => &[],
    }
}

/// Returns the specification for the given directive name, if known.
pub fn spec_for_directive(name: &str) -> Option<&'static DirectiveSpec> {
    let lower = name.to_ascii_lowercase();
    DIRECTIVE_SPECS.iter().find(|s| {
        s.canonical.eq_ignore_ascii_case(&lower)
            || s.deprecated.is_some_and(|d| d.eq_ignore_ascii_case(&lower))
    })
}

pub const ALL_DIRECTIVE_NAMES: &[&str] = &[
    // 35 Canonical
    "removes",
    "allow-assertion-drop",
    "allow-case-drop",
    "allow-ignore",
    "allow-gate-weakening",
    "allow-toolchain-weakening",
    "allow-sandbox-widening",
    "allow-stub",
    "allow-swallow",
    "allow-agent-instructions",
    "allow-commit-provenance",
    "allow-citation-metadata",
    "allow-build-hook",
    "allow-golden-update",
    "allow-regression",
    "allow-command",
    "allow-behavior-change",
    "allow-dependency",
    "allow-test-shrink",
    "allow-ci-weakening",
    "allow-nul",
    "secrets-argv-ok",
    "no-issue",
    "allow-provenance",
    "allow-archive-leak",
    "allow-manifest-drift",
    "allow-version-mismatch",
    "allow-scope",
    "allow-suppression",
    "allow-pr-checklist",
    "allow-unsafe",
    "allow-msrv",
    "allow-miri",
    "allow-sanitizers",
    "allow-vacuous-test",
    // 10 Deprecated aliases
    "deletes",
    "allow-floor-drop",
    "allow-unpinned-action",
    "allow-nul-byte",
    "discipline:no-issue",
    "allow-unpaired-figures",
    "allow-scope-confinement",
    "allow-suppression-delta",
    "allow-checklist",
    "allow-unsafe-budget",
    // Namespaced forms
    "discipline:allow(deletion-rationale)",
    "allow(deletion-rationale)",
    "discipline:allow(assertion-reduction)",
    "allow(assertion-reduction)",
    "discipline:allow(ignored-tests)",
    "allow(ignored-tests)",
    "discipline:allow(config-integrity)",
    "allow(config-integrity)",
    "discipline:allow(toolchain-config)",
    "allow(toolchain-config)",
    "discipline:allow(sandbox-config)",
    "allow(sandbox-config)",
    "discipline:allow(stub-bodies)",
    "allow(stub-bodies)",
    "discipline:allow(error-swallowing)",
    "allow(error-swallowing)",
    "discipline:allow(instruction-smuggling)",
    "allow(instruction-smuggling)",
    "discipline:allow(commit-provenance)",
    "allow(commit-provenance)",
    "discipline:allow(citation-metadata)",
    "allow(citation-metadata)",
    "discipline:allow(build-hooks)",
    "allow(build-hooks)",
    "discipline:allow(golden-output)",
    "allow(golden-output)",
    "discipline:allow(bench-regression)",
    "allow(bench-regression)",
    "discipline:allow(command)",
    "allow(command)",
    "discipline:allow(behavior-change)",
    "allow(behavior-change)",
    "discipline:allow(dependency-delta)",
    "allow(dependency-delta)",
    "discipline:allow(test-budget)",
    "allow(test-budget)",
    "discipline:allow(test-floor)",
    "allow(test-floor)",
    "discipline:allow(ci-integrity)",
    "allow(ci-integrity)",
    "discipline:allow(vacuous-tests)",
    "allow(vacuous-tests)",
    "discipline:allow(shell-secrets)",
    "allow(shell-secrets)",
    "discipline:allow(issue-link)",
    "allow(issue-link)",
    "discipline:allow(provenance-tags)",
    "allow(provenance-tags)",
    "discipline:allow(archive-contents)",
    "allow(archive-contents)",
    "discipline:allow(manifest-sync)",
    "allow(manifest-sync)",
    "discipline:allow(version-lockstep)",
    "allow(version-lockstep)",
    "discipline:allow(scope-confinement)",
    "allow(scope-confinement)",
    "discipline:allow(suppression-delta)",
    "allow(suppression-delta)",
    "discipline:allow(pr-checklist)",
    "allow(pr-checklist)",
    "discipline:allow(unsafe-budget)",
    "allow(unsafe-budget)",
    "discipline:allow(msrv)",
    "allow(msrv)",
    "discipline:allow(miri)",
    "allow(miri)",
    "discipline:allow(sanitizers)",
    "allow(sanitizers)",
];

const PLACEHOLDERS: &[&str] = &[
    "todo",
    "tbd",
    "none",
    "n/a",
    "na",
    "reason",
    "why",
    "...",
    "xxx",
    "fixme",
    "-",
    "ok",
    "temp",
    "dummy",
    "null",
    "placeholder",
    "asdf",
];

/// Parses all directives from `text` matching any names in `names`.
pub fn parse_directives_with_names(
    text: &str,
    names: &[&str],
    source: OverrideSource,
) -> Vec<ParsedDirective> {
    directive_lines_with_names(text, names, source)
        .into_iter()
        .map(|(_, d)| d)
        .collect()
}

/// Every directive in `text` that parses, with the index of the line it is on: a line in
/// a code fence, the subject line of a commit message, and one whose reason is empty or a
/// placeholder are not directives and are not listed.
pub fn directive_lines(text: &str, source: OverrideSource) -> Vec<(usize, ParsedDirective)> {
    directive_lines_with_names(text, ALL_DIRECTIVE_NAMES, source)
}

fn directive_lines_with_names(
    text: &str,
    names: &[&str],
    source: OverrideSource,
) -> Vec<(usize, ParsedDirective)> {
    let patterns = names
        .iter()
        .map(|n| {
            let esc = regex::escape(n);
            if n.ends_with(')') {
                format!("{esc}(?::|[ \\t])")
            } else {
                format!("{esc}:")
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    let re = Regex::new(&format!(
        r"(?i)^[ \t]*(<!--[ \t]*)?(?:discipline:[ \t]+)?({patterns})[ \t]*(.*)$"
    ))
    .expect("directive regex is static");

    let mut directives = Vec::new();
    // The open fence's character and length: CommonMark closes it only with a fence of the
    // same character at least as long, on a line of its own.
    let mut fence: Option<(char, usize)> = None;
    let mut in_html_comment = false;
    let is_commit = matches!(source, OverrideSource::Commit(_));
    for (line_idx, line) in text.lines().enumerate() {
        if line_idx == 0 && is_commit {
            // Directives belong in the commit body, never in the subject line (Q6).
            continue;
        }
        let trimmed = line.trim_start();
        let marker = fence_marker(line);
        match (fence, marker) {
            (None, Some(m)) => {
                fence = Some(m);
                continue;
            }
            (Some((c, n)), Some((mc, mn)))
                if c == mc && mn >= n && trimmed.trim_start_matches(mc).trim().is_empty() =>
            {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, None) => {}
        }
        // An indented code block (four columns, a tab counting as four) is quoted code (F8).
        if indent_columns(line) >= 4 {
            continue;
        }

        let line_has_open_comment =
            !in_html_comment && line.contains("<!--") && !line.contains("-->");
        let line_closes_comment = in_html_comment && line.contains("-->");

        if let Some(caps) = re.captures(line) {
            let reason = clean_reason(&caps[3]);
            if !is_placeholder(&reason) {
                let directive_str = caps[2].trim_end_matches(':').trim().to_string();
                directives.push((
                    line_idx,
                    ParsedDirective {
                        directive: directive_str,
                        reason,
                        source: source.clone(),
                        hidden: in_html_comment || caps.get(1).is_some() || line_has_open_comment,
                    },
                ));
            }
        }

        if line_has_open_comment {
            in_html_comment = true;
        } else if line_closes_comment {
            in_html_comment = false;
        }
    }
    directives
}

/// The fence a line opens or closes: three or more backticks or tildes after at most three
/// columns of indentation, as `(character, length)`.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    if indent_columns(line) > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let c = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = trimmed.chars().take_while(|x| *x == c).count();
    (n >= 3).then_some((c, n))
}

/// Leading indentation in columns, a tab counting as four (CommonMark's tab stop).
fn indent_columns(line: &str) -> usize {
    let mut cols = 0;
    for ch in line.chars() {
        match ch {
            ' ' => cols += 1,
            '\t' => cols += 4 - cols % 4,
            _ => break,
        }
    }
    cols
}

/// Returns the first directive name found in `text` (e.g. commit subject line or PR title), if any.
/// Directives belong in the body, never in the subject line (Q6).
pub fn find_directive_in_subject(text: &str) -> Option<String> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        let patterns = ALL_DIRECTIVE_NAMES
            .iter()
            .map(|n| {
                let esc = regex::escape(n);
                if n.ends_with(')') {
                    format!("{esc}(?::|[ \\t])")
                } else {
                    format!("{esc}:")
                }
            })
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!(r"(?i)(?:discipline:[ \t]+)?({patterns})"))
            .expect("directive subject regex is static")
    });

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(caps) = re.captures(trimmed) {
        let dir = caps
            .get(1)
            .map(|m| m.as_str().trim_end_matches(':').trim().to_string())
            .unwrap_or_else(|| caps[0].to_string());
        return Some(dir);
    }
    None
}

/// Parses all directives in `text` against all known directive names.
pub fn parse_directives(text: &str, source: OverrideSource) -> Vec<ParsedDirective> {
    parse_directives_with_names(text, ALL_DIRECTIVE_NAMES, source)
}

/// Extracts active valid directives according to the policy in `policy`.
/// Directives from disallowed sources or hidden when `allow_hidden` is false
/// are dropped, and explanatory notes are emitted.
pub fn extract_directives(
    pr_body: Option<&str>,
    commits: &[(String, String)],
    policy: &crate::config::DirectivesConfig,
) -> (Vec<ParsedDirective>, Vec<String>) {
    let mut active = Vec::new();
    let mut notes = Vec::new();

    let pr_body_allowed = policy.sources.iter().any(|s| s == "pr-body");
    let commits_allowed = policy.sources.iter().any(|s| s == "commits");

    if let Some(body) = pr_body {
        let parsed = parse_directives(body, OverrideSource::PrBody);
        if pr_body_allowed {
            for d in parsed {
                if d.hidden && !policy.allow_hidden {
                    notes.push(format!(
                        "hidden directive `{}` in PR body ignored (directives.allow_hidden is false)",
                        d.directive
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push("PR-body directives are disabled by policy; ignored".to_string());
        }
    }

    for (oid, msg) in commits {
        let parsed = parse_directives(msg, OverrideSource::Commit(oid.clone()));
        if commits_allowed {
            for d in parsed {
                if d.hidden && !policy.allow_hidden {
                    notes.push(format!(
                        "hidden directive `{}` in commit {oid} ignored (directives.allow_hidden is false)",
                        d.directive
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push(format!(
                "commit-message directives are disabled by policy; ignored directive from commit {oid}"
            ));
        }
    }

    (active, notes)
}

/// Extracts active valid directives taking into account both global policy and per-gate settings.
/// A merged pull request's body, read for a pushed commit (`merged-pr-body`).
#[derive(Debug, Clone)]
pub struct MergedBody {
    pub number: u64,
    pub author: String,
    pub body: String,
}

pub fn extract_directives_for_config(
    pr_body: Option<&str>,
    commits: &[(String, String)],
    config: &crate::config::DisciplineConfig,
) -> (Vec<ParsedDirective>, Vec<String>) {
    extract_directives_with_merged(pr_body, commits, &[], config)
}

/// As [`extract_directives_for_config`], plus the bodies of the merged pull requests the
/// pushed commits arrived through. A merged body is trusted like a PR body (hidden
/// directives, scoped subjects), and, when `allowed_override_actors` is set, only when
/// its author is listed.
pub fn extract_directives_with_merged(
    pr_body: Option<&str>,
    commits: &[(String, String)],
    merged: &[MergedBody],
    config: &crate::config::DisciplineConfig,
) -> (Vec<ParsedDirective>, Vec<String>) {
    let policy = &config.directives;
    let mut active = Vec::new();
    let mut notes = Vec::new();

    let pr_body_allowed = policy.sources.iter().any(|s| s == "pr-body");
    let commits_allowed = policy.sources.iter().any(|s| s == "commits");
    let merged_allowed = policy.sources.iter().any(|s| s == "merged-pr-body");

    let is_hidden_allowed = |d: &ParsedDirective| -> bool {
        if policy.allow_hidden {
            return true;
        }
        if REMOVES.iter().any(|n| n.eq_ignore_ascii_case(&d.directive)) {
            if let Some(gate_hidden) = config.gates.deletion_rationale.allow_hidden {
                return gate_hidden;
            }
        }
        false
    };

    if let Some(body) = pr_body {
        let parsed = parse_directives(body, OverrideSource::PrBody);
        if pr_body_allowed {
            for d in parsed {
                if d.hidden && !is_hidden_allowed(&d) {
                    notes.push(format!(
                        "hidden directive `{}` in PR body ignored (directives.allow_hidden is false)",
                        d.directive
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push("PR-body directives are disabled by policy; ignored".to_string());
        }
    }

    for (oid, msg) in commits {
        let parsed = parse_directives(msg, OverrideSource::Commit(oid.clone()));
        if commits_allowed {
            for d in parsed {
                if d.hidden && !is_hidden_allowed(&d) {
                    notes.push(format!(
                        "hidden directive `{}` in commit {oid} ignored (directives.allow_hidden is false)",
                        d.directive
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push(format!(
                "commit-message directives are disabled by policy; ignored directive from commit {oid}"
            ));
        }
    }

    for m in merged {
        let parsed = parse_directives(&m.body, OverrideSource::MergedPrBody(m.number));
        if !merged_allowed {
            if !parsed.is_empty() {
                notes.push(format!(
                    "merged pull request #{} body directives are disabled by policy; ignored",
                    m.number
                ));
            }
            continue;
        }
        let author_ok = policy.allowed_override_actors.is_empty()
            || policy
                .allowed_override_actors
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&m.author));
        if !author_ok {
            if !parsed.is_empty() {
                notes.push(format!(
                    "directives in merged pull request #{} ignored: its author is not in `allowed_override_actors`",
                    m.number
                ));
            }
            continue;
        }
        let mut n = 0;
        for d in parsed {
            if d.hidden && !is_hidden_allowed(&d) {
                notes.push(format!(
                    "hidden directive `{}` in merged pull request #{} ignored (directives.allow_hidden is false)",
                    d.directive, m.number
                ));
            } else {
                n += 1;
                active.push(d);
            }
        }
        notes.push(format!(
            "{n} directive(s) read from merged pull request #{} (author {})",
            m.number, m.author
        ));
    }

    (active, notes)
}

/// Markers written as a YAML comment in a workflow file rather than in a pull request
/// body. `discipline doctor` reads them; no gate does.
pub const WORKFLOW_MARKERS: &[&str] = &[ADVISORY_MARKER];

/// `# discipline:advisory <reason>`: an action step with `advisory: true` is a shadow
/// run on purpose. It changes how `discipline doctor` reports that step, nothing else.
pub const ADVISORY_MARKER: &str = "discipline:advisory";

/// The reason of a workflow marker `name` in `comment`, the text after a YAML `#`.
/// The marker must begin the comment; a reason that is empty or a placeholder
/// (`<reason>`, `TODO`, ...) arms nothing, as for a directive.
pub fn workflow_marker_reason(comment: &str, name: &str) -> Option<String> {
    let rest = comment.trim_start();
    let head = rest.get(..name.len())?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    let tail = &rest[name.len()..];
    if !(tail.is_empty() || tail.starts_with([' ', '\t'])) {
        return None;
    }
    let reason = clean_reason(tail);
    (!is_placeholder(&reason)).then_some(reason)
}

/// Reasons of every well-formed directive named in `names` found in `text`.
pub fn directive_reasons(text: &str, names: &[&str]) -> Vec<String> {
    parse_directives_with_names(text, names, OverrideSource::PrBody)
        .into_iter()
        .map(|d| d.reason)
        .collect()
}

/// True when some directive's reason names `subject` (or, for paths, a
/// directory prefix of it or its file name).
pub fn covers(reasons: &[String], subject: &str) -> bool {
    reasons.iter().any(|r| reason_names(r, subject))
}

fn reason_names(reason: &str, subject: &str) -> bool {
    let is_token_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@');
    let trimmed_subject = subject.trim();

    if !trimmed_subject.is_empty() {
        // 1. Quoted subject anywhere in the reason: "name", 'name', or `name`.
        for quote in ['"', '\'', '`'] {
            let quoted = format!("{quote}{trimmed_subject}{quote}");
            for (idx, _) in reason.match_indices(&quoted) {
                let remainder = format!("{} {}", &reason[..idx], &reason[idx + quoted.len()..]);
                if is_valid_rationale(&remainder) {
                    return true;
                }
            }
        }

        // 2. Multi-word subject containing ':' (e.g. `type: ignore`) anywhere in the reason, bounded by non-token boundary.
        if trimmed_subject.contains(':') {
            for (idx, _) in reason.match_indices(trimmed_subject) {
                let prev_ok = if idx == 0 {
                    true
                } else {
                    let prev_char = reason[..idx].chars().next_back().unwrap();
                    !is_token_char(prev_char) && prev_char != ':'
                };
                let end_idx = idx + trimmed_subject.len();
                let next_ok = if end_idx == reason.len() {
                    true
                } else {
                    let next_char = reason[end_idx..].chars().next().unwrap();
                    !is_token_char(next_char) && next_char != ':'
                };
                if prev_ok && next_ok {
                    let remainder = format!("{} {}", &reason[..idx], &reason[end_idx..]);
                    if is_valid_rationale(&remainder) {
                        return true;
                    }
                }
            }
        }

        // 3. Subject at the beginning of the reason (e.g. `allow-ignore: my test name <reason>` or `allow-unpinned-action: actions/checkout@v4 <reason>`).
        let unquoted_reason = reason.trim_start_matches(['"', '\'', '`']);
        if let Some(rest) = unquoted_reason.strip_prefix(trimmed_subject) {
            let rest_after_quote = rest.trim_start_matches(['"', '\'', '`']);
            if rest_after_quote.starts_with(|c: char| !is_token_char(c) || c == ':' || c == '@')
                && is_valid_rationale(rest_after_quote)
            {
                return true;
            }
        }
    }

    let raw_tokens: Vec<&str> = reason
        .split(|c: char| !is_token_char(c))
        .filter(|t| !t.is_empty())
        .collect();
    let file_name = subject.rsplit('/').next().unwrap_or(subject);
    for raw_token in &raw_tokens {
        let has_slash = raw_token.contains('/');
        let trimmed_leading = raw_token.strip_prefix("./").unwrap_or(raw_token);
        let token = trimmed_leading.trim_end_matches('.').trim_matches('/');
        if token.is_empty() {
            continue;
        }
        let matched = if token == subject || token == file_name {
            true
        } else if has_slash && subject.contains('/') && subject.starts_with(&format!("{token}/")) {
            // Directory prefix: only when written with a slash (e.g. `tests/legacy` or `tests/`).
            // A bare word like `tests` in ordinary prose never acts as a directory prefix.
            true
        } else {
            false
        };

        if matched {
            for (idx, _) in reason.match_indices(raw_token) {
                let prev_ok = if idx == 0 {
                    true
                } else {
                    let prev_char = reason[..idx].chars().next_back().unwrap();
                    !is_token_char(prev_char)
                };
                let end_idx = idx + raw_token.len();
                let next_ok = if end_idx == reason.len() {
                    true
                } else {
                    let next_char = reason[end_idx..].chars().next().unwrap();
                    !is_token_char(next_char)
                };
                if prev_ok && next_ok {
                    let remainder = format!("{} {}", &reason[..idx], &reason[end_idx..]);
                    if is_valid_rationale(&remainder) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub(crate) fn reason_names_subject(reason: &str, subject: &str) -> bool {
    let is_token_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@');
    let trimmed_subject = subject.trim();

    if !trimmed_subject.is_empty() {
        for quote in ['"', '\'', '`'] {
            let quoted = format!("{quote}{trimmed_subject}{quote}");
            if reason.contains(&quoted) {
                return true;
            }
        }

        if trimmed_subject.contains(':') {
            for (idx, _) in reason.match_indices(trimmed_subject) {
                let prev_ok = if idx == 0 {
                    true
                } else {
                    let prev_char = reason[..idx].chars().next_back().unwrap();
                    !is_token_char(prev_char) && prev_char != ':'
                };
                let end_idx = idx + trimmed_subject.len();
                let next_ok = if end_idx == reason.len() {
                    true
                } else {
                    let next_char = reason[end_idx..].chars().next().unwrap();
                    !is_token_char(next_char) && next_char != ':'
                };
                if prev_ok && next_ok {
                    return true;
                }
            }
        }

        let unquoted_reason = reason.trim_start_matches(['"', '\'', '`']);
        if let Some(rest) = unquoted_reason.strip_prefix(trimmed_subject) {
            let rest_after_quote = rest.trim_start_matches(['"', '\'', '`']);
            if rest_after_quote.is_empty()
                || rest_after_quote.starts_with(|c: char| !is_token_char(c) || c == ':' || c == '@')
            {
                return true;
            }
        }
    }

    let raw_tokens: Vec<&str> = reason
        .split(|c: char| !is_token_char(c))
        .filter(|t| !t.is_empty())
        .collect();
    let file_name = subject.rsplit('/').next().unwrap_or(subject);
    for raw_token in &raw_tokens {
        let has_slash = raw_token.contains('/');
        let trimmed_leading = raw_token.strip_prefix("./").unwrap_or(raw_token);
        let token = trimmed_leading.trim_end_matches('.').trim_matches('/');
        if token.is_empty() {
            continue;
        }
        if token == subject || token == file_name {
            return true;
        }
        if has_slash && subject.contains('/') && subject.starts_with(&format!("{token}/")) {
            return true;
        }
    }
    false
}

fn clean_reason(raw: &str) -> String {
    let mut r = raw.trim();
    // `--!>` also closes an HTML comment; left on the reason it once let a
    // placeholder through.
    for closer in ["--!>", "-->"] {
        if let Some(stripped) = r.strip_suffix(closer) {
            r = stripped.trim_end();
        }
    }
    r.to_string()
}

fn is_invisible(c: char) -> bool {
    crate::guards::instruction_smuggling::invisible_class(c).is_some()
}

fn normalize_placeholder_token(token: &str) -> String {
    crate::guards::instruction_smuggling::fold(token)
}

pub fn is_valid_rationale(rest: &str) -> bool {
    let cleaned = rest
        .trim()
        .trim_matches(|c: char| {
            matches!(
                c,
                ':' | '-' | ',' | ';' | '.' | '?' | '!' | '"' | '\'' | '`'
            )
        })
        .trim();
    if cleaned.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
        return false;
    }
    !is_placeholder(cleaned)
}

fn is_placeholder_word(w: &str) -> bool {
    let cw = w.trim_matches(|c: char| {
        matches!(
            c,
            ':' | '-' | ',' | ';' | '.' | '?' | '!' | '"' | '\'' | '`'
        )
    });
    let ncw = normalize_placeholder_token(cw);
    let ncw_trimmed = ncw.trim_matches(|c: char| {
        matches!(
            c,
            ':' | '-' | ',' | ';' | '.' | '?' | '!' | '"' | '\'' | '`'
        )
    });
    ncw_trimmed.is_empty()
        || ncw_trimmed.chars().filter(|c| c.is_alphanumeric()).count() == 0
        || (w.starts_with('<') && w.ends_with('>'))
        || PLACEHOLDERS.contains(&ncw_trimmed)
}

fn is_segmented_placeholder(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let n = s.len();
    let mut dp = vec![false; n + 1];
    dp[0] = true;
    for i in 1..=n {
        // A segment starts and ends on a character boundary: slicing inside a
        // multi-byte character panics.
        if !s.is_char_boundary(i) {
            continue;
        }
        for j in 0..i {
            if dp[j] && s.is_char_boundary(j) && PLACEHOLDERS.contains(&&s[j..i]) {
                dp[i] = true;
                break;
            }
        }
    }
    dp[n]
}

fn is_placeholder(reason: &str) -> bool {
    let r = reason
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim_matches(|c: char| {
            matches!(
                c,
                ':' | '-' | ',' | ';' | '.' | '?' | '!' | '"' | '\'' | '`'
            )
        })
        .trim();
    if r.is_empty() {
        return true;
    }
    if r.starts_with('<') && r.ends_with('>') {
        return true;
    }
    let norm = normalize_placeholder_token(r);
    let norm_trimmed = norm.trim_matches(|c: char| {
        matches!(
            c,
            ':' | '-' | ',' | ';' | '.' | '?' | '!' | '"' | '\'' | '`'
        )
    });
    if norm_trimmed.chars().filter(|c| c.is_alphanumeric()).count() == 0 {
        return true;
    }
    if PLACEHOLDERS.contains(&norm_trimmed) || is_segmented_placeholder(norm_trimmed) {
        return true;
    }
    // Splitting by whitespace OR invisible/zero-width characters preserves
    // invisible characters as token boundaries (e.g. `todo\u{200b}fixme`).
    let words: Vec<&str> = r
        .split(|c: char| c.is_whitespace() || is_invisible(c))
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() > 1 && words.iter().all(|w| is_placeholder_word(w)) {
        return true;
    }
    false
}

static CITATION_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)https?://[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+/(?:actions/runs/\d+|pipelines/\d+|jobs/\d+)|(?:results|docs|crates|scripts|benches|tests|src|target)/[a-zA-Z0-9_./-]+\.(?:json|txt|csv|md|svg|log|out)").expect("valid regex")
});

/// Extracts a verifiable citation (CI run URL or committed artifact path) from an override reason.
pub fn extract_citation(reason: &str) -> Option<String> {
    CITATION_RE.find(reason).map(|m| m.as_str().to_string())
}

/// Every citation (CI run URL or committed artifact path) in an override reason, in order.
///
/// A reason carrying two citations rests on both; checking only the first lets word order
/// decide which source is examined.
pub fn extract_citations(reason: &str) -> Vec<String> {
    CITATION_RE
        .find_iter(reason)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Checks whether an override reason explicitly names a benchmark arm (in full, tail, or stem).
pub fn reason_cites_arm(reason: &str, arm: &str) -> bool {
    let haystack = reason.to_lowercase();
    let arm_lower = arm.to_lowercase();
    if haystack.contains(&arm_lower) {
        return true;
    }
    let tail = arm.rsplit("::").next().unwrap_or(arm).to_lowercase();
    if haystack.contains(&tail) {
        return true;
    }
    let stem = tail.split('/').next().unwrap_or(&tail);
    if haystack.contains(stem) {
        return true;
    }
    false
}

/// Returns the subset of regressed arms that the override reason does NOT name.
pub fn unapproved_regressed_arms<'a>(reason: &str, regressed: &'a [String]) -> Vec<&'a str> {
    regressed
        .iter()
        .filter(|arm| !reason_cites_arm(reason, arm))
        .map(|s| s.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_anchored_directive_is_accepted() {
        let body = "Summary\n\nremoves: tests/old.rs superseded by tests/new.rs\n";
        let reasons = directive_reasons(body, REMOVES);
        assert_eq!(reasons, vec!["tests/old.rs superseded by tests/new.rs"]);
        assert!(covers(&reasons, "tests/old.rs"));
    }

    #[test]
    fn html_comment_wrapping_is_accepted_and_closer_stripped() {
        let reasons = directive_reasons("<!-- deletes: benches/x.rs obsolete -->", REMOVES);
        assert_eq!(reasons, vec!["benches/x.rs obsolete"]);
    }

    #[test]
    fn prose_table_and_code_mentions_do_not_arm_the_override() {
        // The verbatim shape of the defect: documentation *about*
        // the directive must never act as the directive.
        let body = "\
We use the removes: token to justify deletions.
| token | meaning |
| removes: tests/old.rs | justification |
`removes: tests/old.rs because`
```
removes: tests/old.rs inside a fence
```
";
        assert!(directive_reasons(body, REMOVES).is_empty());
    }

    #[test]
    fn indented_blocks_and_nested_fences_stay_code() {
        let armed = |body: &str| !directive_reasons(body, REMOVES).is_empty();
        // Positive controls: up to three spaces, and a line after a closed fence.
        assert!(armed("   removes: tests/old.rs moved\n"));
        assert!(armed("```\nexample\n```\nremoves: tests/old.rs moved\n"));
        assert!(armed("````\nexample\n`````\nremoves: tests/old.rs moved\n"));
        assert!(armed("~~~\nexample\n~~~\nremoves: tests/old.rs moved\n"));
        // Negative controls: an indented block, and a fence quoting a shorter or other fence.
        for body in [
            "    removes: tests/old.rs moved\n",
            "\tremoves: tests/old.rs moved\n",
            "  \tremoves: tests/old.rs moved\n",
            "````md\n```\nremoves: tests/old.rs moved\n````\n",
            "~~~\n```\nremoves: tests/old.rs moved\n~~~\n",
            "```\n``` not a close\nremoves: tests/old.rs moved\n```\n",
            "```\n    ```\nremoves: tests/old.rs moved\n```\n",
        ] {
            assert!(!armed(body), "armed from code: {body:?}");
        }
        assert_eq!(fence_marker("   ````md"), Some(('`', 4)));
        assert_eq!(fence_marker("    ```"), None);
        assert_eq!(fence_marker("``"), None);
        assert_eq!(indent_columns("  \tx"), 4);
    }

    #[test]
    fn workflow_marker_needs_a_real_reason() {
        let m = ADVISORY_MARKER;
        assert_eq!(
            workflow_marker_reason(" discipline:advisory shadow of the lint script", m).as_deref(),
            Some("shadow of the lint script")
        );
        for comment in [
            " discipline:advisory",
            " discipline:advisory   ",
            " discipline:advisory <reason>",
            " discipline:advisory TODO",
            " discipline:advisory n/a",
            " discipline:advisoryshadow run",
            " see discipline:advisory shadow run",
            " discipline:allow(command) shadow run",
        ] {
            assert_eq!(workflow_marker_reason(comment, m), None, "{comment:?}");
        }
    }

    /// Found by the `directives` fuzz target: a reason holding a multi-byte character
    /// that confusable folding leaves alone sliced the segmentation table mid-character
    /// and panicked, so a pull request body could abort the whole check.
    #[test]
    fn rationale_with_multibyte_characters_does_not_panic() {
        for reason in [
            "cu\u{fffd}",
            "\u{fffd}",
            "日本語のテスト理由",
            "tod\u{fffd}o",
            "todo\u{fffd}",
            "caf\u{e9} au lait, for a documented reason",
        ] {
            let _ = is_valid_rationale(reason);
            let _ = is_placeholder(reason);
        }
        // Segmentation still reads a run of placeholders written without spaces, and
        // still does not mistake a non-placeholder for one.
        assert!(!is_valid_rationale("todotbd"));
        assert!(is_valid_rationale("caf\u{e9} au lait, kept on purpose"));
    }

    #[test]
    fn placeholders_are_rejected() {
        for body in [
            "removes:",
            "removes: <reason>",
            "removes: TODO",
            "removes: n/a",
            "<!-- removes: <reason> -->",
            "<!-- removes: <reason> --!>",
            "removes: ...",
        ] {
            assert!(
                directive_reasons(body, REMOVES).is_empty(),
                "placeholder accepted: {body:?}"
            );
        }
    }

    #[test]
    fn override_is_scoped_to_named_subjects() {
        let reasons =
            directive_reasons("removes: tests/legacy replaced by proptest suite", REMOVES);
        assert!(covers(&reasons, "tests/legacy/a.rs"));
        assert!(covers(&reasons, "tests/legacy/deep/b.rs"));
        assert!(!covers(&reasons, "tests/other.rs"));
        assert!(!covers(&reasons, "tests/legacy_extra.rs"));

        let by_name = directive_reasons("removes: old.rs, moved", REMOVES);
        assert!(covers(&by_name, "tests/old.rs"));
        assert!(!covers(&by_name, "tests/very_old.rs"));

        // A bare word must NOT act as a directory prefix.
        let bare = directive_reasons("removes: tests were refactored into benchmarks", REMOVES);
        assert!(!covers(&bare, "tests/a.rs"));
        assert!(!covers(&bare, "tests/legacy/a.rs"));

        // But an explicit directory prefix with a slash DOES cover it.
        let with_slash =
            directive_reasons("removes: tests/ were refactored into benchmarks", REMOVES);
        assert!(covers(&with_slash, "tests/a.rs"));

        // Dotfile and dotdirectory paths preserve leading dot
        let dotfile = directive_reasons("removes: .github/workflows/pages.yml retired", REMOVES);
        assert!(covers(&dotfile, ".github/workflows/pages.yml"));
        assert!(!covers(&dotfile, ".github/workflows/ci.yml"));

        let dotdir = directive_reasons("removes: .github/workflows/ retired", REMOVES);
        assert!(covers(&dotdir, ".github/workflows/pages.yml"));
    }

    #[test]
    fn unrelated_directive_name_is_ignored() {
        assert!(directive_reasons("allow-ignore: flaky_test on CI", REMOVES).is_empty());
        let r = directive_reasons("allow-ignore: flaky_test on CI", ALLOW_IGNORE);
        assert!(covers(&r, "flaky_test"));
        assert!(!covers(&r, "flaky"));
    }

    #[test]
    fn discipline_allow_directive_form_accepted() {
        let bare = "<!-- discipline:allow(deletion-rationale) tests -->";
        let bare_reasons = directive_reasons(bare, REMOVES);
        assert_eq!(bare_reasons, vec!["tests"]);
        assert!(!covers(&bare_reasons, "tests/legacy/old.rs"));

        // Subject-only directive without a reason does NOT cover:
        let scoped = "<!-- discipline:allow(deletion-rationale) tests/legacy/ -->";
        let scoped_reasons = directive_reasons(scoped, REMOVES);
        assert_eq!(scoped_reasons, vec!["tests/legacy/"]);
        assert!(!covers(&scoped_reasons, "tests/legacy/old.rs"));

        // Subject with a non-empty, non-placeholder reason DOES cover:
        let scoped_with_reason =
            "<!-- discipline:allow(deletion-rationale) tests/legacy/ superseded by proptest suite -->";
        let scoped_with_reason_reasons = directive_reasons(scoped_with_reason, REMOVES);
        assert!(covers(&scoped_with_reason_reasons, "tests/legacy/old.rs"));
    }

    #[test]
    fn directive_requires_non_empty_non_placeholder_rationale() {
        // Bare filename without reason:
        let r_empty = directive_reasons("removes: tests/old.rs", REMOVES);
        assert!(!covers(&r_empty, "tests/old.rs"));

        // Bare quoted filename without reason:
        let r_quoted_empty = directive_reasons("removes: \"tests/old.rs\"", REMOVES);
        assert!(!covers(&r_quoted_empty, "tests/old.rs"));

        // Filename with placeholder reason:
        for placeholder in [
            "todo", "TODO", "tbd", "TBD", "n/a", "N/A", "fixme", "<reason>", "...", "-",
        ] {
            let line = format!("removes: tests/old.rs {placeholder}");
            let r_ph = directive_reasons(&line, REMOVES);
            assert!(
                !covers(&r_ph, "tests/old.rs"),
                "placeholder '{placeholder}' must not satisfy rationale requirement"
            );

            let line_quoted = format!("removes: \"tests/old.rs\" {placeholder}");
            let r_qph = directive_reasons(&line_quoted, REMOVES);
            assert!(
                !covers(&r_qph, "tests/old.rs"),
                "placeholder '{placeholder}' with quoted subject must not satisfy rationale requirement"
            );
        }

        // Multi-token placeholders:
        for ph in ["todo fixme", "TODO: TBD", "n/a - none"] {
            let line = format!("removes: tests/old.rs {ph}");
            let r_ph = directive_reasons(&line, REMOVES);
            assert!(!covers(&r_ph, "tests/old.rs"));
        }

        // Punctuation and single-character non-rationales:
        for punctuation in [".", "?", "!", "/", "_", "*", "::", "x", "1"] {
            let line = format!("removes: tests/old.rs {punctuation}");
            let r_punct = directive_reasons(&line, REMOVES);
            assert!(
                !covers(&r_punct, "tests/old.rs"),
                "punctuation/single-char '{punctuation}' must not satisfy rationale requirement"
            );
        }

        // Homoglyphs, confusables, and invisible character placeholder evasions:
        for confusable in [
            "t\u{043e}d\u{043e}",                        // Cyrillic small letter o
            "t\u{200b}o\u{200b}d\u{200b}o",              // zero-width space
            "ｔｏｄｏ",                                  // fullwidth Latin
            "ｆｉｘｍｅ",                                // fullwidth Latin
            "t\u{043e}d\u{043e} f\u{0456}xme",           // mixed Cyrillic/Latin multi-token
            "n/a\u{feff}",                               // byte order mark
            "todo\u{200b}fixme", // zero-width space between placeholder words
            "todo\u{200b}tbd",   // zero-width separator
            "t\u{200b}o\u{200b}d\u{200b}o\u{200b}fixme", // zero-width inside and between words
            "\u{1D42D}\u{1D428}\u{1D41D}\u{1D428}", // mathematical bold todo
            "to\u{0301}do",      // combining acute mark
            "t0d0",              // leetspeak digits
            "todo.",             // trailing sentence period
            "TBD!",              // trailing sentence exclamation
        ] {
            let line = format!("removes: tests/old.rs {confusable}");
            let r_conf = directive_reasons(&line, REMOVES);
            assert!(
                !covers(&r_conf, "tests/old.rs"),
                "confusable placeholder '{confusable}' must not satisfy rationale requirement"
            );
        }

        // Valid issue reference rationale:
        let r_issue = directive_reasons("removes: tests/old.rs #123", REMOVES);
        assert!(covers(&r_issue, "tests/old.rs"));

        let r_gh = directive_reasons("removes: tests/old.rs GH-45", REMOVES);
        assert!(covers(&r_gh, "tests/old.rs"));

        // Valid rationale with filename:
        let r_valid = directive_reasons(
            "removes: tests/old.rs superseded by tests/new_suite.rs",
            REMOVES,
        );
        assert!(covers(&r_valid, "tests/old.rs"));

        let r_valid_colon = directive_reasons(
            "removes: tests/old.rs: superseded by tests/new_suite.rs",
            REMOVES,
        );
        assert!(covers(&r_valid_colon, "tests/old.rs"));

        let r_valid_quoted = directive_reasons(
            "removes: \"tests/old.rs\" superseded by tests/new_suite.rs",
            REMOVES,
        );
        assert!(covers(&r_valid_quoted, "tests/old.rs"));

        // Multi-word test name subject:
        let r_test_empty = directive_reasons("allow-ignore: my test name", ALLOW_IGNORE);
        assert!(!covers(&r_test_empty, "my test name"));

        let r_test_todo = directive_reasons("allow-ignore: my test name todo", ALLOW_IGNORE);
        assert!(!covers(&r_test_todo, "my test name"));

        for ph in [
            "ok",
            "temp",
            "dummy",
            "null",
            "placeholder",
            "asdf",
            "t\u{043e}d\u{043e}",
        ] {
            let line = format!("allow-ignore: my test name {ph}");
            let r_ph = directive_reasons(&line, ALLOW_IGNORE);
            assert!(
                !covers(&r_ph, "my test name"),
                "allow-ignore placeholder '{ph}' must not lift"
            );
        }

        let r_test_quoted_empty = directive_reasons("allow-ignore: 'my test name'", ALLOW_IGNORE);
        assert!(!covers(&r_test_quoted_empty, "my test name"));

        let r_test_valid = directive_reasons(
            "allow-ignore: my test name skipped because upstream bug #123",
            ALLOW_IGNORE,
        );
        assert!(covers(&r_test_valid, "my test name"));

        // Multi-word with colon subject:
        let r_colon_empty = directive_reasons("allow-suppression: type: ignore", ALLOW_SUPPRESSION);
        assert!(!covers(&r_colon_empty, "type: ignore"));

        let r_colon_todo =
            directive_reasons("allow-suppression: type: ignore todo", ALLOW_SUPPRESSION);
        assert!(!covers(&r_colon_todo, "type: ignore"));

        let r_colon_valid = directive_reasons(
            "allow-suppression: type: ignore needed for untyped third-party library",
            ALLOW_SUPPRESSION,
        );
        assert!(covers(&r_colon_valid, "type: ignore"));
    }

    #[test]
    fn multi_word_and_quoted_subjects_are_covered() {
        let r1 = directive_reasons(
            "allow-ignore: skips this test skipped for refactoring",
            ALLOW_IGNORE,
        );
        assert!(covers(&r1, "skips this test"));
        assert!(!covers(&r1, "this test"));

        let r2 = directive_reasons(
            "allow-ignore: temporarily disabled 'skips this test' pending fix",
            ALLOW_IGNORE,
        );
        assert!(covers(&r2, "skips this test"));
        assert!(!covers(&r2, "other test"));
    }

    #[test]
    fn allow_nul_directives_parsed_and_discriminate() {
        let r1 = directive_reasons(
            "allow-nul: tests/string_to_entry_005.phpt binary cache payload",
            ALLOW_NUL,
        );
        assert!(covers(&r1, "tests/string_to_entry_005.phpt"));
        assert!(!covers(&r1, "tests/other.phpt"));

        let r1_by_name = directive_reasons(
            "allow-nul: string_to_entry_005.phpt binary cache payload",
            ALLOW_NUL,
        );
        assert!(covers(&r1_by_name, "tests/string_to_entry_005.phpt"));

        let r2 = directive_reasons(
            "allow-nul-byte: src/bad.rs test fixture with binary payload",
            ALLOW_NUL,
        );
        assert!(covers(&r2, "src/bad.rs"));
        assert!(!covers(&r2, "src/good.rs"));

        // Per S3 alias collapsing, allow-corrupt is dropped; only canonical allow-nul
        // and deprecated alias allow-nul-byte are recognized.
        let r3 = directive_reasons("allow-corrupt: tests/fixture.bin raw fuzz input", ALLOW_NUL);
        assert!(r3.is_empty());
    }

    #[test]
    fn secrets_argv_ok_and_no_issue_directives_parsed() {
        let r1 = directive_reasons(
            "secrets-argv-ok: deploy.sh legacy container entrypoint",
            SECRETS_ARGV_OK,
        );
        assert!(covers(&r1, "deploy.sh"));
        assert!(!covers(&r1, "build.sh"));

        let r2 = directive_reasons(
            "<!-- discipline:allow(shell-secrets) scripts/run.sh dev test runner -->",
            SECRETS_ARGV_OK,
        );
        assert!(covers(&r2, "scripts/run.sh"));

        let r3 = directive_reasons("no-issue: trivial documentation fix", NO_ISSUE);
        assert_eq!(r3, vec!["trivial documentation fix"]);
        assert!(!is_placeholder(&r3[0]));

        let r4 = directive_reasons("no-issue: <reason>", NO_ISSUE);
        assert!(r4.is_empty());
    }

    #[test]
    fn discipline_namespaced_directives_parsed() {
        let r1 = directive_reasons("discipline: removes: old_test.rs refactored", REMOVES);
        assert_eq!(r1, vec!["old_test.rs refactored"]);
        assert!(covers(&r1, "old_test.rs"));

        let r2 = directive_reasons(
            "<!-- discipline: allow-assertion-drop: test_sync removed redundant assert -->",
            ALLOW_ASSERTION_DROP,
        );
        assert_eq!(r2, vec!["test_sync removed redundant assert"]);
        assert!(covers(&r2, "test_sync"));

        let r3 = directive_reasons(
            "discipline: allow-regression: bench_run #822 verified",
            ALLOW_REGRESSION,
        );
        assert_eq!(r3, vec!["bench_run #822 verified"]);
        assert!(covers(&r3, "bench_run"));
    }

    #[test]
    fn test_scoped_reason_names_multi_item_and_gate_scoping() {
        let multi = "dead_code noqa type: ignore legacy";
        assert!(reason_names(multi, "dead_code"));
        assert!(reason_names(multi, "noqa"));
        assert!(reason_names(multi, "type: ignore"));
        assert!(!reason_names(multi, "unrelated"));

        let unrelated = "totally unrelated words here";
        assert!(!reason_names(unrelated, "dead_code"));
        assert!(!reason_names(unrelated, "noqa"));
        assert!(!reason_names(unrelated, "type: ignore"));

        let dirs = vec![ParsedDirective {
            directive: "allow-suppression".to_string(),
            reason: unrelated.to_string(),
            source: OverrideSource::Commit("abc".to_string()),
            hidden: false,
        }];
        // With the fix, an unrelated reason returns None.
        assert!(find_override(
            &dirs,
            "suppression-delta",
            &crate::findings::SUPPRESSION_ADDED,
            ALLOW_SUPPRESSION,
            "suppression-delta"
        )
        .is_none());
    }

    #[test]
    fn extract_citations_returns_every_citation_in_order() {
        let reason = "trade in results/fallback.json, measured in run https://github.com/acme/widgets/actions/runs/7 and docs/RULES.md";
        assert_eq!(
            extract_citations(reason),
            vec![
                "results/fallback.json".to_string(),
                "https://github.com/acme/widgets/actions/runs/7".to_string(),
                "docs/RULES.md".to_string(),
            ]
        );
        assert!(extract_citations("no source at all").is_empty());
    }

    #[test]
    fn test_extract_citation_and_arm_naming() {
        assert_eq!(
            extract_citation("#480 trade: reduces random lookup branch mispredictions by 45.6% (1.49 vs 2.74 per probe) via independent L1 loads"),
            None
        );
        assert_eq!(extract_citation("intentional SIMD trade-off"), None);
        assert_eq!(extract_citation("approved by review"), None);
        assert_eq!(extract_citation("measured at commit 4c4e852"), None);
        assert_eq!(
            extract_citation(
                "TLB win, run https://github.com/example-org/example-project/actions/runs/33325789949"
            ),
            Some("https://github.com/example-org/example-project/actions/runs/33325789949".to_string())
        );
        assert_eq!(
            extract_citation("paired CI in results/baseline_vs_libjudy.json"),
            Some("results/baseline_vs_libjudy.json".to_string())
        );
        assert_eq!(
            extract_citation(
                "fallback in docs/benchmarks/concurrency/results/fallback_maturity.json"
            ),
            Some("docs/benchmarks/concurrency/results/fallback_maturity.json".to_string())
        );

        let lock_padded_reason = "zero-sharing per-writer coordination under feature lock-padded (refs CI run https://github.com/example-org/example-project/actions/runs/34490311084)";
        assert!(extract_citation(lock_padded_reason).is_some());
        let regressed = vec![
            "instructions::cost::sync_map_insert/random".to_string(),
            "instructions::cost::sync_set_insert/random".to_string(),
        ];
        assert_eq!(
            unapproved_regressed_arms(lock_padded_reason, &regressed),
            vec![
                "instructions::cost::sync_map_insert/random",
                "instructions::cost::sync_set_insert/random"
            ]
        );

        let reason_named = "sync_map_insert pays for the OLC bracket, run https://x/actions/runs/1";
        assert_eq!(
            unapproved_regressed_arms(
                reason_named,
                &["instructions::cost::sync_map_insert/random".to_string()]
            ),
            Vec::<&str>::new()
        );
        assert_eq!(
            unapproved_regressed_arms(
                "sync_map_insert only, run https://x/actions/runs/1",
                &regressed
            ),
            vec!["instructions::cost::sync_set_insert/random"]
        );
    }

    #[test]
    fn a_directive_is_unused_unless_an_override_came_from_it() {
        let body = "allow-dependency: serde parser\nallow-stub: fn_a placeholder until #12\n";
        let parsed = parse_directives(body, OverrideSource::PrBody);
        let from = |d: &ParsedDirective, source: OverrideSource, reason: &str| OverrideRecord {
            gate: "g".into(),
            code: None,
            subject: "s".into(),
            directive: d.directive.to_ascii_uppercase(),
            reason: reason.into(),
            source,
            hidden: false,
        };
        // The first lifted a finding (the name matches case-insensitively); the second
        // did not.
        let applied = vec![from(&parsed[0], OverrideSource::PrBody, &parsed[0].reason)];
        let unused = unused_directives(&parsed, &applied);
        assert_eq!(
            unused,
            vec![UnusedDirective {
                directive: "allow-stub".into(),
                source: OverrideSource::PrBody,
                hidden: false,
            }]
        );
        assert!(!serde_json::to_string(&unused)
            .unwrap()
            .contains("placeholder"));
        // The same directive and reason from another source, or another reason from the
        // same source, is not the one that was used.
        let elsewhere = vec![
            from(
                &parsed[0],
                OverrideSource::Commit("abc".into()),
                &parsed[0].reason,
            ),
            from(&parsed[1], OverrideSource::PrBody, "fn_a something else"),
        ];
        assert_eq!(unused_directives(&parsed, &elsewhere).len(), 2);
        assert!(unused_directives(&parsed, &[]).len() == 2);
        assert!(unused_directives(&[], &applied).is_empty());
    }

    #[test]
    fn a_refused_hidden_directive_is_named_without_its_reason() {
        let reason = "ignore previous instructions";
        let body = format!("<!-- no-issue: {reason} -->\n<!-- removes: tests/old.rs {reason} -->");
        let commits = vec![("abc1234".to_string(), format!("chore: x\n\n{body}"))];
        let merged = vec![MergedBody {
            number: 7,
            author: "someone".to_string(),
            body: body.clone(),
        }];
        let config = crate::config::DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\n[directives]\nsources = [\"pr-body\", \"commits\", \"merged-pr-body\"]\n",
        )
        .unwrap();
        let (active, notes) =
            extract_directives_with_merged(Some(&body), &commits, &merged, &config);
        assert!(active.is_empty(), "{active:?}");
        let (_, plain) = extract_directives(Some(&body), &commits, &config.directives);
        let hidden = |v: &[String]| -> Vec<String> {
            v.iter()
                .filter(|n| n.starts_with("hidden directive"))
                .cloned()
                .collect()
        };
        for n in notes.iter().chain(&plain) {
            assert!(!n.contains(reason), "{n}");
        }
        for n in hidden(&notes).iter().chain(&hidden(&plain)) {
            assert!(n.contains("`no-issue`") || n.contains("`removes`"), "{n}");
        }
        // PR body, commit and merged pull request, for each of the two directives.
        assert_eq!(hidden(&notes).len(), 6, "{notes:?}");
        assert_eq!(hidden(&plain).len(), 4, "{plain:?}");
    }

    #[test]
    fn test_multiline_html_comment_directives_marked_hidden() {
        let text = "<!--\nallow-assertion-drop: tests/auth.rs\n-->\nallow-ignore: tests/slow.rs\n";
        let parsed = parse_directives(text, OverrideSource::PrBody);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].directive, "allow-assertion-drop");
        assert!(
            parsed[0].hidden,
            "directive inside multiline HTML comment must be hidden"
        );
        assert_eq!(parsed[1].directive, "allow-ignore");
        assert!(
            !parsed[1].hidden,
            "directive outside HTML comment must not be hidden"
        );
    }
}
