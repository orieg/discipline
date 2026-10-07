//! Runtime sanitizers sentinel (`sanitizers`).
//!
//! Executes address (ASan) or thread (TSan) sanitizers with negative-control race canaries
//! and audited suppression list verification.

use crate::guards::command::{run_argv_bounded, run_command_bounded, ExecutedKeys};
use crate::guards::presets::resolve_preset;
use crate::guards::{toolchain_unavailable, Context, GateOutcome};
use crate::tokens::ALLOW_SANITIZERS;
use anyhow::Result;

pub const GATE: &str = "sanitizers";

pub fn evaluate_sanitizers(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.sanitizers;
    let mut out = GateOutcome::new(GATE);

    // `sanitizer` reaches `cargo` and `canary` selects a second command: a change cannot
    // supply either (`configured_execution`). Nothing runs before this, the canary included.
    let Some(execution_notes) = crate::guards::command::vouched_execution(
        ctx,
        &mut out,
        settings.severity,
        &crate::findings::SANITIZERS_UNTRUSTED_COMMAND_MODIFICATION,
        &executed_keys,
    )?
    else {
        return Ok(out);
    };

    out.examined = 1;

    let preset = resolve_preset("sanitizers").expect("sanitizers preset is statically defined");
    let argv = sanitizer_argv(&settings.sanitizer)?;
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
                    .find_override(
                        GATE,
                        &crate::findings::SANITIZER_CANARY_DIAGNOSTIC_MISSING,
                        ALLOW_SANITIZERS,
                        "canary",
                    )
                    .or_else(|| {
                        ctx.find_override(
                            GATE,
                            &crate::findings::SANITIZER_CANARY_DIAGNOSTIC_MISSING,
                            ALLOW_SANITIZERS,
                            "sanitizers",
                        )
                    })
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
    let res = match run_argv_bounded("sanitizers", &argv, timeout_secs, root) {
        Ok(r) => r,
        // A run that could not start verified nothing: exit 2 (could not check), which no
        // directive lifts. A job without the toolchain disables the gate instead.
        Err(e) => return Err(e.context("sanitizer could not run")),
    };
    out.notes.extend(execution_notes);

    if !res.status.success() {
        if let Some(fault) = toolchain_unavailable(&res.stdout, &res.stderr) {
            // Fail-closed: a missing nightly channel or sanitizer support means
            // the run never happened; reporting it as a detected race would
            // invert the meaning of the result.
            return Err(crate::could_not_check::tag(
                crate::could_not_check::Reason::ToolchainUnavailable,
                anyhow::anyhow!(
                    "sanitizer ({}) could not run: {fault}. Sanitizers need a nightly \
                     toolchain (`cargo +nightly`) or the gate must be disabled.",
                    settings.sanitizer
                ),
            ));
        }
        if let Some(ov) = ctx
            .find_override(
                GATE,
                &crate::findings::SANITIZER_VIOLATION_DETECTED,
                ALLOW_SANITIZERS,
                "failure",
            )
            .or_else(|| {
                ctx.find_override(
                    GATE,
                    &crate::findings::SANITIZER_VIOLATION_DETECTED,
                    ALLOW_SANITIZERS,
                    "sanitizers",
                )
            })
            .or_else(|| {
                ctx.find_override(
                    GATE,
                    &crate::findings::SANITIZER_VIOLATION_DETECTED,
                    ALLOW_SANITIZERS,
                    "toolchain",
                )
            })
            .or_else(|| {
                ctx.find_override(
                    GATE,
                    &crate::findings::SANITIZER_VIOLATION_DETECTED,
                    ALLOW_SANITIZERS,
                    "nightly",
                )
            })
        {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` (sanitizer failure allowed) ({})",
                ov.directive, ov.reason, ov.source
            ));
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

/// The keys of `[gates.sanitizers]` whose value reaches a process invocation:
/// `sanitizer`, placed in an argument of the built-in command, and `canary`, which
/// selects whether the built-in canary command runs first.
pub(crate) fn executed_keys(gates: &crate::config::Gates) -> ExecutedKeys {
    vec![
        ("sanitizer", gates.sanitizers.sanitizer.clone()),
        ("canary", gates.sanitizers.canary.to_string()),
    ]
}

/// The configuration key of the sanitizer name, as a message names it.
pub const SANITIZER_KEY: &str = "gates.sanitizers.sanitizer";

/// The configuration key of the canary switch, as a message names it.
pub const CANARY_KEY: &str = "gates.sanitizers.canary";

/// The one sanitizer the built-in canary belongs to: the canary is a data race, and the
/// diagnostic it is expected to print is ThreadSanitizer's.
pub const CANARY_SANITIZER: &str = "thread";

/// `canary = true` with a `sanitizer` other than [`CANARY_SANITIZER`] is a configuration
/// error naming the key: no other sanitizer prints the diagnostic the canary is checked
/// for, so the pair could only ever report a missing diagnostic.
pub fn check_canary_sanitizer(settings: &crate::config::SanitizersGate) -> Result<()> {
    if !settings.canary || settings.sanitizer == CANARY_SANITIZER {
        return Ok(());
    }
    Err(crate::could_not_check::tag(
        crate::could_not_check::Reason::Configuration,
        anyhow::anyhow!(
            "`{CANARY_KEY}` is set with `{SANITIZER_KEY} = {:?}`: the built-in canary is a data race and the diagnostic it is checked for is ThreadSanitizer's, which only `sanitizer = \"{CANARY_SANITIZER}\"` prints; set `sanitizer = \"{CANARY_SANITIZER}\"` or `canary = false`",
            settings.sanitizer
        ),
    ))
}

/// The longest sanitizer name accepted.
const MAX_SANITIZER_NAME: usize = 32;

/// `sanitizer` is one sanitizer name: a lower-case ASCII letter, then lower-case
/// letters, digits and `-`, at most [`MAX_SANITIZER_NAME`] characters (`address`,
/// `thread`, `shadow-call-stack`). A value of another shape is a configuration error
/// naming the key: it is placed in an argument of `cargo`.
pub fn check_sanitizer_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let shaped = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.len() <= MAX_SANITIZER_NAME;
    if shaped {
        return Ok(());
    }
    Err(crate::could_not_check::tag(
        crate::could_not_check::Reason::Configuration,
        anyhow::anyhow!(
            "`{SANITIZER_KEY}` value {name:?} is not a sanitizer name: a lower-case letter, then lower-case letters, digits and `-`, at most {MAX_SANITIZER_NAME} characters (`address`, `thread`)"
        ),
    ))
}

/// The built-in command with the sanitizer name in one argument, never split again.
pub(crate) fn sanitizer_argv(name: &str) -> Result<Vec<String>> {
    check_sanitizer_name(name)?;
    Ok(vec![
        "cargo".to_string(),
        "test".to_string(),
        format!("-Zsanitizer={name}"),
    ])
}

pub fn evaluate_canary_diagnostic(output: &str, expected: &str) -> bool {
    output.contains(expected)
}

#[cfg(test)]
mod tests {
    use crate::config::{SanitizersGate, Severity};

    #[test]
    fn a_sanitizer_name_has_one_shape_and_is_one_argument() {
        use super::{check_sanitizer_name, sanitizer_argv};
        for ok in [
            "address",
            "thread",
            "memory",
            "leak",
            "hwaddress",
            "shadow-call-stack",
            "kcfi",
        ] {
            assert!(check_sanitizer_name(ok).is_ok(), "{ok}");
        }
        let too_long = "a".repeat(33);
        for bad in [
            "",
            "address thread",
            "-Zunstable-options",
            "address;x",
            "Address",
            "address,leak",
            "a$(x)",
            "1address",
            "address\n",
            too_long.as_str(),
        ] {
            let e = check_sanitizer_name(bad).unwrap_err().to_string();
            assert!(e.contains(super::SANITIZER_KEY), "{bad}: {e}");
        }
        assert_eq!(
            sanitizer_argv("thread").unwrap(),
            ["cargo", "test", "-Zsanitizer=thread"]
        );
        assert!(sanitizer_argv("thread --config x").is_err());
    }

    #[test]
    fn the_name_and_the_canary_are_the_executed_keys() {
        let mut gates = crate::config::Gates::default();
        let before = super::executed_keys(&gates);
        gates.sanitizers.enabled = true;
        gates.sanitizers.timeout_seconds = 5;
        assert_eq!(super::executed_keys(&gates), before);
        gates.sanitizers.canary = true;
        let with_canary = super::executed_keys(&gates);
        assert_ne!(with_canary, before);
        gates.sanitizers.sanitizer = "thread".to_string();
        assert_ne!(super::executed_keys(&gates), with_canary);
    }

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
