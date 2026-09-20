pub mod gitlab;
pub mod junit;
pub mod sarif;

use crate::cli::OutputFormat;
use crate::config::Severity;
use crate::guards::{CheckSummary, Violation};
use crate::style;
use anyhow::{Context, Result};
use std::io::Write;

pub fn format_report_content(
    summary: &CheckSummary,
    format: OutputFormat,
    fail_on_warnings: bool,
    fail_on_overrides: bool,
) -> Result<String> {
    match format {
        OutputFormat::Terminal | OutputFormat::GithubSummary => {
            let mut buf = Vec::new();
            render_terminal_to_writer(&mut buf, summary, fail_on_warnings, fail_on_overrides)?;
            Ok(String::from_utf8_lossy(&buf).to_string())
        }
        OutputFormat::Json => Ok(serde_json::to_string_pretty(summary)?),
        OutputFormat::Junit => Ok(junit::format_junit(summary, fail_on_warnings)),
        OutputFormat::Sarif => Ok(serde_json::to_string_pretty(&sarif::format_sarif(summary))?),
        OutputFormat::Gitlab => Ok(gitlab::format_gitlab(summary)),
        OutputFormat::AgentPrompt => Ok(format_agent_prompt(summary)),
    }
}

pub fn render_report(
    summary: &CheckSummary,
    format: OutputFormat,
    fail_on_warnings: bool,
    fail_on_overrides: bool,
) -> Result<()> {
    match format {
        OutputFormat::Terminal => render_terminal(summary, fail_on_warnings, fail_on_overrides),
        OutputFormat::GithubSummary => {
            render_terminal(summary, fail_on_warnings, fail_on_overrides);
            render_annotations(summary);
            render_step_summary(summary, fail_on_warnings, fail_on_overrides)?;
            render_step_outputs(summary, fail_on_warnings, fail_on_overrides)?;
        }
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(summary)?),
        OutputFormat::Junit => println!("{}", junit::format_junit(summary, fail_on_warnings)),
        OutputFormat::Sarif => {
            println!(
                "{}",
                serde_json::to_string_pretty(&sarif::format_sarif(summary))?
            )
        }
        OutputFormat::Gitlab => println!("{}", gitlab::format_gitlab(summary)),
        OutputFormat::AgentPrompt => print!("{}", format_agent_prompt(summary)),
    }
    Ok(())
}

fn location(v: &Violation) -> Option<String> {
    match (&v.file, v.line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.clone()),
        _ => None,
    }
}

fn render_terminal(summary: &CheckSummary, fail_on_warnings: bool, fail_on_overrides: bool) {
    let _ = render_terminal_to_writer(
        &mut std::io::stdout(),
        summary,
        fail_on_warnings,
        fail_on_overrides,
    );
}

fn render_terminal_to_writer<W: Write>(
    w: &mut W,
    summary: &CheckSummary,
    fail_on_warnings: bool,
    fail_on_overrides: bool,
) -> Result<()> {
    writeln!(w, "\n{}", style::bold("=== Discipline Gate Report ==="))?;
    writeln!(w, "base: {}\n", summary.base)?;

    // The gate table is printed on every run, pass or fail: which gates ran,
    // over how much, and which were off. A pass is only as good as this table.
    writeln!(
        w,
        "{:<24} {:<8} {:>9} {:>11}",
        "GATE", "STATE", "EXAMINED", "VIOLATIONS"
    )?;
    for o in &summary.outcomes {
        // Pad before styling: escape codes would count toward the width.
        let state = if o.enabled {
            style::green(&format!("{:<8}", "on"))
        } else {
            style::yellow(&format!("{:<8}", "OFF"))
        };
        writeln!(
            w,
            "{:<24} {} {:>9} {:>11}",
            o.gate,
            state,
            o.examined,
            o.violations.len()
        )?;
        if o.inline_exemptions > 0 {
            writeln!(
                w,
                "  · {} line(s) exempted by inline `discipline:allow` markers",
                o.inline_exemptions
            )?;
        }
        for ov in &o.overrides {
            writeln!(
                w,
                "  · override applied: `{}: {}` on `{}` ({})",
                ov.directive,
                ov.reason,
                ov.subject,
                match &ov.source {
                    crate::tokens::OverrideSource::PrBody => "PR body".to_string(),
                    crate::tokens::OverrideSource::Commit(oid) => format!("commit {oid}"),
                    crate::tokens::OverrideSource::Inline { file, line } =>
                        format!("{file}:{line}"),
                }
            )?;
        }
        for note in &o.notes {
            writeln!(w, "  · {note}")?;
        }
    }
    if !summary.planned_gates.is_empty() {
        writeln!(
            w,
            "\n{} {}",
            style::dim("not checked (planned gates, not in this version):"),
            style::dim(&summary.planned_gates.join(", "))
        )?;
    }

    for v in summary.violations() {
        let icon = match v.severity {
            Severity::Error => style::red("error"),
            Severity::Warning => style::yellow("warning"),
        };
        let loc = location(v).map(|l| format!(" [{l}]")).unwrap_or_default();
        writeln!(
            w,
            "\n{icon} {} {}{}",
            style::bold(&format!("[{}]", v.gate)),
            style::bold(&v.title),
            style::cyan(&loc)
        )?;
        writeln!(w, "   {}", v.message)?;
        if let Some(rem) = &v.remediation {
            writeln!(w, "   {} {rem}", style::bold("Remediation:"))?;
        }
    }

    let total_ov = summary.total_overrides();
    let (passed, failed, disabled, examined) =
        summary.gate_counts(fail_on_warnings, fail_on_overrides);
    let disabled_suffix = if disabled > 0 {
        format!(", {disabled} disabled")
    } else {
        String::new()
    };
    let items_label = if examined == 1 { "item" } else { "items" };
    writeln!(
        w,
        "\ngates:  {} passed, {} failed{} ({} {} examined)",
        passed, failed, disabled_suffix, examined, items_label
    )?;
    writeln!(
        w,
        "errors: {}  warnings: {}  overrides: {}",
        summary.errors, summary.warnings, total_ov
    )?;
    if fail_on_overrides && total_ov > 0 {
        writeln!(
            w,
            "{}",
            style::red("failure: applied overrides require human sign-off (directives.fail_on_overrides / --fail-on-overrides)")
        )?;
    }
    if summary.is_success(fail_on_warnings, fail_on_overrides) {
        writeln!(w, "{}", style::green("Status: PASS"))?;
    } else {
        writeln!(w, "{}", style::red("Status: FAILED"))?;
    }
    Ok(())
}

/// Workflow-command annotations, understood by GitHub and Gitea runners.
fn render_annotations(summary: &CheckSummary) {
    let escape = |s: &str| {
        s.replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
    };
    let escape_prop = |s: &str| escape(s).replace(':', "%3A").replace(',', "%2C");
    for v in summary.violations() {
        let level = match v.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let mut props = format!(
            "title={}",
            escape_prop(&format!("discipline/{}: {}", v.gate, v.title))
        );
        if let Some(f) = v.file.as_deref().filter(|f| !f.starts_with('<')) {
            props.push_str(&format!(",file={}", escape_prop(f)));
            if let Some(l) = v.line {
                props.push_str(&format!(",line={l}"));
            }
        }
        println!("::{level} {props}::{}", escape(&v.message));
    }
}

fn render_step_summary(
    summary: &CheckSummary,
    fail_on_warnings: bool,
    fail_on_overrides: bool,
) -> Result<()> {
    let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open job summary {path}"))?;
    let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");

    let heading = if summary.is_success(fail_on_warnings, fail_on_overrides) {
        "### Discipline gate: passed"
    } else {
        "### Discipline gate: FAILED"
    };
    writeln!(file, "{heading}\n\nBase: `{}`\n", summary.base)?;

    let (passed, failed, disabled, examined) =
        summary.gate_counts(fail_on_warnings, fail_on_overrides);
    let disabled_suffix = if disabled > 0 {
        format!(", {disabled} disabled")
    } else {
        String::new()
    };
    let items_label = if examined == 1 { "item" } else { "items" };
    let total_ov = summary.total_overrides();
    writeln!(
        file,
        "**Summary:** {} passed, {} failed{} ({} {} examined) · {} errors · {} warnings · {} overrides\n",
        passed, failed, disabled_suffix, examined, items_label, summary.errors, summary.warnings, total_ov
    )?;

    writeln!(
        file,
        "| Gate | State | Examined | Violations |\n|---|---|---:|---:|"
    )?;
    for o in &summary.outcomes {
        writeln!(
            file,
            "| `{}` | {} | {} | {} |",
            o.gate,
            if o.enabled { "on" } else { "**off**" },
            o.examined,
            o.violations.len()
        )?;
    }
    if !summary.planned_gates.is_empty() {
        writeln!(
            file,
            "\nNot checked (planned): {}",
            summary.planned_gates.join(", ")
        )?;
    }
    let total_ov = summary.total_overrides();
    if total_ov > 0 {
        writeln!(
            file,
            "\n#### Overrides Applied ({total_ov})\n\n| Gate | Directive | Subject | Reason | Source |\n|---|---|---|---|---|"
        )?;
        for ov in summary.overrides() {
            let src = match &ov.source {
                crate::tokens::OverrideSource::PrBody => "PR body".to_string(),
                crate::tokens::OverrideSource::Commit(oid) => format!("commit `{oid}`"),
                crate::tokens::OverrideSource::Inline { file, line } => {
                    format!("`{file}:{line}`")
                }
            };
            writeln!(
                file,
                "| `{}` | `{}` | `{}` | {} | {} |",
                ov.gate,
                cell(&ov.directive),
                cell(&ov.subject),
                cell(&ov.reason),
                src
            )?;
        }
    }
    if summary.violations().next().is_some() {
        writeln!(
            file,
            "\n| Severity | Gate | Violation | Location | Details |\n|---|---|---|---|---|"
        )?;
        for v in summary.violations() {
            writeln!(
                file,
                "| {:?} | `{}` | **{}** | {} | {}<br>_{}_ |",
                v.severity,
                v.gate,
                cell(&v.title),
                location(v)
                    .map(|l| format!("`{}`", cell(&l)))
                    .unwrap_or_else(|| "—".into()),
                cell(&v.message),
                cell(v.remediation.as_deref().unwrap_or("—"))
            )?;
        }
    }
    Ok(())
}

/// `errors`, `warnings` and `status` as step outputs, so a workflow can assert
/// on *why* a run failed rather than on the exit code alone.
fn render_step_outputs(
    summary: &CheckSummary,
    fail_on_warnings: bool,
    fail_on_overrides: bool,
) -> Result<()> {
    let Ok(path) = std::env::var("GITHUB_OUTPUT") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open step output file {path}"))?;
    let status = if summary.is_success(fail_on_warnings, fail_on_overrides) {
        "pass"
    } else {
        "fail"
    };
    let mut fired: Vec<&str> = summary.violations().map(|v| v.gate).collect();
    fired.dedup();
    let mut overridden: Vec<&str> = summary.overrides().map(|o| o.gate.as_str()).collect();
    overridden.dedup();
    let (passed, _failed, _disabled, examined) =
        summary.gate_counts(fail_on_warnings, fail_on_overrides);
    writeln!(file, "errors={}", summary.errors)?;
    writeln!(file, "warnings={}", summary.warnings)?;
    writeln!(file, "overrides={}", summary.total_overrides())?;
    writeln!(file, "status={status}")?;
    writeln!(file, "failed_gates={}", fired.join(","))?;
    writeln!(file, "overridden_gates={}", overridden.join(","))?;
    writeln!(file, "passed_gates={passed}")?;
    writeln!(file, "examined_items={examined}")?;
    Ok(())
}

/// Formats check violations into an actionable prompt for autonomous AI coding agents.
/// Guarantees that override directive syntax is never exposed to the agent.
pub fn format_agent_prompt(summary: &CheckSummary) -> String {
    let violations: Vec<&Violation> = summary.violations().collect();

    if violations.is_empty() {
        let (passed, _, _, examined) = summary.gate_counts(false, false);
        let items_label = if examined == 1 { "item" } else { "items" };
        return format!(
            "No discipline violations found ({passed} gates passed, {examined} {items_label} examined).\n"
        );
    }

    let mut out = String::new();
    out.push_str(
        "Discipline gatekeeper detected violations in your changes. Please fix each issue:\n\n",
    );

    for (idx, v) in violations.iter().enumerate() {
        let loc = location(v).unwrap_or_else(|| "global".to_string());
        let repair = repair_action_for_violation(v);
        out.push_str(&format!(
            "### Issue {} [{}]: {}\n- Location: {}\n- Problem: {}\n- Repair: {}\n\n",
            idx + 1,
            v.gate,
            v.title,
            loc,
            v.message,
            repair
        ));
    }

    scrub_override_directives(&out)
}

/// Provides direct, actionable repair guidance for a violation without mentioning escape hatches.
pub fn repair_action_for_violation(v: &Violation) -> String {
    let raw = match v.gate {
        "assertion-reduction" => {
            "Restore the assertions that were removed or weakened to match or exceed the original assertion count.".to_string()
        }
        "vacuous-tests" => {
            "Add substantive assertions that verify the behavior of the unit under test so the test can fail if behavior regresses.".to_string()
        }
        "ignored-tests" => {
            "Remove #[ignore] or skip annotations and fix the test so it passes cleanly.".to_string()
        }
        "unsafe-safety-comment" => {
            "Add a substantive `// SAFETY:` invariant comment directly preceding the unsafe block or impl explaining why the operation is sound.".to_string()
        }
        "deletion-rationale" => {
            "Restore the deleted file or test function.".to_string()
        }
        "time-estimates" => {
            "Remove all time estimates, calendar durations, or sprint projections from the text. Express timelines using ordering, dependencies, or completion gates instead.".to_string()
        }
        "forbidden-words" => {
            "Remove the forbidden term and replace it with precise architectural or technical layer terminology (e.g. engine, runtime, AST parser, memory hierarchy).".to_string()
        }
        "pii" | "host-leaks" => {
            "Remove local absolute paths, usernames, LAN IPs, or private hostnames from the file.".to_string()
        }
        "command" => {
            "Fix the code or configuration so that the verification command passes cleanly.".to_string()
        }
        "dependency-delta" => {
            "Remove the added dependency from the manifest and use existing in-tree dependencies or standard library features.".to_string()
        }
        "test-budget" => {
            "Restore the property test runs, shrink iterations, or fuzzing parameters to meet or exceed previous thresholds.".to_string()
        }
        "bench-regression" => {
            "Optimize the code to eliminate the performance or cycle count regression.".to_string()
        }
        "golden-output" | "golden-tests" => {
            "Restore or regenerate the golden test output to match expected behavior.".to_string()
        }
        "nul-bytes" => {
            "Remove the null bytes from the source file.".to_string()
        }
        "parse-errors" => {
            "Fix the syntax error so that the file parses cleanly.".to_string()
        }
        _ => {
            if let Some(ref rem) = v.remediation {
                if let Some(idx) = rem.find(", or justify") {
                    format!("{}.", &rem[..idx])
                } else if let Some(idx) = rem.find(", or document") {
                    format!("{}.", &rem[..idx])
                } else {
                    rem.clone()
                }
            } else {
                "Fix the violation in the indicated file and line.".to_string()
            }
        }
    };
    scrub_override_directives(&raw)
}

/// Strictly scrubs any override directive syntax, ensuring AI coding agents cannot learn bypass tokens.
pub fn scrub_override_directives(input: &str) -> String {
    let directive_patterns = [
        "allow-assertion-drop",
        "allow-command",
        "allow-dependency",
        "allow-test-shrink",
        "allow-test-budget",
        "allow-gate-weakening",
        "allow-golden-update",
        "allow-nul-byte",
        "allow-nul",
        "allow-corrupt",
        "allow-regression",
        "allow-bench-regression",
        "allow-ignored-test",
        "allow-ignore",
        "allow-vacuous-test",
        "allow-unsafe",
        "discipline:allow",
        "allow(",
        "removes:",
        "deletes:",
    ];

    let mut result = input.to_string();
    for pat in &directive_patterns {
        result = result.replace(pat, "[redacted-directive]");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::guards::GateOutcome;

    #[test]
    fn test_agent_prompt_format_never_emits_directives() {
        let mut o1 = GateOutcome::new("assertion-reduction");
        o1.violations.push(Violation {
            gate: "assertion-reduction",
            severity: Severity::Error,
            title: "Assertion Reduction In Existing Test".into(),
            file: Some("tests/foo.rs".into()),
            line: Some(42),
            message: "Test `test_bar`: effective assertions dropped from 5 to 2.".into(),
            remediation: Some("Restore the assertions, or justify the drop on its own line in the PR body or a commit message: `allow-assertion-drop: test_bar <reason>`.".into()),
        });

        let mut o2 = GateOutcome::new("command");
        o2.violations.push(Violation {
            gate: "command",
            severity: Severity::Error,
            title: "Command Gate Failed".into(),
            file: None,
            line: None,
            message: "Verification command `cargo test` exited with code 101.".into(),
            remediation: Some("Fix command failures, or bypass on its own line in PR body: `allow-command: cargo test <reason>`.".into()),
        });

        let mut o3 = GateOutcome::new("dependency-delta");
        o3.violations.push(Violation {
            gate: "dependency-delta",
            severity: Severity::Error,
            title: "Disallowed Dependency Added".into(),
            file: Some("Cargo.toml".into()),
            line: Some(15),
            message: "Disallowed dependency `tokio` added.".into(),
            remediation: Some(
                "Remove dependency or justify: `allow-dependency: tokio <reason>`.".into(),
            ),
        });

        let mut o4 = GateOutcome::new("deletion-rationale");
        o4.violations.push(Violation {
            gate: "deletion-rationale",
            severity: Severity::Error,
            title: "Undocumented Test Deletion".into(),
            file: Some("tests/old.rs".into()),
            line: Some(1),
            message: "Deleted test `tests/old.rs`.".into(),
            remediation: Some("Revert or document: `removes: tests/old.rs <reason>`.".into()),
        });

        let mut o5 = GateOutcome::new("unsafe-safety-comment");
        o5.violations.push(Violation {
            gate: "unsafe-safety-comment",
            severity: Severity::Error,
            title: "Undocumented Unsafe Block".into(),
            file: Some("src/lib.rs".into()),
            line: Some(10),
            message: "Unsafe block lacks a // SAFETY: comment.".into(),
            remediation: Some(
                "Add // SAFETY: comment or `discipline:allow(unsafe-safety-comment)`.".into(),
            ),
        });

        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 5,
            warnings: 0,
            overrides: 0,
            outcomes: vec![o1, o2, o3, o4, o5],
            planned_gates: vec![],
        };

        let prompt = format_agent_prompt(&summary);

        assert!(prompt.contains("Discipline gatekeeper detected violations"));
        assert!(prompt.contains("Location: tests/foo.rs:42"));
        assert!(
            prompt.contains("Problem: Test `test_bar`: effective assertions dropped from 5 to 2.")
        );
        assert!(prompt.contains("Repair: Restore the assertions"));
        assert!(prompt.contains("Repair: Add a substantive `// SAFETY:` invariant comment"));

        let forbidden_tokens = [
            "allow-assertion-drop",
            "allow-command",
            "allow-dependency",
            "allow-test-shrink",
            "allow-test-budget",
            "allow-gate-weakening",
            "allow-golden-update",
            "allow-nul",
            "allow-regression",
            "allow-ignore",
            "allow-ignored-test",
            "allow-vacuous-test",
            "allow-unsafe",
            "discipline:allow",
            "allow(",
            "removes:",
            "deletes:",
        ];

        for token in forbidden_tokens {
            assert!(
                !prompt.contains(token),
                "agent-prompt leaked forbidden directive token: '{token}'\nFull prompt:\n{prompt}"
            );
        }
    }

    #[test]
    fn test_render_terminal_affirmative_summary() {
        let mut o1 = GateOutcome::new("agents-md");
        o1.examined = 1;

        let mut o2 = GateOutcome::new("pii");
        o2.examined = 42;

        let mut o3 = GateOutcome::new("shell-secrets");
        o3.enabled = false;

        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 0,
            warnings: 0,
            overrides: 0,
            outcomes: vec![o1, o2, o3],
            planned_gates: vec![],
        };

        let mut buf = Vec::new();
        render_terminal_to_writer(&mut buf, &summary, false, false).unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert!(
            out.contains("gates:  2 passed, 0 failed, 1 disabled (43 items examined)"),
            "unexpected gates summary in: {out}"
        );
        assert!(out.contains("errors: 0  warnings: 0  overrides: 0"));
        assert!(out.contains("Status: PASS"));
    }

    #[test]
    fn test_render_terminal_affirmative_summary_singular_and_failed() {
        let mut o1 = GateOutcome::new("agents-md");
        o1.examined = 1;
        o1.violations.push(Violation {
            gate: "agents-md",
            severity: Severity::Error,
            title: "Agents MD Missing".into(),
            file: Some("AGENTS.md".into()),
            line: None,
            message: "Missing AGENTS.md".into(),
            remediation: None,
        });

        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 1,
            warnings: 0,
            overrides: 0,
            outcomes: vec![o1],
            planned_gates: vec![],
        };

        let mut buf = Vec::new();
        render_terminal_to_writer(&mut buf, &summary, false, false).unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert!(
            out.contains("gates:  0 passed, 1 failed (1 item examined)"),
            "unexpected gates summary in: {out}"
        );
        assert!(out.contains("errors: 1  warnings: 0  overrides: 0"));
        assert!(out.contains("Status: FAILED"));
    }

    #[test]
    fn test_agent_prompt_format_clean_summary() {
        let mut o1 = GateOutcome::new("agents-md");
        o1.examined = 1;
        let mut o2 = GateOutcome::new("pii");
        o2.examined = 50;

        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 0,
            warnings: 0,
            overrides: 0,
            outcomes: vec![o1, o2],
            planned_gates: vec![],
        };

        let prompt = format_agent_prompt(&summary);
        assert_eq!(
            prompt,
            "No discipline violations found (2 gates passed, 51 items examined).\n"
        );
    }
}
