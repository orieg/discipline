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
    writeln!(
        w,
        "\nerrors: {}  warnings: {}  overrides: {}",
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
    writeln!(file, "errors={}", summary.errors)?;
    writeln!(file, "warnings={}", summary.warnings)?;
    writeln!(file, "overrides={}", summary.total_overrides())?;
    writeln!(file, "status={status}")?;
    writeln!(file, "failed_gates={}", fired.join(","))?;
    writeln!(file, "overridden_gates={}", overridden.join(","))?;
    Ok(())
}
