//! Universal test count floor ratchet sentinel (`test-floor`).
//!
//! Enforces:
//! - Workspace test count does not drop below a pinned floor constant or base ref count.
//! - Floor constant in base ref cannot be lowered without an `allow-test-shrink:` directive.
//! - Required test suite files must exist on disk unless explicitly excused.
//! - Fails closed if the base floor cannot be resolved when configured.

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
    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let filter = exempt_filter(settings)?;

    // Read base discipline.toml to get base configuration
    let base_cfg = ctx
        .base_config_text()
        .ok()
        .flatten()
        .and_then(|s| crate::config::DisciplineConfig::from_toml_str(&s).ok());
    let base_min_tests = base_cfg.as_ref().and_then(|c| c.gates.test_floor.min_tests);
    let head_min_tests = settings.min_tests;

    // 1. Resolve base floor constant from constant_file if configured.
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
                        return Ok(out);
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
                    return Ok(out);
                }
            }
            Err(e) => {
                bail!("Failed to read '{const_file}' from base ref: {e}");
            }
        }
    }

    // 2. Check if head constant is lower than base constant.
    if let (Some(const_file), Some(const_name), Some(base_floor)) = (
        &settings.constant_file,
        &settings.constant_name,
        base_floor_const,
    ) {
        let head_path = Path::new(ctx.git.root()).join(const_file);
        if head_path.is_file() {
            if let Ok(head_src) = std::fs::read_to_string(&head_path) {
                let pat = format!(
                    r"(?m)^[ \t]*(?:(?:pub|export)\s+)?(?:const\s+)?{}(?:\s*:\s*[a-zA-Z0-9_]+)?\s*=\s*(\d+)",
                    regex::escape(const_name)
                );
                if let Ok(re) = Regex::new(&pat) {
                    if let Some(caps) = re.captures(&head_src) {
                        if let Ok(head_val) = caps[1].parse::<usize>() {
                            if head_val < base_floor {
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
                                        message: format!(
                                            "Floor constant '{const_name}' ({head_val}) was decreased below base ref ({base_floor})."
                                        ),
                                        remediation: Some(
                                            "Restore the floor constant or provide an allow-test-shrink: <reason> directive in the PR description."
                                                .to_string(),
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Check min_tests comparison against base discipline.toml.
    if let Some(base_min) = base_min_tests {
        let lowered = match head_min_tests {
            Some(h) => h < base_min,
            None => true,
        };
        if lowered {
            if let Some(ov) =
                find_test_floor_override(ctx, &crate::findings::CONFIGURED_FLOOR_DECREASED)
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

    // 4. Required test suites check.
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

    // Base ref discipline.toml takes precedence over HEAD discipline.toml to prevent self-lowering.
    let explicit_floor = base_min_tests.or(head_min_tests).or(base_floor_const);

    // The zero-config ratchet counts the base ref statically; it cannot build
    // and run the base ref's tests. Comparing that against a runtime count
    // mixes two counting bases (see docs/GATES.md, test-floor), so the result
    // would be meaningless in either direction. Refuse before running anything.
    if settings.test_command.is_some() && explicit_floor.is_none() {
        bail!(
            "test-floor: `test_command` supplies a runtime test count, but no floor is configured to \
             compare it against; set `min_tests` (or `constant_file` + `constant_name`) to a count on \
             the same basis, or remove `test_command` to use the static ratchet"
        );
    }

    // 5. Resolve test reports for identity-based ratcheting
    let env_head = std::env::var("DISCIPLINE_TEST_HEAD_REPORT").ok();
    let env_report = std::env::var("DISCIPLINE_TEST_REPORT").ok();
    let head_report_path: Option<&str> = ctx
        .test_head_report
        .as_deref()
        .and_then(|p| p.to_str())
        .or(env_head.as_deref())
        .or(settings.head_report.as_deref())
        .or_else(|| ctx.test_report.as_deref().and_then(|p| p.to_str()))
        .or(env_report.as_deref())
        .or(settings.test_report.as_deref());

    let env_base = std::env::var("DISCIPLINE_TEST_BASE_REPORT").ok();
    let base_report_path: Option<&str> = ctx
        .test_base_report
        .as_deref()
        .and_then(|p| p.to_str())
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

    // 6. Test Identity Ratchet
    if let (Some(base_cases), Some(head_cases)) = (&base_cases_opt, &head_cases_opt) {
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
                candidate_subjects.push(cn.as_str());
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
                    anchor: None,
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

    // 7. Calculate measured test count.
    let measured_count = if let Some(cmd) = &settings.test_command {
        count_tests_via_command(cmd, Path::new(ctx.git.root()))?
    } else if let Some(head_cases) = &head_cases_opt {
        head_cases.len()
    } else {
        let head = count_workspace_ast_tests(ctx, &filter)?;
        out.notes.extend(head.notes("head"));
        head.running
    };
    out.examined = measured_count;

    // 8. Compare against the effective floor.
    if let Some(floor) = explicit_floor {
        if measured_count + settings.tolerance < floor {
            if let Some(ov) =
                find_test_floor_override(ctx, &crate::findings::TEST_COUNT_BELOW_FLOOR)
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
    } else if let Some(base_cases) = &base_cases_opt {
        let base_count = base_cases.len();
        if base_count > 0 && measured_count + settings.tolerance < base_count {
            if let Some(ov) =
                find_test_floor_override(ctx, &crate::findings::TEST_COUNT_BELOW_FLOOR)
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
        }
    } else {
        // Zero-config ratchet: compare head AST test count against base ref AST test count.
        let base = count_base_workspace_ast_tests(ctx, &filter)?;
        out.notes.extend(base.notes("base"));
        let base_count = base.running;
        if base_count > 0 && measured_count + settings.tolerance < base_count {
            if let Some(ov) =
                find_test_floor_override(ctx, &crate::findings::TEST_COUNT_BELOW_FLOOR)
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
            out.notes.push(
                "no test count floor configured and zero base ref tests detected".to_string(),
            );
        }
    }

    Ok(out)
}

/// An override for the test floor: a directive naming the gate, `min_tests`, a changed
/// file or a test it removed. `lifts` is the finding the caller would report.
fn find_test_floor_override(
    ctx: &Context,
    lifts: &crate::findings::FindingKind,
) -> Option<crate::tokens::OverrideRecord> {
    if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_GATE_WEAKENING, GATE) {
        return Some(ov);
    }
    if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, "min_tests") {
        return Some(ov);
    }
    if let Ok(changed) = ctx.git.changed_files() {
        let registry = crate::ast::default_registry();
        let v = crate::guards::agent_diff::assert_vocabulary(ctx.config);

        for cf in &changed {
            if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, &cf.path) {
                return Some(ov);
            }
            if cf.old_path != cf.path {
                if let Some(ov) =
                    ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, &cf.old_path)
                {
                    return Some(ov);
                }
            }
            if let Some(file_name) = cf.path.rsplit('/').next() {
                if let Some(ov) =
                    ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, file_name)
                {
                    return Some(ov);
                }
            }
            if let Some(stem) = Path::new(&cf.path).file_stem().and_then(|s| s.to_str()) {
                if let Some(ov) = ctx.find_override(GATE, lifts, tokens::ALLOW_TEST_SHRINK, stem) {
                    return Some(ov);
                }
            }

            if let Ok(Some(base_src)) = ctx.git.base_content(&cf.old_path) {
                if let Some(pack) = registry.find_pack(&cf.old_path) {
                    if let Ok(base_facts) = pack.extract(&cf.old_path, &base_src, &v) {
                        let head_names: std::collections::HashSet<String> = if cf.is_deleted() {
                            std::collections::HashSet::new()
                        } else if let Ok(Some(head_src)) = ctx.git.head_content(&cf.path) {
                            pack.extract(&cf.path, &head_src, &v)
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
                                    return Some(ov);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
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
}

impl AstTestCount {
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
) -> Result<AstTestCount> {
    let registry = crate::ast::default_registry();
    let v = crate::guards::agent_diff::assert_vocabulary(ctx.config);
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
        count.add(&path, std::fs::read_to_string(&full).ok(), &registry, &v);
    }
    Ok(count)
}

/// Counts running test functions across all supported language packs in base ref.
pub fn count_base_workspace_ast_tests(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
) -> Result<AstTestCount> {
    let registry = crate::ast::default_registry();
    let v = crate::guards::agent_diff::assert_vocabulary(ctx.config);
    let mut count = AstTestCount::default();
    for path in ctx.git.base_tracked_files()? {
        if filter.matches(&path) || !registry.is_supported(&path) {
            continue;
        }
        count.add(
            &path,
            ctx.git.base_content(&path).ok().flatten(),
            &registry,
            &v,
        );
    }
    Ok(count)
}

/// Executes an external test listing command and counts tests from output lines.
pub fn count_tests_via_command(cmd: &str, cwd: &Path) -> Result<usize> {
    let parts = crate::guards::command::split_command_line(cmd)?;
    if parts.is_empty() {
        bail!("test_command is empty");
    }
    let mut process = std::process::Command::new(&parts[0]);
    process.args(&parts[1..]);
    process.current_dir(cwd);

    let output = process
        .output()
        .with_context(|| format!("Failed to execute test listing command: '{cmd}'"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Test listing command failed with exit code {:?}:\n{}",
            output.status.code(),
            stderr
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_test_count_output(&stdout))
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
        return Ok(Vec::new());
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

    Ok(cases)
}

fn unescape_xml(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
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
}
