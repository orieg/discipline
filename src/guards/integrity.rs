//! Gate-integrity: a change must not quietly lower the bar it is judged by.
//!
//! `discipline.toml` is fully user-configurable, which makes it the cheapest
//! thing for an agent to edit when a gate is in the way. This gate compares
//! the configuration on the base side with the head side and demands a scoped
//! `allow-gate-weakening:` directive for every loosening.

use super::{Context, GateOutcome, PathFilter};
use crate::config::{DisciplineConfig, GateSettings, Severity};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;
use toml::Value;

/// List options where a *longer* list is looser.
const LOOSER_WHEN_GROWN: &[&str] = &[
    "exempt_paths",
    "allow_patterns",
    "allowed_users",
    "extra_assert_macros",
    "assert_helper_fns",
    "allowed_unpinned_actions",
    "allowed_hosts",
    "allow_dependencies",
    "approved_predicates",
    "allowed_rules",
];
/// List options where a *shorter* list is looser.
const LOOSER_WHEN_SHRUNK: &[&str] = &[
    "paths",
    "include",
    "extra_patterns",
    "hostname_denylist",
    "superseded_json_paths",
    "citation_source_paths",
    "citation_measurement_jobs",
];
/// Optional references to evidence files: removing one, or pointing it elsewhere, drops
/// or replaces what the gate checks against.
const EVIDENCE_REFERENCES: &[&str] = &["superseded_registry", "ratio_baseline"];
/// Mode switches whose non-default value is the stricter check.
const STRICTER_MODES: &[(&str, &str)] = &[("mode", "paired-ratio")];

#[derive(Debug, PartialEq, Eq)]
pub struct Weakening {
    pub gate: String,
    pub what: String,
}

pub fn config_integrity(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "config-integrity";
    let settings = &ctx.config.gates.config_integrity;
    let mut out = GateOutcome::new(GATE);

    let base_src = match ctx.git.base_content(ctx.config_path)? {
        Some(s) => Some(s),
        None if ctx.config_path != "discipline.toml" => ctx.git.base_content("discipline.toml")?,
        None => None,
    };
    if let Some(base_src) = base_src {
        match DisciplineConfig::from_toml_str(&base_src) {
            Ok(base) => {
                let head = ctx.config;
                let weakenings = diff_configs(&base, head)?;
                out.examined = Value::try_from(&base.gates)?
                    .as_table()
                    .map(|t| t.len())
                    .unwrap_or(0);
                for w in weakenings {
                    if let Some(record) =
                        ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, &w.gate)
                    {
                        out.overrides.push(record);
                        continue;
                    }
                    out.push(
                        ctx.overridable(settings.severity()),
                        "Gate Weakened By This Change",
                        Some(ctx.config_path),
                        None,
                        format!("[{}] {}.", w.gate, w.what),
                        &format!(
                            "Revert the change, or justify it on its own line in the PR body or a commit \
                             message: `allow-gate-weakening: {} <reason>`.",
                            w.gate
                        ),
                    );
                }
            }
            Err(e) => {
                // Blocking here would deadlock the PR that repairs the base config.
                out.push(
                    Severity::Warning,
                    "Base Configuration Unreadable",
                    Some(ctx.config_path),
                    None,
                    format!("The base-side configuration does not load with this binary ({e:#}); weakening could not be checked."),
                    "Repair the configuration on the base branch.",
                );
            }
        }
    } else {
        out.notes.push(format!(
            "`{}` does not exist on the base side; nothing to compare against",
            ctx.config_path
        ));
    }

    // Baseline file integrity: growing grandfathered baseline or adding new ungrandfathered fingerprints is a weakening
    let baseline_filename = ctx
        .baseline_path
        .unwrap_or(crate::baseline::DEFAULT_BASELINE_FILE);
    let base_baseline: Option<crate::baseline::DisciplineBaseline> =
        match ctx.git.base_content(baseline_filename)? {
            Some(s) => toml::from_str(&s).ok(),
            None => None,
        };

    let head_baseline_path = ctx.git.root().join(baseline_filename);
    let head_baseline: Option<crate::baseline::DisciplineBaseline> = if head_baseline_path.exists()
    {
        std::fs::read_to_string(&head_baseline_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
    } else {
        None
    };

    if let Some(ref h_base) = head_baseline {
        let b_count = base_baseline
            .as_ref()
            .map(|b| b.findings.len())
            .unwrap_or(0);
        let h_count = h_base.findings.len();

        let base_fps: std::collections::HashSet<&str> = base_baseline
            .as_ref()
            .map(|b| b.findings.iter().map(|f| f.fingerprint.as_str()).collect())
            .unwrap_or_default();

        let new_fps: Vec<&str> = h_base
            .findings
            .iter()
            .map(|f| f.fingerprint.as_str())
            .filter(|fp| !base_fps.contains(fp))
            .collect();

        if h_count > b_count || !new_fps.is_empty() {
            if let Some(record) = ctx
                .find_override(GATE, tokens::ALLOW_GATE_WEAKENING, "baseline")
                .or_else(|| {
                    ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, baseline_filename)
                })
            {
                out.overrides.push(record);
            } else if !new_fps.is_empty() && h_count <= b_count {
                let count = new_fps.len();
                let sample = new_fps.first().unwrap_or(&"");
                out.push(
                    ctx.overridable(settings.severity()),
                    "Baseline Contains New Findings Without Directive",
                    Some(baseline_filename),
                    None,
                    format!("[baseline] Grandfathered baseline contains {count} new fingerprint(s) not present on base (e.g. `{sample}`). A 1-for-1 replacement of grandfathered findings with new ones is forbidden."),
                    "Revert the baseline modification, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
            } else {
                let diff = h_count.saturating_sub(b_count);
                out.push(
                    ctx.overridable(settings.severity()),
                    "Baseline Grew Without Directive",
                    Some(baseline_filename),
                    None,
                    format!("[baseline] Grandfathered baseline grew from {b_count} to {h_count} findings ({diff} new grandfathered findings)."),
                    "Revert the baseline growth, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
            }
        }
    }

    Ok(out)
}

pub fn golden_output(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "golden-output";
    let settings = &ctx.config.gates.golden_output;
    let mut out = GateOutcome::new(GATE);

    let path_filter = PathFilter::new(&settings.paths)?;
    let exempt_filter = PathFilter::new(&settings.exempt_paths)?;

    let changed = ctx.git.changed_files()?;
    for file in &changed {
        if file.kind == ChangeKind::Added {
            continue;
        }

        let matches_target = path_filter.matches(&file.path)
            || (!file.old_path.is_empty() && path_filter.matches(&file.old_path));
        if !matches_target {
            continue;
        }

        let is_exempt = exempt_filter.matches(&file.path)
            || (!file.old_path.is_empty() && exempt_filter.matches(&file.old_path));
        if is_exempt {
            continue;
        }

        out.examined += 1;

        if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_GOLDEN_UPDATE, &file.path) {
            out.overrides.push(ov);
            continue;
        }
        if !file.old_path.is_empty() && file.old_path != file.path {
            if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_GOLDEN_UPDATE, &file.old_path) {
                out.overrides.push(ov);
                continue;
            }
        }

        let action = match file.kind {
            ChangeKind::Deleted => "deleted",
            _ => "modified",
        };

        out.push(
            ctx.overridable(settings.severity()),
            "Golden Output Modified Without Directive",
            Some(&file.path),
            None,
            format!(
                "Committed golden/snapshot file `{}` was {action} without an explicit override.",
                file.path
            ),
            "Provide a scoped override on its own line in the PR body or a commit message: \
             `allow-golden-update: <path-or-prefix> <reason>` (or `discipline:allow(golden-output): ...`).",
        );
    }

    Ok(out)
}

pub fn diff_configs(base: &DisciplineConfig, head: &DisciplineConfig) -> Result<Vec<Weakening>> {
    let mut found = Vec::new();

    // Check [directives] table
    let mut dir_note = |what: String| {
        found.push(Weakening {
            gate: "directives".to_string(),
            what,
        })
    };
    if !base.directives.allow_hidden && head.directives.allow_hidden {
        dir_note("`allow_hidden` changed from false to true".to_string());
    }
    let gained_sources: Vec<_> = head
        .directives
        .sources
        .iter()
        .filter(|s| !base.directives.sources.contains(s))
        .collect();
    if gained_sources.iter().any(|s| s.as_str() == "commits") {
        dir_note("`sources` gained commits".to_string());
    }
    if base.directives.fail_on_overrides && !head.directives.fail_on_overrides {
        dir_note("`fail_on_overrides` changed from true to false".to_string());
    }

    let base_v = Value::try_from(&base.gates)?;
    let head_v = Value::try_from(&head.gates)?;
    let (Some(base_t), Some(head_t)) = (base_v.as_table(), head_v.as_table()) else {
        return Ok(found);
    };

    for (gate, base_gate) in base_t {
        let (Some(b), Some(h)) = (
            base_gate.as_table(),
            head_t.get(gate).and_then(Value::as_table),
        ) else {
            continue;
        };
        let mut note = |what: String| {
            found.push(Weakening {
                gate: gate.clone(),
                what,
            })
        };
        for (key, bv) in b {
            let Some(hv) = h.get(key) else {
                if EVIDENCE_REFERENCES.contains(&key.as_str()) {
                    note(format!("`{key}` removed (was {bv})"));
                }
                continue;
            };
            if EVIDENCE_REFERENCES.contains(&key.as_str()) && bv != hv {
                note(format!("`{key}` changed from {bv} to {hv}"));
                continue;
            }
            if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                if STRICTER_MODES
                    .iter()
                    .any(|(k, strict)| k == key && bs == strict && hs != strict)
                {
                    note(format!("`{key}` changed from {bs} to {hs}"));
                    continue;
                }
            }
            match (bv, hv) {
                (Value::Boolean(true), Value::Boolean(false)) => {
                    note(format!("`{key}` changed from true to false"))
                }
                (Value::Boolean(false), Value::Boolean(true))
                    if key == "allow_hidden"
                        || key == "allow_zero"
                        || key == "diff_only"
                        || key == "allow_stale"
                        || key == "allow_missing_base" =>
                {
                    note(format!("`{key}` changed from false to true"))
                }
                (Value::Integer(bi), Value::Integer(hi)) => {
                    // Floors: lowering is a weakening
                    if (key == "min_count" || key == "floor" || key == "test_floor") && hi < bi {
                        note(format!("`{key}` decreased from {bi} to {hi}"));
                    }
                    // Budgets: increasing is a weakening
                    if (key == "max_unsafe"
                        || key == "max_suppressions"
                        || key == "max_count"
                        || key == "budget")
                        && hi > bi
                    {
                        note(format!("`{key}` increased from {bi} to {hi}"));
                    }
                }
                (Value::String(bs), Value::String(hs))
                    if key == "severity"
                        && ((bs == "error" && (hs == "warning" || hs == "note"))
                            || (bs == "warning" && hs == "note")) =>
                {
                    note(format!("`severity` lowered from {bs} to {hs}"))
                }
                (Value::Array(ba), Value::Array(ha)) => {
                    let gained: Vec<_> = ha.iter().filter(|x| !ba.contains(x)).collect();
                    let lost: Vec<_> = ba.iter().filter(|x| !ha.contains(x)).collect();
                    if LOOSER_WHEN_GROWN.contains(&key.as_str()) && !gained.is_empty() {
                        note(format!("`{key}` gained {} entr(y/ies)", gained.len()));
                    }
                    if LOOSER_WHEN_SHRUNK.contains(&key.as_str()) && !lost.is_empty() {
                        note(format!("`{key}` lost {} entr(y/ies)", lost.len()));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(body: &str) -> DisciplineConfig {
        DisciplineConfig::from_toml_str(&format!("[meta]\nversion = 1\nname = \"t\"\n{body}"))
            .unwrap()
    }

    #[test]
    fn identical_and_stricter_configs_are_not_weakenings() {
        let base = cfg("[gates.pii]\nexempt_paths = [\"a/**\"]\n");
        assert!(diff_configs(&base, &base).unwrap().is_empty());
        let stricter = cfg("[gates.pii]\nhostname_denylist = [\"h\"]\n");
        assert!(diff_configs(&base, &stricter).unwrap().is_empty());
    }

    #[test]
    fn each_loosening_class_is_detected_and_attributed() {
        let base = cfg("[gates.pii]\nhostname_denylist = [\"h\"]\n");
        let head = cfg(
            "[gates.pii]\nlan_ips = false\nexempt_paths = [\"docs/**\"]\n\
             [gates.vacuous-tests]\nenabled = false\n\
             [gates.agent-scratch]\nseverity = \"warning\"\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what.contains(needle))
        };
        assert!(has("pii", "`lan_ips` changed from true to false"));
        assert!(has("pii", "`exempt_paths` gained 1"));
        assert!(has("pii", "`hostname_denylist` lost 1"));
        assert!(has("vacuous-tests", "`enabled` changed from true to false"));
        assert!(has("agent-scratch", "`severity` lowered"));
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn integer_and_security_boolean_loosening_detected() {
        let base = cfg("[gates.command]\nmin_count = 10\n[gates.unsafe-budget]\nmax_unsafe = 5\n[gates.pii]\ndiff_only = false\n");
        let head = cfg("[gates.command]\nmin_count = 5\n[gates.unsafe-budget]\nmax_unsafe = 10\n[gates.pii]\ndiff_only = true\n");
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what.contains(needle))
        };
        assert!(has("command", "`min_count` decreased from 10 to 5"));
        assert!(has("unsafe-budget", "`max_unsafe` increased from 5 to 10"));
        assert!(has("pii", "`diff_only` changed from false to true"));
    }

    #[test]
    fn evidence_references_and_strict_modes_cannot_be_dropped_silently() {
        let base = cfg(
            "[gates.provenance-tags]\nsuperseded_registry = \"reg.json\"\n\
             superseded_json_paths = [\"docs/**/*.json\"]\nrequire_open_pending_issues = true\n\
             [gates.bench-regression]\nmode = \"paired-ratio\"\nratio_baseline = \"b.json\"\n\
             citation_source_paths = [\"src\"]\n",
        );
        let removed = cfg("[gates.bench-regression]\nratio_baseline = \"other.json\"\n");
        let found = diff_configs(&base, &removed).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what.contains(needle))
        };
        assert!(
            has("provenance-tags", "`superseded_registry` removed"),
            "{found:?}"
        );
        assert!(
            has("provenance-tags", "`superseded_json_paths` lost 1"),
            "{found:?}"
        );
        assert!(
            has(
                "provenance-tags",
                "`require_open_pending_issues` changed from true to false"
            ),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`mode` changed from paired-ratio"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`ratio_baseline` changed from"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`citation_source_paths` lost 1"),
            "{found:?}"
        );

        // Adopting the registry or the stricter mode is not a weakening.
        let plain = cfg("");
        assert!(diff_configs(&plain, &base).unwrap().is_empty());
    }
}
