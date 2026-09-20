//! Scope confinement sentinel (`scope-confinement`).
//!
//! Restricts agent modifications strictly within authorized directory and file paths.

use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_SCOPE;
use anyhow::Result;
use globset::{Glob, GlobSetBuilder};

pub const GATE: &str = "scope-confinement";

/// Evaluates git diff paths against authorized and forbidden scope rules.
pub fn evaluate_scope_confinement(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.scope_confinement;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let changed = ctx.git.changed_files()?;
    out.examined = changed.len();

    if changed.is_empty() {
        return Ok(out);
    }

    // Build exempt paths globset
    let mut exempt_builder = GlobSetBuilder::new();
    for pat in &settings.exempt_paths {
        if let Ok(g) = Glob::new(pat) {
            exempt_builder.add(g);
        }
    }
    let exempt_set = exempt_builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap());

    // Build allowed paths globset (if non-empty)
    let has_allowed = !settings.allowed_paths.is_empty();
    let mut allowed_builder = GlobSetBuilder::new();
    for pat in &settings.allowed_paths {
        if let Ok(g) = Glob::new(pat) {
            allowed_builder.add(g);
        }
    }
    let allowed_set = allowed_builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap());

    // Build forbidden paths globset
    let mut forbidden_builder = GlobSetBuilder::new();
    for pat in &settings.forbidden_paths {
        if let Ok(g) = Glob::new(pat) {
            forbidden_builder.add(g);
        }
    }
    let forbidden_set = forbidden_builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap());

    for file in &changed {
        if exempt_set.is_match(&file.path) {
            continue;
        }

        // 1. Check forbidden paths first
        if forbidden_set.is_match(&file.path) {
            if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_SCOPE, &file.path) {
                out.notes.push(format!(
                    "override applied: `{}: {}` for forbidden file `{}` ({})",
                    ov.directive, ov.reason, file.path, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &file.path,
                    1,
                    format!("file `{}` is inside forbidden scope", file.path),
                    "changes to this path are forbidden by [gates.scope-confinement.forbidden_paths]; use `discipline:allow(scope-confinement): <reason>` to waive",
                );
            }
            continue;
        }

        // 2. Check allowed paths if configured
        if has_allowed && !allowed_set.is_match(&file.path) {
            if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_SCOPE, &file.path) {
                out.notes.push(format!(
                    "override applied: `{}: {}` for out-of-scope file `{}` ({})",
                    ov.directive, ov.reason, file.path, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &file.path,
                    1,
                    format!("file `{}` is outside authorized scope", file.path),
                    "file is not included in [gates.scope-confinement.allowed_paths]; use `discipline:allow(scope-confinement): <reason>` to waive",
                );
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::config::{ScopeConfinementGate, Severity};

    #[test]
    fn test_scope_confinement_defaults() {
        let gate = ScopeConfinementGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(gate.allowed_paths.is_empty());
        assert!(gate.forbidden_paths.is_empty());
    }
}
