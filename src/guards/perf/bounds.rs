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
//!
//! # References
//! - Wilson, E. B. (1927) *Probable inference, the law of succession, and statistical inference*,
//!   J. Am. Stat. Assoc. 22:209-212.

use anyhow::{bail, Result};

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
    /// Returns an error if base is 0.
    pub fn delta_pct(&self, head: &DiscreteMetric) -> Result<f64> {
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
    /// Method used: `"conservative_interval"` or `"point_estimate_only"`.
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
pub fn evaluate_continuous_regression(
    base: &ContinuousEstimate,
    head: &ContinuousEstimate,
    tolerance_pct: f64,
) -> Result<RegressionDecision> {
    if base.point_estimate == 0.0 {
        bail!("cannot compute regression against base point estimate of 0.0");
    }

    let point_delta_pct =
        ((head.point_estimate - base.point_estimate) / base.point_estimate) * 100.0;

    if let (Some(base_ci), Some(head_ci)) = (&base.ci, &head.ci) {
        if base_ci.upper == 0.0 {
            bail!("cannot compute regression against base upper confidence bound of 0.0");
        }

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
}
