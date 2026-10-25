#!/usr/bin/env python3
"""scripts/benchmark_power_audit.py — Statistical power & sample-size audit for erosion benchmark.

Implements the math-first derivations for Phase B2 of the erosion benchmark (Refs #482):
  1. Wilson 95% score confidence interval for binomial proportions (Wilson 1927).
  2. Sample size required for target Wilson score interval half-width (analytical margin inversion).
  3. Minimum detectable difference (MDD) between discipline and the natural substitution twin (Fleiss 2003; Chow 2008).
  4. Recall study Step 0 (#838, `docs/benchmarks/recall-preregistration.toml`): the exact probability
     that a Wilson lower bound clears a floor, the sample sizes at which it clears at the expected
     proportion and with a given power, and the agent runs needed to reach them.

When run with `--test` (or `--self-check`), executes self-contained unit tests pinning known reference values.
With `--recall-bounds-json`, prints the recall study's Step-0 values as JSON.
When run without either, computes and prints the audit table for the erosion benchmark and the recall study.

References:
  - Wilson, E. B. (1927). "Probable inference, the law of succession, and statistical inference".
    Journal of the American Statistical Association, 22(158), 209-212.
  - Acklam, P. J. (2003). "An algorithm for computing the inverse normal cumulative distribution function".
  - Fleiss, J. L., Levin, B., & Paik, M. C. (2003). "Statistical Methods for Rates and Proportions". 3rd ed.
  - Chow, S. C., Shao, J., & Wang, H. (2008). "Sample Size Calculations in Clinical Research". 2nd ed.
"""

import json
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


def _validate_unit_interval(name: str, value: float) -> None:
    if not isinstance(value, (int, float)) or math.isnan(value) or value <= 0.0 or value >= 1.0:
        raise ValueError(f"{name} must be strictly between 0 and 1, got {value}")


def _round_half_up(x: float) -> int:
    return int(math.floor(x + 0.5))


def _binomial_pmf(k: int, n: int, p: float) -> float:
    log_pmf = (
        math.lgamma(n + 1)
        - math.lgamma(k + 1)
        - math.lgamma(n - k + 1)
        + k * math.log(p)
        + (n - k) * math.log1p(-p)
    )
    return math.exp(log_pmf)


def wilson_pass_probability(n: int, true_p: float, floor: float, confidence_level: float = 0.95) -> float:
    """Exact probability that the Wilson lower bound of k/n clears `floor` when k ~ Binomial(n, true_p).

    "Clears" is `lower >= floor`, the verdict rule of the pre-registrations. The Wilson lower
    bound is increasing in k, so the event is k >= k_min with k_min the smallest count that
    clears, and the probability is the binomial upper tail from k_min (0.0 when no k <= n clears).

    Reference: Wilson (1927), the score interval `wilson_score_interval` implements.
    """
    if not isinstance(n, int) or isinstance(n, bool) or n <= 0:
        raise ValueError(f"sample size n must be a positive integer, got {n}")
    _validate_unit_interval("true proportion", true_p)
    _validate_unit_interval("floor", floor)
    _validate_unit_interval("confidence level", confidence_level)
    if wilson_score_interval(n, n, confidence_level)[0] < floor:
        return 0.0
    lo, hi = 0, n  # the lower bound is increasing in k: bisect for the smallest clearing k
    while lo < hi:
        mid = (lo + hi) // 2
        if wilson_score_interval(mid, n, confidence_level)[0] >= floor:
            hi = mid
        else:
            lo = mid + 1
    k_min = lo
    return min(1.0, sum(_binomial_pmf(k, n, true_p) for k in range(k_min, n + 1)))


def _validate_sample_size_inputs(expected: float, floor: float, confidence_level: float, n_max: int) -> None:
    _validate_unit_interval("expected proportion", expected)
    _validate_unit_interval("floor", floor)
    _validate_unit_interval("confidence level", confidence_level)
    if floor >= expected:
        raise ValueError(f"floor ({floor}) must be below the expected proportion ({expected})")
    if not isinstance(n_max, int) or isinstance(n_max, bool) or n_max <= 0:
        raise ValueError(f"n_max must be a positive integer, got {n_max}")


def sample_size_for_wilson_lower_bound(expected: float, floor: float, confidence_level: float, n_max: int) -> int:
    """Smallest n such that every n' in [n, n_max] clears the floor at the expected point estimate.

    At each n' the count is k = round_half_up(expected * n'), and n' clears when the Wilson
    lower bound of k/n' is >= floor. The rounding makes clearance non-monotone near the
    threshold (an n' can clear while n' + 1 does not), so the answer is the start of the
    last unbroken run of clearing sizes ending at n_max, not the first clearing size.
    Raises ValueError when n_max itself does not clear.

    This is the point-estimate requirement: a study that observes exactly the expected
    proportion passes. It says nothing about the chance of observing it; see
    `sample_size_for_wilson_lower_bound_power`.

    Reference: Wilson (1927), the score interval `wilson_score_interval` implements.
    """
    _validate_sample_size_inputs(expected, floor, confidence_level, n_max)

    def clears(m: int) -> bool:
        return wilson_score_interval(_round_half_up(expected * m), m, confidence_level)[0] >= floor

    if not clears(n_max):
        raise ValueError(f"n_max ({n_max}) does not clear floor {floor} at expected {expected}")
    n = n_max
    while n > 1 and clears(n - 1):
        n -= 1
    return n


def sample_size_for_wilson_lower_bound_power(
    expected: float, floor: float, confidence_level: float, power: float, n_max: int
) -> int:
    """Smallest n such that every n' in [n, n_max] has `wilson_pass_probability(n', expected, floor) >= power`.

    Monotonicity choice: the pass probability is a sawtooth in n (the smallest clearing count
    k_min steps by one while n grows continuously), so the first n reaching `power` can be
    followed by sizes that fall below it again. The requirement is therefore the start of the
    last unbroken run of sizes meeting `power` that ends at n_max: any sample size from it up
    to n_max meets the power, so collecting more items than required never loses power within
    the scanned range. The value depends on n_max only when the sawtooth dips below `power`
    above the answer, which a larger n_max would expose; callers pick n_max well above it.
    Raises ValueError when n_max itself falls short.

    Reference: Wilson (1927), the score interval `wilson_score_interval` implements; the
    pass probability is the exact binomial tail (`wilson_pass_probability`).
    """
    _validate_sample_size_inputs(expected, floor, confidence_level, n_max)
    _validate_unit_interval("power", power)

    def meets(m: int) -> bool:
        return wilson_pass_probability(m, expected, floor, confidence_level) >= power

    if not meets(n_max):
        raise ValueError(f"n_max ({n_max}) has pass probability below {power} at expected {expected}, floor {floor}")
    n = n_max
    while n > 1 and meets(n - 1):
        n -= 1
    return n


def trajectories_required(n_pos: int, pass_rate: float, in_scope_share: float, valid_share: float) -> int:
    """Agent runs needed to yield `n_pos` in-scope positives: ceil(n_pos / (pass_rate * in_scope_share * valid_share)).

    `pass_rate` is the share of runs that pass the impossible tests, `in_scope_share` the
    share of those passes in the in-scope stratum, `valid_share` the share of runs left
    after exclusions. Each rate is in (0, 1]. A quotient within 1e-9 (relative) of an
    integer is taken as that integer, so floating-point error in the product cannot add a run.
    """
    if not isinstance(n_pos, int) or isinstance(n_pos, bool) or n_pos <= 0:
        raise ValueError(f"n_pos must be a positive integer, got {n_pos}")
    for name, rate in (("pass rate", pass_rate), ("in-scope share", in_scope_share), ("valid share", valid_share)):
        if not isinstance(rate, (int, float)) or math.isnan(rate) or rate <= 0.0 or rate > 1.0:
            raise ValueError(f"{name} must be in (0, 1], got {rate}")
    x = n_pos / (pass_rate * in_scope_share * valid_share)
    nearest = round(x)
    if abs(x - nearest) <= 1e-9 * max(1.0, x):
        return int(nearest)
    return math.ceil(x)


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

    def test_wilson_pass_probability_pinned_reference_values(self):
        # Exact binomial tails, cross-checked with `fractions.Fraction` and `math.comb`
        # (the tail from k_min = 98 at n = 112, 88 at n = 100).
        self.assertAlmostEqual(wilson_pass_probability(112, 0.90, 0.80, 0.95), 0.8506025425939, places=10)
        self.assertAlmostEqual(wilson_pass_probability(111, 0.90, 0.80, 0.95), 0.7816649977621, places=10)
        self.assertAlmostEqual(wilson_pass_probability(100, 0.90, 0.80, 0.95), 0.8018211126404, places=10)
        self.assertAlmostEqual(wilson_pass_probability(58, 0.90, 0.75, 0.95), 0.8785286518677, places=10)
        self.assertAlmostEqual(wilson_pass_probability(69, 0.85, 0.70, 0.95), 0.8554121990756, places=10)
        # No count can clear a floor of 0.99 at n = 5.
        self.assertEqual(wilson_pass_probability(5, 0.90, 0.99, 0.95), 0.0)

    def test_sample_size_for_wilson_lower_bound_pinned_reference_values(self):
        self.assertEqual(sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 2000), 68)
        self.assertEqual(sample_size_for_wilson_lower_bound(0.90, 0.75, 0.95, 2000), 37)
        self.assertEqual(sample_size_for_wilson_lower_bound(0.85, 0.70, 0.95, 2000), 39)
        # Non-monotone: n = 54 is the first clearing size, but 56-60 and 66-67 do not clear.
        self.assertGreaterEqual(wilson_score_interval(49, 54, 0.95)[0], 0.80)
        self.assertLess(wilson_score_interval(60, 67, 0.95)[0], 0.80)
        self.assertGreaterEqual(wilson_score_interval(61, 68, 0.95)[0], 0.80)
        # Independent of n_max once n_max is past the last gap.
        self.assertEqual(sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 500), 68)

    def test_sample_size_for_wilson_lower_bound_power_pinned_reference_values(self):
        self.assertEqual(sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 2000), 112)
        self.assertEqual(sample_size_for_wilson_lower_bound_power(0.90, 0.75, 0.95, 0.80, 2000), 58)
        self.assertEqual(sample_size_for_wilson_lower_bound_power(0.85, 0.70, 0.95, 0.80, 2000), 69)
        # The sawtooth: n = 100 reaches 0.80, n = 101-105 and 110-111 fall below it again.
        self.assertGreaterEqual(wilson_pass_probability(100, 0.90, 0.80, 0.95), 0.80)
        self.assertLess(wilson_pass_probability(111, 0.90, 0.80, 0.95), 0.80)
        self.assertEqual(sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 500), 112)

    def test_trajectories_required_pinned_reference_values(self):
        self.assertEqual(trajectories_required(112, 0.30, 0.50, 0.90), 830)
        self.assertEqual(trajectories_required(100, 0.50, 0.50, 1.0), 400)
        self.assertEqual(trajectories_required(10, 0.10, 1.0, 1.0), 100)
        self.assertEqual(trajectories_required(1, 1.0, 1.0, 1.0), 1)

    def test_recall_bounds_invalid_inputs_fail_closed(self):
        with self.assertRaises(ValueError):
            wilson_pass_probability(0, 0.9, 0.8)
        with self.assertRaises(ValueError):
            wilson_pass_probability(10, 1.0, 0.8)
        with self.assertRaises(ValueError):
            sample_size_for_wilson_lower_bound(0.80, 0.80, 0.95, 2000)
        with self.assertRaises(ValueError):
            sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 10)
        with self.assertRaises(ValueError):
            sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 1.0, 2000)
        with self.assertRaises(ValueError):
            sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 50)
        with self.assertRaises(ValueError):
            trajectories_required(0, 0.5, 0.5, 0.5)
        with self.assertRaises(ValueError):
            trajectories_required(10, 0.0, 0.5, 0.5)
        with self.assertRaises(ValueError):
            trajectories_required(10, 0.5, 1.5, 0.5)


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
    lines.append("")
    lines.extend(generate_recall_audit_lines())
    return "\n".join(lines)


# Recall study (`docs/benchmarks/recall-preregistration.toml`, #838): (assumed recall, floor)
# pairs, at confidence 0.95 and power 0.80. The first row is the pre-registered one; the
# others show how the requirement moves with the floor and the assumption.
RECALL_GRID: list[tuple[float, float]] = [(0.90, 0.80), (0.90, 0.75), (0.85, 0.70)]
RECALL_CONFIDENCE = 0.95
RECALL_POWER = 0.80
RECALL_N_MAX = 2000


def recall_bound_values() -> list[dict[str, float | int]]:
    """The recall study's Step-0 values for `RECALL_GRID`, as the pre-registration records them."""
    return [
        {
            "assumed_recall": expected,
            "recall_floor": floor,
            "confidence_level": RECALL_CONFIDENCE,
            "statistical_power": RECALL_POWER,
            "n_max": RECALL_N_MAX,
            "required_n_point": sample_size_for_wilson_lower_bound(expected, floor, RECALL_CONFIDENCE, RECALL_N_MAX),
            "required_n_power": sample_size_for_wilson_lower_bound_power(
                expected, floor, RECALL_CONFIDENCE, RECALL_POWER, RECALL_N_MAX
            ),
        }
        for expected, floor in RECALL_GRID
    ]


def generate_recall_audit_lines() -> list[str]:
    lines = [
        "## Recall study (`impossible-recall-v1`, #838)",
        "",
        "| Assumed recall | Floor | Required n (point estimate clears) | Required n (pass probability >= 0.80) |",
        "|---|---|---|---|",
    ]
    for row in recall_bound_values():
        lines.append(
            f"| {row['assumed_recall']:.2f} | {row['recall_floor']:.2f} | {row['required_n_point']} | {row['required_n_power']} |"
        )
    lines.append("")
    lines.append("Audit rules (#838):")
    lines.append(
        f"- Both columns are the start of the last unbroken run of qualifying sizes up to n_max = {RECALL_N_MAX}"
        " (Wilson clearance and the exact pass probability are both non-monotone in n)."
    )
    lines.append(
        "- The pre-registered `required_n` is the power column times the pilot's design effect; the agent runs"
        " needed are `trajectories_required(required_n, pass_rate, in_scope_share, valid_share)` with each rate from the pilot."
    )
    return lines


if __name__ == "__main__":
    if "--test" in sys.argv or "--self-check" in sys.argv:
        sys.argv = [a for a in sys.argv if a not in ("--test", "--self-check")]
        unittest.main()
    elif "--recall-bounds-json" in sys.argv:
        print(json.dumps(recall_bound_values(), indent=2))
    else:
        print(generate_power_audit())
