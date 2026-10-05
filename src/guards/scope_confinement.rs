//! Scope confinement sentinel (`scope-confinement`).
//!
//! Restricts agent modifications strictly within authorized directory and file paths.

use crate::guards::{Context, GateOutcome, PathFilter};
use crate::tokens::ALLOW_SCOPE;
use anyhow::Result;

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

    // A malformed glob is a configuration error (exit 2), never a silently
    // skipped pattern that would let a forbidden path pass.
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    // Build allowed paths filter (if non-empty)
    let has_allowed = !settings.allowed_paths.is_empty();
    let allowed = PathFilter::new(&settings.allowed_paths)?;

    // Build forbidden paths filter
    let forbidden = PathFilter::new(&settings.forbidden_paths)?;

    for file in &changed {
        if exempt.matches(&file.path) {
            continue;
        }

        // 1. Check forbidden paths first
        if forbidden.matches(&file.path) {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::FILE_IN_FORBIDDEN_SCOPE,
                ALLOW_SCOPE,
                &file.path,
            ) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` for forbidden file `{}` ({})",
                    ov.directive, ov.reason, file.path, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::FILE_IN_FORBIDDEN_SCOPE,
                    &file.path,
                    1,
                    format!("file `{}` is inside forbidden scope", file.path),
                    "changes to this path are forbidden by [gates.scope-confinement.forbidden_paths]; use `discipline:allow(scope-confinement): <reason>` to waive",
                );
            }
            continue;
        }

        // 2. Check allowed paths if configured
        if has_allowed && !allowed.matches(&file.path) {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::FILE_OUTSIDE_AUTHORIZED_SCOPE,
                ALLOW_SCOPE,
                &file.path,
            ) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` for out-of-scope file `{}` ({})",
                    ov.directive, ov.reason, file.path, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::FILE_OUTSIDE_AUTHORIZED_SCOPE,
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

pub fn check_path_confinement(
    path: &str,
    exempt: &PathFilter,
    allowed: &PathFilter,
    has_allowed: bool,
    forbidden: &PathFilter,
) -> Option<&'static str> {
    if exempt.matches(path) {
        return None;
    }
    if forbidden.matches(path) {
        return Some("forbidden");
    }
    if has_allowed && !allowed.matches(path) {
        return Some("outside-allowed");
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::config::{ScopeConfinementGate, Severity};
    use crate::guards::PathFilter;

    #[test]
    fn test_scope_confinement_defaults() {
        let gate = ScopeConfinementGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(gate.allowed_paths.is_empty());
        assert!(gate.forbidden_paths.is_empty());
    }

    #[test]
    fn invalid_glob_is_an_error_not_a_silently_skipped_pattern() {
        let bad = ScopeConfinementGate {
            enabled: true,
            forbidden_paths: vec!["[".to_string()],
            ..Default::default()
        };
        assert!(PathFilter::new(&bad.exempt_paths).is_ok());
        assert!(PathFilter::new(&bad.allowed_paths).is_ok());
        assert!(PathFilter::new(&bad.forbidden_paths).is_err());
    }

    #[test]
    fn valid_globs_still_match_through_check_path_confinement() {
        let exempt = PathFilter::new(&["tests/fixtures/**".to_string()]).unwrap();
        let allowed = PathFilter::new(&["src/**".to_string()]).unwrap();
        let forbidden = PathFilter::new(&[".github/**".to_string()]).unwrap();
        assert!(
            super::check_path_confinement("src/lib.rs", &exempt, &allowed, true, &forbidden)
                .is_none()
        );
        assert_eq!(
            super::check_path_confinement(
                ".github/workflows/ci.yml",
                &exempt,
                &allowed,
                true,
                &forbidden
            ),
            Some("forbidden")
        );
        assert_eq!(
            super::check_path_confinement("docs/guide.md", &exempt, &allowed, true, &forbidden),
            Some("outside-allowed")
        );
        assert!(super::check_path_confinement(
            "tests/fixtures/data.bin",
            &exempt,
            &allowed,
            true,
            &forbidden
        )
        .is_none());
    }
}
