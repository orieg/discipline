//! Unsafe code budget sentinel (`unsafe-budget`).
//!
//! Enforces an unsafe code count ratchet: the total number of `unsafe` blocks
//! and functions cannot increase without an explicit justification directive.

use crate::ast::{default_registry, AssertVocabulary};
use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_UNSAFE;
use anyhow::Result;
use globset::{Glob, GlobSetBuilder};

pub const GATE: &str = "unsafe-budget";

pub fn evaluate_unsafe_budget(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.unsafe_budget;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let changed = ctx.git.changed_files()?;
    if changed.is_empty() {
        return Ok(out);
    }

    let mut exempt_builder = GlobSetBuilder::new();
    for pat in &settings.exempt_paths {
        if let Ok(g) = Glob::new(pat) {
            exempt_builder.add(g);
        }
    }
    let exempt_set = exempt_builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap());

    let registry = default_registry();
    let vocab = AssertVocabulary::default();

    let mut base_unsafe_count = 0;
    let mut head_unsafe_count = 0;
    let mut files_examined = 0;
    let mut new_unsafe_sites = Vec::new();

    for file in &changed {
        if exempt_set.is_match(&file.path) {
            continue;
        }

        if !registry.is_supported(&file.path) && !registry.is_supported(&file.old_path) {
            continue;
        }

        files_examined += 1;

        // Base facts
        let base_sites = if let Some(bytes) = ctx.git.base_bytes(&file.old_path)? {
            if let Some(pack) = registry.find_pack(&file.old_path) {
                let src = String::from_utf8_lossy(&bytes);
                pack.extract(&file.old_path, &src, &vocab)?
                    .unsafe_sites
                    .len()
            } else {
                0
            }
        } else {
            0
        };
        base_unsafe_count += base_sites;

        // Head facts
        let head_sites = match file.kind {
            ChangeKind::Deleted => Vec::new(),
            _ => {
                if let Some(bytes) = ctx.git.head_bytes(&file.path)? {
                    if let Some(pack) = registry.find_pack(&file.path) {
                        let src = String::from_utf8_lossy(&bytes);
                        pack.extract(&file.path, &src, &vocab)?.unsafe_sites
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
        };

        if head_sites.len() > base_sites {
            for site in &head_sites {
                if file.added_lines.contains(&site.line) {
                    new_unsafe_sites.push((file.path.clone(), site.line, site.kind));
                }
            }
        }
        head_unsafe_count += head_sites.len();
    }

    out.examined = files_examined;

    if let Some(max) = settings.max_unsafe {
        if head_unsafe_count > max {
            if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_UNSAFE, GATE) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (unsafe count {} exceeds cap of {}) ({})",
                    ov.directive, ov.reason, head_unsafe_count, max, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    "workspace",
                    1,
                    format!(
                        "unsafe count {} exceeds maximum budget of {}",
                        head_unsafe_count, max
                    ),
                    "use `discipline:allow(unsafe-budget): <reason>` to waive",
                );
            }
        }
    }

    if !settings.allow_increase && head_unsafe_count > base_unsafe_count {
        let delta = head_unsafe_count - base_unsafe_count;
        if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_UNSAFE, GATE) {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` (+{} unsafe sites admitted) ({})",
                ov.directive, ov.reason, delta, ov.source
            ));
        } else {
            for (path, line, kind) in &new_unsafe_sites {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    path,
                    *line,
                    format!("unsafe {} added without budget increase authorization (+{} net)", kind, delta),
                    format!("unsafe count increased from {} to {}; use `discipline:allow(unsafe-budget): <reason>` to waive", base_unsafe_count, head_unsafe_count),
                );
            }
            if new_unsafe_sites.is_empty() {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    "workspace",
                    1,
                    format!(
                        "unsafe count increased from {} to {} (+{})",
                        base_unsafe_count, head_unsafe_count, delta
                    ),
                    "use `discipline:allow(unsafe-budget): <reason>` to waive",
                );
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::config::{Severity, UnsafeBudgetGate};

    #[test]
    fn test_unsafe_budget_defaults() {
        let gate = UnsafeBudgetGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(gate.max_unsafe, None);
        assert!(!gate.allow_increase);
    }
}
