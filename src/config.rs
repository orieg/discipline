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
}

/// Single source of truth for gate identifiers.
pub const GATES: &[GateInfo] = &[
    GateInfo {
        id: "agents-md",
        suite: Suite::AgentGuard,
        summary: "AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "assertion-reduction",
        suite: Suite::AgentGuard,
        summary: "assertion count / strength must not drop in an existing test",
        languages: "Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby",
        available: true,
    },
    GateInfo {
        id: "vacuous-tests",
        suite: Suite::AgentGuard,
        summary: "new tests must carry a non-tautological assertion",
        languages: "Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby",
        available: true,
    },
    GateInfo {
        id: "ignored-tests",
        suite: Suite::AgentGuard,
        summary: "tests must not be newly #[ignore]d or skipped without directive",
        languages: "Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby",
        available: true,
    },
    GateInfo {
        id: "unsafe-safety-comment",
        suite: Suite::AgentGuard,
        summary: "unsafe blocks / impls carry a // SAFETY: comment",
        languages: "Rust",
        available: true,
    },
    GateInfo {
        id: "deletion-rationale",
        suite: Suite::AgentGuard,
        summary: "deleted files and removed tests need a scoped removes: rationale",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "time-estimates",
        suite: Suite::Hygiene,
        summary: "no calendar / duration estimates in markdown or the PR body",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "pii",
        suite: Suite::Hygiene,
        summary: "no home paths, LAN IPs, or denylisted hostnames in tracked text",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "agent-scratch",
        suite: Suite::Hygiene,
        summary: "agent scratch state is never tracked",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "shell-secrets",
        suite: Suite::Hygiene,
        summary: "no command-line secrets or unverified piped scripts in shell, docker, or CI",
        languages: "shell, docker, workflows",
        available: true,
    },
    GateInfo {
        id: "issue-link",
        suite: Suite::Hygiene,
        summary: "PR title or description links a tracking issue (#123, Fixes #123)",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "config-integrity",
        suite: Suite::Integrity,
        summary: "a change cannot weaken its own discipline.toml without a token",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "scope-confinement",
        suite: Suite::AgentGuard,
        summary: "changes stay inside authorized paths",
        languages: "any",
        available: false,
    },
    GateInfo {
        id: "suppression-delta",
        suite: Suite::AgentGuard,
        summary: "new #[allow], commented-out tests, cfg-gated tests",
        languages: "per pack",
        available: false,
    },
    GateInfo {
        id: "provenance-tags",
        suite: Suite::Hygiene,
        summary: "published numerics carry (measured|target|projected)",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "ci-integrity",
        suite: Suite::Integrity,
        summary: "workflow weakening: continue-on-error, || true, unpinned actions",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "test-floor",
        suite: Suite::Integrity,
        summary: "test-count ratchet read from the base ref",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "golden-output",
        suite: Suite::Integrity,
        summary:
            "prevents stealth edits to committed golden/test output files without explicit override",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "dependency-delta",
        suite: Suite::Integrity,
        summary: "manifest diff inspection: zero wildcards, source/license allowlists, and deny.toml verification",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "test-budget",
        suite: Suite::Integrity,
        summary: "property-test and fuzz effort ratchet (cases, shrink iters, fuzztime, seed corpus)",
        languages: "Rust, Python, JS/TS, Go, any",
        available: true,
    },
    GateInfo {
        id: "pr-checklist",
        suite: Suite::Hygiene,
        summary: "ticked PR checkboxes are reconciled against the diff",
        languages: "any",
        available: false,
    },
    GateInfo {
        id: "command",
        suite: Suite::Verification,
        summary: "fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet",
        languages: "any",
        available: true,
    },
    GateInfo {
        id: "sanitizers",
        suite: Suite::Verification,
        summary: "ASan / TSan preset with audited suppressions and a race canary",
        languages: "Rust, C/C++",
        available: false,
    },
    GateInfo {
        id: "msrv",
        suite: Suite::Quality,
        summary: "cargo check under the pinned MSRV",
        languages: "Rust",
        available: false,
    },
    GateInfo {
        id: "miri",
        suite: Suite::Verification,
        summary: "Miri tiers with zero-tests guard",
        languages: "Rust",
        available: false,
    },
    GateInfo {
        id: "unsafe-budget",
        suite: Suite::Verification,
        summary: "unsafe count ratchet",
        languages: "Rust",
        available: false,
    },
    GateInfo {
        id: "bench-regression",
        suite: Suite::Bench,
        summary: "benchmark drift via harness adapters (deterministic counts or BCa intervals)",
        languages: "Rust, Go, Python, C/C++",
        available: true,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DirectivesConfig {
    pub sources: Vec<String>,
    pub allow_hidden: bool,
    pub fail_on_overrides: bool,
}

impl Default for DirectivesConfig {
    fn default() -> Self {
        Self {
            sources: vec!["pr-body".to_string(), "commits".to_string()],
            allow_hidden: false,
            fail_on_overrides: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisciplineConfig {
    pub meta: MetaConfig,
    #[serde(default)]
    pub directives: DirectivesConfig,
    #[serde(default)]
    pub gates: Gates,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaConfig {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Gates {
    pub agents_md: BasicGate,
    pub assertion_reduction: AssertionGate,
    pub vacuous_tests: AssertionGate,
    pub ignored_tests: IgnoredTestsGate,
    pub unsafe_safety_comment: UnsafeSafetyCommentGate,
    pub deletion_rationale: DeletionGate,
    pub time_estimates: TimeEstimateGate,
    pub pii: PiiGate,
    pub agent_scratch: ScratchGate,
    pub config_integrity: BasicGate,
    pub golden_output: GoldenGate,
    pub bench_regression: BenchRegressionGate,
    pub command: CommandGate,
    pub dependency_delta: DependencyDeltaGate,
    pub test_budget: TestBudgetGate,
    pub test_floor: TestFloorGate,
    pub ci_integrity: CiIntegrityGate,
    pub shell_secrets: ShellSecretsGate,
    pub issue_link: IssueLinkGate,
    pub provenance_tags: ProvenanceTagsGate,
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
    ShellSecretsGate,
    IssueLinkGate,
    ProvenanceTagsGate
);

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IgnoredTestsGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Conditional ignore predicates (e.g. `miri`) that are approved by repository policy.
    pub approved_predicates: Vec<String>,
}

impl Default for IgnoredTestsGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            approved_predicates: Vec::new(),
        }
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
    /// Extra macro names (final path segment, no `!`) counted as assertions.
    pub extra_assert_macros: Vec<String>,
    /// Function names (final path segment) whose call counts as an assertion,
    /// for suites that assert through helpers such as `check_invariants(&t)`.
    pub assert_helper_fns: Vec<String>,
    /// Minimum assertions required per test method (default: None).
    pub min_assertions_per_test: Option<usize>,
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
    /// A line matching any of these is not a violation.
    pub allow_patterns: Vec<String>,
    pub scan_pr_body: bool,
    /// When true, scans only modified lines in the git diff rather than all tracked files.
    pub diff_only: bool,
}

impl Default for TimeEstimateGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
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
}

impl Default for PiiGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            home_paths: true,
            lan_ips: true,
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
            exempt_paths: Vec::new(),
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
                "**/*.snap",
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
    /// Declared exempt arms (e.g. random arms of map_get, set_contains).
    pub exempt_arms: Vec<String>,
    /// Require allow-regression directive reasons to carry a verifiable citation and arm names.
    pub require_sourced_override: bool,
}

impl Default for BenchRegressionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
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
    /// Output patterns that must NOT appear in stdout or stderr.
    pub forbid_output: Vec<String>,
    /// Pattern that indicates zero items were executed (e.g. `running 0 tests`).
    pub zero_items_pattern: Option<String>,
    /// Whether zero items selected is allowed (default: false).
    pub allow_zero: bool,
    /// Optional negative-control canary command.
    pub canary_command: Option<String>,
    /// Expected diagnostic string or regex that the canary MUST produce.
    pub canary_expected_diagnostic: Option<String>,
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
    /// Fuzz manifest and harness globs (default: ["fuzz/Cargo.toml", "fuzz/fuzz_targets/**"]).
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
}

impl Default for IssueLinkGate {
    fn default() -> Self {
        Self {
            enabled: false,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            pattern: None,
            require_in_commit_if_no_pr: false,
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
    pub first_party_action_prefixes: Vec<String>,
}

impl Default for CiIntegrityGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            workflows: vec![
                ".github/workflows/*.yml".to_string(),
                ".github/workflows/*.yaml".to_string(),
            ],
            rollup_job: Some("ci-gate".to_string()),
            excluded_jobs: vec!["detect-changes".to_string()],
            pin_actions: true,
            forbid_continue_on_error: true,
            forbid_or_true: true,
            diff_only: true,
            documented_job_count_path: None,
            documented_job_count_pattern: None,
            first_party_action_prefixes: vec!["actions/".to_string(), "github/".to_string()],
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
            "config-integrity" => &self.config_integrity,
            "golden-output" => &self.golden_output,
            "bench-regression" => &self.bench_regression,
            "command" => &self.command,
            "dependency-delta" => &self.dependency_delta,
            "test-budget" => &self.test_budget,
            "test-floor" => &self.test_floor,
            "ci-integrity" => &self.ci_integrity,
            "provenance-tags" => &self.provenance_tags,
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

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.config_override.is_none()
            && self.enable.is_empty()
            && self.disable.is_empty()
            && self.hostname_denylist.is_empty()
            && self.directive_sources.is_none()
            && self.fail_on_overrides.is_none()
    }
}

impl DisciplineConfig {
    pub fn default_for_repo(name: &str) -> Self {
        Self {
            meta: MetaConfig {
                version: SCHEMA_VERSION,
                name: name.to_string(),
                description: None,
            },
            directives: DirectivesConfig::default(),
            gates: Gates::default(),
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

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::resolve(Some(path.as_ref()), &Overrides::default())
    }

    /// Resolve the effective configuration. `path = None` starts from defaults.
    pub fn resolve(path: Option<&Path>, overrides: &Overrides) -> Result<Self> {
        let mut source_info: Option<(std::path::PathBuf, String)> = None;
        let mut value = match path {
            Some(p) => {
                let content = std::fs::read_to_string(p).with_context(|| {
                    format!("failed to read configuration file {}", p.display())
                })?;
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

    fn from_value(value: Value) -> Result<Self> {
        // Name planned gates explicitly: "unknown field" would read as a typo,
        // and a user must learn the gate exists but is not shipped yet.
        if let Some(gates) = value.get("gates").and_then(Value::as_table) {
            for id in gates.keys() {
                check_gate_id(id)?;
            }
        }
        let config: DisciplineConfig = value
            .try_into()
            .context("discipline configuration failed schema validation")?;
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

pub const LONGER_IS_STRICTER: &[&str] = &[
    "hostname_denylist",
    "extra_patterns",
    "paths",
    "include",
    "forbid_output",
    "deny_dependencies",
    "manifests",
    "corpus_dirs",
    "fuzz_targets",
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
