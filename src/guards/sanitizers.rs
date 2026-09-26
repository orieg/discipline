//! Runtime sanitizers sentinel (`sanitizers`).
//!
//! Executes address (ASan) or thread (TSan) sanitizers with negative-control race canaries
//! and audited suppression list verification.

use crate::guards::command::run_command_bounded;
use crate::guards::presets::resolve_preset;
use crate::guards::{toolchain_unavailable, Context, GateOutcome};
use crate::tokens::ALLOW_SANITIZERS;
use anyhow::Result;

pub const GATE: &str = "sanitizers";

pub fn evaluate_sanitizers(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.sanitizers;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    out.examined = 1;

    let preset = resolve_preset("sanitizers").expect("sanitizers preset is statically defined");
    let cmd = format!("cargo test -Zsanitizer={}", settings.sanitizer);
    let timeout_secs = if settings.timeout_seconds > 0 {
        settings.timeout_seconds
    } else {
        preset.default_timeout_seconds
    };

    let root = ctx.git.root();

    // 1. Canary verification if enabled
    if settings.canary {
        if let Some(canary_cmd) = preset.canary_command {
            let canary_res = run_command_bounded("sanitizers-canary", canary_cmd, 60, root)?;
            let combined = format!("{}\n{}", canary_res.stdout, canary_res.stderr);
            let expected = preset.canary_expected_diagnostic.unwrap_or("Sanitizer");
            if !combined.contains(expected) {
                if let Some(ov) = ctx
                    .find_override(GATE, ALLOW_SANITIZERS, "canary")
                    .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "sanitizers"))
                {
                    out.overrides.push(ov.clone());
                    out.notes.push(format!(
                        "override applied: `{}: {}` (canary diagnostic mismatch allowed) ({})",
                        ov.directive, ov.reason, ov.source
                    ));
                } else {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        &crate::findings::SANITIZER_CANARY_DIAGNOSTIC_MISSING,
                        "sanitizers-canary",
                        1,
                        "sanitizer negative-control canary failed to produce expected diagnostic",
                        format!(
                            "canary command `{}` did not report `{}`",
                            canary_cmd, expected
                        ),
                    );
                    return Ok(out);
                }
            } else {
                out.notes.push(format!(
                    "sanitizer race canary verified: produced `{expected}`"
                ));
            }
        }
    }

    // 2. Main sanitizer execution
    let res = match run_command_bounded("sanitizers", &cmd, timeout_secs, root) {
        Ok(r) => r,
        Err(e) => {
            if let Some(ov) = ctx
                .find_override(GATE, ALLOW_SANITIZERS, "execution")
                .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "sanitizers"))
                .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "toolchain"))
                .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "nightly"))
            {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (sanitizer execution error allowed) ({})",
                    ov.directive, ov.reason, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::SANITIZER_COULD_NOT_RUN,
                    "sanitizers",
                    1,
                    format!("sanitizer command execution failed: {e}"),
                    "ensure nightly Rust and sanitizer libraries are available; use `discipline:allow(sanitizers): <reason>` to waive",
                );
            }
            return Ok(out);
        }
    };

    if !res.status.success() {
        if let Some(ov) = ctx
            .find_override(GATE, ALLOW_SANITIZERS, "failure")
            .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "sanitizers"))
            .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "toolchain"))
            .or_else(|| ctx.find_override(GATE, ALLOW_SANITIZERS, "nightly"))
        {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` (sanitizer failure allowed) ({})",
                ov.directive, ov.reason, ov.source
            ));
        } else if let Some(fault) = toolchain_unavailable(&res.stdout, &res.stderr) {
            // Fail-closed: a missing nightly channel or sanitizer support means
            // the run never happened; reporting it as a detected race would
            // invert the meaning of the result.
            anyhow::bail!(
                "sanitizer ({}) could not run: {fault}. Sanitizers need a nightly \
                 toolchain (`cargo +nightly`) or the gate must be disabled.",
                settings.sanitizer
            );
        } else {
            let diag = if !res.stderr.is_empty() {
                &res.stderr
            } else {
                &res.stdout
            };
            out.add_violation(
                ctx.overridable(settings.severity),
                &crate::findings::SANITIZER_VIOLATION_DETECTED,
                "sanitizers",
                1,
                format!(
                    "sanitizer ({}) detected memory safety or race violations",
                    settings.sanitizer
                ),
                diag.lines().next().unwrap_or("sanitizer reported failure"),
            );
        }
    } else {
        out.notes.push(format!(
            "sanitizer ({}) test run completed cleanly",
            settings.sanitizer
        ));
    }

    Ok(out)
}

pub fn evaluate_canary_diagnostic(output: &str, expected: &str) -> bool {
    output.contains(expected)
}

#[cfg(test)]
mod tests {
    use crate::config::{SanitizersGate, Severity};

    #[test]
    fn test_sanitizers_defaults() {
        let gate = SanitizersGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(gate.sanitizer, "address");
        assert!(!gate.canary);
        assert_eq!(gate.timeout_seconds, 300);
    }
}
