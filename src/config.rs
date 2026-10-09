//! Configuration schema, gate registry, and layered configuration resolution.
//!
//! Layers, lowest to highest precedence:
//!   1. built-in defaults (every available gate enabled, severity `error`)
//!   2. `discipline.toml`
//!   3. inline TOML override (`--config-override` / action input `config_override`)
//!   4. `--enable` / `--disable` gate lists
//!   5. `DISCIPLINE_HOSTNAME_DENYLIST` (appended; meant for CI secrets)
//!
//! Layers are merged as `toml::Value` trees and deserialized once at the end, so
//! every layer goes through the same strict (`deny_unknown_fields`) validation.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use toml::Value;

pub const HOSTNAME_DENYLIST_ENV: &str = "DISCIPLINE_HOSTNAME_DENYLIST";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    AgentGuard,
    Hygiene,
    Integrity,
    Quality,
    Verification,
    Bench,
}

impl Suite {
    pub fn label(self) -> &'static str {
        match self {
            Suite::AgentGuard => "agent-guard",
            Suite::Hygiene => "hygiene",
            Suite::Integrity => "integrity",
            Suite::Quality => "quality",
            Suite::Verification => "verification",
            Suite::Bench => "bench",
        }
    }
}

pub struct GateInfo {
    pub id: &'static str,
    pub suite: Suite,
    pub summary: &'static str,
    pub languages: &'static str,
    /// `false` = planned in the roadmap but not shipped in this binary. A planned
    /// gate cannot be enabled or configured: asking for it is an error, never
    /// a silent pass.
    pub available: bool,
    /// The gate's rule describes a change (base against head). A whole-tree run has no
    /// change: every file is "added", so such a gate would record every dependency, every
    /// ignored test and every instruction file as debt, and is not evaluated there.
    pub delta_only: bool,
}

/// Single source of truth for gate identifiers.
pub const GATES: &[GateInfo] = &[
    GateInfo {
        id: "agents-md",
        suite: Suite::AgentGuard,
        summary: "AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "assertion-reduction",
        suite: Suite::AgentGuard,
        summary: "assertion count / strength must not drop in an existing test",
        languages: "Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin, Swift, Scala, Objective-C",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "vacuous-tests",
        suite: Suite::AgentGuard,
        summary: "new tests must carry a non-tautological assertion",
        languages: "Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin, Swift, Scala, Objective-C",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "ignored-tests",
        suite: Suite::AgentGuard,
        summary: "tests must not be newly #[ignore]d or skipped without directive",
        languages: "Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin, Swift, Scala, Objective-C",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "unsafe-safety-comment",
        suite: Suite::AgentGuard,
        summary: "unsafe blocks / impls carry a // SAFETY: comment",
        languages: "Rust",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "deletion-rationale",
        suite: Suite::AgentGuard,
        summary: "deleted files and removed tests need a scoped removes: rationale",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "time-estimates",
        suite: Suite::Hygiene,
        summary: "no calendar / duration estimates in markdown or the PR body",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "pii",
        suite: Suite::Hygiene,
        summary: "no home paths, LAN IPs, denylisted hostnames, or fixed-format credentials (private keys, AWS, GitHub, Slack, OpenAI and Anthropic tokens, literal bearer headers) in tracked text",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "agent-scratch",
        suite: Suite::Hygiene,
        summary: "agent scratch state is never tracked",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "shell-secrets",
        suite: Suite::Hygiene,
        summary: "no command-line secrets or unverified piped scripts in shell, docker, or CI",
        languages: "shell, docker, workflows",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "issue-link",
        suite: Suite::Hygiene,
        summary: "PR title or description links a tracking issue (#123, Fixes #123)",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "review-threads",
        suite: Suite::Hygiene,
        summary: "the pull request has no unresolved review thread",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "ratified-paths",
        suite: Suite::AgentGuard,
        summary: "edits to protected paths carry an owner's ratification on an issue the pull request closes",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "citation-metadata",
        suite: Suite::Hygiene,
        summary: "CITATION.cff and .zenodo.json are valid, agree with each other, and cite the concept DOI",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "commit-provenance",
        suite: Suite::Hygiene,
        summary: "commits carry the required trailers; an agent-produced commit carries a review by someone else",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "config-integrity",
        suite: Suite::Integrity,
        summary: "a change cannot weaken its own discipline.toml without a token",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "stub-bodies",
        suite: Suite::AgentGuard,
        summary: "added functions are not stubs; existing bodies are not replaced by todo!() / NotImplementedError / return null",
        languages: "Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin, Swift, Scala, Objective-C",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "harness-tampering",
        suite: Suite::AgentGuard,
        summary: "a file the test runner loads does not make a failing run pass",
        languages: "Go, Python, JS/TS, Rust",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "error-swallowing",
        suite: Suite::AgentGuard,
        summary: "no new empty error handler or discarded Result outside tests",
        languages: "Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin, Swift, Scala, Objective-C",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "instruction-smuggling",
        suite: Suite::AgentGuard,
        summary: "no invisible Unicode, unreviewed agent-instruction edits, or instruction-like text in comments and prose",
        languages: "any (invisible characters, instruction files); Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin, Swift, Scala, Objective-C and prose files (phrases)",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "build-hooks",
        suite: Suite::Integrity,
        summary: "install and build hooks that gain network or shell access, and package-manager configuration edits, need a token",
        languages: "package.json, build.rs, setup.py, .npmrc, .pypirc, pip.conf, .cargo/config.toml, .env*",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "toolchain-config",
        suite: Suite::Integrity,
        summary: "compiler, linter, type-checker, test-runner and coverage configuration cannot be loosened without a token",
        languages: "tsconfig, ruff, mypy, pytest, coverage, flake8, Cargo lints, rustflags, nextest, eslintrc, golangci, jest, codecov, phpstan, phpunit, and compiler warning flags in Makefile, CMake, setup.py and build.rs",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "sandbox-config",
        suite: Suite::Integrity,
        summary: "a change cannot widen an agent's permissions or sandbox, or a container's isolation, without a token",
        languages: "Claude Code, Codex, Gemini CLI, Qwen Code, OpenCode, Cursor and Copilot CLI settings, MCP server lists, devcontainer.json, Docker Compose, CI job and service containers",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "scope-confinement",
        suite: Suite::AgentGuard,
        summary: "changes stay inside authorized paths",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "suppression-delta",
        suite: Suite::AgentGuard,
        summary: "newly added linter / compiler suppression annotations",
        languages: "per pack",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "provenance-tags",
        suite: Suite::Hygiene,
        summary: "published numerics carry (measured|target|projected)",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "ci-integrity",
        suite: Suite::Integrity,
        summary: "workflow weakening: continue-on-error, || true, unpinned actions",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "ci-skip-set",
        suite: Suite::Integrity,
        summary: "rollup skip set matches each job's `if:` under the observed filter outputs",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "test-floor",
        suite: Suite::Integrity,
        summary: "test-count ratchet read from the base ref",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "golden-output",
        suite: Suite::Integrity,
        summary:
            "prevents stealth edits to committed golden/test output files without explicit override",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "dependency-delta",
        suite: Suite::Integrity,
        summary: "manifest diff inspection: zero wildcards, source/license allowlists, and deny.toml verification",
        languages: "any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "test-budget",
        suite: Suite::Integrity,
        summary: "property-test and fuzz effort ratchet (cases, shrink iters, fuzztime, seed corpus)",
        languages: "Rust, Python, JS/TS, Go, any",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "pr-checklist",
        suite: Suite::Hygiene,
        summary: "ticked PR checkboxes are reconciled against the diff",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "command",
        suite: Suite::Verification,
        summary: "fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "sanitizers",
        suite: Suite::Verification,
        summary: "ASan / TSan preset with audited suppressions and a race canary",
        languages: "Rust, C/C++",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "msrv",
        suite: Suite::Quality,
        summary: "cargo check under the pinned MSRV",
        languages: "Rust",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "miri",
        suite: Suite::Verification,
        summary: "Miri tiers with zero-tests guard",
        languages: "Rust",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "unsafe-budget",
        suite: Suite::Verification,
        summary: "unsafe count ratchet",
        languages: "Rust",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "bench-regression",
        suite: Suite::Bench,
        summary: "benchmark drift via harness adapters (deterministic counts or BCa intervals)",
        languages: "Rust, Go, Python, C/C++",
        available: true,
        delta_only: true,
    },
    GateInfo {
        id: "archive-contents",
        suite: Suite::Integrity,
        summary: "distribution archive must contain required paths and zero forbidden developer artifacts",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "manifest-sync",
        suite: Suite::Integrity,
        summary: "reconcile git-tracked files against packaging manifest declarations",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "version-lockstep",
        suite: Suite::Integrity,
        summary: "version declarations across headers, manifests, and files must remain in lockstep",
        languages: "any",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "gate-command-lint",
        suite: Suite::AgentGuard,
        summary: "reject vacuous, masked, or inverted verification commands in plan files",
        languages: "markdown, shell",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "mechanism-sections",
        suite: Suite::Hygiene,
        summary: "require (inferred) or (verified: <target>) evidence tags in root cause sections",
        languages: "markdown",
        available: true,
        delta_only: false,
    },
    GateInfo {
        id: "citation-anchors",
        suite: Suite::Hygiene,
        summary: "verify path:line@sha citations against quoted text and git history",
        languages: "markdown, any",
        available: true,
        delta_only: false,
    },
];

pub fn gate_info(id: &str) -> Option<&'static GateInfo> {
    GATES.iter().find(|g| g.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    /// This severity, held to at most a warning: an error becomes a warning, a warning
    /// and a note stay as they are. For a finding a gate reports on a heuristic, or on
    /// a file it could not analyse, which must not fail the run on its own.
    pub fn capped_at_warning(self) -> Severity {
        match self {
            Severity::Error => Severity::Warning,
            other => other,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Note => write!(f, "note"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DirectivesConfig {
    pub sources: Vec<String>,
    pub allow_hidden: bool,
    pub fail_on_overrides: bool,
    pub allowed_override_actors: Vec<String>,
    /// Most directive overrides (PR body and commit bodies; inline markers are not
    /// counted) one change may apply. Unset = no cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_overrides: Option<usize>,
    /// Most inline exemption markers one change may apply. Unset = no cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_inline_overrides: Option<usize>,
    /// Directive overrides fail the run until the forge shows an approving review of the
    /// head commit by an `allowed_override_actors` member who is not the author.
    pub require_approval: bool,
    /// On a push event, when the `merged-pr-body` source cannot be read (forge
    /// unreachable, token without permission), continue with a named note (the finding
    /// the body might have lifted stands) instead of stopping with exit 2. Default true:
    /// a least-privilege token cannot always read pull requests. `false` makes the
    /// review record a hard requirement of the push run.
    #[serde(default = "default_true")]
    pub degrade_offline: bool,
}

impl Default for DirectivesConfig {
    fn default() -> Self {
        Self {
            sources: vec![
                "pr-body".to_string(),
                "commits".to_string(),
                "merged-pr-body".to_string(),
            ],
            allow_hidden: false,
            fail_on_overrides: false,
            allowed_override_actors: Vec::new(),
            max_overrides: None,
            max_inline_overrides: None,
            require_approval: false,
            degrade_offline: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisciplineConfig {
    #[serde(default)]
    pub meta: MetaConfig,
    #[serde(default)]
    pub directives: DirectivesConfig,
    #[serde(default)]
    pub tests: TestsConfig,
    #[serde(default)]
    pub languages: LanguagesConfig,
    #[serde(default)]
    pub gates: Gates,
    #[serde(default)]
    pub hooks: HooksConfig,
    /// Deprecated key names this configuration used, one note each (see [`KEY_ALIASES`]).
    #[serde(skip)]
    pub deprecations: Vec<String>,
}

/// A configuration key renamed after 1.0. The old name keeps working until the next major
/// version: it is read as the new one, and the report carries a deprecation note.
#[derive(Debug, Clone, Copy)]
pub struct KeyAlias {
    /// Dotted path of the old key, e.g. `gates.vacuous-tests.old_name`. A `*` segment
    /// matches any key at that level (a key every gate table carries).
    pub old: &'static str,
    /// The new key's name, at the same level as the old one.
    pub new: &'static str,
}

/// Every renamed configuration key. Empty until a key is renamed: the mechanism exists so
/// the 1.0 promise (docs/ARCHITECTURE.md §3.2) is kept by code, not by hand.
pub const KEY_ALIASES: &[KeyAlias] = &[];

/// Configuration keys this project accepted on its default branch and later removed, as
/// dotted paths. A `discipline.toml` in a repository's history can still carry one, and
/// `discipline audit` reads history: [`DisciplineConfig::from_history_toml_str`] drops
/// these keys so the rest of such a file can be compared. `check` never reads this list;
/// to it a removed key is an unknown key (exit 2).
///
/// A key goes here when it is removed, with the change that removed it. A name that was
/// never a key is not listed and stays an error everywhere.
pub const REMOVED_KEYS: &[&str] = &[
    // Added in #504, removed in #513, between two releases.
    "gates.commit-provenance.allow_author_review",
];

/// Removes every [`REMOVED_KEYS`] entry from `value` and returns each one found with its
/// value as written, in the order of the list.
fn take_removed_keys(value: &mut Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for dotted in REMOVED_KEYS {
        let Some((tables, key)) = dotted.rsplit_once('.') else {
            continue;
        };
        let mut at = Some(&mut *value);
        for segment in tables.split('.') {
            at = at.and_then(|v| v.get_mut(segment));
        }
        if let Some(removed) = at
            .and_then(Value::as_table_mut)
            .and_then(|table| table.remove(key))
        {
            found.push((dotted.to_string(), removed.to_string()));
        }
    }
    found
}

/// Reads an empty `gates.commit-provenance.review_trailer` as `require_agent_review = false`.
/// Through v0.17 the empty name was the rule's switch, so a configuration written for it
/// must still load: refusing it stops every run that reads such a base configuration,
/// including the change that migrates it. An explicit `require_agent_review = true` beside
/// it contradicts it and is an error.
fn rewrite_empty_review_trailer(value: &mut Value) -> Result<Option<String>> {
    let Some(gate) = value
        .get_mut("gates")
        .and_then(|g| g.get_mut("commit-provenance"))
        .and_then(Value::as_table_mut)
    else {
        return Ok(None);
    };
    if !gate
        .get("review_trailer")
        .and_then(Value::as_str)
        .is_some_and(|name| name.trim().is_empty())
    {
        return Ok(None);
    }
    if gate.get("require_agent_review").and_then(Value::as_bool) == Some(true) {
        bail!(
            "`gates.commit-provenance.review_trailer` is empty and `require_agent_review` is true: an empty `review_trailer` is the deprecated spelling of `require_agent_review = false`; name the trailer, or set `require_agent_review = false` and remove `review_trailer`"
        );
    }
    gate.remove("review_trailer");
    gate.insert("require_agent_review".to_string(), Value::Boolean(false));
    Ok(Some(
        "`gates.commit-provenance.review_trailer = \"\"` is deprecated: it is read as `require_agent_review = false` until the next major version; replace it"
            .to_string(),
    ))
}

/// Rewrites each old key in `value` to its new name and returns one deprecation note per
/// rewrite. Both names set in the same table is an error: the two values could disagree,
/// and neither can be picked silently.
pub fn apply_key_aliases(value: &mut Value, aliases: &[KeyAlias]) -> Result<Vec<String>> {
    fn walk(
        table: &mut toml::map::Map<String, Value>,
        prefix: &str,
        segments: &[&str],
        alias: &KeyAlias,
        notes: &mut Vec<String>,
    ) -> Result<()> {
        let (first, rest) = segments.split_first().expect("non-empty path");
        if rest.is_empty() {
            if let Some(old_value) = table.remove(*first) {
                let old_path = format!("{prefix}{first}");
                let new_path = format!("{prefix}{}", alias.new);
                if table.contains_key(alias.new) {
                    bail!("`{old_path}` and `{new_path}` are both set; `{old_path}` is the deprecated name of `{new_path}`: keep only `{new_path}`");
                }
                table.insert(alias.new.to_string(), old_value);
                notes.push(format!(
                    "`{old_path}` is deprecated: it is read as `{new_path}` until the next major version; rename it"
                ));
            }
            return Ok(());
        }
        let keys: Vec<String> = if *first == "*" {
            table.keys().cloned().collect()
        } else {
            vec![first.to_string()]
        };
        for k in keys {
            if let Some(Value::Table(child)) = table.get_mut(&k) {
                walk(child, &format!("{prefix}{k}."), rest, alias, notes)?;
            }
        }
        Ok(())
    }
    let mut notes = Vec::new();
    if let Value::Table(root) = value {
        for alias in aliases {
            let segments: Vec<&str> = alias.old.split('.').collect();
            walk(root, "", &segments, alias, &mut notes)?;
        }
    }
    Ok(notes)
}

/// Checks `discipline hook run` makes inside an agent's session and nowhere else. `check`
/// never reads this table: these are not gates, and CI sees no agent session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksConfig {
    #[serde(rename = "premature-stop")]
    pub premature_stop: PrematureStopConfig,
}

/// At the end of a turn: a final message that announces an action the agent did not
/// take (`src/turn.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrematureStopConfig {
    pub enabled: bool,
    pub mode: StopMode,
    /// Most stops refused in one session; the next one is let through.
    pub max_per_session: u32,
    /// Also judge a final message that ends in a tool call written out as text.
    pub tool_call_as_text: bool,
}

impl Default for PrematureStopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: StopMode::Observe,
            max_per_session: 3,
            tool_call_as_text: true,
        }
    }
}

/// What a matched end of turn does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StopMode {
    /// Log the match and let the stop through.
    #[default]
    Observe,
    /// Refuse the stop and hand the agent the reason.
    Refuse,
}

/// Per-language parsing settings, read by the language packs before any gate runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LanguagesConfig {
    pub c: CLanguageConfig,
}

/// Macros the C and C++ packs rewrite before parsing (`src/ast/c_macros.rs`), appended to
/// the built-in Zend, CPython and Ruby C API lists. An entry ending in `*` is a prefix.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CLanguageConfig {
    /// Blanked with their arguments: statement or declaration macros written without a
    /// semicolon, list entries written without a comma, attribute-like prefixes.
    pub macros: Vec<String>,
    /// Expand to a function head (`MYEXT_METHOD(Class, name) { ... }`).
    pub function_macros: Vec<String>,
}

/// What the repository counts as test code beyond what each language's conventions say.
/// Read by every gate that separates test code from production code.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TestsConfig {
    /// Function names (leaf) that are test entry points wherever they appear, e.g. a
    /// script's `self_test`.
    pub functions: Vec<String>,
    /// Path globs whose every line is test scope.
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    #[default]
    Enforcing,
    Advisory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetaConfig {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub mode: RunMode,
}

impl Default for MetaConfig {
    fn default() -> Self {
        Self {
            version: 1,
            name: "discipline-project".to_string(),
            description: None,
            mode: RunMode::Enforcing,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Gates {
    pub agents_md: AgentsMdGate,
    pub assertion_reduction: AssertionGate,
    pub vacuous_tests: AssertionGate,
    pub ignored_tests: IgnoredTestsGate,
    pub unsafe_safety_comment: UnsafeSafetyCommentGate,
    pub deletion_rationale: DeletionGate,
    pub time_estimates: TimeEstimateGate,
    pub pii: PiiGate,
    pub agent_scratch: ScratchGate,
    pub config_integrity: BasicGate,
    pub toolchain_config: BasicGate,
    pub sandbox_config: BasicGate,
    pub stub_bodies: BasicGate,
    pub error_swallowing: ErrorSwallowingGate,
    pub harness_tampering: HarnessTamperingGate,
    pub instruction_smuggling: InstructionSmugglingGate,
    pub build_hooks: BasicGate,
    pub golden_output: GoldenGate,
    pub bench_regression: BenchRegressionGate,
    pub command: CommandGate,
    pub dependency_delta: DependencyDeltaGate,
    pub test_budget: TestBudgetGate,
    pub test_floor: TestFloorGate,
    pub ci_integrity: CiIntegrityGate,
    pub ci_skip_set: CiSkipSetGate,
    pub shell_secrets: ShellSecretsGate,
    pub issue_link: IssueLinkGate,
    pub ratified_paths: RatifiedPathsGate,
    pub review_threads: ReviewThreadsGate,
    pub commit_provenance: CommitProvenanceGate,
    pub citation_metadata: CitationMetadataGate,
    pub provenance_tags: ProvenanceTagsGate,
    pub archive_contents: ArchiveContentsGate,
    pub manifest_sync: ManifestSyncGate,
    pub version_lockstep: VersionLockstepGate,
    pub scope_confinement: ScopeConfinementGate,
    pub suppression_delta: SuppressionDeltaGate,
    pub pr_checklist: PrChecklistGate,
    pub unsafe_budget: UnsafeBudgetGate,
    pub msrv: MsrvGate,
    pub miri: MiriGate,
    pub sanitizers: SanitizersGate,
    pub gate_command_lint: GateCommandLintGate,
    pub mechanism_sections: MechanismSectionsGate,
    pub citation_anchors: CitationAnchorsGate,
}

/// `error-swallowing`: the shared keys plus the paths where a numeric fallback in an
/// error handler is reported.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ErrorSwallowingGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of the files (a benchmark or evaluation harness) in which a new handler that
    /// puts a numeric literal in place of the result is reported, at `warning` at most.
    /// Empty: the check is off.
    pub constant_fallback_paths: Vec<String>,
}

impl Default for ErrorSwallowingGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            constant_fallback_paths: Vec::new(),
        }
    }
}

/// `instruction-smuggling`: the shared keys plus the repository's own agent-instruction
/// files, beyond the built-in list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstructionSmugglingGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of files that instruct agents in this repository (a prompt an MCP server
    /// loads, a runtime context file), reported like `AGENTS.md`.
    pub instruction_files: Vec<String>,
}

impl Default for InstructionSmugglingGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            instruction_files: Vec::new(),
        }
    }
}

/// Settings every gate shares.
pub trait GateSettings {
    fn enabled(&self) -> bool;
    fn severity(&self) -> Severity;
    fn exempt_paths(&self) -> &[String];
}

macro_rules! impl_gate_settings {
    ($($t:ty),*) => {$(
        impl GateSettings for $t {
            fn enabled(&self) -> bool { self.enabled }
            fn severity(&self) -> Severity { self.severity }
            fn exempt_paths(&self) -> &[String] { &self.exempt_paths }
        }
    )*};
}
impl_gate_settings!(
    BasicGate,
    ErrorSwallowingGate,
    HarnessTamperingGate,
    InstructionSmugglingGate,
    AgentsMdGate,
    IgnoredTestsGate,
    UnsafeSafetyCommentGate,
    AssertionGate,
    DeletionGate,
    TimeEstimateGate,
    PiiGate,
    ScratchGate,
    GoldenGate,
    BenchRegressionGate,
    CommandGate,
    DependencyDeltaGate,
    TestBudgetGate,
    TestFloorGate,
    CiIntegrityGate,
    CiSkipSetGate,
    ShellSecretsGate,
    IssueLinkGate,
    RatifiedPathsGate,
    ReviewThreadsGate,
    CommitProvenanceGate,
    CitationMetadataGate,
    ProvenanceTagsGate,
    ArchiveContentsGate,
    ManifestSyncGate,
    VersionLockstepGate,
    ScopeConfinementGate,
    SuppressionDeltaGate,
    PrChecklistGate,
    UnsafeBudgetGate,
    MsrvGate,
    MiriGate,
    SanitizersGate,
    GateCommandLintGate,
    MechanismSectionsGate,
    CitationAnchorsGate
);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsMdGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
}

impl Default for AgentsMdGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Warning,
            exempt_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BasicGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
}

impl Default for BasicGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
        }
    }
}

/// `harness-tampering`: the common keys, off until a repository turns it on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HarnessTamperingGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
}

impl Default for HarnessTamperingGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IgnoredTestsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Conditional ignore predicates (e.g. `miri`) that are approved by repository policy.
    pub approved_predicates: Vec<String>,
    /// Severity for skips conditioned on CI environment variables (defaults to gate severity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_skip_severity: Option<Severity>,
}

impl Default for IgnoredTestsGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            approved_predicates: Vec::new(),
            ci_skip_severity: None,
        }
    }
}

impl IgnoredTestsGate {
    pub fn ci_skip_severity(&self) -> Severity {
        self.ci_skip_severity.unwrap_or(self.severity)
    }
}

pub const DEFAULT_SAFETY_PLACEHOLDERS: &[&str] = &[
    "todo", "tbd", "n/a", "na", "none", "safe", "safety", "unsafe", "ok", "fine", "valid",
    "trust me", "trust", "me", "this", "is", "totally",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnsafeSafetyCommentGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub placeholders: Vec<String>,
}

impl Default for UnsafeSafetyCommentGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            placeholders: DEFAULT_SAFETY_PLACEHOLDERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssertionGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Extra macro names (final path segment) counted as assertions. A trailing `!` is
    /// accepted and removed at load.
    pub extra_assert_macros: Vec<String>,
    /// Function names (final path segment) whose call counts as an assertion,
    /// for suites that assert through helpers such as `check_invariants(&t)`.
    pub assert_helper_fns: Vec<String>,
    /// Minimum assertions required per test method (default: None).
    pub min_assertions_per_test: Option<usize>,
    /// Callee fragments that construct or program a test double, beyond the built-in
    /// vocabulary (`Mock(`, `jest.fn`, `when(`, `.Setup(`, ...).
    pub mock_setup_fns: Vec<String>,
    /// Callee fragments that assert on a double's interactions, beyond the built-in
    /// vocabulary (`assert_called_with`, `toHaveBeenCalled`, `verify(`, ...).
    pub mock_assert_fns: Vec<String>,
}

impl Default for AssertionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            extra_assert_macros: Vec::new(),
            assert_helper_fns: Vec::new(),
            min_assertions_per_test: None,
            mock_setup_fns: Vec::new(),
            mock_assert_fns: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeletionGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of paths whose deletion requires a rationale.
    pub paths: Vec<String>,
    /// When true (default), directives must name the specific file, directory, or test.
    /// When false, an unscoped removes: directive waives all deletions.
    #[serde(default = "default_true")]
    pub require_scope: bool,
    /// When Some(true), HTML-comment-wrapped directives are accepted for deletions.
    #[serde(default)]
    pub allow_hidden: Option<bool>,
}

impl Default for DeletionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            paths: vec!["**".to_string()],
            require_scope: true,
            allow_hidden: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeEstimateGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of files to scan.
    pub include: Vec<String>,
    /// Additional banned regexes.
    pub extra_patterns: Vec<String>,
    /// Text matched by any of these is not a violation. Patterns match per
    /// line and across soft-wrapped lines of a paragraph; the exemption covers
    /// only the matched text.
    pub allow_patterns: Vec<String>,
    pub scan_pr_body: bool,
    /// When true, scans only modified lines in the git diff rather than all tracked files.
    pub diff_only: bool,
}

impl Default for TimeEstimateGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Warning,
            exempt_paths: Vec::new(),
            include: vec!["**/*.md".to_string()],
            extra_patterns: Vec::new(),
            allow_patterns: Vec::new(),
            scan_pr_body: true,
            diff_only: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PiiGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub home_paths: bool,
    pub lan_ips: bool,
    /// When true, redacts private RFC 1918 LAN IP addresses in findings instead of echoing them for triage.
    pub redact_lan_ips: bool,
    pub secrets: bool,
    /// Home-directory user names that are not a leak (CI users, placeholders).
    pub allowed_users: Vec<String>,
    /// Hostnames that must never appear. Matched as whole tokens,
    /// case-insensitively, and never echoed back in reports.
    pub hostname_denylist: Vec<String>,
    /// Additional banned regexes (emails, internal domains, ticket prefixes ...).
    pub extra_patterns: Vec<String>,
    /// A line matching any of these is not a violation.
    pub allow_patterns: Vec<String>,
    pub scan_pr_body: bool,
    /// When true, scans only modified lines in the git diff rather than all tracked files.
    pub diff_only: bool,
    /// When true (default), flags references to personal agent configuration directories and playbooks.
    #[serde(default = "default_true")]
    pub agent_config_refs: bool,
    /// With `agent_config_refs`: a reference to an agent tool's home directory itself or
    /// to an entry the tool documents there (`~/.copilot/hooks/`, `~/.claude/settings.json`)
    /// is not reported; any other path under it still is. `false` reports every one.
    #[serde(default = "default_true")]
    pub agent_config_standard_paths: bool,
}

impl Default for PiiGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            home_paths: true,
            lan_ips: true,
            redact_lan_ips: false,
            secrets: true,
            allowed_users: [
                "runner", "user", "username", "you", "me", "name", "example", "shared",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            hostname_denylist: Vec::new(),
            extra_patterns: Vec::new(),
            allow_patterns: Vec::new(),
            scan_pr_body: true,
            diff_only: false,
            agent_config_refs: true,
            agent_config_standard_paths: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScratchGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of paths that must never be tracked.
    pub paths: Vec<String>,
}

impl Default for ScratchGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            // The shared hook configuration `discipline hook install` writes, and Cursor's
            // project MCP server list, are project configuration, not scratch state. A
            // change to them is still an agent-control change `instruction-smuggling`
            // reports.
            exempt_paths: [
                ".claude/settings.json",
                ".claude/hooks/discipline-bootstrap.sh",
                ".cursor/hooks.json",
                ".cursor/mcp.json",
                ".aider.conf.yml",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            paths: [
                ".claude/**",
                ".gemini/**",
                ".antigravity/**",
                ".cursor/**",
                ".aider*",
                "scratch/**",
                "**/*.session.*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GoldenGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of committed output / snapshot files guarded against unexcused edits.
    pub paths: Vec<String>,
}

impl Default for GoldenGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            paths: [
                "**/golden/**",
                "**/snapshots/**",
                "**/__snapshots__/**",
                "**/*.snap",
                "**/*.ambr",
                "**/*.golden",
                "**/*.approved.*",
                "tests/fixtures/**/output*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BenchRegressionGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub tolerance_pct: f64,
    pub paths: Vec<String>,
    pub provenance: Option<String>,
    pub allow_cross_host: bool,
    /// Maximum acceptable coefficient of variation (std_dev / mean). Baselines exceeding this trigger a stability warning.
    pub max_noise_cv: Option<f64>,
    /// Configurable noise margin added to tolerance_pct.
    pub noise_margin_pct: Option<f64>,
    /// In-job base benchmark result file path for dual-file regression checks.
    pub base_file: Option<String>,
    /// In-job head benchmark result file path for dual-file regression checks.
    pub head_file: Option<String>,
    /// Noise floor percentage (default: 0.5%). Arms regressing below this are ignored as noise.
    pub noise_floor_pct: Option<f64>,
    /// Advisory review percentage (default: 0.1%). Regressions above this render review notices in notes.
    pub advisory_pct: Option<f64>,
    /// Benchmark arms exempted from regression checks. Matches the exact name, the name
    /// as the benchmark prints it (`map_get random` matches `map_get/random`), a glob
    /// (`*.heap.*`, `*::random_*`), a trailing-`*` prefix, or a `::`/`/` path suffix.
    /// An entry that matches no arm in the run is an error, so a stale exemption cannot
    /// silently stop covering something.
    pub exempt_arms: Vec<String>,
    /// Require allow-regression directive reasons to carry a verifiable citation and arm names.
    /// Every citation the reason carries is also checked for freshness: a cited CI run must
    /// have completed, reached its regression guard, and measured a commit reachable from the
    /// head; a cited data artifact must post-date the branch's newest change under
    /// `citation_source_paths`. A citation that cannot be decided leaves the gate armed.
    pub require_sourced_override: bool,
    /// Paths (repo-relative files or directories) whose changes can move a gated number.
    /// A cited data artifact last committed before the branch's newest change under these
    /// paths describes the code the change replaced.
    pub citation_source_paths: Vec<String>,
    /// CI jobs that produce gated numbers, each with the one step that gates them. A cited run
    /// that concluded `failure` is admitted only when every one of these jobs that started
    /// reached its guard step with every earlier step green.
    pub citation_measurement_jobs: Vec<MeasurementJob>,
    /// Evaluation mode: `version-vs-version` (base and head artifacts, the default) or
    /// `paired-ratio` (a ratio of two arms measured in the same interleaved rounds, compared
    /// against a committed ratio baseline).
    pub mode: BenchMode,
    /// Committed paired-ratio baseline (the threshold file). Read from the base ref, never
    /// from head.
    pub ratio_baseline: Option<String>,
    /// Optional minimum paired-ratio threshold in percent. It only ever widens a derived
    /// floor; configured for an axis the baseline has no derived floor for, it is an error.
    pub ratio_tolerance_pct: Option<f64>,
}

/// Evaluation mode of the `bench-regression` gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BenchMode {
    /// Base and head benchmark artifacts of the same arms (the preferred model).
    #[default]
    VersionVsVersion,
    /// A paired within-run ratio against a committed ratio baseline.
    PairedRatio,
}

/// A CI job that produces gated numbers and the step in it that gates them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementJob {
    /// Job display name as the CI API lists it.
    pub job: String,
    /// Name of the step in that job that reads the numbers and enforces the guard.
    pub guard: String,
}

impl Default for BenchRegressionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Warning,
            exempt_paths: [
                ".github/**",
                ".gitea/**",
                ".forgejo/**",
                ".gitlab/**",
                "docs/**",
                "research/**",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            tolerance_pct: 0.5,
            paths: [
                "target/iai/**",
                "**/callgrind.*",
                "target/criterion/**",
                "**/*bench*.json",
                "**/*bench*.log",
                "**/*bench*.txt",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            provenance: None,
            allow_cross_host: false,
            max_noise_cv: None,
            noise_margin_pct: None,
            base_file: None,
            head_file: None,
            noise_floor_pct: Some(0.5),
            advisory_pct: Some(0.1),
            exempt_arms: Vec::new(),
            require_sourced_override: false,
            citation_source_paths: Vec::new(),
            citation_measurement_jobs: Vec::new(),
            mode: BenchMode::VersionVsVersion,
            ratio_baseline: None,
            ratio_tolerance_pct: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvenanceTagsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Check markdown tables for unit-bearing numbers without table or caption provenance tags.
    pub check_tables: bool,
    /// Check for mechanism claims without hardware counter evidence or explicit hypothesis qualifiers.
    pub check_mechanisms: bool,
    /// Check published wall-clock ratios for confidence intervals or explicit qualifiers.
    pub check_intervals: bool,
    /// Check paired figures (e.g. 11.9 ns vs 108.9 ns) for shared workload IDs or differentiation tags.
    pub check_paired_figures: bool,
    /// Repository path (read at HEAD) of a JSON registry of withdrawn figures. A registered
    /// figure may be republished only next to a retraction marker.
    pub superseded_registry: Option<String>,
    /// Globs of tracked JSON datasets swept for registered figures.
    pub superseded_json_paths: Vec<String>,
    /// A pending-measurement statement must cite a tracking issue.
    pub check_pending_citations: bool,
    /// A pending-measurement statement must cite at least one open issue, read from the forge.
    /// Implies `check_pending_citations`.
    pub require_open_pending_issues: bool,
    /// Other repositories (`owner/name`) whose issues a pending statement may cite. By
    /// default only this repository's issues count.
    pub pending_issue_repos: Vec<String>,
    /// What satisfies a published wall-clock ratio, replacing the built-in list when set:
    /// `interval` (a `[lo, hi]` / BCa / CI mention), `marker:<word>`, `artifact:<glob>`
    /// (a path reference matching the glob), `regex:<pattern>`. Paragraph-scoped.
    pub ratio_satisfied_by: Vec<String>,
    /// Units whose figures are deterministic and exempt from the interval requirement,
    /// added to the built-in list (instructions, cycles, bytes, allocations).
    pub deterministic_units: Vec<String>,
    /// Judge only paragraphs that contain an added line (default: whole changed file).
    pub diff_only: bool,
    /// A `(measured: <host>, <commit>)` tag on an added line must name a host and a commit
    /// that resolves in the local object database.
    pub verify_measured_commit: bool,
    /// Globs of JSON / JSONL result records whose commit key must be a full object id that
    /// resolves to a commit.
    pub record_paths: Vec<String>,
    /// Key of a result record that holds the commit it was measured at.
    pub record_commit_key: String,
    /// Each figure of a paragraph or table carrying a `measured` tag must equal a numeric
    /// value of the tracked data artifact (`.json`, `.jsonl`, `.csv`) the paragraph cites.
    pub verify_cited_figures: bool,
    /// Relative tolerance, in percent, added to rounding when a figure is compared with
    /// the artifact's values.
    pub figure_tolerance_pct: f64,
}

impl Default for ProvenanceTagsGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            check_tables: true,
            check_mechanisms: true,
            check_intervals: true,
            check_paired_figures: true,
            superseded_registry: None,
            superseded_json_paths: Vec::new(),
            check_pending_citations: false,
            require_open_pending_issues: false,
            pending_issue_repos: Vec::new(),
            ratio_satisfied_by: Vec::new(),
            deterministic_units: Vec::new(),
            diff_only: false,
            verify_measured_commit: false,
            record_paths: Vec::new(),
            record_commit_key: "commit".to_string(),
            verify_cited_figures: false,
            figure_tolerance_pct: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Predefined turnkey preset name (e.g. cargo-mutants, cargo-deny, loom).
    pub preset: Option<String>,
    /// Primary command to execute.
    pub command: Option<String>,
    /// Execution timeout in seconds (default: 60s). Exceeding this triggers exit 2.
    pub timeout_seconds: Option<u64>,
    /// Regex pattern to extract an integer count (e.g. `test result: ok. (\\d+) passed`).
    pub count_pattern: Option<String>,
    /// Minimum count required. If base ref has a higher count, the base count acts as ratchet floor.
    pub min_count: Option<u64>,
    /// Regular expressions that must NOT match stdout or stderr. One that does not compile
    /// is a configuration error, never matched as literal text.
    pub forbid_output: Vec<String>,
    /// Regular expression whose match means zero items were executed (e.g. `running 0 tests`).
    pub zero_items_pattern: Option<String>,
    /// Whether zero items selected is allowed (default: false).
    pub allow_zero: bool,
    /// Optional negative-control canary command.
    pub canary_command: Option<String>,
    /// Regular expression the canary's output MUST match.
    pub canary_expected_diagnostic: Option<String>,
    /// Repository-relative path of a committed file the command's stdout must match.
    pub snapshot: Option<String>,
    /// Regexes for lines left out of the snapshot comparison, on both sides.
    pub snapshot_ignore: Vec<String>,
    /// Multi-command suite support.
    pub commands: Vec<CommandEntry>,
}

impl Default for CommandGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            preset: None,
            command: None,
            timeout_seconds: None,
            count_pattern: None,
            min_count: None,
            forbid_output: Vec::new(),
            zero_items_pattern: None,
            allow_zero: false,
            canary_command: None,
            canary_expected_diagnostic: None,
            snapshot: None,
            snapshot_ignore: Vec::new(),
            commands: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandEntry {
    pub name: String,
    pub preset: Option<String>,
    pub command: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub count_pattern: Option<String>,
    pub min_count: Option<u64>,
    pub forbid_output: Vec<String>,
    pub zero_items_pattern: Option<String>,
    pub allow_zero: bool,
    pub canary_command: Option<String>,
    pub canary_expected_diagnostic: Option<String>,
    /// Repository-relative path of a committed file the command's stdout must match.
    pub snapshot: Option<String>,
    /// Regexes for lines left out of the snapshot comparison, on both sides.
    pub snapshot_ignore: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DependencyDeltaGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Manifest file globs to inspect (default covers Cargo.toml, package.json, pyproject.toml, go.mod, etc.).
    pub manifests: Vec<String>,
    /// Whether wildcard versions ("*", "latest", "") are permitted (default: false).
    pub allow_wildcards: bool,
    /// Whether git dependencies must specify an immutable commit or tag pin (default: true).
    pub require_git_pins: bool,
    /// Path to deny.toml policy file (default: "deny.toml").
    pub deny_file: Option<String>,
    /// Explicit list of allowed dependency package names.
    pub allow_dependencies: Vec<String>,
    /// Explicit list of forbidden dependency package names.
    pub deny_dependencies: Vec<String>,
}

impl Default for DependencyDeltaGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            manifests: [
                "**/Cargo.toml",
                "**/package.json",
                "**/pyproject.toml",
                "**/requirements*.txt",
                "**/go.mod",
                "**/composer.json",
                "**/Gemfile",
                "**/*.csproj",
                "**/Directory.Packages.props",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            allow_wildcards: false,
            require_git_pins: true,
            deny_file: Some("deny.toml".to_string()),
            allow_dependencies: Vec::new(),
            deny_dependencies: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TestBudgetGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Corpus directory patterns to monitor for seed file shrink (default: ["fuzz/corpus/**", "corpus/**", "**/tests/corpus/**"]).
    pub corpus_dirs: Vec<String>,
    /// Globs of fuzz manifests and Rust harness files outside the root `fuzz/` crate, which
    /// is always watched (default: ["fuzz/Cargo.toml", "fuzz/fuzz_targets/**"]).
    pub fuzz_targets: Vec<String>,
    /// Whether to scan workflow files (.github/workflows, .gitlab-ci.yml) (default: true).
    pub scan_workflows: bool,
    /// Whether to scan shell scripts (*.sh, *.bash) (default: true).
    pub scan_scripts: bool,
}

impl Default for TestBudgetGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            corpus_dirs: vec![
                "fuzz/corpus/**".to_string(),
                "corpus/**".to_string(),
                "**/tests/corpus/**".to_string(),
            ],
            fuzz_targets: vec![
                "fuzz/Cargo.toml".to_string(),
                "fuzz/fuzz_targets/**".to_string(),
            ],
            scan_workflows: true,
            scan_scripts: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellSecretsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub extra_secret_patterns: Vec<String>,
    pub allow_patterns: Vec<String>,
    /// When true, scans only modified lines in the git diff rather than all tracked files.
    pub diff_only: bool,
}

impl Default for ShellSecretsGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            extra_secret_patterns: Vec::new(),
            allow_patterns: Vec::new(),
            diff_only: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IssueLinkGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub pattern: Option<String>,
    pub require_in_commit_if_no_pr: bool,
    /// Look each reference up on the forge: at least one must be an issue of this
    /// repository (or of `reference_repos`). Needs forge access.
    pub verify_references: bool,
    /// With `verify_references`: a closed issue does not satisfy the gate.
    pub require_open_issue: bool,
    /// With `verify_references`: other repositories (`owner/repo`, a GitLab project path)
    /// a reference may resolve in.
    pub reference_repos: Vec<String>,
    /// With `verify_references`: a reference to a pull or merge request satisfies the gate.
    pub accept_pull_references: bool,
    /// Whether `no-issue: <reason>` is accepted.
    pub waiver: IssueWaiver,
    /// Pull-request authors (exact logins, case-insensitive) whose pull requests need no
    /// tracking-issue reference: dependency-update bots. Matched against the author the
    /// forge's event payload names (on GitLab, the merge request's), never the actor of the run.
    pub exempt_authors: Vec<String>,
}

/// Edits to protected paths need an owner's ratification: a comment, by a listed human
/// login, on an issue the pull request closes, naming each path exactly. Off by default:
/// who ratifies and what is protected are the repository's own policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RatifiedPathsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of the paths whose edits need a ratification.
    pub protected_paths: Vec<String>,
    /// Globs of paths no ratification can cover (the CI workflow directories).
    pub never_ratifiable: Vec<String>,
    /// Logins whose comments ratify.
    pub ratifiers: Vec<String>,
    /// Logins that never ratify, even when also listed in `ratifiers`.
    pub agent_logins: Vec<String>,
    /// The line that starts a ratification block.
    pub marker: String,
    /// Where the closing references come from.
    pub closing_source: ClosingSource,
    /// Closing keywords for `closing_source = "body"`; empty means the forge's own.
    pub closing_keywords: Vec<String>,
    /// A closed issue carries no ratification.
    pub require_open_issue: bool,
    /// Other repositories whose issues may carry a ratification.
    pub ratification_repos: Vec<String>,
    /// How old a ratification may be.
    pub ratification_valid_from: RatificationWindow,
    /// Most days a ratification stays valid; unset means no cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratification_max_age_days: Option<u64>,
    /// Whether an edited comment can still ratify.
    pub accept_edited: AcceptEdited,
    /// Whether a comment created by an email reply (GitHub) can ratify.
    pub accept_email_replies: bool,
    /// A ratification written by the pull request's own author is refused, so a second
    /// login must agree. Off by default: it only means something once the agents open
    /// pull requests under a login of their own.
    pub refuse_author_ratification: bool,
}

impl Default for RatifiedPathsGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            protected_paths: Vec::new(),
            never_ratifiable: vec![
                ".github/**".to_string(),
                ".gitea/**".to_string(),
                ".forgejo/**".to_string(),
                ".gitlab-ci.yml".to_string(),
                ".gitlab/**".to_string(),
            ],
            ratifiers: Vec::new(),
            agent_logins: Vec::new(),
            marker: "Owner-ratified-paths:".to_string(),
            closing_source: ClosingSource::Server,
            closing_keywords: Vec::new(),
            require_open_issue: true,
            ratification_repos: Vec::new(),
            ratification_valid_from: RatificationWindow::PathLastChanged,
            ratification_max_age_days: None,
            accept_edited: AcceptEdited::Never,
            accept_email_replies: false,
            refuse_author_ratification: false,
        }
    }
}

/// The pull request has no unresolved review thread. Off by default: it reads the forge,
/// and GitHub and GitLab can enforce the same at merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewThreadsGate {
    pub enabled: bool,
    pub severity: Severity,
    /// Threads on these paths are not counted.
    pub exempt_paths: Vec<String>,
}

impl Default for ReviewThreadsGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
        }
    }
}

/// Where `ratified-paths` reads the issues a pull request closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClosingSource {
    /// The forge's own list where it has one (GitHub `closingIssuesReferences`, GitLab
    /// `closes_issues`); the body elsewhere.
    Server,
    /// The pull request's body, parsed with the closing keywords.
    Body,
}

/// How old a ratification may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RatificationWindow {
    /// Newer than the last change to the path on the base branch.
    PathLastChanged,
    /// Newer than the pull request.
    PullCreated,
    /// Any age.
    Any,
}

/// Whether an edited comment can ratify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptEdited {
    Never,
    /// Only when the forge names the editor and it is the author (GitHub).
    ByAuthor,
}

/// Whether the `issue-link` waiver directive is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueWaiver {
    /// `no-issue: <reason>` lifts the finding.
    Directive,
    /// No waiver: every pull request references an issue.
    None,
}

/// Checks `CITATION.cff` and `.zenodo.json` at the repository root; with neither
/// present it examines nothing, so it ships enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CitationMetadataGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
}

impl Default for CitationMetadataGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommitProvenanceGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Trailer keys every commit in the change must carry (`Signed-off-by`, `Agent-Tool`).
    pub required_trailers: Vec<String>,
    /// Substrings (case-insensitive) of a trailer line, the author name or the author
    /// email that identify an agent-produced commit.
    pub agent_markers: Vec<String>,
    /// Trailer an agent-produced commit must carry, naming someone other than its
    /// author. An empty name in a configuration file is read as `require_agent_review = false`.
    pub review_trailer: String,
    /// Whether an agent-produced commit must carry `review_trailer`. The trailer is the
    /// change's own claim; separating reviewer from author is the forge's required
    /// review, which `discipline doctor` reports.
    pub require_agent_review: bool,
}

impl Default for CommitProvenanceGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            required_trailers: Vec::new(),
            agent_markers: [
                "Agent-Tool:",
                "Agent:",
                "Generated-by:",
                "Co-authored-by: Claude",
                "Co-authored-by: Copilot",
                "Co-authored-by: Gemini",
                "Co-authored-by: Codex",
                "Co-authored-by: Cursor",
                "Co-authored-by: aider",
                "[bot]",
                "noreply@anthropic.com",
                "noreply@openai.com",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            review_trailer: "Reviewed-by".to_string(),
            require_agent_review: true,
        }
    }
}

impl Default for IssueLinkGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            pattern: None,
            require_in_commit_if_no_pr: false,
            verify_references: false,
            require_open_issue: true,
            reference_repos: Vec::new(),
            accept_pull_references: false,
            waiver: IssueWaiver::Directive,
            exempt_authors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TestFloorGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub min_tests: Option<usize>,
    /// Allowed test count decrease below floor or base before violation (default: 0).
    pub tolerance: usize,
    pub constant_file: Option<String>,
    pub constant_name: Option<String>,
    pub required_suites: Vec<String>,
    pub test_command: Option<String>,
    pub test_report: Option<String>,
    pub base_report: Option<String>,
    pub head_report: Option<String>,
}

impl Default for TestFloorGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            min_tests: None,
            tolerance: 0,
            constant_file: None,
            constant_name: None,
            required_suites: Vec::new(),
            test_command: None,
            test_report: None,
            base_report: None,
            head_report: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CiIntegrityGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub workflows: Vec<String>,
    pub rollup_job: Option<String>,
    pub excluded_jobs: Vec<String>,
    pub pin_actions: bool,
    pub forbid_continue_on_error: bool,
    pub forbid_or_true: bool,
    pub diff_only: bool,
    pub documented_job_count_path: Option<String>,
    pub documented_job_count_pattern: Option<String>,
    /// Action owners exempt from the commit-SHA rule. Empty by default: GitHub's
    /// "require actions to be pinned to a full-length commit SHA" policy exempts no
    /// owner, so `actions/` and `github/` tag refs are reported like any other.
    pub first_party_action_prefixes: Vec<String>,
    /// Actions and reusable workflows that must not be referenced anywhere in the tree:
    /// `owner/repo` (every ref), `owner/repo@ref`, or a table `{ uses = "...", reason =
    /// "..." }`. Checked against every scanned file whatever `diff_only` says; no
    /// directive lifts the finding.
    pub banned_actions: Vec<BannedAction>,
}

/// One `banned_actions` entry. Written as a string (`"owner/repo@ref"`) or a table with
/// `uses` and an optional `reason`; both read to this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BannedAction {
    /// `owner/repo`, `owner/repo/path`, optionally `@ref`.
    pub uses: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl BannedAction {
    /// The entry split into its target (`owner/repo[/path]`) and its ref, if one is named.
    pub fn target_and_ref(&self) -> (&str, Option<&str>) {
        match self.uses.split_once('@') {
            Some((t, r)) => (t, Some(r)),
            None => (self.uses.as_str(), None),
        }
    }
}

/// Why a `banned_actions` value is not a remote action reference, or `None` when it is.
pub fn banned_action_problem(uses: &str) -> Option<&'static str> {
    let segment = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if uses.starts_with("./") || uses.starts_with("docker://") {
        return Some("a local path or a docker:// image is not a remote action");
    }
    let (target, r) = match uses.split_once('@') {
        Some((t, r)) => (t, Some(r)),
        None => (uses, None),
    };
    let parts: Vec<&str> = target.split('/').collect();
    if parts.len() < 2 || !parts.iter().all(|p| segment(p)) {
        return Some("expected `owner/repo` or `owner/repo/path`, optionally followed by `@ref`");
    }
    if let Some(r) = r {
        if r.is_empty()
            || !r
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+'))
        {
            return Some("the ref after `@` is empty or holds characters a git ref cannot");
        }
    }
    None
}

impl<'de> Deserialize<'de> for BannedAction {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Entry {
            uses: String,
            #[serde(default)]
            reason: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Ref(String),
            Entry(Entry),
        }
        let entry = match Raw::deserialize(d).map_err(|_| {
            serde::de::Error::custom(
                "a banned_actions entry is a string \"owner/repo[@ref]\" or a table { uses = \"owner/repo[@ref]\", reason = \"...\" }",
            )
        })? {
            Raw::Ref(uses) => BannedAction { uses, reason: None },
            Raw::Entry(e) => BannedAction {
                uses: e.uses,
                reason: e.reason,
            },
        };
        if let Some(why) = banned_action_problem(&entry.uses) {
            return Err(serde::de::Error::custom(format!(
                "banned_actions entry `{}`: {why}",
                entry.uses
            )));
        }
        Ok(entry)
    }
}

impl Default for CiIntegrityGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            // Every Actions-shaped workflow directory `doctor::WORKFLOW_DIRS` knows.
            workflows: crate::doctor::WORKFLOW_DIRS
                .iter()
                .flat_map(|dir| [format!("{dir}/*.yml"), format!("{dir}/*.yaml")])
                .chain(
                    [
                        ".gitlab-ci.yml",
                        ".gitlab/ci/*.yml",
                        ".gitlab/ci/*.yaml",
                        // Composite action metadata: only their nested `uses:` are checked.
                        ".github/actions/**/action.yml",
                        ".github/actions/**/action.yaml",
                        "action.yml",
                        "action.yaml",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                )
                .collect(),
            rollup_job: Some("ci-gate".to_string()),
            excluded_jobs: vec!["detect-changes".to_string()],
            pin_actions: true,
            forbid_continue_on_error: true,
            forbid_or_true: true,
            diff_only: true,
            documented_job_count_path: None,
            documented_job_count_pattern: None,
            first_party_action_prefixes: Vec::new(),
            banned_actions: Vec::new(),
        }
    }
}

/// `ci-skip-set`: runtime check of a rollup job's skip set. It reads the
/// rollup's `needs` context from `DISCIPLINE_CI_CONTEXT`; with no context it
/// reports a named "not evaluated" note and never passes or fails silently.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CiSkipSetGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Repo-relative path of the workflow whose rollup supplies the context.
    pub workflow: String,
    /// Change-detection job whose outputs gate the conditional jobs. It must
    /// have succeeded. Empty string = the workflow has no such job.
    pub change_job: String,
    /// Jobs that must never be `skipped`, whatever their dependencies did.
    pub unconditional_jobs: Vec<String>,
}

impl Default for CiSkipSetGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            workflow: ".github/workflows/ci.yml".to_string(),
            change_job: "detect-changes".to_string(),
            unconditional_jobs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArchiveContentsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub archive_path: Option<String>,
    pub required_paths: Vec<String>,
    pub forbidden_patterns: Vec<String>,
    pub strip_components: usize,
    /// Read each entry's bytes and report source maps that embed the original
    /// source (`sourcesContent`), inline or as `.map` entries.
    pub scan_contents: bool,
    /// Entries larger than this are not scanned; they are named in a note.
    pub max_entry_bytes: u64,
    /// A named `forbidden_patterns` list (`no-source`, `no-source-npm`, ...)
    /// merged with the configured patterns.
    pub preset: Option<String>,
}

/// Default `max_entry_bytes`: 16 MiB.
pub const ARCHIVE_MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;

impl Default for ArchiveContentsGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            archive_path: None,
            required_paths: Vec::new(),
            forbidden_patterns: Vec::new(),
            strip_components: 0,
            scan_contents: false,
            max_entry_bytes: ARCHIVE_MAX_ENTRY_BYTES,
            preset: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManifestSyncGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub rules: Vec<ManifestSyncRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSyncRule {
    pub manifest: String,
    pub extract_regex: String,
    pub watched_paths: Vec<String>,
    #[serde(default)]
    pub exclude_paths: Vec<String>,
}

impl Default for ManifestSyncGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VersionLockstepGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub groups: Vec<VersionGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionGroup {
    pub name: String,
    pub sources: Vec<VersionSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSource {
    pub path: String,
    pub regex: String,
}

impl Default for VersionLockstepGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            groups: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScopeConfinementGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub forbidden_paths: Vec<String>,
    pub check_plans: bool,
    pub plan_paths: Vec<String>,
    pub require_declared_outputs: bool,
}

impl Default for ScopeConfinementGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            allowed_paths: Vec::new(),
            forbidden_paths: Vec::new(),
            check_plans: false,
            plan_paths: vec![
                "plans/**/*.md".to_string(),
                "docs/plans/**/*.md".to_string(),
            ],
            require_declared_outputs: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuppressionDeltaGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub max_increase: usize,
    pub allowed_suppressions: Vec<String>,
}

impl Default for SuppressionDeltaGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Warning,
            exempt_paths: Vec::new(),
            max_increase: 0,
            allowed_suppressions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrChecklistGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
}

impl Default for PrChecklistGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnsafeBudgetGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub max_unsafe: Option<usize>,
    pub allow_increase: bool,
}

impl Default for UnsafeBudgetGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            max_unsafe: None,
            allow_increase: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MsrvGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub pinned_version: Option<String>,
    pub command: Option<String>,
}

impl Default for MsrvGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            pinned_version: None,
            command: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MiriGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub args: Vec<String>,
    pub timeout_seconds: u64,
}

impl Default for MiriGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            args: Vec::new(),
            timeout_seconds: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SanitizersGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub sanitizer: String,
    pub canary: bool,
    pub timeout_seconds: u64,
}

impl Default for SanitizersGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            sanitizer: "address".to_string(),
            canary: false,
            timeout_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GateCommandLintGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub plan_paths: Vec<String>,
    pub allow_pipeline_without_pipefail: bool,
}

impl Default for GateCommandLintGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            plan_paths: vec![
                "plans/**/*.md".to_string(),
                "docs/plans/**/*.md".to_string(),
            ],
            allow_pipeline_without_pipefail: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MechanismSectionsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub diff_only: bool,
    pub section_headings: Vec<String>,
}

impl Default for MechanismSectionsGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            diff_only: true,
            section_headings: vec![
                "Mechanism".to_string(),
                "Root cause".to_string(),
                "Cause".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnanchoredCitationsMode {
    Ignore,
    Warn,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CitationAnchorsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    pub diff_only: bool,
    pub unanchored_citations: UnanchoredCitationsMode,
    pub verify_tracker_titles: bool,
    pub require_online: bool,
}

impl Default for CitationAnchorsGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            diff_only: true,
            unanchored_citations: UnanchoredCitationsMode::Warn,
            verify_tracker_titles: false,
            require_online: false,
        }
    }
}

impl Gates {
    pub fn settings(&self, id: &str) -> Option<&dyn GateSettings> {
        Some(match id {
            "agents-md" => &self.agents_md,
            "assertion-reduction" => &self.assertion_reduction,
            "vacuous-tests" => &self.vacuous_tests,
            "ignored-tests" => &self.ignored_tests,
            "unsafe-safety-comment" => &self.unsafe_safety_comment,
            "deletion-rationale" => &self.deletion_rationale,
            "time-estimates" => &self.time_estimates,
            "pii" => &self.pii,
            "agent-scratch" => &self.agent_scratch,
            "shell-secrets" => &self.shell_secrets,
            "issue-link" => &self.issue_link,
            "ratified-paths" => &self.ratified_paths,
            "review-threads" => &self.review_threads,
            "commit-provenance" => &self.commit_provenance,
            "citation-metadata" => &self.citation_metadata,
            "config-integrity" => &self.config_integrity,
            "toolchain-config" => &self.toolchain_config,
            "sandbox-config" => &self.sandbox_config,
            "stub-bodies" => &self.stub_bodies,
            "error-swallowing" => &self.error_swallowing,
            "harness-tampering" => &self.harness_tampering,
            "instruction-smuggling" => &self.instruction_smuggling,
            "build-hooks" => &self.build_hooks,
            "golden-output" => &self.golden_output,
            "bench-regression" => &self.bench_regression,
            "command" => &self.command,
            "dependency-delta" => &self.dependency_delta,
            "test-budget" => &self.test_budget,
            "test-floor" => &self.test_floor,
            "ci-integrity" => &self.ci_integrity,
            "ci-skip-set" => &self.ci_skip_set,
            "provenance-tags" => &self.provenance_tags,
            "archive-contents" => &self.archive_contents,
            "manifest-sync" => &self.manifest_sync,
            "version-lockstep" => &self.version_lockstep,
            "scope-confinement" => &self.scope_confinement,
            "suppression-delta" => &self.suppression_delta,
            "pr-checklist" => &self.pr_checklist,
            "unsafe-budget" => &self.unsafe_budget,
            "msrv" => &self.msrv,
            "miri" => &self.miri,
            "sanitizers" => &self.sanitizers,
            "gate-command-lint" => &self.gate_command_lint,
            "mechanism-sections" => &self.mechanism_sections,
            "citation-anchors" => &self.citation_anchors,
            _ => return None,
        })
    }
}

/// Everything above `discipline.toml` in the precedence order.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub config_override: Option<String>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
    pub hostname_denylist: Vec<String>,
    pub directive_sources: Option<Vec<String>>,
    pub fail_on_overrides: Option<bool>,
}

impl DisciplineConfig {
    pub fn default_for_repo(name: &str) -> Self {
        Self {
            meta: MetaConfig {
                version: SCHEMA_VERSION,
                name: name.to_string(),
                description: None,
                mode: RunMode::Enforcing,
            },
            directives: DirectivesConfig::default(),
            tests: TestsConfig::default(),
            languages: LanguagesConfig::default(),
            gates: Gates::default(),
            hooks: HooksConfig::default(),
            deprecations: Vec::new(),
        }
    }

    /// Parse a `discipline.toml` body with no overrides applied.
    pub fn from_toml_str(content: &str) -> Result<Self> {
        let value: Value = match toml::from_str(content) {
            Ok(v) => v,
            Err(e) => {
                bail!(
                    "{}",
                    format_toml_error("discipline.toml is not valid TOML", content, &e)
                );
            }
        };
        match Self::from_value(value) {
            Ok(cfg) => Ok(cfg),
            Err(orig_err) => {
                if orig_err.to_string().contains("failed schema validation") {
                    if let Err(direct_err) = toml::from_str::<DisciplineConfig>(content) {
                        bail!(
                            "{}",
                            format_toml_error(
                                "discipline configuration failed schema validation",
                                content,
                                &direct_err
                            )
                        );
                    }
                }
                Err(orig_err)
            }
        }
    }

    /// Parse a `discipline.toml` body read from a repository's history, for
    /// `discipline audit`: a key in [`REMOVED_KEYS`] is dropped instead of refused, and
    /// returned by its dotted name with the value it had. Everything else is validated as
    /// [`Self::from_toml_str`] validates it, so a name that was never a key is an error.
    pub fn from_history_toml_str(content: &str) -> Result<(Self, Vec<(String, String)>)> {
        let removed = match toml::from_str::<Value>(content) {
            Ok(mut value) => {
                let removed = take_removed_keys(&mut value);
                if !removed.is_empty() {
                    return Ok((Self::from_value(value)?, removed));
                }
                removed
            }
            // Not TOML: the strict reader below says so.
            Err(_) => Vec::new(),
        };
        Ok((Self::from_toml_str(content)?, removed))
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::resolve(Some(path.as_ref()), &Overrides::default())
    }

    /// Resolve the effective configuration. `path = None` starts from defaults.
    pub fn resolve(path: Option<&Path>, overrides: &Overrides) -> Result<Self> {
        let source = match path {
            Some(p) => Some((
                p,
                std::fs::read_to_string(p).with_context(|| {
                    format!("failed to read configuration file {}", p.display())
                })?,
            )),
            None => None,
        };
        Self::resolve_source(source, overrides)
    }

    /// [`Self::resolve`] over content already in hand (a blob read from the base ref).
    /// The path only labels diagnostics.
    pub fn resolve_source(source: Option<(&Path, String)>, overrides: &Overrides) -> Result<Self> {
        let mut source_info: Option<(std::path::PathBuf, String)> = None;
        let mut value = match source {
            Some((p, content)) => {
                let val = match toml::from_str::<Value>(&content) {
                    Ok(v) => v,
                    Err(e) => {
                        bail!(
                            "{}",
                            format_toml_error(
                                &format!("{} is not valid TOML", p.display()),
                                &content,
                                &e
                            )
                        );
                    }
                };
                source_info = Some((p.to_path_buf(), content));
                val
            }
            None => Value::try_from(Self::default_for_repo("workspace"))?,
        };

        if value.get("directives").is_none() {
            if let Some(table) = value.as_table_mut() {
                if let Ok(def_dir) = Value::try_from(DirectivesConfig::default()) {
                    table.insert("directives".to_string(), def_dir);
                }
            }
        }
        if value.get("tests").is_none() {
            if let Some(table) = value.as_table_mut() {
                if let Ok(def) = Value::try_from(TestsConfig::default()) {
                    table.insert("tests".to_string(), def);
                }
            }
        }

        if let Some(extra) = &overrides.config_override {
            let extra: Value =
                toml::from_str(extra).context("config override is not valid TOML")?;
            merge(&mut value, extra);
        }

        for (ids, enabled) in [(&overrides.enable, true), (&overrides.disable, false)] {
            for id in ids {
                check_gate_id(id)?;
                set_path(
                    &mut value,
                    &["gates", id, "enabled"],
                    Value::Boolean(enabled),
                );
            }
        }
        if let Some(both) = overrides
            .enable
            .iter()
            .find(|id| overrides.disable.contains(id))
        {
            bail!("gate `{both}` is listed in both --enable and --disable");
        }

        if !overrides.hostname_denylist.is_empty() {
            let extra = Value::Array(
                overrides
                    .hostname_denylist
                    .iter()
                    .map(|h| Value::String(h.clone()))
                    .collect(),
            );
            let mut layer = Value::Table(Default::default());
            set_path(&mut layer, &["gates", "pii", "hostname_denylist"], extra);
            merge(&mut value, layer);
        }

        if let Some(sources) = &overrides.directive_sources {
            let filtered: Vec<Value> = sources
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_string()))
                .collect();
            if !filtered.is_empty() {
                set_path(
                    &mut value,
                    &["directives", "sources"],
                    Value::Array(filtered),
                );
            }
        }
        if let Some(fail) = overrides.fail_on_overrides {
            set_path(
                &mut value,
                &["directives", "fail_on_overrides"],
                Value::Boolean(fail),
            );
        }

        match Self::from_value(value) {
            Ok(cfg) => Ok(cfg),
            Err(orig_err) => {
                if orig_err.to_string().contains("failed schema validation") {
                    if let Some((p, content)) = source_info {
                        if let Err(direct_err) = toml::from_str::<DisciplineConfig>(&content) {
                            bail!(
                                "{}",
                                format_toml_error(
                                    &format!("{} failed schema validation", p.display()),
                                    &content,
                                    &direct_err
                                )
                            );
                        }
                    }
                }
                Err(orig_err)
            }
        }
    }

    /// Spellings with one reading are brought to the form the gates match: a macro
    /// name in `extra_assert_macros` written with its `!` (`assert_matches!`) is the
    /// same macro as `assert_matches`, which is what the parsers compare against.
    fn normalize(&mut self) {
        for gate in [
            &mut self.gates.assertion_reduction,
            &mut self.gates.vacuous_tests,
        ] {
            for m in &mut gate.extra_assert_macros {
                let name = m.trim().trim_end_matches('!').trim_end();
                if name.len() != m.len() {
                    *m = name.to_string();
                }
            }
        }
    }

    fn from_value(value: Value) -> Result<Self> {
        Self::from_value_with_aliases(value, KEY_ALIASES)
    }

    /// [`Self::from_toml_str`] under another alias table. For tests of the rename
    /// mechanism, which ships with no renamed key.
    #[doc(hidden)]
    pub fn from_toml_str_with_aliases(content: &str, aliases: &[KeyAlias]) -> Result<Self> {
        let value: Value = toml::from_str(content)?;
        Self::from_value_with_aliases(value, aliases)
    }

    fn from_value_with_aliases(mut value: Value, aliases: &[KeyAlias]) -> Result<Self> {
        let deprecations = apply_key_aliases(&mut value, aliases)?;
        // Name planned gates explicitly: "unknown field" would read as a typo,
        // and a user must learn the gate exists but is not shipped yet.
        if let Some(gates) = value.get("gates").and_then(Value::as_table) {
            for id in gates.keys() {
                check_gate_id(id)?;
            }
        }
        let mut deprecations = deprecations;
        if let Some(note) = rewrite_empty_review_trailer(&mut value)? {
            deprecations.push(note);
        }
        let mut config: DisciplineConfig = value
            .try_into()
            .context("discipline configuration failed schema validation")?;
        config.normalize();
        config.deprecations = deprecations;
        if config.meta.version != SCHEMA_VERSION {
            bail!(
                "unsupported [meta] version {} (this binary understands version {})",
                config.meta.version,
                SCHEMA_VERSION
            );
        }
        Ok(config)
    }
}

/// Helper to convert a byte offset in TOML content to 1-based (line, column).
pub fn byte_offset_to_line_col(content: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in content.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Helper to format a `toml::de::Error` with human-readable line and column spans.
fn format_toml_error(prefix: &str, content: &str, err: &toml::de::Error) -> String {
    if let Some(range) = err.span() {
        let (line, col) = byte_offset_to_line_col(content, range.start);
        format!("{prefix} at line {line}, column {col}: {err}")
    } else {
        format!("{prefix}: {err}")
    }
}

fn check_gate_id(id: &str) -> Result<()> {
    match gate_info(id) {
        Some(g) if g.available => Ok(()),
        Some(_) => Err(anyhow!(
            "gate `{id}` is planned but not available in this version of discipline; \
             it cannot be enabled or configured yet"
        )),
        None => Err(anyhow!(
            "unknown gate `{id}` (run `discipline gates` for the list)"
        )),
    }
}

/// Parse a comma / whitespace / newline separated list.
pub fn split_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub const SHORTER_IS_STRICTER: &[&str] = &[
    "exempt_paths",
    "allow_patterns",
    "allowed_users",
    "assert_helper_fns",
    "extra_assert_macros",
    "sources",
    "allow_dependencies",
];

fn is_reset_token(val: &Value) -> bool {
    val.as_str() == Some("__reset__")
}

fn extract_table_items(table: &toml::map::Map<String, Value>) -> Option<(bool, Vec<Value>)> {
    if table.contains_key("reset") || table.contains_key("items") {
        let reset = table.get("reset").and_then(Value::as_bool).unwrap_or(false);
        let items = table
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Some((reset, items))
    } else {
        None
    }
}

fn clean_value(v: Value) -> Value {
    match v {
        Value::Table(ref tbl) if extract_table_items(tbl).is_some() => {
            let (_, items) = extract_table_items(tbl).unwrap();
            let filtered: Vec<Value> = items
                .into_iter()
                .filter(|x| !is_reset_token(x))
                .map(clean_value)
                .collect();
            Value::Array(filtered)
        }
        Value::Table(tbl) => {
            let mut cleaned = toml::map::Map::new();
            for (k, val) in tbl {
                cleaned.insert(k, clean_value(val));
            }
            Value::Table(cleaned)
        }
        Value::Array(arr) => {
            let filtered: Vec<Value> = arr
                .into_iter()
                .filter(|x| !is_reset_token(x))
                .map(clean_value)
                .collect();
            Value::Array(filtered)
        }
        other => other,
    }
}

/// Deep merge: tables merge key-wise, scalars are replaced.
///
/// Array merging is asymmetric:
/// - Lists where shorter is stricter (`exempt_paths`, `allow_patterns`, `allowed_users`,
///   `assert_helper_fns`, `extra_assert_macros`, `sources`) support explicit reset via `["__reset__", ...]`
///   or `{ reset = true, items = [...] }` by clearing the base vector before inserting new items.
/// - Lists where longer is stricter (`hostname_denylist`, `extra_patterns`, `paths`,
///   `include`) ignore reset and remain strictly append-only.
pub fn merge(base: &mut Value, over: Value) {
    merge_inner(base, over, None);
}

fn merge_inner(base: &mut Value, over: Value, key: Option<&str>) {
    let is_shorter_stricter = key
        .map(|k| SHORTER_IS_STRICTER.contains(&k))
        .unwrap_or(false);

    match (base, over) {
        (Value::Table(b), Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge_inner(slot, v, Some(&k)),
                    None => {
                        b.insert(k, clean_value(v));
                    }
                }
            }
        }
        (Value::Array(b), Value::Array(o)) => {
            let has_reset = is_shorter_stricter && o.iter().any(is_reset_token);
            if has_reset {
                b.clear();
            }
            for v in o {
                if !is_reset_token(&v) && !b.contains(&v) {
                    b.push(v);
                }
            }
        }
        (Value::Array(b), Value::Table(ref o)) if extract_table_items(o).is_some() => {
            let (reset, items) = extract_table_items(o).unwrap();
            if reset && is_shorter_stricter {
                b.clear();
            }
            for v in items {
                if !is_reset_token(&v) && !b.contains(&v) {
                    b.push(v);
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

fn set_path(root: &mut Value, path: &[&str], leaf: Value) {
    let mut cur = root;
    for (i, key) in path.iter().enumerate() {
        if !cur.is_table() {
            *cur = Value::Table(Default::default());
        }
        let table = cur.as_table_mut().expect("just ensured table");
        if i == path.len() - 1 {
            table.insert((*key).to_string(), leaf);
            return;
        }
        cur = table
            .entry((*key).to_string())
            .or_insert_with(|| Value::Table(Default::default()));
    }
}
