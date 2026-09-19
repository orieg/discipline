//! Deterministic instruction-count and benchmark regression gate (`bench-regression`).
//!
//! Inspects callgrind instruction counters (`Ir` instruction reads/cycles from
//! callgrind output files or iai-callgrind summaries) and Criterion benchmarks.
//! Fails builds when instruction cycle counts or execution times regress beyond `tolerance_pct`
//! (default: 0.5%) unless accompanied by an `allow-regression: <arm> <reason>` directive.

use super::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;

pub const GATE: &str = "bench-regression";

#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkMetric {
    pub name: String,
    pub count: f64,
    pub unit: &'static str,
}

pub fn bench_regression(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.bench_regression;
    let exempt = exempt_filter(settings)?;
    let watched = PathFilter::new(&settings.paths)?;

    let mut out = GateOutcome::new(GATE);
    let changed = ctx.git.changed_files()?;

    let matching_files: Vec<_> = changed
        .iter()
        .filter(|f| watched.matches(&f.path) && !exempt.matches(&f.path))
        .collect();

    out.examined = matching_files.len();
    if matching_files.is_empty() {
        out.notes
            .push("no benchmark artifacts modified in this diff".to_string());
        return Ok(out);
    }

    for file in matching_files {
        if file.kind == ChangeKind::Deleted {
            continue;
        }

        let base_bytes = ctx.git.base_bytes(&file.old_path)?;
        let head_bytes = ctx.git.head_bytes(&file.path)?;

        let Some(head_bytes) = head_bytes else {
            continue;
        };
        let head_text = String::from_utf8_lossy(&head_bytes);
        let head_metrics = parse_metrics(&file.path, &head_text);

        let base_metrics = match base_bytes {
            Some(bytes) => {
                let base_text = String::from_utf8_lossy(&bytes);
                parse_metrics(&file.old_path, &base_text)
            }
            None => Vec::new(),
        };

        for h in &head_metrics {
            if let Some(b) = base_metrics.iter().find(|m| m.name == h.name) {
                if b.count > 0.0 && h.count > b.count {
                    let delta_pct = ((h.count - b.count) / b.count) * 100.0;
                    if delta_pct > settings.tolerance_pct {
                        let subjects = benchmark_subjects(&file.path, &h.name);
                        let primary_subject = subjects
                            .first()
                            .cloned()
                            .unwrap_or_else(|| file.path.clone());

                        let allowed = subjects
                            .iter()
                            .find_map(|s| ctx.find_override(GATE, tokens::ALLOW_REGRESSION, s));

                        if let Some(ov) = allowed {
                            out.overrides.push(ov);
                            continue;
                        }

                        let title = if h.unit == "Ir" || h.unit == "instructions" {
                            "Instruction Count Regressed"
                        } else {
                            "Benchmark Performance Regressed"
                        };

                        let msg = format!(
                            "Benchmark `{primary_subject}` in `{}` regressed by {delta_pct:.2}% (from {:.0} to {:.0} {}), exceeding the {:.2}% tolerance threshold.",
                            file.path, b.count, h.count, h.unit, settings.tolerance_pct
                        );

                        let remediation = format!(
                            "Optimize the implementation to restore performance baselines, or justify the regression on its own line in the PR body or a commit message: `allow-regression: {primary_subject} <reason>` (or `discipline:allow({GATE}): {primary_subject} <reason>`)."
                        );

                        out.push(
                            ctx.overridable(settings.severity),
                            title,
                            Some(&file.path),
                            None,
                            msg,
                            &remediation,
                        );
                    }
                }
            }
        }
    }

    Ok(out)
}

fn file_stem(path: &str) -> String {
    let mut filename = path.rsplit('/').next().unwrap_or(path);
    if let Some(rest) = filename.strip_prefix("callgrind.") {
        filename = rest;
    }
    if let Some(rest) = filename.strip_suffix(".out") {
        filename = rest;
    }
    if let Some(rest) = filename.strip_suffix(".json") {
        filename = rest;
    }
    filename.to_string()
}

fn benchmark_subjects(path: &str, metric_name: &str) -> Vec<String> {
    let mut subjects = Vec::new();
    let stem = file_stem(path);
    if metric_name != "summary"
        && metric_name != "instructions"
        && metric_name != "mean"
        && !metric_name.is_empty()
    {
        subjects.push(metric_name.to_string());
    }
    if !stem.is_empty() && stem != "callgrind" && stem != "estimates" {
        subjects.push(stem);
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        if !parent.is_empty() && parent != "iai" && parent != "criterion" && parent != "target" {
            subjects.push(parent.to_string());
        }
    }
    subjects.push(path.to_string());
    subjects
}

pub fn parse_metrics(path: &str, content: &str) -> Vec<BenchmarkMetric> {
    if path.ends_with(".json") {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
            return parse_json_metrics(&val);
        }
    }
    parse_text_metrics(content)
}

fn parse_json_metrics(val: &serde_json::Value) -> Vec<BenchmarkMetric> {
    let mut metrics = Vec::new();
    // 1. Criterion estimates.json format
    if let Some(mean) = val
        .get("mean")
        .and_then(|m| m.get("point_estimate"))
        .and_then(|p| p.as_f64())
    {
        metrics.push(BenchmarkMetric {
            name: "mean".to_string(),
            count: mean,
            unit: "ns",
        });
        return metrics;
    }
    // 2. iai / callgrind JSON format: {"instructions": 12345} or {"ir": 12345}
    if let Some(ir) = val
        .get("instructions")
        .or_else(|| val.get("ir"))
        .and_then(|v| v.as_f64())
    {
        let name = val
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("instructions")
            .to_string();
        metrics.push(BenchmarkMetric {
            name,
            count: ir,
            unit: "Ir",
        });
        return metrics;
    }
    // 3. Nested benchmarks list
    if let Some(benchmarks) = val.get("benchmarks").and_then(|b| b.as_array()) {
        for b in benchmarks {
            let name = b
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("bench")
                .to_string();
            if let Some(count) = b
                .get("instructions")
                .or_else(|| b.get("ir"))
                .and_then(|v| v.as_f64())
            {
                metrics.push(BenchmarkMetric {
                    name,
                    count,
                    unit: "Ir",
                });
            }
        }
    }
    metrics
}

fn parse_text_metrics(content: &str) -> Vec<BenchmarkMetric> {
    let mut metrics = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // Callgrind summary line: "summary: 1234567"
        if let Some(rest) = trimmed.strip_prefix("summary:") {
            if let Some(first_num) = rest.split_whitespace().next() {
                if let Ok(count) = first_num.parse::<f64>() {
                    metrics.push(BenchmarkMetric {
                        name: "summary".to_string(),
                        count,
                        unit: "Ir",
                    });
                }
            }
        } else if let Some(rest) = trimmed
            .strip_prefix("Instructions:")
            .or_else(|| trimmed.strip_prefix("instructions:"))
            .or_else(|| trimmed.strip_prefix("Ir:"))
            .or_else(|| trimmed.strip_prefix("ir:"))
        {
            let num_part = rest.split_whitespace().next().unwrap_or("");
            let clean: String = num_part
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(count) = clean.parse::<f64>() {
                metrics.push(BenchmarkMetric {
                    name: "instructions".to_string(),
                    count,
                    unit: "Ir",
                });
            }
        }
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_callgrind_summary_and_events() {
        let sample = "\
version: 1
creator: callgrind-3.18.1
pid: 12345
cmd: target/release/my_bench
part: 1

events: Ir

summary: 4200000 100
";
        let metrics = parse_metrics("target/iai/bench/callgrind.bench.out", sample);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "summary");
        assert_eq!(metrics[0].count, 4200000.0);
        assert_eq!(metrics[0].unit, "Ir");
    }

    #[test]
    fn parses_iai_text_instructions() {
        let sample = "Instructions: 1,234,567\n";
        let metrics = parse_metrics("iai_output.log", sample);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].count, 1234567.0);
        assert_eq!(metrics[0].unit, "Ir");
    }

    #[test]
    fn parses_criterion_json_estimates() {
        let sample = r#"{"mean": {"point_estimate": 1542.50}}"#;
        let metrics = parse_metrics("target/criterion/bench/estimates.json", sample);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "mean");
        assert_eq!(metrics[0].count, 1542.50);
        assert_eq!(metrics[0].unit, "ns");
    }
}
