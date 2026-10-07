//! Unsafe code budget sentinel (`unsafe-budget`).
//!
//! Enforces an unsafe code count ratchet: the total number of `unsafe` blocks
//! and functions cannot increase without an explicit justification directive.

use crate::ast::default_registry;
use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome, PathFilter};
use crate::tokens::ALLOW_UNSAFE;
use anyhow::Result;

pub const GATE: &str = "unsafe-budget";

pub fn evaluate_unsafe_budget(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.unsafe_budget;
    let mut out = GateOutcome::new(GATE);

    let changed = ctx.git.changed_files()?;
    if changed.is_empty() {
        return Ok(out);
    }

    // A malformed glob is a configuration error (exit 2), never a silently
    // skipped exemption.
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    let registry = default_registry();
    let vocab = super::agent_diff::assert_vocabulary(ctx.config);

    let mut base_unsafe_count = 0;
    let mut head_unsafe_count = 0;
    let mut files_examined = 0;
    let mut new_unsafe_sites = Vec::new();

    for file in &changed {
        if exempt.matches(&file.path) {
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
            if let Some(ov) = ctx
                .find_override(
                    GATE,
                    &crate::findings::UNSAFE_BUDGET_EXCEEDED,
                    ALLOW_UNSAFE,
                    "max_unsafe",
                )
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::UNSAFE_BUDGET_EXCEEDED,
                        ALLOW_UNSAFE,
                        "unsafe-budget",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::UNSAFE_BUDGET_EXCEEDED,
                        ALLOW_UNSAFE,
                        "budget",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::UNSAFE_BUDGET_EXCEEDED,
                        ALLOW_UNSAFE,
                        "FFI",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::UNSAFE_BUDGET_EXCEEDED,
                        ALLOW_UNSAFE,
                        "pointer",
                    )
                })
            {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (unsafe count {} exceeds cap of {}) ({})",
                    ov.directive, ov.reason, head_unsafe_count, max, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &crate::findings::UNSAFE_BUDGET_EXCEEDED,
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
        for (path, line, kind) in &new_unsafe_sites {
            let file_stem = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str());
            let ov = ctx
                .find_override(
                    GATE,
                    &crate::findings::UNSAFE_ADDED_WITHOUT_AUTHORIZATION,
                    ALLOW_UNSAFE,
                    path,
                )
                .or_else(|| {
                    file_stem.and_then(|s| {
                        ctx.find_override(
                            GATE,
                            &crate::findings::UNSAFE_ADDED_WITHOUT_AUTHORIZATION,
                            ALLOW_UNSAFE,
                            s,
                        )
                    })
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::UNSAFE_ADDED_WITHOUT_AUTHORIZATION,
                        ALLOW_UNSAFE,
                        "FFI",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::UNSAFE_ADDED_WITHOUT_AUTHORIZATION,
                        ALLOW_UNSAFE,
                        "unsafe-budget",
                    )
                });

            if let Some(record) = ov {
                out.overrides.push(record.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (unsafe {kind} in `{path}:{line}` allowed) ({})",
                    record.directive, record.reason, record.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::UNSAFE_ADDED_WITHOUT_AUTHORIZATION,
                    path,
                    *line,
                    format!("unsafe {} added without budget increase authorization (+{} net)", kind, delta),
                    format!("unsafe count increased from {} to {}; use `allow-unsafe: <path> <reason>` to waive", base_unsafe_count, head_unsafe_count),
                );
            }
        }
        if new_unsafe_sites.is_empty() {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::UNSAFE_COUNT_INCREASED,
                ALLOW_UNSAFE,
                "unsafe-budget",
            ) {
                out.overrides.push(ov.clone());
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &crate::findings::UNSAFE_COUNT_INCREASED,
                    "workspace",
                    1,
                    format!(
                        "unsafe count increased from {} to {} (+{})",
                        base_unsafe_count, head_unsafe_count, delta
                    ),
                    "use `allow-unsafe: <path> <reason>` to waive",
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

    /// Calls the gate over a change to `generated/bindings.rs` with `body` as its table.
    fn evaluate(body: &str) -> anyhow::Result<crate::guards::GateOutcome> {
        let config = crate::config::DisciplineConfig::from_toml_str(&format!(
            "[meta]\nversion = 1\nname = \"t\"\n[gates.unsafe-budget]\nenabled = true\n{body}\n"
        ))?;
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "generated/bindings.rs",
            "pub fn a() {}\n",
            "pub fn a() {}\npub fn read(p: *const u8) -> u8 {\n    // SAFETY: callers pass a pointer that is valid for reads.\n    unsafe { *p }\n}\n",
        );
        super::evaluate_unsafe_budget(&crate::guards::test_support::context(&config, &git))
    }

    /// `run_checks` compiles every glob before a gate runs, so the binary never brings
    /// an invalid one this far; the gate is called directly to reach its own refusal.
    #[test]
    fn invalid_exempt_glob_is_an_error_not_a_silently_skipped_pattern() {
        let Err(err) = evaluate("exempt_paths = [\"[\"]") else {
            panic!("an invalid exempt glob was accepted");
        };
        let text = format!("{err:#}");
        assert!(text.contains("invalid glob `[` in configuration"), "{text}");
    }

    /// Control: a glob that compiles exempts the file it matches, and only that file.
    #[test]
    fn valid_exempt_glob_still_matches() {
        let reported = evaluate("exempt_paths = [\"vendor/**\"]").unwrap();
        assert_eq!(reported.violations.len(), 1, "{:?}", reported.notes);
        let exempted = evaluate("exempt_paths = [\"generated/**\"]").unwrap();
        assert!(exempted.violations.is_empty(), "{:?}", exempted.notes);
    }
}
