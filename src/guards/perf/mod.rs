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
//!
//! # Fail-Closed Statistical Contract (G1)
//! 1. Missing baseline artifact -> FAIL (exit 2).
//! 2. Unparseable / garbage artifact (e.g. `{"mean":"n/a"}`) -> FAIL (exit 2).
//! 3. Deleted benchmark artifact -> FAIL (exit 1). Generic `removes:` does not lift benchmark deletion.
//! 4. Missing baseline entry for an existing head benchmark (rename/add) -> FAIL (exit 1).
//! 5. Provenance tracking: Mismatched host/runner tags fail unless `--allow-cross-host-bench` is set.
//! 6. Mathematical bounds: Wall-clock regressions with confidence intervals are evaluated using
//!    conservative interval clearing derived from interval arithmetic; overlapping CIs do not fail.

pub mod bounds;

use self::bounds::{
    evaluate_continuous_regression, ConfidenceInterval, ContinuousEstimate, DiscreteMetric,
};
use super::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::{bail, Context as _, Result};

pub const GATE: &str = "bench-regression";

#[derive(Debug, Clone, PartialEq)]
pub enum MetricValue {
    Discrete(DiscreteMetric),
    Continuous(ContinuousEstimate),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkMetric {
    pub name: String,
    pub count: f64,
    pub value: MetricValue,
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

    let allow_cross = ctx.allow_cross_host_bench || settings.allow_cross_host;

    for file in matching_files {
        // G1(c): Deleted benchmark artifact without a scoped directive -> FAIL (exit 1).
        // A generic `removes:` on the file does NOT silently lift benchmark deletions;
        // benchmark removal requires its own scoped directive (`allow-regression:`).
        if file.kind == ChangeKind::Deleted {
            let subjects = benchmark_subjects(&file.path, "");
            let allowed = subjects
                .iter()
                .find_map(|s| ctx.find_override(GATE, tokens::ALLOW_REGRESSION, s));

            if let Some(ov) = allowed {
                out.overrides.push(ov);
            } else {
                out.push(
                    ctx.overridable(settings.severity),
                    "Benchmark Artifact Deleted",
                    Some(&file.path),
                    None,
                    format!(
                        "benchmark artifact `{}` was deleted without an explicit scoped `allow-regression:` directive (generic `removes:` does not permit benchmark removal)",
                        file.path
                    ),
                    &format!(
                        "restore the benchmark artifact, or justify its deletion on its own line in the PR body or a commit message: `allow-regression: {} <rationale>`",
                        file.path
                    ),
                );
            }
            continue;
        }

        let base_bytes = ctx.git.base_bytes(&file.old_path)?;
        let head_bytes = ctx.git.head_bytes(&file.path)?;

        if file.kind == ChangeKind::Added {
            let subjects = benchmark_subjects(&file.path, "");
            let allowed = subjects
                .iter()
                .find_map(|s| ctx.find_override(GATE, tokens::ALLOW_REGRESSION, s));

            let Some(head_raw) = head_bytes else {
                bail!("missing benchmark artifact `{}` at head", file.path);
            };
            let head_text = String::from_utf8_lossy(&head_raw);
            let _head_metrics = parse_metrics(&file.path, &head_text).with_context(|| {
                format!(
                    "unparseable or malformed benchmark artifact `{}`",
                    file.path
                )
            })?;

            if let Some(ov) = allowed {
                out.overrides.push(ov);
            } else {
                out.push(
                    ctx.overridable(settings.severity),
                    "New Benchmark Artifact Lacks Baseline",
                    Some(&file.path),
                    None,
                    format!(
                        "benchmark artifact `{}` was added without a merge-base baseline; newly added benchmark artifacts require an explicit scoped `allow-regression:` directive",
                        file.path
                    ),
                    &format!(
                        "justify adding the new benchmark baseline on its own line in the PR body or a commit message: `allow-regression: {} <rationale>`",
                        file.path
                    ),
                );
            }
            continue;
        }

        // G1(c): Missing baseline artifact on modified file -> FAIL (exit 2, cannot run).
        let Some(base_raw) = base_bytes else {
            bail!(
                "missing baseline artifact `{}` at merge base; cannot verify performance regression without baseline",
                file.old_path
            );
        };

        let Some(head_raw) = head_bytes else {
            bail!("missing benchmark artifact `{}` at head", file.path);
        };

        let base_text = String::from_utf8_lossy(&base_raw);
        let head_text = String::from_utf8_lossy(&head_raw);

        // G1(c): Unparseable / garbage artifact (`{"mean":"n/a"}`) -> FAIL (exit 2).
        let base_metrics = parse_metrics(&file.old_path, &base_text).with_context(|| {
            format!(
                "unparseable or malformed baseline benchmark artifact `{}`",
                file.old_path
            )
        })?;
        if base_metrics.is_empty() && !base_text.trim().is_empty() {
            bail!(
                "unparseable or malformed baseline benchmark artifact `{}`: no recognized benchmark metrics",
                file.old_path
            );
        }

        let head_metrics = parse_metrics(&file.path, &head_text).with_context(|| {
            format!(
                "unparseable or malformed benchmark artifact `{}`",
                file.path
            )
        })?;
        if head_metrics.is_empty() && !head_text.trim().is_empty() {
            bail!(
                "unparseable or malformed benchmark artifact `{}`: no recognized benchmark metrics",
                file.path
            );
        }

        // G1(d): Provenance tracking
        let base_prov = extract_provenance(&base_text);
        let head_prov = extract_provenance(&head_text);

        if let Some(req) = &ctx.bench_provenance {
            if head_prov.as_deref() != Some(req.as_str()) {
                out.push(
                    ctx.overridable(settings.severity),
                    "Mismatched Benchmark Provenance",
                    Some(&file.path),
                    None,
                    format!(
                        "benchmark artifact `{}` has provenance `{:?}`, which does not match required `--bench-provenance` tag `{}`",
                        file.path, head_prov, req
                    ),
                    "ensure benchmark was run on the required runner or update --bench-provenance",
                );
            }
        }

        if let (Some(b_p), Some(h_p)) = (&base_prov, &head_prov) {
            if b_p != h_p && !allow_cross {
                out.push(
                    ctx.overridable(settings.severity),
                    "Cross-Host Benchmark Comparison Mismatch",
                    Some(&file.path),
                    None,
                    format!(
                        "benchmark artifact `{}` has provenance `{}` while baseline has `{}`; cross-host comparison is statistically invalid measurement noise",
                        file.path, h_p, b_p
                    ),
                    "pass `--allow-cross-host-bench` or set `allow_cross_host = true` to allow cross-host comparison",
                );
            }
        }

        // Check for removed benchmarks within surviving artifact
        for b in &base_metrics {
            if !head_metrics.iter().any(|h| h.name == b.name) {
                let subjects = benchmark_subjects(&file.path, &b.name);
                let allowed = subjects
                    .iter()
                    .find_map(|s| ctx.find_override(GATE, tokens::ALLOW_REGRESSION, s));
                if let Some(ov) = allowed {
                    out.overrides.push(ov);
                } else {
                    out.push(
                        ctx.overridable(settings.severity),
                        "Benchmark Removed",
                        Some(&file.path),
                        None,
                        format!(
                            "benchmark `{}` was removed from `{}` without a scoped `allow-regression:` directive",
                            b.name, file.path
                        ),
                        &format!(
                            "restore benchmark `{}` or add directive `allow-regression: {} <rationale>`",
                            b.name, b.name
                        ),
                    );
                }
            }
        }

        // Check head metrics: missing baseline entries (G1.c) or regressions (G1.e)
        for h in &head_metrics {
            let subjects = benchmark_subjects(&file.path, &h.name);
            let allowed = subjects
                .iter()
                .find_map(|s| ctx.find_override(GATE, tokens::ALLOW_REGRESSION, s));

            let matching_base = base_metrics.iter().find(|b| b.name == h.name);
            let Some(b) = matching_base else {
                // G1(c): Missing base benchmark entry for existing head benchmark (rename, add) -> FAIL (exit 1).
                if let Some(ov) = allowed {
                    out.overrides.push(ov);
                } else {
                    out.push(
                        ctx.overridable(settings.severity),
                        "New or Renamed Benchmark Lacks Baseline",
                        Some(&file.path),
                        None,
                        format!(
                            "benchmark `{}` in `{}` lacks baseline entry in `{}`; missing base benchmark entry requires explicit scoped `allow-regression:` directive",
                            h.name, file.path, file.old_path
                        ),
                        &format!(
                            "add baseline entry for `{}` or add directive `allow-regression: {} <rationale>`",
                            h.name, h.name
                        ),
                    );
                }
                continue;
            };

            // Evaluate regression
            match (&b.value, &h.value) {
                (MetricValue::Discrete(d_base), MetricValue::Discrete(d_head)) => {
                    if d_base.regressed(d_head, settings.tolerance_pct)? {
                        if let Some(ov) = allowed {
                            out.overrides.push(ov);
                        } else {
                            let delta = d_base.delta_pct(d_head)?;
                            out.push(
                                ctx.overridable(settings.severity),
                                "Instruction Count Regressed",
                                Some(&file.path),
                                None,
                                format!(
                                    "deterministic counter `{}` in `{}` regressed by +{:.2}% ({} -> {} {}), exceeding tolerance {:.2}%",
                                    h.name, file.path, delta, d_base.count, d_head.count, h.unit, settings.tolerance_pct
                                ),
                                &format!("optimize `{}` or add directive `allow-regression: {} <rationale>`", h.name, h.name),
                            );
                        }
                    }
                }
                (MetricValue::Continuous(c_base), MetricValue::Continuous(c_head)) => {
                    let decision =
                        evaluate_continuous_regression(c_base, c_head, settings.tolerance_pct)?;
                    if decision.is_regression {
                        if let Some(ov) = allowed {
                            out.overrides.push(ov);
                        } else {
                            out.push(
                                ctx.overridable(settings.severity),
                                "Benchmark Performance Regressed",
                                Some(&file.path),
                                None,
                                format!(
                                    "benchmark `{}` in `{}` regressed: {} (point: {:.2} -> {:.2} {})",
                                    h.name, file.path, decision.note, c_base.point_estimate, c_head.point_estimate, h.unit
                                ),
                                &format!("optimize `{}` or add directive `allow-regression: {} <rationale>`", h.name, h.name),
                            );
                        }
                    } else if decision.method == "not_comparable_no_ci"
                        || decision.point_delta_pct > settings.tolerance_pct
                    {
                        out.notes
                            .push(format!("benchmark `{}`: {}", h.name, decision.note));
                    }
                }
                _ => {
                    bail!(
                        "mismatched metric types between baseline and head for benchmark `{}`",
                        h.name
                    );
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

pub fn extract_provenance(content: &str) -> Option<String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(prov) = val.get("provenance").and_then(|p| p.as_str()) {
            return Some(prov.to_string());
        }
        if let Some(host) = val.get("host").and_then(|h| h.as_str()) {
            return Some(host.to_string());
        }
        if let Some(runner) = val.get("runner").and_then(|r| r.as_str()) {
            return Some(runner.to_string());
        }
        if let Some(ctx) = val.get("context") {
            if let Some(h) = ctx.get("host_name").and_then(|h| h.as_str()) {
                return Some(h.to_string());
            }
        }
    }
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("# provenance:")
            .or_else(|| trimmed.strip_prefix("# host:"))
            .or_else(|| trimmed.strip_prefix("# runner:"))
            .or_else(|| trimmed.strip_prefix("provenance:"))
        {
            let tag = rest.trim();
            if !tag.is_empty() {
                return Some(tag.to_string());
            }
        }
    }
    None
}

pub fn parse_metrics(path: &str, content: &str) -> Result<Vec<BenchmarkMetric>> {
    if path.ends_with(".json") {
        let val = serde_json::from_str::<serde_json::Value>(content)
            .with_context(|| format!("invalid JSON in benchmark artifact `{path}`"))?;
        return parse_json_metrics(&val);
    }
    Ok(parse_text_metrics(content))
}

fn parse_json_metrics(val: &serde_json::Value) -> Result<Vec<BenchmarkMetric>> {
    let mut metrics = Vec::new();

    // 1. Criterion estimates.json format
    if let Some(mean_val) = val.get("mean") {
        if let Some(point) = mean_val.get("point_estimate").and_then(|p| p.as_f64()) {
            let ci = if let Some(ci_obj) = mean_val.get("confidence_interval") {
                let lower = ci_obj
                    .get("lower_bound")
                    .or_else(|| ci_obj.get("lower_limit"))
                    .and_then(|l| l.as_f64());
                let upper = ci_obj
                    .get("upper_bound")
                    .or_else(|| ci_obj.get("upper_limit"))
                    .and_then(|u| u.as_f64());
                let level = ci_obj
                    .get("confidence_level")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.95);
                if let (Some(l), Some(u)) = (lower, upper) {
                    ConfidenceInterval::new(l, u, level).ok()
                } else {
                    None
                }
            } else {
                None
            };
            let est = ContinuousEstimate {
                point_estimate: point,
                ci,
                unit: "ns".to_string(),
            };
            metrics.push(BenchmarkMetric {
                name: "mean".to_string(),
                count: point,
                value: MetricValue::Continuous(est),
                unit: "ns".to_string(),
            });
            return Ok(metrics);
        } else {
            // "mean" exists but has no valid point_estimate (e.g. `{"mean":"n/a"}`)
            bail!("malformed Criterion benchmark artifact: 'mean' is not a valid estimate object");
        }
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
            value: MetricValue::Discrete(DiscreteMetric::new(ir as u64)),
            unit: "Ir".to_string(),
        });
        return Ok(metrics);
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
                let est = ContinuousEstimate::point_only(cpu_time, &unit)?;
                metrics.push(BenchmarkMetric {
                    name,
                    count: cpu_time,
                    value: MetricValue::Continuous(est),
                    unit,
                });
            }
            // pytest-benchmark format: stats.mean (or stats.median)
            else if let Some(mean) = b
                .get("stats")
                .and_then(|s| s.get("mean").or_else(|| s.get("median")))
                .and_then(|v| v.as_f64())
            {
                let est = ContinuousEstimate::point_only(mean, "s")?;
                metrics.push(BenchmarkMetric {
                    name,
                    count: mean,
                    value: MetricValue::Continuous(est),
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
                    value: MetricValue::Discrete(DiscreteMetric::new(count as u64)),
                    unit: "Ir".to_string(),
                });
            }
        }
    }

    Ok(metrics)
}

fn parse_text_metrics(content: &str) -> Vec<BenchmarkMetric> {
    let mut metrics = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // 1. Callgrind summary line: "summary: 1234567"
        if let Some(rest) = trimmed.strip_prefix("summary:") {
            if let Some(first_num) = rest.split_whitespace().next() {
                if let Ok(count) = first_num.parse::<u64>() {
                    metrics.push(BenchmarkMetric {
                        name: "summary".to_string(),
                        count: count as f64,
                        value: MetricValue::Discrete(DiscreteMetric::new(count)),
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
            let clean: String = num_part.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(count) = clean.parse::<u64>() {
                metrics.push(BenchmarkMetric {
                    name: "instructions".to_string(),
                    count: count as f64,
                    value: MetricValue::Discrete(DiscreteMetric::new(count)),
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
                            if let Ok(est) = ContinuousEstimate::point_only(count, "ns/op") {
                                metrics.push(BenchmarkMetric {
                                    name: name.clone(),
                                    count,
                                    value: MetricValue::Continuous(est),
                                    unit: "ns/op".to_string(),
                                });
                            }
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
        let metrics = parse_metrics("target/iai/bench/callgrind.bench.out", sample).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "summary");
        assert_eq!(
            metrics[0].value,
            MetricValue::Discrete(DiscreteMetric::new(4200000))
        );
        assert_eq!(metrics[0].unit, "Ir");
    }

    #[test]
    fn parses_iai_text_instructions() {
        let sample = "Instructions: 1,234,567\n";
        let metrics = parse_metrics("iai_output.log", sample).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(
            metrics[0].value,
            MetricValue::Discrete(DiscreteMetric::new(1234567))
        );
        assert_eq!(metrics[0].unit, "Ir");
    }

    #[test]
    fn parses_criterion_json_estimates() {
        let sample = r#"{"mean": {"point_estimate": 1542.50, "confidence_interval": {"confidence_level": 0.95, "lower_limit": 1500.0, "upper_limit": 1580.0}}}"#;
        let metrics = parse_metrics("target/criterion/bench/estimates.json", sample).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "mean");
        if let MetricValue::Continuous(c) = &metrics[0].value {
            assert_eq!(c.point_estimate, 1542.50);
            assert!(c.ci.is_some());
            let ci = c.ci.unwrap();
            assert_eq!(ci.lower, 1500.0);
            assert_eq!(ci.upper, 1580.0);
        } else {
            panic!("expected continuous metric");
        }
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
        let metrics = parse_metrics("benchmarks/go_bench.txt", sample).unwrap();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "BenchmarkSearch-8");
        assert_eq!(
            metrics[0].value,
            MetricValue::Continuous(ContinuousEstimate::point_only(12.40, "ns/op").unwrap())
        );
        assert_eq!(metrics[0].unit, "ns/op");

        assert_eq!(metrics[1].name, "BenchmarkInsert/small-16");
        assert_eq!(
            metrics[1].value,
            MetricValue::Continuous(ContinuousEstimate::point_only(245.50, "ns/op").unwrap())
        );
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
    "host_name": "worker-1",
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
        let metrics = parse_metrics("build/benchmarks/results.json", sample).unwrap();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "BM_StringCreation");
        assert_eq!(
            metrics[0].value,
            MetricValue::Continuous(ContinuousEstimate::point_only(120.12, "ns").unwrap())
        );
        assert_eq!(metrics[0].unit, "ns");

        assert_eq!(metrics[1].name, "BM_SetInsert/1024");
        assert_eq!(
            metrics[1].value,
            MetricValue::Continuous(ContinuousEstimate::point_only(440.0, "ns").unwrap())
        );
        assert_eq!(metrics[1].unit, "ns");

        assert_eq!(extract_provenance(sample), Some("worker-1".to_string()));

        // Verify subjects extraction
        let subjects = benchmark_subjects("build/benchmarks/results.json", &metrics[1].name);
        assert!(subjects.contains(&"BM_SetInsert/1024".to_string()));
        assert!(subjects.contains(&"BM_SetInsert".to_string()));
    }

    #[test]
    fn parses_pytest_benchmark_json() {
        let sample = r#"{
  "version": "4.0.0",
  "host": "ci-runner-py",
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
        let metrics = parse_metrics("reports/pytest_bench.json", sample).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "test_serialize");
        assert_eq!(
            metrics[0].value,
            MetricValue::Continuous(ContinuousEstimate::point_only(0.000135, "s").unwrap())
        );
        assert_eq!(metrics[0].unit, "s");
        assert_eq!(extract_provenance(sample), Some("ci-runner-py".to_string()));

        // Verify subjects extraction
        let subjects = benchmark_subjects("reports/pytest_bench.json", &metrics[0].name);
        assert!(subjects.contains(&"test_serialize".to_string()));
    }

    #[test]
    fn rejects_malformed_garbage_json() {
        let garbage = r#"{"mean":"n/a"}"#;
        assert!(parse_metrics("target/criterion/bench/estimates.json", garbage).is_err());
    }
}
