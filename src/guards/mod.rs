pub mod agent_diff;
pub mod archive_contents;
pub mod ci_gitlab;
pub mod ci_integrity;
pub mod ci_skip_set;
pub mod claim_registry;
pub mod command;
pub mod dependency;
pub mod hygiene;
pub mod integrity;
pub mod issue_link;
pub mod lockfile;
pub mod manifest_sync;
pub mod miri;
pub mod msrv;
pub mod perf;
pub mod pr_checklist;
pub mod presets;
pub mod provenance_tags;
pub mod sanitizers;
pub mod scope_confinement;
pub mod shell_secrets;
pub mod suppression_delta;
pub mod test_budget;
pub mod test_floor;
pub mod unsafe_budget;
pub mod version_lockstep;

use crate::cli::SuiteChoice;
pub use crate::config::Severity;
use crate::config::{gate_info, DisciplineConfig, GateSettings, Suite, GATES};
use crate::gitctx::GitCtx;
use anyhow::{anyhow, bail, Context as _, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub gate: &'static str,
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

    pub fn push(
        &mut self,
        severity: Severity,
        title: &str,
        file: Option<&str>,
        line: Option<usize>,
        message: String,
        remediation: &str,
    ) {
        self.violations.push(Violation {
            gate: self.gate,
            severity,
            title: title.to_string(),
            file: file.map(str::to_string),
            line,
            message,
            remediation: Some(remediation.to_string()),
        });
    }

    pub fn add_violation(
        &mut self,
        severity: Severity,
        file: impl AsRef<str>,
        line: usize,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) {
        let msg = message.into();
        self.violations.push(Violation {
            gate: self.gate,
            severity,
            title: msg.clone(),
            file: Some(file.as_ref().to_string()),
            line: Some(line),
            message: msg,
            remediation: Some(remediation.into()),
        });
    }
}

#[derive(Debug, Serialize)]
pub struct CheckSummary {
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub policy_failures: Vec<String>,
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
}

impl Context<'_> {
    pub fn find_override(
        &self,
        gate: &str,
        names: &[&str],
        subject: &str,
    ) -> Option<crate::tokens::OverrideRecord> {
        crate::tokens::find_override(&self.directives, gate, names, subject)
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

pub fn run_checks(
    config: &DisciplineConfig,
    suite: SuiteChoice,
    ctx: &Context,
) -> Result<CheckSummary> {
    let wanted: Vec<Suite> = match suite {
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
    };

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
        let outcome = match gate.id {
            "agents-md" => hygiene::agents_md(ctx),
            "time-estimates" => hygiene::time_estimates(ctx),
            "pii" => hygiene::pii(ctx),
            "agent-scratch" => hygiene::agent_scratch(ctx),
            "shell-secrets" => shell_secrets::evaluate_shell_secrets(ctx),
            "issue-link" => issue_link::evaluate_issue_link(ctx),
            "config-integrity" => integrity::config_integrity(ctx),
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
                    ast_outcomes = Some(agent_diff::run(ctx)?);
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
        .with_context(|| format!("gate `{}` could not run", gate.id))?;
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

    for note in &ctx.directive_notes {
        let target_gate = if note.contains("removes") || note.contains("deletes") {
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

    // Deduplicate violations per gate by (file, line, title, message) preserving discovery order
    for o in &mut outcomes {
        let mut seen = std::collections::HashSet::new();
        o.violations.retain(|v| {
            let key = (v.file.clone(), v.line, v.title.clone(), v.message.clone());
            seen.insert(key)
        });
    }

    // Grandfathered findings baseline matching
    if let Some(baseline) = ctx.baseline {
        crate::baseline::apply_baseline_with_git(ctx.git, baseline, &mut outcomes);
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
        outcomes,
        policy_failures: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
