//! The base side's policy floor: what a change cannot loosen in its own diff, whichever
//! side's configuration judges it (`--policy-from head` or `base`).
//!
//! Three things hold from the base branch's configuration file:
//! - the override budgets and the approval rule (`[directives]`): a change that raises
//!   or drops them, or deletes the file that sets them, is still held to the base's;
//! - the gates that guard the pipeline ([`PROTECTIVE_GATES`]): switching one off,
//!   demoting it, or exempting a protected path from it is a refusal no directive lifts;
//! - the file itself: deleting it is a refusal.
//!
//! A weakening reported by `config-integrity` stays a finding a directive can lift; the
//! refusals here are the part of it that a single author-written line must not excuse,
//! because each one disarms the check of the next change.

use crate::config::{DirectivesConfig, DisciplineConfig, Severity};
use crate::gitctx::GitCtx;
use crate::refusals::{PolicyFailure, RefusalKind};
use anyhow::Result;
use globset::Glob;

/// The gates that guard the pipeline itself: the configuration, the CI wiring, the
/// owner's ratification of protected paths, the agents' sandboxes and the instructions
/// they read.
pub const PROTECTIVE_GATES: &[&str] = &[
    "config-integrity",
    "ci-integrity",
    "ratified-paths",
    "sandbox-config",
    "instruction-smuggling",
];

/// Paths an `exempt_paths` entry of a protective gate must not reach: the files those
/// gates exist to watch. The base's `ratified-paths` `protected_paths` are added to them.
const PROTECTED_PROBES: &[&str] = &[
    "discipline.toml",
    "action.yml",
    ".github/workflows/ci.yml",
    ".gitea/workflows/ci.yml",
    ".forgejo/workflows/ci.yml",
    ".gitlab-ci.yml",
    "AGENTS.md",
    "CLAUDE.md",
    ".claude/settings.json",
];

/// What the base side's configuration fixes for this run.
#[derive(Debug, Default)]
pub struct Floor {
    /// The base side's `[directives]`, when the base holds a configuration that loads.
    pub base_directives: Option<DirectivesConfig>,
    /// The refusals the change earns against the base side.
    pub failures: Vec<PolicyFailure>,
}

/// The configuration source text of one side, as `config-integrity` reads it: the file
/// the run was given when it is in the tree, else `discipline.toml`.
fn side_text(
    config_path: &str,
    read: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Option<String>> {
    let own = if crate::gitctx::config_in_tree(config_path) {
        read(config_path)?
    } else {
        None
    };
    Ok(match own {
        Some(s) => Some(s),
        None if config_path != "discipline.toml" => read("discipline.toml")?,
        None => None,
    })
}

/// The base side's floor for the change `git` describes.
///
/// A configuration outside the repository is the operator's, and a base without one has
/// no floor. A base file that does not load sets none either: `config-integrity` reports
/// it, and blocking here would deadlock the change that repairs it.
pub fn evaluate(git: &GitCtx, config_path: &str) -> Result<Floor> {
    if !crate::gitctx::config_in_tree(config_path) {
        return Ok(Floor::default());
    }
    let Some(base_src) = side_text(config_path, |p| git.base_content(p))? else {
        return Ok(Floor::default());
    };
    let Ok(base) = DisciplineConfig::from_toml_str(&base_src) else {
        return Ok(Floor::default());
    };
    let head_src = side_text(config_path, |p| git.head_content(p))?;
    let mut failures = Vec::new();
    let head = match &head_src {
        Some(src) => DisciplineConfig::from_toml_str(src)?,
        None => {
            failures.push(PolicyFailure::new(
                RefusalKind::PolicyFileDeleted,
                format!("`{config_path}` is deleted by this change; the base branch's override budgets and approval rule still apply"),
            ));
            DisciplineConfig::default()
        }
    };
    let disarmed = disarms(&base, &head);
    if !disarmed.is_empty() {
        failures.push(PolicyFailure::new(
            RefusalKind::ProtectiveGateDisarmed,
            format!(
                "this change disarms a gate that guards the pipeline: {}; no directive lifts it",
                disarmed.join("; ")
            ),
        ));
    }
    Ok(Floor {
        base_directives: Some(base.directives),
        failures,
    })
}

fn rank(s: Severity) -> u8 {
    match s {
        Severity::Error => 2,
        Severity::Warning => 1,
        Severity::Note => 0,
    }
}

/// Whether an `exempt_paths` pattern reaches a path a protective gate watches. A pattern
/// that does not compile cannot be shown not to, so it counts as reaching one.
fn reaches_protected(pattern: &str, probes: &[String]) -> bool {
    match Glob::new(pattern) {
        Ok(glob) => {
            let m = glob.compile_matcher();
            probes.iter().any(|p| m.is_match(p))
        }
        Err(_) => true,
    }
}

/// Each way `head` disarms a protective gate the base runs, in words: switched off,
/// demoted, or given an `exempt_paths` entry that reaches a protected path.
pub fn disarms(base: &DisciplineConfig, head: &DisciplineConfig) -> Vec<String> {
    let mut probes: Vec<String> = PROTECTED_PROBES.iter().map(|p| p.to_string()).collect();
    // A protected-path pattern stands for the paths under it.
    probes.extend(
        base.gates
            .ratified_paths
            .protected_paths
            .iter()
            .map(|p| p.replace("**", "x").replace('*', "x")),
    );
    let mut out = Vec::new();
    for gate in PROTECTIVE_GATES {
        let (Some(b), Some(h)) = (base.gates.settings(gate), head.gates.settings(gate)) else {
            continue;
        };
        if !b.enabled() {
            continue;
        }
        if !h.enabled() {
            out.push(format!("`{gate}` is switched off"));
            continue;
        }
        if rank(h.severity()) < rank(b.severity()) {
            out.push(format!(
                "`{gate}` is demoted from `{}` to `{}`",
                b.severity(),
                h.severity()
            ));
        }
        let reaching: Vec<&str> = h
            .exempt_paths()
            .iter()
            .filter(|p| !b.exempt_paths().contains(p))
            .filter(|p| reaches_protected(p, &probes))
            .map(String::as_str)
            .collect();
        if !reaching.is_empty() {
            out.push(format!(
                "`{gate}` exempts a protected path (`exempt_paths` gains `{}`)",
                reaching.join("`, `")
            ));
        }
    }
    out
}

/// The `[directives]` the run's budgets and approval rule are judged by: the side in
/// force, held to the base side's floor. A budget is the lower of the two; an approval
/// rule the base sets stays, with the base's reviewers, so a change cannot name its own.
pub fn stricter_directives(
    in_force: &DirectivesConfig,
    base: Option<&DirectivesConfig>,
) -> DirectivesConfig {
    let mut out = in_force.clone();
    let Some(base) = base else {
        return out;
    };
    let lower = |a: Option<usize>, b: Option<usize>| match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    out.max_overrides = lower(in_force.max_overrides, base.max_overrides);
    out.max_inline_overrides = lower(in_force.max_inline_overrides, base.max_inline_overrides);
    if base.require_approval {
        out.require_approval = true;
        out.allowed_override_actors = base.allowed_override_actors.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml: &str) -> DisciplineConfig {
        DisciplineConfig::from_toml_str(toml).unwrap()
    }

    #[test]
    fn switching_off_demoting_or_exempting_a_protective_gate_is_a_disarm() {
        let base = cfg("[gates.ci-integrity]\nenabled = true\n");
        let off = disarms(&base, &cfg("[gates.ci-integrity]\nenabled = false\n"));
        assert_eq!(off, vec!["`ci-integrity` is switched off"]);
        let demoted = disarms(
            &base,
            &cfg("[gates.ci-integrity]\nseverity = \"warning\"\n"),
        );
        assert_eq!(
            demoted,
            vec!["`ci-integrity` is demoted from `error` to `warning`"]
        );
        let exempt = disarms(
            &base,
            &cfg("[gates.ci-integrity]\nexempt_paths = [\"**\"]\n"),
        );
        assert_eq!(
            exempt,
            vec!["`ci-integrity` exempts a protected path (`exempt_paths` gains `**`)"]
        );
        let workflows = disarms(
            &base,
            &cfg("[gates.ci-integrity]\nexempt_paths = [\".github/**\"]\n"),
        );
        assert_eq!(workflows.len(), 1);
    }

    #[test]
    fn a_narrow_exemption_a_weakened_ordinary_gate_or_a_tightening_is_not_a_disarm() {
        let base = cfg("[gates.instruction-smuggling]\nseverity = \"warning\"\n");
        let narrow = cfg(
            "[gates.instruction-smuggling]\nseverity = \"warning\"\nexempt_paths = [\"tests/fixtures/**\"]\n",
        );
        assert!(disarms(&base, &narrow).is_empty());
        let ordinary = cfg("[gates.assertion-reduction]\nenabled = false\n");
        assert!(disarms(&cfg(""), &ordinary).is_empty());
        let tighter = cfg("[gates.instruction-smuggling]\nseverity = \"error\"\n");
        assert!(disarms(&base, &tighter).is_empty());
        // A gate the base does not run has nothing to disarm.
        let base_off = cfg("[gates.sandbox-config]\nenabled = false\n");
        assert!(disarms(&base_off, &cfg("[gates.sandbox-config]\nenabled = false\n")).is_empty());
    }

    #[test]
    fn an_exemption_reaching_a_ratified_path_is_a_disarm() {
        let base = cfg("[gates.ratified-paths]\nprotected_paths = [\"deploy/**\"]\n");
        let head = cfg(
            "[gates.ratified-paths]\nprotected_paths = [\"deploy/**\"]\n\n[gates.ci-integrity]\nexempt_paths = [\"deploy/**\"]\n",
        );
        assert_eq!(disarms(&base, &head).len(), 1);
    }

    #[test]
    fn the_base_budget_and_approval_rule_hold_whatever_the_side_in_force_says() {
        let base = DirectivesConfig {
            max_overrides: Some(3),
            max_inline_overrides: Some(1),
            require_approval: true,
            allowed_override_actors: vec!["owner".into()],
            ..DirectivesConfig::default()
        };
        let head = DirectivesConfig {
            max_overrides: None,
            max_inline_overrides: Some(5),
            require_approval: false,
            allowed_override_actors: vec!["agent".into()],
            ..DirectivesConfig::default()
        };
        let d = stricter_directives(&head, Some(&base));
        assert_eq!(d.max_overrides, Some(3));
        assert_eq!(d.max_inline_overrides, Some(1));
        assert!(d.require_approval);
        assert_eq!(d.allowed_override_actors, vec!["owner".to_string()]);
        // A side in force that is already stricter keeps its own budget.
        let strict = DirectivesConfig {
            max_overrides: Some(1),
            ..DirectivesConfig::default()
        };
        assert_eq!(
            stricter_directives(&strict, Some(&base)).max_overrides,
            Some(1)
        );
        // No base configuration: the side in force decides alone.
        assert_eq!(stricter_directives(&head, None).max_overrides, None);
    }
}
