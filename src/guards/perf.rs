//! Deterministic instruction-count and benchmark regression gate (`bench-regression`).
//!
//! Inspects benchmark artifacts from multiple platforms and languages:
//! - Valgrind Callgrind output files (`callgrind.out.*`, `**/callgrind.*`) across all compiled languages (C, C++, Rust, Zig, Go, etc.)
//! - Rust Criterion benchmark estimates (`target/criterion/**/estimates.json`)
//! - Rust IAI / iai-callgrind instruction summaries
//! - Go benchmark standard output (`BenchmarkSearch-8  100000  12.40 ns/op`)
//! - Python `pytest-benchmark` JSON summaries (`benchmarks[].stats.mean`)
//! - Google Benchmark JSON output (`benchmarks[].cpu_time` or `real_time` with `time_unit`)
//!
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
    pub unit: String,
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

                        let b_disp = if b.count.fract() == 0.0 {
                            format!("{:.0}", b.count)
                        } else if b.count < 0.01 {
                            format!("{:.6}", b.count)
                        } else {
                            format!("{:.2}", b.count)
                        };

                        let h_disp = if h.count.fract() == 0.0 {
                            format!("{:.0}", h.count)
                        } else if h.count < 0.01 {
                            format!("{:.6}", h.count)
                        } else {
                            format!("{:.2}", h.count)
                        };

                        let msg = format!(
                            "Benchmark `{primary_subject}` in `{}` regressed by {delta_pct:.2}% (from {} to {} {}), exceeding the {:.2}% tolerance threshold.",
                            file.path, b_disp, h_disp, h.unit, settings.tolerance_pct
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
    if let Some(rest) = filename.strip_suffix(".log") {
        filename = rest;
    }
    if let Some(rest) = filename.strip_suffix(".txt") {
        filename = rest;
    }
    filename.to_string()
}

pub(crate) fn benchmark_subjects(path: &str, metric_name: &str) -> Vec<String> {
    let mut subjects = Vec::new();
    let stem = file_stem(path);
    if metric_name != "summary"
        && metric_name != "instructions"
        && metric_name != "mean"
        && !metric_name.is_empty()
    {
        subjects.push(metric_name.to_string());
        // For Go benchmarks: "BenchmarkSearch-8" -> also allow "BenchmarkSearch"
        if let Some(idx) = metric_name.rfind('-') {
            let prefix = &metric_name[..idx];
            if prefix.starts_with("Benchmark")
                && metric_name[idx + 1..].chars().all(|c| c.is_ascii_digit())
            {
                subjects.push(prefix.to_string());
            }
        }
        // For parameterized benchmarks with slash (e.g. "BM_SetInsert/1024", "BenchmarkInsert/small-16"):
        if let Some(idx) = metric_name.rfind('/') {
            let prefix = &metric_name[..idx];
            if !prefix.is_empty() {
                subjects.push(prefix.to_string());
            }
        }
        // For python pytest benchmarks (e.g. "tests/test_perf.py::test_serialize"):
        if let Some(idx) = metric_name.rfind("::") {
            let suffix = &metric_name[idx + 2..];
            if !suffix.is_empty() {
                subjects.push(suffix.to_string());
            }
        }
    }
    if !stem.is_empty()
        && stem != "callgrind"
        && stem != "estimates"
        && stem != "benchmarks"
        && stem != "benchmark"
    {
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
            unit: "ns".to_string(),
        });
        return metrics;
    }
    // 2. Single-object iai / callgrind JSON format: {"instructions": 12345} or {"ir": 12345}
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
            unit: "Ir".to_string(),
        });
        return metrics;
    }
    // 3. Array of benchmarks (Google Benchmark, pytest-benchmark, iai array, or top-level array)
    let benchmark_items: Option<&[serde_json::Value]> = if let Some(arr) = val.as_array() {
        Some(arr.as_slice())
    } else {
        val.get("benchmarks")
            .and_then(|b| b.as_array())
            .map(|v| v.as_slice())
    };

    if let Some(benchmarks) = benchmark_items {
        for b in benchmarks {
            let name = b
                .get("name")
                .or_else(|| b.get("fullname"))
                .and_then(|n| n.as_str())
                .unwrap_or("bench")
                .to_string();

            // Google Benchmark format: cpu_time (or real_time) with time_unit
            if let Some(cpu_time) = b
                .get("cpu_time")
                .or_else(|| b.get("real_time"))
                .and_then(|v| v.as_f64())
            {
                let unit = b
                    .get("time_unit")
                    .and_then(|u| u.as_str())
                    .unwrap_or("ns")
                    .to_string();
                metrics.push(BenchmarkMetric {
                    name,
                    count: cpu_time,
                    unit,
                });
            }
            // pytest-benchmark format: stats.mean (or stats.median)
            else if let Some(mean) = b
                .get("stats")
                .and_then(|s| s.get("mean").or_else(|| s.get("median")))
                .and_then(|v| v.as_f64())
            {
                metrics.push(BenchmarkMetric {
                    name,
                    count: mean,
                    unit: "s".to_string(),
                });
            }
            // Generic / IAI / Callgrind instruction counts
            else if let Some(count) = b
                .get("instructions")
                .or_else(|| b.get("ir"))
                .and_then(|v| v.as_f64())
            {
                metrics.push(BenchmarkMetric {
                    name,
                    count,
                    unit: "Ir".to_string(),
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
        // 1. Callgrind summary line: "summary: 1234567"
        if let Some(rest) = trimmed.strip_prefix("summary:") {
            if let Some(first_num) = rest.split_whitespace().next() {
                if let Ok(count) = first_num.parse::<f64>() {
                    metrics.push(BenchmarkMetric {
                        name: "summary".to_string(),
                        count,
                        unit: "Ir".to_string(),
                    });
                }
            }
        }
        // 2. Instructions / Ir label: "Instructions: 1,234,567"
        else if let Some(rest) = trimmed
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
                    unit: "Ir".to_string(),
                });
            }
        }
        // 3. Go benchmark format: "BenchmarkSearch-8   100000   12.40 ns/op"
        else if trimmed.starts_with("Benchmark") {
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            if tokens.len() >= 3 {
                let name = tokens[0].to_string();
                for i in 1..tokens.len() {
                    if tokens[i] == "ns/op" && i > 0 {
                        if let Ok(count) = tokens[i - 1].parse::<f64>() {
                            metrics.push(BenchmarkMetric {
                                name: name.clone(),
                                count,
                                unit: "ns/op".to_string(),
                            });
                            break;
                        }
                    }
                }
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

    #[test]
    fn parses_go_benchmark_text() {
        let sample = "\
goos: darwin
goarch: arm64
pkg: example.com/search
BenchmarkSearch-8              100000             12.40 ns/op               0 B/op          0 allocs/op
BenchmarkInsert/small-16        50000            245.50 ns/op              32 B/op          1 allocs/op
PASS
";
        let metrics = parse_metrics("benchmarks/go_bench.txt", sample);
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "BenchmarkSearch-8");
        assert_eq!(metrics[0].count, 12.40);
        assert_eq!(metrics[0].unit, "ns/op");

        assert_eq!(metrics[1].name, "BenchmarkInsert/small-16");
        assert_eq!(metrics[1].count, 245.50);
        assert_eq!(metrics[1].unit, "ns/op");

        // Verify subjects extraction
        let subjects = benchmark_subjects("benchmarks/go_bench.txt", &metrics[0].name);
        assert!(subjects.contains(&"BenchmarkSearch-8".to_string()));
        assert!(subjects.contains(&"BenchmarkSearch".to_string()));

        let insert_subjects = benchmark_subjects("benchmarks/go_bench.txt", &metrics[1].name);
        assert!(insert_subjects.contains(&"BenchmarkInsert".to_string()));
    }

    #[test]
    fn parses_google_benchmark_json() {
        let sample = r#"{
  "context": {
    "date": "2026-09-18T20:00:00-07:00",
    "num_cpus": 8,
    "mhz_per_cpu": 3200
  },
  "benchmarks": [
    {
      "name": "BM_StringCreation",
      "iterations": 1000,
      "real_time": 123.45,
      "cpu_time": 120.12,
      "time_unit": "ns"
    },
    {
      "name": "BM_SetInsert/1024",
      "iterations": 500,
      "real_time": 450.0,
      "cpu_time": 440.0,
      "time_unit": "ns"
    }
  ]
}"#;
        let metrics = parse_metrics("build/benchmarks/results.json", sample);
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "BM_StringCreation");
        assert_eq!(metrics[0].count, 120.12);
        assert_eq!(metrics[0].unit, "ns");

        assert_eq!(metrics[1].name, "BM_SetInsert/1024");
        assert_eq!(metrics[1].count, 440.0);
        assert_eq!(metrics[1].unit, "ns");

        // Verify subjects extraction
        let subjects = benchmark_subjects("build/benchmarks/results.json", &metrics[1].name);
        assert!(subjects.contains(&"BM_SetInsert/1024".to_string()));
        assert!(subjects.contains(&"BM_SetInsert".to_string()));
    }

    #[test]
    fn parses_pytest_benchmark_json() {
        let sample = r#"{
  "version": "4.0.0",
  "benchmarks": [
    {
      "name": "test_serialize",
      "fullname": "tests/test_perf.py::test_serialize",
      "stats": {
        "mean": 0.000135,
        "median": 0.000132,
        "min": 0.00012,
        "max": 0.00018
      }
    }
  ]
}"#;
        let metrics = parse_metrics("reports/pytest_bench.json", sample);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "test_serialize");
        assert_eq!(metrics[0].count, 0.000135);
        assert_eq!(metrics[0].unit, "s");

        // Verify subjects extraction
        let subjects = benchmark_subjects("reports/pytest_bench.json", &metrics[0].name);
        assert!(subjects.contains(&"test_serialize".to_string()));
    }
}
