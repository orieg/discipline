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
    /// `false` = planned in the PRD but not shipped in this binary. A planned
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
        available: true,
    },
    GateInfo {
        id: "assertion-reduction",
        suite: Suite::AgentGuard,
        summary: "assertion count / strength must not drop in an existing test",
        available: true,
    },
    GateInfo {
        id: "vacuous-tests",
        suite: Suite::AgentGuard,
        summary: "new tests must carry a non-tautological assertion",
        available: true,
    },
    GateInfo {
        id: "ignored-tests",
        suite: Suite::AgentGuard,
        summary: "tests must not be newly #[ignore]d",
        available: true,
    },
    GateInfo {
        id: "unsafe-safety-comment",
        suite: Suite::AgentGuard,
        summary: "unsafe blocks / impls carry a // SAFETY: comment",
        available: true,
    },
    GateInfo {
        id: "deletion-rationale",
        suite: Suite::AgentGuard,
        summary: "deleted files and removed tests need a scoped removes: rationale",
        available: true,
    },
    GateInfo {
        id: "time-estimates",
        suite: Suite::Hygiene,
        summary: "no calendar / duration estimates in markdown or the PR body",
        available: true,
    },
    GateInfo {
        id: "pii",
        suite: Suite::Hygiene,
        summary: "no home paths, LAN IPs, or denylisted hostnames in tracked text",
        available: true,
    },
    GateInfo {
        id: "agent-scratch",
        suite: Suite::Hygiene,
        summary: "agent scratch state is never tracked",
        available: true,
    },
    GateInfo {
        id: "config-integrity",
        suite: Suite::Integrity,
        summary: "a change cannot weaken its own discipline.toml without a token",
        available: true,
    },
    GateInfo {
        id: "scope-confinement",
        suite: Suite::AgentGuard,
        summary: "changes stay inside authorized paths",
        available: false,
    },
    GateInfo {
        id: "suppression-delta",
        suite: Suite::AgentGuard,
        summary: "new #[allow], commented-out tests, cfg-gated tests",
        available: false,
    },
    GateInfo {
        id: "provenance-tags",
        suite: Suite::Hygiene,
        summary: "published numerics carry (measured|target|projected)",
        available: false,
    },
    GateInfo {
        id: "ci-integrity",
        suite: Suite::Integrity,
        summary: "workflow weakening: continue-on-error, || true, unpinned actions",
        available: false,
    },
    GateInfo {
        id: "test-floor",
        suite: Suite::Integrity,
        summary: "test-count ratchet read from the base ref",
        available: false,
    },
    GateInfo {
        id: "golden-output",
        suite: Suite::Integrity,
        summary:
            "prevents stealth edits to committed golden/test output files without explicit override",
        available: true,
    },
    GateInfo {
        id: "pr-checklist",
        suite: Suite::Hygiene,
        summary: "ticked PR checkboxes are reconciled against the diff",
        available: false,
    },
    GateInfo {
        id: "command",
        suite: Suite::Verification,
        summary: "fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet",
        available: false,
    },
    GateInfo {
        id: "sanitizers",
        suite: Suite::Verification,
        summary: "ASan / TSan preset with audited suppressions and a race canary",
        available: false,
    },
    GateInfo {
        id: "msrv",
        suite: Suite::Quality,
        summary: "cargo check under the pinned MSRV",
        available: false,
    },
    GateInfo {
        id: "miri",
        suite: Suite::Verification,
        summary: "Miri tiers with zero-tests guard",
        available: false,
    },
    GateInfo {
        id: "unsafe-budget",
        suite: Suite::Verification,
        summary: "unsafe count ratchet",
        available: false,
    },
    GateInfo {
        id: "bench-regression",
        suite: Suite::Bench,
        summary: "benchmark drift via harness adapters (deterministic counts or BCa intervals)",
        available: false,
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
    pub ignored_tests: BasicGate,
    pub unsafe_safety_comment: UnsafeSafetyCommentGate,
    pub deletion_rationale: DeletionGate,
    pub time_estimates: TimeEstimateGate,
    pub pii: PiiGate,
    pub agent_scratch: ScratchGate,
    pub config_integrity: BasicGate,
    pub golden_output: GoldenGate,
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
    UnsafeSafetyCommentGate,
    AssertionGate,
    DeletionGate,
    TimeEstimateGate,
    PiiGate,
    ScratchGate,
    GoldenGate,
    BenchRegressionGate
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
}

impl Default for AssertionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            extra_assert_macros: Vec::new(),
            assert_helper_fns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeletionGate {
    pub enabled: bool,
    pub severity: Severity,
    pub exempt_paths: Vec<String>,
    /// Globs of paths whose deletion requires a rationale.
    pub paths: Vec<String>,
}

impl Default for DeletionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            paths: vec!["**".to_string()],
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
}

impl Default for PiiGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            home_paths: true,
            lan_ips: true,
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
}

impl Default for BenchRegressionGate {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: Severity::Error,
            exempt_paths: Vec::new(),
            tolerance_pct: 0.5,
            paths: [
                "target/iai/**",
                "**/callgrind.*",
                "target/criterion/**",
                "**/*benchmark*.json",
                "**/*benchmarks*.json",
                "**/*benchmark*.log",
                "**/*benchmark*.txt",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
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
            "config-integrity" => &self.config_integrity,
            "golden-output" => &self.golden_output,
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
        let value: Value = toml::from_str(content).context("discipline.toml is not valid TOML")?;
        Self::from_value(value)
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::resolve(Some(path.as_ref()), &Overrides::default())
    }

    /// Resolve the effective configuration. `path = None` starts from defaults.
    pub fn resolve(path: Option<&Path>, overrides: &Overrides) -> Result<Self> {
        let mut value = match path {
            Some(p) => {
                let content = std::fs::read_to_string(p).with_context(|| {
                    format!("failed to read configuration file {}", p.display())
                })?;
                toml::from_str::<Value>(&content)
                    .with_context(|| format!("{} is not valid TOML", p.display()))?
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

        Self::from_value(value)
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
];

pub const LONGER_IS_STRICTER: &[&str] =
    &["hostname_denylist", "extra_patterns", "paths", "include"];

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
