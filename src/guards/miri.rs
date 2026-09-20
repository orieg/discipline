//! Miri Undefined Behavior verification sentinel (`miri`).
//!
//! Executes `cargo miri test` with a zero-tests guard to detect undefined behavior
//! without allowing silent passes when test filters match zero items.

use crate::guards::command::run_command_bounded;
use crate::guards::presets::resolve_preset;
use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_MIRI;
use anyhow::Result;

pub const GATE: &str = "miri";

pub fn evaluate_miri(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.miri;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    out.examined = 1;

    let preset = resolve_preset("miri").expect("miri preset is statically defined");
    let base_cmd = preset.default_command;
    let full_cmd = if settings.args.is_empty() {
        base_cmd.to_string()
    } else {
        format!("{} {}", base_cmd, settings.args.join(" "))
    };

    let timeout_secs = if settings.timeout_seconds > 0 {
        settings.timeout_seconds
    } else {
        preset.default_timeout_seconds
    };

    let root = ctx.git.root();

    let res = match run_command_bounded("miri", &full_cmd, timeout_secs, root) {
        Ok(r) => r,
        Err(e) => {
            if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_MIRI, GATE) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (miri execution error allowed) ({})",
                    ov.directive, ov.reason, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    "miri",
                    1,
                    format!("miri command execution failed: {e}"),
                    "ensure cargo-miri is installed (`cargo miri setup`); use `discipline:allow(miri): <reason>` to waive",
                );
            }
            return Ok(out);
        }
    };

    let stdout = &res.stdout;
    let stderr = &res.stderr;

    // Check zero-tests guard
    if let Some(zero_pat) = preset.zero_items_pattern {
        if stdout.contains(zero_pat) || stderr.contains(zero_pat) {
            if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_MIRI, GATE) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (miri 0 tests allowed) ({})",
                    ov.directive, ov.reason, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    "miri",
                    1,
                    "miri reported 0 tests executed (zero-tests guard triggered)",
                    "miri suite executed zero tests; use `discipline:allow(miri): <reason>` to waive",
                );
                return Ok(out);
            }
        }
    }

    if !res.status.success() {
        if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_MIRI, GATE) {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` (miri failure allowed) ({})",
                ov.directive, ov.reason, ov.source
            ));
        } else {
            let diag = if !stderr.is_empty() { stderr } else { stdout };
            out.add_violation(
                ctx.overridable(settings.severity),
                "miri",
                1,
                "miri detected undefined behavior or assertion failure",
                diag.lines().next().unwrap_or("miri exited non-zero"),
            );
        }
    } else {
        out.notes
            .push("miri verification completed with zero undefined behavior findings".to_string());
    }

    Ok(out)
}

pub fn evaluate_miri_output(
    status_success: bool,
    stdout: &str,
    stderr: &str,
    zero_pat: Option<&str>,
) -> Option<&'static str> {
    if let Some(zero_pat) = zero_pat {
        if stdout.contains(zero_pat) || stderr.contains(zero_pat) {
            return Some("zero-tests");
        }
    }
    if !status_success {
        return Some("failure");
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::config::{MiriGate, Severity};

    #[test]
    fn test_miri_defaults() {
        let gate = MiriGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(gate.timeout_seconds, 600);
    }
}
