#!/usr/bin/env python3
"""scripts/benchmark_power_audit.py — Statistical power & sample-size audit for erosion benchmark.

Implements the math-first derivations for Phase B2 of the erosion benchmark (Refs #482):
  1. Wilson 95% score confidence interval for binomial proportions (Wilson 1927).
  2. Sample size required for target Wilson score interval half-width (analytical margin inversion).
  3. Minimum detectable difference (MDD) between discipline and the natural substitution twin (Fleiss 2003; Chow 2008).

When run with `--test`, executes self-contained unit tests pinning known reference values.
When run without `--test`, computes and prints the audit table for the erosion benchmark.

References:
  - Wilson, E. B. (1927). "Probable inference, the law of succession, and statistical inference".
    Journal of the American Statistical Association, 22(158), 209-212.
  - Acklam, P. J. (2003). "An algorithm for computing the inverse normal cumulative distribution function".
  - Fleiss, J. L., Levin, B., & Paik, M. C. (2003). "Statistical Methods for Rates and Proportions". 3rd ed.
  - Chow, S. C., Shao, J., & Wang, H. (2008). "Sample Size Calculations in Clinical Research". 2nd ed.
"""

import math
import sys
import unittest


def standard_normal_quantile(p: float) -> float:
    """Standard normal quantile function (inverse CDF, \Phi^{-1}(p)).

    Uses Peter J. Acklam's rational Chebyshev approximation algorithm.
    Precision: absolute error < 1.15e-9 across the entire domain (0, 1).
    """
    if p <= 0.0 or p >= 1.0 or math.isnan(p):
        raise ValueError(f"probability p must be strictly between 0 and 1, got {p}")

    # Exact constants for common statistical critical values
    if abs(p - 0.975) < 1e-6:
        return 1.959963984540054
    if abs(p - 0.025) < 1e-6:
        return -1.959963984540054
    if abs(p - 0.995) < 1e-6:
        return 2.5758293035489004
    if abs(p - 0.005) < 1e-6:
        return -2.5758293035489004
    if abs(p - 0.95) < 1e-6:
        return 1.6448536269514722
    if abs(p - 0.05) < 1e-6:
        return -1.6448536269514722
    if abs(p - 0.80) < 1e-6:
        return 0.8416212335729143
    if abs(p - 0.20) < 1e-6:
        return -0.8416212335729143
    if abs(p - 0.90) < 1e-6:
        return 1.2815515655446004
    if abs(p - 0.10) < 1e-6:
        return -1.2815515655446004

    # Coefficients in rational approximations
    a = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383577518672690e+02,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ]
    b = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ]
    c = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ]
    d = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ]

    p_low = 0.02425
    p_high = 1.0 - p_low

    if p < p_low:
        q = math.sqrt(-2.0 * math.log(p))
        num = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]
        den = (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
        return num / den
    elif p <= p_high:
        q = p - 0.5
        r = q * q
        num = (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
        den = ((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0
        return num / den
    else:
        q = math.sqrt(-2.0 * math.log(1.0 - p))
        num = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]
        den = (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
        return -num / den


def wilson_score_interval(k: int, n: int, confidence_level: float = 0.95) -> tuple[float, float]:
    """Compute the Wilson score confidence interval for binomial proportion k / n.

    Reference: Wilson (1927).
    """
    if n <= 0:
        raise ValueError("sample size n must be greater than 0")
    if k < 0 or k > n:
        raise ValueError(f"successes k ({k}) cannot exceed total trials n ({n})")
    if confidence_level <= 0.0 or confidence_level >= 1.0 or math.isnan(confidence_level):
        raise ValueError(f"confidence level must be between 0 and 1, got {confidence_level}")

    alpha = 1.0 - confidence_level
    z = standard_normal_quantile(1.0 - alpha / 2.0)

    p_hat = k / n
    z2 = z * z
    denom = 1.0 + z2 / n
    center = (p_hat + z2 / (2.0 * n)) / denom
    radicand = (p_hat * (1.0 - p_hat) / n) + (z2 / (4.0 * n * n))
    margin = (z / denom) * math.sqrt(max(0.0, radicand))

    lower = max(0.0, center - margin)
    upper = min(1.0, center + margin)
    return (lower, upper)


def sample_size_for_interval(
    assumed_precision: float,
    target_half_width: float,
    confidence_level: float = 0.95,
) -> int:
    """Compute the minimum sample size n such that the Wilson score interval half-width <= target_half_width.

    Solves the Wilson margin equation analytically:
      w = (z / (1 + z^2 / n)) * sqrt(p * (1 - p) / n + z^2 / (4 n^2)) <= target_half_width
    which inverts to the closed-form quadratic solution:
      n = ceil( z^2 * (p * (1 - p) - 2 * w^2 + sqrt((p * (1 - p))^2 + w^2 * (1 - 2p)^2)) / (2 * w^2) )
    """
    if assumed_precision <= 0.0 or assumed_precision >= 1.0 or math.isnan(assumed_precision):
        raise ValueError(f"assumed precision must be strictly between 0 and 1, got {assumed_precision}")
    if target_half_width <= 0.0 or target_half_width >= 0.5 or math.isnan(target_half_width):
        raise ValueError(f"target half-width must be strictly between 0 and 0.5, got {target_half_width}")
    if confidence_level <= 0.0 or confidence_level >= 1.0 or math.isnan(confidence_level):
        raise ValueError(f"confidence level must be between 0 and 1, got {confidence_level}")

    alpha = 1.0 - confidence_level
    z = standard_normal_quantile(1.0 - alpha / 2.0)
    p = assumed_precision
    w = target_half_width

    v = p * (1.0 - p)
    w2 = w * w
    term1 = v - 2.0 * w2
    term2 = math.sqrt(v * v + w2 * (1.0 - 2.0 * p) ** 2)
    n = (z * z * (term1 + term2)) / (2.0 * w2)
    return max(1, math.ceil(n))


def minimum_detectable_difference(
    n1: int,
    n2: int,
    baseline_precision: float,
    confidence_level: float = 0.95,
    power: float = 0.80,
) -> float:
    """Compute the minimum detectable difference (MDD) in precision between discipline (n1) and twin (n2).

    Solves the standard two-sample proportion test power equation:
      delta = (z_{alpha/2} + z_beta) * sqrt( p1 * (1 - p1) / n1 + (p1 + delta) * (1 - p1 - delta) / n2 )
    expanding to the quadratic equation:
      (1 + K2) * delta^2 - K2 * (1 - 2 * p1) * delta - (K1 + K2) * p1 * (1 - p1) = 0
    where K1 = (z_{alpha/2} + z_beta)^2 / n1 and K2 = (z_{alpha/2} + z_beta)^2 / n2.
    """
    if n1 <= 0:
        raise ValueError("sample size n1 must be greater than 0")
    if n2 <= 0:
        raise ValueError("sample size n2 must be greater than 0")
    if baseline_precision <= 0.0 or baseline_precision >= 1.0 or math.isnan(baseline_precision):
        raise ValueError(f"baseline precision must be strictly between 0 and 1, got {baseline_precision}")
    if confidence_level <= 0.0 or confidence_level >= 1.0 or math.isnan(confidence_level):
        raise ValueError(f"confidence level must be between 0 and 1, got {confidence_level}")
    if power <= 0.0 or power >= 1.0 or math.isnan(power):
        raise ValueError(f"statistical power must be between 0 and 1, got {power}")

    alpha = 1.0 - confidence_level
    z_alpha = standard_normal_quantile(1.0 - alpha / 2.0)
    z_beta = standard_normal_quantile(power)
    z_sum_sq = (z_alpha + z_beta) ** 2

    k1 = z_sum_sq / n1
    k2 = z_sum_sq / n2
    p1 = baseline_precision
    v1 = p1 * (1.0 - p1)

    a = 1.0 + k2
    b = -k2 * (1.0 - 2.0 * p1)
    c = -(k1 + k2) * v1

    disc = max(0.0, b * b - 4.0 * a * c)
    delta = (-b + math.sqrt(disc)) / (2.0 * a)
    return delta


class TestStatisticalBounds(unittest.TestCase):
    """Unit tests pinning reference values for statistical power and bounds functions."""

    def test_standard_normal_quantile_pinned_reference_values(self):
        self.assertAlmostEqual(standard_normal_quantile(0.975), 1.959963984540054, places=6)
        self.assertAlmostEqual(standard_normal_quantile(0.025), -1.959963984540054, places=6)
        self.assertAlmostEqual(standard_normal_quantile(0.995), 2.5758293035489004, places=6)
        self.assertAlmostEqual(standard_normal_quantile(0.95), 1.6448536269514722, places=6)
        self.assertAlmostEqual(standard_normal_quantile(0.80), 0.8416212335729143, places=6)
        self.assertAlmostEqual(standard_normal_quantile(0.90), 1.2815515655446004, places=6)

        # Symmetry
        for p in [0.01, 0.05, 0.10, 0.25, 0.40, 0.50, 0.75, 0.90, 0.99]:
            self.assertAlmostEqual(standard_normal_quantile(p), -standard_normal_quantile(1.0 - p), places=6)

    def test_sample_size_for_interval_pinned_reference_values(self):
        # Pinned reference: p=0.90, w=0.05, 95% CI -> n=141
        self.assertEqual(sample_size_for_interval(0.90, 0.05, 0.95), 141)

        # Verification against Wilson interval margin at n=141 and n=140
        low_141, up_141 = wilson_score_interval(127, 141, 0.95)
        self.assertLessEqual((up_141 - low_141) / 2.0, 0.05)

        low_140, up_140 = wilson_score_interval(126, 140, 0.95)
        self.assertGreater((up_140 - low_140) / 2.0, 0.05)

        # Grid of pinned values
        self.assertEqual(sample_size_for_interval(0.80, 0.05, 0.95), 245)
        self.assertEqual(sample_size_for_interval(0.50, 0.05, 0.95), 381)
        self.assertEqual(sample_size_for_interval(0.95, 0.05, 0.95), 83)
        self.assertEqual(sample_size_for_interval(0.80, 0.10, 0.95), 60)
        self.assertEqual(sample_size_for_interval(0.90, 0.05, 0.99), 244)
        self.assertEqual(sample_size_for_interval(0.90, 0.05, 0.90), 100)

        # Monotonicity
        self.assertGreater(
            sample_size_for_interval(0.90, 0.02, 0.95),
            sample_size_for_interval(0.90, 0.05, 0.95),
        )

    def test_minimum_detectable_difference_pinned_reference_values(self):
        # Pinned reference values
        d1 = minimum_detectable_difference(100, 100, 0.60, 0.95, 0.80)
        self.assertAlmostEqual(d1, 0.179767, places=4)

        d2 = minimum_detectable_difference(200, 200, 0.70, 0.95, 0.80)
        self.assertAlmostEqual(d2, 0.118611, places=4)

        d3 = minimum_detectable_difference(50, 100, 0.50, 0.95, 0.80)
        self.assertAlmostEqual(d3, 0.233629, places=4)

        d4 = minimum_detectable_difference(141, 141, 0.80, 0.95, 0.80)
        self.assertAlmostEqual(d4, 0.115040, places=4)

        # Larger sample size gives smaller MDD
        d_n100 = minimum_detectable_difference(100, 100, 0.60, 0.95, 0.80)
        d_n400 = minimum_detectable_difference(400, 400, 0.60, 0.95, 0.80)
        self.assertLess(d_n400, d_n100)


def generate_power_audit() -> str:
    """Generate the pre-registration power & sample size audit table for the erosion benchmark."""
    gates = [
        ("test-floor", 150, 0.90, 0.05),
        ("assertion-reduction", 150, 0.90, 0.05),
        ("unsafe-safety-comment", 100, 0.95, 0.05),
        ("ci-integrity", 80, 0.90, 0.05),
        ("harness-tampering", 50, 0.95, 0.05),
        ("stub-bodies", 100, 0.90, 0.05),
        ("error-swallowing", 80, 0.90, 0.05),
        ("version-lockstep", 60, 0.90, 0.05),
        ("config-integrity", 50, 0.90, 0.05),
    ]

    baseline_precision = 0.60
    confidence = 0.95
    power = 0.80

    lines = [
        "# Erosion Benchmark Pre-Registration Power & Sample-Size Audit",
        "",
        "| Gate | Planned Findings (n) | Assumed Precision | Target Half-Width (w) | Required n | Power Verdict | MDD vs Twin Baseline (p_twin=0.60, 80% pwr) |",
        "|---|---|---|---|---|---|---|",
    ]

    for name, planned_n, assumed_p, target_w in gates:
        req_n = sample_size_for_interval(assumed_p, target_w, confidence)
        status = "SUFFICIENT" if planned_n >= req_n else "insufficient-n"
        mdd = minimum_detectable_difference(planned_n, planned_n, baseline_precision, confidence, power)
        lines.append(
            f"| `{name}` | {planned_n} | {assumed_p:.2f} | {target_w:.2f} | {req_n} | `{status}` | +{mdd*100:.1f}% |"
        )

    lines.append("")
    lines.append("Audit rules (Refs #482 §B2):")
    lines.append("- A gate whose planned findings in the evaluated population cannot clear `Required n` is declared `insufficient-n` in advance rather than run.")
    lines.append("- Wilson 95% intervals computed per Rule 1.1; claim PASS iff CI lower bound >= floor.")
    return "\n".join(lines)


if __name__ == "__main__":
    if "--test" in sys.argv:
        sys.argv.remove("--test")
        unittest.main()
    else:
        print(generate_power_audit())
