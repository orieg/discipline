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

use crate::baseline::DEFAULT_BASELINE_FILE;
use crate::config::{DirectivesConfig, DisciplineConfig, Severity};
use crate::gitctx::GitCtx;
use crate::guards::instruction_smuggling::{is_instruction_file, INSTRUCTION_FILES};
use crate::guards::sandbox_config::{self, EXECUTABLE, PROJECT_SETTINGS, RULES};
use crate::guards::PathFilter;
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

/// A concrete path standing for a watched-path pattern: each wildcard filled with `x`, and a
/// directory entry (`dir/`) with a file under it.
fn instance(pattern: &str) -> String {
    let p = pattern.replace("**", "x").replace(['*', '?'], "x");
    if p.ends_with('/') {
        format!("{p}x")
    } else {
        p
    }
}

/// The paths an `exempt_paths` entry of a protective gate must not reach, derived from the
/// tables those gates read:
/// - `instruction-smuggling`: [`INSTRUCTION_FILES`] and the base's `instruction_files`;
/// - `sandbox-config`: the files of its rule table and the scripts it reads ([`RULES`],
///   [`EXECUTABLE`]), and the agent settings files `doctor` reads;
/// - `ci-integrity`: the base's `workflows` patterns;
/// - `config-integrity`: the configuration file and the default baseline file;
/// - `ratified-paths`: the base's `protected_paths`.
///
/// The first two tables name a file by its name or its last directories, wherever it sits;
/// each such entry is probed at the root and under every directory the tables name.
pub fn probes(base: &DisciplineConfig, config_path: &str) -> Vec<String> {
    let anywhere: Vec<String> = INSTRUCTION_FILES
        .iter()
        .copied()
        .chain(RULES.iter().flat_map(|r| r.files.iter().copied()))
        .chain(EXECUTABLE.iter().copied())
        .map(instance)
        .collect();
    let anchored: Vec<String> = base
        .gates
        .instruction_smuggling
        .instruction_files
        .iter()
        .chain(&base.gates.ci_integrity.workflows)
        .chain(&base.gates.ratified_paths.protected_paths)
        .map(|p| instance(p))
        .chain(PROJECT_SETTINGS.iter().map(|(_, p)| p.to_string()))
        .chain(
            [config_path, "discipline.toml", DEFAULT_BASELINE_FILE]
                .iter()
                .map(|p| p.to_string()),
        )
        .collect();
    let mut dirs: Vec<&str> = anywhere
        .iter()
        .chain(&anchored)
        .filter_map(|p| p.split_once('/').map(|(d, _)| d))
        .collect();
    dirs.sort_unstable();
    dirs.dedup();
    let mut out: Vec<String> = anywhere
        .iter()
        .flat_map(|p| {
            std::iter::once(p.clone()).chain(dirs.iter().map(move |d| format!("{d}/{p}")))
        })
        .chain(anchored.iter().cloned())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether a protective gate reads `path`, by the gates' own tests: what an `exempt_paths`
/// entry naming a single watched file reaches.
fn watched(base: &DisciplineConfig, config_path: &str, path: &str) -> bool {
    let globs = |g: &[String]| PathFilter::new(g).is_ok_and(|f| f.matches(path));
    is_instruction_file(path)
        || globs(&base.gates.instruction_smuggling.instruction_files)
        || sandbox_config::classify(path).is_some()
        || globs(&base.gates.ci_integrity.workflows)
        || globs(&base.gates.ratified_paths.protected_paths)
        || [config_path, "discipline.toml", DEFAULT_BASELINE_FILE].contains(&path)
}

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
    let disarmed = disarms(&base, &head, config_path);
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

/// Whether an `exempt_paths` pattern reaches a path a protective gate watches: it matches
/// one of the probes, or the path it names (wildcards filled) is one the gates read. A
/// pattern that does not compile cannot be shown not to, so it counts as reaching one.
fn reaches_protected(pattern: &str, probes: &[String], watched: impl Fn(&str) -> bool) -> bool {
    match Glob::new(pattern) {
        Ok(glob) => {
            let m = glob.compile_matcher();
            probes.iter().any(|p| m.is_match(p)) || watched(&instance(pattern))
        }
        Err(_) => true,
    }
}

/// Each way `head` disarms a protective gate the base runs, in words: switched off,
/// demoted, or given an `exempt_paths` entry that reaches a protected path. `config_path`
/// is the configuration file the run was given.
pub fn disarms(base: &DisciplineConfig, head: &DisciplineConfig, config_path: &str) -> Vec<String> {
    let probes = probes(base, config_path);
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
            .filter(|p| reaches_protected(p, &probes, |q| watched(base, config_path, q)))
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

    fn disarms_(base: &DisciplineConfig, head: &DisciplineConfig) -> Vec<String> {
        disarms(base, head, "discipline.toml")
    }

    #[test]
    fn every_path_a_protective_gate_watches_is_probed() {
        let base = cfg(
            "[gates.ratified-paths]\nprotected_paths = [\"deploy/**\"]\n\n[gates.instruction-smuggling]\ninstruction_files = [\"prompts/**\"]\n",
        );
        let probes = probes(&base, "ci/discipline.toml");
        // Each probe is a path one of the gates reads, by that gate's own test.
        for p in &probes {
            assert!(
                watched(&base, "ci/discipline.toml", p),
                "`{p}` is probed but no protective gate reads it"
            );
        }
        // Each table the gates read is probed, entry by entry.
        let tables: Vec<(&str, Vec<String>)> = vec![
            (
                "instruction-smuggling INSTRUCTION_FILES",
                INSTRUCTION_FILES.iter().map(|f| instance(f)).collect(),
            ),
            (
                "instruction-smuggling instruction_files",
                vec!["prompts/x".into()],
            ),
            (
                "sandbox-config RULES",
                RULES
                    .iter()
                    .flat_map(|r| r.files.iter().map(|f| instance(f)))
                    .collect(),
            ),
            (
                "sandbox-config EXECUTABLE",
                EXECUTABLE.iter().map(|f| instance(f)).collect(),
            ),
            (
                "sandbox-config PROJECT_SETTINGS",
                PROJECT_SETTINGS
                    .iter()
                    .map(|(_, f)| f.to_string())
                    .collect(),
            ),
            (
                "ci-integrity workflows",
                base.gates
                    .ci_integrity
                    .workflows
                    .iter()
                    .map(|f| instance(f))
                    .collect(),
            ),
            (
                "config-integrity",
                vec![
                    "ci/discipline.toml".into(),
                    "discipline.toml".into(),
                    DEFAULT_BASELINE_FILE.into(),
                ],
            ),
            ("ratified-paths protected_paths", vec!["deploy/x".into()]),
        ];
        for (table, paths) in &tables {
            assert!(!paths.is_empty(), "{table} is empty");
            for p in paths {
                assert!(
                    probes.contains(p),
                    "{table} watches `{p}`, which is not probed"
                );
            }
        }
        // A file the gates read wherever it sits is probed where the tables put it.
        for p in [
            ".github/copilot-instructions.md",
            ".github/copilot/settings.json",
            ".github/workflows/x.yml",
        ] {
            assert!(probes.contains(&p.to_string()), "`{p}` is not probed");
        }
    }

    #[test]
    fn an_exemption_reaching_a_file_the_tables_name_is_a_disarm() {
        let smuggling = cfg("[gates.instruction-smuggling]\nseverity = \"error\"\n");
        for path in [
            "GEMINI.md",
            ".github/copilot-instructions.md",
            ".opencode/**",
            ".github/instructions/*.md",
            "QWEN.md",
        ] {
            let head = cfg(&format!(
                "[gates.instruction-smuggling]\nseverity = \"error\"\nexempt_paths = [\"{path}\"]\n"
            ));
            assert_eq!(disarms_(&smuggling, &head).len(), 1, "`{path}`");
        }
        let sandbox = cfg("[gates.sandbox-config]\nenabled = true\n");
        for path in [
            ".github/copilot/settings.json",
            ".mcp.json",
            "docker-compose.yml",
            ".vscode/**",
        ] {
            let head = cfg(&format!(
                "[gates.sandbox-config]\nenabled = true\nexempt_paths = [\"{path}\"]\n"
            ));
            assert_eq!(disarms_(&sandbox, &head).len(), 1, "`{path}`");
        }
        let ci = cfg("[gates.ci-integrity]\nenabled = true\n");
        for path in [
            ".github/workflows/ci.yml",
            ".gitlab/ci/*.yml",
            "discipline-baseline.toml",
        ] {
            let head = cfg(&format!(
                "[gates.ci-integrity]\nenabled = true\nexempt_paths = [\"{path}\"]\n"
            ));
            assert_eq!(disarms_(&ci, &head).len(), 1, "`{path}`");
        }
        // The configuration file the run was given counts, wherever it is.
        let head = cfg("[gates.ci-integrity]\nenabled = true\nexempt_paths = [\"ci/**\"]\n");
        assert!(disarms_(&ci, &head).is_empty());
        assert_eq!(disarms(&ci, &head, "ci/discipline.toml").len(), 1);
    }

    #[test]
    fn switching_off_demoting_or_exempting_a_protective_gate_is_a_disarm() {
        let base = cfg("[gates.ci-integrity]\nenabled = true\n");
        let off = disarms_(&base, &cfg("[gates.ci-integrity]\nenabled = false\n"));
        assert_eq!(off, vec!["`ci-integrity` is switched off"]);
        let demoted = disarms_(
            &base,
            &cfg("[gates.ci-integrity]\nseverity = \"warning\"\n"),
        );
        assert_eq!(
            demoted,
            vec!["`ci-integrity` is demoted from `error` to `warning`"]
        );
        let exempt = disarms_(
            &base,
            &cfg("[gates.ci-integrity]\nexempt_paths = [\"**\"]\n"),
        );
        assert_eq!(
            exempt,
            vec!["`ci-integrity` exempts a protected path (`exempt_paths` gains `**`)"]
        );
        let workflows = disarms_(
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
        assert!(disarms_(&base, &narrow).is_empty());
        let ordinary = cfg("[gates.assertion-reduction]\nenabled = false\n");
        assert!(disarms_(&cfg(""), &ordinary).is_empty());
        let tighter = cfg("[gates.instruction-smuggling]\nseverity = \"error\"\n");
        assert!(disarms_(&base, &tighter).is_empty());
        // A gate the base does not run has nothing to disarm.
        let base_off = cfg("[gates.sandbox-config]\nenabled = false\n");
        assert!(disarms_(&base_off, &cfg("[gates.sandbox-config]\nenabled = false\n")).is_empty());
    }

    #[test]
    fn an_exemption_reaching_a_ratified_path_is_a_disarm() {
        let base = cfg("[gates.ratified-paths]\nprotected_paths = [\"deploy/**\"]\n");
        let head = cfg(
            "[gates.ratified-paths]\nprotected_paths = [\"deploy/**\"]\n\n[gates.ci-integrity]\nexempt_paths = [\"deploy/**\"]\n",
        );
        assert_eq!(disarms_(&base, &head).len(), 1);
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
