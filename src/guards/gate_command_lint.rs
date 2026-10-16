//! Gate command lint sentinel (`gate-command-lint`).
//!
//! Rejects vacuous, masked, or inverted verification commands in plan files.

use crate::guards::{shell_masking, Context, GateOutcome, PathFilter};
use anyhow::Result;

pub const GATE: &str = "gate-command-lint";

#[derive(Debug, Clone)]
pub struct DeclaredGate {
    pub line_num: usize,
    pub command: String,
    pub context: String,
}

/// Extracts declared gate verification commands from a plan file.
///
/// Recognizes:
/// 1. Fenced ```gate code blocks
/// 2. List items prefixed with `- Gate:`, `* Gate:`, `- Verification:`, `* Verification:`,
///    or with parenthesized qualifiers like `- Gate (expected to fail):`
pub fn extract_declared_gates(content: &str) -> Vec<DeclaredGate> {
    let mut gates = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut in_gate_block = false;
    let mut block_comments = Vec::new();

    let list_gate_re =
        regex::Regex::new(r"(?i)^\s*[-*]\s*(?:gate|verification)(?:\s*\([^)]*\))?:\s*(.+)$")
            .unwrap();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            let tag = trimmed.trim_start_matches('`').trim();
            if !in_gate_block
                && tag
                    .split_whitespace()
                    .any(|w| w.eq_ignore_ascii_case("gate"))
            {
                in_gate_block = true;
                block_comments.clear();
                continue;
            } else if in_gate_block {
                in_gate_block = false;
                block_comments.clear();
                continue;
            }
        }

        if in_gate_block {
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('#') {
                block_comments.push(trimmed.to_string());
                continue;
            }
            let mut context = block_comments.join(" ");
            if !context.is_empty() {
                context.push(' ');
            }
            context.push_str(line);
            gates.push(DeclaredGate {
                line_num,
                command: trimmed.to_string(),
                context,
            });
            block_comments.clear();
            continue;
        }

        if let Some(caps) = list_gate_re.captures(line) {
            if let Some(m) = caps.get(1) {
                let cmd = m.as_str().trim();
                gates.push(DeclaredGate {
                    line_num,
                    command: cmd.to_string(),
                    context: line.to_string(),
                });
            }
        }
    }

    gates
}

/// How a declared gate command masks its exit code, read from its bash syntax tree (a
/// gate runs as `sh -c`, `errexit` and `pipefail` off): text in a string or a comment
/// neither masks nor sets an option. `exit_masked()` is an `unfailable-command`,
/// `pipeline_masked()` a `pipeline-exit-masked`.
pub fn analyze_gate_command(command: &str) -> shell_masking::Analysis {
    shell_masking::analyze(command, shell_masking::GATE_COMMAND)
}

/// Evaluates plan files against gate-command-lint invariants.
pub fn evaluate_gate_command_lint(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.gate_command_lint;
    let mut out = GateOutcome::new(GATE);

    let changed = ctx.git.changed_files()?;
    let plan_filter = PathFilter::new(&settings.plan_paths)?;
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    let mut plan_files = Vec::new();
    for file in &changed {
        if file.is_deleted() || exempt.matches(&file.path) || !plan_filter.matches(&file.path) {
            continue;
        }
        plan_files.push(file);
    }

    if plan_files.is_empty() {
        out.examined = 0;
        out.notes.push("no plan files modified in diff".to_string());
        return Ok(out);
    }

    let path_test_re =
        regex::Regex::new(r#"(?:test\s+-[fe]\s+|\[\[?\s+-[fe]\s+)([`"']?[a-zA-Z0-9_./\-]+[`"']?)"#)
            .unwrap();
    let prose_negation_re =
        regex::Regex::new(r"(?i)\b(?:expected\s+to\s+fail|should\s+fail)\b").unwrap();

    for file in plan_files {
        let Some(content) = ctx.git.head_content(&file.path)? else {
            continue;
        };

        let declared_gates = extract_declared_gates(&content);
        if declared_gates.is_empty() {
            out.add_violation(
                ctx.overridable(settings.severity),
                &crate::findings::NO_GATES_DECLARED,
                &file.path,
                1,
                format!("plan file `{}` declares zero verification gates", file.path),
                "plans must include testable verification gates in ```gate blocks or `- Gate: <cmd>` items",
            );
            continue;
        }

        for gate in declared_gates {
            out.examined += 1;
            let analysis = analyze_gate_command(&gate.command);
            if analysis.unparsed {
                out.notes.push(format!(
                    "`{}` line {}: the gate command does not parse as shell, so whether it masks its exit code is not fully verified",
                    file.path, gate.line_num
                ));
            }

            // 1. Unfailable command
            if let Some(mask) = analysis.exit_masked() {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &crate::findings::UNFAILABLE_COMMAND,
                    &file.path,
                    gate.line_num,
                    format!(
                        "gate command has no non-zero exit path (`{}`): `{}`",
                        mask.snippet, gate.command
                    ),
                    "remove the `|| true`, `|| :`, `|| echo`, trailing `; true` or `exit 0` so failure propagates to the runner",
                );
            }

            // 2. Pipeline exit status masking
            if !settings.allow_pipeline_without_pipefail {
                if let Some(mask) = analysis.pipeline_masked() {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        &crate::findings::PIPELINE_EXIT_MASKED,
                        &file.path,
                        gate.line_num,
                        format!(
                            "pipeline exit status is masked by its last stage (`{}`): `{}`",
                            mask.snippet, gate.command
                        ),
                        "run `set -o pipefail` before the pipeline so an upstream failure is not masked",
                    );
                }
            }

            // 3. Pre-existing path test
            for caps in path_test_re.captures_iter(&gate.command) {
                if let Some(m) = caps.get(1) {
                    let raw_path = m.as_str().trim_matches(|c| matches!(c, '`' | '"' | '\''));
                    if ctx.git.has_base() {
                        if let Ok(Some(_)) = ctx.git.base_bytes(raw_path) {
                            out.add_violation(
                                ctx.overridable(settings.severity),
                                &crate::findings::PRE_EXISTING_PATH_TEST,
                                &file.path,
                                gate.line_num,
                                format!(
                                    "file existence check for `{raw_path}` tests a path that already exists at base commit"
                                ),
                                "test -f / test -e on a pre-existing path cannot fail; test a newly created output instead",
                            );
                        }
                    }
                }
            }

            // 4. Prose negation
            if prose_negation_re.is_match(&gate.context)
                || prose_negation_re.is_match(&gate.command)
            {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &crate::findings::PROSE_NEGATION_DETECTED,
                    &file.path,
                    gate.line_num,
                    format!(
                        "prose negation detected beside verification command: `{}`",
                        gate.command
                    ),
                    "use shell negation (`! <cmd>`) instead of prose expectations such as `expected to fail`",
                );
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DisciplineConfig, GateCommandLintGate, Severity};

    #[test]
    fn test_gate_command_lint_defaults() {
        let gate = GateCommandLintGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(!gate.allow_pipeline_without_pipefail);
        assert_eq!(
            gate.plan_paths,
            vec![
                "plans/**/*.md".to_string(),
                "docs/plans/**/*.md".to_string()
            ]
        );
    }

    #[test]
    fn test_extract_declared_gates() {
        let content = r#"
# Plan
- Gate: cargo test --test unit
* Verification: cargo check
- Gate (expected to fail): cargo build
```gate
# expected to fail
cargo test --test integration
cargo clippy
```
"#;
        let gates = extract_declared_gates(content);
        assert_eq!(gates.len(), 5);
        assert_eq!(gates[0].command, "cargo test --test unit");
        assert_eq!(gates[1].command, "cargo check");
        assert_eq!(gates[2].command, "cargo build");
        assert!(gates[2].context.contains("expected to fail"));
        assert_eq!(gates[3].command, "cargo test --test integration");
        assert!(gates[3].context.contains("expected to fail"));
        assert_eq!(gates[4].command, "cargo clippy");
    }

    fn eval(plan_content: &str, base_files: &[(&str, &str)]) -> GateOutcome {
        let toml = r#"
[meta]
version = 1
name = "t"
[gates.gate-command-lint]
enabled = true
"#;
        let config = DisciplineConfig::from_toml_str(toml).unwrap();
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(
            base_files,
            &[("plans/feature.md", plan_content)],
        );
        evaluate_gate_command_lint(&crate::guards::test_support::context(&config, &git)).unwrap()
    }

    #[test]
    fn test_pre_existing_path_test() {
        let bad = "- Gate: test -f config/defaults.json\n";
        let out = eval(bad, &[("config/defaults.json", "{}")]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "gate-command-lint/pre-existing-path-test"
        );

        let good = "- Gate: test -f config/new_schema.json\n";
        let out_good = eval(good, &[("config/defaults.json", "{}")]);
        assert!(out_good.violations.is_empty(), "{:?}", out_good.violations);
    }

    #[test]
    fn test_pipeline_masking() {
        let bad = "- Gate: cargo test --test integration | grep \"passed\"\n";
        let out = eval(bad, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "gate-command-lint/pipeline-exit-masked"
        );

        let good = "- Gate: set -o pipefail; cargo test --test integration | grep \"passed\"\n";
        let out_good = eval(good, &[]);
        assert!(out_good.violations.is_empty(), "{:?}", out_good.violations);
    }

    #[test]
    fn test_unfailable_command() {
        let bad = "- Gate: cargo test || true\n";
        let out = eval(bad, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "gate-command-lint/unfailable-command"
        );

        let good = "- Gate: cargo test\n";
        let out_good = eval(good, &[]);
        assert!(out_good.violations.is_empty());
    }

    #[test]
    fn test_prose_negation() {
        let bad = "- Gate: cargo check # expected to fail\n";
        let out = eval(bad, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "gate-command-lint/prose-negation-detected"
        );

        let good = "- Gate: ! cargo check\n";
        let out_good = eval(good, &[]);
        assert!(out_good.violations.is_empty());
    }

    #[test]
    fn test_no_gates_declared() {
        let empty = "# Plan\nSome narrative without gates.\n";
        let out = eval(empty, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "gate-command-lint/no-gates-declared"
        );
    }

    #[test]
    fn test_motivating_example_verbatim() {
        let content = "- Gate: cargo test --test integration | grep \"passed\" || true\n";
        let out = eval(content, &[]);
        // Triggers both unfailable-command and pipeline-exit-masked
        assert_eq!(out.violations.len(), 2);
        let codes: Vec<_> = out.violations.iter().map(|v| v.code.as_str()).collect();
        assert!(codes.contains(&"gate-command-lint/unfailable-command"));
        assert!(codes.contains(&"gate-command-lint/pipeline-exit-masked"));
    }

    #[test]
    fn test_unfailable_evasion_variants() {
        for cmd in &[
            "- Gate: cargo test || true # with comment\n",
            "- Gate: cargo test ; true\n",
            "- Gate: cargo test ; exit 0\n",
            "- Gate: true\n",
            "- Gate: :\n",
            "- Gate: cargo test || :\n",
            "- Gate: cargo test ; :\n",
        ] {
            let out = eval(cmd, &[]);
            let codes: Vec<_> = out.violations.iter().map(|v| v.code.as_str()).collect();
            assert!(
                codes.contains(&"gate-command-lint/unfailable-command"),
                "command `{cmd}` must trigger unfailable-command: {codes:?}"
            );
        }
    }

    fn codes(cmd: &str) -> Vec<String> {
        eval(&format!("- Gate: {cmd}\n"), &[])
            .violations
            .iter()
            .map(|v| v.code.clone())
            .collect()
    }

    /// Forms the old trailing-`true` pattern let through (audit M6).
    #[test]
    fn test_unfailable_bypasses_are_caught() {
        for cmd in [
            "cargo test || echo ok",
            "cargo test || /bin/true",
            "cargo test || /usr/bin/true",
            "(cargo test || true)",
            "cargo test || { true; }",
            "cargo test || exit 00",
            "cargo test ||true",
            "cargo test || printf 'failed'",
        ] {
            assert_eq!(
                codes(cmd),
                vec!["gate-command-lint/unfailable-command".to_string()],
                "`{cmd}`"
            );
        }
    }

    #[test]
    fn test_failure_paths_are_not_unfailable() {
        for cmd in [
            "cargo test || exit 1",
            "cargo test || { echo failed; exit 1; }",
            "cargo test || echo failed && exit 1",
            "cargo test # || true",
            "grep -q '|| true' notes.txt",
        ] {
            assert!(codes(cmd).is_empty(), "`{cmd}`: {:?}", codes(cmd));
        }
    }

    #[test]
    fn test_pipefail_must_be_a_command_and_any_filter_counts() {
        for cmd in [
            "echo \"set -o pipefail\"; cargo test | grep passed",
            "cargo test | xargs echo",
            "cargo test | python3 summarize.py",
        ] {
            assert_eq!(
                codes(cmd),
                vec!["gate-command-lint/pipeline-exit-masked".to_string()],
                "`{cmd}`"
            );
        }
        for cmd in [
            "set -euo pipefail; cargo test | xargs echo",
            "echo y | cargo test",
        ] {
            assert!(codes(cmd).is_empty(), "`{cmd}`: {:?}", codes(cmd));
        }
    }

    #[test]
    fn test_unparsable_command_is_noted_not_passed() {
        let out = eval("- Gate: cargo test || (\n", &[]);
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("does not parse as shell")),
            "{:?}",
            out.notes
        );
    }
}
