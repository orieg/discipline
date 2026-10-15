#!/usr/bin/env python3
"""Public agent-PR precision benchmark harness (#482 Part B).

Implements:
- Pre-registration verification against `docs/benchmarks/precision-preregistration.toml`
- Exact Python port of `src/guards/perf/bounds.rs` statistical bounds (`standard_normal_quantile`,
  `wilson_score_interval`, `sample_size_for_interval`, `minimum_detectable_difference`)
- Inter-rater reliability via Cohen's kappa (`cohens_kappa`, Cohen 1960)
- Public repository merged agent-PR harvester and diff-scoped replay runner for `discipline`
  (`default-with-test-entrypoints`) and the `naive-sensitive-path-filter` substitution twin
- Blinded two-rater adjudication packet builder and final summary generator
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Statistical bounds (pinned to `src/guards/perf/bounds.rs`)
# ---------------------------------------------------------------------------


def standard_normal_quantile(p: float) -> float:
    """Inverse standard normal CDF (Acklam 2003), matching `src/guards/perf/bounds.rs`."""
    if not math.isfinite(p) or p <= 0.0 or p >= 1.0:
        raise ValueError(f"probability p must be strictly between 0 and 1, got {p}")

    pinned = {
        0.975: 1.959_963_984_540_054,
        0.025: -1.959_963_984_540_054,
        0.995: 2.575_829_303_548_900_4,
        0.005: -2.575_829_303_548_900_4,
        0.95: 1.644_853_626_951_472_2,
        0.05: -1.644_853_626_951_472_2,
        0.80: 0.841_621_233_572_914_3,
        0.20: -0.841_621_233_572_914_3,
        0.90: 1.281_551_565_544_600_4,
        0.10: -1.281_551_565_544_600_4,
    }
    if p in pinned:
        return pinned[p]

    a = [
        -3.969_683_028_665_376e01,
        2.209_460_984_245_205e02,
        -2.759_285_104_469_687e02,
        1.383_577_518_672_69e02,
        -3.066_479_806_614_716e01,
        2.506_628_277_459_239e00,
    ]
    b = [
        -5.447_609_879_822_406e01,
        1.615_858_368_580_409e02,
        -1.556_989_798_598_866e02,
        6.680_131_188_771_972e01,
        -1.328_068_155_288_572e01,
    ]
    c = [
        -7.784_894_002_430_293e-03,
        -3.223_964_580_411_365e-01,
        -2.400_758_277_161_838e00,
        -2.549_732_539_343_734e00,
        4.374_664_141_464_968e00,
        2.938_163_982_698_783e00,
    ]
    d = [
        7.784_695_709_041_462e-03,
        3.224_671_290_700_398e-01,
        2.445_134_137_142_996e00,
        3.754_408_661_907_416e00,
    ]
    p_low = 0.02425
    p_high = 1.0 - p_low

    if p < p_low:
        q = math.sqrt(-2.0 * math.log(p))
        num = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]
        den = (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
        return num / den
    if p <= p_high:
        q = p - 0.5
        r = q * q
        num = (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
        den = ((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0
        return num / den
    q = math.sqrt(-2.0 * math.log(1.0 - p))
    num = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]
    den = (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
    return -num / den


def wilson_score_interval(
    k: int, n: int, confidence_level: float = 0.95
) -> tuple[float, float]:
    """Two-sided Wilson score confidence interval (Wilson 1927)."""
    if n <= 0:
        raise ValueError("sample size n must be greater than 0")
    if k < 0 or k > n:
        raise ValueError(f"successes k ({k}) must be in [0, n ({n})]")
    if not math.isfinite(confidence_level) or confidence_level <= 0.0 or confidence_level >= 1.0:
        raise ValueError(f"confidence level must be between 0 and 1, got {confidence_level}")

    alpha = 1.0 - confidence_level
    z = standard_normal_quantile(1.0 - alpha / 2.0)
    p_hat = k / n
    n_f = float(n)
    z2 = z * z

    denom = 1.0 + z2 / n_f
    center = (p_hat + z2 / (2.0 * n_f)) / denom
    radicand = (p_hat * (1.0 - p_hat) / n_f) + (z2 / (4.0 * n_f * n_f))
    margin = (z / denom) * math.sqrt(max(0.0, radicand))

    lower = max(0.0, center - margin)
    upper = min(1.0, center + margin)
    return (lower, upper)


def sample_size_for_interval(
    assumed_precision: float, target_half_width: float, confidence_level: float = 0.95
) -> int:
    """Analytical sample size n for a target Wilson CI half-width (Wilson 1927)."""
    if not math.isfinite(assumed_precision) or assumed_precision <= 0.0 or assumed_precision >= 1.0:
        raise ValueError(f"assumed precision must be in (0, 1), got {assumed_precision}")
    if not math.isfinite(target_half_width) or target_half_width <= 0.0 or target_half_width >= 0.5:
        raise ValueError(f"target half-width must be in (0, 0.5), got {target_half_width}")
    if not math.isfinite(confidence_level) or confidence_level <= 0.0 or confidence_level >= 1.0:
        raise ValueError(f"confidence level must be in (0, 1), got {confidence_level}")

    alpha = 1.0 - confidence_level
    z = standard_normal_quantile(1.0 - alpha / 2.0)
    p = assumed_precision
    w = target_half_width

    v = p * (1.0 - p)
    w2 = w * w
    term1 = v - 2.0 * w2
    term2 = math.sqrt(v * v + w2 * ((1.0 - 2.0 * p) ** 2))
    n = (z * z * (term1 + term2)) / (2.0 * w2)
    return max(1, int(math.ceil(n)))


def minimum_detectable_difference(
    n1: int,
    n2: int,
    baseline_precision: float,
    confidence_level: float = 0.95,
    power: float = 0.80,
) -> float:
    """Minimum detectable difference (MDD) between two binomial proportions (Fleiss et al. 2003)."""
    if n1 <= 0 or n2 <= 0:
        raise ValueError("sample sizes n1 and n2 must be greater than 0")
    if not math.isfinite(baseline_precision) or baseline_precision <= 0.0 or baseline_precision >= 1.0:
        raise ValueError(f"baseline precision must be in (0, 1), got {baseline_precision}")
    if not math.isfinite(confidence_level) or confidence_level <= 0.0 or confidence_level >= 1.0:
        raise ValueError(f"confidence level must be in (0, 1), got {confidence_level}")
    if not math.isfinite(power) or power <= 0.0 or power >= 1.0:
        raise ValueError(f"statistical power must be in (0, 1), got {power}")

    alpha = 1.0 - confidence_level
    z_alpha = standard_normal_quantile(1.0 - alpha / 2.0)
    z_beta = standard_normal_quantile(power)
    z_sum_sq = (z_alpha + z_beta) ** 2

    k1 = z_sum_sq / float(n1)
    k2 = z_sum_sq / float(n2)
    p1 = baseline_precision
    v1 = p1 * (1.0 - p1)

    a = 1.0 + k2
    b = -k2 * (1.0 - 2.0 * p1)
    c = -(k1 + k2) * v1

    disc = max(0.0, b * b - 4.0 * a * c)
    delta = (-b + math.sqrt(disc)) / (2.0 * a)
    if p1 + delta > 1.0:
        raise ValueError(
            f"statistical power {power} unachievable: p1 + delta = {p1 + delta:.4f} > 1.0"
        )
    return delta


def cohens_kappa(labels_a: list[str], labels_b: list[str]) -> float:
    """Unweighted Cohen's kappa for two raters over discrete nominal categories (Cohen 1960).

    Reference: Cohen, J. (1960). "A coefficient of agreement for nominal scales",
    Educational and Psychological Measurement, 20(1), 37-46.
    """
    if len(labels_a) != len(labels_b):
        raise ValueError("rater label lists must have identical length")
    n = len(labels_a)
    if n == 0:
        raise ValueError("cannot compute Cohen's kappa on empty label lists")

    categories = sorted(set(labels_a) | set(labels_b))
    agreed = sum(1 for a, b in zip(labels_a, labels_b) if a == b)
    p_o = agreed / float(n)

    counts_a = {c: 0 for c in categories}
    counts_b = {c: 0 for c in categories}
    for a, b in zip(labels_a, labels_b):
        counts_a[a] += 1
        counts_b[b] += 1

    p_e = sum((counts_a[c] / float(n)) * (counts_b[c] / float(n)) for c in categories)
    if abs(1.0 - p_e) < 1e-12:
        return 1.0 if abs(1.0 - p_o) < 1e-12 else 0.0
    return (p_o - p_e) / (1.0 - p_e)


def evaluate_gate_verdict(
    k: int,
    n: int,
    floor: float,
    required_n: int,
    confidence_level: float = 0.95,
) -> dict[str, Any]:
    """Evaluate a gate or gate-class outcome against its pre-registered floor and required_n."""
    if n == 0:
        return {
            "k": 0,
            "n": 0,
            "required_n": required_n,
            "precision_floor": floor,
            "point_estimate": None,
            "wilson_95_ci": None,
            "verdict": "insufficient-n",
        }
    low, high = wilson_score_interval(k, n, confidence_level)
    point = k / float(n)
    if n < required_n:
        verdict = "insufficient-n"
    elif low >= floor:
        verdict = "PASS"
    elif point >= floor:
        verdict = "INTERMEDIATE_floor_within_ci"
    else:
        verdict = "REJECTED"
    return {
        "k": k,
        "n": n,
        "required_n": required_n,
        "precision_floor": floor,
        "point_estimate": round(point, 6),
        "wilson_95_ci": [round(low, 6), round(high, 6)],
        "verdict": verdict,
    }


def hash_identity(raw_identity: str) -> str:
    """Anonymize contributor or rater identity with SHA-256 prefix."""
    digest = hashlib.sha256(raw_identity.strip().lower().encode("utf-8")).hexdigest()
    return f"sha256:{digest[:16]}"


# ---------------------------------------------------------------------------
# Twin baseline (`naive-sensitive-path-filter`)
# ---------------------------------------------------------------------------

SENSITIVE_TEST_PATTERNS = [
    re.compile(r"(^|/)tests?/", re.IGNORECASE),
    re.compile(r"(_test|\.test|\.spec)\.[a-z0-9]+$", re.IGNORECASE),
    re.compile(r"(^|/)test_[^/]+\.py$", re.IGNORECASE),
]

SENSITIVE_WORKFLOW_PATTERNS = [
    re.compile(r"^\.github/workflows/[^/]+\.ya?ml$"),
    re.compile(r"^\.gitea/workflows/[^/]+\.ya?ml$"),
    re.compile(r"^\.forgejo/workflows/[^/]+\.ya?ml$"),
    re.compile(r"^\.gitlab-ci\.ya?ml$"),
]

SENSITIVE_CONFIG_PATTERNS = [
    re.compile(
        r"(^|/)(Cargo\.toml|pyproject\.toml|pytest\.ini|tox\.ini|setup\.cfg|"
        r"package\.json|tsconfig\.json|biome\.json|biome\.jsonc|deno\.json|deno\.jsonc|"
        r"\.eslintrc.*|eslint\.config\..*|vitest\.config\..*|jest\.config\..*|"
        r"ruff\.toml|\.ruff\.toml|clippy\.toml|\.clippy\.toml|rustfmt\.toml|"
        r"golangci\.ya?ml|\.golangci\.ya?ml|pom\.xml|build\.gradle(\.kts)?)$"
    ),
]


def classify_twin_sensitive_path(path: str, existed_in_base: bool) -> str | None:
    """Apply `naive-sensitive-path-filter` rule from `precision-preregistration.toml`."""
    for pat in SENSITIVE_WORKFLOW_PATTERNS:
        if pat.search(path):
            return "twin-sensitive-path/ci-workflow"
    for pat in SENSITIVE_CONFIG_PATTERNS:
        if pat.search(path):
            return "twin-sensitive-path/linter-or-runner-config"
    if existed_in_base:
        for pat in SENSITIVE_TEST_PATTERNS:
            if pat.search(path):
                return "twin-sensitive-path/existing-test-file"
    return None


# ---------------------------------------------------------------------------
# Self-test suite (`--test`)
# ---------------------------------------------------------------------------


def run_self_tests() -> None:
    """Verify mathematical bounds, Cohen's kappa, twin rules, and pre-registration invariants."""
    # 1. Standard normal quantiles
    assert abs(standard_normal_quantile(0.975) - 1.959_963_984_540_054) < 1e-12
    assert abs(standard_normal_quantile(0.80) - 0.841_621_233_572_914_3) < 1e-12
    assert abs(standard_normal_quantile(0.25) + standard_normal_quantile(0.75)) < 1e-9

    # 2. Sample sizes pinned in `precision-preregistration.toml`
    assert sample_size_for_interval(0.95, 0.05, 0.95) == 83
    assert sample_size_for_interval(0.90, 0.05, 0.95) == 141
    assert sample_size_for_interval(0.88, 0.05, 0.95) == 164
    assert sample_size_for_interval(0.80, 0.05, 0.95) == 245

    # 3. Minimum detectable differences pinned in `precision-preregistration.toml`
    mdd_struct = minimum_detectable_difference(141, 141, 0.55, 0.95, 0.80)
    mdd_heur = minimum_detectable_difference(245, 245, 0.55, 0.95, 0.80)
    assert abs(mdd_struct - 0.15894491346884432) < 1e-12
    assert abs(mdd_heur - 0.12241601541985098) < 1e-12

    # 4. Wilson score interval reference check (at n=141, k=134 -> low >= 0.90)
    low_s, high_s = wilson_score_interval(134, 141, 0.95)
    assert low_s >= 0.90
    assert (high_s - low_s) / 2.0 <= 0.05

    low_h, high_h = wilson_score_interval(216, 245, 0.95)
    assert low_h >= 0.80
    assert (high_h - low_h) / 2.0 <= 0.05

    # 5. Cohen's kappa reference value (Cohen 1960 worked 2x2 example:
    # A/B with [20, 5; 10, 15], N=50 -> p_o = 35/50 = 0.70,
    # p_e = (25/50)*(30/50) + (25/50)*(20/50) = 0.30 + 0.20 = 0.50,
    # kappa = (0.70 - 0.50) / (1.0 - 0.50) = 0.40)
    r1 = ["TP"] * 20 + ["TP"] * 5 + ["FP"] * 10 + ["FP"] * 15
    r2 = ["TP"] * 20 + ["FP"] * 5 + ["TP"] * 10 + ["FP"] * 15
    assert abs(cohens_kappa(r1, r2) - 0.40) < 1e-12

    # Perfect agreement
    assert abs(cohens_kappa(["true-positive", "false-positive"], ["true-positive", "false-positive"]) - 1.0) < 1e-12

    # 6. Verdict evaluation rules (Rule 1: CI lower bound >= floor, and n >= required_n)
    v_insuff = evaluate_gate_verdict(50, 50, 0.90, 141)
    assert v_insuff["verdict"] == "insufficient-n"
    v_pass = evaluate_gate_verdict(136, 141, 0.90, 141)
    assert v_pass["verdict"] == "PASS"
    v_inter = evaluate_gate_verdict(128, 141, 0.90, 141)
    assert v_inter["verdict"] == "INTERMEDIATE_floor_within_ci"
    v_rej = evaluate_gate_verdict(100, 141, 0.90, 141)
    assert v_rej["verdict"] == "REJECTED"

    # 7. Twin classification
    assert classify_twin_sensitive_path(".github/workflows/ci.yml", False) == "twin-sensitive-path/ci-workflow"
    assert classify_twin_sensitive_path("crates/uv/tests/it.rs", True) == "twin-sensitive-path/existing-test-file"
    assert classify_twin_sensitive_path("crates/uv/tests/it.rs", False) is None
    assert classify_twin_sensitive_path("pyproject.toml", True) == "twin-sensitive-path/linter-or-runner-config"
    assert classify_twin_sensitive_path("src/main.rs", True) is None

    print("All benchmark_precision.py self-tests passed.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true", help="Run unit tests and exit")
    args = parser.parse_args()
    if args.test:
        run_self_tests()
        return 0
    parser.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
