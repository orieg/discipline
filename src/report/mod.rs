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
                    crate::tokens::OverrideSource::MergedPrBody(n) =>
                        format!("merged pull request #{n} body"),
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
            Severity::Note => style::cyan("note"),
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
        writeln!(
            w,
            "   {} https://orieg.github.io/discipline/gates/#{}",
            style::bold("Doc:"),
            v.gate
        )?;
    }

    let total_ov = summary.total_overrides();
    let (passed, failed, disabled, examined) =
        summary.gate_counts(fail_on_warnings, fail_on_overrides);
    let mut disabled_suffix = if disabled > 0 {
        format!(", {disabled} disabled")
    } else {
        String::new()
    };
    let not_evaluated = summary.not_evaluated_count();
    if not_evaluated > 0 {
        disabled_suffix.push_str(&format!(", {not_evaluated} not evaluated"));
    }
    let items_label = if examined == 1 { "item" } else { "items" };
    writeln!(
        w,
        "\ngates:  {} passed, {} failed{} ({} {} examined)",
        passed, failed, disabled_suffix, examined, items_label
    )?;
    let baselined_suffix = if summary.baselined > 0 {
        format!("  baselined: {}", summary.baselined)
    } else {
        String::new()
    };
    writeln!(
        w,
        "errors: {}  warnings: {}  overrides: {}{baselined_suffix}",
        summary.errors, summary.warnings, total_ov
    )?;
    if fail_on_overrides && total_ov > 0 {
        writeln!(
            w,
            "{}",
            style::red("failure: applied overrides require human sign-off (directives.fail_on_overrides / --fail-on-overrides)")
        )?;
    }
    for failure in &summary.policy_failures {
        writeln!(w, "{}", style::red(&format!("failure: {failure}")))?;
    }
    for note in &summary.deprecations {
        writeln!(w, "deprecated: {note}")?;
    }
    if summary.is_success(fail_on_warnings, fail_on_overrides) {
        writeln!(w, "{}", style::green("Status: PASS"))?;
    } else {
        writeln!(w, "{}", style::red("Status: FAILED"))?;
        if summary.baselined == 0 && summary.errors > 0 {
            writeln!(
                w,
                "\n{}",
                style::cyan("Tip: fix what this change introduced. Findings in code it did not touch (existing debt when adopting Discipline) can be recorded with 'discipline baseline --write'.")
            )?;
        }
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
            Severity::Note => "notice",
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
    render_step_summary_to_writer(&mut file, summary, fail_on_warnings, fail_on_overrides)
}

pub fn render_step_summary_to_writer(
    mut file: impl std::io::Write,
    summary: &CheckSummary,
    fail_on_warnings: bool,
    fail_on_overrides: bool,
) -> Result<()> {
    let cell_code = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let cell = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('|', "\\|")
            .replace('\n', " ")
    };

    let heading = if summary.is_success(fail_on_warnings, fail_on_overrides) {
        "### Discipline gate: passed"
    } else {
        "### Discipline gate: FAILED"
    };
    writeln!(file, "{heading}\n\nBase: `{}`\n", summary.base)?;
    for failure in &summary.policy_failures {
        writeln!(file, "**Refused:** {failure}\n")?;
    }
    for note in &summary.deprecations {
        writeln!(file, "**Deprecated:** {note}\n")?;
    }

    let (passed, failed, disabled, examined) =
        summary.gate_counts(fail_on_warnings, fail_on_overrides);
    let mut disabled_suffix = if disabled > 0 {
        format!(", {disabled} disabled")
    } else {
        String::new()
    };
    let not_evaluated = summary.not_evaluated_count();
    if not_evaluated > 0 {
        disabled_suffix.push_str(&format!(", {not_evaluated} not evaluated"));
    }
    let items_label = if examined == 1 { "item" } else { "items" };
    let total_ov = summary.total_overrides();
    let baselined_part = if summary.baselined > 0 {
        format!(" · {} baselined", summary.baselined)
    } else {
        String::new()
    };
    writeln!(
        file,
        "**Summary:** {} passed, {} failed{} ({} {} examined) · {} errors · {} warnings · {} overrides{}\n",
        passed, failed, disabled_suffix, examined, items_label, summary.errors, summary.warnings, total_ov, baselined_part
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
                crate::tokens::OverrideSource::MergedPrBody(n) => {
                    format!("merged pull request #{n} body")
                }
            };
            writeln!(
                file,
                "| `{}` | `{}` | `{}` | {} | {} |",
                ov.gate,
                cell_code(&ov.directive),
                cell_code(&ov.subject),
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
                "| {:?} | [`{}`](https://orieg.github.io/discipline/gates/#{}) | **{}** | {} | {}<br>_{}_ |",
                v.severity,
                v.gate,
                v.gate,
                cell(&v.title),
                location(v)
                    .map(|l| format!("`{}`", cell_code(&l)))
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
    fired.sort_unstable();
    fired.dedup();
    let mut overridden: Vec<&str> = summary.overrides().map(|o| o.gate.as_str()).collect();
    overridden.sort_unstable();
    overridden.dedup();
    let (passed, _failed, _disabled, examined) =
        summary.gate_counts(fail_on_warnings, fail_on_overrides);
    writeln!(file, "errors={}", summary.errors)?;
    writeln!(file, "warnings={}", summary.warnings)?;
    writeln!(file, "overrides={}", summary.total_overrides())?;
    writeln!(file, "baselined={}", summary.baselined)?;
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
        let baselined_part = if summary.baselined > 0 {
            format!(" · {} baselined findings not blocking", summary.baselined)
        } else {
            String::new()
        };
        return format!(
            "No discipline violations found ({passed} gates passed, {examined} {items_label} examined{baselined_part}).\n"
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
            v.code,
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
    let raw = repair_for_code(&v.code)
        .or_else(|| repair_for_gate(v.gate))
        .map(str::to_string)
        .unwrap_or_else(|| {
            // A gate without a written repair: its remediation up to the waiver clause.
            match v.remediation.as_deref() {
                Some(rem) => match rem
                    .find(", or justify")
                    .or_else(|| rem.find(", or document"))
                {
                    Some(idx) => format!("{}.", &rem[..idx]),
                    None => rem.to_string(),
                },
                None => "Fix the violation in the indicated file and line.".to_string(),
            }
        });
    scrub_override_directives(&raw)
}

/// The repair for one kind of finding (`crate::findings`), where the gate's own repair
/// would send the agent the wrong way. A repair names the fix, never a waiver, and never
/// tells the agent to regenerate the evidence a finding is about.
pub const REPAIRS: &[(&str, &str)] = &[
    ("assertion-reduction/source-parsed-with-errors", "Fix the syntax error so that the file parses cleanly."),
    ("assertion-reduction/source-parsed-with-errors-preprocessor", "Fix the syntax error so that the file parses cleanly."),
    ("assertion-reduction/nul-byte-added", "Remove the NUL bytes the change added to the file."),
    ("assertion-reduction/assertion-bound-loosened", "Restore the original bound in the assertion; a looser bound accepts results the old one rejected."),
    ("assertion-reduction/mocking-increased-without-stronger-assertions", "Assert on what the code returns or changes, not only on the test doubles the change added."),
    ("assertion-reduction/fatal-assertions-weakened", "Restore the fatal form of the assertions, which stops the test at the first failure."),
    ("vacuous-tests/asserts-only-on-mocks", "Assert on what the code returns or changes, not only on how its test doubles were called."),
    ("vacuous-tests/asserts-only-trivial-properties", "Assert on the value or effect the code produces, not only that something came back."),
    ("ignored-tests/test-sleep-added", "Replace the sleep with a wait on the condition the test depends on."),
    ("ignored-tests/test-retry-added", "Remove the retry and fix the cause of the intermittent failure."),
    ("ignored-tests/skip-justification-insufficient", "Remove the skip and fix the test so it passes."),
    ("dependency-delta/lockfile-deleted", "Restore the deleted lockfile."),
    ("dependency-delta/lockfile-entry-from-new-source", "Restore the package's original source: resolve it from the registry the project already uses."),
    ("dependency-delta/dependency-source-changed", "Restore the package's original source: resolve it from the registry the project already uses."),
    ("dependency-delta/lockfile-integrity-hash-removed", "Regenerate the lockfile from the manifest with the package manager, so every entry keeps its integrity hash."),
    ("dependency-delta/manifest-changed-without-lockfile", "Update the lockfile with the package manager and include it in the change."),
    ("dependency-delta/dependency-constraint-loosened", "Restore the dependency's original version constraint."),
    ("dependency-delta/wildcard-dependency-version", "Pin the dependency to a specific version or range."),
    ("dependency-delta/unpinned-git-dependency", "Pin the git dependency to a specific commit."),
    ("dependency-delta/banned-dependency", "Remove the dependency, or use one the repository already allows."),
    ("dependency-delta/dependency-outside-allowlist", "Remove the dependency, or use one the repository already allows."),
    ("dependency-delta/dependency-outside-deny-allowlist", "Remove the dependency, or use one the repository already allows."),
    ("dependency-delta/unauthorized-git-source", "Remove the dependency, or use one the repository already allows."),
    ("golden-output/golden-output-changed-without-directive", "Restore the golden output to its committed content, and fix the code so it produces that output."),
    ("golden-output/golden-output-regenerated-without-source-change", "Restore the golden output to its committed content, and fix the code so it produces that output."),
    ("golden-output/snapshot-added-for-existing-test", "Assert the expected value in the test itself instead of recording a new snapshot for it."),
    ("bench-regression/benchmark-artifact-deleted", "Restore the deleted benchmark artifact."),
    ("bench-regression/benchmark-removed", "Restore the removed benchmark."),
    ("bench-regression/new-artifact-baseline-missing", "Provide the missing benchmark data: run the benchmark on the base and the head commit with the same harness."),
    ("bench-regression/benchmark-baseline-missing", "Provide the missing benchmark data: run the benchmark on the base and the head commit with the same harness."),
    ("bench-regression/new-or-renamed-arm-baseline-missing", "Provide the missing benchmark data: run the benchmark on the base and the head commit with the same harness."),
    ("bench-regression/paired-ratio-cell-missing", "Provide the missing benchmark data: run the benchmark on the base and the head commit with the same harness."),
    ("bench-regression/paired-ratio-run-missing", "Provide the missing benchmark data: run the benchmark on the base and the head commit with the same harness."),
    ("bench-regression/benchmark-provenance-mismatch", "Compare benchmark results recorded on the same host and toolchain."),
    ("bench-regression/cross-host-comparison", "Compare benchmark results recorded on the same host and toolchain."),
    ("bench-regression/paired-ratio-not-comparable", "Re-run the paired benchmark so that its ratio and its rounds agree."),
    ("bench-regression/paired-ratio-inconsistent-with-rounds", "Re-run the paired benchmark so that its ratio and its rounds agree."),
    ("bench-regression/ratio-baseline-loosened", "Restore the ratio baseline's original values."),
    ("bench-regression/ratio-baseline-changed-with-source", "Leave the ratio baseline unchanged in a change that edits the code it measures."),
    ("bench-regression/stale-arm-exemption", "Remove the benchmark arm exemption that no longer matches any arm."),
    ("bench-regression/override-void-citation-does-not-measure", "Eliminate the performance regression in the code."),
    ("bench-regression/override-unverified-citation-undecidable", "Eliminate the performance regression in the code."),
    ("bench-regression/override-void-no-resolvable-citation", "Eliminate the performance regression in the code."),
    ("bench-regression/override-void-names-no-regressed-arm", "Eliminate the performance regression in the code."),
];

fn repair_for_code(code: &str) -> Option<&'static str> {
    REPAIRS.iter().find(|(c, _)| *c == code).map(|(_, r)| *r)
}

/// The repair shared by every finding of a gate that has one.
fn repair_for_gate(gate: &str) -> Option<&'static str> {
    Some(match gate {
        "assertion-reduction" => {
            "Restore the assertions that were removed or weakened to match or exceed the original assertion count."
        }
        "vacuous-tests" => {
            "Add substantive assertions that verify the behavior of the unit under test so the test can fail if behavior regresses."
        }
        "ignored-tests" => {
            "Remove #[ignore] or skip annotations and fix the test so it passes cleanly."
        }
        "unsafe-safety-comment" => {
            "Add a substantive `// SAFETY:` invariant comment directly preceding the unsafe block or impl explaining why the operation is sound."
        }
        "deletion-rationale" => "Restore the deleted file or test function.",
        "time-estimates" => {
            "Remove all time estimates, calendar durations, or sprint projections from the text. Express timelines using ordering, dependencies, or completion gates instead."
        }
        "pii" => {
            "Remove local absolute paths, usernames, LAN IPs, or private hostnames from the file."
        }
        "command" => {
            "Fix the code or configuration so that the verification command passes cleanly."
        }
        "dependency-delta" => {
            "Remove the added dependency from the manifest and use existing in-tree dependencies or standard library features."
        }
        "test-budget" => {
            "Restore the property test runs, shrink iterations, or fuzzing parameters to meet or exceed previous thresholds."
        }
        "bench-regression" => {
            "Optimize the code to eliminate the performance or cycle count regression."
        }
        _ => return None,
    })
}

/// Strictly scrubs any override directive syntax, ensuring AI coding agents cannot learn bypass tokens.
pub fn scrub_override_directives(input: &str) -> String {
    let mut result = input.to_string();
    for pat in crate::tokens::ALL_DIRECTIVE_NAMES {
        result = result.replace(pat, "[redacted-directive]");
    }
    for extra in &[
        "discipline:allow",
        "allow(",
        "docs-lint: allow",
        "docs-lint:allow",
        "removes:",
        "deletes:",
    ] {
        result = result.replace(extra, "[redacted-directive]");
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
            code: "assertion-reduction/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
            severity: Severity::Error,
            title: "Assertion Count Decreased In Existing Test".into(),
            file: Some("tests/foo.rs".into()),
            line: Some(42),
            message: "Test `test_bar`: effective assertions dropped from 5 to 2.".into(),
            remediation: Some("Restore the assertions, or justify the drop on its own line in the PR body or a commit message: `allow-assertion-drop: test_bar <reason>`.".into()),
        });

        let mut o2 = GateOutcome::new("command");
        o2.violations.push(Violation {
            gate: "command",
            code: "command/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
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
            code: "dependency-delta/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
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
            code: "deletion-rationale/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
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
            code: "unsafe-safety-comment/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
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
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![o1, o2, o3, o4, o5],
            planned_gates: vec![],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
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
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![o1, o2, o3],
            planned_gates: vec![],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
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

    fn finding_of(gate: &'static str, code: &str) -> Violation {
        Violation {
            gate,
            code: format!("{gate}/{code}"),
            fingerprint: String::new(),
            legacy_title: None,
            severity: Severity::Error,
            title: String::new(),
            file: None,
            line: None,
            message: String::new(),
            remediation: Some(
                "Fix it, or justify it on its own line: `allow-gate-weakening: x <reason>`.".into(),
            ),
        }
    }

    #[test]
    fn every_repair_is_keyed_by_a_registered_code() {
        let registered: Vec<String> = crate::findings::FINDINGS
            .iter()
            .map(|k| format!("{}/{}", k.gates[0], k.code))
            .collect();
        let unknown: Vec<&str> = REPAIRS
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| !registered.iter().any(|r| r == c))
            .collect();
        assert_eq!(unknown, Vec::<&str>::new());
    }

    #[test]
    fn no_repair_names_a_waiver_or_tells_the_agent_to_regenerate() {
        for k in crate::findings::FINDINGS {
            let v = finding_of(k.gates[0], k.code);
            let raw = repair_for_code(&v.code).or_else(|| repair_for_gate(v.gate));
            if let Some(raw) = raw {
                assert_eq!(scrub_override_directives(raw), raw, "{}: {raw}", v.code);
                assert!(
                    !raw.to_lowercase().contains("regenerate the golden")
                        && !raw.to_lowercase().contains("or regenerate"),
                    "{}: {raw}",
                    v.code
                );
            }
        }
    }

    #[test]
    fn a_finding_gets_the_repair_for_its_own_kind() {
        let repair = |gate, code| repair_action_for_violation(&finding_of(gate, code));
        assert!(
            repair("dependency-delta", "lockfile-integrity-hash-removed")
                .contains("integrity hash")
        );
        assert!(repair("ignored-tests", "test-sleep-added").contains("sleep"));
        assert!(repair("assertion-reduction", "nul-byte-added").contains("NUL"));
        assert!(repair("assertion-reduction", "source-parsed-with-errors").contains("parses"));
        assert!(
            !repair("golden-output", "golden-output-changed-without-directive")
                .contains("regenerate")
        );
        // No repair of its own: the gate's.
        assert!(repair("dependency-delta", "direct-dependency-added")
            .starts_with("Remove the added dependency"));
        // Neither: the remediation, cut before the waiver clause.
        assert_eq!(repair("agents-md", "agents-md-missing"), "Fix it.");
    }

    #[test]
    fn deprecations_are_reported_and_never_fail_the_run() {
        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 0,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![],
            planned_gates: vec![],
            policy_failures: Vec::new(),
            deprecations: vec!["`gates.x.old` is deprecated".into()],
        };
        let mut buf = Vec::new();
        render_terminal_to_writer(&mut buf, &summary, false, false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("deprecated: `gates.x.old` is deprecated"),
            "{out}"
        );
        assert!(out.contains("Status: PASS"), "{out}");
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["deprecations"][0], "`gates.x.old` is deprecated");
        // An empty list stays out of the report, as policy_failures does.
        let quiet = CheckSummary {
            deprecations: Vec::new(),
            ..summary
        };
        assert!(serde_json::to_value(&quiet)
            .unwrap()
            .get("deprecations")
            .is_none());
    }

    #[test]
    fn test_render_terminal_affirmative_summary_singular_and_failed() {
        let mut o1 = GateOutcome::new("agents-md");
        o1.examined = 1;
        o1.violations.push(Violation {
            gate: "agents-md",
            code: "agents-md/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
            severity: Severity::Error,
            title: "Agents MD Missing".into(),
            file: Some("AGENTS.md".into()),
            line: None,
            message: "AGENTS.md Missing".into(),
            remediation: None,
        });

        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 1,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![o1],
            planned_gates: vec![],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
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
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![o1, o2],
            planned_gates: vec![],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
        };

        let prompt = format_agent_prompt(&summary);
        assert_eq!(
            prompt,
            "No discipline violations found (2 gates passed, 51 items examined).\n"
        );
    }

    #[test]
    fn test_render_step_summary_escapes_html_in_table_cells() {
        let mut o1 = GateOutcome::new("shell-secrets");
        o1.examined = 1;
        o1.violations.push(Violation {
            gate: "shell-secrets",
            code: "shell-secrets/fixture".to_string(),
            fingerprint: String::new(),
            legacy_title: None,
            severity: Severity::Error,
            title: "Secret <Key> Detected".into(),
            file: Some("<pr-body>".into()),
            line: Some(42),
            message: "Contains <token> & raw `value`".into(),
            remediation: Some("Replace <token> with env var".into()),
        });

        let summary = CheckSummary {
            base: "main".to_string(),
            errors: 1,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![o1],
            planned_gates: vec![],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
        };

        let mut buf = Vec::new();
        render_step_summary_to_writer(&mut buf, &summary, false, false).unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert!(
            out.contains("**Secret &lt;Key&gt; Detected**"),
            "expected escaped title in: {out}"
        );
        assert!(
            out.contains(
                "Contains &lt;token&gt; &amp; raw `value`<br>_Replace &lt;token&gt; with env var_"
            ),
            "expected escaped message and remediation in: {out}"
        );
        assert!(
            out.contains("`<pr-body>:42`"),
            "expected code span for location to retain raw backtick content in: {out}"
        );
    }
}
