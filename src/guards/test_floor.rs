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
use std::path::Path;

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
                    if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, subject) {
                        out.overrides.push(ov);
                    } else {
                        out.violations.push(Violation {
                            gate: GATE,
                            severity: ctx.overridable(settings.severity),
                            code: crate::findings::full_code(GATE, &crate::findings::FLOOR_CONSTANT_MISSING_IN_BASE),
                            fingerprint: String::new(),
                            title: crate::findings::FLOOR_CONSTANT_MISSING_IN_BASE.fixed_title().to_string(),
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
                if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, const_file) {
                    out.overrides.push(ov);
                } else {
                    out.violations.push(Violation {
                        gate: GATE,
                        severity: ctx.overridable(settings.severity),
                        code: crate::findings::full_code(GATE, &crate::findings::FLOOR_CONSTANT_FILE_MISSING_IN_BASE),
                        fingerprint: String::new(),
                        title: crate::findings::FLOOR_CONSTANT_FILE_MISSING_IN_BASE.fixed_title().to_string(),
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
                                if let Some(ov) =
                                    ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, const_name)
                                {
                                    out.overrides.push(ov);
                                } else {
                                    out.violations.push(Violation {
                                        gate: GATE,
                                        severity: ctx.overridable(settings.severity),
                                        code: crate::findings::full_code(GATE, &crate::findings::FLOOR_CONSTANT_DECREASED),
                                        fingerprint: String::new(),
                                        title: crate::findings::FLOOR_CONSTANT_DECREASED.fixed_title().to_string(),
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
            if let Some(ov) = find_test_floor_override(ctx) {
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
                    title: crate::findings::CONFIGURED_FLOOR_DECREASED.fixed_title().to_string(),
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
            if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, suite) {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::REQUIRED_SUITE_MISSING),
                    fingerprint: String::new(),
                    title: crate::findings::REQUIRED_SUITE_MISSING.fixed_title().to_string(),
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

    // 5. Calculate measured test count.
    let measured_count = if let Some(cmd) = &settings.test_command {
        count_tests_via_command(cmd, Path::new(ctx.git.root()))?
    } else {
        let head = count_workspace_ast_tests(ctx, &filter)?;
        out.notes.extend(head.notes("head"));
        head.running
    };
    out.examined = measured_count;

    // 6. Compare against the effective floor.
    if let Some(floor) = explicit_floor {
        if measured_count + settings.tolerance < floor {
            if let Some(ov) = find_test_floor_override(ctx) {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::TEST_COUNT_BELOW_FLOOR),
                    fingerprint: String::new(),
                    title: crate::findings::TEST_COUNT_BELOW_FLOOR.fixed_title().to_string(),
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
    } else {
        // Zero-config ratchet: compare head AST test count against base ref AST test count.
        let base = count_base_workspace_ast_tests(ctx, &filter)?;
        out.notes.extend(base.notes("base"));
        let base_count = base.running;
        if base_count > 0 && measured_count + settings.tolerance < base_count {
            if let Some(ov) = find_test_floor_override(ctx) {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    code: crate::findings::full_code(GATE, &crate::findings::TEST_COUNT_BELOW_FLOOR),
                    fingerprint: String::new(),
                    title: crate::findings::TEST_COUNT_BELOW_FLOOR.fixed_title().to_string(),
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

fn find_test_floor_override(ctx: &Context) -> Option<crate::tokens::OverrideRecord> {
    if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, GATE) {
        return Some(ov);
    }
    if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, "min_tests") {
        return Some(ov);
    }
    if let Ok(changed) = ctx.git.changed_files() {
        let registry = crate::ast::default_registry();
        let v = crate::guards::agent_diff::assert_vocabulary(ctx.config);

        for cf in &changed {
            if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, &cf.path) {
                return Some(ov);
            }
            if cf.old_path != cf.path {
                if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, &cf.old_path) {
                    return Some(ov);
                }
            }
            if let Some(file_name) = cf.path.rsplit('/').next() {
                if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, file_name) {
                    return Some(ov);
                }
            }
            if let Some(stem) = Path::new(&cf.path).file_stem().and_then(|s| s.to_str()) {
                if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, stem) {
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
                                if let Some(ov) =
                                    ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, &t.name)
                                {
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
}
