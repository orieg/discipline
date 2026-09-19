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
                    if let Some(ov) = ctx
                        .find_gate_or_subject_override(GATE, tokens::ALLOW_TEST_SHRINK, subject)
                        .or_else(|| {
                            ctx.find_gate_or_subject_override(
                                GATE,
                                tokens::ALLOW_TEST_SHRINK,
                                const_file,
                            )
                        })
                    {
                        out.overrides.push(ov);
                    } else {
                        out.violations.push(Violation {
                            gate: GATE,
                            severity: ctx.overridable(settings.severity),
                            title: "Floor Constant Missing in Base Ref".to_string(),
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
                if let Some(ov) =
                    ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_TEST_SHRINK, const_file)
                {
                    out.overrides.push(ov);
                } else {
                    out.violations.push(Violation {
                        gate: GATE,
                        severity: ctx.overridable(settings.severity),
                        title: "Floor Constant File Missing in Base Ref".to_string(),
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
                                if let Some(ov) = ctx
                                    .find_gate_or_subject_override(
                                        GATE,
                                        tokens::ALLOW_TEST_SHRINK,
                                        const_name,
                                    )
                                    .or_else(|| {
                                        ctx.find_gate_or_subject_override(
                                            GATE,
                                            tokens::ALLOW_TEST_SHRINK,
                                            const_file,
                                        )
                                    })
                                {
                                    out.overrides.push(ov);
                                } else {
                                    out.violations.push(Violation {
                                        gate: GATE,
                                        severity: ctx.overridable(settings.severity),
                                        title: "Floor Constant Decreased".to_string(),
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
    if let Some(head_min) = settings.min_tests {
        if let Ok(Some(base_cfg_str)) = ctx.git.base_content(ctx.config_path) {
            if let Ok(base_cfg) = crate::config::DisciplineConfig::from_toml_str(&base_cfg_str) {
                if let Some(base_min) = base_cfg.gates.test_floor.min_tests {
                    if head_min < base_min {
                        if let Some(ov) = ctx
                            .find_gate_or_subject_override(
                                GATE,
                                tokens::ALLOW_TEST_SHRINK,
                                "min_tests",
                            )
                            .or_else(|| {
                                ctx.find_gate_or_subject_override(
                                    GATE,
                                    tokens::ALLOW_TEST_SHRINK,
                                    "test-floor",
                                )
                            })
                        {
                            out.overrides.push(ov);
                        } else {
                            out.violations.push(Violation {
                                gate: GATE,
                                severity: ctx.overridable(settings.severity),
                                title: "Configured Test Floor Decreased".to_string(),
                                file: Some(ctx.config_path.to_string()),
                                line: None,
                                message: format!(
                                    "Test count floor (min_tests = {head_min}) was lowered below base ref ({base_min})."
                                ),
                                remediation: Some(
                                    "Restore min_tests or provide an allow-test-shrink: <reason> directive in the PR description."
                                        .to_string(),
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // 4. Required test suites check.
    for suite in &settings.required_suites {
        let full = Path::new(ctx.git.root()).join(suite);
        if !full.is_file() {
            if let Some(ov) = ctx
                .find_gate_or_subject_override(GATE, tokens::ALLOW_TEST_SHRINK, suite)
                .or_else(|| {
                    ctx.find_gate_or_subject_override(
                        GATE,
                        tokens::ALLOW_TEST_SHRINK,
                        "required_suites",
                    )
                })
            {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    title: "Required Test Suite Missing".to_string(),
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

    // 5. Calculate measured test count.
    let measured_count = if let Some(cmd) = &settings.test_command {
        count_tests_via_command(cmd, Path::new(ctx.git.root()))?
    } else {
        count_workspace_ast_tests(ctx, &filter)?
    };
    out.examined = measured_count;

    // 6. Determine effective floor and compare.
    let effective_floor = settings.min_tests.or(base_floor_const);

    if let Some(floor) = effective_floor {
        if measured_count < floor {
            if let Some(ov) = ctx
                .find_gate_or_subject_override(GATE, tokens::ALLOW_TEST_SHRINK, "test-floor")
                .or_else(|| {
                    ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_TEST_SHRINK, "tests")
                })
            {
                out.overrides.push(ov);
            } else {
                out.violations.push(Violation {
                    gate: GATE,
                    severity: ctx.overridable(settings.severity),
                    title: "Test Count Below Floor".to_string(),
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
        out.notes.push(
            "no test count floor configured and no base ref test count available".to_string(),
        );
    }

    Ok(out)
}

/// Counts test functions across all supported language packs in tracked repository files.
pub fn count_workspace_ast_tests(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
) -> Result<usize> {
    let files = ctx.git.tracked_files()?;
    let registry = crate::ast::default_registry();
    let v = crate::ast::AssertVocabulary::default();
    let mut total = 0;

    for path in files {
        if filter.matches(&path) || !registry.is_supported(&path) {
            continue;
        }
        let full = Path::new(ctx.git.root()).join(&path);
        if let Ok(content) = std::fs::read_to_string(&full) {
            if let Some(pack) = registry.find_pack(&path) {
                if let Ok(facts) = pack.extract(&path, &content, &v) {
                    total += facts.tests.len();
                }
            }
        }
    }
    Ok(total)
}

/// Counts test functions across all supported language packs in base ref.
pub fn count_base_workspace_ast_tests(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
) -> Result<usize> {
    let files = ctx.git.tracked_files()?;
    let registry = crate::ast::default_registry();
    let v = crate::ast::AssertVocabulary::default();
    let mut total = 0;

    for path in files {
        if filter.matches(&path) || !registry.is_supported(&path) {
            continue;
        }
        if let Ok(Some(content)) = ctx.git.base_content(&path) {
            if let Some(pack) = registry.find_pack(&path) {
                if let Ok(facts) = pack.extract(&path, &content, &v) {
                    total += facts.tests.len();
                }
            }
        }
    }
    Ok(total)
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
