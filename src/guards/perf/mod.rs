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
pub mod citation;
pub mod paired_ratio;

use self::bounds::{
    evaluate_continuous_regression, ConfidenceInterval, ContinuousEstimate, DiscreteMetric,
};
use super::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use regex::Regex;
use std::sync::LazyLock;

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
    if settings.mode == crate::config::BenchMode::PairedRatio {
        return paired_ratio::bench_paired_ratio(ctx);
    }

    let env_head = std::env::var("DISCIPLINE_BENCH_HEAD_FILE")
        .ok()
        .filter(|s| !s.is_empty());
    let env_base = std::env::var("DISCIPLINE_BENCH_BASE_FILE")
        .ok()
        .filter(|s| !s.is_empty());

    let head_file = ctx
        .bench_head_file
        .as_deref()
        .and_then(|p| p.to_str())
        .or(env_head.as_deref())
        .or(settings.head_file.as_deref());
    let base_file = ctx
        .bench_base_file
        .as_deref()
        .and_then(|p| p.to_str())
        .or(env_base.as_deref())
        .or(settings.base_file.as_deref());

    if head_file.is_some() || base_file.is_some() {
        return run_dual_file_bench_regression(ctx, settings, base_file, head_file);
    }

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

        evaluate_metrics_regression(
            ctx,
            settings,
            &base_metrics,
            &head_metrics,
            &file.old_path,
            &file.path,
            &mut out,
        )?;
    }

    // Stale exemptions are judged against every arm in the tracked benchmark artifacts at
    // head, not only the changed ones: an entry covering an unchanged artifact is live.
    if !settings.exempt_arms.is_empty() {
        if let Some(arms) = head_arm_names(ctx, &watched, &exempt, &mut out)? {
            report_stale_exempt_arms(&settings.exempt_arms, &arms, None, &mut out)?;
        }
    }

    Ok(out)
}

/// Arm names across every tracked, watched, non-exempt benchmark artifact at head.
/// `None` (with a named note) when an artifact cannot be read or parsed, so staleness
/// cannot be determined.
fn head_arm_names(
    ctx: &Context,
    watched: &PathFilter,
    exempt: &PathFilter,
    out: &mut GateOutcome,
) -> Result<Option<Vec<String>>> {
    let mut arms = Vec::new();
    for path in ctx.git.tracked_files()? {
        if !watched.matches(&path) || exempt.matches(&path) {
            continue;
        }
        let parsed = ctx
            .git
            .head_bytes(&path)?
            .map(|raw| parse_metrics(&path, &String::from_utf8_lossy(&raw)));
        match parsed {
            Some(Ok(metrics)) => arms.extend(metrics.into_iter().map(|m| m.name)),
            _ => {
                out.notes.push(format!(
                    "stale `exempt_arms` check skipped: benchmark artifact `{path}` could not be read or parsed at head"
                ));
                return Ok(None);
            }
        }
    }
    Ok(Some(arms))
}

fn run_dual_file_bench_regression(
    ctx: &Context,
    settings: &crate::config::BenchRegressionGate,
    base_file: Option<&str>,
    head_file: Option<&str>,
) -> Result<GateOutcome> {
    let mut out = GateOutcome::new(GATE);
    out.examined = 1;

    let Some(h_path) = head_file else {
        bail!("missing benchmark artifact at head; base benchmark file was provided without head file");
    };

    if !std::path::Path::new(h_path).exists() {
        bail!("missing benchmark artifact `{h_path}` at head");
    }

    let head_content = std::fs::read_to_string(h_path)
        .with_context(|| format!("failed to read head benchmark file `{h_path}`"))?;
    if head_content.trim().is_empty() {
        bail!("empty benchmark artifact `{h_path}` at head");
    }

    let (base_metrics, head_metrics, base_prov, head_prov, b_path_display) = if let Some(b_path) =
        base_file
    {
        if !std::path::Path::new(b_path).exists() {
            out.push(
                    ctx.overridable(settings.severity),
                    "Missing Benchmark Baseline",
                    Some(b_path),
                    None,
                    "NO BASELINE — regression gate did not run".to_string(),
                    "provide a valid baseline benchmark artifact via --bench-base-file or commit a baseline artifact",
                );
            out.notes
                .push("NO BASELINE — regression gate did not run".to_string());
            return Ok(out);
        }

        let base_content = std::fs::read_to_string(b_path)
            .with_context(|| format!("failed to read base benchmark file `{b_path}`"))?;
        if base_content.trim().is_empty() {
            out.push(
                ctx.overridable(settings.severity),
                "Missing Benchmark Baseline",
                Some(b_path),
                None,
                "NO BASELINE — regression gate did not run".to_string(),
                "base benchmark artifact is empty",
            );
            out.notes
                .push("NO BASELINE — regression gate did not run".to_string());
            return Ok(out);
        }

        let base_metrics = parse_metrics(b_path, &base_content)?;
        if base_metrics.is_empty() {
            out.push(
                ctx.overridable(settings.severity),
                "Missing Benchmark Baseline",
                Some(b_path),
                None,
                "NO BASELINE — regression gate did not run".to_string(),
                "base benchmark artifact contains no recognized benchmark metrics",
            );
            out.notes
                .push("NO BASELINE — regression gate did not run".to_string());
            return Ok(out);
        }

        let head_metrics = parse_metrics(h_path, &head_content)?;
        if head_metrics.is_empty() {
            bail!("unparseable or malformed benchmark artifact `{h_path}`: no recognized benchmark metrics");
        }

        let b_p = extract_provenance(&base_content);
        let h_p = extract_provenance(&head_content);
        (base_metrics, head_metrics, b_p, h_p, b_path.to_string())
    } else {
        // Single file mode with embedded baseline (e.g. iai-callgrind console output)
        let (iai_head, iai_base) = parse_iai_callgrind_console_both(&head_content);
        if !iai_head.is_empty() && !iai_base.is_empty() {
            let h_p = extract_provenance(&head_content);
            (iai_base, iai_head, h_p.clone(), h_p, h_path.to_string())
        } else {
            out.push(
                    ctx.overridable(settings.severity),
                    "Missing Benchmark Baseline",
                    Some(h_path),
                    None,
                    "NO BASELINE — regression gate did not run".to_string(),
                    "provide a valid baseline benchmark artifact via --bench-base-file or commit a baseline artifact",
                );
            out.notes
                .push("NO BASELINE — regression gate did not run".to_string());
            return Ok(out);
        }
    };

    // G1(d): Provenance tracking
    let allow_cross = ctx.allow_cross_host_bench || settings.allow_cross_host;
    if let Some(req) = &ctx.bench_provenance {
        if head_prov.as_deref() != Some(req.as_str()) {
            out.push(
                ctx.overridable(settings.severity),
                "Mismatched Benchmark Provenance",
                Some(h_path),
                None,
                format!(
                    "benchmark artifact `{}` has provenance `{:?}`, which does not match required `--bench-provenance` tag `{}`",
                    h_path, head_prov, req
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
                Some(h_path),
                None,
                format!(
                    "benchmark artifact `{}` has provenance `{}` while baseline has `{}`; cross-host comparison is statistically invalid measurement noise",
                    h_path, h_p, b_p
                ),
                "pass `--allow-cross-host-bench` or set `allow_cross_host = true` to allow cross-host comparison",
            );
        }
    }

    evaluate_metrics_regression(
        ctx,
        settings,
        &base_metrics,
        &head_metrics,
        &b_path_display,
        h_path,
        &mut out,
    )?;

    if !settings.exempt_arms.is_empty() {
        let arms: Vec<String> = head_metrics.iter().map(|m| m.name.clone()).collect();
        report_stale_exempt_arms(&settings.exempt_arms, &arms, Some(h_path), &mut out)?;
    }

    Ok(out)
}

pub fn evaluate_metrics_regression(
    ctx: &Context,
    settings: &crate::config::BenchRegressionGate,
    base_metrics: &[BenchmarkMetric],
    head_metrics: &[BenchmarkMetric],
    base_path: &str,
    head_path: &str,
    out: &mut GateOutcome,
) -> Result<()> {
    evaluate_metrics_regression_with_instruments(
        &ctx.directives,
        settings,
        (base_metrics, head_metrics),
        (base_path, head_path),
        ctx.overridable(settings.severity),
        &citation::LiveInstruments::new(ctx.git),
        out,
    )
}

/// `evaluate_metrics_regression_with_instruments` with no instruments: every citation an
/// override rests on is undecidable, so a sourced override never admits a regression here.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_metrics_regression_with_directives(
    directives: &[crate::tokens::ParsedDirective],
    settings: &crate::config::BenchRegressionGate,
    base_metrics: &[BenchmarkMetric],
    head_metrics: &[BenchmarkMetric],
    base_path: &str,
    head_path: &str,
    severity: crate::config::Severity,
    out: &mut GateOutcome,
) -> Result<()> {
    evaluate_metrics_regression_with_instruments(
        directives,
        settings,
        (base_metrics, head_metrics),
        (base_path, head_path),
        severity,
        &citation::Unavailable,
        out,
    )
}

/// Reports an override whose citations are stale or undecidable, with every regression it
/// would have approved. Returns true when the override must not be admitted.
fn report_unfresh_override(
    report: &citation::FreshnessReport,
    directive: &crate::tokens::ParsedDirective,
    regressions: &[(String, f64, u64, u64, String)],
    head_path: &str,
    severity: crate::config::Severity,
    out: &mut GateOutcome,
) -> bool {
    if report.is_fresh() {
        return false;
    }
    if !report.problems.is_empty() {
        out.push(
            severity,
            "Regression Override Is Void — Citation Does Not Measure This Code",
            Some(head_path),
            None,
            format!(
                "regression override `{}` is void: its citation does not resolve to a measurement of the head being gated: {}",
                directive.reason,
                report.problems.join("; ")
            ),
            "cite a completed run at a commit reachable from this head, or regenerate the cited artifact after the change",
        );
    } else {
        for item in &report.undecidable {
            out.notes.push(format!(
                "allow-regression citation not verified — {item}; the gate stays armed"
            ));
        }
        out.push(
            severity,
            "Regression Override Not Verified — Citation Undecidable",
            Some(head_path),
            None,
            format!(
                "regression override `{}` is not admitted: its citations could not be checked for freshness: {}",
                directive.reason,
                report.undecidable.join("; ")
            ),
            "make `gh` available and authenticated to the job (or configure `citation_measurement_jobs` / `citation_source_paths`), or cite a committed artifact",
        );
    }
    for (arm, delta, base_c, head_c, unit) in regressions {
        out.push(
            severity,
            "Instruction Count Regressed",
            Some(head_path),
            None,
            format!(
                "deterministic counter `{arm}` in `{head_path}` regressed by +{delta:.2}% ({base_c} -> {head_c} {unit}), and the override for it rests on a citation that is not verified fresh"
            ),
            &format!("optimize `{arm}` or cite a fresh measurement"),
        );
    }
    true
}

/// Evaluates base and head metrics; a sourced override is admitted only when every
/// citation it rests on is verified fresh with `instruments`. `metrics` and `paths` are
/// `(base, head)`.
pub fn evaluate_metrics_regression_with_instruments(
    directives: &[crate::tokens::ParsedDirective],
    settings: &crate::config::BenchRegressionGate,
    metrics: (&[BenchmarkMetric], &[BenchmarkMetric]),
    paths: (&str, &str),
    severity: crate::config::Severity,
    instruments: &dyn citation::CitationInstruments,
    out: &mut GateOutcome,
) -> Result<()> {
    let (base_metrics, head_metrics) = metrics;
    let (base_path, head_path) = paths;
    // 1. Check for removed benchmarks within surviving artifact
    for b in base_metrics {
        if !head_metrics.iter().any(|h| h.name == b.name) {
            let subjects = benchmark_subjects(head_path, &b.name);
            let allowed = subjects
                .iter()
                .find_map(|s| tokens::find_override(directives, GATE, tokens::ALLOW_REGRESSION, s));
            if let Some(ov) = allowed {
                out.overrides.push(ov);
            } else {
                out.push(
                    severity,
                    "Benchmark Removed",
                    Some(head_path),
                    None,
                    format!(
                        "benchmark `{}` was removed from `{}` without a scoped `allow-regression:` directive",
                        b.name, head_path
                    ),
                    &format!(
                        "restore benchmark `{}` or add directive `allow-regression: {} <rationale>`",
                        b.name, b.name
                    ),
                );
            }
        }
    }

    // 2. Check head metrics: missing baseline entries (G1.c) or regressions (G1.e)
    let noise_floor = settings.noise_floor_pct.unwrap_or(0.5);
    let advisory_threshold = settings.advisory_pct.unwrap_or(0.1);
    let effective_tolerance = settings.tolerance_pct + settings.noise_margin_pct.unwrap_or(0.0);

    let exemptions = ArmExemptions::new(&settings.exempt_arms)?;
    let mut discrete_regressions: Vec<(String, f64, u64, u64, String)> = Vec::new();

    for h in head_metrics {
        let is_exempt = exemptions.matches(&h.name);

        if is_exempt {
            out.notes.push(format!(
                "benchmark arm `{}` is exempt from regression checks per config",
                h.name
            ));
            continue;
        }

        let matching_base = base_metrics.iter().find(|b| b.name == h.name);
        let Some(b) = matching_base else {
            let subjects = benchmark_subjects(head_path, &h.name);
            let allowed = subjects
                .iter()
                .find_map(|s| tokens::find_override(directives, GATE, tokens::ALLOW_REGRESSION, s));
            if let Some(ov) = allowed {
                out.overrides.push(ov);
            } else {
                out.push(
                    severity,
                    "New or Renamed Benchmark Lacks Baseline",
                    Some(head_path),
                    None,
                    format!(
                        "benchmark `{}` in `{}` lacks baseline entry in `{}`; missing base benchmark entry requires explicit scoped `allow-regression:` directive",
                        h.name, head_path, base_path
                    ),
                    &format!(
                        "add baseline entry for `{}` or add directive `allow-regression: {} <rationale>`",
                        h.name, h.name
                    ),
                );
            }
            continue;
        };

        match (&b.value, &h.value) {
            (MetricValue::Discrete(d_base), MetricValue::Discrete(d_head)) => {
                let delta = d_base.delta_pct(d_head)?;
                if delta > advisory_threshold && delta <= noise_floor {
                    out.notes.push(format!(
                        "advisory notice: benchmark `{}` regressed by +{:.2}% ({} -> {} {}), within noise tolerance",
                        h.name, delta, d_base.count, d_head.count, h.unit
                    ));
                }
                if delta > noise_floor || delta > effective_tolerance {
                    discrete_regressions.push((
                        h.name.clone(),
                        delta,
                        d_base.count,
                        d_head.count,
                        h.unit.clone(),
                    ));
                }
            }
            (MetricValue::Continuous(c_base), MetricValue::Continuous(c_head)) => {
                let decision = evaluate_continuous_regression(c_base, c_head, effective_tolerance)?;

                if let Some(max_cv) = settings.max_noise_cv {
                    if c_base.is_noisy(max_cv) || c_head.is_noisy(max_cv) {
                        let base_cv_str = c_base
                            .cv()
                            .map(|cv| format!("{:.1}%", cv * 100.0))
                            .unwrap_or_else(|| "n/a".into());
                        let head_cv_str = c_head
                            .cv()
                            .map(|cv| format!("{:.1}%", cv * 100.0))
                            .unwrap_or_else(|| "n/a".into());
                        out.notes.push(format!(
                            "warning: benchmark `{}` exhibits high variance (base CV: {base_cv_str}, head CV: {head_cv_str}, exceeds threshold {:.1}%); baseline/head comparison may be noisy",
                            h.name, max_cv * 100.0
                        ));
                    }
                }

                let subjects = benchmark_subjects(head_path, &h.name);
                let allowed = subjects.iter().find_map(|s| {
                    tokens::find_override(directives, GATE, tokens::ALLOW_REGRESSION, s)
                });

                if decision.is_regression {
                    if let Some(ov) = allowed {
                        out.overrides.push(ov);
                    } else {
                        out.push(
                            severity,
                            "Benchmark Performance Regressed",
                            Some(head_path),
                            None,
                            format!(
                                "benchmark `{}` in `{}` regressed: {} (point: {:.2} -> {:.2} {})",
                                h.name,
                                head_path,
                                decision.note,
                                c_base.point_estimate,
                                c_head.point_estimate,
                                h.unit
                            ),
                            &format!(
                                "optimize `{}` or add directive `allow-regression: {} <rationale>`",
                                h.name, h.name
                            ),
                        );
                    }
                } else if decision.method.starts_with("not_comparable")
                    || decision.point_delta_pct > effective_tolerance
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

    if !discrete_regressions.is_empty() {
        let worst_delta = discrete_regressions
            .iter()
            .map(|r| r.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let above_noise_count = discrete_regressions
            .iter()
            .filter(|r| r.1 > noise_floor)
            .count();
        let is_violating = worst_delta > effective_tolerance || above_noise_count >= 2;

        if is_violating {
            let regressed_arm_names: Vec<String> =
                discrete_regressions.iter().map(|r| r.0.clone()).collect();

            if settings.require_sourced_override {
                let regression_directive = directives.iter().find(|d| {
                    tokens::ALLOW_REGRESSION
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(&d.directive))
                });

                match regression_directive {
                    Some(d) => {
                        let citation = tokens::extract_citation(&d.reason);
                        if citation.is_none() {
                            out.push(
                                severity,
                                "Regression Override Is Void — No Resolvable Citation",
                                Some(head_path),
                                None,
                                format!(
                                    "regression override `{}` is void: reason cites no CI run URL and no committed artifact path (AGENTS.md §6)",
                                    d.reason
                                ),
                                "include a CI run URL or committed artifact path in the override reason",
                            );
                            for (arm, delta, base_c, head_c, unit) in &discrete_regressions {
                                out.push(
                                    severity,
                                    "Instruction Count Regressed",
                                    Some(head_path),
                                    None,
                                    format!(
                                        "deterministic counter `{}` in `{}` regressed by +{:.2}% ({} -> {} {}), exceeding tolerance",
                                        arm, head_path, delta, base_c, head_c, unit
                                    ),
                                    &format!("optimize `{}` or provide a valid sourced override", arm),
                                );
                            }
                        } else {
                            let unapproved =
                                tokens::unapproved_regressed_arms(&d.reason, &regressed_arm_names);
                            if unapproved.len() == regressed_arm_names.len() {
                                out.push(
                                    severity,
                                    "Regression Override Is Void — Names No Regressed Arm",
                                    Some(head_path),
                                    None,
                                    format!(
                                        "regression override `{}` is void: reason names none of the regressed arms ({:?}) (AGENTS.md §6)",
                                        d.reason, regressed_arm_names
                                    ),
                                    "explicitly name the regressed benchmark arm(s) in the override reason",
                                );
                                for (arm, delta, base_c, head_c, unit) in &discrete_regressions {
                                    out.push(
                                        severity,
                                        "Instruction Count Regressed",
                                        Some(head_path),
                                        None,
                                        format!(
                                            "deterministic counter `{}` in `{}` regressed by +{:.2}% ({} -> {} {})",
                                            arm, head_path, delta, base_c, head_c, unit
                                        ),
                                        &format!("name `{}` in the override reason", arm),
                                    );
                                }
                            } else if report_unfresh_override(
                                &citation::check_citation_freshness(
                                    &d.reason,
                                    &citation::FreshnessPolicy {
                                        measurement_jobs: &settings.citation_measurement_jobs,
                                        source_paths: &settings.citation_source_paths,
                                    },
                                    instruments,
                                ),
                                d,
                                &discrete_regressions,
                                head_path,
                                severity,
                                out,
                            ) {
                                // Stale or undecidable: the gate stays armed.
                            } else {
                                for (arm, delta, base_c, head_c, unit) in &discrete_regressions {
                                    if unapproved.contains(&arm.as_str()) {
                                        out.push(
                                            severity,
                                            "Instruction Count Regressed (Unapproved Arm)",
                                            Some(head_path),
                                            None,
                                            format!(
                                                "deterministic counter `{}` in `{}` regressed by +{:.2}% ({} -> {} {}), and is not named in override `{}` (AGENTS.md §6)",
                                                arm, head_path, delta, base_c, head_c, unit, d.reason
                                            ),
                                            &format!("name `{}` in the override reason or optimize the benchmark", arm),
                                        );
                                    }
                                }
                                out.overrides.push(crate::tokens::OverrideRecord {
                                    gate: GATE.to_string(),
                                    subject: regressed_arm_names
                                        .iter()
                                        .filter(|a| !unapproved.contains(&a.as_str()))
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    directive: d.directive.clone(),
                                    reason: d.reason.clone(),
                                    source: d.source.clone(),
                                    hidden: d.hidden,
                                });
                                out.notes.push(format!(
                                    "performance regression override acknowledged: {}",
                                    d.reason
                                ));
                            }
                        }
                    }
                    None => {
                        for (arm, delta, base_c, head_c, unit) in &discrete_regressions {
                            out.push(
                                severity,
                                "Instruction Count Regressed",
                                Some(head_path),
                                None,
                                format!(
                                    "deterministic counter `{}` in `{}` regressed by +{:.2}% ({} -> {} {}), exceeding threshold (worst: +{:.2}%, noise floor: {:.2}%)",
                                    arm, head_path, delta, base_c, head_c, unit, worst_delta, noise_floor
                                ),
                                &format!("optimize `{}` or add directive `allow-regression: {} <rationale>`", arm, arm),
                            );
                        }
                    }
                }
            } else {
                for (arm, delta, base_c, head_c, unit) in &discrete_regressions {
                    let subjects = benchmark_subjects(head_path, arm);
                    let allowed = subjects.iter().find_map(|s| {
                        tokens::find_override(directives, GATE, tokens::ALLOW_REGRESSION, s)
                    });
                    if let Some(ov) = allowed {
                        if !out.overrides.iter().any(|o| o.reason == ov.reason) {
                            out.overrides.push(ov);
                        }
                    } else {
                        out.push(
                            severity,
                            "Instruction Count Regressed",
                            Some(head_path),
                            None,
                            format!(
                                "deterministic counter `{}` in `{}` regressed by +{:.2}% ({} -> {} {}), exceeding tolerance {:.2}%",
                                arm, head_path, delta, base_c, head_c, unit, effective_tolerance
                            ),
                            &format!("optimize `{}` or add directive `allow-regression: {} <rationale>`", arm, arm),
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// Printed arm form (`map_get random`, or the full iai header
/// `instructions::cost::map_get random:"random"`) mapped to the reported name `map_get/random`.
static PRINTED_ARM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(?:[\w:]+::)?(\w+)\s+([^:\s"\(]+):?.*$"#).unwrap());

/// One `exempt_arms` entry, in every form it may match under.
struct ArmExemption {
    raw: String,
    /// The entry as written, plus the reported name when the entry is in printed form.
    forms: Vec<String>,
    /// Compiled glob for each form that carries glob metacharacters.
    globs: Vec<globset::GlobMatcher>,
}

impl ArmExemption {
    fn new(raw: &str) -> Result<Self> {
        let mut forms = vec![raw.to_string()];
        if raw.contains(char::is_whitespace) {
            if let Some(cap) = PRINTED_ARM.captures(raw.trim()) {
                let reported = format!("{}/{}", &cap[1], &cap[2]);
                if !forms.contains(&reported) {
                    forms.push(reported);
                }
            }
        }
        let mut globs = Vec::new();
        for form in &forms {
            if form.contains(['*', '?', '[', '{']) {
                let glob = globset::Glob::new(form).with_context(|| {
                    format!("invalid glob `{form}` in `gates.bench-regression.exempt_arms`")
                })?;
                globs.push(glob.compile_matcher());
            }
        }
        Ok(Self {
            raw: raw.to_string(),
            forms,
            globs,
        })
    }

    fn matches(&self, name: &str) -> bool {
        // `::` path suffix (`tests/perf.py::test_x` -> `test_x`).
        let tail = name.rsplit("::").next().unwrap_or(name);
        let literal = self.forms.iter().any(|ex| {
            // exact
            ex == name
                // trailing-wildcard literal prefix
                || ex.strip_suffix('*').is_some_and(|p| name.starts_with(p))
                // path suffix, or its `/` parameter head (`map_get/random` -> `map_get`)
                || tail == ex
                || tail.split('/').next() == Some(ex.as_str())
        });
        literal || self.globs.iter().any(|g| g.is_match(name))
    }
}

/// Compiled `gates.bench-regression.exempt_arms` entries.
///
/// An entry exempts an arm when it is the arm name exactly, a trailing-`*` literal prefix of it,
/// its `::` path suffix or that suffix's `/` parameter head, a glob matching it (`*.heap.*`),
/// or the printed form a benchmark harness shows for it (`map_get random` for `map_get/random`).
pub struct ArmExemptions {
    entries: Vec<ArmExemption>,
}

impl ArmExemptions {
    /// Fails on an entry that is a malformed glob (F6: strict configuration).
    pub fn new(entries: &[String]) -> Result<Self> {
        Ok(Self {
            entries: entries
                .iter()
                .map(|e| ArmExemption::new(e))
                .collect::<Result<_>>()?,
        })
    }

    /// Whether any entry exempts the arm `name`.
    pub fn matches(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.matches(name))
    }

    /// Entries, as written, that exempt none of `arms`.
    pub fn unmatched<'a, I>(&self, arms: I) -> Vec<&str>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let arms: Vec<&str> = arms.into_iter().collect();
        self.entries
            .iter()
            .filter(|e| !arms.iter().any(|a| e.matches(a)))
            .map(|e| e.raw.as_str())
            .collect()
    }
}

/// Reports every `exempt_arms` entry that matches no arm in `arms` as a violation.
///
/// A stale exemption is how a gate quietly stops covering something, so it is an error
/// regardless of the gate's configured severity: it is a deterministic configuration
/// defect, not measurement noise. No directive lifts it; the fix is to edit the entry.
pub fn report_stale_exempt_arms(
    exempt_arms: &[String],
    arms: &[String],
    location: Option<&str>,
    out: &mut GateOutcome,
) -> Result<()> {
    let exemptions = ArmExemptions::new(exempt_arms)?;
    for entry in exemptions.unmatched(arms.iter().map(String::as_str)) {
        out.push(
            crate::config::Severity::Error,
            "Stale Benchmark Arm Exemption",
            location,
            None,
            format!(
                "`gates.bench-regression.exempt_arms` entry `{entry}` matches no benchmark arm in this run ({} arm(s) examined); a stale exemption silently stops covering whatever it was written for",
                arms.len()
            ),
            &format!(
                "remove `{entry}` from `exempt_arms`, or correct it to the arm name as reported or printed by the benchmark"
            ),
        );
    }
    Ok(())
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

static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());
static IAI_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^([\w:]+)::(\w+)(?:\s+([^:\s"\(]+):?.*)?$"#).unwrap());
static IAI_METRIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s{2,}([\w+ ]+):\s+([\d,]+)(?:\|([\d,]+|N/A))?"#).unwrap());

pub fn parse_iai_callgrind_console(content: &str) -> Vec<BenchmarkMetric> {
    parse_iai_callgrind_console_both(content).0
}

pub fn parse_iai_callgrind_console_both(
    content: &str,
) -> (Vec<BenchmarkMetric>, Vec<BenchmarkMetric>) {
    let mut head_metrics = Vec::new();
    let mut base_metrics = Vec::new();
    let clean = ANSI_RE.replace_all(content, "");
    let mut current_bench: Option<String> = None;

    for line in clean.lines() {
        let line_trimmed = line.trim_end();
        if let Some(cap) = IAI_HEADER.captures(line_trimmed) {
            let bench = &cap[2];
            let arg = cap.get(3).map(|m| m.as_str());
            let name = if let Some(a) = arg {
                format!("{bench}/{a}")
            } else {
                bench.to_string()
            };
            current_bench = Some(name);
            continue;
        }

        if let Some(ref bench_name) = current_bench {
            if let Some(cap) = IAI_METRIC.captures(line_trimmed) {
                let metric_name = cap[1].trim();
                if metric_name.eq_ignore_ascii_case("Instructions") {
                    let head_raw: String = cap[2].chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(head_count) = head_raw.parse::<u64>() {
                        head_metrics.push(BenchmarkMetric {
                            name: bench_name.clone(),
                            count: head_count as f64,
                            value: MetricValue::Discrete(DiscreteMetric::new(head_count)),
                            unit: "Ir".to_string(),
                        });
                    }
                    if let Some(base_match) = cap.get(3) {
                        let base_str = base_match.as_str().trim();
                        if base_str != "N/A" {
                            let base_raw: String =
                                base_str.chars().filter(|c| c.is_ascii_digit()).collect();
                            if let Ok(base_count) = base_raw.parse::<u64>() {
                                base_metrics.push(BenchmarkMetric {
                                    name: bench_name.clone(),
                                    count: base_count as f64,
                                    value: MetricValue::Discrete(DiscreteMetric::new(base_count)),
                                    unit: "Ir".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    (head_metrics, base_metrics)
}

pub fn parse_metrics(path: &str, content: &str) -> Result<Vec<BenchmarkMetric>> {
    let trimmed = content.trim_start();
    if path.ends_with(".json") || trimmed.starts_with('{') || trimmed.starts_with('[') {
        let val = serde_json::from_str::<serde_json::Value>(content)
            .with_context(|| format!("invalid JSON in benchmark artifact `{path}`"))?;
        return parse_json_metrics(&val);
    }
    let iai = parse_iai_callgrind_console(content);
    if !iai.is_empty() {
        return Ok(iai);
    }
    Ok(parse_text_metrics(content))
}

fn parse_json_metrics(val: &serde_json::Value) -> Result<Vec<BenchmarkMetric>> {
    let mut metrics = Vec::new();

    // 1b. Wasm-fuel / neutral JSON with arms: {"arms": {"...": 1000}} or {"arms": [{"name": "...", "fuel": 1000}]}
    if let Some(arms_val) = val.get("arms") {
        if let Some(arms_map) = arms_val.as_object() {
            for (name, count_val) in arms_map {
                if let Some(count) = count_val.as_f64() {
                    metrics.push(BenchmarkMetric {
                        name: name.clone(),
                        count,
                        value: MetricValue::Discrete(DiscreteMetric::new(count as u64)),
                        unit: "fuel".to_string(),
                    });
                }
            }
            if !metrics.is_empty() {
                return Ok(metrics);
            }
        } else if let Some(arms_arr) = arms_val.as_array() {
            for arm in arms_arr {
                let name = arm
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("arm")
                    .to_string();
                if let Some(count) = arm
                    .get("fuel")
                    .or_else(|| arm.get("instructions"))
                    .or_else(|| arm.get("ir"))
                    .and_then(|v| v.as_f64())
                {
                    let unit = if arm.get("fuel").is_some() {
                        "fuel"
                    } else {
                        "Ir"
                    };
                    metrics.push(BenchmarkMetric {
                        name,
                        count,
                        value: MetricValue::Discrete(DiscreteMetric::new(count as u64)),
                        unit: unit.to_string(),
                    });
                }
            }
            if !metrics.is_empty() {
                return Ok(metrics);
            }
        }
    }

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
            let std_dev = val
                .get("std_dev")
                .and_then(|s| s.get("point_estimate"))
                .and_then(|p| p.as_f64())
                .or_else(|| mean_val.get("standard_error").and_then(|s| s.as_f64()));
            let mut est = ContinuousEstimate {
                point_estimate: point,
                ci,
                std_dev: None,
                unit: "ns".to_string(),
            };
            if let Some(sd) = std_dev {
                est = est.with_std_dev(sd);
            }
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
                let std_dev = b
                    .get("stddev")
                    .or_else(|| b.get("standard_deviation"))
                    .and_then(|v| v.as_f64())
                    .or_else(|| b.get("cv").and_then(|v| v.as_f64()).map(|cv| cv * cpu_time));
                let mut est = ContinuousEstimate::point_only(cpu_time, &unit)?;
                if let Some(sd) = std_dev {
                    est = est.with_std_dev(sd);
                }
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
                let std_dev = b
                    .get("stats")
                    .and_then(|s| s.get("stddev").or_else(|| s.get("std_dev")))
                    .and_then(|v| v.as_f64());
                let mut est = ContinuousEstimate::point_only(mean, "s")?;
                if let Some(sd) = std_dev {
                    est = est.with_std_dev(sd);
                }
                metrics.push(BenchmarkMetric {
                    name,
                    count: mean,
                    value: MetricValue::Continuous(est),
                    unit: "s".to_string(),
                });
            }
            // Generic / IAI / Callgrind / Wasm-Fuel counts
            else if let Some(count) = b
                .get("instructions")
                .or_else(|| b.get("ir"))
                .or_else(|| b.get("fuel"))
                .and_then(|v| v.as_f64())
            {
                let unit = if b.get("fuel").is_some() {
                    "fuel"
                } else {
                    "Ir"
                };
                metrics.push(BenchmarkMetric {
                    name,
                    count,
                    value: MetricValue::Discrete(DiscreteMetric::new(count as u64)),
                    unit: unit.to_string(),
                });
            }
        }
        if !metrics.is_empty() {
            return Ok(metrics);
        }
    }

    // 4. Object/map of benchmarks (e.g. php-judy / generic custom JSON: {"benchmarks": { "<name>": { "runs_ms": [...], "median_ms": ... } }})
    if let Some(benchmarks_map) = val.get("benchmarks").and_then(|b| b.as_object()) {
        for (name, b) in benchmarks_map {
            let mut unit = "ms".to_string();
            let runs_arr = b
                .get("runs_ms")
                .or_else(|| {
                    if let Some(arr) = b.get("runs_ns") {
                        unit = "ns".to_string();
                        Some(arr)
                    } else if let Some(arr) = b.get("runs_us") {
                        unit = "us".to_string();
                        Some(arr)
                    } else if let Some(arr) = b.get("runs_s") {
                        unit = "s".to_string();
                        Some(arr)
                    } else if let Some(arr) = b.get("runs") {
                        Some(arr)
                    } else {
                        b.get("samples")
                    }
                })
                .and_then(|r| r.as_array());

            let median_val = b
                .get("median_ms")
                .or_else(|| {
                    if let Some(m) = b.get("median_ns") {
                        unit = "ns".to_string();
                        Some(m)
                    } else if let Some(m) = b.get("median_us") {
                        unit = "us".to_string();
                        Some(m)
                    } else if let Some(m) = b.get("median_s") {
                        unit = "s".to_string();
                        Some(m)
                    } else if let Some(m) = b.get("median") {
                        Some(m)
                    } else {
                        b.get("mean")
                    }
                })
                .and_then(|v| v.as_f64());

            // Memory rows (`{"median_ms": 0, "heap_bytes": 160, "rss_bytes": 20480}`) carry a
            // placeholder zero timing. With no usable timing signal they are parsed as a
            // deterministic byte counter so they gate like instruction counts, not as a
            // 0.0 wall-clock estimate that admits no relative delta.
            let timing_usable = median_val.is_some_and(|m| m > 0.0)
                || runs_arr.is_some_and(|r| r.iter().filter_map(|v| v.as_f64()).any(|v| v > 0.0));
            if !timing_usable {
                if let Some(bytes) = memory_bytes(b)? {
                    metrics.push(BenchmarkMetric {
                        name: name.clone(),
                        count: bytes as f64,
                        value: MetricValue::Discrete(DiscreteMetric::new(bytes)),
                        unit: "bytes".to_string(),
                    });
                    continue;
                }
            }

            if let Some(runs) = runs_arr {
                let sample_vec: Vec<f64> = runs.iter().filter_map(|v| v.as_f64()).collect();
                if !sample_vec.is_empty() {
                    let est = ContinuousEstimate::from_samples(&sample_vec, median_val, &unit)?;
                    metrics.push(BenchmarkMetric {
                        name: name.clone(),
                        count: est.point_estimate,
                        value: MetricValue::Continuous(est),
                        unit,
                    });
                    continue;
                }
            }

            if let Some(median) = median_val {
                let est = ContinuousEstimate::point_only(median, &unit)?;
                metrics.push(BenchmarkMetric {
                    name: name.clone(),
                    count: median,
                    value: MetricValue::Continuous(est),
                    unit,
                });
                continue;
            }

            if let Some(count) = b
                .get("instructions")
                .or_else(|| b.get("ir"))
                .or_else(|| b.get("fuel"))
                .and_then(|v| v.as_f64())
            {
                let unit = if b.get("fuel").is_some() {
                    "fuel"
                } else {
                    "Ir"
                };
                metrics.push(BenchmarkMetric {
                    name: name.clone(),
                    count,
                    value: MetricValue::Discrete(DiscreteMetric::new(count as u64)),
                    unit: unit.to_string(),
                });
            }
        }
        if !metrics.is_empty() {
            return Ok(metrics);
        }
    }

    Ok(metrics)
}

/// Memory footprint of a benchmark row, preferring `heap_bytes` (allocator-counted, deterministic)
/// over a generic `bytes` field and `rss_bytes` (resident set size, page-granular).
/// A present field that is not a non-negative integer is malformed.
fn memory_bytes(row: &serde_json::Value) -> Result<Option<u64>> {
    for key in ["heap_bytes", "bytes", "rss_bytes"] {
        if let Some(v) = row.get(key) {
            let Some(n) = v
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0)
            else {
                bail!(
                    "malformed memory benchmark row: `{key}` is not a non-negative integer ({v})"
                );
            };
            return Ok(Some(n as u64));
        }
    }
    Ok(None)
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

    #[test]
    fn parses_iai_callgrind_console_both_test() {
        let sample = "\
cost::map_insert random:\"random\"
  Instructions:               1,050|1,000 (+5.0000%)
  Estimated Cycles:           2,100|2,000 (+5.0000%)
smoke_cost::set_contains
  Instructions:               500|N/A (No baseline)
";
        let (head, base) = parse_iai_callgrind_console_both(sample);
        assert_eq!(head.len(), 2);
        assert_eq!(head[0].name, "map_insert/random");
        assert_eq!(head[0].count, 1050.0);
        assert_eq!(head[1].name, "set_contains");
        assert_eq!(head[1].count, 500.0);

        assert_eq!(base.len(), 1);
        assert_eq!(base[0].name, "map_insert/random");
        assert_eq!(base[0].count, 1000.0);
    }

    fn exempt(entries: &[&str]) -> ArmExemptions {
        let owned: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
        ArmExemptions::new(&owned).unwrap()
    }

    const MIXED_TIMING_AND_MEMORY: &str = r#"{
        "benchmarks": {
            "core.bitset.write.judy": {
                "median_ms": 14.5314,
                "runs_ms": [14.3895, 14.476, 14.5057, 14.5314, 14.5369, 14.538, 14.5991]
            },
            "core.bitset.read.judy": { "median_ms": 5.12 },
            "core.bitset.heap.judy": { "median_ms": 0, "heap_bytes": 160, "rss_bytes": 20480 },
            "core.int_to_int.heap.php": { "median_ms": 0, "rss_bytes": 40960 }
        }
    }"#;

    #[test]
    fn memory_rows_parse_as_discrete_bytes_alongside_timing_rows() {
        let metrics = parse_metrics("baselines/latest.json", MIXED_TIMING_AND_MEMORY).unwrap();
        assert_eq!(metrics.len(), 4);

        let write = metrics
            .iter()
            .find(|m| m.name == "core.bitset.write.judy")
            .unwrap();
        assert!(matches!(write.value, MetricValue::Continuous(_)));
        assert_eq!(write.unit, "ms");

        // heap_bytes is preferred over rss_bytes: allocator counts are deterministic,
        // resident set size carries page granularity.
        let heap = metrics
            .iter()
            .find(|m| m.name == "core.bitset.heap.judy")
            .unwrap();
        assert_eq!(heap.value, MetricValue::Discrete(DiscreteMetric::new(160)));
        assert_eq!(heap.unit, "bytes");

        let rss = metrics
            .iter()
            .find(|m| m.name == "core.int_to_int.heap.php")
            .unwrap();
        assert_eq!(rss.value, MetricValue::Discrete(DiscreteMetric::new(40960)));
        assert_eq!(rss.unit, "bytes");
    }

    #[test]
    fn memory_rows_gate_as_deterministic_counters() {
        use crate::config::{BenchRegressionGate, Severity};
        let settings = BenchRegressionGate {
            tolerance_pct: 5.0,
            ..Default::default()
        };
        let base = parse_metrics("b.json", MIXED_TIMING_AND_MEMORY).unwrap();
        let grown = MIXED_TIMING_AND_MEMORY.replace("\"heap_bytes\": 160", "\"heap_bytes\": 320");
        let head = parse_metrics("h.json", &grown).unwrap();
        let mut out = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &settings,
            &base,
            &head,
            "b.json",
            "h.json",
            Severity::Error,
            &mut out,
        )
        .unwrap();
        assert_eq!(out.violations.len(), 1, "{:?}", out.violations);
        assert!(out.violations[0].message.contains("core.bitset.heap.judy"));
        assert!(out.violations[0].message.contains("160 -> 320 bytes"));

        // Unchanged memory rows evaluate cleanly: no bail on the zero timing field.
        let mut clean = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &settings,
            &base,
            &base,
            "b.json",
            "h.json",
            Severity::Error,
            &mut clean,
        )
        .unwrap();
        assert!(clean.violations.is_empty(), "{:?}", clean.violations);
    }

    #[test]
    fn zero_point_estimate_is_not_comparable_rather_than_a_bail() {
        use crate::config::{BenchRegressionGate, Severity};
        let json = r#"{"benchmarks": {"core.noop.judy": {"median_ms": 0}}}"#;
        let metrics = parse_metrics("b.json", json).unwrap();
        assert!(matches!(metrics[0].value, MetricValue::Continuous(_)));
        let mut out = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &BenchRegressionGate::default(),
            &metrics,
            &metrics,
            "b.json",
            "h.json",
            Severity::Error,
            &mut out,
        )
        .expect("a zero point estimate must not abort the gate");
        assert!(out.violations.is_empty());
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("core.noop.judy") && n.contains("not comparable")),
            "{:?}",
            out.notes
        );
    }

    #[test]
    fn exempt_arms_accepts_globs() {
        let ex = exempt(&["*.heap.*"]);
        assert!(ex.matches("core.bitset.heap.judy"));
        assert!(ex.matches("core.int_to_int.heap.php"));
        assert!(!ex.matches("core.bitset.write.judy"));

        let suffix = exempt(&["*.heap"]);
        assert!(suffix.matches("core.bitset.heap"));
        assert!(!suffix.matches("core.bitset.heap.judy"));

        let path = exempt(&["*::random_*"]);
        assert!(path.matches("tests/test_perf.py::random_lookup"));
        assert!(!path.matches("tests/test_perf.py::sequential_lookup"));
    }

    #[test]
    fn exempt_arms_preserves_existing_forms() {
        // exact
        assert!(exempt(&["map_get/random"]).matches("map_get/random"));
        // trailing-wildcard literal prefix, including glob metacharacters in the prefix
        assert!(exempt(&["map_get*"]).matches("map_get/random"));
        assert!(exempt(&["bm[1]*"]).matches("bm[1]/large"));
        // `::` path suffix and `/` parameter head
        assert!(exempt(&["test_serialize"]).matches("tests/test_perf.py::test_serialize"));
        assert!(exempt(&["map_get"]).matches("map_get/random"));
        // negative controls
        assert!(!exempt(&["map_get"]).matches("map_insert/random"));
        assert!(!exempt(&["map"]).matches("map_get/random"));
    }

    #[test]
    fn exempt_arms_accepts_the_printed_arm_form() {
        let sample = "instructions::cost::map_get random:\"random\"\n  Instructions:               1,050|1,000 (+5.0000%)\n";
        let (head, _) = parse_iai_callgrind_console_both(sample);
        assert_eq!(head[0].name, "map_get/random");

        assert!(exempt(&["map_get random"]).matches(&head[0].name));
        assert!(exempt(&["instructions::cost::map_get random:\"random\""]).matches(&head[0].name));
        assert!(!exempt(&["map_get sequential"]).matches(&head[0].name));
        assert!(!exempt(&["map_insert random"]).matches(&head[0].name));
    }

    #[test]
    fn exempt_arms_rejects_a_malformed_glob() {
        assert!(ArmExemptions::new(&["core.[heap".to_string()]).is_err());
    }

    #[test]
    fn stale_exempt_arm_is_an_error() {
        let arms = vec![
            "map_get/random".to_string(),
            "core.bitset.heap.judy".to_string(),
        ];
        let entries = vec![
            "map_get random".to_string(),
            "*.heap.*".to_string(),
            "set_contains".to_string(),
        ];
        let ex = ArmExemptions::new(&entries).unwrap();
        assert_eq!(
            ex.unmatched(arms.iter().map(String::as_str)),
            vec!["set_contains"]
        );

        let mut out = GateOutcome::new(GATE);
        report_stale_exempt_arms(&entries, &arms, Some("h.json"), &mut out).unwrap();
        assert_eq!(out.violations.len(), 1, "{:?}", out.violations);
        assert_eq!(out.violations[0].title, "Stale Benchmark Arm Exemption");
        assert_eq!(out.violations[0].severity, crate::config::Severity::Error);
        assert!(out.violations[0].message.contains("set_contains"));

        let mut live = GateOutcome::new(GATE);
        report_stale_exempt_arms(&entries[..2], &arms, Some("h.json"), &mut live).unwrap();
        assert!(live.violations.is_empty());
    }

    #[test]
    fn test_evaluate_metrics_regression_two_tier_and_sourced_override() {
        use crate::config::{BenchRegressionGate, Severity};
        use crate::tokens::{OverrideSource, ParsedDirective};

        let settings = BenchRegressionGate {
            tolerance_pct: 5.0,
            noise_floor_pct: Some(0.5),
            advisory_pct: Some(0.1),
            require_sourced_override: true,
            ..Default::default()
        };

        let base = vec![
            BenchmarkMetric {
                name: "sync_map_insert".to_string(),
                count: 1000.0,
                value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                unit: "Ir".to_string(),
            },
            BenchmarkMetric {
                name: "sync_set_insert".to_string(),
                count: 1000.0,
                value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                unit: "Ir".to_string(),
            },
        ];

        // Case 1: single arm at 2% regression (< 5% single-worst, 1 arm > 0.5% noise floor) -> PASS
        let head_single_2pct = vec![
            BenchmarkMetric {
                name: "sync_map_insert".to_string(),
                count: 1020.0, // +2%
                value: MetricValue::Discrete(DiscreteMetric::new(1020)),
                unit: "Ir".to_string(),
            },
            BenchmarkMetric {
                name: "sync_set_insert".to_string(),
                count: 1000.0,
                value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                unit: "Ir".to_string(),
            },
        ];
        let mut out1 = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &settings,
            &base,
            &head_single_2pct,
            "base.txt",
            "head.txt",
            Severity::Error,
            &mut out1,
        )
        .unwrap();
        assert!(
            out1.violations.is_empty(),
            "single 2% regression must pass 5% gate"
        );

        // Case 2: two arms at 2% regression (> 0.5% noise floor, >= 2 arms) -> VIOLATION
        let head_two_2pct = vec![
            BenchmarkMetric {
                name: "sync_map_insert".to_string(),
                count: 1020.0, // +2%
                value: MetricValue::Discrete(DiscreteMetric::new(1020)),
                unit: "Ir".to_string(),
            },
            BenchmarkMetric {
                name: "sync_set_insert".to_string(),
                count: 1020.0, // +2%
                value: MetricValue::Discrete(DiscreteMetric::new(1020)),
                unit: "Ir".to_string(),
            },
        ];
        let mut out2 = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &settings,
            &base,
            &head_two_2pct,
            "base.txt",
            "head.txt",
            Severity::Error,
            &mut out2,
        )
        .unwrap();
        assert_eq!(out2.violations.len(), 2, "two arms > noise floor must fail");

        // Case 3: single arm at 6% regression (> 5% single-worst) -> VIOLATION
        let head_single_6pct = vec![
            BenchmarkMetric {
                name: "sync_map_insert".to_string(),
                count: 1060.0, // +6%
                value: MetricValue::Discrete(DiscreteMetric::new(1060)),
                unit: "Ir".to_string(),
            },
            BenchmarkMetric {
                name: "sync_set_insert".to_string(),
                count: 1000.0,
                value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                unit: "Ir".to_string(),
            },
        ];
        let mut out3 = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &settings,
            &base,
            &head_single_6pct,
            "base.txt",
            "head.txt",
            Severity::Error,
            &mut out3,
        )
        .unwrap();
        assert_eq!(
            out3.violations.len(),
            1,
            "single 6% regression must fail 5% gate"
        );

        // Case 4: Unsourced override -> VOID (fails closed)
        let dir_unsourced = vec![ParsedDirective {
            directive: "allow-regression".to_string(),
            reason: "sync_map_insert is 45.6% faster, trust me".to_string(),
            source: OverrideSource::PrBody,
            hidden: false,
        }];
        let mut out4 = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &dir_unsourced,
            &settings,
            &base,
            &head_single_6pct,
            "base.txt",
            "head.txt",
            Severity::Error,
            &mut out4,
        )
        .unwrap();
        assert!(
            out4.violations
                .iter()
                .any(|v| v.title.contains("No Resolvable Citation")),
            "unsourced override must be void"
        );

        // Case 5: Sourced override that names no regressed arm (#822 shape) -> VOID (fails closed)
        let dir_unnamed = vec![ParsedDirective {
            directive: "allow-regression".to_string(),
            reason:
                "coordination trade refs https://github.com/orieg/expanse/actions/runs/34490311084"
                    .to_string(),
            source: OverrideSource::PrBody,
            hidden: false,
        }];
        let mut out5 = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &dir_unnamed,
            &settings,
            &base,
            &head_single_6pct,
            "base.txt",
            "head.txt",
            Severity::Error,
            &mut out5,
        )
        .unwrap();
        assert!(
            out5.violations
                .iter()
                .any(|v| v.title.contains("Names No Regressed Arm")),
            "unnamed arm override must be void"
        );

        // Case 6: Sourced override naming only subset of regressed arms -> named approved, unnamed fails
        // (the cited run is verified fresh: completed at the head under review)
        let dir_subset = vec![ParsedDirective {
            directive: "allow-regression".to_string(),
            reason: "sync_map_insert pays the bracket refs https://github.com/acme/widgets/actions/runs/4401".to_string(),
            source: OverrideSource::PrBody,
            hidden: false,
        }];
        let canned = |conclusion: &str| citation::CannedInstruments {
            responses: std::collections::BTreeMap::from([
                (
                    "repos/acme/widgets/actions/runs/4401".to_string(),
                    serde_json::json!({"conclusion": conclusion, "head_sha": "abc123"}),
                ),
                (
                    "repos/acme/widgets/compare/abc123...abc123".to_string(),
                    serde_json::json!({"status": "identical"}),
                ),
            ]),
            head: Some("abc123".to_string()),
            ..Default::default()
        };
        let run6 = |instruments: &dyn citation::CitationInstruments| {
            let mut out = GateOutcome::new(GATE);
            evaluate_metrics_regression_with_instruments(
                &dir_subset,
                &settings,
                (&base, &head_two_2pct),
                ("base.txt", "head.txt"),
                Severity::Error,
                instruments,
                &mut out,
            )
            .unwrap();
            out
        };
        let out6 = run6(&canned("success"));
        assert_eq!(out6.overrides.len(), 1, "named arm must be approved");
        assert_eq!(out6.violations.len(), 1, "unnamed arm must fail");
        assert!(out6.violations[0].title.contains("Unapproved Arm"));

        // Case 6b: the same override citing a cancelled run is void, and says which run.
        let stale = run6(&canned("cancelled"));
        assert!(
            stale.overrides.is_empty(),
            "a stale citation admits nothing"
        );
        assert!(stale
            .violations
            .iter()
            .any(|v| v.title.contains("Citation Does Not Measure This Code")
                && v.message.contains("run 4401 concluded `cancelled`")));
        assert_eq!(
            stale
                .violations
                .iter()
                .filter(|v| v.title == "Instruction Count Regressed")
                .count(),
            2,
            "every regression stays armed"
        );

        // Case 6c: no instruments -> undecidable -> named notice, gate stays armed.
        let undecided = run6(&citation::Unavailable);
        assert!(undecided.overrides.is_empty(), "undecidable admits nothing");
        assert!(undecided
            .violations
            .iter()
            .any(|v| v.title.contains("Citation Undecidable")));
        assert!(undecided
            .notes
            .iter()
            .any(|n| n.contains("citation not verified") && n.contains("run 4401")));

        // Case 7: Exempt arm
        let mut settings_exempt = settings.clone();
        settings_exempt.exempt_arms = vec!["sync_set_insert".to_string()];
        let mut out7 = GateOutcome::new(GATE);
        evaluate_metrics_regression_with_directives(
            &[],
            &settings_exempt,
            &base,
            &head_two_2pct,
            "base.txt",
            "head.txt",
            Severity::Error,
            &mut out7,
        )
        .unwrap();
        assert!(
            out7.violations.is_empty(),
            "after exempting one arm, only 1 arm remains > noise floor, passing the gate"
        );
    }

    #[test]
    fn test_parse_metrics_custom_json_sample_array() {
        let json_content = r#"{
            "benchmarks": {
                "core.bitset.write.judy": {
                    "median_ms": 14.5314,
                    "runs_ms": [14.3895, 14.476, 14.5057, 14.5314, 14.5369, 14.538, 14.5991]
                },
                "core.bitset.read.judy": {
                    "median_ms": 5.12
                }
            }
        }"#;
        let metrics = parse_metrics("baselines/latest.json", json_content).unwrap();
        assert_eq!(metrics.len(), 2);

        let write_metric = metrics
            .iter()
            .find(|m| m.name == "core.bitset.write.judy")
            .unwrap();
        assert_eq!(write_metric.unit, "ms");
        assert_eq!(write_metric.count, 14.5314);
        match &write_metric.value {
            MetricValue::Continuous(est) => {
                assert_eq!(est.point_estimate, 14.5314);
                let ci = est.ci.expect("bootstrap CI must be present");
                assert!(ci.lower <= 14.5314);
                assert!(ci.upper >= 14.5314);
            }
            _ => panic!("expected continuous metric"),
        }

        let read_metric = metrics
            .iter()
            .find(|m| m.name == "core.bitset.read.judy")
            .unwrap();
        assert_eq!(read_metric.unit, "ms");
        assert_eq!(read_metric.count, 5.12);
        match &read_metric.value {
            MetricValue::Continuous(est) => {
                assert_eq!(est.point_estimate, 5.12);
                assert!(est.ci.is_none());
            }
            _ => panic!("expected continuous metric"),
        }
    }
}
