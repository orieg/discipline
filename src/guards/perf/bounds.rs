//! Mathematical bounds and statistical decision rules for performance benchmarks.
//!
//! # Methodological Foundation & Derivation
//!
//! Statistical performance regression gating follows high-assurance evaluation rules:
//! 1. **Conservative Confidence Interval Rule (docs/GATES.md#pillar-5-benchmark-drift):**
//!    A wall-clock regression claim against tolerance `tau` is statistically verified
//!    iff the conservative lower bound of the performance difference exceeds `tau`.
//!    Derived from interval arithmetic: for independent estimators `X in [L_base, U_base]`
//!    and `Y in [L_head, U_head]`, the minimal possible difference is `Y - X >= L_head - U_base`.
//!    Normalizing by `U_base` yields the conservative lower bound on the percentage change:
//!    `delta_min = ((L_head - U_base) / U_base) * 100.0`.
//!    When `L_head <= U_base`, the data cannot reject `head <= base`; `delta_min <= 0.0`.
//!    A regression is flagged only when `delta_min > tolerance_pct`.
//!    When confidence intervals are missing for wall-clock data, the verdict degrades to
//!    "not comparable", never failing on point-estimate deltas alone.
//! 2. **Deterministic Integer Counters (Callgrind Ir, instruction cycles):**
//!    Counters have zero sampling variance and are evaluated directly against `tau`.
//! 3. **Wilson Score Interval (Wilson 1927):**
//!    Binomial proportion confidence interval for discrete pass/fail trials.
//! 4. **Paired within-run ratios (`paired-ratio` mode):** percentile-bootstrap interval of a
//!    median ratio, the minimum round count at which that interval stops collapsing onto the
//!    sample extremes, the ratio-of-ratios decision (the whole interval must clear the
//!    threshold), derived noise floors from repeated same-code runs, and the in-situ control
//!    scatter.
//!
//! # References
//! - Wilson, E. B. (1927) *Probable inference, the law of succession, and statistical inference*,
//!   J. Am. Stat. Assoc. 22:209-212.
//! - Efron, B. and Tibshirani, R. J. (1993) *An Introduction to the Bootstrap*, Chapman & Hall.
//! - Hyndman, R. J. and Fan, Y. (1996) *Sample quantiles in statistical packages*,
//!   The American Statistician 50(4):361-365.
//! - Schuirmann, D. J. (1987) *A comparison of the two one-sided tests procedure and the power
//!   approach for assessing the equivalence of average bioavailability*,
//!   J. Pharmacokinet. Biopharm. 15(6):657-680.

use anyhow::{bail, Result};
use std::collections::BTreeMap;

/// Two-sided confidence interval for a continuous metric.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfidenceInterval {
    pub lower: f64,
    pub upper: f64,
    pub confidence_level: f64,
}

impl ConfidenceInterval {
    pub fn new(lower: f64, upper: f64, confidence_level: f64) -> Result<Self> {
        if lower < 0.0 || upper < 0.0 {
            bail!(
                "interval bounds must be non-negative (got [{}, {}])",
                lower,
                upper
            );
        }
        if lower > upper {
            bail!(
                "invalid interval: lower bound {} exceeds upper bound {}",
                lower,
                upper
            );
        }
        if confidence_level <= 0.0 || confidence_level >= 1.0 {
            bail!(
                "confidence level must be in (0, 1), got {}",
                confidence_level
            );
        }
        Ok(Self {
            lower,
            upper,
            confidence_level,
        })
    }
}

/// Continuous benchmark estimate (e.g. Criterion or pytest-benchmark wall-clock timing).
#[derive(Debug, Clone, PartialEq)]
pub struct ContinuousEstimate {
    pub point_estimate: f64,
    pub ci: Option<ConfidenceInterval>,
    pub std_dev: Option<f64>,
    pub unit: String,
}

impl ContinuousEstimate {
    pub fn point_only(point_estimate: f64, unit: impl Into<String>) -> Result<Self> {
        if point_estimate < 0.0 {
            bail!(
                "point estimate must be non-negative, got {}",
                point_estimate
            );
        }
        Ok(Self {
            point_estimate,
            ci: None,
            std_dev: None,
            unit: unit.into(),
        })
    }

    pub fn with_ci(
        point_estimate: f64,
        lower: f64,
        upper: f64,
        confidence_level: f64,
        unit: impl Into<String>,
    ) -> Result<Self> {
        if point_estimate < 0.0 {
            bail!(
                "point estimate must be non-negative, got {}",
                point_estimate
            );
        }
        let ci = ConfidenceInterval::new(lower, upper, confidence_level)?;
        // High-assurance sanity check: point estimate must lie within or reasonably near the interval
        if point_estimate < lower * 0.95 || point_estimate > upper * 1.05 {
            bail!(
                "point estimate {} lies significantly outside confidence interval [{}, {}]",
                point_estimate,
                lower,
                upper
            );
        }
        Ok(Self {
            point_estimate,
            ci: Some(ci),
            std_dev: None,
            unit: unit.into(),
        })
    }

    pub fn with_std_dev(mut self, std_dev: f64) -> Self {
        if std_dev >= 0.0 {
            self.std_dev = Some(std_dev);
        }
        self
    }

    /// Coefficient of variation: relative dispersion $\sigma / \mu$.
    pub fn cv(&self) -> Option<f64> {
        if let Some(sd) = self.std_dev {
            if self.point_estimate > 0.0 {
                return Some(sd / self.point_estimate);
            }
        }
        None
    }

    /// Evaluates whether the benchmark variance exceeds an acceptable noise threshold.
    pub fn is_noisy(&self, max_cv: f64) -> bool {
        self.cv().is_some_and(|cv| cv > max_cv)
    }

    /// Constructs a `ContinuousEstimate` from empirical sample measurements, computing sample
    /// variance and a deterministic 95% bootstrap confidence interval (B=2000 resamples).
    pub fn from_samples(
        samples: &[f64],
        declared_median: Option<f64>,
        unit: impl Into<String>,
    ) -> Result<Self> {
        if samples.is_empty() {
            bail!("cannot construct estimate from empty sample slice");
        }
        for (i, &s) in samples.iter().enumerate() {
            if s < 0.0 || !s.is_finite() {
                bail!("sample {} must be non-negative and finite, got {}", i, s);
            }
        }

        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = sorted.len();
        let sample_median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };

        let point_estimate = if let Some(med) = declared_median {
            if med < 0.0 || !med.is_finite() {
                bail!(
                    "declared median must be non-negative and finite, got {}",
                    med
                );
            }
            med
        } else {
            sample_median
        };

        let unit_str = unit.into();
        if n == 1 {
            return Self::point_only(point_estimate, unit_str);
        }

        let mean = samples.iter().sum::<f64>() / n as f64;
        let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        let std_dev = variance.sqrt();

        if variance == 0.0 {
            let ci = ConfidenceInterval::new(point_estimate, point_estimate, 0.95)?;
            return Ok(Self {
                point_estimate,
                ci: Some(ci),
                std_dev: Some(0.0),
                unit: unit_str,
            });
        }

        let (boot_lower, boot_upper) = bootstrap_median_ci(samples, 0.95)?;
        let lower = boot_lower.min(point_estimate);
        let upper = boot_upper.max(point_estimate);

        let ci = ConfidenceInterval::new(lower, upper, 0.95)?;
        Ok(Self {
            point_estimate,
            ci: Some(ci),
            std_dev: Some(std_dev),
            unit: unit_str,
        })
    }
}

/// Exact deterministic integer metric (e.g. Callgrind instruction count `Ir`, fuel, or allocations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscreteMetric {
    pub count: u64,
}

impl DiscreteMetric {
    pub fn new(count: u64) -> Self {
        Self { count }
    }

    /// Compute exact percentage delta: `((head - base) / base) * 100.0`.
    /// A zero base with a zero head is unchanged (0.0). A zero base with a non-zero head
    /// has no finite percentage and returns an error.
    pub fn delta_pct(&self, head: &DiscreteMetric) -> Result<f64> {
        if self.count == 0 && head.count == 0 {
            return Ok(0.0);
        }
        if self.count == 0 {
            bail!("cannot compute percentage regression against base count of 0");
        }
        let diff = head.count as f64 - self.count as f64;
        Ok((diff / self.count as f64) * 100.0)
    }

    /// Evaluates whether an exact counter regressed beyond `tolerance_pct`.
    pub fn regressed(&self, head: &DiscreteMetric, tolerance_pct: f64) -> Result<bool> {
        let delta = self.delta_pct(head)?;
        Ok(delta > tolerance_pct)
    }
}

/// Statistical decision for continuous benchmark comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct RegressionDecision {
    /// Percentage delta reported. When CIs are present, this is the conservative lower bound `delta_min`.
    pub delta_pct: f64,
    /// Point estimate delta: `((head.point - base.point) / base.point) * 100.0`.
    pub point_delta_pct: f64,
    /// Whether the regression is statistically verified beyond `tolerance_pct`.
    pub is_regression: bool,
    /// Method used: `"conservative_interval"`, `"not_comparable_no_ci"`, or `"not_comparable_zero_base"`.
    pub method: &'static str,
    /// Diagnostic note explaining interval clearing or overlap.
    pub note: String,
}

/// Evaluates continuous benchmark regression using conservative interval bounds derived from interval arithmetic.
///
/// If both `base` and `head` have confidence intervals `[L_base, U_base]` and `[L_head, U_head]`:
/// - The conservative lower bound of the increase is:
///   `delta_min = ((L_head - U_base) / U_base) * 100.0`.
/// - If `L_head <= U_base`, `delta_min <= 0.0`. The confidence intervals overlap or head is faster;
///   the data cannot reject `head <= base`.
/// - A regression claim is accepted (`is_regression = true`) iff `delta_min > tolerance_pct`.
///
/// If confidence intervals are missing, falls back to point estimate comparison and notes
/// the absence of interval bounds.
///
/// A base point estimate (or base upper confidence bound) of 0.0 admits no relative delta;
/// the verdict is the named degradation `"not_comparable_zero_base"`, never a regression and
/// never an error (fail-closed contract F7, docs/ARCHITECTURE.md section 3).
pub fn evaluate_continuous_regression(
    base: &ContinuousEstimate,
    head: &ContinuousEstimate,
    tolerance_pct: f64,
) -> Result<RegressionDecision> {
    let zero_upper = base.ci.as_ref().is_some_and(|ci| ci.upper == 0.0);
    if base.point_estimate == 0.0 || zero_upper {
        return Ok(RegressionDecision {
            delta_pct: 0.0,
            point_delta_pct: 0.0,
            is_regression: false,
            method: "not_comparable_zero_base",
            note: format!(
                "not comparable (zero base estimate): base point estimate {} {} admits no relative delta (head: {} {}); a zero timing usually means the row is not a timing measurement",
                base.point_estimate, base.unit, head.point_estimate, head.unit
            ),
        });
    }

    let point_delta_pct =
        ((head.point_estimate - base.point_estimate) / base.point_estimate) * 100.0;

    if let (Some(base_ci), Some(head_ci)) = (&base.ci, &head.ci) {
        let delta_min = ((head_ci.lower - base_ci.upper) / base_ci.upper) * 100.0;
        let is_regression = delta_min > tolerance_pct;

        let note = if is_regression {
            format!(
                "conservative interval lower bound +{:.2}% clears tolerance {:.2}% (base [{}, {}], head [{}, {}])",
                delta_min, tolerance_pct, base_ci.lower, base_ci.upper, head_ci.lower, head_ci.upper
            )
        } else if point_delta_pct > tolerance_pct {
            format!(
                "point estimate +{:.2}% exceeds tolerance {:.2}%, but 95% CIs overlap (base [{}, {}], head [{}, {}]; delta_min: {:.2}%); no statistically verified regression",
                point_delta_pct, tolerance_pct, base_ci.lower, base_ci.upper, head_ci.lower, head_ci.upper, delta_min
            )
        } else {
            format!(
                "within tolerance (point delta: {:.2}%, delta_min: {:.2}%)",
                point_delta_pct, delta_min
            )
        };

        Ok(RegressionDecision {
            delta_pct: delta_min,
            point_delta_pct,
            is_regression,
            method: "conservative_interval",
            note,
        })
    } else {
        // Point estimate only (no confidence intervals reported for wall-clock data)
        // High-assurance discipline: without confidence intervals, wall-clock data
        // cannot reject the null hypothesis; verdict degrades to "not comparable" and never fails.
        let is_regression = false;
        let note = format!(
            "not comparable (no CI available): point-estimate delta {:+.2}% vs tolerance {:.2}%; wall-clock data lacks confidence intervals and cannot verify regression",
            point_delta_pct, tolerance_pct
        );

        Ok(RegressionDecision {
            delta_pct: point_delta_pct,
            point_delta_pct,
            is_regression,
            method: "not_comparable_no_ci",
            note,
        })
    }
}

/// Compute a two-sided Wilson score confidence interval for a binomial proportion (Wilson 1927).
///
/// Given `k` successes out of `n` independent trials at confidence level `z`:
/// `center = (p_hat + z^2 / (2n)) / (1 + z^2 / n)`
/// `margin = (z / (1 + z^2 / n)) * sqrt(p_hat * (1 - p_hat) / n + z^2 / (4n^2))`
/// `CI = [center - margin, center + margin]`
pub fn wilson_score_interval(k: usize, n: usize, confidence_level: f64) -> Result<(f64, f64)> {
    if n == 0 {
        bail!("sample size n must be greater than 0");
    }
    if k > n {
        bail!("successes k ({}) cannot exceed total trials n ({})", k, n);
    }
    if confidence_level <= 0.0 || confidence_level >= 1.0 {
        bail!(
            "confidence level must be between 0 and 1, got {}",
            confidence_level
        );
    }

    // z-score for common two-sided confidence intervals
    let z = if (confidence_level - 0.95).abs() < 1e-4 {
        1.959_963_984_540_054
    } else if (confidence_level - 0.99).abs() < 1e-4 {
        2.575_829_303_548_900_4
    } else if (confidence_level - 0.90).abs() < 1e-4 {
        1.644_853_626_951_472_2
    } else {
        // Default standard normal approximation
        1.959_963_984_540_054
    };

    let p_hat = k as f64 / n as f64;
    let n_f = n as f64;
    let z2 = z * z;

    let denom = 1.0 + z2 / n_f;
    let center = (p_hat + z2 / (2.0 * n_f)) / denom;
    let radicand = (p_hat * (1.0 - p_hat) / n_f) + (z2 / (4.0 * n_f * n_f));
    let margin = (z / denom) * radicand.max(0.0).sqrt();

    let lower = (center - margin).max(0.0);
    let upper = (center + margin).min(1.0);

    Ok((lower, upper))
}

// ---- Paired within-run ratio bounds --------------------------------------------
//
// The `paired-ratio` evaluation mode compares a ratio of two arms measured in the same
// interleaved rounds against a stored baseline ratio (a ratio of ratios). Everything
// that decides a verdict in that mode lives here, with its source and pinned values.

/// Number of bootstrap resamples used by every percentile-bootstrap interval here.
pub const BOOTSTRAP_RESAMPLES: usize = 2000;

/// Quantile of pairwise same-code drift a derived floor is set from.
pub const DERIVE_QUANTILE: f64 = 0.95;
/// Safety factor applied to the drift quantile for the axis floor.
pub const DERIVE_AXIS_SAFETY: f64 = 1.25;
/// Safety factor applied to a cell's own worst drift for its per-cell floor.
pub const DERIVE_CELL_SAFETY: f64 = 1.5;
/// Smallest floor a derivation may produce, in percent.
pub const DERIVE_MIN_FLOOR_PCT: f64 = 1.0;
/// Runs needed before a per-cell floor may be tighter than the pooled axis floor.
pub const DERIVE_PER_CELL_MIN_RUNS: usize = 8;
/// Distinct runners needed before a per-cell floor may be tighter than the axis floor.
pub const DERIVE_PER_CELL_MIN_RUNNERS: usize = 4;
/// Quantile of |control deviation| that is this run's measured noise floor.
pub const CONTROL_SCATTER_QUANTILE: f64 = 0.90;

fn check_finite_positive(xs: &[f64], what: &str) -> Result<()> {
    for (i, &x) in xs.iter().enumerate() {
        if !x.is_finite() || x < 0.0 {
            bail!("{what} {i} must be finite and non-negative, got {x}");
        }
    }
    Ok(())
}

fn sorted(xs: &[f64]) -> Vec<f64> {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

fn median_of_sorted(v: &[f64]) -> f64 {
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Sample median (the mean of the two middle order statistics for even `n`).
pub fn median(samples: &[f64]) -> Result<f64> {
    if samples.is_empty() {
        bail!("median of an empty sample");
    }
    if samples.iter().any(|x| !x.is_finite()) {
        bail!("median of a sample with a non-finite value");
    }
    Ok(median_of_sorted(&sorted(samples)))
}

/// Median in log space, `exp(median(ln x))`, used wherever ratios are pooled.
///
/// Ratios are multiplicative: 0.5x and 2.0x are the same magnitude of change in opposite
/// directions, and their arithmetic mean (1.25x) claims a change that is not there.
pub fn log_space_median(ratios: &[f64]) -> Result<f64> {
    if ratios.is_empty() {
        bail!("log-space median of an empty sample");
    }
    if ratios.iter().any(|r| !r.is_finite() || *r <= 0.0) {
        bail!("log-space median needs strictly positive, finite ratios");
    }
    let logs: Vec<f64> = ratios.iter().map(|r| r.ln()).collect();
    Ok(median(&logs)?.exp())
}

/// Nearest-rank sample quantile: `x_(ceil(n p))`, Hyndman & Fan (1996) definition 1.
///
/// Used for every "how far do things scatter" statistic in paired-ratio mode. It is a
/// sample order statistic, never an interpolation, so the value reported is one that
/// was observed.
///
/// Reference: Hyndman, R. J. and Fan, Y. (1996) *Sample quantiles in statistical
/// packages*, The American Statistician 50(4):361-365.
pub fn nearest_rank_quantile(xs: &[f64], p: f64) -> Result<f64> {
    if xs.is_empty() {
        bail!("quantile of an empty sample");
    }
    if !(p > 0.0 && p <= 1.0) {
        bail!("quantile probability must be in (0, 1], got {p}");
    }
    if xs.iter().any(|x| !x.is_finite()) {
        bail!("quantile of a sample with a non-finite value");
    }
    let v = sorted(xs);
    let rank = (p * v.len() as f64).ceil() as usize;
    Ok(v[rank.clamp(1, v.len()) - 1])
}

/// Deterministic percentile-bootstrap confidence interval for the sample median.
///
/// `B = 2000` resamples drawn with a SplitMix64 generator seeded from `n`, so the same
/// sample always yields the same interval. The interval is the `(1 - c) / 2` and
/// `(1 + c) / 2` percentiles of the resampled medians.
///
/// Reference: Efron, B. and Tibshirani, R. J. (1993) *An Introduction to the
/// Bootstrap*, Chapman & Hall, chapter 13 (the percentile interval).
pub fn bootstrap_median_ci(samples: &[f64], confidence: f64) -> Result<(f64, f64)> {
    if samples.is_empty() {
        bail!("cannot bootstrap an empty sample");
    }
    if confidence <= 0.0 || confidence >= 1.0 {
        bail!("confidence level must be in (0, 1), got {confidence}");
    }
    if samples.iter().any(|x| !x.is_finite()) {
        bail!("cannot bootstrap a sample with a non-finite value");
    }
    let n = samples.len();
    let v = sorted(samples);
    if v[0] == v[n - 1] {
        return Ok((v[0], v[0]));
    }

    let mut rng_state = 0x9e3779b97f4a7c15u64 ^ (n as u64);
    let mut next_usize = |limit: usize| -> usize {
        rng_state = rng_state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        let val = z ^ (z >> 31);
        (val % (limit as u64)) as usize
    };

    let b = BOOTSTRAP_RESAMPLES;
    let mut boot_stats = Vec::with_capacity(b);
    let mut resample_buf = vec![0.0; n];
    for _ in 0..b {
        for slot in &mut resample_buf {
            *slot = samples[next_usize(n)];
        }
        resample_buf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        boot_stats.push(median_of_sorted(&resample_buf));
    }
    boot_stats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let alpha = (1.0 - confidence) / 2.0;
    let lower_idx = (b as f64 * alpha).floor() as usize;
    let upper_idx = ((b as f64 * (1.0 - alpha)).ceil() as usize).min(b - 1);
    Ok((boot_stats[lower_idx], boot_stats[upper_idx]))
}

fn ln_choose(n: usize, k: usize) -> f64 {
    let ln_fact = |m: usize| (2..=m).map(|i| (i as f64).ln()).sum::<f64>();
    ln_fact(n) - ln_fact(k) - ln_fact(n - k)
}

/// Probability that a bootstrap resample's median equals the sample minimum.
///
/// With `n` distinct values, the resample median is the minimum iff at least
/// `floor(n / 2) + 1` of the `n` draws are the minimum, a Binomial(`n`, `1/n`) upper
/// tail. When this mass exceeds `(1 - c) / 2`, the lower end of a percentile interval
/// at level `c` is the sample minimum itself: the "interval" is then the sample range
/// and says nothing a single extreme round did not.
///
/// Reference: Efron & Tibshirani (1993), section 13.3 (percentile interval), with the
/// resample-median distribution written as a binomial tail on draw counts.
pub fn bootstrap_median_extreme_mass(n: usize) -> Result<f64> {
    if n == 0 {
        bail!("sample size must be at least 1");
    }
    let p = 1.0 / n as f64;
    let q = 1.0 - p;
    let need = n / 2 + 1;
    let mut mass = 0.0;
    for j in need..=n {
        let ln_term = ln_choose(n, j)
            + j as f64 * p.ln()
            + if n - j == 0 {
                0.0
            } else {
                (n - j) as f64 * q.ln()
            };
        mass += ln_term.exp();
    }
    Ok(mass)
}

/// Smallest number of paired rounds for which a percentile-bootstrap median interval
/// at level `confidence` does not collapse onto the sample extremes
/// (`bootstrap_median_extreme_mass(n) < (1 - confidence) / 2`).
pub fn min_rounds_for_median_ci(confidence: f64) -> Result<usize> {
    if confidence <= 0.0 || confidence >= 1.0 {
        bail!("confidence level must be in (0, 1), got {confidence}");
    }
    let alpha = (1.0 - confidence) / 2.0;
    for n in 2..=10_000 {
        if bootstrap_median_extreme_mass(n)? < alpha {
            return Ok(n);
        }
    }
    bail!("no round count up to 10000 satisfies confidence {confidence}")
}

/// Which movement of a ratio is a regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adverse {
    /// A higher ratio is worse (e.g. subject time over twin time).
    Up,
    /// A lower ratio is worse (e.g. twin bytes over subject bytes).
    Down,
}

/// Outcome of one ratio-of-ratios comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatioVerdict {
    /// The whole interval clears the threshold in the adverse direction.
    Regression,
    /// The whole interval clears the threshold in the favourable direction.
    Improvement,
    /// The point estimate is past the threshold but the interval straddles it.
    Movement,
    /// Inside the threshold.
    Null,
}

/// A ratio-of-ratios decision with the drift it was made on.
#[derive(Debug, Clone, PartialEq)]
pub struct RatioDecision {
    pub verdict: RatioVerdict,
    /// `(current / baseline - 1) x 100`.
    pub drift_pct: f64,
    /// Drift of the interval's lower end.
    pub drift_lo_pct: f64,
    /// Drift of the interval's upper end.
    pub drift_hi_pct: f64,
    pub threshold_pct: f64,
}

/// Decides a paired within-run ratio against its stored baseline ratio.
///
/// `drift = current / baseline - 1`. A cell is a regression only when the WHOLE
/// interval clears `baseline x (1 +/- threshold)` in the adverse direction; an
/// improvement when it clears in the favourable one. A point estimate past the
/// threshold with an interval that straddles it is movement, not a regression.
///
/// This is the interval form of a one-sided test against a margin: rejecting
/// "drift <= threshold" at level `(1 - c) / 2` exactly when the interval's lower end
/// exceeds the threshold, as in the two one-sided tests procedure.
///
/// Reference: Schuirmann, D. J. (1987) *A comparison of the two one-sided tests
/// procedure and the power approach for assessing the equivalence of average
/// bioavailability*, J. Pharmacokinet. Biopharm. 15(6):657-680.
pub fn ratio_of_ratios_decision(
    current: f64,
    ci: (f64, f64),
    baseline: f64,
    threshold_pct: f64,
    adverse: Adverse,
) -> Result<RatioDecision> {
    let (lo, hi) = ci;
    for (name, v) in [
        ("current ratio", current),
        ("interval lower", lo),
        ("interval upper", hi),
    ] {
        if !v.is_finite() || v <= 0.0 {
            bail!("{name} must be finite and positive, got {v}");
        }
    }
    if lo > hi {
        bail!("invalid interval: lower {lo} exceeds upper {hi}");
    }
    if !baseline.is_finite() || baseline <= 0.0 {
        bail!("baseline ratio must be finite and positive, got {baseline}");
    }
    if !threshold_pct.is_finite() || threshold_pct < 0.0 {
        bail!("threshold must be finite and non-negative, got {threshold_pct}");
    }
    let drift = |r: f64| (r / baseline - 1.0) * 100.0;
    let (drift_pct, drift_lo_pct, drift_hi_pct) = (drift(current), drift(lo), drift(hi));
    let (regressed, improved) = match adverse {
        Adverse::Up => (drift_lo_pct > threshold_pct, drift_hi_pct < -threshold_pct),
        Adverse::Down => (drift_hi_pct < -threshold_pct, drift_lo_pct > threshold_pct),
    };
    let verdict = if regressed {
        RatioVerdict::Regression
    } else if improved {
        RatioVerdict::Improvement
    } else if drift_pct.abs() > threshold_pct {
        RatioVerdict::Movement
    } else {
        RatioVerdict::Null
    };
    Ok(RatioDecision {
        verdict,
        drift_pct,
        drift_lo_pct,
        drift_hi_pct,
        threshold_pct,
    })
}

/// A derived per-cell floor.
#[derive(Debug, Clone, PartialEq)]
pub struct CellFloor {
    pub floor_pct: f64,
    /// The cell's own worst pairwise same-code drift.
    pub worst_drift_pct: f64,
    /// False when the floor exceeds the ceiling: the cell cannot resolve a regression
    /// worth a tool, so it is reported but never gated.
    pub gateable: bool,
}

/// Noise floors derived from repeated same-code runs of one axis on one platform.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedFloors {
    /// `max(1.0, p95(|drift|) x 1.25)`, rounded up to 0.5pp.
    pub axis_floor_pct: f64,
    pub p95_drift_pct: f64,
    pub pairwise_samples: usize,
    /// True once there are enough runs across enough runners for a per-cell floor to
    /// be tighter than the pooled axis floor.
    pub per_cell_below_axis_allowed: bool,
    pub cells: BTreeMap<String, CellFloor>,
}

fn round_up_half_point(x: f64) -> f64 {
    (x * 2.0).ceil() / 2.0
}

/// Derives the noise floors that set a paired-ratio threshold, from `runs` of the SAME
/// code (each a map of cell id to that run's ratio).
///
/// The spread of a ratio between runs of unchanged code IS the false-positive rate of
/// any threshold below it. Every pairwise drift `(max / min - 1) x 100` of every cell is
/// pooled. The drift is taken larger-over-smaller so the floor does not depend on the
/// order the runs are supplied in; `|a / b - 1|` differs from `|b / a - 1|`, and a rule
/// using it gives different floors for the same runs listed in a different order. The axis floor is the nearest-rank p95 of `|drift|` x 1.25,
/// rounded up to 0.5pp and floored at 1.0. Each cell's floor is its own worst drift
/// x 1.5, rounded up to 0.5pp, and is never below the axis floor until there are at
/// least 8 runs across 4 distinct runners: cross-runner drift is a systematic
/// per-runner offset, and a per-cell maximum over a handful of runs understates it.
/// A cell whose floor exceeds `ceiling_pct` is reported but not gated.
///
/// Reference: nearest-rank quantile per Hyndman & Fan (1996) definition 1; the rule
/// and its constants follow a consumer's shared-runner benchmark gate, whose first
/// gated run produced false regressions when per-cell floors were allowed below the
/// pooled axis floor on four runs.
pub fn derive_noise_floors(
    runs: &[BTreeMap<String, f64>],
    distinct_runners: usize,
    ceiling_pct: f64,
) -> Result<DerivedFloors> {
    if runs.len() < 2 {
        bail!("deriving a noise floor needs at least two runs of the same code");
    }
    if !ceiling_pct.is_finite() || ceiling_pct <= 0.0 {
        bail!("ceiling must be finite and positive, got {ceiling_pct}");
    }
    let mut by_cell: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for run in runs {
        for (id, &ratio) in run {
            if !ratio.is_finite() || ratio <= 0.0 {
                bail!("cell `{id}` has a non-positive or non-finite ratio {ratio}");
            }
            by_cell.entry(id.as_str()).or_default().push(ratio);
        }
    }
    let mut drifts = Vec::new();
    let mut worst: BTreeMap<String, f64> = BTreeMap::new();
    for (id, series) in &by_cell {
        if series.len() < 2 {
            continue;
        }
        for i in 0..series.len() {
            for j in i + 1..series.len() {
                let (hi, lo) = if series[i] >= series[j] {
                    (series[i], series[j])
                } else {
                    (series[j], series[i])
                };
                let d = (hi / lo - 1.0) * 100.0;
                drifts.push(d);
                let w = worst.entry((*id).to_string()).or_insert(0.0);
                *w = w.max(d);
            }
        }
    }
    if drifts.is_empty() {
        bail!("no cell appears in two or more runs; nothing to derive a floor from");
    }
    let p95 = nearest_rank_quantile(&drifts, DERIVE_QUANTILE)?;
    let axis_floor_pct = DERIVE_MIN_FLOOR_PCT.max(round_up_half_point(p95 * DERIVE_AXIS_SAFETY));
    let per_cell_below_axis_allowed =
        runs.len() >= DERIVE_PER_CELL_MIN_RUNS && distinct_runners >= DERIVE_PER_CELL_MIN_RUNNERS;
    let floor_min = if per_cell_below_axis_allowed {
        DERIVE_MIN_FLOOR_PCT
    } else {
        axis_floor_pct
    };
    let cells = worst
        .into_iter()
        .map(|(id, w)| {
            let floor_pct = floor_min.max(round_up_half_point(w * DERIVE_CELL_SAFETY));
            (
                id,
                CellFloor {
                    floor_pct,
                    worst_drift_pct: w,
                    gateable: floor_pct <= ceiling_pct,
                },
            )
        })
        .collect();
    Ok(DerivedFloors {
        axis_floor_pct,
        p95_drift_pct: p95,
        pairwise_samples: drifts.len(),
        per_cell_below_axis_allowed,
        cells,
    })
}

/// This run's measured noise floor: the nearest-rank p90 of `|r - 1| x 100` over the
/// in-situ control cells (two builds of identical source, so every ratio should be 1).
///
/// p90, not the maximum: over many cells the maximum is an extreme-value statistic that
/// reads high on a healthy runner and would pin every threshold wide.
pub fn control_scatter_pct(control_ratios: &[f64]) -> Result<f64> {
    check_finite_positive(control_ratios, "control ratio")?;
    let devs: Vec<f64> = control_ratios
        .iter()
        .map(|r| ((r - 1.0) * 100.0).abs())
        .collect();
    nearest_rank_quantile(&devs, CONTROL_SCATTER_QUANTILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discrete_metric_exact_tolerance() {
        let base = DiscreteMetric::new(100_000);
        let head_clean = DiscreteMetric::new(100_400); // +0.4%
        let head_regressed = DiscreteMetric::new(101_000); // +1.0%

        assert_eq!(base.delta_pct(&head_clean).unwrap(), 0.4);
        assert!(!base.regressed(&head_clean, 0.5).unwrap());

        assert_eq!(base.delta_pct(&head_regressed).unwrap(), 1.0);
        assert!(base.regressed(&head_regressed, 0.5).unwrap());
    }

    #[test]
    fn test_overlapping_ci_point_regression_passes_audit_case() {
        // Audit Case 1: Point estimate +2.0%, but overlapping 95% CIs [900, 1100] vs [920, 1120] at 0.5% tolerance.
        // Under naive point estimate comparison: (1020 - 1000) / 1000 = +2.0% > 0.5% -> FAILED.
        // Under conservative CI bound: L_head (920) <= U_base (1100) -> delta_min <= 0 -> PASSES (is_regression = false).
        let base = ContinuousEstimate::with_ci(1000.0, 900.0, 1100.0, 0.95, "ns").unwrap();
        let head = ContinuousEstimate::with_ci(1020.0, 920.0, 1120.0, 0.95, "ns").unwrap();

        let decision = evaluate_continuous_regression(&base, &head, 0.5).unwrap();
        assert_eq!(decision.method, "conservative_interval");
        assert!(
            !decision.is_regression,
            "Overlapping CIs must not be flagged as regression"
        );
        assert!(decision.point_delta_pct > 0.5);
        assert!(decision.delta_pct <= 0.0);
    }

    #[test]
    fn test_cleared_interval_regression_fails() {
        // Genuine statistically verified regression:
        // Base [900, 1100], Head [1200, 1400], tolerance 0.5%
        // delta_min = (1200 - 1100) / 1100 = +9.09% > 0.5% -> is_regression = true.
        let base = ContinuousEstimate::with_ci(1000.0, 900.0, 1100.0, 0.95, "ns").unwrap();
        let head = ContinuousEstimate::with_ci(1300.0, 1200.0, 1400.0, 0.95, "ns").unwrap();

        let decision = evaluate_continuous_regression(&base, &head, 0.5).unwrap();
        assert!(decision.is_regression);
        assert!(decision.delta_pct > 0.5);
        assert_eq!(decision.method, "conservative_interval");
    }

    #[test]
    fn test_missing_interval_degrades_to_not_comparable() {
        let base = ContinuousEstimate::point_only(10.0, "s").unwrap();
        let head = ContinuousEstimate::point_only(10.2, "s").unwrap(); // +2.0%

        let decision = evaluate_continuous_regression(&base, &head, 0.5).unwrap();
        assert_eq!(decision.method, "not_comparable_no_ci");
        assert!(
            !decision.is_regression,
            "missing CI must never fail as a regression"
        );
        assert!((decision.point_delta_pct - 2.0).abs() < 1e-6);
        assert!(decision.note.contains("not comparable (no CI available)"));
    }

    #[test]
    fn zero_base_point_estimate_is_not_comparable() {
        let zero = ContinuousEstimate::point_only(0.0, "ms").unwrap();
        let head = ContinuousEstimate::point_only(1.0, "ms").unwrap();
        let decision = evaluate_continuous_regression(&zero, &head, 0.5)
            .expect("a zero base point estimate must not bail");
        assert!(!decision.is_regression);
        assert_eq!(decision.method, "not_comparable_zero_base");
        assert!(decision.note.contains("not comparable"));

        let zero_ci = ContinuousEstimate::with_ci(0.0, 0.0, 0.0, 0.95, "ms").unwrap();
        let decision_ci = evaluate_continuous_regression(&zero_ci, &zero_ci, 0.5).unwrap();
        assert_eq!(decision_ci.method, "not_comparable_zero_base");
    }

    #[test]
    fn discrete_zero_to_zero_is_unchanged() {
        let zero = DiscreteMetric::new(0);
        assert_eq!(zero.delta_pct(&DiscreteMetric::new(0)).unwrap(), 0.0);
        assert!(zero.delta_pct(&DiscreteMetric::new(1)).is_err());
    }

    #[test]
    fn test_wilson_score_interval_pinned_reference_values() {
        // Pinned reference value: 90 successes out of 100 trials, 95% confidence interval
        // Reference: Wilson (1927); standard scipy.stats proportion_confint(90, 100, method='wilson')
        // Expected interval: [0.825747, 0.944983]
        let (lower, upper) = wilson_score_interval(90, 100, 0.95).unwrap();
        assert!((lower - 0.8257).abs() < 0.005, "lower: {}", lower);
        assert!((upper - 0.9450).abs() < 0.005, "upper: {}", upper);

        // Reference value: 0 successes out of 10 trials
        let (lower_zero, upper_zero) = wilson_score_interval(0, 10, 0.95).unwrap();
        assert_eq!(lower_zero, 0.0);
        assert!((upper_zero - 0.2775).abs() < 0.01, "upper: {}", upper_zero);
    }

    #[test]
    fn test_continuous_estimate_cv_and_noise_detection() {
        let est = ContinuousEstimate::point_only(100.0, "ns")
            .unwrap()
            .with_std_dev(15.0);
        assert_eq!(est.cv().unwrap(), 0.15);
        assert!(!est.is_noisy(0.20));
        assert!(est.is_noisy(0.10));
    }

    #[test]
    fn test_continuous_estimate_from_samples() {
        let samples = [14.3895, 14.476, 14.5057, 14.5314, 14.5369, 14.538, 14.5991];
        let est = ContinuousEstimate::from_samples(&samples, Some(14.5314), "ms").unwrap();
        assert_eq!(est.point_estimate, 14.5314);
        assert_eq!(est.unit, "ms");
        let ci = est.ci.expect("CI must be computed from sample runs");
        assert!(ci.lower <= est.point_estimate);
        assert!(ci.upper >= est.point_estimate);
        assert!(ci.lower >= 14.38);
        assert!(ci.upper <= 14.60);
        assert!(est.std_dev.unwrap() > 0.0);

        // Identical samples produce exact CI [val, val] and zero std dev
        let uniform = [5.0, 5.0, 5.0];
        let est_uni = ContinuousEstimate::from_samples(&uniform, None, "ns").unwrap();
        assert_eq!(est_uni.point_estimate, 5.0);
        assert_eq!(est_uni.std_dev.unwrap(), 0.0);
        assert_eq!(est_uni.ci.unwrap().lower, 5.0);
        assert_eq!(est_uni.ci.unwrap().upper, 5.0);

        // Single sample degrades to point-only
        let single = [42.0];
        let est_single = ContinuousEstimate::from_samples(&single, None, "ns").unwrap();
        assert_eq!(est_single.point_estimate, 42.0);
        assert!(est_single.ci.is_none());

        // Empty sample fails
        assert!(ContinuousEstimate::from_samples(&[], None, "ns").is_err());

        // Deterministic bootstrap yields identical intervals across runs
        let est2 = ContinuousEstimate::from_samples(&samples, Some(14.5314), "ms").unwrap();
        assert_eq!(est.ci.unwrap().lower, est2.ci.unwrap().lower);
        assert_eq!(est.ci.unwrap().upper, est2.ci.unwrap().upper);
    }

    // ---- Paired within-run ratio bounds -----------------------------------------

    #[test]
    fn bootstrap_median_extreme_mass_pins_binomial_tail() {
        // Reference: scipy.stats.binom.sf(n // 2, n, 1 / n), the probability that a
        // bootstrap resample's median is the sample minimum.
        let pinned = [
            (3usize, 0.259_259_259_259_259_2),
            (4, 0.050_781_25),
            (5, 0.057_920_000_000_000_01),
            (6, 0.008_701_989_026_063_098),
            (7, 0.010_150_046_809_941_922),
            (8, 0.001_230_061_054_229_736_3),
        ];
        for (n, want) in pinned {
            let got = bootstrap_median_extreme_mass(n).unwrap();
            assert!((got - want).abs() < 1e-12, "n={n}: got {got}, want {want}");
        }
        assert!(bootstrap_median_extreme_mass(0).is_err());
    }

    #[test]
    fn min_rounds_for_median_ci_is_six_at_95_percent() {
        // scipy: min n with binom.sf(n // 2, n, 1 / n) < 0.025 is 6; < 0.005 is 8.
        assert_eq!(min_rounds_for_median_ci(0.95).unwrap(), 6);
        assert_eq!(min_rounds_for_median_ci(0.99).unwrap(), 8);
        assert!(min_rounds_for_median_ci(1.0).is_err());
    }

    #[test]
    fn bootstrap_median_ci_lands_on_exact_bootstrap_order_statistics() {
        // The exact percentile-bootstrap distribution of the median of n = 19 distinct
        // values puts P(median <= x_(j)) = binom.sf(9, 19, j / 19). The 2.5% / 97.5%
        // cut points computed with scipy fall on x_(6) and x_(14), each more than three
        // Monte Carlo standard errors from the neighbouring order statistic.
        let xs: Vec<f64> = (1..=19).map(f64::from).collect();
        let (lo, hi) = bootstrap_median_ci(&xs, 0.95).unwrap();
        assert_eq!((lo, hi), (6.0, 14.0));

        // n = 21: exact cut points are x_(7) and x_(15), but P(median <= x_(6)) is
        // 0.0183, 2 standard errors from the 0.0255 cut, and this generator draws 53 of
        // 2000 resample medians at or below x_(6) (36.7 expected). An independent Python
        // re-implementation of the same SplitMix64 stream reproduces (6, 15).
        let xs: Vec<f64> = (1..=21).map(f64::from).collect();
        let (lo, hi) = bootstrap_median_ci(&xs, 0.95).unwrap();
        assert_eq!((lo, hi), (6.0, 15.0));
        // Deterministic.
        assert_eq!(bootstrap_median_ci(&xs, 0.95).unwrap(), (lo, hi));
        // Degenerate inputs.
        assert_eq!(
            bootstrap_median_ci(&[2.0, 2.0, 2.0], 0.95).unwrap(),
            (2.0, 2.0)
        );
        assert!(bootstrap_median_ci(&[], 0.95).is_err());
        assert!(bootstrap_median_ci(&[1.0, f64::NAN], 0.95).is_err());
    }

    #[test]
    fn median_and_log_space_median() {
        assert_eq!(median(&[3.0, 1.0, 2.0]).unwrap(), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]).unwrap(), 2.5);
        // exp(median(ln x)) of {0.5, 1, 2} is 1: ratios combine multiplicatively.
        assert!((log_space_median(&[0.5, 1.0, 2.0]).unwrap() - 1.0).abs() < 1e-12);
        assert!(log_space_median(&[1.0, 0.0]).is_err());
    }

    #[test]
    fn nearest_rank_quantile_pins_hyndman_fan_type_1() {
        let xs: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(nearest_rank_quantile(&xs, 0.9).unwrap(), 9.0);
        assert_eq!(nearest_rank_quantile(&xs, 0.95).unwrap(), 10.0);
        assert_eq!(nearest_rank_quantile(&xs, 0.5).unwrap(), 5.0);
        assert_eq!(nearest_rank_quantile(&[3.0], 0.95).unwrap(), 3.0);
        assert!(nearest_rank_quantile(&[], 0.5).is_err());
        assert!(nearest_rank_quantile(&xs, 0.0).is_err());
    }

    #[test]
    fn ratio_of_ratios_decision_requires_the_whole_interval_to_clear() {
        let d = ratio_of_ratios_decision(1.10, (1.07, 1.13), 1.0, 5.0, Adverse::Up).unwrap();
        assert_eq!(d.verdict, RatioVerdict::Regression);
        assert!((d.drift_pct - 10.0).abs() < 1e-9);
        assert!((d.drift_lo_pct - 7.0).abs() < 1e-9);
        assert!((d.drift_hi_pct - 13.0).abs() < 1e-9);

        // Point past the threshold, interval straddling it: movement, never a regression.
        let d = ratio_of_ratios_decision(1.10, (1.03, 1.13), 1.0, 5.0, Adverse::Up).unwrap();
        assert_eq!(d.verdict, RatioVerdict::Movement);

        // Improvements are reported and are never a regression.
        let d = ratio_of_ratios_decision(0.90, (0.87, 0.93), 1.0, 5.0, Adverse::Up).unwrap();
        assert_eq!(d.verdict, RatioVerdict::Improvement);

        let d = ratio_of_ratios_decision(1.02, (0.99, 1.05), 1.0, 5.0, Adverse::Up).unwrap();
        assert_eq!(d.verdict, RatioVerdict::Null);

        // Adverse direction down (a memory advantage shrinking).
        let d = ratio_of_ratios_decision(0.90, (0.87, 0.93), 1.0, 5.0, Adverse::Down).unwrap();
        assert_eq!(d.verdict, RatioVerdict::Regression);
        let d = ratio_of_ratios_decision(1.10, (1.07, 1.13), 1.0, 5.0, Adverse::Down).unwrap();
        assert_eq!(d.verdict, RatioVerdict::Improvement);

        // Exactly on the threshold does not clear it (1.25 and 25.0 are exact in binary).
        let d = ratio_of_ratios_decision(1.25, (1.25, 1.25), 1.0, 25.0, Adverse::Up).unwrap();
        assert_ne!(d.verdict, RatioVerdict::Regression);

        assert!(ratio_of_ratios_decision(1.0, (1.1, 0.9), 1.0, 5.0, Adverse::Up).is_err());
        assert!(ratio_of_ratios_decision(1.0, (0.9, 1.1), 0.0, 5.0, Adverse::Up).is_err());
        assert!(ratio_of_ratios_decision(1.0, (0.9, 1.1), 1.0, -1.0, Adverse::Up).is_err());
    }

    #[test]
    fn derive_noise_floors_pins_reference_values() {
        // Reference computed in Python with the same rule: every pairwise between-run
        // drift (larger over smaller), axis floor = max(1.0, p95 x 1.25 rounded up
        // to 0.5pp), cell floor = max(axis floor, own worst x 1.5 rounded up to 0.5pp)
        // while runs < 8 or distinct runners < 4.
        let run = |a: f64, b: f64, c: f64| {
            BTreeMap::from([
                ("a".to_string(), a),
                ("b".to_string(), b),
                ("c".to_string(), c),
            ])
        };
        let runs = vec![
            run(1.00, 2.00, 0.50),
            run(1.02, 2.10, 0.50),
            run(0.99, 1.90, 0.51),
        ];
        let d = derive_noise_floors(&runs, 2, 50.0).unwrap();
        assert_eq!(d.pairwise_samples, 9);
        assert!((d.p95_drift_pct - 10.526_315_789_473_696).abs() < 1e-9);
        assert_eq!(d.axis_floor_pct, 13.5);
        assert!(!d.per_cell_below_axis_allowed);
        assert_eq!(d.cells["a"].floor_pct, 13.5);
        assert_eq!(d.cells["b"].floor_pct, 16.0);
        assert!((d.cells["b"].worst_drift_pct - 10.526_315_789_473_696).abs() < 1e-9);
        assert_eq!(d.cells["c"].floor_pct, 13.5);
        assert!(d.cells.values().all(|c| c.gateable));

        // The order the runs are listed in does not move any floor.
        let mut reversed = runs.clone();
        reversed.reverse();
        assert_eq!(derive_noise_floors(&reversed, 2, 50.0).unwrap(), d);

        // A ceiling below a cell's floor reports the cell as not gateable.
        let d = derive_noise_floors(&runs, 2, 15.0).unwrap();
        assert!(!d.cells["b"].gateable);
        assert!(d.cells["a"].gateable);

        // Four runners are not enough on their own: fewer than 8 runs keep the axis floor
        // as the lower bound for every cell.
        let few: Vec<_> = (0..4)
            .map(|i| run(1.0 + 0.001 * f64::from(i), 2.0, 0.5))
            .collect();
        let d = derive_noise_floors(&few, 4, 50.0).unwrap();
        assert!(!d.per_cell_below_axis_allowed);
        assert_eq!(d.cells["b"].floor_pct, d.axis_floor_pct);

        // Enough runs across enough runners lets a cell go below the axis floor.
        let many: Vec<_> = (0..8)
            .map(|i| run(1.0 + 0.001 * f64::from(i), 2.0, 0.5))
            .collect();
        let d = derive_noise_floors(&many, 4, 50.0).unwrap();
        assert!(d.per_cell_below_axis_allowed);
        assert_eq!(d.cells["b"].floor_pct, 1.0);

        assert!(derive_noise_floors(&runs[..1], 1, 50.0).is_err());
    }

    #[test]
    fn control_scatter_is_p90_of_absolute_deviation() {
        let ratios: Vec<f64> = (1..=10).map(|i| 1.0 + f64::from(i) / 100.0).collect();
        // |r - 1| x 100 = 1..10; nearest-rank p90 = 9.
        assert!((control_scatter_pct(&ratios).unwrap() - 9.0).abs() < 1e-9);
        assert!(control_scatter_pct(&[]).is_err());
    }
}
