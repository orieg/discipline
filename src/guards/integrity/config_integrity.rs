//! `config-integrity`: a change that loosens `discipline.toml` needs a scoped
//! `allow-gate-weakening:` directive.

use super::diff_configs_under;
use crate::config::{DisciplineConfig, GateSettings, RunMode, Severity};
use crate::guards::{Context, GateOutcome};
use crate::tokens;
use anyhow::Result;
use toml::Value;

/// The base-side configuration source. A `--config` pointing at a file the base does not
/// have still compares against the base `discipline.toml`.
fn base_config_source(ctx: &Context) -> Result<Option<String>> {
    ctx.base_config_text()
}

/// Whether the base side runs this gate. The change under review cannot switch off the
/// gate that judges its configuration: `enabled = false` takes effect once it has merged.
///
/// A base configuration that exists but does not load cannot say the gate was off, so it
/// counts as on: the gate then reports that it could not compare.
pub fn enabled_on_base(ctx: &Context) -> Result<bool> {
    Ok(match base_config_source(ctx)? {
        None => false,
        Some(src) => DisciplineConfig::from_toml_str(&src)
            .map_or(true, |base| base.gates.config_integrity.enabled()),
    })
}

/// Whether this change is what switched the run to advisory mode, with no scoped override
/// lifting it. Advisory mode exits 0 whatever the gates report, so honouring it here would
/// let the change excuse itself. Adopting discipline in advisory mode (no base
/// configuration) is unaffected.
pub fn advisory_mode_unapproved(ctx: &Context) -> Result<bool> {
    if ctx.config.meta.mode != RunMode::Advisory {
        return Ok(false);
    }
    // A base configuration that does not load cannot say the run was advisory already.
    let base_enforcing = match base_config_source(ctx)? {
        None => false,
        Some(src) => DisciplineConfig::from_toml_str(&src)
            .map_or(true, |base| base.meta.mode == RunMode::Enforcing),
    };
    Ok(base_enforcing
        && ctx
            .find_override(
                "config-integrity",
                &crate::findings::GATE_WEAKENED,
                tokens::ALLOW_GATE_WEAKENING,
                "meta",
            )
            .is_none())
}

/// The stricter of two severities.
fn stricter(a: Severity, b: Severity) -> Severity {
    let rank = |s: Severity| match s {
        Severity::Error => 2,
        Severity::Warning => 1,
        Severity::Note => 0,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

pub fn config_integrity(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "config-integrity";
    let settings = &ctx.config.gates.config_integrity;
    let mut out = GateOutcome::new(GATE);
    // Findings are reported at the stricter of the base and head severity, so a change
    // cannot demote the report of its own weakenings.
    let mut severity = settings.severity();

    // A configuration outside the repository is the operator's, not the change's: the
    // change cannot have weakened it, so there is nothing of its own to compare.
    if !crate::gitctx::config_in_tree(ctx.config_path) {
        out.notes.push(format!(
            "the configuration in force (`{}`) is outside the repository, so this change cannot edit it; no weakening compared",
            ctx.config_path
        ));
    } else if let Some(base_src) = base_config_source(ctx)? {
        match DisciplineConfig::from_toml_str(&base_src) {
            Ok(base) => {
                let head = ctx.head_config.unwrap_or(ctx.config);
                severity = stricter(severity, base.gates.config_integrity.severity());
                if !settings.enabled() {
                    out.notes.push(
                        "this change disables `config-integrity`; evaluated anyway because the \
                         base configuration enables it"
                            .to_string(),
                    );
                }
                let weakenings = diff_configs_under(
                    &base,
                    head,
                    super::command::runner_authorises_command_change(),
                )?;
                out.examined = Value::try_from(&base.gates)?
                    .as_table()
                    .map(|t| t.len())
                    .unwrap_or(0);
                for w in weakenings {
                    if let Some(record) = ctx.find_override(
                        GATE,
                        &crate::findings::GATE_WEAKENED,
                        tokens::ALLOW_GATE_WEAKENING,
                        &w.gate,
                    ) {
                        out.overrides.push(record);
                        continue;
                    }
                    out.push(
                        ctx.overridable(severity),
                        &crate::findings::GATE_WEAKENED,
                        Some(ctx.config_path),
                        None,
                        format!("[{}] {}.", w.gate, w.what()),
                        &format!(
                            "Revert the change, or justify it on its own line in the PR body or a commit \
                             message: `allow-gate-weakening: {} <reason>`.",
                            w.gate
                        ),
                    );
                    out.anchor_last(format!("{}.{}", w.gate, w.key()));
                }
            }
            Err(e) => {
                // Blocking here would deadlock the PR that repairs the base config.
                out.push(
                    Severity::Warning,
                    &crate::findings::BASE_CONFIGURATION_UNREADABLE,
                    Some(ctx.config_path),
                    None,
                    format!("The base-side configuration does not load with this binary ({e:#}); weakening could not be checked, and the base-side `min_tests` and `min_count` ratchets and assertion vocabulary were not read."),
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
    let baseline_given = ctx
        .baseline_path
        .unwrap_or(crate::baseline::DEFAULT_BASELINE_FILE);
    let head_baseline_path = ctx.git.root().join(baseline_given);
    // A baseline outside the repository is the operator's, as a configuration outside it
    // is: no side of the change holds it, so there is no growth of the change's own to
    // compare. It is named by its file name alone; its directory is the runner's.
    let Some(baseline_in_tree) =
        crate::baseline::path_in_repository(ctx.git.root(), &head_baseline_path)
    else {
        out.notes.push(format!(
            "the baseline in force (`{}`) is outside the repository, so this change cannot edit it; baseline growth was not compared",
            crate::baseline::path_for_message(ctx.git.root(), &head_baseline_path)
        ));
        return Ok(out);
    };
    // The path git knows the file by, which is also how messages and findings name it.
    let baseline_filename = baseline_in_tree.as_str();
    let baseline_shown = baseline_filename;
    let base_baseline: Option<crate::baseline::DisciplineBaseline> = match ctx
        .git
        .base_content(baseline_filename)?
    {
        Some(s) => match toml::from_str(&s) {
            Ok(baseline) => Some(baseline),
            Err(_) => {
                out.notes.push(format!(
                        "`{baseline_shown}` does not parse on the base side; baseline growth was not checked"
                    ));
                None
            }
        },
        None => None,
    };

    let head_baseline: Option<crate::baseline::DisciplineBaseline> = if head_baseline_path.exists()
    {
        std::fs::read_to_string(&head_baseline_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
    } else {
        None
    };

    // A fingerprint-version migration (`discipline baseline --migrate`) rewrites every
    // fingerprint. It is accepted without a directive when it is the change's only file,
    // does not grow, and keeps every entry's gate and path: then no finding can have been
    // swapped in under it.
    if let (Some(b), Some(h)) = (&base_baseline, &head_baseline) {
        if b.version < crate::baseline::FINGERPRINT_VERSION
            && h.version >= crate::baseline::FINGERPRINT_VERSION
        {
            let others: Vec<String> = ctx
                .git
                .changed_files()?
                .into_iter()
                .map(|f| f.path)
                .filter(|p| p != baseline_filename)
                .collect();
            let mut base_places: std::collections::HashMap<(&str, &str), usize> =
                std::collections::HashMap::new();
            for e in &b.findings {
                *base_places
                    .entry((e.gate.as_str(), e.path.as_str()))
                    .or_insert(0) += 1;
            }
            let kept_places = h.findings.iter().all(|e| {
                base_places
                    .get_mut(&(e.gate.as_str(), e.path.as_str()))
                    .filter(|n| **n > 0)
                    .map(|n| *n -= 1)
                    .is_some()
            });
            if others.is_empty() && kept_places {
                out.notes.push(format!(
                    "`{baseline_shown}` migrated from fingerprint version {} to {} ({} of {} entries kept)",
                    b.version,
                    h.version,
                    h.findings.len(),
                    b.findings.len()
                ));
                return Ok(out);
            }
            if !others.is_empty() {
                out.lift_or_push(
                    ctx.find_override(
                        GATE,
                        &crate::findings::BASELINE_MIGRATION_NOT_ALONE,
                        tokens::ALLOW_GATE_WEAKENING,
                        "baseline",
                    ),
                    ctx.overridable(severity),
                    &crate::findings::BASELINE_MIGRATION_NOT_ALONE,
                    (Some(baseline_shown), None),
                    format!(
                        "[baseline] `{baseline_shown}` is rewritten from fingerprint version {} to {} in a change that also touches {} other file(s) (e.g. `{}`); a migration is only verifiable on its own.",
                        b.version,
                        h.version,
                        others.len(),
                        others[0]
                    ),
                    "Commit `discipline baseline --migrate` in a change of its own, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
                return Ok(out);
            }
        }
    }

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
            // The finding the else-branches below report: new findings under an unchanged
            // count, else a grown baseline.
            let lifts = if !new_fps.is_empty() && h_count <= b_count {
                &crate::findings::BASELINE_NEW_FINDINGS
            } else {
                &crate::findings::BASELINE_INCREASED
            };
            if let Some(record) = ctx
                .find_override(GATE, lifts, tokens::ALLOW_GATE_WEAKENING, "baseline")
                .or_else(|| {
                    ctx.find_override(GATE, lifts, tokens::ALLOW_GATE_WEAKENING, baseline_filename)
                        .or_else(|| {
                            // The file as the run was given it, when that differs.
                            ctx.find_override(
                                GATE,
                                lifts,
                                tokens::ALLOW_GATE_WEAKENING,
                                baseline_given,
                            )
                        })
                })
            {
                out.overrides.push(record);
            } else if !new_fps.is_empty() && h_count <= b_count {
                let count = new_fps.len();
                let sample = new_fps.first().unwrap_or(&"");
                out.push(
                    ctx.overridable(severity),
                    &crate::findings::BASELINE_NEW_FINDINGS,
                    Some(baseline_shown),
                    None,
                    format!("[baseline] Grandfathered baseline contains {count} new fingerprint(s) not present on base (e.g. `{sample}`). A 1-for-1 replacement of grandfathered findings with new ones is forbidden."),
                    "Revert the baseline modification, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
            } else {
                let diff = h_count.saturating_sub(b_count);
                out.push(
                    ctx.overridable(severity),
                    &crate::findings::BASELINE_INCREASED,
                    Some(baseline_shown),
                    None,
                    format!("[baseline] Grandfathered baseline grew from {b_count} to {h_count} findings ({diff} new grandfathered findings)."),
                    "Revert the baseline growth, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
            }
        }
    }

    Ok(out)
}
