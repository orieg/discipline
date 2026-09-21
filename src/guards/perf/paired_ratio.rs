//! `paired-ratio` evaluation mode of the `bench-regression` gate.
//!
//! # Why a ratio of two arms measured in the same rounds
//!
//! Absolute wall-clock numbers taken on shared CI runners cannot be compared across runs:
//! a runner that is 40% slower than yesterday's makes every number 40% slower. What
//! survives a change of runner is a ratio of two arms measured seconds apart in the same
//! interleaved rounds, with the arm order alternating: the slower runner slows both arms
//! and the ratio holds. This mode compares such a ratio against a stored baseline ratio
//! (a ratio of ratios).
//!
//! It is the fallback, not the preferred model. When the old version can be built and
//! run in the same job, version-vs-version in the same run (the default mode, fed with
//! `--bench-base-file` and `--bench-head-file`) compares the two versions directly and
//! needs no stored baseline at all. Paired-ratio is for when building the old version is
//! impractical, and the two are not interchangeable: a paired-ratio verdict is about the
//! subject relative to its twin, not about the subject relative to its own past.
//!
//! # The contract
//!
//! - **Per-round data.** A run carries `rounds[]` of `{subject, twin, order}` per cell.
//!   discipline computes the ratio (the median of per-round `subject / twin`) and its
//!   95% percentile-bootstrap interval. A supplied `ratio` is advisory; a mismatch with
//!   its own rounds is a violation. A cell with only means is "not comparable".
//! - **In-situ control.** Every axis carries control cells: two independently built arms
//!   of IDENTICAL source interleaved into the same rounds. Every control cell must read
//!   null; one whose whole interval clears its floor makes the run "not comparable". A
//!   run without controls is refused.
//! - **Derived thresholds.** A cell's threshold is the larger of its floor derived offline
//!   from repeated same-code runs (`discipline bench derive`) and this run's control
//!   scatter (p90 of |control ratio - 1|). A configured `ratio_tolerance_pct` only ever
//!   widens a derived floor; it never stands in for one.
//! - **The whole interval decides.** A cell regresses only when its whole interval clears
//!   `baseline x (1 +/- threshold)` in the adverse direction. A point estimate past the
//!   threshold with a straddling interval is movement. Improvements are reported and never
//!   fail.
//! - **The twin is part of the baseline's identity.** A changed twin identity or version
//!   invalidates the baseline ("baseline invalidated by twin change"); a twin that moves
//!   outside its own historical band makes the cell "not comparable". Both arms moving
//!   together leaves the ratio unchanged and is never read as a pass by itself.
//! - **The baseline is a threshold file.** It is read from the base ref, never from head.
//!   Loosening it needs a scoped `allow-regression: <baseline path> <reason>` directive
//!   and appears in the overrides audit; so does a diff that touches the baseline and
//!   non-benchmark source together.
//!
//! # Known limit
//!
//! A ratio of ratios is still a cross-run comparison, one level removed. The in-situ
//! control validates this run's stability; it does not show that a baseline recorded on
//! one runner generation stays valid after a runner fleet rotates to a different CPU
//! generation. Re-derive the baseline when the fleet changes.

use super::bounds::{
    bootstrap_median_ci, control_scatter_pct, derive_noise_floors, log_space_median, median,
    min_rounds_for_median_ci, ratio_of_ratios_decision, Adverse, RatioDecision, RatioVerdict,
    DERIVE_AXIS_SAFETY, DERIVE_CELL_SAFETY, DERIVE_QUANTILE,
};
use super::citation;
use super::GATE;
use crate::config::{BenchRegressionGate, Severity};
use crate::guards::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::tokens::{self, ParsedDirective};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Schema tag of a paired-ratio run file.
pub const RUN_SCHEMA: &str = "discipline-bench-ratio/v1";
/// Schema tag of a paired-ratio baseline (threshold) file.
pub const BASELINE_SCHEMA: &str = "discipline-bench-ratio-baseline/v1";
/// Confidence level of every interval in this mode.
pub const CONFIDENCE: f64 = 0.95;
/// Relative tolerance between a supplied advisory `ratio` and the one its rounds give.
pub const ADVISORY_RATIO_TOLERANCE: f64 = 1e-4;
/// Default ceiling above which a derived cell floor is reported but not gated.
pub const DEFAULT_CEILING_PCT: f64 = 50.0;

// ---- Run file ---------------------------------------------------------------------

/// Identity and version of the reference arm every ratio is taken against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwinIdentity {
    pub identity: String,
    pub version: String,
}

/// Where and against what a run was measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Platform key: libc, OS and instruction set (e.g. `linux-glibc-x86_64`).
    pub platform: String,
    /// Runner class (e.g. an image label); a ratio is compared only within one class.
    pub runner_class: String,
    /// Identity of the runner instance, when known (counts distinct runners in derive).
    #[serde(default)]
    pub runner_id: Option<String>,
    /// Commit the subject arm was built from.
    pub commit: String,
    pub twin: TwinIdentity,
}

/// Which arm ran first in a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubjectOrder {
    SubjectFirst,
    TwinFirst,
}

/// One paired round of a gated cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Round {
    pub subject: f64,
    pub twin: f64,
    pub order: SubjectOrder,
}

/// Which control build ran first in a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControlOrder {
    AFirst,
    BFirst,
}

/// One paired round of an in-situ control cell: two builds of identical source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRound {
    pub a: f64,
    pub b: f64,
    pub order: ControlOrder,
}

/// A gated cell. Fields other than `rounds` and `ratio` are ignored, so a file that
/// carries only means parses and is reported "not comparable" rather than refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCell {
    #[serde(default)]
    pub rounds: Vec<Round>,
    /// Advisory: discipline computes the ratio from `rounds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f64>,
}

/// An in-situ control cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlCell {
    pub rounds: Vec<ControlRound>,
}

/// Direction in which a ratio is adverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdverseSpec {
    Up,
    Down,
}

impl From<AdverseSpec> for Adverse {
    fn from(a: AdverseSpec) -> Self {
        match a {
            AdverseSpec::Up => Adverse::Up,
            AdverseSpec::Down => Adverse::Down,
        }
    }
}

/// One named axis (`timing`, `memory`, ...) of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAxis {
    pub adverse: AdverseSpec,
    pub cells: BTreeMap<String, RunCell>,
    #[serde(default)]
    pub controls: BTreeMap<String, ControlCell>,
}

/// A `discipline-bench-ratio/v1` run file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatioRun {
    pub schema: String,
    pub provenance: Provenance,
    pub axes: BTreeMap<String, RunAxis>,
}

impl RatioRun {
    /// Parses and validates a run file. A run with an axis that carries no control cells
    /// is refused: without the in-situ control it cannot say whether it was stable.
    pub fn parse(content: &str, origin: &str) -> Result<Self> {
        let run: RatioRun = serde_json::from_str(content)
            .with_context(|| format!("`{origin}` is not a `{RUN_SCHEMA}` run file"))?;
        if run.schema != RUN_SCHEMA {
            bail!(
                "`{origin}` declares schema `{}`, expected `{RUN_SCHEMA}`",
                run.schema
            );
        }
        if run.axes.is_empty() {
            bail!("`{origin}` has no axes");
        }
        for (name, axis) in &run.axes {
            if axis.controls.is_empty() {
                bail!(
                    "refused: axis `{name}` of `{origin}` carries no in-situ control cells (two independently built arms of identical source interleaved into the same rounds); a paired-ratio run without them cannot show it was stable"
                );
            }
            for (id, cell) in &axis.cells {
                for (i, r) in cell.rounds.iter().enumerate() {
                    check_value(r.subject, origin, name, id, i)?;
                    check_value(r.twin, origin, name, id, i)?;
                }
                if let Some(ratio) = cell.ratio {
                    check_value(ratio, origin, name, id, 0)?;
                }
            }
            for (id, cell) in &axis.controls {
                for (i, r) in cell.rounds.iter().enumerate() {
                    check_value(r.a, origin, name, id, i)?;
                    check_value(r.b, origin, name, id, i)?;
                }
            }
        }
        Ok(run)
    }
}

fn check_value(v: f64, origin: &str, axis: &str, id: &str, round: usize) -> Result<()> {
    if !v.is_finite() || v <= 0.0 {
        bail!("`{origin}` axis `{axis}` cell `{id}` round {round}: value {v} is not a positive, finite measurement");
    }
    Ok(())
}

// ---- Baseline (threshold) file ------------------------------------------------------

/// What a platform's floors were derived from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedFrom {
    pub runs: usize,
    pub distinct_runners: usize,
    pub commits: Vec<String>,
    pub mixed_commits: bool,
}

/// The derivation that set an axis's floors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisDerivation {
    pub axis_floor_pct: f64,
    pub p95_drift_pct: f64,
    pub pairwise_samples: usize,
    pub quantile: f64,
    pub axis_safety_factor: f64,
    pub cell_safety_factor: f64,
    pub per_cell_below_axis_allowed: bool,
    pub ceiling_pct: f64,
    /// Floor of the twin's own between-run movement, pooled over the axis.
    pub twin_axis_floor_pct: f64,
}

/// A cell's stored ratio and the floors that govern it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellBaseline {
    /// Log-space median of the derivation runs' ratios.
    pub ratio: f64,
    pub floor_pct: f64,
    pub worst_drift_pct: f64,
    pub gateable: bool,
    /// Log-space median of the twin's per-run medians: its historical centre.
    pub twin_median: f64,
    /// Floor of the twin's own between-run movement for this cell.
    pub twin_floor_pct: f64,
}

/// One axis of a platform baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisBaseline {
    pub adverse: AdverseSpec,
    /// Absent only in a hand-written file; an axis without it is a configuration error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<AxisDerivation>,
    pub cells: BTreeMap<String, CellBaseline>,
}

/// One platform's baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformBaseline {
    pub runner_class: String,
    pub twin: TwinIdentity,
    pub derived_from: DerivedFrom,
    pub axes: BTreeMap<String, AxisBaseline>,
}

/// A `discipline-bench-ratio-baseline/v1` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatioBaseline {
    pub schema: String,
    pub platforms: BTreeMap<String, PlatformBaseline>,
}

impl RatioBaseline {
    pub fn parse(content: &str, origin: &str) -> Result<Self> {
        let b: RatioBaseline = serde_json::from_str(content)
            .with_context(|| format!("`{origin}` is not a `{BASELINE_SCHEMA}` file"))?;
        if b.schema != BASELINE_SCHEMA {
            bail!(
                "`{origin}` declares schema `{}`, expected `{BASELINE_SCHEMA}`",
                b.schema
            );
        }
        Ok(b)
    }
}

// ---- Estimation ---------------------------------------------------------------------

/// A cell's paired ratio, its interval, and the twin's own centre and interval.
#[derive(Debug, Clone, PartialEq)]
pub struct CellEstimate {
    pub ratio: f64,
    pub ci: (f64, f64),
    pub n: usize,
    pub twin_median: f64,
    pub twin_ci: (f64, f64),
}

fn balanced(first: usize, second: usize) -> bool {
    first.abs_diff(second) <= 1
}

/// Estimates a gated cell, or says why it is not comparable.
pub fn estimate_cell(cell: &RunCell) -> Result<std::result::Result<CellEstimate, String>> {
    let n = cell.rounds.len();
    if n == 0 {
        return Ok(Err(
            "no per-round data: a ratio or means without the rounds they came from cannot be compared"
                .to_string(),
        ));
    }
    let min_rounds = min_rounds_for_median_ci(CONFIDENCE)?;
    if n < min_rounds {
        return Ok(Err(format!(
            "only {n} paired round(s); a {:.0}% bootstrap interval of the median needs at least {min_rounds}, or it collapses onto the sample extremes",
            CONFIDENCE * 100.0
        )));
    }
    let subject_first = cell
        .rounds
        .iter()
        .filter(|r| r.order == SubjectOrder::SubjectFirst)
        .count();
    if !balanced(subject_first, n - subject_first) {
        return Ok(Err(format!(
            "arm order not alternated ({subject_first} of {n} rounds ran the subject first); drift within a round loads onto whichever arm always runs last"
        )));
    }
    let ratios: Vec<f64> = cell.rounds.iter().map(|r| r.subject / r.twin).collect();
    let twins: Vec<f64> = cell.rounds.iter().map(|r| r.twin).collect();
    Ok(Ok(CellEstimate {
        ratio: median(&ratios)?,
        ci: bootstrap_median_ci(&ratios, CONFIDENCE)?,
        n,
        twin_median: median(&twins)?,
        twin_ci: bootstrap_median_ci(&twins, CONFIDENCE)?,
    }))
}

/// A control cell's median `a / b` ratio and its interval.
pub type ControlEstimate = (f64, (f64, f64));

/// Estimates a control cell (`a / b` per round), or says why it cannot be read.
pub fn estimate_control(
    cell: &ControlCell,
) -> Result<std::result::Result<ControlEstimate, String>> {
    let n = cell.rounds.len();
    let min_rounds = min_rounds_for_median_ci(CONFIDENCE)?;
    if n < min_rounds {
        return Ok(Err(format!(
            "only {n} control round(s); at least {min_rounds} are needed"
        )));
    }
    let a_first = cell
        .rounds
        .iter()
        .filter(|r| r.order == ControlOrder::AFirst)
        .count();
    if !balanced(a_first, n - a_first) {
        return Ok(Err(format!(
            "control arm order not alternated ({a_first} of {n} rounds ran build a first)"
        )));
    }
    let ratios: Vec<f64> = cell.rounds.iter().map(|r| r.a / r.b).collect();
    Ok(Ok((
        median(&ratios)?,
        bootstrap_median_ci(&ratios, CONFIDENCE)?,
    )))
}

// ---- Evaluation ---------------------------------------------------------------------

/// Everything the evaluation needs besides the run and the baseline.
pub struct EvalOptions<'a> {
    pub severity: Severity,
    pub tolerance_pct: Option<f64>,
    pub allow_cross_runner: bool,
    pub require_sourced_override: bool,
    pub directives: &'a [ParsedDirective],
    pub policy: citation::FreshnessPolicy<'a>,
    pub instruments: &'a dyn citation::CitationInstruments,
    /// Where findings are located in the report.
    pub location: &'a str,
}

fn not_comparable(out: &mut GateOutcome, opts: &EvalOptions<'_>, reason: String) {
    out.notes
        .push(format!("paired-ratio: not comparable — {reason}"));
    out.push(
        opts.severity,
        "Paired Ratio Not Comparable",
        Some(opts.location),
        None,
        format!("not comparable — {reason}; this run asserts nothing, and is not a pass"),
        "fix the run so it can be compared (rounds, controls, provenance), or re-derive the baseline in a dedicated change",
    );
}

/// Whether a regression in `cell` is approved by an `allow-regression:` directive that
/// names it (and, under `require_sourced_override`, cites a fresh measurement).
fn approve(cell: &str, opts: &EvalOptions<'_>, out: &mut GateOutcome) -> bool {
    let Some(ov) = tokens::find_override(opts.directives, GATE, tokens::ALLOW_REGRESSION, cell)
    else {
        return false;
    };
    if opts.require_sourced_override {
        if tokens::extract_citation(&ov.reason).is_none() {
            out.notes.push(format!(
                "paired-ratio: override for `{cell}` is void — its reason cites no CI run URL and no committed artifact path"
            ));
            return false;
        }
        let report = citation::check_citation_freshness(&ov.reason, &opts.policy, opts.instruments);
        if !report.is_fresh() {
            for p in &report.problems {
                out.notes
                    .push(format!("paired-ratio: override for `{cell}` is void — {p}"));
            }
            for u in &report.undecidable {
                out.notes.push(format!(
                    "allow-regression citation not verified — {u}; the gate stays armed"
                ));
            }
            return false;
        }
    }
    out.overrides.push(ov);
    true
}

/// Evaluates one paired-ratio run against the baseline read from the base ref.
pub fn evaluate_run(
    run: &RatioRun,
    baseline: Option<&RatioBaseline>,
    opts: &EvalOptions<'_>,
    out: &mut GateOutcome,
) -> Result<()> {
    out.examined += run.axes.values().map(|a| a.cells.len()).sum::<usize>();
    let prov = &run.provenance;

    let Some(baseline) = baseline else {
        not_comparable(
            out,
            opts,
            "no ratio baseline at the base ref (bootstrap: derive one from repeated same-commit runs with `discipline bench derive` and commit it in a dedicated change)".to_string(),
        );
        return Ok(());
    };
    let Some(plat) = baseline.platforms.get(&prov.platform) else {
        not_comparable(
            out,
            opts,
            format!("the baseline has no entry for platform `{}`", prov.platform),
        );
        return Ok(());
    };
    if plat.twin != prov.twin {
        not_comparable(
            out,
            opts,
            format!(
                "baseline invalidated by twin change: the baseline was derived against twin `{}` {} and this run measured against `{}` {}; ratios taken against different twins are never compared",
                plat.twin.identity, plat.twin.version, prov.twin.identity, prov.twin.version
            ),
        );
        return Ok(());
    }
    if plat.runner_class != prov.runner_class && !opts.allow_cross_runner {
        not_comparable(
            out,
            opts,
            format!(
                "runner class `{}` differs from the baseline's `{}` (pass `--allow-cross-host-bench` or set `allow_cross_host = true` to compare across runner classes)",
                prov.runner_class, plat.runner_class
            ),
        );
        return Ok(());
    }

    for (axis_name, axis) in &run.axes {
        let Some(axis_base) = plat.axes.get(axis_name) else {
            not_comparable(
                out,
                opts,
                format!(
                    "the baseline for `{}` has no axis `{axis_name}`",
                    prov.platform
                ),
            );
            continue;
        };
        let Some(derived) = &axis_base.derived else {
            bail!(
                "configuration error: axis `{axis_name}` of platform `{}` in the ratio baseline has no derived noise floor; thresholds are derived from repeated same-commit runs (`discipline bench derive`), never configured",
                prov.platform
            );
        };
        if axis_base.adverse != axis.adverse {
            not_comparable(
                out,
                opts,
                format!(
                    "axis `{axis_name}` declares adverse direction {:?} but the baseline records {:?}",
                    axis.adverse, axis_base.adverse
                ),
            );
            continue;
        }

        // In-situ control: every cell must read null against its own floor.
        let mut control_ratios = Vec::new();
        let mut moved = Vec::new();
        for (id, ctl) in &axis.controls {
            match estimate_control(ctl)? {
                Err(why) => moved.push(format!("control `{id}`: {why}")),
                Ok((ratio, ci)) => {
                    control_ratios.push(ratio);
                    let floor = axis_base
                        .cells
                        .get(id)
                        .map_or(derived.axis_floor_pct, |c| c.floor_pct);
                    let d = ratio_of_ratios_decision(ratio, ci, 1.0, floor, Adverse::Up)?;
                    if matches!(
                        d.verdict,
                        RatioVerdict::Regression | RatioVerdict::Improvement
                    ) {
                        moved.push(format!(
                            "control `{id}` read {:+.2}% [{:+.2}, {:+.2}] against its {floor:.2}% floor",
                            d.drift_pct, d.drift_lo_pct, d.drift_hi_pct
                        ));
                    }
                }
            }
        }
        let contaminated = !moved.is_empty();
        if contaminated {
            not_comparable(
                out,
                opts,
                format!(
                    "axis `{axis_name}`: the in-situ control (two builds of identical source) did not read null — {}; a movement in this run cannot be told from runner noise, and is not a code regression",
                    moved.join("; ")
                ),
            );
        }
        let scatter = if control_ratios.is_empty() {
            0.0
        } else {
            control_scatter_pct(&control_ratios)?
        };
        out.notes.push(format!(
            "paired-ratio axis `{axis_name}`: {} control cell(s), scatter (p90) {scatter:.2}%; axis floor {:.2}% (derived from {} pairwise same-code drifts)",
            axis.controls.len(),
            derived.axis_floor_pct,
            derived.pairwise_samples
        ));

        for id in axis_base.cells.keys() {
            if axis.cells.contains_key(id) {
                continue;
            }
            match tokens::find_override(opts.directives, GATE, tokens::ALLOW_REGRESSION, id) {
                Some(ov) => out.overrides.push(ov),
                None => {
                    out.push(
                        opts.severity,
                        "Paired Ratio Cell Missing",
                        Some(opts.location),
                        None,
                        format!("baselined cell `{axis_name}/{id}` is absent from this run; a cell that stops being measured stops being gated"),
                        &format!("restore the cell, or justify its removal: `allow-regression: {id} <rationale>`"),
                    );
                }
            }
        }

        for (id, cell) in &axis.cells {
            let est = match estimate_cell(cell)? {
                Ok(e) => e,
                Err(why) => {
                    not_comparable(out, opts, format!("cell `{axis_name}/{id}`: {why}"));
                    continue;
                }
            };
            if let Some(supplied) = cell.ratio {
                if (supplied / est.ratio - 1.0).abs() > ADVISORY_RATIO_TOLERANCE {
                    out.push(
                        Severity::Error,
                        "Paired Ratio Disagrees With Its Rounds",
                        Some(opts.location),
                        None,
                        format!(
                            "cell `{axis_name}/{id}` supplies ratio {supplied} but the median of its own {} per-round ratios is {:.6}; the supplied ratio is advisory and discipline computes it",
                            est.n, est.ratio
                        ),
                        "drop the supplied `ratio` or emit it from the same rounds",
                    );
                }
            }
            let Some(base) = axis_base.cells.get(id) else {
                out.notes.push(format!(
                    "paired-ratio: cell `{axis_name}/{id}` has no baseline entry; measured {:.5} [{:.5}, {:.5}], not gated",
                    est.ratio, est.ci.0, est.ci.1
                ));
                continue;
            };
            // The twin must stay inside its own historical band.
            let twin = ratio_of_ratios_decision(
                est.twin_median,
                est.twin_ci,
                base.twin_median,
                base.twin_floor_pct,
                Adverse::Up,
            )?;
            if matches!(
                twin.verdict,
                RatioVerdict::Regression | RatioVerdict::Improvement
            ) {
                not_comparable(
                    out,
                    opts,
                    format!(
                        "cell `{axis_name}/{id}`: the twin moved outside its own historical band ({:+.2}% [{:+.2}, {:+.2}] against a {:.2}% floor); a ratio against a twin that moved says nothing about the subject",
                        twin.drift_pct, twin.drift_lo_pct, twin.drift_hi_pct, base.twin_floor_pct
                    ),
                );
                continue;
            }
            if !base.gateable {
                out.notes.push(format!(
                    "paired-ratio: cell `{axis_name}/{id}` is reported, not gated (derived floor {:.2}% exceeds the {:.2}% ceiling)",
                    base.floor_pct, derived.ceiling_pct
                ));
                continue;
            }
            let mut threshold = base.floor_pct.max(scatter);
            if let Some(t) = opts.tolerance_pct {
                threshold = threshold.max(t);
            }
            let d: RatioDecision = ratio_of_ratios_decision(
                est.ratio,
                est.ci,
                base.ratio,
                threshold,
                axis.adverse.into(),
            )?;
            let detail = format!(
                "baseline {:.5} -> {:.5} ({:+.2}% [{:+.2}, {:+.2}], threshold {:.2}%: floor {:.2}%, control scatter {:.2}%)",
                base.ratio,
                est.ratio,
                d.drift_pct,
                d.drift_lo_pct,
                d.drift_hi_pct,
                threshold,
                base.floor_pct,
                scatter
            );
            match d.verdict {
                RatioVerdict::Regression if contaminated => out.notes.push(format!(
                    "paired-ratio: `{axis_name}/{id}` would read as a regression but the run is not comparable; suppressed: {detail}"
                )),
                RatioVerdict::Regression => {
                    if !approve(id, opts, out) {
                        out.push(
                            opts.severity,
                            "Paired Ratio Regressed",
                            Some(opts.location),
                            None,
                            format!(
                                "cell `{axis_name}/{id}` on `{}` regressed: the whole {:.0}% interval of its ratio of ratios clears the threshold in the adverse direction: {detail}",
                                prov.platform,
                                CONFIDENCE * 100.0
                            ),
                            &format!("fix the regression or justify it: `allow-regression: {id} <rationale>`"),
                        );
                    }
                }
                RatioVerdict::Improvement => out.notes.push(format!(
                    "paired-ratio: `{axis_name}/{id}` improved: {detail}"
                )),
                RatioVerdict::Movement => out.notes.push(format!(
                    "paired-ratio: `{axis_name}/{id}` moved past its threshold but its interval straddles it (movement, not a regression): {detail}"
                )),
                RatioVerdict::Null => {}
            }
        }
    }
    Ok(())
}

// ---- Baseline integrity -------------------------------------------------------------

/// Every way `head` is looser than `base`: coverage removed, a floor or ceiling widened,
/// a cell made ungateable, a twin changed, or a stored ratio moved in the adverse
/// direction (which makes a regression easier to pass).
pub fn loosenings(base: &RatioBaseline, head: &RatioBaseline) -> Vec<String> {
    let mut out = Vec::new();
    for (pname, bp) in &base.platforms {
        let Some(hp) = head.platforms.get(pname) else {
            out.push(format!("platform `{pname}` removed"));
            continue;
        };
        if hp.twin != bp.twin {
            out.push(format!(
                "platform `{pname}` twin changed from `{}` {} to `{}` {}",
                bp.twin.identity, bp.twin.version, hp.twin.identity, hp.twin.version
            ));
        }
        for (aname, ba) in &bp.axes {
            let Some(ha) = hp.axes.get(aname) else {
                out.push(format!("`{pname}` axis `{aname}` removed"));
                continue;
            };
            if ha.adverse != ba.adverse {
                out.push(format!("`{pname}/{aname}` adverse direction changed"));
            }
            match (&ba.derived, &ha.derived) {
                (Some(bd), Some(hd)) => {
                    if hd.axis_floor_pct > bd.axis_floor_pct {
                        out.push(format!(
                            "`{pname}/{aname}` axis floor widened {:.2}% -> {:.2}%",
                            bd.axis_floor_pct, hd.axis_floor_pct
                        ));
                    }
                    if hd.ceiling_pct > bd.ceiling_pct {
                        out.push(format!(
                            "`{pname}/{aname}` ceiling raised {:.2}% -> {:.2}%",
                            bd.ceiling_pct, hd.ceiling_pct
                        ));
                    }
                }
                (Some(_), None) => out.push(format!("`{pname}/{aname}` derivation removed")),
                _ => {}
            }
            for (cname, bc) in &ba.cells {
                let Some(hc) = ha.cells.get(cname) else {
                    out.push(format!("`{pname}/{aname}/{cname}` removed"));
                    continue;
                };
                if hc.floor_pct > bc.floor_pct {
                    out.push(format!(
                        "`{pname}/{aname}/{cname}` floor widened {:.2}% -> {:.2}%",
                        bc.floor_pct, hc.floor_pct
                    ));
                }
                if hc.twin_floor_pct > bc.twin_floor_pct {
                    out.push(format!(
                        "`{pname}/{aname}/{cname}` twin band widened {:.2}% -> {:.2}%",
                        bc.twin_floor_pct, hc.twin_floor_pct
                    ));
                }
                if bc.gateable && !hc.gateable {
                    out.push(format!("`{pname}/{aname}/{cname}` made ungateable"));
                }
                let adverse_move = match ha.adverse {
                    AdverseSpec::Up => hc.ratio > bc.ratio,
                    AdverseSpec::Down => hc.ratio < bc.ratio,
                };
                if adverse_move {
                    out.push(format!(
                        "`{pname}/{aname}/{cname}` stored ratio moved in the adverse direction {:.5} -> {:.5}",
                        bc.ratio, hc.ratio
                    ));
                }
            }
        }
    }
    out
}

fn check_baseline_change(
    ctx: &Context,
    settings: &BenchRegressionGate,
    path: &str,
    out: &mut GateOutcome,
) -> Result<()> {
    let changed = ctx.git.changed_files()?;
    if !changed.iter().any(|f| f.path == path || f.old_path == path) {
        return Ok(());
    }
    let severity = ctx.overridable(settings.severity);
    let named = ctx.find_override(GATE, tokens::ALLOW_REGRESSION, path);

    // Loosening.
    let base = ctx.git.base_content(path)?;
    let head = ctx.git.head_content(path)?;
    let found: Vec<String> = match (base, head) {
        (Some(b), Some(h)) => {
            let b = RatioBaseline::parse(&b, &format!("{path} (base)"))?;
            let h = RatioBaseline::parse(&h, &format!("{path} (head)"))?;
            loosenings(&b, &h)
        }
        (Some(_), None) => vec!["the baseline file was deleted".to_string()],
        (None, Some(h)) => {
            RatioBaseline::parse(&h, &format!("{path} (head)"))?;
            Vec::new()
        }
        (None, None) => Vec::new(),
    };
    if !found.is_empty() {
        match &named {
            Some(ov) => out.overrides.push(ov.clone()),
            None => out.push(
                severity,
                "Ratio Baseline Loosened",
                Some(path),
                None,
                format!(
                    "the paired-ratio baseline is a threshold file, and this change loosens it: {}",
                    found.join("; ")
                ),
                &format!("re-derive it with `discipline bench derive` in a dedicated change, and justify it: `allow-regression: {path} <rationale>`"),
            ),
        }
    }

    // A baseline change travels alone.
    let watched = PathFilter::new(&settings.paths)?;
    let exempt = exempt_filter(settings)?;
    let source: Vec<&str> = changed
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| *p != path && !watched.matches(p) && !exempt.matches(p))
        .collect();
    if !source.is_empty() {
        match &named {
            Some(ov) => {
                if !out.overrides.iter().any(|o| o == ov) {
                    out.overrides.push(ov.clone());
                }
            }
            None => out.push(
                severity,
                "Ratio Baseline Changed With Source",
                Some(path),
                None,
                format!(
                    "this change touches the paired-ratio baseline and non-benchmark source together ({}); a baseline refreshed alongside the code it gates can absorb that code's regression",
                    source.iter().take(5).copied().collect::<Vec<_>>().join(", ")
                ),
                &format!("move the baseline refresh into its own change, or justify it: `allow-regression: {path} <rationale>`"),
            ),
        }
    }
    Ok(())
}

// ---- Gate entry -------------------------------------------------------------------

/// `bench-regression` with `mode = "paired-ratio"`.
pub fn bench_paired_ratio(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.bench_regression;
    let mut out = GateOutcome::new(GATE);
    let Some(baseline_path) = settings.ratio_baseline.as_deref() else {
        bail!("configuration error: `gates.bench-regression.mode = \"paired-ratio\"` requires `ratio_baseline`, the committed ratio baseline file");
    };
    if let Some(t) = settings.ratio_tolerance_pct {
        if !t.is_finite() || t < 0.0 {
            bail!("configuration error: `ratio_tolerance_pct` must be finite and non-negative, got {t}");
        }
    }

    check_baseline_change(ctx, settings, baseline_path, &mut out)?;

    let env_head = std::env::var("DISCIPLINE_BENCH_HEAD_FILE")
        .ok()
        .filter(|s| !s.is_empty());
    let head_file = ctx
        .bench_head_file
        .as_deref()
        .and_then(|p| p.to_str())
        .map(str::to_string)
        .or(env_head)
        .or_else(|| settings.head_file.clone());
    if ctx.bench_base_file.is_some() || settings.base_file.is_some() {
        out.notes.push("paired-ratio: a base benchmark file is ignored in this mode; the baseline is `ratio_baseline` at the base ref".to_string());
    }
    let Some(head_file) = head_file else {
        let opts_location = baseline_path;
        out.notes
            .push("paired-ratio: no run file supplied (`--bench-head-file`, `DISCIPLINE_BENCH_HEAD_FILE` or `head_file`); nothing was measured".to_string());
        out.push(
            ctx.overridable(settings.severity),
            "Paired Ratio Run Missing",
            Some(opts_location),
            None,
            "paired-ratio mode is configured but no run file was supplied; the gate measured nothing".to_string(),
            "pass the run JSON with `--bench-head-file`",
        );
        return Ok(out);
    };
    let content = std::fs::read_to_string(&head_file)
        .with_context(|| format!("failed to read paired-ratio run file `{head_file}`"))?;
    let run = RatioRun::parse(&content, &head_file)?;

    // The threshold file comes from the base ref, never from head.
    let baseline = match ctx.git.base_content(baseline_path)? {
        Some(text) => Some(RatioBaseline::parse(
            &text,
            &format!("{baseline_path} (base ref)"),
        )?),
        None => None,
    };
    if settings.ratio_tolerance_pct.is_some() {
        if let Some(b) = &baseline {
            if let Some(plat) = b.platforms.get(&run.provenance.platform) {
                for (name, axis) in &plat.axes {
                    if axis.derived.is_none() {
                        bail!("configuration error: `ratio_tolerance_pct` is configured but axis `{name}` has no derived noise floor; a configured tolerance can widen a derived floor, never replace one");
                    }
                }
            }
        }
    }

    let instruments = citation::LiveInstruments::new(ctx.git);
    let opts = EvalOptions {
        severity: ctx.overridable(settings.severity),
        tolerance_pct: settings.ratio_tolerance_pct,
        allow_cross_runner: ctx.allow_cross_host_bench || settings.allow_cross_host,
        require_sourced_override: settings.require_sourced_override,
        directives: &ctx.directives,
        policy: citation::FreshnessPolicy {
            measurement_jobs: &settings.citation_measurement_jobs,
            source_paths: &settings.citation_source_paths,
        },
        instruments: &instruments,
        location: &head_file,
    };
    evaluate_run(&run, baseline.as_ref(), &opts, &mut out)?;
    Ok(out)
}

// ---- Derivation ---------------------------------------------------------------------

/// Derives one platform's baseline entry from repeated runs of the same code.
///
/// All runs must share platform, runner class and twin; they must share a commit unless
/// `allow_mixed_commits` (the commits are recorded either way). Each axis's floors come
/// from `derive_noise_floors` over the per-run cell ratios; each cell's stored ratio is
/// the log-space median of those ratios, and the twin's band from its per-run medians.
/// Cells absent from any run are not recorded.
pub fn derive_platform(
    runs: &[RatioRun],
    allow_mixed_commits: bool,
    ceiling_pct: f64,
) -> Result<(String, PlatformBaseline)> {
    if runs.len() < 2 {
        bail!("deriving floors needs at least two run files of the same code");
    }
    let first = &runs[0].provenance;
    for r in runs {
        let p = &r.provenance;
        if p.platform != first.platform {
            bail!(
                "runs span platforms `{}` and `{}`; derive one platform at a time",
                first.platform,
                p.platform
            );
        }
        if p.runner_class != first.runner_class {
            bail!(
                "runs span runner classes `{}` and `{}`",
                first.runner_class,
                p.runner_class
            );
        }
        if p.twin != first.twin {
            bail!(
                "runs span twins `{}` {} and `{}` {}",
                first.twin.identity,
                first.twin.version,
                p.twin.identity,
                p.twin.version
            );
        }
    }
    let commits: Vec<String> = runs
        .iter()
        .map(|r| r.provenance.commit.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if commits.len() > 1 && !allow_mixed_commits {
        bail!(
            "runs span several commits ({}); a floor derived across different code measures change, not noise. Pass `--allow-mixed-commits` only when the differences cannot move a number",
            commits.join(", ")
        );
    }
    let distinct_runners = runs
        .iter()
        .filter_map(|r| r.provenance.runner_id.as_deref())
        .collect::<BTreeSet<_>>()
        .len()
        .max(1);

    let axis_names: BTreeSet<&String> = runs.iter().flat_map(|r| r.axes.keys()).collect();
    let mut axes = BTreeMap::new();
    for name in axis_names {
        let mut per_run_ratio: Vec<BTreeMap<String, f64>> = Vec::new();
        let mut per_run_twin: Vec<BTreeMap<String, f64>> = Vec::new();
        let mut adverse = None;
        for r in runs {
            let Some(axis) = r.axes.get(name) else {
                bail!("axis `{name}` is not in every run");
            };
            if *adverse.get_or_insert(axis.adverse) != axis.adverse {
                bail!("axis `{name}` declares different adverse directions across runs");
            }
            let mut ratios = BTreeMap::new();
            let mut twins = BTreeMap::new();
            for (id, cell) in &axis.cells {
                match estimate_cell(cell)? {
                    Ok(e) => {
                        ratios.insert(id.clone(), e.ratio);
                        twins.insert(id.clone(), e.twin_median);
                    }
                    Err(why) => bail!("cell `{name}/{id}` cannot be derived from: {why}"),
                }
            }
            per_run_ratio.push(ratios);
            per_run_twin.push(twins);
        }
        let floors = derive_noise_floors(&per_run_ratio, distinct_runners, ceiling_pct)?;
        let twin_floors = derive_noise_floors(&per_run_twin, distinct_runners, f64::MAX)?;
        let mut cells = BTreeMap::new();
        for (id, floor) in &floors.cells {
            let series: Vec<f64> = per_run_ratio
                .iter()
                .filter_map(|m| m.get(id).copied())
                .collect();
            if series.len() < runs.len() {
                continue;
            }
            let twin_series: Vec<f64> = per_run_twin
                .iter()
                .filter_map(|m| m.get(id).copied())
                .collect();
            cells.insert(
                id.clone(),
                CellBaseline {
                    ratio: log_space_median(&series)?,
                    floor_pct: floor.floor_pct,
                    worst_drift_pct: floor.worst_drift_pct,
                    gateable: floor.gateable,
                    twin_median: log_space_median(&twin_series)?,
                    twin_floor_pct: twin_floors
                        .cells
                        .get(id)
                        .map_or(twin_floors.axis_floor_pct, |c| c.floor_pct),
                },
            );
        }
        axes.insert(
            name.clone(),
            AxisBaseline {
                adverse: adverse.expect("at least two runs"),
                derived: Some(AxisDerivation {
                    axis_floor_pct: floors.axis_floor_pct,
                    p95_drift_pct: floors.p95_drift_pct,
                    pairwise_samples: floors.pairwise_samples,
                    quantile: DERIVE_QUANTILE,
                    axis_safety_factor: DERIVE_AXIS_SAFETY,
                    cell_safety_factor: DERIVE_CELL_SAFETY,
                    per_cell_below_axis_allowed: floors.per_cell_below_axis_allowed,
                    ceiling_pct,
                    twin_axis_floor_pct: twin_floors.axis_floor_pct,
                }),
                cells,
            },
        );
    }
    Ok((
        first.platform.clone(),
        PlatformBaseline {
            runner_class: first.runner_class.clone(),
            twin: first.twin.clone(),
            derived_from: DerivedFrom {
                runs: runs.len(),
                distinct_runners,
                commits,
                mixed_commits: allow_mixed_commits
                    && runs
                        .iter()
                        .map(|r| &r.provenance.commit)
                        .collect::<BTreeSet<_>>()
                        .len()
                        > 1,
            },
            axes,
        },
    ))
}

/// `discipline bench derive <run.json>... [--baseline <path>]`.
///
/// Prints the derived platform entry. With `baseline`, merges it into that file (created
/// when absent) under its platform key. The file is a threshold file: commit it in a
/// dedicated change.
pub fn cli_derive(
    run_paths: &[std::path::PathBuf],
    baseline: Option<&std::path::Path>,
    allow_mixed_commits: bool,
    ceiling_pct: f64,
) -> Result<bool> {
    let mut runs = Vec::new();
    for p in run_paths {
        let text = std::fs::read_to_string(p)
            .with_context(|| format!("failed to read run file `{}`", p.display()))?;
        runs.push(RatioRun::parse(&text, &p.display().to_string())?);
    }
    let (platform, entry) = derive_platform(&runs, allow_mixed_commits, ceiling_pct)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "platform": platform, "entry": entry }))?
    );
    if let Some(path) = baseline {
        let mut file = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read `{}`", path.display()))?;
            RatioBaseline::parse(&text, &path.display().to_string())?
        } else {
            RatioBaseline {
                schema: BASELINE_SCHEMA.to_string(),
                platforms: BTreeMap::new(),
            }
        };
        file.platforms.insert(platform.clone(), entry);
        std::fs::write(path, serde_json::to_string_pretty(&file)? + "\n")
            .with_context(|| format!("failed to write `{}`", path.display()))?;
        eprintln!(
            "updated {} for platform {platform}; commit it in a dedicated change",
            path.display()
        );
    }
    Ok(true)
}

/// Dispatches `discipline bench <subcommand>`.
pub fn cli_bench(args: crate::cli::BenchArgs) -> Result<bool> {
    match args.command {
        crate::cli::BenchCommand::Derive(d) => cli_derive(
            &d.runs,
            d.baseline.as_deref(),
            d.allow_mixed_commits,
            d.ceiling_pct,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::OverrideSource;

    /// Rounds whose per-round ratios are `center x (1 + j)` for the offsets `j`, with
    /// the twin at `twin` and the arm order alternating.
    fn rounds(center: f64, jitter: &[f64], twin: f64) -> Vec<Round> {
        jitter
            .iter()
            .enumerate()
            .map(|(i, j)| Round {
                subject: center * (1.0 + j) * twin,
                twin,
                order: if i % 2 == 0 {
                    SubjectOrder::SubjectFirst
                } else {
                    SubjectOrder::TwinFirst
                },
            })
            .collect()
    }

    const TIGHT: &[f64] = &[
        -0.002, 0.001, 0.0, 0.002, -0.001, 0.001, 0.0, -0.002, 0.002, 0.0,
    ];

    fn control(center: f64) -> ControlCell {
        ControlCell {
            rounds: rounds(center, TIGHT, 1.0)
                .into_iter()
                .enumerate()
                .map(|(i, r)| ControlRound {
                    a: r.subject,
                    b: r.twin,
                    order: if i % 2 == 0 {
                        ControlOrder::AFirst
                    } else {
                        ControlOrder::BFirst
                    },
                })
                .collect(),
        }
    }

    fn twin() -> TwinIdentity {
        TwinIdentity {
            identity: "reference-build".to_string(),
            version: "1.0.5".to_string(),
        }
    }

    fn run_with(cells: &[(&str, Vec<Round>)], controls: &[(&str, f64)]) -> RatioRun {
        RatioRun {
            schema: RUN_SCHEMA.to_string(),
            provenance: Provenance {
                platform: "linux-glibc-x86_64".to_string(),
                runner_class: "ubuntu-24.04".to_string(),
                runner_id: None,
                commit: "abc123".to_string(),
                twin: twin(),
            },
            axes: BTreeMap::from([(
                "timing".to_string(),
                RunAxis {
                    adverse: AdverseSpec::Up,
                    cells: cells
                        .iter()
                        .map(|(id, r)| {
                            (
                                id.to_string(),
                                RunCell {
                                    rounds: r.clone(),
                                    ratio: None,
                                },
                            )
                        })
                        .collect(),
                    controls: controls
                        .iter()
                        .map(|(id, c)| (id.to_string(), control(*c)))
                        .collect(),
                },
            )]),
        }
    }

    fn baseline_with(floor: f64, cells: &[&str]) -> RatioBaseline {
        RatioBaseline {
            schema: BASELINE_SCHEMA.to_string(),
            platforms: BTreeMap::from([(
                "linux-glibc-x86_64".to_string(),
                PlatformBaseline {
                    runner_class: "ubuntu-24.04".to_string(),
                    twin: twin(),
                    derived_from: DerivedFrom {
                        runs: 4,
                        distinct_runners: 2,
                        commits: vec!["abc123".to_string()],
                        mixed_commits: false,
                    },
                    axes: BTreeMap::from([(
                        "timing".to_string(),
                        AxisBaseline {
                            adverse: AdverseSpec::Up,
                            derived: Some(AxisDerivation {
                                axis_floor_pct: 5.0,
                                p95_drift_pct: 3.5,
                                pairwise_samples: 24,
                                quantile: DERIVE_QUANTILE,
                                axis_safety_factor: DERIVE_AXIS_SAFETY,
                                cell_safety_factor: DERIVE_CELL_SAFETY,
                                per_cell_below_axis_allowed: false,
                                ceiling_pct: DEFAULT_CEILING_PCT,
                                twin_axis_floor_pct: 10.0,
                            }),
                            cells: cells
                                .iter()
                                .map(|id| {
                                    (
                                        id.to_string(),
                                        CellBaseline {
                                            ratio: 1.0,
                                            floor_pct: floor,
                                            worst_drift_pct: floor / 1.5,
                                            gateable: true,
                                            twin_median: 1.0,
                                            twin_floor_pct: 10.0,
                                        },
                                    )
                                })
                                .collect(),
                        },
                    )]),
                },
            )]),
        }
    }

    fn eval_with(
        run: &RatioRun,
        baseline: Option<&RatioBaseline>,
        directives: &[ParsedDirective],
        tolerance: Option<f64>,
    ) -> Result<GateOutcome> {
        let mut out = GateOutcome::new(GATE);
        let opts = EvalOptions {
            severity: Severity::Error,
            tolerance_pct: tolerance,
            allow_cross_runner: false,
            require_sourced_override: false,
            directives,
            policy: citation::FreshnessPolicy {
                measurement_jobs: &[],
                source_paths: &[],
            },
            instruments: &citation::Unavailable,
            location: "run.json",
        };
        evaluate_run(run, baseline, &opts, &mut out)?;
        Ok(out)
    }

    fn eval(run: &RatioRun, baseline: Option<&RatioBaseline>) -> GateOutcome {
        eval_with(run, baseline, &[], None).unwrap()
    }

    fn titles(out: &GateOutcome) -> Vec<&str> {
        out.violations.iter().map(|v| v.title.as_str()).collect()
    }

    #[test]
    fn clean_run_reads_null_and_passes() {
        let run = run_with(&[("map_get", rounds(1.01, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert_eq!(out.examined, 1);
    }

    #[test]
    fn whole_interval_past_the_threshold_is_a_regression() {
        let run = run_with(&[("map_get", rounds(1.15, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Regressed"], "{:?}", out);
        assert!(out.violations[0].message.contains("+15.00%"));
    }

    #[test]
    fn point_past_the_threshold_with_a_straddling_interval_is_movement() {
        let wide = [-0.10, 0.10, -0.08, 0.08, 0.0, 0.0, -0.09, 0.09, -0.05, 0.05];
        let run = run_with(&[("map_get", rounds(1.07, &wide, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("movement, not a regression")));
    }

    #[test]
    fn improvement_is_reported_and_never_fails() {
        let run = run_with(&[("map_get", rounds(0.80, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert!(out.notes.iter().any(|n| n.contains("improved")));
    }

    #[test]
    fn control_that_does_not_read_null_makes_the_run_not_comparable() {
        // The control (identical source) moved 20%, which also widens the threshold to its
        // scatter; the cell at +40% still clears that and is suppressed: "not comparable",
        // never a pass and never a code regression.
        let run = run_with(&[("map_get", rounds(1.40, TIGHT, 1.0))], &[("ctl", 1.20)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Not Comparable"], "{out:?}");
        assert!(out.violations[0]
            .message
            .contains("control `ctl` read +20.00%"));
        assert!(out.notes.iter().any(|n| n.contains("suppressed")));
    }

    #[test]
    fn control_scatter_widens_the_threshold() {
        // Cell floor 2%, cell at +3%: a regression at the floor alone. The control reads
        // +4% (inside its 5% axis floor, so null), and its scatter raises the threshold
        // to 4%, which the cell does not clear.
        let run = run_with(&[("map_get", rounds(1.03, TIGHT, 1.0))], &[("ctl", 1.04)]);
        let out = eval(&run, Some(&baseline_with(2.0, &["map_get"])));
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        let quiet = run_with(&[("map_get", rounds(1.03, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&quiet, Some(&baseline_with(2.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Regressed"]);
    }

    #[test]
    fn configured_tolerance_only_widens_a_derived_floor() {
        let run = run_with(&[("map_get", rounds(1.03, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval_with(
            &run,
            Some(&baseline_with(2.0, &["map_get"])),
            &[],
            Some(4.0),
        )
        .unwrap();
        assert!(out.violations.is_empty());
        let mut hand_written = baseline_with(2.0, &["map_get"]);
        hand_written
            .platforms
            .get_mut("linux-glibc-x86_64")
            .unwrap()
            .axes
            .get_mut("timing")
            .unwrap()
            .derived = None;
        let err = eval_with(&run, Some(&hand_written), &[], Some(4.0)).unwrap_err();
        assert!(format!("{err:#}").contains("no derived noise floor"));
    }

    #[test]
    fn run_without_controls_is_refused() {
        let mut run = run_with(&[("map_get", rounds(1.0, TIGHT, 1.0))], &[("ctl", 1.0)]);
        run.axes.get_mut("timing").unwrap().controls.clear();
        let text = serde_json::to_string(&run).unwrap();
        let err = RatioRun::parse(&text, "run.json").unwrap_err();
        assert!(format!("{err:#}").contains("no in-situ control cells"));
    }

    #[test]
    fn cell_with_only_means_is_not_comparable() {
        let text = r#"{
          "schema": "discipline-bench-ratio/v1",
          "provenance": {"platform": "linux-glibc-x86_64", "runner_class": "ubuntu-24.04",
                         "commit": "abc123", "twin": {"identity": "reference-build", "version": "1.0.5"}},
          "axes": {"timing": {"adverse": "up",
            "cells": {"map_get": {"subject_mean": 1.2, "twin_mean": 1.0, "ratio": 1.2}},
            "controls": {"ctl": {"rounds": [
              {"a": 1, "b": 1, "order": "a-first"}, {"a": 1, "b": 1, "order": "b-first"},
              {"a": 1, "b": 1, "order": "a-first"}, {"a": 1, "b": 1, "order": "b-first"},
              {"a": 1, "b": 1, "order": "a-first"}, {"a": 1, "b": 1, "order": "b-first"}]}}}}
        }"#;
        let run = RatioRun::parse(text, "run.json").unwrap();
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Not Comparable"]);
        assert!(out.violations[0].message.contains("no per-round data"));
    }

    #[test]
    fn supplied_ratio_that_disagrees_with_its_rounds_is_a_violation() {
        let mut run = run_with(&[("map_get", rounds(1.0, TIGHT, 1.0))], &[("ctl", 1.0)]);
        run.axes
            .get_mut("timing")
            .unwrap()
            .cells
            .get_mut("map_get")
            .unwrap()
            .ratio = Some(0.9);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Disagrees With Its Rounds"]);
        assert_eq!(out.violations[0].severity, Severity::Error);
        // A matching advisory ratio is accepted.
        run.axes
            .get_mut("timing")
            .unwrap()
            .cells
            .get_mut("map_get")
            .unwrap()
            .ratio = Some(1.0);
        assert!(eval(&run, Some(&baseline_with(5.0, &["map_get"])))
            .violations
            .is_empty());
    }

    #[test]
    fn twin_version_change_invalidates_the_baseline() {
        let mut run = run_with(&[("map_get", rounds(1.30, TIGHT, 1.0))], &[("ctl", 1.0)]);
        run.provenance.twin.version = "1.0.6".to_string();
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Not Comparable"]);
        assert!(out.violations[0]
            .message
            .contains("baseline invalidated by twin change"));
    }

    #[test]
    fn twin_outside_its_own_band_is_not_comparable_even_with_the_ratio_unchanged() {
        // Both arms 40% slower: the ratio holds at 1.0, but the twin left its band (10%).
        let run = run_with(&[("map_get", rounds(1.0, TIGHT, 1.4))], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Not Comparable"], "{out:?}");
        assert!(out.violations[0]
            .message
            .contains("twin moved outside its own historical band"));
    }

    #[test]
    fn too_few_rounds_or_unalternated_order_is_not_comparable() {
        let run = run_with(
            &[("map_get", rounds(1.0, &TIGHT[..5], 1.0))],
            &[("ctl", 1.0)],
        );
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert!(
            out.violations[0].message.contains("needs at least 6"),
            "{out:?}"
        );

        let mut same_order = rounds(1.0, TIGHT, 1.0);
        for r in &mut same_order {
            r.order = SubjectOrder::SubjectFirst;
        }
        let run = run_with(&[("map_get", same_order)], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get"])));
        assert!(
            out.violations[0]
                .message
                .contains("arm order not alternated"),
            "{out:?}"
        );
    }

    #[test]
    fn no_baseline_or_no_platform_is_not_comparable() {
        let run = run_with(&[("map_get", rounds(1.0, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&run, None);
        assert_eq!(titles(&out), vec!["Paired Ratio Not Comparable"]);
        let mut other = run.clone();
        other.provenance.platform = "macos-arm64".to_string();
        let out = eval(&other, Some(&baseline_with(5.0, &["map_get"])));
        assert!(out.violations[0].message.contains("no entry for platform"));
    }

    #[test]
    fn baselined_cell_absent_from_the_run_needs_a_directive() {
        let run = run_with(&[("map_get", rounds(1.0, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let out = eval(&run, Some(&baseline_with(5.0, &["map_get", "map_insert"])));
        assert_eq!(titles(&out), vec!["Paired Ratio Cell Missing"]);
    }

    #[test]
    fn regression_named_by_a_directive_is_approved_and_audited() {
        let run = run_with(&[("map_get", rounds(1.15, TIGHT, 1.0))], &[("ctl", 1.0)]);
        let dirs = vec![ParsedDirective {
            directive: "allow-regression".to_string(),
            reason: "map_get pays for the new bounds check".to_string(),
            source: OverrideSource::PrBody,
            hidden: false,
        }];
        let out = eval_with(&run, Some(&baseline_with(5.0, &["map_get"])), &dirs, None).unwrap();
        assert!(out.violations.is_empty());
        assert_eq!(out.overrides.len(), 1);
    }

    #[test]
    fn loosenings_are_named_and_tightenings_are_not() {
        let base = baseline_with(5.0, &["a", "b"]);
        assert!(loosenings(&base, &base).is_empty());

        let mut tighter = base.clone();
        {
            let axis = tighter
                .platforms
                .get_mut("linux-glibc-x86_64")
                .unwrap()
                .axes
                .get_mut("timing")
                .unwrap();
            axis.cells.get_mut("a").unwrap().floor_pct = 3.0;
            axis.cells.get_mut("a").unwrap().ratio = 0.9; // favourable move
        }
        assert!(loosenings(&base, &tighter).is_empty());

        let mut looser = base.clone();
        {
            let plat = looser.platforms.get_mut("linux-glibc-x86_64").unwrap();
            let axis = plat.axes.get_mut("timing").unwrap();
            axis.cells.get_mut("a").unwrap().floor_pct = 8.0;
            axis.cells.get_mut("a").unwrap().ratio = 1.1;
            axis.cells.remove("b");
            axis.derived.as_mut().unwrap().axis_floor_pct = 9.0;
        }
        let found = loosenings(&base, &looser);
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(found
            .iter()
            .any(|f| f.contains("floor widened 5.00% -> 8.00%")));
        assert!(found.iter().any(|f| f.contains("adverse direction")));
        assert!(found
            .iter()
            .any(|f| f.contains("`linux-glibc-x86_64/timing/b` removed")));
        assert!(found.iter().any(|f| f.contains("axis floor widened")));
    }

    fn derive_run(commit: &str, center_a: f64, center_b: f64) -> RatioRun {
        let mut r = run_with(
            &[
                ("a", rounds(center_a, TIGHT, 1.0)),
                ("b", rounds(center_b, TIGHT, 2.0)),
            ],
            &[("ctl", 1.0)],
        );
        r.provenance.commit = commit.to_string();
        r
    }

    #[test]
    fn derive_records_floors_ratios_and_twin_bands_from_same_code_runs() {
        let runs = vec![
            derive_run("abc123", 1.00, 2.00),
            derive_run("abc123", 1.02, 2.10),
            derive_run("abc123", 0.99, 1.90),
        ];
        let (platform, entry) = derive_platform(&runs, false, DEFAULT_CEILING_PCT).unwrap();
        assert_eq!(platform, "linux-glibc-x86_64");
        let axis = &entry.axes["timing"];
        let derived = axis.derived.as_ref().unwrap();
        // Same pooled drifts as bounds::derive_noise_floors' pinned reference minus cell c.
        let expect = derive_noise_floors(
            &[
                BTreeMap::from([("a".to_string(), 1.00), ("b".to_string(), 2.00)]),
                BTreeMap::from([("a".to_string(), 1.02), ("b".to_string(), 2.10)]),
                BTreeMap::from([("a".to_string(), 0.99), ("b".to_string(), 1.90)]),
            ],
            1,
            DEFAULT_CEILING_PCT,
        )
        .unwrap();
        assert!((derived.axis_floor_pct - expect.axis_floor_pct).abs() < 1e-9);
        assert!((axis.cells["b"].floor_pct - expect.cells["b"].floor_pct).abs() < 1e-9);
        assert!(
            (axis.cells["a"].ratio - 1.0).abs() < 1e-9,
            "log-space median of 1.00, 1.02, 0.99"
        );
        assert!((axis.cells["b"].twin_median - 2.0).abs() < 1e-9);
        assert_eq!(entry.derived_from.runs, 3);
        assert!(!entry.derived_from.mixed_commits);

        let mixed = vec![
            derive_run("abc123", 1.0, 2.0),
            derive_run("def456", 1.0, 2.0),
        ];
        assert!(derive_platform(&mixed, false, DEFAULT_CEILING_PCT).is_err());
        let (_, e) = derive_platform(&mixed, true, DEFAULT_CEILING_PCT).unwrap();
        assert!(e.derived_from.mixed_commits);

        let mut other_twin = derive_run("abc123", 1.0, 2.0);
        other_twin.provenance.twin.version = "2.0".to_string();
        assert!(
            derive_platform(&[derive_run("abc123", 1.0, 2.0), other_twin], false, 50.0).is_err()
        );
    }
}
