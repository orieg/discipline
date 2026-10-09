//! Universal test count floor ratchet sentinel (`test-floor`).
//!
//! Enforces:
//! - Workspace test count does not drop below a pinned floor constant or base ref count.
//! - Floor constant in base ref cannot be lowered without an `allow-test-shrink:` directive.
//! - Required test suite files must exist on disk unless explicitly excused.
//! - Fails closed if the base floor cannot be resolved when configured.

use crate::escape::unescape_xml;
use crate::guards::{exempt_filter, Context, GateOutcome, Violation};
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use regex::Regex;
use std::path::{Path, PathBuf};

pub const GATE: &str = "test-floor";

/// Evaluates test count and floor invariants.
pub fn evaluate_test_floor(ctx: &Context) -> Result<GateOutcome> {
    let mut out = GateOutcome::new(GATE);
    let settings = &ctx.config.gates.test_floor;

    let filter = exempt_filter(settings)?;
    let sides = SideVocabularies::new(ctx);

    let base_cfg = base_configuration(ctx, &mut out)?;
    let base_min_tests = base_cfg.as_ref().and_then(|c| c.gates.test_floor.min_tests);
    let untrusted_test_command = test_command_is_untrusted(ctx, base_cfg.as_ref());
    let head_min_tests = settings.min_tests;

    // 1. Resolve base floor constant from constant_file if configured.
    let BaseFloorConstant::Read(base_floor_const) = base_floor_constant(ctx, &mut out)? else {
        return Ok(out);
    };

    // 2. Check if head constant is lower than base constant.
    check_floor_constant_not_lowered(ctx, base_floor_const, &mut out)?;

    // 3. Check min_tests comparison against base discipline.toml.
    check_configured_floor_not_lowered(ctx, &sides, base_min_tests, &mut out)?;

    // 4. Required test suites check.
    check_required_suites(ctx, &mut out);

    // Base ref discipline.toml takes precedence over HEAD discipline.toml to prevent self-lowering.
    let explicit_floor = base_min_tests.or(head_min_tests).or(base_floor_const);

    // The zero-config ratchet counts the base ref statically; it cannot build
    // and run the base ref's tests. Comparing that against a runtime count
    // mixes two counting bases (see docs/GATES.md, test-floor), so the result
    // would be meaningless in either direction. Refuse before running anything.
    if settings.test_command.is_some() && explicit_floor.is_none() && !untrusted_test_command {
        bail!(
            "test-floor: `test_command` supplies a runtime test count, but no floor is configured to \
             compare it against; set `min_tests` (or `constant_file` + `constant_name`) to a count on \
             the same basis, or remove `test_command` to use the static ratchet"
        );
    }

    // 5. Check untrusted test_command before running anything.
    // The test files the change moves out of the default run. This reads the runner
    // rules of both sides, whatever the counting basis.
    if untrusted_test_command {
        let moved = tests_moved_out(ctx, &filter, &sides)?;
        out.push(
            settings.severity,
            &crate::findings::UNTRUSTED_TEST_COMMAND,
            Some(ctx.config_path),
            None,
            if ctx.git.has_base() {
                "The change adds or alters `test_command` in `[gates.test-floor]` without runner environment authorization; a command cannot be introduced or altered by the change it judges, so it was not run and the test count was not taken."
            } else {
                "This CI run has no base ref to compare `test_command` in `[gates.test-floor]` with, and no runner environment authorization; a command cannot be introduced by the change it judges, so it was not run and the test count was not taken."
            }
            .to_string(),
            "Configure `test_command` in the merge base ref's discipline.toml, or set DISCIPLINE_ALLOW_COMMAND_CHANGE on the runner to accept the change.",
        );
        report_tests_moved_out(ctx, &mut out, &moved, false);
        return Ok(out);
    }

    // 6. Run trusted test_command if configured before reading the report it produces.
    let cmd_count = if let Some(cmd) = &settings.test_command {
        Some(count_tests_via_command(cmd, Path::new(ctx.git.root()))?)
    } else {
        None
    };

    // 7. Resolve test reports for identity-based ratcheting
    let reports = resolve_test_reports(ctx, &mut out)?;

    // 8. Test Identity Ratchet
    check_test_identities(ctx, &reports, &mut out);

    // The test files the change moves out of the default run. This reads the runner
    // rules of both sides, whatever the counting basis.
    let moved = tests_moved_out(ctx, &filter, &sides)?;

    // 9. Calculate measured test count.
    let measured = measure_test_count(
        ctx,
        &filter,
        &sides,
        cmd_count,
        reports.head.as_deref(),
        &mut out,
    )?;
    out.examined = measured.count;

    // 10. Compare against the effective floor.
    let before_count = out.violations.len();
    compare_with_floor(
        ctx,
        &filter,
        &sides,
        explicit_floor,
        &reports,
        &measured,
        &mut out,
    )?;

    // 11. Tests the change moves out of the default run. A count finding this run
    // reports already covers the files that left the count with them. One that a
    // directive lifted does not: the directive named the count, not the rule.
    let count_reported = before_count != out.violations.len();
    report_tests_moved_out(ctx, &mut out, &moved, count_reported);

    Ok(out)
}

/// The base side's configuration. One that does not load with this binary is noted and
/// read as absent.
fn base_configuration(
    ctx: &Context,
    out: &mut GateOutcome,
) -> Result<Option<crate::config::DisciplineConfig>> {
    // Read base discipline.toml to get base configuration
    Ok(match ctx.base_config_text()? {
        None => None,
        Some(s) => match crate::config::DisciplineConfig::from_toml_str(&s) {
            Ok(cfg) => Some(cfg),
            Err(_) => {
                out.notes.push(
                    "the base-side configuration does not load with this binary; the base `min_tests` ratchet was not checked"
                        .to_string(),
                );
                None
            }
        },
    })
}

/// Whether `test_command` is one the change supplies and the runner does not authorise.
fn test_command_is_untrusted(
    ctx: &Context,
    base_cfg: Option<&crate::config::DisciplineConfig>,
) -> bool {
    let settings = &ctx.config.gates.test_floor;
    // `test_command` is executed. Under the default policy side the configuration in
    // force is the change's own copy, so a command the base side does not have is one the
    // change supplies. Under `--policy-from base` the copy in force is the base's and the
    // two are equal. A base configuration that is absent, or does not load, vouches for
    // no command, as in the `command` gate.
    // With no base at all (`--staged` before the first commit) nothing vouches for it
    // either: on a CI runner it is not run, on a developer's machine it is their own.
    let unvouched = if ctx.git.has_base() {
        test_command_supplied_by_change(
            settings.test_command.as_deref(),
            base_cfg.and_then(|c| c.gates.test_floor.test_command.as_deref()),
        )
    } else {
        settings.test_command.is_some() && crate::gitctx::is_ci_environment()
    };
    unvouched && !crate::guards::command::runner_authorises_command_change()
}

/// What reading the floor constant on the base side gave.
enum BaseFloorConstant {
    /// The value, or `None` when no constant is configured or a directive lifted its
    /// absence.
    Read(Option<usize>),
    /// The constant or its file is not on the base side and the finding is reported:
    /// nothing else is checked.
    Missing,
}

/// The floor constant of `constant_file` on the base side, when one is configured.
fn base_floor_constant(ctx: &Context, out: &mut GateOutcome) -> Result<BaseFloorConstant> {
    let settings = &ctx.config.gates.test_floor;
    let mut base_floor_const: Option<usize> = None;
    if let (Some(const_file), Some(const_name)) = (&settings.constant_file, &settings.constant_name)
    {
        match ctx.git.base_content(const_file) {
            Ok(Some(base_src)) => {
                let pat = format!(
                    r"(?m)^[ \t]*(?:(?:pub|export)\s+)?(?:const\s+)?{}(?:\s*:\s*[a-zA-Z0-9_]+)?\s*=\s*(\d+)",
                    regex::escape(const_name)
                );
                let re = Regex::new(&pat)?;
                if let Some(caps) = re.captures(&base_src) {
                    let val: usize = caps[1].parse().with_context(|| {
                        format!("Failed to parse integer from '{const_name}' in base ref")
                    })?;
                    base_floor_const = Some(val);
                } else {
                    let subject = const_name.as_str();
                    if let Some(ov) = ctx.find_override(
                        GATE,
                        &crate::findings::FLOOR_CONSTANT_MISSING_IN_BASE,
                        tokens::ALLOW_TEST_SHRINK,
                        subject,
                    ) {
                        out.overrides.push(ov);
                    } else {
                        out.violations.push(Violation {
                            gate: GATE,
                            severity: ctx.overridable(settings.severity),
                            code: crate::findings::full_code(GATE, &crate::findings::FLOOR_CONSTANT_MISSING_IN_BASE),
                            fingerprint: String::new(),
                            title: crate::findings::FLOOR_CONSTANT_MISSING_IN_BASE.title.to_string(),
                            anchor: None,
                            legacy_title: crate::findings::FLOOR_CONSTANT_MISSING_IN_BASE.was_title(),
                            file: Some(const_file.clone()),
                            line: None,
                            message: format!(
                                "Floor constant '{const_name}' not found in '{const_file}' on base ref."
                            ),
                            remediation: Some(
                                "Ensure the constant is defined on the base branch or provide an allow-test-shrink directive."
                                    .to_string(),
                            ),
                        });
                        return Ok(BaseFloorConstant::Missing);
                    }
                }
            }
            Ok(None) => {
                if let Some(ov) = ctx.find_override(
                    GATE,
                    &crate::findings::FLOOR_CONSTANT_FILE_MISSING_IN_BASE,
                    tokens::ALLOW_TEST_SHRINK,
                    const_file,
                ) {
                    out.overrides.push(ov);
                } else {
                    out.violations.push(Violation {
                        gate: GATE,
                        severity: ctx.overridable(settings.severity),
                        code: crate::findings::full_code(GATE, &crate::findings::FLOOR_CONSTANT_FILE_MISSING_IN_BASE),
                        fingerprint: String::new(),
                        title: crate::findings::FLOOR_CONSTANT_FILE_MISSING_IN_BASE.title.to_string(),
                        anchor: None,
                        legacy_title: crate::findings::FLOOR_CONSTANT_FILE_MISSING_IN_BASE.was_title(),
                        file: Some(const_file.clone()),
                        line: None,
                        message: format!("Base ref does not contain floor constant file '{const_file}'."),
                        remediation: Some(
                            "Ensure the file exists on the base branch or provide an allow-test-shrink directive."
                                .to_string(),
                        ),
                    });
                    return Ok(BaseFloorConstant::Missing);
                }
            }
            Err(e) => {
                bail!("Failed to read '{const_file}' from base ref: {e}");
            }
        }
    }
    Ok(BaseFloorConstant::Read(base_floor_const))
}

/// Reports a floor constant the head side lowered below the base side's or no longer
/// defines.
fn check_floor_constant_not_lowered(
    ctx: &Context,
    base_floor_const: Option<usize>,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.test_floor;
    if let (Some(const_file), Some(const_name), Some(base_floor)) = (
        &settings.constant_file,
        &settings.constant_name,
        base_floor_const,
    ) {
        // A constant the head side no longer defines (the file is gone or is not text, the
        // name is gone, the value is not a number) lifts the floor altogether: the same
        // finding as a lowered one, never a skipped comparison.
        let pat = format!(
            r"(?m)^[ \t]*(?:(?:pub|export)\s+)?(?:const\s+)?{}(?:\s*:\s*[a-zA-Z0-9_]+)?\s*=\s*(\d+)",
            regex::escape(const_name)
        );
        let re = Regex::new(&pat)?;
        let head_val: Option<usize> = ctx.git.head_content(const_file)?.and_then(|src| {
            re.captures(&src)
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse().ok())
        });
        if head_val.is_none_or(|v| v < base_floor) {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::FLOOR_CONSTANT_DECREASED,
                tokens::ALLOW_TEST_SHRINK,
                const_name,
            ) {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::FLOOR_CONSTANT_DECREASED),
                    fingerprint: String::new(),
                    title: crate::findings::FLOOR_CONSTANT_DECREASED.title.to_string(),
                    anchor: None,
                    legacy_title: crate::findings::FLOOR_CONSTANT_DECREASED.was_title(),
                    file: Some(const_file.clone()),
                    line: None,
                    message: match head_val {
                        Some(head_val) => format!(
                            "Floor constant '{const_name}' ({head_val}) was decreased below base ref ({base_floor})."
                        ),
                        None => format!(
                            "Floor constant '{const_name}' ({base_floor} on the base ref) is no longer defined as a number in '{const_file}'."
                        ),
                    },
                    remediation: Some(
                        "Restore the floor constant or provide an allow-test-shrink: <reason> directive in the PR description."
                            .to_string(),
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Reports `min_tests` lowered below, or removed from, the base side's configuration.
fn check_configured_floor_not_lowered(
    ctx: &Context,
    sides: &SideVocabularies,
    base_min_tests: Option<usize>,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.test_floor;
    let head_min_tests = settings.min_tests;
    if let Some(base_min) = base_min_tests {
        let lowered = match head_min_tests {
            Some(h) => h < base_min,
            None => true,
        };
        if lowered {
            if let Some(ov) =
                find_test_floor_override(ctx, sides, &crate::findings::CONFIGURED_FLOOR_DECREASED)?
            {
                out.overrides.push(ov);
            } else {
                let msg = match head_min_tests {
                    Some(h) => format!("Test count floor (min_tests = {h}) was lowered below base ref ({base_min})."),
                    None => format!("Test count floor (min_tests = {base_min}) was removed from discipline.toml."),
                };
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::CONFIGURED_FLOOR_DECREASED),
                    fingerprint: String::new(),
                    title: crate::findings::CONFIGURED_FLOOR_DECREASED.title.to_string(),
                    anchor: None,
                    legacy_title: crate::findings::CONFIGURED_FLOOR_DECREASED.was_title(),
                    file: Some(ctx.config_path.to_string()),
                    line: None,
                    message: msg,
                    remediation: Some(
                        "Restore min_tests or provide an allow-test-shrink: <reason> directive in the PR description."
                            .to_string(),
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Reports each required test suite file that is missing from the repository.
fn check_required_suites(ctx: &Context, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.test_floor;
    for suite in &settings.required_suites {
        let full = Path::new(ctx.git.root()).join(suite);
        if !full.is_file() {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::REQUIRED_SUITE_MISSING,
                tokens::ALLOW_TEST_SHRINK,
                suite,
            ) {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::REQUIRED_SUITE_MISSING),
                    fingerprint: String::new(),
                    title: crate::findings::REQUIRED_SUITE_MISSING.title.to_string(),
                    anchor: None,
                    legacy_title: crate::findings::REQUIRED_SUITE_MISSING.was_title(),
                    file: Some(suite.clone()),
                    line: None,
                    message: format!("Required test suite file '{suite}' is missing from the repository."),
                    remediation: Some(
                        "Restore the required test suite or provide an allow-test-shrink: <reason> directive in the PR description."
                            .to_string(),
                    ),
                });
            }
        }
    }
}

/// The test cases of the head and base test reports, for each side that has one.
struct TestReports {
    head: Option<Vec<TestCaseReport>>,
    base: Option<Vec<TestCaseReport>>,
}

/// Locates and parses the head and base test reports the identity ratchet compares.
fn resolve_test_reports(ctx: &Context, out: &mut GateOutcome) -> Result<TestReports> {
    let settings = &ctx.config.gates.test_floor;
    let env_head = std::env::var("DISCIPLINE_TEST_HEAD_REPORT")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_report = std::env::var("DISCIPLINE_TEST_REPORT")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let head_report_path: Option<&str> = ctx
        .test_head_report
        .as_deref()
        .and_then(|p| p.to_str())
        .filter(|s| !s.trim().is_empty())
        .or(env_head.as_deref())
        .or(settings.head_report.as_deref())
        .or_else(|| {
            ctx.test_report
                .as_deref()
                .and_then(|p| p.to_str())
                .filter(|s| !s.trim().is_empty())
        })
        .or(env_report.as_deref())
        .or(settings.test_report.as_deref());

    let env_base = std::env::var("DISCIPLINE_TEST_BASE_REPORT")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let base_report_path: Option<&str> = ctx
        .test_base_report
        .as_deref()
        .and_then(|p| p.to_str())
        .filter(|s| !s.trim().is_empty())
        .or(env_base.as_deref())
        .or(settings.base_report.as_deref());

    if base_report_path.is_some() && head_report_path.is_none() {
        bail!("test-floor: `base_report` was configured or provided, but `head_report` is missing");
    }

    let mut head_cases_opt: Option<Vec<TestCaseReport>> = None;
    let mut base_cases_opt: Option<Vec<TestCaseReport>> = None;

    if let Some(h_path) = head_report_path {
        let full_head = if Path::new(h_path).is_absolute() {
            PathBuf::from(h_path)
        } else {
            Path::new(ctx.git.root()).join(h_path)
        };
        if !full_head.is_file() {
            bail!("test-floor: head test report file '{h_path}' not found");
        }
        let head_xml = std::fs::read_to_string(&full_head)
            .with_context(|| format!("failed to read head test report '{h_path}'"))?;
        let parsed_head = parse_junit_xml(&head_xml)
            .with_context(|| format!("failed to parse head test report '{h_path}'"))?;
        head_cases_opt = Some(parsed_head);

        // Load base report
        if let Some(b_path) = base_report_path {
            let full_base = if Path::new(b_path).is_absolute() {
                PathBuf::from(b_path)
            } else {
                Path::new(ctx.git.root()).join(b_path)
            };
            if !full_base.is_file() {
                bail!("test-floor: base test report file '{b_path}' not found");
            }
            let base_xml = std::fs::read_to_string(&full_base)
                .with_context(|| format!("failed to read base test report '{b_path}'"))?;
            let parsed_base = parse_junit_xml(&base_xml)
                .with_context(|| format!("failed to parse base test report '{b_path}'"))?;
            base_cases_opt = Some(parsed_base);
        } else if let Some(configured_report) = settings.test_report.as_deref().or_else(|| {
            ctx.test_report
                .as_deref()
                .and_then(|p| p.to_str())
                .or(env_report.as_deref())
        }) {
            if !Path::new(configured_report).is_absolute() {
                match ctx.git.base_content(configured_report) {
                    Ok(Some(base_xml)) => {
                        let parsed_base = parse_junit_xml(&base_xml).with_context(|| {
                            format!("failed to parse base ref test report '{configured_report}'")
                        })?;
                        base_cases_opt = Some(parsed_base);
                    }
                    Ok(None) => {
                        out.notes.push(format!(
                            "test-floor: test report '{configured_report}' not present in base ref; identity ratchet will be enforced on subsequent changes"
                        ));
                    }
                    Err(e) => {
                        bail!("Failed to read '{configured_report}' from base ref: {e}");
                    }
                }
            }
        }
    }

    Ok(TestReports {
        head: head_cases_opt,
        base: base_cases_opt,
    })
}

/// Reports each test that passed in the base report and is missing, skipped or failed in
/// the head report.
fn check_test_identities(ctx: &Context, reports: &TestReports, out: &mut GateOutcome) {
    let settings = &ctx.config.gates.test_floor;
    if let (Some(base_cases), Some(head_cases)) = (&reports.base, &reports.head) {
        let identity_violations = compare_test_identities(base_cases, head_cases);
        let base_passed_count = base_cases
            .iter()
            .filter(|c| c.status == TestStatus::Passed)
            .count();
        out.notes.push(format!(
            "test-floor: verified {base_passed_count} passed base test identity/identities against head report"
        ));

        for viol in identity_violations {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::TEST_IDENTITY_DROPPED,
                tokens::ALLOW_GATE_WEAKENING,
                GATE,
            ) {
                out.overrides.push(ov);
                continue;
            }

            let mut candidate_subjects = vec![viol.id.as_str(), viol.name.as_str()];
            let dot_fmt;
            let colon_fmt;
            if let Some(cn) = &viol.classname {
                dot_fmt = format!("{cn}.{}", viol.name);
                candidate_subjects.push(&dot_fmt);
                colon_fmt = format!("{cn}::{}", viol.name);
                if colon_fmt != viol.id {
                    candidate_subjects.push(&colon_fmt);
                }
            }

            let mut found_override = None;
            for subj in candidate_subjects {
                if let Some(ov) = ctx.find_override(
                    GATE,
                    &crate::findings::TEST_IDENTITY_DROPPED,
                    tokens::ALLOW_TEST_SHRINK,
                    subj,
                ) {
                    found_override = Some(ov);
                    break;
                }
                if let Some(ov) = ctx.find_override(
                    GATE,
                    &crate::findings::TEST_IDENTITY_DROPPED,
                    tokens::REMOVES,
                    subj,
                ) {
                    found_override = Some(ov);
                    break;
                }
            }

            if let Some(ov) = found_override {
                out.overrides.push(ov);
            } else {
                let (action_msg, remediation_verb) = match viol.issue {
                    TestIdentityIssue::Missing => {
                        ("is missing from head test report", "Restore the test")
                    }
                    TestIdentityIssue::Skipped => {
                        ("is skipped in head test report", "Re-enable the test")
                    }
                    TestIdentityIssue::Failed => {
                        ("failed in head test report", "Fix the test failure")
                    }
                };
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::TEST_IDENTITY_DROPPED),
                    fingerprint: String::new(),
                    title: crate::findings::TEST_IDENTITY_DROPPED.title.to_string(),
                    // No file and no line: the test's identity tells it from another.
                    anchor: Some(format!("test:{}", viol.id)),
                    legacy_title: crate::findings::TEST_IDENTITY_DROPPED.was_title(),
                    file: None,
                    line: None,
                    message: format!("Test '{}' passed on base ref but {action_msg}.", viol.id),
                    remediation: Some(format!(
                        "{remediation_verb} or provide an allow-test-shrink: <test-id> <reason> directive in the PR description."
                    )),
                });
            }
        }
    }
}

/// The measured test count, with the head side's static count when that is its basis.
struct MeasuredCount {
    count: usize,
    head_static: Option<AstTestCount>,
}

/// Measures the test count: from the head report, else `test_command`, else a static
/// count of the head side.
fn measure_test_count(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    sides: &SideVocabularies,
    cmd_count: Option<usize>,
    head_cases: Option<&[TestCaseReport]>,
    out: &mut GateOutcome,
) -> Result<MeasuredCount> {
    let mut head_static: Option<AstTestCount> = None;
    let measured_count = if let Some(head_cases) = head_cases {
        head_cases
            .iter()
            .filter(|c| c.status == TestStatus::Passed)
            .count()
    } else if let Some(count) = cmd_count {
        count
    } else {
        let head = count_workspace_ast_tests(ctx, filter, sides.head()?)?;
        for note in head.notes("head") {
            if !out.notes.contains(&note) {
                out.notes.push(note);
            }
        }
        let running = head.running;
        head_static = Some(head);
        running
    };
    Ok(MeasuredCount {
        count: measured_count,
        head_static,
    })
}

/// Compares the measured count with the effective floor: the configured one, else the
/// base report's count, else a static count of the base side.
fn compare_with_floor(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    sides: &SideVocabularies,
    explicit_floor: Option<usize>,
    reports: &TestReports,
    measured: &MeasuredCount,
    out: &mut GateOutcome,
) -> Result<()> {
    if let Some(floor) = explicit_floor {
        compare_with_configured_floor(ctx, sides, floor, measured.count, out)
    } else if let Some(base_cases) = reports.base.as_deref() {
        compare_with_base_report(
            ctx,
            sides,
            base_cases,
            reports.head.as_deref(),
            measured.count,
            out,
        )
    } else if measured.head_static.is_none() {
        bail!(
            "test-floor: a runtime test report supplies the test count, but no base report or floor is configured to \
             compare it against; provide `base_report`, set `min_tests` (or `constant_file` + `constant_name`), \
             or remove the report to use the static ratchet"
        );
    } else {
        compare_with_base_static_count(ctx, filter, sides, measured, out)
    }
}

/// Reports a measured count below the configured floor.
fn compare_with_configured_floor(
    ctx: &Context,
    sides: &SideVocabularies,
    floor: usize,
    measured_count: usize,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.test_floor;
    if measured_count + settings.tolerance < floor {
        if let Some(ov) =
            find_test_floor_override(ctx, sides, &crate::findings::TEST_COUNT_BELOW_FLOOR)?
        {
            out.overrides.push(ov);
        } else {
            out.violations.push(Violation {
                gate: GATE,
                severity: ctx.overridable(settings.severity),
                code: crate::findings::full_code(GATE, &crate::findings::TEST_COUNT_BELOW_FLOOR),
                fingerprint: String::new(),
                title: crate::findings::TEST_COUNT_BELOW_FLOOR.title.to_string(),
                anchor: None,
                legacy_title: crate::findings::TEST_COUNT_BELOW_FLOOR.was_title(),
                file: None,
                line: None,
                message: format!(
                    "Workspace test count ({measured_count}) is below the required floor of {floor}."
                ),
                remediation: Some(
                    "Restore deleted tests or provide an allow-test-shrink: <reason> directive in the PR description."
                        .to_string(),
                ),
            });
        }
    }
    Ok(())
}

/// Reports a measured count below the number of cases in the base report.
fn compare_with_base_report(
    ctx: &Context,
    sides: &SideVocabularies,
    base_cases: &[TestCaseReport],
    head_cases: Option<&[TestCaseReport]>,
    measured_count: usize,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.test_floor;
    let base_count = base_cases
        .iter()
        .filter(|c| c.status == TestStatus::Passed)
        .count();
    if base_count > 0 && measured_count + settings.tolerance < base_count {
        if let Some(ov) =
            find_test_floor_override(ctx, sides, &crate::findings::TEST_COUNT_BELOW_FLOOR)?
        {
            out.overrides.push(ov);
            return Ok(());
        }

        // Check if any test that passed in base but not in head was excused
        if let Some(head) = head_cases {
            let passed_head_ids: std::collections::HashSet<&str> = head
                .iter()
                .filter(|c| c.status == TestStatus::Passed)
                .map(|c| c.id.as_str())
                .collect();
            for base_case in base_cases.iter().filter(|c| c.status == TestStatus::Passed) {
                if !passed_head_ids.contains(base_case.id.as_str()) {
                    let mut candidate_subjects =
                        vec![base_case.id.as_str(), base_case.name.as_str()];
                    let dot_fmt;
                    let colon_fmt;
                    if let Some(cn) = &base_case.classname {
                        dot_fmt = format!("{cn}.{}", base_case.name);
                        candidate_subjects.push(&dot_fmt);
                        colon_fmt = format!("{cn}::{}", base_case.name);
                        if colon_fmt != base_case.id {
                            candidate_subjects.push(&colon_fmt);
                        }
                    }
                    for subj in candidate_subjects {
                        if let Some(ov) = ctx.find_override(
                            GATE,
                            &crate::findings::TEST_COUNT_BELOW_FLOOR,
                            tokens::ALLOW_TEST_SHRINK,
                            subj,
                        ) {
                            out.overrides.push(ov);
                            return Ok(());
                        }
                        if let Some(ov) = ctx.find_override(
                            GATE,
                            &crate::findings::TEST_COUNT_BELOW_FLOOR,
                            tokens::REMOVES,
                            subj,
                        ) {
                            out.overrides.push(ov);
                            return Ok(());
                        }
                    }
                }
            }
        }
        out.violations.push(Violation {
                gate: GATE,
                severity: ctx.overridable(settings.severity),
                code: crate::findings::full_code(GATE, &crate::findings::TEST_COUNT_BELOW_FLOOR),
                fingerprint: String::new(),
                title: crate::findings::TEST_COUNT_BELOW_FLOOR.title.to_string(),
                anchor: None,
                legacy_title: crate::findings::TEST_COUNT_BELOW_FLOOR.was_title(),
                file: None,
                line: None,
                message: format!(
                    "Workspace test count ({measured_count}) dropped below base ref count ({base_count}) [tolerance: {}].",
                    settings.tolerance
                ),
                remediation: Some(
                    "Restore deleted tests or provide an allow-test-shrink: <reason> directive in the PR description."
                        .to_string(),
                ),
            });
    }
    Ok(())
}

/// Reports a measured count below a static count of the base side.
fn compare_with_base_static_count(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    sides: &SideVocabularies,
    measured: &MeasuredCount,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.test_floor;
    let measured_count = measured.count;
    // Zero-config ratchet: compare head AST test count against base ref AST test count.
    let base = count_base_workspace_ast_tests(ctx, filter, sides.base()?)?;
    for note in base.notes("base") {
        if !out.notes.contains(&note) {
            out.notes.push(note);
        }
    }
    if let Some(head) = &measured.head_static {
        // A renamed file is the base side's file under its old path.
        let renamed: std::collections::BTreeMap<String, String> = ctx
            .git
            .changed_files()?
            .into_iter()
            .filter(|cf| cf.old_path != cf.path && !cf.is_deleted())
            .map(|cf| (cf.path, cf.old_path))
            .collect();
        out.notes.extend(head.transition_notes(&base, &renamed));
    }
    let base_count = base.running;
    if base_count > 0 && measured_count + settings.tolerance < base_count {
        if let Some(ov) =
            find_test_floor_override(ctx, sides, &crate::findings::TEST_COUNT_BELOW_FLOOR)?
        {
            out.overrides.push(ov);
        } else {
            out.violations.push(Violation {
                gate: GATE,
                severity: ctx.overridable(settings.severity),
                code: crate::findings::full_code(GATE, &crate::findings::TEST_COUNT_BELOW_FLOOR),
                fingerprint: String::new(),
                title: crate::findings::TEST_COUNT_BELOW_FLOOR.title.to_string(),
                anchor: None,
                legacy_title: crate::findings::TEST_COUNT_BELOW_FLOOR.was_title(),
                file: None,
                line: None,
                message: format!(
                    "Workspace test count ({measured_count}) dropped below base ref count ({base_count}) [tolerance: {}].",
                    settings.tolerance
                ),
                remediation: Some(
                    "Restore deleted tests or provide an allow-test-shrink: <reason> directive in the PR description."
                        .to_string(),
                ),
            });
        }
    } else if base_count == 0 {
        out.notes
            .push("no test count floor configured and zero base ref tests detected".to_string());
    }
    Ok(())
}

/// The assertion vocabulary of each side, with its runner collection rules, built at
/// most once for one evaluation of the gate.
struct SideVocabularies<'a, 'c> {
    ctx: &'a Context<'c>,
    base: std::cell::OnceCell<crate::ast::AssertVocabulary>,
    head: std::cell::OnceCell<crate::ast::AssertVocabulary>,
}

impl<'a, 'c> SideVocabularies<'a, 'c> {
    fn new(ctx: &'a Context<'c>) -> Self {
        Self {
            ctx,
            base: std::cell::OnceCell::new(),
            head: std::cell::OnceCell::new(),
        }
    }

    fn base(&self) -> Result<&crate::ast::AssertVocabulary> {
        if let Some(v) = self.base.get() {
            return Ok(v);
        }
        let built = crate::guards::agent_diff::assert_vocabulary_for_base(self.ctx)?;
        Ok(self.base.get_or_init(|| built))
    }

    fn head(&self) -> Result<&crate::ast::AssertVocabulary> {
        if let Some(v) = self.head.get() {
            return Ok(v);
        }
        let built = crate::guards::agent_diff::assert_vocabulary_for_head(self.ctx)?;
        Ok(self.head.get_or_init(|| built))
    }
}

/// The test files one rule of the change takes out of the default run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MovedOut {
    mechanism: crate::ast::runner_collection::Mechanism,
    /// Each file with the number of tests it holds on the head side, in path order.
    files: Vec<(String, usize)>,
    /// The head-side static count still counts the files (their collection is not
    /// determined there), so the count does not show the move.
    still_counted: bool,
}

impl MovedOut {
    /// How many of the files a message names.
    const NAMED: usize = 3;

    fn tests(&self) -> usize {
        self.files.iter().map(|(_, n)| n).sum()
    }

    fn message(&self) -> String {
        let named: Vec<String> = self
            .files
            .iter()
            .take(Self::NAMED)
            .map(|(path, _)| format!("`{path}`"))
            .collect();
        let more = match self.files.len().saturating_sub(Self::NAMED) {
            0 => String::new(),
            n => format!(", and {n} more"),
        };
        let count = if self.still_counted {
            "they are still in the static count, which does not show the move"
        } else {
            "they are no longer in the static count"
        };
        format!(
            "{} `{}` now leaves {} test(s) in {} file(s) out of the default run that the base side's default run collected: {}{}; {}.",
            self.mechanism.what,
            self.mechanism.file,
            self.tests(),
            self.files.len(),
            named.join(", "),
            more,
            count
        )
    }
}

/// The test files the change moves out of the default run, by the rule that does it:
/// a file the base side's default run collected, that still exists and still holds
/// tests on the head side, that the head side's default run does not collect, and that
/// a rule the change added leaves out
/// ([`crate::ast::runner_collection::moved_out_by`]). A deleted file, a file with no
/// test left, and a file new in the change are not among them.
fn tests_moved_out(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    sides: &SideVocabularies,
) -> Result<Vec<MovedOut>> {
    use crate::ast::runner_collection::RunnerCollectionStatus;
    use crate::ast::runner_collection::{check_runner_collected, moved_out_by, Mechanism};
    if !ctx.git.has_base() {
        return Ok(Vec::new());
    }
    let registry = crate::ast::default_registry();
    let base_tracked = ctx.git.base_tracked_files()?;
    if !base_tracked.iter().any(|p| registry.is_supported(p)) {
        return Ok(Vec::new());
    }
    let head_tracked: std::collections::HashSet<String> =
        ctx.git.tracked_files()?.into_iter().collect();
    let (base_v, head_v) = (sides.base()?, sides.head()?);
    let mut groups: std::collections::BTreeMap<Mechanism, MovedOut> = Default::default();
    for path in &base_tracked {
        if filter.matches(path) || !registry.is_supported(path) || !head_tracked.contains(path) {
            continue;
        }
        let mechanisms = moved_out_by(path, base_v, head_v);
        if mechanisms.is_empty() {
            continue;
        }
        // The tests the file holds on each side: a file with none on the base side was
        // not a test file, and one with none left is a deletion, which the count shows.
        let mut base_found = AstTestCount::default();
        base_found.add(path, ctx.git.base_content(path)?, &registry, base_v);
        let mut head_found = AstTestCount::default();
        head_found.add(path, ctx.git.head_content(path)?, &registry, head_v);
        let tests = head_found.running + head_found.ignored;
        if base_found.running + base_found.ignored == 0 || tests == 0 {
            continue;
        }
        let still_counted = matches!(
            check_runner_collected(path, head_v),
            RunnerCollectionStatus::Unknown(_)
        );
        for mechanism in mechanisms {
            let group = groups.entry(mechanism.clone()).or_insert_with(|| MovedOut {
                mechanism,
                files: Vec::new(),
                still_counted: false,
            });
            group.files.push((path.clone(), tests));
            group.still_counted |= still_counted;
        }
    }
    Ok(groups.into_values().collect())
}

/// Reports each rule of [`tests_moved_out`] as `Tests Moved Out Of Default Run`, unless
/// a directive naming the gate or the file that holds the rule lifts it. When a count
/// finding this run reports already covers the files (`count_reported`, and the files
/// left the static count with the move), the rule is a note beside that finding instead
/// of a second finding for one cause.
fn report_tests_moved_out(
    ctx: &Context,
    out: &mut GateOutcome,
    moved: &[MovedOut],
    count_reported: bool,
) {
    let kind = &crate::findings::TESTS_MOVED_OUT_OF_DEFAULT_RUN;
    let settings = &ctx.config.gates.test_floor;
    for group in moved {
        if count_reported && !group.still_counted {
            out.notes.push(format!("head: {}", group.message()));
            continue;
        }
        // Only a directive that names the file holding the rule, or one that names the
        // gate, lifts it: a subject that lifts a count finding (`min_tests`, a changed
        // test file, a removed test) says nothing about this rule.
        let lifted = ctx
            .find_override(GATE, kind, tokens::ALLOW_GATE_WEAKENING, GATE)
            .or_else(|| {
                ctx.find_override(GATE, kind, tokens::ALLOW_TEST_SHRINK, &group.mechanism.file)
            });
        if let Some(ov) = lifted {
            out.overrides.push(ov);
            continue;
        }
        out.push(
            ctx.overridable(settings.severity),
            kind,
            Some(&group.mechanism.file),
            group.mechanism.line,
            group.message(),
            "Take the rule back so the default run collects the tests again, or provide an allow-test-shrink: <file> <reason> directive in the PR description naming the file that holds the rule.",
        );
        out.anchor_last(format!("moved-out:{}", group.mechanism.key));
    }
}

/// An override for the test floor: a directive naming the gate, `min_tests`, a changed
/// file or a test it removed. `lifts` is the finding the caller would report.
fn find_test_floor_override(
    ctx: &Context,
    sides: &SideVocabularies,
    lifts: &crate::findings::FindingKind,
) -> Result<Option<crate::tokens::OverrideRecord>> {
    if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_GATE_WEAKENING, GATE) {
        return Ok(Some(ov));
    }
    if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, "min_tests") {
        return Ok(Some(ov));
    }
    let changed = ctx.git.changed_files()?;
    let registry = crate::ast::default_registry();
    let (base_v, head_v) = (sides.base()?, sides.head()?);

    for cf in &changed {
        if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, &cf.path) {
            return Ok(Some(ov));
        }
        if cf.old_path != cf.path {
            if let Some(ov) =
                ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, &cf.old_path)
            {
                return Ok(Some(ov));
            }
        }
        if let Some(file_name) = cf.path.rsplit('/').next() {
            if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, file_name) {
                return Ok(Some(ov));
            }
        }
        if let Some(stem) = Path::new(&cf.path).file_stem().and_then(|s| s.to_str()) {
            if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, stem) {
                return Ok(Some(ov));
            }
        }

        if let Some(base_src) = ctx.git.base_content(&cf.old_path)? {
            if let Some(pack) = registry.find_pack(&cf.old_path) {
                if crate::ast::runner_collection::is_runner_collected(&cf.old_path, base_v) {
                    if let Ok(base_facts) = pack.extract(&cf.old_path, &base_src, base_v) {
                        let head_names: std::collections::HashSet<String> = if cf.is_deleted()
                            || !crate::ast::runner_collection::is_runner_collected(&cf.path, head_v)
                        {
                            std::collections::HashSet::new()
                        } else if let Some(head_src) = ctx.git.head_content(&cf.path)? {
                            pack.extract(&cf.path, &head_src, head_v)
                                .map(|f| f.tests.into_iter().map(|t| t.name).collect())
                                .unwrap_or_default()
                        } else {
                            std::collections::HashSet::new()
                        };

                        for t in base_facts.tests {
                            if !head_names.contains(&t.name) {
                                if let Some(ov) = ctx.find_override(
                                    GATE,
                                    lifts,
                                    tokens::ALLOW_TEST_SHRINK,
                                    &t.name,
                                ) {
                                    return Ok(Some(ov));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

/// A static test count over one side of the change.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AstTestCount {
    /// Tests that run. An ignored or skipped test does not count toward a floor: it
    /// verifies nothing until someone re-enables it.
    pub running: usize,
    pub ignored: usize,
    /// Supported-language files that could not be read (contributed nothing) or that
    /// parse with errors (tree-sitter recovers what it can; the count may be short).
    pub unread: Vec<String>,
    pub notes: Vec<String>,
    /// Files with tests that were counted although runner collection was not
    /// determined, by reason.
    pub unknown_collection: std::collections::BTreeMap<String, usize>,
    /// Files with tests that were left out with a note, by reason: nothing tracked in
    /// the repository could run a test in their language, or `.gitattributes` marks
    /// them as vendored or generated.
    pub no_runner: std::collections::BTreeMap<String, usize>,
    /// Each file with tests whose runner collection is determined, and that counts.
    pub collected_files: std::collections::BTreeSet<String>,
    /// Each file with tests that counts although collection was not determined, with
    /// the reason.
    pub unknown_files: std::collections::BTreeMap<String, String>,
}

impl AstTestCount {
    /// As [`Self::add`], for a file runner collection gave `status` for. A file is left
    /// out when a parsed runner configuration excludes it, or, with a note, when the
    /// repository holds no manifest a runner of its language needs; one whose collection
    /// is not determined counts every test its pack finds, and is recorded for the notes.
    fn add_collected(
        &mut self,
        path: &str,
        status: crate::ast::runner_collection::RunnerCollectionStatus,
        content: impl FnOnce() -> Result<Option<String>>,
        registry: &crate::ast::LanguageRegistry,
        v: &crate::ast::AssertVocabulary,
    ) -> Result<()> {
        use crate::ast::runner_collection::RunnerCollectionStatus;
        let unknown = match status {
            RunnerCollectionStatus::Collected => None,
            RunnerCollectionStatus::NotCollected => return Ok(()),
            RunnerCollectionStatus::Unknown(reason) => Some(reason),
            RunnerCollectionStatus::NoRunner(reason) => {
                // Not counted; the note still says how many such files hold tests.
                let mut found = AstTestCount::default();
                found.add(path, content()?, registry, v);
                if found.running + found.ignored > 0 {
                    *self.no_runner.entry(reason).or_default() += 1;
                }
                return Ok(());
            }
        };
        let before = self.running + self.ignored;
        self.add(path, content()?, registry, v);
        if self.running + self.ignored > before {
            match unknown {
                Some(reason) => {
                    *self.unknown_collection.entry(reason.clone()).or_default() += 1;
                    self.unknown_files.insert(path.to_string(), reason);
                }
                None => {
                    self.collected_files.insert(path.to_string());
                }
            }
        }
        Ok(())
    }

    /// Notes for the files the change takes out of determined collection while they
    /// keep counting: collected on the base side, and on the head side counted with
    /// collection not determined. The count cannot show it, so each file is named (the
    /// first few), with the reason. `renamed` gives the base-side path of each file the
    /// change renamed, by its head-side path: a file moved into a place where its
    /// collection is not determined is the base side's file under its old path.
    fn transition_notes(
        &self,
        base: &AstTestCount,
        renamed: &std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        const NAMED: usize = 5;
        let moved: Vec<(&String, &String, Option<&String>)> = self
            .unknown_files
            .iter()
            .filter_map(|(path, reason)| {
                if base.collected_files.contains(path) {
                    return Some((path, reason, None));
                }
                let old = renamed.get(path)?;
                base.collected_files
                    .contains(old)
                    .then_some((path, reason, Some(old)))
            })
            .collect();
        let mut notes: Vec<String> = moved
            .iter()
            .take(NAMED)
            .map(|(path, reason, old)| {
                let named = match old {
                    Some(old) => format!("`{path}` (renamed from `{old}`)"),
                    None => format!("`{path}`"),
                };
                format!(
                    "head: {named} was collected on the base side, and the change leaves whether its tests run not determined ({reason}); they are still counted, so the count does not show the change"
                )
            })
            .collect();
        if moved.len() > NAMED {
            notes.push(format!(
                "head: {} more file(s) were collected on the base side and are counted with collection not determined on the head side",
                moved.len() - NAMED
            ));
        }
        notes
    }

    fn add(
        &mut self,
        path: &str,
        content: Option<String>,
        registry: &crate::ast::LanguageRegistry,
        v: &crate::ast::AssertVocabulary,
    ) {
        let facts = content.and_then(|c| {
            registry
                .find_pack(path)
                .and_then(|pack| pack.extract(path, &c, v).ok())
        });
        match facts {
            Some(facts) => {
                if facts.has_parse_errors {
                    self.unread.push(path.to_string());
                }
                for note in &facts.notes {
                    self.notes.push(note.clone());
                }
                // A conditional skip (`skipif`, `cfg_attr(..., ignore)`) still runs somewhere.
                let ignored = facts
                    .tests
                    .iter()
                    .filter(|t| t.ignored && t.conditional_ignore.is_none())
                    .count();
                self.ignored += ignored;
                self.running += facts.tests.len() - ignored;
            }
            None => self.unread.push(path.to_string()),
        }
    }

    /// Notes for the report: what was left out of the count, and why.
    fn notes(&self, side: &str) -> Vec<String> {
        let mut notes = Vec::new();
        for note in &self.notes {
            notes.push(note.clone());
        }
        for (reason, files) in &self.unknown_collection {
            notes.push(format!(
                "{side}: runner collection unknown ({reason}): every test the language packs found in {files} file(s) is counted"
            ));
        }
        for (reason, files) in &self.no_runner {
            notes.push(format!(
                "{side}: {reason}: the tests the language packs found in {files} file(s) are not counted"
            ));
        }
        if self.ignored > 0 {
            notes.push(format!(
                "{side}: {} ignored / skipped test(s) are not counted toward the floor",
                self.ignored
            ));
        }
        if !self.unread.is_empty() {
            let shown: Vec<&str> = self.unread.iter().take(5).map(String::as_str).collect();
            notes.push(format!(
                "{side}: {} file(s) could not be read, or parse with errors, so their tests may be uncounted: {}{}",
                self.unread.len(),
                shown.join(", "),
                if self.unread.len() > shown.len() {
                    ", ..."
                } else {
                    ""
                }
            ));
        }
        notes
    }
}

/// Counts running test functions across all supported language packs in the workspace.
pub fn count_workspace_ast_tests(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    v: &crate::ast::AssertVocabulary,
) -> Result<AstTestCount> {
    let registry = crate::ast::default_registry();
    let mut count = AstTestCount::default();
    for path in ctx.git.tracked_files()? {
        if filter.matches(&path) || !registry.is_supported(&path) {
            continue;
        }
        let full = Path::new(ctx.git.root()).join(&path);
        // A tracked file deleted from the working tree is gone, not unreadable.
        if !full.exists() {
            continue;
        }
        let status = crate::ast::runner_collection::check_runner_collected(&path, v);
        count.add_collected(
            &path,
            status,
            || Ok(std::fs::read_to_string(&full).ok()),
            &registry,
            v,
        )?;
    }
    Ok(count)
}

/// Counts running test functions across all supported language packs in base ref.
pub fn count_base_workspace_ast_tests(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    v: &crate::ast::AssertVocabulary,
) -> Result<AstTestCount> {
    let registry = crate::ast::default_registry();
    let mut count = AstTestCount::default();
    for path in ctx.git.base_tracked_files()? {
        if filter.matches(&path) || !registry.is_supported(&path) {
            continue;
        }
        let status = crate::ast::runner_collection::check_runner_collected(&path, v);
        count.add_collected(&path, status, || ctx.git.base_content(&path), &registry, v)?;
    }
    Ok(count)
}

/// Whether the `test_command` in force is one the base side does not vouch for: it is
/// set, and the base side's is absent or different. Removing a `test_command` executes
/// nothing; `config-integrity` reports that.
pub(crate) fn test_command_supplied_by_change(in_force: Option<&str>, base: Option<&str>) -> bool {
    in_force.is_some() && in_force != base
}

/// How long a `test_command` may run. `[gates.test-floor]` has no `timeout_seconds`, and
/// a listing may have to build the tests first, so this is the longest limit a built-in
/// gate gives a command (`miri`'s default).
pub const TEST_COMMAND_TIMEOUT_SECONDS: u64 = 600;

/// Executes an external test listing command and counts tests from output lines, under
/// the bounds of the `command` gate's runner: [`TEST_COMMAND_TIMEOUT_SECONDS`] and the
/// same capture limit. A command that times out, cannot be started, fails, or prints past
/// the capture limit is an error (exit 2), never a count of zero or of the part captured.
pub fn count_tests_via_command(cmd: &str, cwd: &Path) -> Result<usize> {
    count_tests_via_command_within(cmd, cwd, TEST_COMMAND_TIMEOUT_SECONDS)
}

/// [`count_tests_via_command`] with the time limit given.
pub fn count_tests_via_command_within(
    cmd: &str,
    cwd: &Path,
    timeout_seconds: u64,
) -> Result<usize> {
    use crate::could_not_check::{tag, Reason};
    use crate::guards::command::{run_command_bounded, MAX_CAPTURE_BYTES};
    let run = run_command_bounded("test_command", cmd, timeout_seconds, cwd)?;
    if !run.status.success() {
        bail!(
            "Test listing command failed with exit code {:?}:\n{}",
            run.status.code(),
            run.stderr
        );
    }
    if run.stdout_truncated {
        return Err(tag(
            Reason::Gate,
            anyhow::anyhow!(
                "`test_command` output went past the {MAX_CAPTURE_BYTES}-byte capture limit or could not be read, so the tests it lists cannot be counted; make the command print the count, or one line per test and nothing else"
            ),
        ));
    }
    Ok(parse_test_count_output(&run.stdout))
}

/// Parses test count from output (e.g. `cargo test -- --list` lines ending in `: test`).
pub fn parse_test_count_output(output: &str) -> usize {
    let mut count = 0;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.ends_with(": test") {
            count += 1;
        }
    }
    if count == 0 {
        // Fallback: if output is a single integer
        if let Ok(n) = output.trim().parse::<usize>() {
            return n;
        }
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCaseReport {
    pub id: String,
    pub name: String,
    pub classname: Option<String>,
    pub status: TestStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestIdentityIssue {
    Missing,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestIdentityViolation {
    pub id: String,
    pub name: String,
    pub classname: Option<String>,
    pub issue: TestIdentityIssue,
}

/// Parses JUnit XML string and returns list of test case records.
pub fn parse_junit_xml(xml: &str) -> Result<Vec<TestCaseReport>> {
    let trimmed = xml.trim();
    if trimmed.is_empty() {
        bail!("JUnit XML test report is empty");
    }

    if !xml.contains("<testcase") && !xml.contains("<testsuite") {
        bail!("Content does not contain JUnit XML <testsuite> or <testcase> elements");
    }

    let re_comment = Regex::new(r#"(?s)<!--.*?-->"#)?;
    let cleaned_xml = re_comment.replace_all(xml, "");

    let mut cases = Vec::new();
    let re_case = Regex::new(r#"(?s)<testcase\b([^>]*?)(?:/>|>(.*?)</testcase>)"#)?;
    let re_attr = Regex::new(r#"([a-zA-Z0-9_\-:]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)?;
    let re_sysout = Regex::new(r#"(?s)<system-out\b[^>]*>.*?</system-out>"#)?;
    let re_syserr = Regex::new(r#"(?s)<system-err\b[^>]*>.*?</system-err>"#)?;
    let re_failure = Regex::new(r#"(?i)<failure[\s>/]"#)?;
    let re_error = Regex::new(r#"(?i)<error[\s>/]"#)?;
    let re_skipped = Regex::new(r#"(?i)<skipped[\s>/]"#)?;

    for cap in re_case.captures_iter(&cleaned_xml) {
        let attr_str = &cap[1];
        let body_opt = cap.get(2).map(|m| m.as_str());

        let mut name = String::new();
        let mut classname = None;
        let mut id_attr = None;

        for attr in re_attr.captures_iter(attr_str) {
            let k = &attr[1];
            let v = attr
                .get(2)
                .or_else(|| attr.get(3))
                .map(|m| m.as_str())
                .unwrap_or("");
            let val = unescape_xml(v);
            match k {
                "name" => name = val,
                "classname" if !val.trim().is_empty() => {
                    classname = Some(val);
                }
                "id" if !val.trim().is_empty() => {
                    id_attr = Some(val);
                }
                _ => {}
            }
        }

        if name.is_empty() {
            if let Some(id) = id_attr.as_ref() {
                name = id.clone();
            } else {
                continue;
            }
        }

        let id = if let Some(id) = id_attr {
            id
        } else if let Some(cn) = &classname {
            format!("{cn}::{name}")
        } else {
            name.clone()
        };

        let status = if let Some(body) = body_opt {
            let cleaned_body = re_sysout.replace_all(body, "");
            let cleaned_body = re_syserr.replace_all(&cleaned_body, "");

            if re_failure.is_match(&cleaned_body) || re_error.is_match(&cleaned_body) {
                TestStatus::Failed
            } else if re_skipped.is_match(&cleaned_body) {
                TestStatus::Skipped
            } else {
                TestStatus::Passed
            }
        } else {
            TestStatus::Passed
        };

        cases.push(TestCaseReport {
            id,
            name,
            classname,
            status,
        });
    }

    if cases.is_empty() {
        bail!("JUnit XML test report contains no <testcase> elements");
    }

    Ok(cases)
}

/// Compares base passed test cases with head test cases.
///
/// Invariant: Every test case that passed on base ref must pass on head ref.
/// A missing, skipped, or failed test is returned as a violation.
pub fn compare_test_identities(
    base_cases: &[TestCaseReport],
    head_cases: &[TestCaseReport],
) -> Vec<TestIdentityViolation> {
    use std::collections::BTreeMap;

    let mut base_passed: BTreeMap<String, &TestCaseReport> = BTreeMap::new();
    for c in base_cases {
        if c.status == TestStatus::Passed {
            base_passed.insert(c.id.clone(), c);
        }
    }

    let mut head_by_id: BTreeMap<String, Vec<&TestCaseReport>> = BTreeMap::new();
    for c in head_cases {
        head_by_id.entry(c.id.clone()).or_default().push(c);
    }

    let mut violations = Vec::new();
    for (id, base_test) in base_passed {
        match head_by_id.get(&id) {
            None => {
                violations.push(TestIdentityViolation {
                    id,
                    name: base_test.name.clone(),
                    classname: base_test.classname.clone(),
                    issue: TestIdentityIssue::Missing,
                });
            }
            Some(runs) => {
                if runs.iter().any(|r| r.status == TestStatus::Passed) {
                    continue;
                } else if runs.iter().all(|r| r.status == TestStatus::Skipped) {
                    violations.push(TestIdentityViolation {
                        id,
                        name: base_test.name.clone(),
                        classname: base_test.classname.clone(),
                        issue: TestIdentityIssue::Skipped,
                    });
                } else {
                    violations.push(TestIdentityViolation {
                        id,
                        name: base_test.name.clone(),
                        classname: base_test.classname.clone(),
                        issue: TestIdentityIssue::Failed,
                    });
                }
            }
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_command_the_base_does_not_have_is_supplied_by_the_change() {
        // Added, with or without a base configuration, and repointed.
        assert!(test_command_supplied_by_change(Some("echo 9"), None));
        assert!(test_command_supplied_by_change(
            Some("echo 9"),
            Some("echo 7")
        ));
        // Unchanged, which is also what `--policy-from base` gives.
        assert!(!test_command_supplied_by_change(
            Some("echo 7"),
            Some("echo 7")
        ));
        // Removed or never set: nothing is executed.
        assert!(!test_command_supplied_by_change(None, Some("echo 7")));
        assert!(!test_command_supplied_by_change(None, None));
    }

    #[test]
    fn parses_cargo_test_list_output() {
        let sample = "
     Running unittests src/lib.rs (target/debug/deps/foo-123)
tests::test_insert: test
tests::test_remove: test
tests::test_get: test
3 tests, 0 benchmarks
     Running tests/test_blob.rs (target/debug/deps/test_blob-456)
test_blob_basic: test
test_blob_compact: test
2 tests, 0 benchmarks
";
        assert_eq!(parse_test_count_output(sample), 5);
    }

    #[test]
    fn parses_numeric_output() {
        assert_eq!(parse_test_count_output("305\n"), 305);
    }

    #[test]
    fn test_parse_junit_xml_various_elements() {
        let sample = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites tests="3" failures="0" errors="0">
  <testsuite name="unit_tests" tests="3">
    <testcase classname="tests::auth" name="test_login" time="0.04"/>
    <testcase name="test_bare" time="0.01"/>
    <testcase id="custom_id_123" classname="tests::api" name="test_endpoint"/>
  </testsuite>
</testsuites>
"#;
        let cases = parse_junit_xml(sample).unwrap();
        assert_eq!(cases.len(), 3);

        assert_eq!(cases[0].id, "tests::auth::test_login");
        assert_eq!(cases[0].name, "test_login");
        assert_eq!(cases[0].classname.as_deref(), Some("tests::auth"));
        assert_eq!(cases[0].status, TestStatus::Passed);

        assert_eq!(cases[1].id, "test_bare");
        assert_eq!(cases[1].name, "test_bare");
        assert_eq!(cases[1].classname, None);
        assert_eq!(cases[1].status, TestStatus::Passed);

        assert_eq!(cases[2].id, "custom_id_123");
        assert_eq!(cases[2].name, "test_endpoint");
        assert_eq!(cases[2].status, TestStatus::Passed);
    }

    #[test]
    fn test_parse_junit_xml_statuses() {
        let sample = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="status_tests">
  <testcase classname="suite" name="test_passed">
    <system-out>this test outputs failure log text inside stdout</system-out>
  </testcase>
  <testcase classname="suite" name="test_failed">
    <failure message="assertion failed: `(left == right)`">details here</failure>
  </testcase>
  <testcase classname="suite" name="test_error">
    <error message="runtime exception">stack trace</error>
  </testcase>
  <testcase classname="suite" name="test_skipped_tag">
    <skipped message="pending fix"/>
  </testcase>
  <testcase classname="suite" name="test_skipped_body">
    <skipped>disabled in this configuration</skipped>
  </testcase>
</testsuite>
"#;
        let cases = parse_junit_xml(sample).unwrap();
        assert_eq!(cases.len(), 5);

        assert_eq!(cases[0].id, "suite::test_passed");
        assert_eq!(cases[0].status, TestStatus::Passed);

        assert_eq!(cases[1].id, "suite::test_failed");
        assert_eq!(cases[1].status, TestStatus::Failed);

        assert_eq!(cases[2].id, "suite::test_error");
        assert_eq!(cases[2].status, TestStatus::Failed);

        assert_eq!(cases[3].id, "suite::test_skipped_tag");
        assert_eq!(cases[3].status, TestStatus::Skipped);

        assert_eq!(cases[4].id, "suite::test_skipped_body");
        assert_eq!(cases[4].status, TestStatus::Skipped);
    }

    #[test]
    fn test_parse_junit_xml_entities_and_comments() {
        let sample = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="entities">
  <!-- <testcase classname="commented" name="test_ignored"/> -->
  <testcase classname='pkg&amp;sub' name='test&quot;quoted&quot;'/>
</testsuite>
"#;
        let cases = parse_junit_xml(sample).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "pkg&sub::test\"quoted\"");
        assert_eq!(cases[0].name, "test\"quoted\"");
        assert_eq!(cases[0].classname.as_deref(), Some("pkg&sub"));
    }

    #[test]
    fn test_parse_junit_xml_empty_fails() {
        assert!(parse_junit_xml("").is_err());
        assert!(parse_junit_xml("   \n\t  ").is_err());
    }

    #[test]
    fn test_parse_junit_xml_no_testcase_fails() {
        let sample = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="empty_suite">
</testsuite>
"#;
        assert!(parse_junit_xml(sample).is_err());
    }

    #[test]
    fn test_compare_test_identities_missing() {
        let base = vec![
            TestCaseReport {
                id: "test_a".into(),
                name: "test_a".into(),
                classname: None,
                status: TestStatus::Passed,
            },
            TestCaseReport {
                id: "test_b".into(),
                name: "test_b".into(),
                classname: None,
                status: TestStatus::Passed,
            },
        ];
        let head = vec![TestCaseReport {
            id: "test_a".into(),
            name: "test_a".into(),
            classname: None,
            status: TestStatus::Passed,
        }];

        let violations = compare_test_identities(&base, &head);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].id, "test_b");
        assert_eq!(violations[0].issue, TestIdentityIssue::Missing);
    }

    #[test]
    fn test_compare_test_identities_skipped() {
        let base = vec![TestCaseReport {
            id: "test_a".into(),
            name: "test_a".into(),
            classname: None,
            status: TestStatus::Passed,
        }];
        let head = vec![TestCaseReport {
            id: "test_a".into(),
            name: "test_a".into(),
            classname: None,
            status: TestStatus::Skipped,
        }];

        let violations = compare_test_identities(&base, &head);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].id, "test_a");
        assert_eq!(violations[0].issue, TestIdentityIssue::Skipped);
    }

    #[test]
    fn test_compare_test_identities_failed() {
        let base = vec![TestCaseReport {
            id: "test_a".into(),
            name: "test_a".into(),
            classname: None,
            status: TestStatus::Passed,
        }];
        let head = vec![TestCaseReport {
            id: "test_a".into(),
            name: "test_a".into(),
            classname: None,
            status: TestStatus::Failed,
        }];

        let violations = compare_test_identities(&base, &head);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].id, "test_a");
        assert_eq!(violations[0].issue, TestIdentityIssue::Failed);
    }

    #[test]
    fn test_compare_test_identities_non_passing_base_not_ratcheted() {
        let base = vec![
            TestCaseReport {
                id: "test_fail".into(),
                name: "test_fail".into(),
                classname: None,
                status: TestStatus::Failed,
            },
            TestCaseReport {
                id: "test_skip".into(),
                name: "test_skip".into(),
                classname: None,
                status: TestStatus::Skipped,
            },
        ];
        // Head doesn't run either of them
        let head = vec![];

        let violations = compare_test_identities(&base, &head);
        assert!(
            violations.is_empty(),
            "Tests that failed or were skipped on base are not ratcheted"
        );
    }

    #[test]
    fn test_compare_test_identities_retry_passed() {
        let base = vec![TestCaseReport {
            id: "test_retry".into(),
            name: "test_retry".into(),
            classname: None,
            status: TestStatus::Passed,
        }];
        let head = vec![
            TestCaseReport {
                id: "test_retry".into(),
                name: "test_retry".into(),
                classname: None,
                status: TestStatus::Failed,
            },
            TestCaseReport {
                id: "test_retry".into(),
                name: "test_retry".into(),
                classname: None,
                status: TestStatus::Passed,
            },
        ];

        let violations = compare_test_identities(&base, &head);
        assert!(
            violations.is_empty(),
            "Test that passed on retry is considered passed on head"
        );
    }

    // ---- #592: `test_command` runs under the command gate's bounds ----

    const LISTING_OF_TWO: &str = "printf 'a: test\\nb: test\\n'";
    const NEVER_ENDS: &str = "sleep 30";
    const FLOOD: &str = "head -c 26214401 /dev/zero";
    const FAILS_AFTER_A_COUNT: &str = "sh -c 'echo 99999; exit 3'";

    fn reason(e: &anyhow::Error) -> crate::could_not_check::Reason {
        crate::could_not_check::classify(e).0
    }

    #[test]
    fn a_test_command_is_counted_within_its_bounds() {
        let cwd = std::env::temp_dir();
        assert_eq!(count_tests_via_command(LISTING_OF_TWO, &cwd).unwrap(), 2);
    }

    #[test]
    fn a_test_command_that_does_not_end_is_stopped_and_is_not_a_count() {
        let cwd = std::env::temp_dir();
        let started = std::time::Instant::now();
        let err = count_tests_via_command_within(NEVER_ENDS, &cwd, 1).unwrap_err();
        assert_eq!(
            reason(&err),
            crate::could_not_check::Reason::ToolTimeout,
            "{err:#}"
        );
        assert!(format!("{err:#}").contains("timed out after 1s"), "{err:#}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn a_test_command_past_the_capture_limit_is_not_a_count() {
        let cwd = std::env::temp_dir();
        let err = count_tests_via_command(FLOOD, &cwd).unwrap_err();
        assert_eq!(
            reason(&err),
            crate::could_not_check::Reason::Gate,
            "{err:#}"
        );
        assert!(format!("{err:#}").contains("capture limit"), "{err:#}");
    }

    #[test]
    fn a_failed_or_missing_test_command_is_not_a_count() {
        let cwd = std::env::temp_dir();
        let failed = count_tests_via_command(FAILS_AFTER_A_COUNT, &cwd).unwrap_err();
        assert!(
            format!("{failed:#}").contains("exit code Some(3)"),
            "{failed:#}"
        );
        let missing =
            count_tests_via_command("discipline-no-such-test-lister --list", &cwd).unwrap_err();
        assert_eq!(
            reason(&missing),
            crate::could_not_check::Reason::ToolMissing,
            "{missing:#}"
        );
    }
}
