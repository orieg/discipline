//! Miri Undefined Behavior verification sentinel (`miri`).
//!
//! Executes `cargo miri test` with a zero-tests guard to detect undefined behavior
//! without allowing silent passes when test filters match zero items.

use crate::guards::command::{run_argv_bounded, split_command_line, ExecutedKeys};
use crate::guards::presets::resolve_preset;
use crate::guards::{toolchain_unavailable, Context, GateOutcome};
use crate::tokens::ALLOW_MIRI;
use anyhow::Result;

pub const GATE: &str = "miri";

pub fn evaluate_miri(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.miri;
    let mut out = GateOutcome::new(GATE);

    // `args` reach `cargo`: a change cannot supply them (`configured_execution`).
    let Some(execution_notes) = crate::guards::command::vouched_execution(
        ctx,
        &mut out,
        settings.severity,
        &crate::findings::MIRI_UNTRUSTED_COMMAND_MODIFICATION,
        &executed_keys,
    )?
    else {
        return Ok(out);
    };

    out.examined = 1;

    let preset = resolve_preset("miri").expect("miri preset is statically defined");
    let argv = miri_argv(preset.default_command, &settings.args)?;

    let timeout_secs = if settings.timeout_seconds > 0 {
        settings.timeout_seconds
    } else {
        preset.default_timeout_seconds
    };

    let root = ctx.git.root();

    let res = match run_argv_bounded("miri", &argv, timeout_secs, root) {
        Ok(r) => r,
        // A run that could not start verified nothing: exit 2 (could not check), which no
        // directive lifts. A job without the toolchain disables the gate instead.
        Err(e) => return Err(e.context("miri could not run")),
    };
    out.notes.extend(execution_notes);

    let stdout = &res.stdout;
    let stderr = &res.stderr;

    if !res.status.success() {
        if let Some(fault) = toolchain_unavailable(stdout, stderr) {
            // Fail-closed: the tool never ran, so this is "could not check"
            // (exit 2), never "undefined behavior detected" (exit 1). Checked before the
            // zero-tests guard: whatever its output holds, a tool that never ran is not a
            // run of zero tests.
            return Err(crate::could_not_check::tag(
                crate::could_not_check::Reason::ToolchainUnavailable,
                anyhow::anyhow!(
                    "miri could not run: {fault}. Install the component \
                     (`rustup +nightly component add miri`) or disable the `miri` gate."
                ),
            ));
        }
    }

    // Check zero-tests guard: every `running N tests` count, the doc-tests' among them,
    // is read together (#883).
    if let Some(zero_pat) = preset.zero_items_pattern {
        if zero_tests_shown(zero_pat, stdout, stderr)? {
            if let Some(ov) = ctx
                .find_override(
                    GATE,
                    &crate::findings::MIRI_ZERO_TESTS_EXECUTED,
                    ALLOW_MIRI,
                    "zero-tests",
                )
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::MIRI_ZERO_TESTS_EXECUTED,
                        ALLOW_MIRI,
                        "tests",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::MIRI_ZERO_TESTS_EXECUTED,
                        ALLOW_MIRI,
                        "miri",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::MIRI_ZERO_TESTS_EXECUTED,
                        ALLOW_MIRI,
                        "cargo-miri",
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::MIRI_ZERO_TESTS_EXECUTED,
                        ALLOW_MIRI,
                        "toolchain",
                    )
                })
            {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` (miri 0 tests allowed) ({})",
                    ov.directive, ov.reason, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::MIRI_ZERO_TESTS_EXECUTED,
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
        if let Some(ov) = ctx
            .find_override(
                GATE,
                &crate::findings::MIRI_UNDEFINED_BEHAVIOR,
                ALLOW_MIRI,
                "failure",
            )
            .or_else(|| {
                ctx.find_override(
                    GATE,
                    &crate::findings::MIRI_UNDEFINED_BEHAVIOR,
                    ALLOW_MIRI,
                    "miri",
                )
            })
            .or_else(|| {
                ctx.find_override(
                    GATE,
                    &crate::findings::MIRI_UNDEFINED_BEHAVIOR,
                    ALLOW_MIRI,
                    "cargo-miri",
                )
            })
            .or_else(|| {
                ctx.find_override(
                    GATE,
                    &crate::findings::MIRI_UNDEFINED_BEHAVIOR,
                    ALLOW_MIRI,
                    "toolchain",
                )
            })
        {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` (miri failure allowed) ({})",
                ov.directive, ov.reason, ov.source
            ));
        } else {
            let diag = if !stderr.is_empty() { stderr } else { stdout };
            out.add_violation(
                ctx.overridable(settings.severity),
                &crate::findings::MIRI_UNDEFINED_BEHAVIOR,
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

/// The keys of `[gates.miri]` whose value reaches a process invocation: `args`, each
/// item one argument after the built-in command.
pub(crate) fn executed_keys(gates: &crate::config::Gates) -> ExecutedKeys {
    vec![("args", format!("{:?}", gates.miri.args))]
}

/// The configuration key of the arguments, as a message names it.
pub const ARGS_KEY: &str = "gates.miri.args";

/// An item of `args` is one argument of `cargo miri test`: ASCII letters, digits and
/// `_ . / : = , @ + -`, and not empty. No whitespace, quote or shell metacharacter: an
/// item is passed as written, as one argument, so text meant as two arguments is two
/// items. A value of another shape is a configuration error naming the key.
pub fn check_arg(arg: &str) -> Result<()> {
    let allowed = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '.' | '/' | ':' | '=' | ',' | '@' | '+' | '-')
    };
    if !arg.is_empty() && arg.chars().all(allowed) {
        return Ok(());
    }
    Err(crate::could_not_check::tag(
        crate::could_not_check::Reason::Configuration,
        anyhow::anyhow!(
            "`{ARGS_KEY}` item {arg:?} is not one argument of `cargo miri test`: an item is ASCII letters, digits and `_ . / : = , @ + -`, not empty, and is passed as one argument; write text meant as two arguments as two items"
        ),
    ))
}

/// The built-in command followed by `args`, one element per item: an item is never
/// split again, whatever it holds.
pub(crate) fn miri_argv(built_in: &str, args: &[String]) -> Result<Vec<String>> {
    let mut argv = split_command_line(built_in)?;
    for arg in args {
        check_arg(arg)?;
        argv.push(arg.clone());
    }
    Ok(argv)
}

/// Whether the output of `cargo miri test` shows that no test ran, by the preset's
/// zero-tests guard, read over stdout and stderr together: libtest prints its counts on
/// stdout and cargo its `Running` lines on stderr.
fn zero_tests_shown(zero_pat: &str, stdout: &str, stderr: &str) -> Result<bool> {
    crate::guards::command::zero_items_shown(
        zero_pat,
        "miri zero-tests guard",
        &format!("{stdout}\n{stderr}"),
    )
}

/// What the gate reports for a run, in the order it checks: a toolchain that never ran,
/// a run that executed no test, a failed run.
pub fn evaluate_miri_output(
    status_success: bool,
    stdout: &str,
    stderr: &str,
    zero_pat: Option<&str>,
) -> Result<Option<&'static str>> {
    // An environment fault is not a finding: see guards::toolchain_unavailable.
    if !status_success && toolchain_unavailable(stdout, stderr).is_some() {
        return Ok(Some("toolchain"));
    }
    if let Some(zero_pat) = zero_pat {
        if zero_tests_shown(zero_pat, stdout, stderr)? {
            return Ok(Some("zero-tests"));
        }
    }
    Ok((!status_success).then_some("failure"))
}

#[cfg(test)]
mod tests {
    use crate::config::{MiriGate, Severity};

    #[test]
    fn an_arg_is_one_argument_of_one_shape() {
        use super::{check_arg, miri_argv};
        for ok in [
            "--lib",
            "-p",
            "core_crate",
            "--features=a,b",
            "tests/it.rs",
            "a::b",
            "+nightly",
            "x@1.2",
        ] {
            assert!(check_arg(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "--lib --tests",
            "a\tb",
            "a;b",
            "$(x)",
            "`x`",
            "a|b",
            "a&b",
            "'a'",
            "\"a\"",
            "a>b",
            "a*",
            "a\nb",
            "caf\u{e9}",
        ] {
            let e = check_arg(bad).unwrap_err().to_string();
            assert!(e.contains(super::ARGS_KEY), "{bad}: {e}");
        }
        let args = vec!["--package".to_string(), "core_crate".to_string()];
        assert_eq!(
            miri_argv("cargo miri test", &args).unwrap(),
            ["cargo", "miri", "test", "--package", "core_crate"]
        );
        assert!(miri_argv("cargo miri test", &["--lib --tests".to_string()]).is_err());
    }

    #[test]
    fn only_args_is_an_executed_key() {
        let mut gates = crate::config::Gates::default();
        let before = super::executed_keys(&gates);
        gates.miri.enabled = true;
        gates.miri.timeout_seconds = 5;
        assert_eq!(super::executed_keys(&gates), before);
        gates.miri.args = vec!["--lib".to_string()];
        assert_ne!(super::executed_keys(&gates), before);
    }

    #[test]
    fn test_miri_defaults() {
        let gate = MiriGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(gate.timeout_seconds, 600);
    }

    #[test]
    fn miri_classifies_missing_toolchain_as_could_not_check_not_as_ub() {
        use super::evaluate_miri_output;
        // The tool never ran: rustup has no miri component for this toolchain.
        let env_fault = "error: the 'miri' component which provides the command 'cargo-miri' is not available for the 'stable-aarch64-apple-darwin' toolchain";
        assert_eq!(
            evaluate_miri_output(false, "", env_fault, None).unwrap(),
            Some("toolchain"),
            "a missing toolchain must not be reported as detected undefined behavior"
        );
        // Nor as zero tests, whatever it printed.
        let guard = Some(crate::guards::presets::LIBTEST_ZERO_TESTS);
        assert_eq!(
            evaluate_miri_output(false, "", env_fault, guard).unwrap(),
            Some("toolchain")
        );

        // The tool RAN and found real UB: still a failure.
        let real_ub =
            "error: Undefined Behavior: attempting a read access using <untagged> at alloc1[0x0]";
        assert_eq!(
            evaluate_miri_output(false, "", real_ub, None).unwrap(),
            Some("failure")
        );
        assert_eq!(
            evaluate_miri_output(false, "running 3 tests\ntest t ... ", real_ub, guard).unwrap(),
            Some("failure")
        );
        // A build that failed printed no test count: a failure, not zero tests.
        let build = "error[E0425]: cannot find value `x` in this scope";
        assert_eq!(
            evaluate_miri_output(false, "", build, guard).unwrap(),
            Some("failure")
        );
    }

    /// `cargo miri test` runs the doc-tests too: an empty `Doc-tests` section after unit
    /// tests ran is not a run of zero tests (#883). Every section empty still is; output
    /// with no test count is not read as zero tests.
    #[test]
    fn miri_zero_tests_reads_the_count_of_every_section() {
        use super::evaluate_miri_output;
        let guard = Some(crate::guards::presets::LIBTEST_ZERO_TESTS);
        let unit_ran = "running 2 tests\ntest t1 ... ok\ntest t2 ... ok\n\n\
            test result: ok. 2 passed; 0 failed\n";
        let doc_empty = "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed\n";
        let cargo = "     Running unittests src/lib.rs\n   Doc-tests p\n";
        let ran = format!("{unit_ran}{doc_empty}");
        assert_eq!(
            evaluate_miri_output(true, &ran, cargo, guard).unwrap(),
            None
        );
        // Control: every section empty.
        let none_ran = format!("{}{doc_empty}", unit_ran.replace("running 2", "running 0"));
        assert_eq!(
            evaluate_miri_output(true, &none_ran, cargo, guard).unwrap(),
            Some("zero-tests")
        );
        assert_eq!(evaluate_miri_output(true, "", cargo, guard).unwrap(), None);
        // A count that ends in `0` is not zero.
        assert_eq!(
            evaluate_miri_output(true, "running 10 tests\n", "", guard).unwrap(),
            None
        );
    }
}
