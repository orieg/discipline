#!/usr/bin/env python3
"""Statistics and twin rule for the public agent-PR precision benchmark (#482 Part B).

What this file contains:
- A Python port of the `src/guards/perf/bounds.rs` bounds (`standard_normal_quantile`,
  `wilson_score_interval`, `sample_size_for_interval`, `minimum_detectable_difference`) with
  self-tests pinning the values recorded in `docs/benchmarks/precision-preregistration.toml`.
- Cohen's kappa (`cohens_kappa`, Cohen 1960) and the verdict rule (`evaluate_gate_verdict`).
- The `naive-sensitive-path-filter` twin path classifier (`classify_twin_sensitive_path`).
- Post-hoc audit statistics, not pre-registered (`--post-hoc-audit`): the Fleiss pooled-null
  detectable difference, the design's pass probability and minimum n, a Newcombe interval for
  the class-versus-twin difference, and PR-clustered intervals for the structural class. They
  read the committed `docs/benchmarks/results/*.json` and write `post-hoc-audit.json`.

What this file does NOT contain: the pull-request harvester, the diff-scoped replay runner,
the twin flag sampling, the shuffled rater packet builder, or the summary generator that
produced `docs/benchmarks/results/{manifest,findings,adjudication,summary}.json`. That code is
not committed anywhere in this repository, so those files cannot be regenerated from it; the
committed tests only check them for internal consistency and against the bounds.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import random
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
    # Rust divides by w2 = 0.0 to +inf; Python raises, so map that case to inf as well.
    n = (z * z * (term1 + term2)) / (2.0 * w2) if w2 > 0.0 else math.inf
    if not math.isfinite(n) or n > MAX_SAMPLE_SIZE:
        raise ValueError(
            f"required sample size for half-width {target_half_width} at precision "
            f"{assumed_precision} exceeds {MAX_SAMPLE_SIZE} (computed {n})"
        )
    return max(1, int(math.ceil(n)))


# Mirrors `MAX_SAMPLE_SIZE` in `src/guards/perf/bounds.rs`.
MAX_SAMPLE_SIZE = 1_000_000_000


def minimum_detectable_difference(
    n1: int,
    n2: int,
    baseline_precision: float,
    confidence_level: float = 0.95,
    power: float = 0.80,
) -> float:
    """Minimum detectable difference between two binomial proportions, unpooled (Wald-type) form.

    Mirrors `src/guards/perf/bounds.rs`: the same unpooled variance under the null and the
    alternative (Chow, Shao & Wang 2008). This is not the Fleiss et al. (2003) pooled-null
    formula; see `fleiss_pooled_mdd`, which gives a slightly larger difference.
    """
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


def fleiss_pooled_mdd(
    n1: int,
    n2: int,
    baseline_precision: float,
    confidence_level: float = 0.95,
    power: float = 0.80,
) -> float:
    """Minimum detectable difference under the Fleiss pooled-null power equation.

    Solves, by bisection on `delta` in `(0, 1 - p1)`:
        delta = z_{a/2} sqrt(pbar (1 - pbar) (1/n1 + 1/n2)) + z_b sqrt(p1 q1 / n1 + p2 q2 / n2)
    with `p2 = p1 + delta` and `pbar = (n1 p1 + n2 p2) / (n1 + n2)`.

    Reference: Fleiss, J. L., Levin, B., & Paik, M. C. (2003). "Statistical Methods for Rates
    and Proportions", 3rd ed., Wiley, chapter 4 (pooled variance under the null hypothesis).
    Post-hoc audit only: the locked pre-registration pins `minimum_detectable_difference`.
    """
    # Validates every argument the same way as the unpooled form.
    minimum_detectable_difference(n1, n2, baseline_precision, confidence_level, power)
    z_alpha = standard_normal_quantile(1.0 - (1.0 - confidence_level) / 2.0)
    z_beta = standard_normal_quantile(power)
    p1 = baseline_precision

    def excess(delta: float) -> float:
        p2 = p1 + delta
        pbar = (n1 * p1 + n2 * p2) / float(n1 + n2)
        null_sd = math.sqrt(pbar * (1.0 - pbar) * (1.0 / n1 + 1.0 / n2))
        alt_sd = math.sqrt(p1 * (1.0 - p1) / n1 + p2 * (1.0 - p2) / n2)
        return delta - z_alpha * null_sd - z_beta * alt_sd

    lo, hi = 0.0, 1.0 - p1
    if excess(hi) < 0.0:
        raise ValueError(f"statistical power {power} unachievable at n1={n1}, n2={n2}, p1={p1}")
    for _ in range(200):
        mid = (lo + hi) / 2.0
        if excess(mid) < 0.0:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2.0


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
# Post-hoc audit statistics (NOT pre-registered)
# ---------------------------------------------------------------------------

RESULTS_DIR = Path(__file__).resolve().parent.parent / "docs" / "benchmarks" / "results"
POST_HOC_BOOTSTRAP_SEED = 20261015
POST_HOC_BOOTSTRAP_RESAMPLES = 20000


def _binomial_pmf(k: int, n: int, p: float) -> float:
    log_pmf = (
        math.lgamma(n + 1)
        - math.lgamma(k + 1)
        - math.lgamma(n - k + 1)
        + k * math.log(p)
        + (n - k) * math.log1p(-p)
    )
    return math.exp(log_pmf)


def wilson_pass_probability(
    n: int, true_precision: float, floor: float, confidence_level: float = 0.95
) -> float:
    """P(Wilson lower bound >= floor) when the k successes are Binomial(n, true_precision).

    The probability that the pre-registered decision rule returns PASS at a fixed n, i.e.
    the design's power against the floor at that true precision (exact binomial sum).
    """
    if n <= 0:
        raise ValueError("sample size n must be greater than 0")
    if not 0.0 < true_precision < 1.0:
        raise ValueError(f"true precision must be in (0, 1), got {true_precision}")
    return sum(
        _binomial_pmf(k, n, true_precision)
        for k in range(n + 1)
        if wilson_score_interval(k, n, confidence_level)[0] >= floor
    )


def wilson_floor_clearing_sizes(
    assumed_precision: float, floor: float, n_max: int, confidence_level: float = 0.95
) -> list[int]:
    """Every n in [1, n_max] at which k = round_half_up(assumed_precision * n) has a Wilson
    lower bound >= floor. The set is not monotone in n because k is rounded."""
    out = []
    for n in range(1, n_max + 1):
        k = int(math.floor(assumed_precision * n + 0.5))
        if wilson_score_interval(k, n, confidence_level)[0] >= floor:
            out.append(n)
    return out


def newcombe_difference_ci(
    k1: int, n1: int, k2: int, n2: int, confidence_level: float = 0.95
) -> tuple[float, float]:
    """Newcombe hybrid-score interval for p1 - p2 from two INDEPENDENT samples.

    Reference: Newcombe, R. G. (1998). "Interval estimation for the difference between
    independent proportions: comparison of eleven methods". Statistics in Medicine 17(8),
    873-890, method 10.
    """
    p1, p2 = k1 / float(n1), k2 / float(n2)
    l1, u1 = wilson_score_interval(k1, n1, confidence_level)
    l2, u2 = wilson_score_interval(k2, n2, confidence_level)
    d = p1 - p2
    return (
        d - math.sqrt((p1 - l1) ** 2 + (u2 - p2) ** 2),
        d + math.sqrt((u1 - p1) ** 2 + (p2 - l2) ** 2),
    )


def cluster_robust_ratio(
    clusters: list[tuple[int, int]], confidence_level: float = 0.95
) -> dict[str, Any]:
    """Cluster-robust (linearised) interval for a pooled proportion sum(k) / sum(n).

    Each cluster is (k_i, n_i): one pull request's true positives and findings. The variance
    of the ratio estimator is m / (m - 1) * sum((k_i - p n_i)^2) / N^2 (Cochran 1977,
    "Sampling Techniques", 3rd ed., chapter 6, ratio estimator variance, with the m / (m - 1) small-sample factor).
    The design effect is that variance over the binomial p (1 - p) / N; the effective n is
    N over the design effect.
    """
    m = len(clusters)
    if m < 2:
        raise ValueError("at least two clusters are required")
    k_total = sum(k for k, _ in clusters)
    n_total = sum(n for _, n in clusters)
    p = k_total / float(n_total)
    var = m / (m - 1.0) * sum((k - p * n) ** 2 for k, n in clusters) / float(n_total) ** 2
    z = standard_normal_quantile(1.0 - (1.0 - confidence_level) / 2.0)
    se = math.sqrt(var)
    design_effect = var / (p * (1.0 - p) / n_total)
    return {
        "clusters": m,
        "k": k_total,
        "n": n_total,
        "point_estimate": p,
        "standard_error": se,
        "ci": (p - z * se, p + z * se),
        "design_effect": design_effect,
        "effective_n": n_total / design_effect,
    }


def _quantile_type7(sorted_xs: list[float], q: float) -> float:
    """Linear-interpolation sample quantile (Hyndman & Fan 1996 type 7, numpy's default)."""
    h = (len(sorted_xs) - 1) * q
    lo = int(math.floor(h))
    hi = min(lo + 1, len(sorted_xs) - 1)
    return sorted_xs[lo] + (h - lo) * (sorted_xs[hi] - sorted_xs[lo])


def pr_cluster_bootstrap_ci(
    clusters: list[tuple[int, int]],
    resamples: int = POST_HOC_BOOTSTRAP_RESAMPLES,
    seed: int = POST_HOC_BOOTSTRAP_SEED,
    confidence_level: float = 0.95,
) -> tuple[float, float]:
    """Percentile bootstrap for sum(k) / sum(n), resampling whole clusters (pull requests).

    Reference: Davison, A. C. & Hinkley, D. V. (1997). "Bootstrap Methods and their
    Application", section 3.8 (resampling the first stage of a clustered sample).
    Deterministic for a given seed (Python's `random.Random`).
    """
    rng = random.Random(seed)
    m = len(clusters)
    estimates = []
    for _ in range(resamples):
        k_sum = n_sum = 0
        for _ in range(m):
            k, n = clusters[rng.randrange(m)]
            k_sum += k
            n_sum += n
        estimates.append(k_sum / float(n_sum))
    estimates.sort()
    alpha = 1.0 - confidence_level
    return (_quantile_type7(estimates, alpha / 2.0), _quantile_type7(estimates, 1.0 - alpha / 2.0))


def load_labelled_items(results_dir: Path = RESULTS_DIR) -> list[dict[str, Any]]:
    """Join findings.json to adjudication.json's final labels and summary.json's gate classes."""
    findings = json.loads((results_dir / "findings.json").read_text())["items"]
    adjudication = json.loads((results_dir / "adjudication.json").read_text())["adjudications"]
    summary = json.loads((results_dir / "summary.json").read_text())
    labels = {a["item_id"]: a for a in adjudication}
    gate_class = {g["gate"]: g["class"] for g in summary["per_gate_results"]}
    items = []
    for item in findings:
        adj = labels[item["item_id"]]
        items.append(
            dict(
                item,
                final_label=adj["final_label"],
                rater_1_label=adj["rater_1_label"],
                rater_2_label=adj["rater_2_label"],
                gate_class=gate_class.get(item["gate"], "twin"),
            )
        )
    return items


def pull_request_clusters(items: list[dict[str, Any]], gate_class: str) -> list[tuple[int, int]]:
    """(true positives, findings) per pull request (repo, head_sha) for one gate class."""
    per_pr: dict[tuple[str, str], list[int]] = {}
    for item in items:
        if item["source_tool"] != "discipline" or item["gate_class"] != gate_class:
            continue
        cell = per_pr.setdefault((item["repo"], item["head_sha"]), [0, 0])
        cell[0] += item["final_label"] == "true-positive"
        cell[1] += 1
    return [(k, n) for k, n in per_pr.values()]


def post_hoc_audit(results_dir: Path = RESULTS_DIR) -> dict[str, Any]:
    """Every number in the ROADMAP #482 Part B post-hoc audit, recomputed from the JSON."""
    items = load_labelled_items(results_dir)
    structural = "structural-ast-and-deterministic"
    clusters = pull_request_clusters(items, structural)
    largest = max(clusters, key=lambda c: c[1])
    robust = cluster_robust_ratio(clusters)
    boot = pr_cluster_bootstrap_ci(clusters)

    s_items = [i for i in items if i["source_tool"] == "discipline" and i["gate_class"] == structural]
    s_k = sum(i["final_label"] == "true-positive" for i in s_items)
    twin = [i for i in items if i["source_tool"] != "discipline"]
    twin_k = sum(i["final_label"] == "true-positive" for i in twin)
    discipline_tp_files = {
        (i["repo"], i["head_sha"], i["file"])
        for i in items
        if i["source_tool"] == "discipline" and i["final_label"] == "true-positive"
    }
    twin_tp = [i for i in twin if i["final_label"] == "true-positive"]
    twin_tp_on_discipline_tp_file = sum(
        (i["repo"], i["head_sha"], i["file"]) in discipline_tp_files for i in twin_tp
    )
    twin_config_fp = [
        i
        for i in twin
        if i["code"] == "twin-sensitive-path/linter-or-runner-config"
        and i["final_label"] == "false-positive"
    ]
    config_fp_files: dict[str, int] = {}
    for i in twin_config_fp:
        name = i["file"].rsplit("/", 1)[-1]
        config_fp_files[name] = config_fp_files.get(name, 0) + 1

    fp_by_gate_repo: dict[str, int] = {}
    for i in s_items:
        if i["final_label"] == "false-positive":
            key = f"{i['gate']} {i['repo']}"
            fp_by_gate_repo[key] = fp_by_gate_repo.get(key, 0) + 1

    clearing = wilson_floor_clearing_sizes(0.95, 0.90, 141)
    def r6(x: float) -> float:
        return round(x, 6)

    lo_d, hi_d = newcombe_difference_ci(s_k, len(s_items), twin_k, len(twin))
    return {
        "study_id": "agent-pr-precision-v1",
        "pre_registered": False,
        "generated_by": "python3 scripts/benchmark_precision.py --post-hoc-audit --write",
        "structural_clustering": {
            "pull_requests_with_findings": robust["clusters"],
            "largest_pull_request_findings": largest[1],
            "largest_pull_request_true_positives": largest[0],
            "k": robust["k"],
            "n": robust["n"],
            "design_effect": r6(robust["design_effect"]),
            "effective_n": r6(robust["effective_n"]),
            "cluster_robust_95_ci": [r6(robust["ci"][0]), r6(robust["ci"][1])],
            "pr_cluster_bootstrap_95_ci": [r6(boot[0]), r6(boot[1])],
            "bootstrap_seed": POST_HOC_BOOTSTRAP_SEED,
            "bootstrap_resamples": POST_HOC_BOOTSTRAP_RESAMPLES,
        },
        "structural_false_positives_by_gate_and_repo": dict(sorted(fp_by_gate_repo.items())),
        "twin_comparison": {
            "difference": r6(s_k / len(s_items) - twin_k / len(twin)),
            "newcombe_independent_95_ci": [r6(lo_d), r6(hi_d)],
            "twin_true_positives": len(twin_tp),
            "twin_true_positives_on_a_file_with_a_discipline_true_positive": twin_tp_on_discipline_tp_file,
            "twin_config_false_positives_by_file_name": dict(sorted(config_fp_files.items())),
        },
        "design_power": {
            "pass_probability_n141_true_0_95": r6(wilson_pass_probability(141, 0.95, 0.90)),
            "smallest_n_clearing_0_90_at_0_95": clearing[0],
            "sizes_up_to_141_clearing_0_90_at_0_95": clearing,
            "wilson_lower_bound_k216_n245": r6(wilson_score_interval(216, 245)[0]),
            "wilson_lower_bound_k134_n141": r6(wilson_score_interval(134, 141)[0]),
        },
        "mdd_unpooled_vs_fleiss_pooled": [
            {
                "n1": n1,
                "n2": n2,
                "p1": p1,
                "unpooled": r6(minimum_detectable_difference(n1, n2, p1)),
                "fleiss_pooled": r6(fleiss_pooled_mdd(n1, n2, p1)),
            }
            for n1, n2, p1 in [(141, 141, 0.55), (245, 245, 0.55), (144, 150, 0.55), (10, 10, 0.30)]
        ],
    }


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

    # 8. sample_size_for_interval refuses an unbounded result (mirrors bounds.rs)
    for w in (1e-300, 1e-5):
        try:
            sample_size_for_interval(0.90 if w < 1e-6 else 0.50, w, 0.95)
        except ValueError as e:
            assert "exceeds" in str(e)
        else:
            raise AssertionError(f"sample_size_for_interval accepted half-width {w}")
    assert 90_000_000 < sample_size_for_interval(0.50, 1e-4, 0.95) <= MAX_SAMPLE_SIZE

    # 9. Fleiss pooled-null MDD, pinned to scipy.optimize.brentq on the same equation;
    # the unpooled form is the smaller (optimistic) one.
    for (n1, n2, p1), ref in [
        ((141, 141, 0.55), 0.160403775020709),
        ((245, 245, 0.55), 0.12307685072897513),
        ((144, 150, 0.55), 0.15723812495195078),
        ((10, 10, 0.30), 0.5804903764235345),
    ]:
        pooled = fleiss_pooled_mdd(n1, n2, p1)
        assert abs(pooled - ref) < 1e-9, (n1, n2, p1, pooled)
        assert minimum_detectable_difference(n1, n2, p1) < pooled

    # 10. Design power, pinned to scipy.stats.binom / statsmodels Wilson values
    assert abs(wilson_pass_probability(141, 0.95, 0.90) - 0.5911030642667645) < 1e-9
    clearing = wilson_floor_clearing_sizes(0.95, 0.90, 141)
    assert clearing[0] == 127 and 131 not in clearing and 141 in clearing, clearing
    assert abs(wilson_score_interval(216, 245)[0] - 0.8351740269146575) < 1e-9
    assert abs(wilson_score_interval(134, 141)[0] - 0.9010743041142572) < 1e-9

    # 11. Newcombe hybrid-score interval, pinned to statsmodels
    # `confint_proportions_2indep(119, 144, 9, 150, method="newcomb")`
    lo_d, hi_d = newcombe_difference_ci(119, 144, 9, 150)
    assert abs(lo_d - 0.6802131110452675) < 1e-9 and abs(hi_d - 0.8265277396812667) < 1e-9

    # 12. Cluster statistics on a hand-checkable input: clusters (1, 1) and (0, 1),
    # p = 0.5, variance = 2 * (0.25 + 0.25) / 4 = 0.25, binomial variance 0.125 -> deff 2.
    toy = cluster_robust_ratio([(1, 1), (0, 1)])
    assert abs(toy["design_effect"] - 2.0) < 1e-12 and abs(toy["effective_n"] - 1.0) < 1e-12
    assert _quantile_type7([0.0, 1.0, 2.0, 3.0], 0.5) == 1.5

    # 13. Post-hoc audit on the committed data (cross-checked with numpy / scipy).
    if (RESULTS_DIR / "findings.json").exists():
        clusters = pull_request_clusters(load_labelled_items(), "structural-ast-and-deterministic")
        robust = cluster_robust_ratio(clusters)
        assert (robust["k"], robust["n"], robust["clusters"]) == (119, 144, 47)
        assert max(n for _, n in clusters) == 52
        assert abs(robust["design_effect"] - 6.00057565055008) < 1e-9
        assert abs(robust["ci"][0] - 0.674842952475609) < 1e-9
        assert abs(robust["ci"][1] - 0.9779348253021687) < 1e-9
        b_lo, b_hi = pr_cluster_bootstrap_ci(clusters)
        assert abs(b_lo - 0.62195122) < 1e-8 and abs(b_hi - 0.92777828) < 1e-8, (b_lo, b_hi)
        committed = RESULTS_DIR / "post-hoc-audit.json"
        if committed.exists():
            assert json.loads(committed.read_text()) == json.loads(
                json.dumps(post_hoc_audit())
            ), "post-hoc-audit.json is stale; rerun with --post-hoc-audit --write"

    print("All benchmark_precision.py self-tests passed.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true", help="Run unit tests and exit")
    parser.add_argument(
        "--post-hoc-audit",
        action="store_true",
        help="Print the post-hoc audit statistics (not pre-registered) as JSON",
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="With --post-hoc-audit, write docs/benchmarks/results/post-hoc-audit.json",
    )
    args = parser.parse_args()
    if args.test:
        run_self_tests()
        return 0
    if args.post_hoc_audit:
        text = json.dumps(post_hoc_audit(), indent=2) + "\n"
        if args.write:
            (RESULTS_DIR / "post-hoc-audit.json").write_text(text)
        else:
            sys.stdout.write(text)
        return 0
    parser.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
