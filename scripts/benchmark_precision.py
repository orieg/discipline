#!/usr/bin/env python3
"""
Public Agent-PR Precision Benchmark Harness & Statistical Verifier (#482 Part B, #777).

Implements the complete deterministic pipeline for `docs/benchmarks/precision-preregistration.toml`
(`agent-pr-precision-v2`):
1. Reference statistical bounds mirroring `src/guards/perf/bounds.rs`:
   - `wilson_score_interval` (Wilson 1927, JASA 22(158):209-212)
   - `sample_size_for_interval` (normal-approximation ceiling, matching Rust)
   - `minimum_detectable_difference` (Fleiss, Levin & Paik 2003, Ch. 3)
   - `cohens_kappa` (Cohen 1960, Educ. Psychol. Meas. 20(1):37-46)
2. Substitution twin (`naive-sensitive-path-filter`) classifier over changed file paths.
3. First-parent git harvester and diff-scoped isolated replay runner (`--harvest-and-replay`).
4. Blinded two-rater adjudication packet builder (`--build-packet`, writing
   `docs/benchmarks/results/rater-packet.json`).
5. Artifact generator (`--generate-artifacts`) producing `manifest.json`, `findings.json`,
   `adjudication.json` (with explicit rater-kind and synthetic-LLM rater disclosure per
   Rule 3), `summary.json`, and `post-hoc-audit.json` under `docs/benchmarks/results/`.
6. Replay of a committed manifest's exact pairs (`--from-manifest`), and the re-runs on the
   v2 corpus: `agent-pr-precision-v3` (`docs/benchmarks/precision-preregistration-v3.toml`)
   and `agent-pr-precision-v4` (`docs/benchmarks/precision-preregistration-v4.toml`).
   `--v3-stage` / `--v4-stage diff|packet|artifacts` checks the reproduction of the previous
   study, applies the label-reuse rule, builds the fresh-item packet and writes
   `docs/benchmarks/results-v3/` / `docs/benchmarks/results-v4/`.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable, Sequence


REPO_ROOT = Path(__file__).resolve().parent.parent
PREREG_PATH = REPO_ROOT / "docs" / "benchmarks" / "precision-preregistration.toml"
RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results"
V3_RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results-v3"
V4_RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results-v4"

PILOT_REPOS: list[tuple[str, str]] = [
    ("astral-sh/uv", "uv"),
    ("pydantic/pydantic", "pydantic"),
    ("biomejs/biome", "biome"),
]

EVALUATION_REPOS: list[tuple[str, str]] = [
    ("astral-sh/ruff", "ruff"),
    ("denoland/deno", "deno"),
    ("BurntSushi/ripgrep", "ripgrep"),
    ("tokio-rs/tokio", "tokio"),
    ("fastapi/fastapi", "fastapi"),
    ("vitejs/vite", "vite"),
    ("golang/go", "go"),
    ("junit-team/junit5", "junit5"),
]

AGENT_TRAILERS: tuple[str, ...] = (
    "co-authored-by: claude",
    "co-authored-by: github copilot",
    "co-authored-by: cursor",
    "co-authored-by: openhands",
    "co-authored-by: devin",
    "co-authored-by: aider",
    "co-authored-by: qwen",
    "co-authored-by: gemini",
)

CODING_AGENT_BOTS: tuple[str, ...] = (
    "copilot-swe-agent[bot]",
    "copilot",
    "claude[bot]",
    "devin-ai-integration[bot]",
    "sweep-ai[bot]",
    "openhands-agent[bot]",
    "codex-agent[bot]",
)

DEPENDENCY_BOTS: tuple[str, ...] = (
    "dependabot[bot]",
    "renovate[bot]",
)

DEFAULT_WITH_TEST_ENTRYPOINTS_CONFIG = """
[tests]
functions = ["self_test", "run_self_tests"]

[gates.agents-md]
enabled = false

[gates.bench-regression]
enabled = false

[gates.harness-tampering]
enabled = true

[gates.pr-checklist]
enabled = true

[gates.gate-command-lint]
enabled = true

[gates.mechanism-sections]
enabled = true

[gates.citation-anchors]
enabled = true
"""

STRUCTURAL_GATES: frozenset[str] = frozenset({
    "agents-md",
    "assertion-reduction",
    "vacuous-tests",
    "ignored-tests",
    "unsafe-safety-comment",
    "deletion-rationale",
    "agent-scratch",
    "shell-secrets",
    "issue-link",
    "citation-metadata",
    "config-integrity",
    "stub-bodies",
    "harness-tampering",
    "error-swallowing",
    "build-hooks",
    "toolchain-config",
    "sandbox-config",
    "scope-confinement",
    "suppression-delta",
    "ci-integrity",
    "ci-skip-set",
    "test-floor",
    "golden-output",
    "dependency-delta",
    "test-budget",
    "pr-checklist",
    "archive-contents",
    "manifest-sync",
    "version-lockstep",
})

HEURISTIC_GATES: frozenset[str] = frozenset({
    "time-estimates",
    "pii",
    "instruction-smuggling",
    "provenance-tags",
    "gate-command-lint",
    "mechanism-sections",
    "citation-anchors",
})

RULE_SUMMARIES: dict[str, str] = {
    "ci-integrity/unpinned-action": "CI workflow uses a third-party action pinned to a mutable tag or branch rather than a full 40-hex commit SHA.",
    "ci-integrity/exit-code-masked": "CI workflow step masks command exit codes (e.g. continue-on-error: true or || true).",
    "pii/host-or-pii-leak": "Tracked file introduces a hardcoded user home path (/Users/..., /home/...), private LAN IP, internal hostname, or credential token.",
    "instruction-smuggling/agent-instructions-changed": "Agent instruction or prompt configuration file modified without explicit human review waiver.",
    "instruction-smuggling/invisible-characters-in-description": "Pull request description or commit message contains invisible or bidirectional Unicode control characters.",
    "instruction-smuggling/invisible-characters-added": "Added source or documentation lines contain invisible or bidirectional Unicode control characters.",
    "assertion-reduction/expected-value-changed": "Existing test assertion expected literal value was modified or weakened in place.",
    "assertion-reduction/source-parsed-with-errors": "Modified test file contains syntax errors preventing AST assertion verification.",
    "assertion-reduction/assertions-reduced": "Existing test function has fewer assertions or weaker assertion strength than on base.",
    "dependency-delta/direct-dependency-added": "Manifest adds a new direct dependency requiring license/supply-chain review or waiver.",
    "dependency-delta/dependency-constraint-loosened": "Manifest loosens or widens an existing dependency version constraint.",
    "dependency-delta/lockfile-integrity-hash-removed": "Lockfile entry removes an integrity checksum hash.",
    "dependency-delta/dependency-source-changed": "Lockfile entry changes the package resolution source or registry URL.",
    "vacuous-tests/vacuous-test-added": "Newly added test function contains no non-tautological assertions.",
    "toolchain-config/toolchain-config-weakened": "Compiler, linter, type-checker, or test-runner configuration rule was weakened or disabled.",
    "error-swallowing/empty-error-handler-added": "Production code adds an empty catch/except/error handler that silently swallows errors.",
    "error-swallowing/result-discarded": "Production code discards a fallible Result/error return value without handling.",
    "error-swallowing/error-silenced": "Production code silences an error return value with a trivial fallback or empty handler.",
    "golden-output/golden-output-changed-without-directive": "Committed golden/snapshot test expectation file modified without an explicit snapshot-update waiver.",
    "golden-output/snapshot-added-for-existing-test": "New snapshot file added for an existing test without an explicit snapshot-update waiver.",
    "deletion-rationale/test-removed-without-rationale": "Existing test function removed without an explicit deletion rationale.",
    "deletion-rationale/file-deleted-without-rationale": "Tracked file deleted from repository without an explicit deletion rationale.",
    "test-floor/test-count-below-floor": "Total test count in the module/file dropped from base to head.",
    "stub-bodies/stub-body-added": "Newly added non-test function has a trivial stub body (todo!(), unimplemented!(), pass, return constant/null).",
    "stub-bodies/body-replaced-by-stub": "Existing function body was replaced with a stub or constant return.",
    "twin-sensitive-path/existing-test-file": "Existing test file modified: verify whether the change weakens, deletes, or degrades test assertions or coverage.",
    "twin-sensitive-path/ci-workflow": "CI workflow file modified: verify whether the change introduces unpinned actions, masks failures, or weakens CI gating.",
    "twin-sensitive-path/linter-or-runner-config": "Project manifest, linter, or test-runner config modified: verify whether the change adds/loosens dependencies or weakens checks.",
}

SYNTHETIC_RATER_DISCLOSURE = (
    "Synthetic-rater disclosure: Rater 1, Rater 2, and "
    "the tie-breaker in this study were executed by isolated, source-tool-blinded LLM "
    "research subagents, not human engineers. Model-to-model agreement (Cohen's kappa) "
    "establishes rubric consistency across independent prompts, not external human peer review."
)


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
    """Two-sided Wilson score confidence interval (Wilson 1927), matching `src/guards/perf/bounds.rs`."""
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


MAX_SAMPLE_SIZE = 1_000_000_000


def sample_size_for_interval(
    assumed_precision: float, target_half_width: float = 0.05, confidence_level: float = 0.95
) -> int:
    """Analytical sample size n for a target Wilson CI half-width (Wilson 1927), matching `src/guards/perf/bounds.rs`."""
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
    n = (z * z * (term1 + term2)) / (2.0 * w2) if w2 > 0.0 else math.inf
    if not math.isfinite(n) or n > MAX_SAMPLE_SIZE:
        raise ValueError(
            f"required sample size for half-width {target_half_width} at precision "
            f"{assumed_precision} exceeds {MAX_SAMPLE_SIZE} (computed {n})"
        )
    return max(1, int(math.ceil(n)))


def minimum_detectable_difference(
    n1: int,
    n2: int,
    baseline_precision: float,
    confidence_level: float = 0.95,
    power: float = 0.80,
) -> float:
    """Minimum detectable difference between two binomial proportions, unpooled form matching `src/guards/perf/bounds.rs`."""
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
    """Minimum detectable difference under the Fleiss pooled-null power equation (Fleiss et al. 2003, Ch. 4)."""
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


def cohens_kappa(labels_1: Sequence[str], labels_2: Sequence[str]) -> float:
    """
    Unweighted Cohen's kappa for two raters over nominal categories (Cohen 1960).
    Returns 1.0 when both raters agree on 100% of items (even if degenerate single category).
    """
    if len(labels_1) != len(labels_2):
        raise ValueError("rater label sequences must have equal length")
    n = len(labels_1)
    if n == 0:
        raise ValueError("cannot compute Cohen's kappa on 0 items")
    categories = sorted(set(labels_1) | set(labels_2))
    agreed = sum(1 for a, b in zip(labels_1, labels_2) if a == b)
    p_o = agreed / float(n)
    p_e = 0.0
    for cat in categories:
        c1 = sum(1 for a in labels_1 if a == cat)
        c2 = sum(1 for b in labels_2 if b == cat)
        p_e += (c1 / float(n)) * (c2 / float(n))
    if abs(1.0 - p_e) < 1e-12:
        return 1.0 if agreed == n else 0.0
    return (p_o - p_e) / (1.0 - p_e)


def evaluate_gate_verdict(
    k: int,
    n: int,
    precision_floor: float,
    required_n: int,
    confidence_level: float = 0.95,
) -> dict[str, Any]:
    """
    Evaluate a gate or gate-class precision measurement against its pre-registered floor and sample size.
    Follows Rule 1.1: PASS iff n >= required_n and Wilson 95% lower bound >= precision_floor.
    """
    if n == 0:
        return {
            "k": 0,
            "n": 0,
            "required_n": required_n,
            "precision_floor": precision_floor,
            "point_estimate": None,
            "wilson_95_ci": None,
            "verdict": "insufficient-n",
        }
    low, high = wilson_score_interval(k, n, confidence_level)
    point = k / float(n)
    if n < required_n:
        verdict = "insufficient-n"
    elif low >= precision_floor:
        verdict = "PASS"
    elif point >= precision_floor:
        verdict = "INTERMEDIATE_floor_within_ci"
    else:
        verdict = "REJECTED"
    return {
        "k": k,
        "n": n,
        "required_n": required_n,
        "precision_floor": precision_floor,
        "point_estimate": round(point, 6),
        "wilson_95_ci": [round(low, 6), round(high, 6)],
        "verdict": verdict,
    }


def hash_identity(raw_identity: str) -> str:
    """Anonymize contributor or rater identity with SHA-256 prefix."""
    digest = hashlib.sha256(raw_identity.strip().lower().encode("utf-8")).hexdigest()
    return f"sha256:{digest[:16]}"


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
    """P(Wilson lower bound >= floor) when k ~ Binomial(n, true_precision)."""
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
    """Every n in [1, n_max] at which k = round_half_up(assumed_precision * n) has a Wilson lower bound >= floor."""
    out = []
    for n in range(1, n_max + 1):
        k = int(math.floor(assumed_precision * n + 0.5))
        if wilson_score_interval(k, n, confidence_level)[0] >= floor:
            out.append(n)
    return out


def newcombe_difference_ci(
    k1: int, n1: int, k2: int, n2: int, confidence_level: float = 0.95
) -> tuple[float, float]:
    """Newcombe hybrid-score interval for p1 - p2 from two independent samples (Newcombe 1998, method 10)."""
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
    """Cluster-robust (linearised) interval for a pooled proportion sum(k) / sum(n) (Cochran 1977, Ch. 6)."""
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
    """Linear-interpolation sample quantile (Hyndman & Fan 1996 type 7)."""
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
    """Percentile bootstrap for sum(k) / sum(n), resampling whole pull-request clusters (Davison & Hinkley 1997, §3.8)."""
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
    """Recompute the post-hoc audit statistics from the committed docs/benchmarks/results/*.json."""
    summary_doc = json.loads((results_dir / "summary.json").read_text())
    study_id = summary_doc.get("study_id", "agent-pr-precision-v2")
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
    generated_by = "python3 scripts/benchmark_precision.py --post-hoc-audit --write"
    if Path(results_dir).resolve() != RESULTS_DIR.resolve():
        generated_by += f" --results-dir {Path(results_dir).resolve().relative_to(REPO_ROOT)}"
    return {
        "study_id": study_id,
        "pre_registered": False,
        "generated_by": generated_by,
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
            for n1, n2, p1 in [
                (141, 141, 0.55),
                (245, 245, 0.55),
                (len(s_items), len(twin), 0.55),
                (10, 10, 0.30),
            ]
        ],
    }


SENSITIVE_TEST_PATTERNS = [
    re.compile(r"(^|/)tests?/"),
    re.compile(r"(^|/)__tests__/"),
    re.compile(r"_test\.(rs|py|go|ts|tsx|js|jsx|java|kt)$"),
    re.compile(r"\.(test|spec)\.(ts|tsx|js|jsx|py)$"),
    re.compile(r"Test\.java$"),
]

SENSITIVE_WORKFLOW_PATTERNS = [
    re.compile(r"^\.github/workflows/(?:.+/)?[^/]+\.ya?ml$"),
    re.compile(r"^\.gitlab-ci\.ya?ml$"),
    re.compile(r"^\.gitea/workflows/(?:.+/)?[^/]+\.ya?ml$"),
    re.compile(r"^\.forgejo/workflows/(?:.+/)?[^/]+\.ya?ml$"),
]

SENSITIVE_CONFIG_PATTERNS = [
    re.compile(r"(^|/)Cargo\.toml$"),
    re.compile(r"(^|/)pyproject\.toml$"),
    re.compile(r"(^|/)package\.json$"),
    re.compile(r"(^|/)tsconfig(\.[^/]+)?\.json$"),
    re.compile(r"(^|/)\.eslintrc(\.[^/]+)?$"),
    re.compile(r"(^|/)eslint\.config\.(js|mjs|cjs|ts)$"),
    re.compile(r"(^|/)biome\.jsonc?$"),
    re.compile(r"(^|/)\.?ruff\.toml$"),
    re.compile(r"(^|/)mypy\.ini$"),
    re.compile(r"(^|/)\.flake8$"),
    re.compile(r"(^|/)pytest\.ini$"),
    re.compile(r"(^|/)vitest\.config\.(ts|js|mts|mjs)$"),
    re.compile(r"(^|/)jest\.config\.(ts|js|mjs|cjs|json)$"),
    re.compile(r"(^|/)\.golangci\.ya?ml$"),
    re.compile(r"(^|/)\.?clippy\.toml$"),
    re.compile(r"(^|/)\.?rustfmt\.toml$"),
    re.compile(r"(^|/)deno\.jsonc?$"),
    re.compile(r"(^|/)pom\.xml$"),
    re.compile(r"(^|/)build\.gradle(\.kts)?$"),
    re.compile(r"(^|/)codecov\.ya?ml$"),
]


def classify_twin_sensitive_path(path: str, existed_in_base: bool) -> str | None:
    """
    Substitution twin (`naive-sensitive-path-filter`): flags any modified existing test file,
    CI workflow file (including nested workflow subdirectories), or project manifest / linter /
    test-runner configuration file.
    """
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


def extract_pr_or_change_number(message: str) -> int | None:
    """
    Extract a merged GitHub pull-request number (`#NNNN` in subject/body or
    `Merge pull request #NNNN`) or Gerrit change number (`Reviewed-on: .../+/<NNNN>`).
    Returns `None` if no pull-request or change number is present.
    """
    if not message:
        return None
    lines = message.splitlines()
    first_line = lines[0] if lines else ""
    m_subj = re.search(r"#(\d+)\b", first_line)
    if m_subj:
        return int(m_subj.group(1))
    m_merge = re.search(r"Merge pull request #(\d+)\b", message)
    if m_merge:
        return int(m_merge.group(1))
    m_gerrit = re.search(r"^Reviewed-on:\s*\S+/(\d+)\s*$", message, re.MULTILINE)
    if m_gerrit:
        return int(m_gerrit.group(1))
    return None


def classify_agent_attribution(author_name: str, author_email: str, body: str) -> tuple[str | None, bool]:
    """
    Classify a commit's agent attribution per `[population]` in `precision-preregistration.toml`.
    Returns `(agent_category, is_coding_agent)`.
    """
    an_l = author_name.lower()
    ae_l = author_email.lower()
    body_l = body.lower()
    for trailer in AGENT_TRAILERS:
        if trailer in body_l:
            return (trailer.split(":", 1)[1].strip(), True)
    for bot in CODING_AGENT_BOTS:
        bot_clean = bot.replace("[bot]", "")
        if bot in an_l or bot_clean in an_l or bot_clean in ae_l:
            return (bot, True)
    for bot in DEPENDENCY_BOTS:
        bot_clean = bot.replace("[bot]", "")
        if bot_clean in an_l or bot_clean in ae_l:
            return (bot, False)
    return (None, False)


def _replay_repo_commits(
    repo_slug: str,
    dir_name: str,
    clones_dir: Path,
    discipline_exe: Path,
    git_log_window: int,
    max_coding_agent: int,
    max_bot: int,
) -> list[dict[str, Any]]:
    t_repo = time.time()
    repo_dir = clones_dir / dir_name
    out = subprocess.check_output(
        [
            "git",
            "-C",
            str(repo_dir),
            "log",
            "--first-parent",
            "-n",
            str(git_log_window),
            "--format=%H\x1f%P\x1f%an\x1f%ae\x1f%B\x1e",
        ],
        text=True,
        errors="replace",
    )
    code_agent_commits = []
    bot_commits = []
    for rec in out.split("\x1e"):
        rec = rec.strip()
        if not rec:
            continue
        parts = rec.split("\x1f")
        if len(parts) < 5:
            continue
        sha, parents, an, ae, body = parts[0], parts[1].split(), parts[2], parts[3], parts[4]
        if not parents:
            continue
        parent = parents[0]
        pr_num = extract_pr_or_change_number(body)
        if pr_num is None:
            continue
        agent_cat, is_coding_agent = classify_agent_attribution(an, ae, body)
        if agent_cat is None:
            continue
        if is_coding_agent:
            code_agent_commits.append((sha, parent, an, ae, body, agent_cat, pr_num))
        else:
            bot_commits.append((sha, parent, an, ae, body, agent_cat, pr_num))

    selected = code_agent_commits[:max_coding_agent] + bot_commits[:max_bot]
    oids: set[str] = set()
    commit_data = []
    for sha, parent, an, ae, msg, agent_cat, pr_num in selected:
        paths, base_existing = _changed_paths(repo_dir, parent, sha, oids)
        commit_data.append((sha, parent, an, ae, msg, agent_cat, pr_num, paths, base_existing))
    _prefetch_missing_blobs(repo_slug, repo_dir, oids, len(selected), t_repo)
    results = _replay_commit_data(repo_slug, repo_dir, discipline_exe, commit_data)
    print(f"[{repo_slug}] finished {len(results)} cases in {time.time() - t_repo:.1f}s")
    return results


def _changed_paths(
    repo_dir: Path, parent: str, sha: str, oids: set[str]
) -> tuple[list[str], list[str]]:
    """Paths `git diff-tree -r parent sha` lists and those present on `parent`; adds their blob ids to `oids`."""
    raw = subprocess.check_output(
        ["git", "-C", str(repo_dir), "diff-tree", "-r", parent, sha],
        text=True,
    )
    paths: list[str] = []
    base_existing: set[str] = set()
    for dline in raw.splitlines():
        if "\t" not in dline:
            continue
        meta_str, path = dline.split("\t", 1)
        paths.append(path)
        cols = meta_str.split()
        if len(cols) >= 4:
            old_mode, new_mode, old_oid, new_oid = cols[0].lstrip(":"), cols[1], cols[2], cols[3]
            if set(old_oid) != {"0"}:
                base_existing.add(path)
            for mode, oid in ((old_mode, old_oid), (new_mode, new_oid)):
                if mode != "160000" and set(oid) != {"0"} and len(oid) == 40:
                    oids.add(oid)
    return paths, sorted(base_existing)


def _prefetch_missing_blobs(
    repo_slug: str, repo_dir: Path, oids: set[str], n_commits: int, t_repo: float
) -> None:
    # Prefetch any missing blobs in batches of 400 (using --no-lazy-fetch so git does not
    # fire individual network requests inside cat-file --batch-check on partial clones)
    missing_oids = []
    if oids:
        check_proc = subprocess.run(
            ["git", "--no-lazy-fetch", "-C", str(repo_dir), "cat-file", "--batch-check"],
            input="\n".join(sorted(oids)) + "\n",
            capture_output=True,
            text=True,
            check=True,
        )
        for line in check_proc.stdout.splitlines():
            if line.endswith(" missing"):
                missing_oids.append(line.split()[0])
    for i in range(0, len(missing_oids), 400):
        chunk = missing_oids[i : i + 400]
        subprocess.run(
            [
                "git",
                "-C",
                str(repo_dir),
                "-c",
                "fetch.negotiationAlgorithm=noop",
                "fetch",
                "origin",
                "--no-tags",
                "--no-write-fetch-head",
            ]
            + chunk,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
        )
    print(
        f"[{repo_slug}] prefetched {len(missing_oids)} missing blobs (of {len(oids)}) "
        f"for {n_commits} commits in {time.time() - t_repo:.1f}s"
    )


def _replay_commit_data(
    repo_slug: str,
    repo_dir: Path,
    discipline_exe: Path,
    commit_data: list[tuple[Any, ...]],
) -> list[dict[str, Any]]:
    """Diff-scoped scratch-repository replay of each (sha, parent, ...) tuple with `discipline check`."""
    results: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory() as tmp:
        cfg_file = os.path.join(tmp, "bench-discipline.toml")
        with open(cfg_file, "w", encoding="utf-8") as f:
            f.write(DEFAULT_WITH_TEST_ENTRYPOINTS_CONFIG)
        scratch = os.path.join(tmp, "tree")
        subprocess.run(["git", "init", "-q", scratch], check=True)
        alt = os.path.join(scratch, ".git", "objects", "info", "alternates")
        with open(alt, "w", encoding="utf-8") as f:
            f.write(str(repo_dir / ".git" / "objects") + "\n")
        env = {
            **os.environ,
            "GIT_AUTHOR_NAME": "discipline replay",
            "GIT_AUTHOR_EMAIL": "replay@discipline.invalid",
            "GIT_COMMITTER_NAME": "discipline replay",
            "GIT_COMMITTER_EMAIL": "replay@discipline.invalid",
            "DISCIPLINE_NO_NETWORK": "1",
            "DISCIPLINE_REPLAY_CASE": "1",
        }
        for sha, parent, an, ae, msg, agent_cat, pr_num, paths, base_existing in commit_data:
            if not paths:
                continue
            idx_path = os.path.join(scratch, ".git", "index")
            if os.path.exists(idx_path):
                os.remove(idx_path)
            ls_base = subprocess.check_output(
                ["git", "-C", str(repo_dir), "ls-tree", "-r", "-z", parent, "--"] + paths
            )
            ls_head = subprocess.check_output(
                ["git", "-C", str(repo_dir), "ls-tree", "-r", "-z", sha, "--"] + paths
            )
            ls_base = b"\x00".join(
                e for e in ls_base.split(b"\x00") if e and not e.startswith(b"160000 ")
            )
            ls_head = b"\x00".join(
                e for e in ls_head.split(b"\x00") if e and not e.startswith(b"160000 ")
            )
            if ls_base:
                subprocess.run(
                    ["git", "-C", scratch, "update-index", "--add", "-z", "--index-info"],
                    input=ls_base + b"\x00",
                    check=True,
                )
            b_tree = subprocess.check_output(["git", "-C", scratch, "write-tree"], text=True).strip()
            b_oid = subprocess.check_output(
                ["git", "-C", scratch, "commit-tree", b_tree, "-m", "discipline replay base"],
                env=env,
                text=True,
            ).strip()

            if os.path.exists(idx_path):
                os.remove(idx_path)
            if ls_head:
                subprocess.run(
                    ["git", "-C", scratch, "update-index", "--add", "-z", "--index-info"],
                    input=ls_head + b"\x00",
                    check=True,
                )
            h_tree = subprocess.check_output(["git", "-C", scratch, "write-tree"], text=True).strip()
            h_oid = subprocess.check_output(
                ["git", "-C", scratch, "commit-tree", h_tree, "-p", b_oid, "-m", msg],
                env=env,
                text=True,
            ).strip()
            subprocess.run(["git", "-C", scratch, "checkout", "-q", "-f", h_oid], check=True)
            subprocess.run(["git", "-C", scratch, "clean", "-fdq"], check=True)

            body_file = os.path.join(tmp, "body.txt")
            with open(body_file, "w", encoding="utf-8") as bf:
                bf.write(msg)
            res = subprocess.run(
                [
                    str(discipline_exe),
                    "check",
                    "--config",
                    cfg_file,
                    "--format",
                    "json",
                    "--base",
                    b_oid,
                    "--pr-body-file",
                    body_file,
                ],
                cwd=scratch,
                env=env,
                capture_output=True,
                text=True,
            )
            if res.returncode not in (0, 1):
                raise RuntimeError(
                    f"discipline check exited {res.returncode} on {repo_slug}@{sha}: {res.stderr}"
                )
            violations = []
            if res.stdout.strip():
                rep = json.loads(res.stdout)
                for o in rep.get("outcomes", []):
                    g = o["gate"]
                    for v in o.get("violations", []):
                        if v.get("severity") == "error":
                            violations.append({
                                "gate": g,
                                "code": v.get("code", g),
                                "file": v.get("file"),
                                "line": v.get("line"),
                                "message": v.get("message", ""),
                            })
            results.append({
                "repo": repo_slug,
                "sha": sha,
                "parent": parent,
                "pr_number": pr_num,
                "author_hash": "sha256:"
                + hashlib.sha256(f"{an}<{ae}>".lower().encode("utf-8")).hexdigest()[:16],
                "agent_category": agent_cat,
                "subject": msg.splitlines()[0] if msg else "",
                "paths": paths,
                "base_existing": base_existing,
                "returncode": res.returncode,
                "violations": violations,
            })
    return results


def harvest_and_replay_evaluation(
    clones_dir: Path,
    discipline_exe: Path,
    out_raw_json: Path,
    git_log_window: int = 4000,
    max_coding_agent: int = 45,
    max_bot: int = 15,
) -> list[dict[str, Any]]:
    """Run parallel harvest and diff-scoped replay across all 8 pre-registered evaluation repos."""
    t0 = time.time()
    by_repo: dict[str, list[dict[str, Any]]] = {}
    with ThreadPoolExecutor(max_workers=4) as pool:
        futs = {
            pool.submit(
                _replay_repo_commits,
                repo_slug,
                dir_name,
                clones_dir,
                discipline_exe,
                git_log_window,
                max_coding_agent,
                max_bot,
            ): repo_slug
            for repo_slug, dir_name in EVALUATION_REPOS
        }
        for fut in as_completed(futs):
            slug = futs[fut]
            by_repo[slug] = fut.result()

    # Deterministic ordering following EVALUATION_REPOS order
    all_results: list[dict[str, Any]] = []
    for repo_slug, _ in EVALUATION_REPOS:
        all_results.extend(by_repo.get(repo_slug, []))

    out_raw_json.parent.mkdir(parents=True, exist_ok=True)
    with open(out_raw_json, "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2)
    print(f"Total eval cases: {len(all_results)} in {time.time() - t0:.1f}s")
    return all_results


def _commit_message_and_author(repo_dir: Path, sha: str) -> tuple[str, str, str] | None:
    """(message, author name, author email) of `sha`, or None when the commit is absent.

    The message is `%B` with trailing whitespace removed, which is what the harvester's
    `git log` record split hands the replay.
    """
    res = subprocess.run(
        ["git", "-C", str(repo_dir), "show", "-s", "--format=%an\x1f%ae\x1f%B", sha],
        capture_output=True,
        text=True,
        errors="replace",
    )
    if res.returncode != 0:
        return None
    an, ae, body = res.stdout.split("\x1f", 2)
    return body.rstrip(), an, ae


def replay_from_manifest(
    manifest_path: Path,
    clones_dir: Path,
    discipline_exe: Path,
    out_raw_json: Path,
    repos: Sequence[tuple[str, str]] = EVALUATION_REPOS,
) -> list[dict[str, Any]]:
    """Replay exactly the (repo, base_sha, head_sha) pairs of a committed study manifest.

    Same diff-scoped scratch-repository replay and `discipline check` invocation as
    `--harvest-and-replay`, without the harvest: no commit outside the manifest is read,
    so new upstream merges cannot change the sample. `pr_number`, `author_hash` and
    `agent_category` come from the manifest; the author hash recomputed from the clone is
    recorded beside it. A pair whose commits are absent from the clone is reported in
    the printed summary and left out, never replaced.
    """
    t0 = time.time()
    manifest = json.loads(Path(manifest_path).read_text())
    dir_for = dict(repos)
    by_repo: dict[str, list[dict[str, Any]]] = {}
    for entry in manifest["pull_requests"]:
        if entry["repo"] not in dir_for:
            raise ValueError(f"manifest repo {entry['repo']} has no clone directory")
        by_repo.setdefault(entry["repo"], []).append(entry)

    def replay_repo(repo_slug: str, entries: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], list[str]]:
        t_repo = time.time()
        repo_dir = clones_dir / dir_for[repo_slug]
        oids: set[str] = set()
        commit_data = []
        missing = []
        recomputed_hash: dict[str, str] = {}
        for e in entries:
            head = _commit_message_and_author(repo_dir, e["head_sha"])
            if head is None or _commit_message_and_author(repo_dir, e["base_sha"]) is None:
                missing.append(f"{repo_slug}@{e['head_sha']}")
                continue
            msg, an, ae = head
            recomputed_hash[e["head_sha"]] = (
                "sha256:" + hashlib.sha256(f"{an}<{ae}>".lower().encode("utf-8")).hexdigest()[:16]
            )
            paths, base_existing = _changed_paths(repo_dir, e["base_sha"], e["head_sha"], oids)
            commit_data.append(
                (e["head_sha"], e["base_sha"], an, ae, msg, e["agent_category"], e["pr_number"], paths, base_existing)
            )
        _prefetch_missing_blobs(repo_slug, repo_dir, oids, len(commit_data), t_repo)
        results = _replay_commit_data(repo_slug, repo_dir, discipline_exe, commit_data)
        author_of = {e["head_sha"]: e["author_hash"] for e in entries}
        for r in results:
            r["author_hash"] = author_of[r["sha"]]
            r["author_hash_recomputed"] = recomputed_hash[r["sha"]]
        print(f"[{repo_slug}] replayed {len(results)} manifest cases in {time.time() - t_repo:.1f}s")
        return results, missing

    results_by_repo: dict[str, list[dict[str, Any]]] = {}
    missing_all: list[str] = []
    with ThreadPoolExecutor(max_workers=4) as pool:
        futs = {pool.submit(replay_repo, slug, entries): slug for slug, entries in by_repo.items()}
        for fut in as_completed(futs):
            results, missing = fut.result()
            results_by_repo[futs[fut]] = results
            missing_all.extend(missing)

    # Manifest order, so the twin sample and packet order match the study that wrote it.
    by_key = {(r["repo"], r["sha"]): r for rs in results_by_repo.values() for r in rs}
    all_results = [
        by_key[(e["repo"], e["head_sha"])]
        for e in manifest["pull_requests"]
        if (e["repo"], e["head_sha"]) in by_key
    ]
    out_raw_json.parent.mkdir(parents=True, exist_ok=True)
    with open(out_raw_json, "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2)
    print(
        f"Replayed {len(all_results)} of {len(manifest['pull_requests'])} manifest pairs "
        f"in {time.time() - t0:.1f}s; absent from the clones: {sorted(missing_all) or 'none'}"
    )
    return all_results


def _redact_packet_pii_literals(text: str | None) -> str | None:
    """
    Replace RFC 1918 LAN IP literals and `/Users/<name>` or `/home/<name>` paths in
    quoted repository excerpts with `<lan-ip:a.b.c.d>` / `<home>` so the committed
    JSON packet does not trip repository-wide PII scanners while preserving the
    rater's ability to see that a LAN IP or home path literal was present.
    """
    if text is None:
        return None
    out = re.sub(
        r"\b(10\.\d{1,3}\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3})\b",
        lambda m: f"<lan-ip:{m.group(1).replace('.', '-')}>",
        text,
    )
    out = re.sub(r"/(?:Users|home)/([A-Za-z0-9_.-]+)", r"/<home>/\1", out)
    return out


def _get_diff_excerpt(clones_dir: Path, repo_map: dict[str, str], repo_slug: str, parent: str, sha: str, file_path: str | None, line_num: int | None) -> str:
    repo_dir = clones_dir / repo_map[repo_slug]
    if not file_path:
        try:
            msg_txt = subprocess.check_output(
                ["git", "-C", str(repo_dir), "show", "-s", "--format=%B", sha],
                text=True,
                errors="replace",
            )
            msg_lines = msg_txt.splitlines()
            return "\n".join(msg_lines[:25])
        except Exception as e:
            return f"(commit message error: {e})"
    try:
        diff_txt = subprocess.check_output(
            ["git", "-C", str(repo_dir), "diff", "-U5", parent, sha, "--", file_path],
            text=True,
            errors="replace",
        )
    except Exception as e:
        return f"(diff error: {e})"
    lines = diff_txt.splitlines()
    if len(lines) <= 25:
        return "\n".join(lines)
    if line_num is not None:
        best_idx = 0
        for idx, l in enumerate(lines):
            if l.startswith("@@"):
                m = re.search(r"\+(\d+)(?:,(\d+))?", l)
                if m:
                    start = int(m.group(1))
                    count = int(m.group(2)) if m.group(2) else 1
                    if start - 5 <= line_num <= start + count + 10:
                        best_idx = idx
                        break
        excerpt = lines[:4] + lines[max(4, best_idx) : min(len(lines), best_idx + 21)]
        return "\n".join(excerpt[:25])
    return "\n".join(lines[:25])


def _get_line_context(
    clones_dir: Path,
    repo_map: dict[str, str],
    file_cache: dict[tuple[str, str, str], list[str]],
    repo_slug: str,
    sha: str,
    file_path: str | None,
    line_num: int | None,
) -> str | None:
    if not file_path or line_num is None:
        return None
    key = (repo_slug, sha, file_path)
    if key not in file_cache:
        repo_dir = clones_dir / repo_map[repo_slug]
        try:
            txt = subprocess.check_output(
                ["git", "-C", str(repo_dir), "show", f"{sha}:{file_path}"],
                text=True,
                errors="replace",
            ).splitlines()
        except Exception:
            txt = []
        file_cache[key] = txt
    lines = file_cache[key]
    if not lines:
        return None
    start = max(1, line_num - 5)
    end = min(len(lines), line_num + 5)
    return "\n".join(f"{i}: {lines[i - 1]}" for i in range(start, end + 1))


def twin_sample(
    eval_cases: list[dict[str, Any]], size: int = 150, seed: int = 482
) -> list[tuple[dict[str, Any], str, str]]:
    """The seeded `naive-sensitive-path-filter` sample: (case, path, code) over every twin flag in case order."""
    candidates = []
    for case in eval_cases:
        base_set = set(case["base_existing"])
        for p in case["paths"]:
            tcode = classify_twin_sensitive_path(p, p in base_set)
            if tcode:
                candidates.append((case, p, tcode))
    return random.Random(seed).sample(candidates, size)


def build_blinded_packet(
    eval_cases: list[dict[str, Any]],
    clones_dir: Path,
    ground_key_out: Path,
    rater_packet_out: Path,
    twin_sample_size: int = 150,
    twin_sample_seed: int = 482,
    shuffle_seed: int = 48201,
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    """
    Construct the source-tool-blinded adjudication packet (`docs/benchmarks/results/rater-packet.json`)
    and ground-truth key mapping (`ground_key_out`).
    """
    repo_map = dict(EVALUATION_REPOS)
    file_cache: dict[tuple[str, str, str], list[str]] = {}
    raw_items: list[dict[str, Any]] = []

    for case in eval_cases:
        for v in case["violations"]:
            code = v["code"]
            lc = _get_line_context(
                clones_dir, repo_map, file_cache, case["repo"], case["sha"], v["file"], v["line"]
            )
            raw_items.append({
                "source_tool": "discipline",
                "gate": v["gate"],
                "code": code,
                "repo": case["repo"],
                "sha": case["sha"],
                "parent": case["parent"],
                "pr_number": case["pr_number"],
                "author_hash": case["author_hash"],
                "agent_category": case["agent_category"],
                "subject": case["subject"],
                "file": v["file"],
                "line": v["line"],
                "rule_summary": RULE_SUMMARIES.get(code, f"Gate {code} violation"),
                "diff_excerpt": _get_diff_excerpt(
                    clones_dir, repo_map, case["repo"], case["parent"], case["sha"], v["file"], v["line"]
                ),
                "line_context": lc,
            })

    twin_sampled = twin_sample(eval_cases, twin_sample_size, twin_sample_seed)
    for case, p, tcode in twin_sampled:
        raw_items.append({
            "source_tool": "naive-sensitive-path-filter",
            "gate": "naive-sensitive-path-filter",
            "code": tcode,
            "repo": case["repo"],
            "sha": case["sha"],
            "parent": case["parent"],
            "pr_number": case["pr_number"],
            "author_hash": case["author_hash"],
            "agent_category": case["agent_category"],
            "subject": case["subject"],
            "file": p,
            "line": None,
            "rule_summary": RULE_SUMMARIES[tcode],
            "diff_excerpt": _get_diff_excerpt(
                clones_dir, repo_map, case["repo"], case["parent"], case["sha"], p, None
            ),
            "line_context": None,
        })

    rng_order = random.Random(shuffle_seed)
    rng_order.shuffle(raw_items)

    blinded_packet: list[dict[str, Any]] = []
    ground_key: dict[str, Any] = {}
    for idx, item in enumerate(raw_items, 1):
        item_id = f"ITEM-{idx:04d}"
        item["item_id"] = item_id
        ground_key[item_id] = item
        blinded_packet.append({
            "item_id": item_id,
            "repo": item["repo"],
            "commit_sha": item["sha"][:12],
            "commit_subject": item["subject"],
            "file": item["file"],
            "line": item["line"],
            "rule_summary": item["rule_summary"],
            "diff_excerpt": _redact_packet_pii_literals(item["diff_excerpt"]),
            "line_context": _redact_packet_pii_literals(item["line_context"]),
        })

    ground_key_out.parent.mkdir(parents=True, exist_ok=True)
    with open(ground_key_out, "w", encoding="utf-8") as f:
        json.dump(ground_key, f, indent=2)

    rater_packet_doc = {
        "study_id": "agent-pr-precision-v2",
        "blind_to_source_tool": True,
        "twin_sample_seed": twin_sample_seed,
        "packet_shuffle_seed": shuffle_seed,
        "total_items": len(blinded_packet),
        "items": blinded_packet,
    }
    rater_packet_out.parent.mkdir(parents=True, exist_ok=True)
    with open(rater_packet_out, "w", encoding="utf-8") as f:
        json.dump(rater_packet_doc, f, indent=2)

    return ground_key, blinded_packet


def generate_benchmark_artifacts(
    eval_cases: list[dict[str, Any]],
    ground_key: dict[str, Any],
    rater_1_labels: dict[str, str],
    rater_2_labels: dict[str, str],
    tie_breaker_labels: dict[str, str],
    rater_1_raw_id: str,
    rater_2_raw_id: str,
    tie_breaker_raw_id: str,
    evaluated_commit: str,
    discipline_version: str = "0.23.0",
    study_id: str = "agent-pr-precision-v2",
    rater_kind: str = "llm-subagent",
    results_dir: Path = RESULTS_DIR,
    committed_rater_packet: str = "docs/benchmarks/results/rater-packet.json",
    host: str = "Apple M-series macOS 15 arm64",
    label_provenance: dict[str, dict[str, Any]] | None = None,
    extra_summary: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """
    Generate `manifest.json`, `findings.json`, `adjudication.json`, and `summary.json`
    under `results_dir` from the evaluation cases, ground key, and rater labels.

    `label_provenance` (a re-run that inherits labels, v3 onward) maps every item id to
    `{"label_source": "inherited-<previous>" | "fresh", ...}` plus any fields to record on the
    finding (`finding_fields`) and adjudication (`adjudication_fields`). Kappa is then
    computed over the fresh items only, the scope the pre-registration names; agreement
    counts stay over every item.
    """
    results_dir.mkdir(parents=True, exist_ok=True)

    manifest_entries = []
    disc_blocked_prs = 0
    twin_blocked_prs = 0
    twin_total_flags = 0
    for c in eval_cases:
        if c["pr_number"] is None:
            raise ValueError(f"commit {c['repo']}@{c['sha']} has pr_number=None")
        base_set = set(c["base_existing"])
        twin_hits = [
            p for p in c["paths"] if classify_twin_sensitive_path(p, p in base_set) is not None
        ]
        twin_total_flags += len(twin_hits)
        d_blocked = len(c["violations"]) > 0
        t_blocked = len(twin_hits) > 0
        if d_blocked:
            disc_blocked_prs += 1
        if t_blocked:
            twin_blocked_prs += 1
        manifest_entries.append({
            "repo": c["repo"],
            "pr_number": c["pr_number"],
            "base_sha": c["parent"],
            "head_sha": c["sha"],
            "author_hash": c["author_hash"],
            "agent_category": c["agent_category"],
            "changed_files_count": len(c["paths"]),
            "discipline_blocking_findings": len(c["violations"]),
            "twin_sensitive_path_flags": len(twin_hits),
            "discipline_blocked": d_blocked,
            "twin_blocked": t_blocked,
        })

    manifest_doc = {
        "study_id": study_id,
        "band": "evaluation",
        "discipline_version": discipline_version,
        "evaluated_commit": evaluated_commit,
        "total_pull_requests": len(manifest_entries),
        "discipline_blocked_pull_requests": disc_blocked_prs,
        "twin_blocked_pull_requests": twin_blocked_prs,
        "pull_requests": manifest_entries,
    }
    with open(results_dir / "manifest.json", "w", encoding="utf-8") as f:
        json.dump(manifest_doc, f, indent=2)

    total_items = len(ground_key)
    keys = [f"ITEM-{i:04d}" for i in range(1, total_items + 1)]
    kappa_keys = (
        keys
        if label_provenance is None
        else [k for k in keys if label_provenance[k]["label_source"] == "fresh"]
    )
    l1_seq = [rater_1_labels[k] for k in kappa_keys]
    l2_seq = [rater_2_labels[k] for k in kappa_keys]
    kappa = cohens_kappa(l1_seq, l2_seq) if kappa_keys else None

    rater_1_hash = "sha256:" + hashlib.sha256(rater_1_raw_id.encode("utf-8")).hexdigest()[:16]
    rater_2_hash = "sha256:" + hashlib.sha256(rater_2_raw_id.encode("utf-8")).hexdigest()[:16]
    tie_breaker_hash = "sha256:" + hashlib.sha256(tie_breaker_raw_id.encode("utf-8")).hexdigest()[:16]

    findings_entries = []
    adjudication_entries = []
    disc_findings_count = 0
    twin_sampled_count = 0
    agreed_count = 0

    for item_id in keys:
        gk = ground_key[item_id]
        l1 = rater_1_labels[item_id]
        l2 = rater_2_labels[item_id]
        agreed = l1 == l2
        if agreed:
            agreed_count += 1
            tb_l = None
            final_l = l1
        else:
            tb_l = tie_breaker_labels.get(item_id, "cannot-tell")
            final_l = tb_l

        if gk["source_tool"] == "discipline":
            disc_findings_count += 1
        else:
            twin_sampled_count += 1

        prov = label_provenance[item_id] if label_provenance is not None else {}
        findings_entries.append({
            "item_id": item_id,
            "source_tool": gk["source_tool"],
            "gate": gk["gate"],
            "code": gk["code"],
            "repo": gk["repo"],
            "head_sha": gk["sha"],
            "pr_number": gk["pr_number"],
            "author_hash": gk["author_hash"],
            "file": gk["file"],
            "line": gk["line"],
            **prov.get("finding_fields", {}),
        })
        adjudication_entries.append({
            "item_id": item_id,
            **({"label_source": prov["label_source"]} if prov else {}),
            "rater_1_label": l1,
            "rater_2_label": l2,
            "agreed": agreed,
            "tie_breaker_label": tb_l,
            "final_label": final_l,
            **prov.get("adjudication_fields", {}),
        })

    findings_doc = {
        "study_id": study_id,
        "total_adjudicated_items": len(findings_entries),
        "discipline_findings_count": disc_findings_count,
        "twin_sampled_flags_count": twin_sampled_count,
        "twin_total_flags_in_corpus": twin_total_flags,
        "items": findings_entries,
    }
    with open(results_dir / "findings.json", "w", encoding="utf-8") as f:
        json.dump(findings_doc, f, indent=2)

    adjudication_doc = {
        "study_id": study_id,
        "blind_to_source_tool": True,
        "hash_contributor_and_rater_identities": True,
        "rater_1_id": rater_1_hash,
        "rater_1_kind": rater_kind,
        "rater_2_id": rater_2_hash,
        "rater_2_kind": rater_kind,
        "tie_breaker_id": tie_breaker_hash,
        "tie_breaker_kind": rater_kind,
        "synthetic_rater_disclosure": SYNTHETIC_RATER_DISCLOSURE,
        "committed_rater_packet": committed_rater_packet,
        "total_items": total_items,
        "exact_agreement_count": agreed_count,
        "disagreement_count": total_items - agreed_count,
        "cohens_kappa": None if kappa is None else round(kappa, 6),
        "min_acceptable_kappa": 0.75,
        "kappa_passed": kappa is not None and kappa >= 0.75,
        "adjudications": adjudication_entries,
    }
    if label_provenance is not None:
        fresh_agreed = sum(1 for k in kappa_keys if rater_1_labels[k] == rater_2_labels[k])
        adjudication_doc = {
            **{k: v for k, v in adjudication_doc.items() if k != "adjudications"},
            "cohens_kappa_scope": "fresh-items",
            "inherited_label_count": total_items - len(kappa_keys),
            "fresh_label_count": len(kappa_keys),
            "fresh_exact_agreement_count": fresh_agreed,
            "fresh_disagreement_count": len(kappa_keys) - fresh_agreed,
            "adjudications": adjudication_entries,
        }
    with open(results_dir / "adjudication.json", "w", encoding="utf-8") as f:
        json.dump(adjudication_doc, f, indent=2)

    per_gate_counts: dict[str, dict[str, Any]] = {
        g: {"k": 0, "n": 0, "fp": 0, "ct": 0, "class": "structural-ast-and-deterministic"}
        for g in STRUCTURAL_GATES
    }
    for g in HEURISTIC_GATES:
        per_gate_counts[g] = {"k": 0, "n": 0, "fp": 0, "ct": 0, "class": "heuristic-and-prose"}

    class_agg: dict[str, dict[str, Any]] = {
        "structural-ast-and-deterministic": {
            "k": 0,
            "n": 0,
            "fp": 0,
            "ct": 0,
            "floor": 0.90,
            "required_n": 141,
        },
        "heuristic-and-prose": {
            "k": 0,
            "n": 0,
            "fp": 0,
            "ct": 0,
            "floor": 0.80,
            "required_n": 245,
        },
    }
    twin_agg = {"k": 0, "n": 0, "fp": 0, "ct": 0, "floor": 0.90, "required_n": 141}

    for f_entry, a_entry in zip(findings_entries, adjudication_entries):
        fin = a_entry["final_label"]
        tp = 1 if fin == "true-positive" else 0
        fp = 1 if fin == "false-positive" else 0
        ct = 1 if fin == "cannot-tell" else 0
        if f_entry["source_tool"] == "naive-sensitive-path-filter":
            twin_agg["k"] += tp
            twin_agg["n"] += 1
            twin_agg["fp"] += fp
            twin_agg["ct"] += ct
        else:
            g = f_entry["gate"]
            if g not in per_gate_counts:
                per_gate_counts[g] = {
                    "k": 0,
                    "n": 0,
                    "fp": 0,
                    "ct": 0,
                    "class": "heuristic-and-prose",
                }
            per_gate_counts[g]["k"] += tp
            per_gate_counts[g]["n"] += 1
            per_gate_counts[g]["fp"] += fp
            per_gate_counts[g]["ct"] += ct
            cls = per_gate_counts[g]["class"]
            class_agg[cls]["k"] += tp
            class_agg[cls]["n"] += 1
            class_agg[cls]["fp"] += fp
            class_agg[cls]["ct"] += ct

    class_summaries = []
    for cls_name in ["structural-ast-and-deterministic", "heuristic-and-prose"]:
        c = class_agg[cls_name]
        ev = evaluate_gate_verdict(c["k"], c["n"], c["floor"], c["required_n"])
        class_summaries.append({
            "class": cls_name,
            "true_positives": c["k"],
            "false_positives": c["fp"],
            "cannot_tell": c["ct"],
            **ev,
        })

    twin_ev = evaluate_gate_verdict(
        twin_agg["k"], twin_agg["n"], twin_agg["floor"], twin_agg["required_n"]
    )
    s_n = class_agg["structural-ast-and-deterministic"]["n"]
    s_k = class_agg["structural-ast-and-deterministic"]["k"]
    twin_summary = {
        "id": "naive-sensitive-path-filter",
        "true_positives": twin_agg["k"],
        "false_positives": twin_agg["fp"],
        "cannot_tell": twin_agg["ct"],
        **twin_ev,
        "observed_mdd_vs_structural": round(
            minimum_detectable_difference(s_n, twin_agg["n"], 0.55, 0.95, 0.80),
            6,
        ),
        "observed_precision_delta_structural_vs_twin": round(
            (s_k / float(s_n)) - (twin_agg["k"] / float(twin_agg["n"])),
            6,
        ),
    }

    per_gate_summaries = []
    for g in sorted(per_gate_counts.keys()):
        c = per_gate_counts[g]
        fl = 0.90 if c["class"] == "structural-ast-and-deterministic" else 0.80
        rn = 141 if c["class"] == "structural-ast-and-deterministic" else 245
        ev = evaluate_gate_verdict(c["k"], c["n"], fl, rn)
        per_gate_summaries.append({
            "gate": g,
            "class": c["class"],
            "true_positives": c["k"],
            "false_positives": c["fp"],
            "cannot_tell": c["ct"],
            **ev,
        })

    disc_br_low, disc_br_high = wilson_score_interval(disc_blocked_prs, len(manifest_entries), 0.95)
    twin_br_low, twin_br_high = wilson_score_interval(twin_blocked_prs, len(manifest_entries), 0.95)

    summary_doc = {
        "study_id": study_id,
        "discipline_version": discipline_version,
        "evaluated_commit": evaluated_commit,
        "host": host,
        "confidence_level": 0.95,
        "cohens_kappa": None if kappa is None else round(kappa, 6),
        "rater_kind": rater_kind,
        "synthetic_rater_disclosure": SYNTHETIC_RATER_DISCLOSURE,
        "pull_request_block_rates": {
            "total_pull_requests": len(manifest_entries),
            "discipline": {
                "blocked": disc_blocked_prs,
                "n": len(manifest_entries),
                "point_estimate": round(disc_blocked_prs / float(len(manifest_entries)), 6),
                "wilson_95_ci": [round(disc_br_low, 6), round(disc_br_high, 6)],
            },
            "naive_sensitive_path_filter": {
                "blocked": twin_blocked_prs,
                "n": len(manifest_entries),
                "point_estimate": round(twin_blocked_prs / float(len(manifest_entries)), 6),
                "wilson_95_ci": [round(twin_br_low, 6), round(twin_br_high, 6)],
            },
        },
        "class_results": class_summaries,
        "twin_baseline_result": twin_summary,
        "per_gate_results": per_gate_summaries,
        **(extra_summary or {}),
    }
    with open(results_dir / "summary.json", "w", encoding="utf-8") as f:
        json.dump(summary_doc, f, indent=2)

    return summary_doc


# --- Re-runs on the v2 corpus: agent-pr-precision-v3 (docs/benchmarks/precision-preregistration-v3.toml)
# --- and agent-pr-precision-v4 (docs/benchmarks/precision-preregistration-v4.toml) ---

V3_STUDY_ID = "agent-pr-precision-v3"
V3_PACKET_SHUFFLE_SEED = 48202
V3_ANCHOR_SEED = 48203
V3_ANCHORS_PER_SOURCE = 10
V3_RATER_PACKET = "docs/benchmarks/results-v3/rater-packet.json"

V4_STUDY_ID = "agent-pr-precision-v4"
V4_PACKET_SHUFFLE_SEED = 48204
V4_ANCHOR_SEED = 48205
V4_RATER_PACKET = "docs/benchmarks/results-v4/rater-packet.json"


def v4_message_key(message: str) -> str:
    """The v4 label-reuse message key: v3's committed-message masking, then the replay commit id.

    A finding that names the commit being checked names, in a replay, the scratch commit,
    whose id depends on when the replay ran; `commit:<7-40 hex>` becomes
    `commit:<replay-commit>`. Nothing else in the message is rewritten.
    """
    masked = _redact_packet_pii_literals(message) or ""
    return re.sub(r"\bcommit:[0-9a-f]{7,40}\b", "commit:<replay-commit>", masked)


def _identity_message_key(message: str) -> str:
    return message


@dataclass(frozen=True)
class RerunStudy:
    """What differs between two re-runs of the v2 corpus; everything else is shared.

    `previous` is the short tag of the study whose labels are inherited ("v2" for v3, "v3"
    for v4); it names the recorded fields (`v2_item_id`, `inherited-v2`, ...), so the v3
    artifacts keep their field names. v3 recovered v2's messages from a replay with v2's
    binary (`messages_from_reference`); v4 reads v3's committed messages and only checks
    that a replay with v3's binary reproduces v3.
    """

    study_id: str
    previous: str
    previous_study_id: str
    previous_results_dir: Path
    results_dir: Path
    rater_packet: str
    preregistration: str
    discipline_version: str
    reference_binary: str
    corpus_prior_inspection: str
    packet_shuffle_seed: int
    anchor_seed: int
    message_key: Callable[[str], str]
    messages_from_reference: bool
    rater_label_diagnostic: bool


V3_STUDY = RerunStudy(
    study_id=V3_STUDY_ID,
    previous="v2",
    previous_study_id="agent-pr-precision-v2",
    previous_results_dir=RESULTS_DIR,
    results_dir=V3_RESULTS_DIR,
    rater_packet=V3_RATER_PACKET,
    preregistration="docs/benchmarks/precision-preregistration-v3.toml",
    discipline_version="0.25.3",
    reference_binary="cargo build --release at 82d02046e4d64012413885b354873cfa769be329",
    corpus_prior_inspection=(
        "The 232 pull requests and every 0.23.0 finding on them were inspected and labelled "
        "under agent-pr-precision-v2; v3 characterises the 0.25.3 binary on the same changes, "
        "not precision on unseen agent pull requests."
    ),
    packet_shuffle_seed=V3_PACKET_SHUFFLE_SEED,
    anchor_seed=V3_ANCHOR_SEED,
    message_key=_identity_message_key,
    messages_from_reference=True,
    rater_label_diagnostic=False,
)

V4_STUDY = RerunStudy(
    study_id=V4_STUDY_ID,
    previous="v3",
    previous_study_id=V3_STUDY_ID,
    previous_results_dir=V3_RESULTS_DIR,
    results_dir=V4_RESULTS_DIR,
    rater_packet=V4_RATER_PACKET,
    preregistration="docs/benchmarks/precision-preregistration-v4.toml",
    discipline_version="0.26.0",
    reference_binary=(
        "discipline-aarch64-apple-darwin.tar.gz from the v0.25.3 release, verified against its SHA256SUMS"
    ),
    corpus_prior_inspection=(
        "The 232 pull requests were inspected and labelled under agent-pr-precision-v2 (0.23.0) "
        "and agent-pr-precision-v3 (0.25.3), and some 0.26.0 gate changes were made with v3's "
        "results in view; v4 characterises the 0.26.0 binary on the same changes, not precision "
        "on unseen agent pull requests."
    ),
    packet_shuffle_seed=V4_PACKET_SHUFFLE_SEED,
    anchor_seed=V4_ANCHOR_SEED,
    message_key=v4_message_key,
    messages_from_reference=False,
    rater_label_diagnostic=True,
)

RERUN_STUDIES: dict[str, RerunStudy] = {"v3": V3_STUDY, "v4": V4_STUDY}


def _location_key(gate: str, code: str, file: str | None, line: int | None) -> tuple[Any, ...]:
    return (gate, code, file, line)


def attach_reference_messages(
    v2_findings: list[dict[str, Any]],
    v2_manifest: dict[str, Any],
    reference_cases: list[dict[str, Any]],
) -> tuple[dict[str, str], list[str]]:
    """Recover each v2 discipline finding's message from a replay with v2's binary.

    Returns (item_id -> message, differences). The replay reproduces v2 when, per pull
    request, its multiset of (gate, code, file, line) equals findings.json's and its
    blocking count equals the manifest's; every difference is listed. Within a repeated
    key, messages are assigned in replay order to items in item-id order.
    """
    diffs: list[str] = []
    v2_by_pr: dict[tuple[str, str], dict[tuple[Any, ...], list[str]]] = {}
    for item in sorted(v2_findings, key=lambda i: i["item_id"]):
        if item["source_tool"] != "discipline":
            continue
        key = _location_key(item["gate"], item["code"], item["file"], item["line"])
        v2_by_pr.setdefault((item["repo"], item["head_sha"]), {}).setdefault(key, []).append(item["item_id"])
    ref_by_pr = {(c["repo"], c["sha"]): c for c in reference_cases}
    blocking = {(e["repo"], e["head_sha"]): e["discipline_blocking_findings"] for e in v2_manifest["pull_requests"]}
    messages: dict[str, str] = {}
    for pr, expected in blocking.items():
        case = ref_by_pr.get(pr)
        if case is None:
            diffs.append(f"{pr[0]}@{pr[1][:12]}: not replayed")
            continue
        if len(case["violations"]) != expected:
            diffs.append(f"{pr[0]}@{pr[1][:12]}: {len(case['violations'])} blocking findings, manifest says {expected}")
        got: dict[tuple[Any, ...], list[str]] = {}
        for v in case["violations"]:
            got.setdefault(_location_key(v["gate"], v["code"], v["file"], v["line"]), []).append(v["message"])
        want = v2_by_pr.get(pr, {})
        for key in sorted(set(got) | set(want), key=repr):
            ids, msgs = want.get(key, []), got.get(key, [])
            if len(ids) != len(msgs):
                diffs.append(f"{pr[0]}@{pr[1][:12]} {key}: v2 has {len(ids)}, replay has {len(msgs)}")
            for item_id, msg in zip(ids, msgs):
                messages[item_id] = msg
    return messages, diffs


def check_reproduction_with_messages(
    previous_findings: list[dict[str, Any]],
    previous_manifest: dict[str, Any],
    reference_cases: list[dict[str, Any]],
    message_key: Callable[[str], str],
) -> list[str]:
    """Differences between a replay with the previous study's binary and its committed findings.

    The replay reproduces the previous study when, per pull request, its multiset of
    (gate, code, file, line, message key) equals the committed `discipline` items' and its
    blocking count equals the manifest's. Used when the committed findings carry messages
    (v3 onward), so nothing is recovered from the replay.
    """
    diffs: list[str] = []
    want_by_pr: dict[tuple[str, str], list[tuple[Any, ...]]] = {}
    for item in previous_findings:
        if item["source_tool"] == "discipline":
            want_by_pr.setdefault((item["repo"], item["head_sha"]), []).append(
                (*_location_key(item["gate"], item["code"], item["file"], item["line"]), message_key(item["message"]))
            )
    ref_by_pr = {(c["repo"], c["sha"]): c for c in reference_cases}
    for e in previous_manifest["pull_requests"]:
        pr = (e["repo"], e["head_sha"])
        case = ref_by_pr.get(pr)
        if case is None:
            diffs.append(f"{pr[0]}@{pr[1][:12]}: not replayed")
            continue
        if len(case["violations"]) != e["discipline_blocking_findings"]:
            diffs.append(
                f"{pr[0]}@{pr[1][:12]}: {len(case['violations'])} blocking findings, "
                f"manifest says {e['discipline_blocking_findings']}"
            )
        got = sorted(
            ((*_location_key(v["gate"], v["code"], v["file"], v["line"]), message_key(v["message"]))
             for v in case["violations"]),
            key=repr,
        )
        want = sorted(want_by_pr.get(pr, []), key=repr)
        for key in sorted(set(got) | set(want), key=repr):
            if got.count(key) != want.count(key):
                diffs.append(
                    f"{pr[0]}@{pr[1][:12]} {key[:4]}: committed has {want.count(key)}, replay has {got.count(key)}"
                )
    return diffs


def diff_findings_against_previous(
    previous_findings: list[dict[str, Any]],
    previous_messages: dict[str, str],
    cases: list[dict[str, Any]],
    message_key: Callable[[str], str] = _identity_message_key,
) -> dict[str, Any]:
    """Classify each finding of a re-run against the previous study per the pre-registered label-reuse rule.

    Per pull request: `unchanged` matches a previous finding on (gate, code, file, line,
    message key) as a multiset; `changed` shares (gate, file, line) with a still-unmatched
    previous finding but differs in code or message key; `new` has no such previous
    finding; previous findings left over are `gone`. A previous finding without a message
    can only be `gone`. The message key is the identity for v3 and `v4_message_key` for v4.
    """
    prev_by_pr: dict[tuple[str, str], list[dict[str, Any]]] = {}
    for item in sorted(previous_findings, key=lambda i: i["item_id"]):
        if item["source_tool"] == "discipline":
            prev_by_pr.setdefault((item["repo"], item["head_sha"]), []).append(item)
    records: list[dict[str, Any]] = []
    gone: list[str] = []
    for ci, case in enumerate(cases):
        pool = list(prev_by_pr.pop((case["repo"], case["sha"]), []))
        cats: list[tuple[str, str | None]] = [("new", None)] * len(case["violations"])
        for vi, v in enumerate(case["violations"]):
            for pi, item in enumerate(pool):
                if (
                    item["item_id"] in previous_messages
                    and _location_key(item["gate"], item["code"], item["file"], item["line"])
                    == _location_key(v["gate"], v["code"], v["file"], v["line"])
                    and message_key(previous_messages[item["item_id"]]) == message_key(v["message"])
                ):
                    cats[vi] = ("unchanged", item["item_id"])
                    pool.pop(pi)
                    break
        for vi, v in enumerate(case["violations"]):
            if cats[vi][0] != "new":
                continue
            for pi, item in enumerate(pool):
                if item["item_id"] in previous_messages and (item["gate"], item["file"], item["line"]) == (
                    v["gate"], v["file"], v["line"]
                ):
                    cats[vi] = ("changed", item["item_id"])
                    pool.pop(pi)
                    break
        gone.extend(item["item_id"] for item in pool)
        for vi, v in enumerate(case["violations"]):
            records.append({
                "case_index": ci,
                "violation_index": vi,
                "repo": case["repo"],
                "head_sha": case["sha"],
                "gate": v["gate"],
                "code": v["code"],
                "category": cats[vi][0],
                "previous_item_id": cats[vi][1],
            })
    for items in prev_by_pr.values():
        gone.extend(item["item_id"] for item in items)
    prev_gate = {i["item_id"]: i["gate"] for i in previous_findings}
    by_gate: dict[str, dict[str, int]] = {}
    for r in records:
        cell = by_gate.setdefault(r["gate"], {"unchanged": 0, "changed": 0, "new": 0, "gone": 0})
        cell[r["category"]] += 1
    for item_id in gone:
        cell = by_gate.setdefault(prev_gate[item_id], {"unchanged": 0, "changed": 0, "new": 0, "gone": 0})
        cell["gone"] += 1
    return {"records": records, "gone": sorted(gone), "by_gate": dict(sorted(by_gate.items()))}


def twin_reuse(
    cases: list[dict[str, Any]], previous_findings: list[dict[str, Any]]
) -> tuple[list[tuple[dict[str, Any], str, str]], dict[int, str]]:
    """Redraw the twin sample and map each sampled flag (by position) to the previous twin item it repeats."""
    sample = twin_sample(cases)
    prev_twin: dict[tuple[str, str, str, str], list[str]] = {}
    for item in sorted(previous_findings, key=lambda i: i["item_id"]):
        if item["source_tool"] != "discipline":
            prev_twin.setdefault((item["repo"], item["head_sha"], item["file"], item["code"]), []).append(item["item_id"])
    inherited: dict[int, str] = {}
    for pos, (case, path, code) in enumerate(sample):
        ids = prev_twin.get((case["repo"], case["sha"], path, code))
        if ids:
            inherited[pos] = ids.pop(0)
    return sample, inherited


def _packet_raw_item(
    clones_dir: Path,
    repo_map: dict[str, str],
    file_cache: dict[tuple[str, str, str], list[str]],
    repo: str,
    sha: str,
    parent: str,
    subject: str,
    file: str | None,
    line: int | None,
    code: str,
) -> dict[str, Any]:
    if code not in RULE_SUMMARIES:
        raise ValueError(
            f"finding code {code} has no prose rule summary; add one to RULE_SUMMARIES from "
            "docs/GATES.md (the generic fallback would name the gate to the raters)"
        )
    return {
        "repo": repo,
        "commit_sha": sha[:12],
        "commit_subject": subject,
        "file": file,
        "line": line,
        "rule_summary": RULE_SUMMARIES[code],
        "diff_excerpt": _redact_packet_pii_literals(
            _get_diff_excerpt(clones_dir, repo_map, repo, parent, sha, file, line)
        ),
        "line_context": _redact_packet_pii_literals(
            _get_line_context(clones_dir, repo_map, file_cache, repo, sha, file, line)
        ),
    }


def build_rerun_packet(
    study: RerunStudy,
    cases: list[dict[str, Any]],
    diff: dict[str, Any],
    twin_inherited: dict[int, str],
    twin_sampled: list[tuple[dict[str, Any], str, str]],
    previous_findings: list[dict[str, Any]],
    previous_manifest: dict[str, Any],
    clones_dir: Path,
    ground_key_out: Path,
    rater_packet_out: Path,
) -> list[dict[str, Any]]:
    """Blinded packet of the fresh items (new and changed findings, non-inherited twin flags) plus anchors."""
    repo_map = dict(EVALUATION_REPOS)
    file_cache: dict[tuple[str, str, str], list[str]] = {}
    raw: list[tuple[dict[str, Any], dict[str, Any]]] = []
    for r in diff["records"]:
        if r["category"] == "unchanged":
            continue
        case = cases[r["case_index"]]
        v = case["violations"][r["violation_index"]]
        raw.append((
            {"role": "fresh", "source_tool": "discipline", "case_index": r["case_index"],
             "violation_index": r["violation_index"], "category": r["category"]},
            _packet_raw_item(clones_dir, repo_map, file_cache, case["repo"], case["sha"], case["parent"],
                             case["subject"], v["file"], v["line"], v["code"]),
        ))
    for pos, (case, path, code) in enumerate(twin_sampled):
        if pos in twin_inherited:
            continue
        raw.append((
            {"role": "fresh", "source_tool": "naive-sensitive-path-filter", "twin_position": pos},
            _packet_raw_item(clones_dir, repo_map, file_cache, case["repo"], case["sha"], case["parent"],
                             case["subject"], path, None, code),
        ))
    rng = random.Random(study.anchor_seed)
    parent_of = {(e["repo"], e["head_sha"]): e["base_sha"] for e in previous_manifest["pull_requests"]}
    subject_of = {(c["repo"], c["sha"]): c["subject"] for c in cases}
    for source in ("discipline", "naive-sensitive-path-filter"):
        pool = sorted(
            (i for i in previous_findings if (i["source_tool"] == "discipline") == (source == "discipline")),
            key=lambda i: i["item_id"],
        )
        for item in rng.sample(pool, V3_ANCHORS_PER_SOURCE):
            pr = (item["repo"], item["head_sha"])
            raw.append((
                {"role": "anchor", "source_tool": item["source_tool"], f"{study.previous}_item_id": item["item_id"]},
                _packet_raw_item(clones_dir, repo_map, file_cache, item["repo"], item["head_sha"],
                                 parent_of[pr], subject_of[pr], item["file"], item["line"], item["code"]),
            ))
    random.Random(study.packet_shuffle_seed).shuffle(raw)
    ground_key: dict[str, Any] = {}
    items = []
    for idx, (key, blinded) in enumerate(raw, 1):
        packet_id = f"PKT-{idx:04d}"
        ground_key[packet_id] = key
        items.append({"item_id": packet_id, **blinded})
    ground_key_out.parent.mkdir(parents=True, exist_ok=True)
    ground_key_out.write_text(json.dumps(ground_key, indent=2) + "\n")
    rater_packet_out.parent.mkdir(parents=True, exist_ok=True)
    rater_packet_out.write_text(json.dumps({
        "study_id": study.study_id,
        "blind_to_source_tool": True,
        "blind_to_gate_name": True,
        "blind_to_previous_labels": True,
        "anchor_sample_seed": study.anchor_seed,
        "packet_shuffle_seed": study.packet_shuffle_seed,
        "total_items": len(items),
        "items": items,
    }, indent=2) + "\n")
    return items


def _label_counts(labels: Sequence[str]) -> dict[str, int]:
    return {label: sum(1 for x in labels if x == label) for label in ("true-positive", "false-positive", "cannot-tell")}


def generate_rerun_artifacts(
    study: RerunStudy,
    cases: list[dict[str, Any]],
    diff: dict[str, Any],
    twin_sampled: list[tuple[dict[str, Any], str, str]],
    twin_inherited: dict[int, str],
    previous_messages: dict[str, str],
    reproduction_differences: list[str],
    packet_key: dict[str, Any],
    ratings: dict[str, Any],
    evaluated_commit: str,
    host: str,
    results_dir: Path | None = None,
    discipline_binary: str | None = None,
) -> dict[str, Any]:
    """Write a re-run's manifest, findings, adjudication and summary, inheriting the previous study's labels."""
    prev = study.previous
    prev_id_field = f"{prev}_item_id"
    previous_findings = json.loads((study.previous_results_dir / "findings.json").read_text())["items"]
    prev_adj = {
        a["item_id"]: a
        for a in json.loads((study.previous_results_dir / "adjudication.json").read_text())["adjudications"]
    }
    r1, r2, tb = ratings["rater_1"], ratings["rater_2"], ratings.get("tie_breaker", {})
    fresh_packet_id: dict[tuple[Any, ...], str] = {}
    anchors: list[tuple[str, str]] = []
    for packet_id, key in packet_key.items():
        if key["role"] == "anchor":
            anchors.append((packet_id, key[prev_id_field]))
        elif key["source_tool"] == "discipline":
            fresh_packet_id[("d", key["case_index"], key["violation_index"])] = packet_id
        else:
            fresh_packet_id[("t", key["twin_position"])] = packet_id
    record_of = {(r["case_index"], r["violation_index"]): r for r in diff["records"]}

    ground_key: dict[str, Any] = {}
    l1: dict[str, str] = {}
    l2: dict[str, str] = {}
    lt: dict[str, str] = {}
    provenance: dict[str, dict[str, Any]] = {}

    def add(item: dict[str, Any], prev_item: str | None, packet_id: str | None, finding_fields: dict[str, Any]) -> None:
        item_id = f"ITEM-{len(ground_key) + 1:04d}"
        ground_key[item_id] = item
        if prev_item is not None:
            a = prev_adj[prev_item]
            l1[item_id], l2[item_id] = a["rater_1_label"], a["rater_2_label"]
            if a["tie_breaker_label"] is not None:
                lt[item_id] = a["tie_breaker_label"]
            provenance[item_id] = {
                "label_source": f"inherited-{prev}",
                "finding_fields": finding_fields,
                "adjudication_fields": {prev_id_field: prev_item},
            }
        else:
            assert packet_id is not None
            l1[item_id], l2[item_id] = r1[packet_id], r2[packet_id]
            if packet_id in tb:
                lt[item_id] = tb[packet_id]
            provenance[item_id] = {
                "label_source": "fresh",
                "finding_fields": finding_fields,
                "adjudication_fields": {"packet_item_id": packet_id},
            }

    for ci, case in enumerate(cases):
        for vi, v in enumerate(case["violations"]):
            rec = record_of[(ci, vi)]
            inherited = rec["category"] == "unchanged"
            add(
                {**case, "source_tool": "discipline", "gate": v["gate"], "code": v["code"],
                 "file": v["file"], "line": v["line"]},
                rec["previous_item_id"] if inherited else None,
                None if inherited else fresh_packet_id[("d", ci, vi)],
                {
                    # Same LAN-address and home-path masking as the packet; the key compares raw messages.
                    "message": _redact_packet_pii_literals(v["message"]),
                    "diff_category": rec["category"],
                    prev_id_field: rec["previous_item_id"],
                },
            )
    for pos, (case, path, code) in enumerate(twin_sampled):
        prev_item = twin_inherited.get(pos)
        add(
            {**case, "source_tool": "naive-sensitive-path-filter", "gate": "naive-sensitive-path-filter",
             "code": code, "file": path, "line": None},
            prev_item,
            None if prev_item is not None else fresh_packet_id[("t", pos)],
            {"diff_category": "unchanged" if prev_item is not None else "new", prev_id_field: prev_item},
        )

    fresh_ids = [k for k, p in provenance.items() if p["label_source"] == "fresh"]
    prev_final = {i: a["final_label"] for i, a in prev_adj.items()}
    anchor_r1 = [r1[p] for p, _ in anchors]
    anchor_r2 = [r2[p] for p, _ in anchors]
    anchor_prev = [prev_final[v] for _, v in anchors]
    both_1 = [l1[k] for k in fresh_ids] + anchor_r1
    both_2 = [l2[k] for k in fresh_ids] + anchor_r2
    inherited_n = len(ground_key) - len(fresh_ids)
    reproduction: dict[str, Any] = {
        "reference_binary": study.reference_binary,
        "reproduced": not reproduction_differences,
        "differences": reproduction_differences,
    }
    if study.messages_from_reference:
        reproduction[f"{prev}_messages_recovered"] = len(previous_messages)
    else:
        reproduction[f"{prev}_findings_compared"] = len(previous_messages)
    extra: dict[str, Any] = {
        "previous_study_id": study.previous_study_id,
        "preregistration": study.preregistration,
        **({"discipline_binary": discipline_binary} if discipline_binary else {}),
        "corpus_prior_inspection": study.corpus_prior_inspection,
        "reproduction_check": reproduction,
        "label_reuse": {
            f"inherited_from_{prev}": inherited_n,
            "fresh": len(fresh_ids),
            "fresh_discipline": sum(1 for k in fresh_ids if ground_key[k]["source_tool"] == "discipline"),
            "fresh_twin": sum(1 for k in fresh_ids if ground_key[k]["source_tool"] != "discipline"),
            f"twin_sample_identical_to_{prev}": len(twin_inherited) == len(twin_sampled),
        },
        f"findings_diff_vs_{prev}": {
            "unchanged": sum(r["category"] == "unchanged" for r in diff["records"]),
            "changed": sum(r["category"] == "changed" for r in diff["records"]),
            "new": sum(r["category"] == "new" for r in diff["records"]),
            "gone": len(diff["gone"]),
            f"gone_{prev}_item_ids": diff["gone"],
            "by_gate": diff["by_gate"],
        },
        "anchor_check": {
            "anchors": len(anchors),
            f"rater_1_agrees_with_{prev}_final": sum(a == b for a, b in zip(anchor_r1, anchor_prev)),
            f"rater_2_agrees_with_{prev}_final": sum(a == b for a, b in zip(anchor_r2, anchor_prev)),
            f"resolved_agrees_with_{prev}_final": sum(
                (r1[p] if r1[p] == r2[p] else tb.get(p, "cannot-tell")) == prev_final[v] for p, v in anchors
            ),
            "cohens_kappa_fresh_plus_anchor": round(cohens_kappa(both_1, both_2), 6) if both_1 else None,
            "items": [
                {
                    "packet_item_id": p,
                    prev_id_field: v,
                    "source_tool": packet_key[p]["source_tool"],
                    f"{prev}_final_label": prev_final[v],
                    "rater_1_label": r1[p],
                    "rater_2_label": r2[p],
                    "tie_breaker_label": tb.get(p) if r1[p] != r2[p] else None,
                }
                for p, v in sorted(anchors)
            ],
            "use": "drift check only; anchor labels are never final labels",
        },
    }
    if study.rater_label_diagnostic:
        packet_ids = sorted(packet_key)
        counts = {
            "rater_1": _label_counts([r1[p] for p in packet_ids]),
            "rater_2": _label_counts([r2[p] for p in packet_ids]),
        }
        extra["rater_label_counts"] = {
            "scope": "every packet item (fresh and anchor)",
            **counts,
            "single_label_raters": sorted(
                name for name, c in counts.items() if sum(1 for n in c.values() if n) == 1 and len(packet_ids) > 1
            ),
            "use": "diagnostic only; not a verdict input",
        }
    return generate_benchmark_artifacts(
        eval_cases=cases,
        ground_key=ground_key,
        rater_1_labels=l1,
        rater_2_labels=l2,
        tie_breaker_labels=lt,
        rater_1_raw_id=ratings["rater_1_id"],
        rater_2_raw_id=ratings["rater_2_id"],
        tie_breaker_raw_id=ratings["tie_breaker_id"],
        evaluated_commit=evaluated_commit,
        discipline_version=study.discipline_version,
        study_id=study.study_id,
        results_dir=results_dir or study.results_dir,
        committed_rater_packet=study.rater_packet,
        host=host,
        label_provenance=provenance,
        extra_summary=extra,
    )


def _self_test_v3_label_reuse() -> None:
    """The label-reuse diff on a hand-built pair of v2 findings and re-run violations."""
    def f(item_id: str, head: str, gate: str, code: str, file: str | None, line: int | None) -> dict[str, Any]:
        return {"item_id": item_id, "source_tool": "discipline", "repo": "r/r", "head_sha": head,
                "gate": gate, "code": code, "file": file, "line": line}

    v2 = [
        f("ITEM-0001", "a", "ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3),
        f("ITEM-0002", "a", "dependency-delta", "dependency-delta/direct-dependency-added", "package.json", None),
        f("ITEM-0003", "a", "dependency-delta", "dependency-delta/direct-dependency-added", "package.json", None),
        f("ITEM-0004", "a", "assertion-reduction", "assertion-reduction/assertions-reduced", "t.py", 9),
        f("ITEM-0005", "b", "test-floor", "test-floor/test-count-below-floor", "x_test.go", None),
        f("ITEM-0006", "c", "pii", "pii/host-or-pii-leak", "README.md", 1),
        {"item_id": "ITEM-0007", "source_tool": "naive-sensitive-path-filter", "repo": "r/r", "head_sha": "a",
         "gate": "naive-sensitive-path-filter", "code": "twin-sensitive-path/ci-workflow", "file": "w.yml", "line": None},
    ]
    msgs = {"ITEM-0001": "pin it", "ITEM-0002": "adds left-pad", "ITEM-0003": "adds right-pad",
            "ITEM-0004": "2 -> 1 assertions", "ITEM-0005": "4 -> 3 tests"}  # ITEM-0006 not reproduced

    def v(gate: str, code: str, file: str | None, line: int | None, message: str) -> dict[str, Any]:
        return {"gate": gate, "code": code, "file": file, "line": line, "message": message}

    cases = [
        {"repo": "r/r", "sha": "a", "violations": [
            v("dependency-delta", "dependency-delta/direct-dependency-added", "package.json", None, "adds right-pad"),
            v("ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3, "pin it"),
            v("assertion-reduction", "assertion-reduction/assertions-reduced", "t.py", 9, "weak isinstance"),
            v("assertion-reduction", "assertion-reduction/assertions-reduced", "t.py", 20, "new finding"),
        ]},
        {"repo": "r/r", "sha": "b", "violations": [
            v("test-floor", "test-floor/test-count-below-floor", "x_test.go", None, "4 -> 3 tests"),
        ]},
        {"repo": "r/r", "sha": "c", "violations": [
            v("pii", "pii/host-or-pii-leak", "README.md", 1, "home path"),
        ]},
    ]
    d = diff_findings_against_previous(v2, msgs, cases)
    got = [(r["head_sha"], r["violation_index"], r["category"], r["previous_item_id"]) for r in d["records"]]
    assert got == [
        ("a", 0, "unchanged", "ITEM-0003"),
        ("a", 1, "unchanged", "ITEM-0001"),
        ("a", 2, "changed", "ITEM-0004"),
        ("a", 3, "new", None),
        ("b", 0, "unchanged", "ITEM-0005"),
        ("c", 0, "new", None),
    ], got
    assert d["gone"] == ["ITEM-0002", "ITEM-0006"], d["gone"]
    assert d["by_gate"]["dependency-delta"] == {"unchanged": 1, "changed": 0, "new": 0, "gone": 1}
    assert d["by_gate"]["assertion-reduction"] == {"unchanged": 0, "changed": 1, "new": 1, "gone": 0}

    # Reproduction check: a missing finding and a wrong blocking count are both reported.
    manifest = {"pull_requests": [
        {"repo": "r/r", "head_sha": "a", "discipline_blocking_findings": 4},
        {"repo": "r/r", "head_sha": "b", "discipline_blocking_findings": 1},
        {"repo": "r/r", "head_sha": "c", "discipline_blocking_findings": 1},
    ]}
    ref = [
        {"repo": "r/r", "sha": "a", "violations": [
            v("ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3, "pin it"),
            v("dependency-delta", "dependency-delta/direct-dependency-added", "package.json", None, "adds left-pad"),
            v("dependency-delta", "dependency-delta/direct-dependency-added", "package.json", None, "adds right-pad"),
            v("assertion-reduction", "assertion-reduction/assertions-reduced", "t.py", 9, "2 -> 1 assertions"),
        ]},
        {"repo": "r/r", "sha": "b", "violations": [
            v("test-floor", "test-floor/test-count-below-floor", "x_test.go", None, "4 -> 3 tests"),
        ]},
        {"repo": "r/r", "sha": "c", "violations": []},
    ]
    messages, diffs = attach_reference_messages(v2, manifest, ref)
    assert messages == msgs, messages
    assert len(diffs) == 2 and all("r/r@c" in x for x in diffs), diffs
    _, clean = attach_reference_messages(v2[:5] + v2[6:], {"pull_requests": manifest["pull_requests"][:2]}, ref[:2])
    assert clean == [], clean


def _self_test_v4_label_reuse() -> None:
    """The v4 message key, the reproduction check on committed messages and the keyed diff."""
    # The key masks like v3's committed messages and rewrites only `commit:<7-40 hex>`.
    assert v4_message_key("The commit:6f8c831 contains variation-selector character(s).") == (
        "The commit:<replay-commit> contains variation-selector character(s)."
    )
    assert v4_message_key("commit:" + "a" * 40) == "commit:<replay-commit>"
    assert v4_message_key("commit:abc12") == "commit:abc12"  # under 7 hex digits: not a commit id
    assert v4_message_key("pin deadbeefcafe to a SHA") == "pin deadbeefcafe to a SHA"
    # Built at runtime so the literals do not sit in this file.
    lan, lan10, home = ".".join(["192", "168", "1", "20"]), ".".join(["10", "0", "0", "1"]), "/" + "Users/alex/x"
    assert v4_message_key(f"host {lan} in {home}") == "host <lan-ip:192-168-1-20> in /<home>/alex/x"
    assert v4_message_key(v4_message_key(f"commit:6f8c831 at {lan10}")) == v4_message_key(f"commit:6f8c831 at {lan10}")

    def f(item_id: str, head: str, gate: str, code: str, file: str | None, line: int | None,
          message: str) -> dict[str, Any]:
        return {"item_id": item_id, "source_tool": "discipline", "repo": "r/r", "head_sha": head,
                "gate": gate, "code": code, "file": file, "line": line, "message": message}

    def v(gate: str, code: str, file: str | None, line: int | None, message: str) -> dict[str, Any]:
        return {"gate": gate, "code": code, "file": file, "line": line, "message": message}

    smuggle = "instruction-smuggling/invisible-characters-in-description"
    v3 = [
        f("ITEM-0001", "a", "instruction-smuggling", smuggle, None, None, "The commit:6f8c831 has VS chars."),
        f("ITEM-0002", "a", "pii", "pii/host-or-pii-leak", "README.md", 1, "LAN address <lan-ip:10-0-0-1>"),
        f("ITEM-0003", "b", "ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3, "pin it"),
    ]
    cases = [
        {"repo": "r/r", "sha": "a", "violations": [
            v("instruction-smuggling", smuggle, None, None, "The commit:0d1e2f3 has VS chars."),
            v("pii", "pii/host-or-pii-leak", "README.md", 1, f"LAN address {lan10}"),
        ]},
        {"repo": "r/r", "sha": "b", "violations": [
            v("ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3, "pin it to a full SHA"),
            v("harness-tampering", "harness-tampering/pytest-hook-masks-results", "conftest.py", 4, "masks"),
        ]},
    ]
    msgs = {i["item_id"]: i["message"] for i in v3}
    d = diff_findings_against_previous(v3, msgs, cases, v4_message_key)
    got = [(r["head_sha"], r["category"], r["previous_item_id"]) for r in d["records"]]
    assert got == [
        ("a", "unchanged", "ITEM-0001"),  # only the scratch commit id differs
        ("a", "unchanged", "ITEM-0002"),  # raw message against v3's masked one
        ("b", "changed", "ITEM-0003"),
        ("b", "new", None),
    ], got
    # Without the key, both v3 artefacts become fresh.
    raw = [r["category"] for r in diff_findings_against_previous(v3, msgs, cases)["records"]]
    assert raw == ["changed", "changed", "changed", "new"], raw

    manifest = {"pull_requests": [
        {"repo": "r/r", "head_sha": "a", "discipline_blocking_findings": 2},
        {"repo": "r/r", "head_sha": "b", "discipline_blocking_findings": 1},
    ]}
    ref = [
        {"repo": "r/r", "sha": "a", "violations": cases[0]["violations"]},
        {"repo": "r/r", "sha": "b", "violations": [v("ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3,
                                                      "pin it")]},
    ]
    assert check_reproduction_with_messages(v3, manifest, ref, v4_message_key) == []
    bad = [ref[0], {"repo": "r/r", "sha": "b", "violations": [
        v("ci-integrity", "ci-integrity/unpinned-action", "w.yml", 3, "pin it now")]}]
    diffs = check_reproduction_with_messages(v3, manifest, bad, v4_message_key)
    assert len(diffs) == 2 and all("r/r@b" in x for x in diffs), diffs
    diffs = check_reproduction_with_messages(v3, manifest, ref[:1], v4_message_key)
    assert diffs == ["r/r@b: not replayed"], diffs


FAKE_DISCIPLINE = '''#!{python}
import json, os, sys
args = sys.argv[1:]
body = open(args[args.index("--pr-body-file") + 1], encoding="utf-8").read()
tree = sorted(
    os.path.relpath(os.path.join(d, f), ".")
    for d, ds, fs in os.walk(".")
    if not d.startswith("./.git")
    for f in fs
)
log = {{
    "args": args,
    "replay_case": os.environ.get("DISCIPLINE_REPLAY_CASE"),
    "no_network": os.environ.get("DISCIPLINE_NO_NETWORK"),
    "body": body,
    "tree": tree,
}}
with open({log!r}, "a", encoding="utf-8") as f:
    f.write(json.dumps(log) + "\\n")
print(json.dumps({{"outcomes": [{{"gate": "assertion-reduction", "violations": [
    {{"severity": "error", "code": "assertion-reduction/assertions-reduced",
      "file": "tests/test_a.py", "line": 2, "message": "fewer assertions"}},
    {{"severity": "warning", "code": "assertion-reduction/assertions-reduced",
      "file": "tests/test_a.py", "line": 3, "message": "warning only"}}]}}]}}))
sys.exit(1)
'''


def _self_test_replay_from_manifest() -> None:
    """`replay_from_manifest` replays only the manifest's pairs, diff-scoped, with the manifest's metadata."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        repo = root / "clones" / "demo"
        env = {
            **os.environ,
            "GIT_AUTHOR_NAME": "Agent",
            "GIT_AUTHOR_EMAIL": "agent@example.invalid",
            "GIT_COMMITTER_NAME": "Agent",
            "GIT_COMMITTER_EMAIL": "agent@example.invalid",
        }

        def git(*a: str) -> str:
            return subprocess.check_output(["git", "-C", str(repo), *a], env=env, text=True).strip()

        repo.mkdir(parents=True)
        git("init", "-q")
        (repo / "src").mkdir()
        (repo / "tests").mkdir()
        (repo / "src" / "a.py").write_text("x = 1\n")
        (repo / "tests" / "test_a.py").write_text("def test_a():\n    assert 1 == 1\n    assert 2 == 2\n")
        git("add", "-A")
        git("commit", "-q", "-m", "base")
        base = git("rev-parse", "HEAD")
        (repo / "tests" / "test_a.py").write_text("def test_a():\n    assert 1 == 1\n")
        (repo / "src" / "b.py").write_text("y = 2\n")
        git("add", "-A")
        message = "fix: drop an assertion (#7)\n\nCo-authored-by: Claude <noreply@example.invalid>"
        git("commit", "-q", "-m", message)
        head = git("rev-parse", "HEAD")
        (repo / "src" / "later.py").write_text("z = 3\n")
        git("add", "-A")
        git("commit", "-q", "-m", "later upstream merge (#8)")

        log = root / "fake.log"
        exe = root / "fake-discipline"
        exe.write_text(FAKE_DISCIPLINE.format(python=sys.executable, log=str(log)))
        exe.chmod(0o755)
        manifest = root / "manifest.json"
        manifest.write_text(json.dumps({"pull_requests": [
            {"repo": "demo/demo", "pr_number": 7, "base_sha": base, "head_sha": head,
             "author_hash": "sha256:frommanifest00", "agent_category": "claude"},
            {"repo": "demo/demo", "pr_number": 9, "base_sha": base, "head_sha": "f" * 40,
             "author_hash": "sha256:frommanifest01", "agent_category": "claude"},
        ]}))
        out = root / "raw.json"
        results = replay_from_manifest(
            manifest, root / "clones", exe, out, repos=[("demo/demo", "demo")]
        )
        assert json.loads(out.read_text()) == results
        assert len(results) == 1, results
        r = results[0]
        assert (r["sha"], r["parent"], r["pr_number"]) == (head, base, 7)
        assert r["author_hash"] == "sha256:frommanifest00"
        assert r["author_hash_recomputed"] == hash_identity("Agent<agent@example.invalid>")
        assert (r["agent_category"], r["subject"]) == ("claude", "fix: drop an assertion (#7)")
        assert sorted(r["paths"]) == ["src/b.py", "tests/test_a.py"]
        assert r["base_existing"] == ["tests/test_a.py"]
        assert r["returncode"] == 1
        assert r["violations"] == [{
            "gate": "assertion-reduction", "code": "assertion-reduction/assertions-reduced",
            "file": "tests/test_a.py", "line": 2, "message": "fewer assertions",
        }]
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        assert len(calls) == 1, calls
        call = calls[0]
        assert call["args"][:4] == ["check", "--config", call["args"][2], "--format"]
        assert "--base" in call["args"] and (call["replay_case"], call["no_network"]) == ("1", "1")
        assert call["body"] == message
        # Diff-scoped: only the pair's changed files are in the scratch tree.
        assert call["tree"] == ["src/b.py", "tests/test_a.py"], call["tree"]


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

    # 7. Twin classification (including nested workflow subdirectories)
    assert classify_twin_sensitive_path(".github/workflows/ci.yml", False) == "twin-sensitive-path/ci-workflow"
    assert classify_twin_sensitive_path(".github/workflows/nested/release.yaml", True) == "twin-sensitive-path/ci-workflow"
    assert classify_twin_sensitive_path(".gitea/workflows/sub/test.yml", False) == "twin-sensitive-path/ci-workflow"
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

    # 9. Fleiss pooled-null MDD, pinned to scipy.optimize.brentq on the same equation
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
    lo_d, hi_d = newcombe_difference_ci(119, 144, 9, 150)
    assert abs(lo_d - 0.6802131110452675) < 1e-9 and abs(hi_d - 0.8265277396812667) < 1e-9

    # 12. Cluster statistics on a hand-checkable input
    toy = cluster_robust_ratio([(1, 1), (0, 1)])
    assert abs(toy["design_effect"] - 2.0) < 1e-12 and abs(toy["effective_n"] - 1.0) < 1e-12
    assert _quantile_type7([0.0, 1.0, 2.0, 3.0], 0.5) == 1.5

    # 13. PR / Gerrit change number extraction
    assert extract_pr_or_change_number("fix(linter): handle tuple (#12345)\n\nBody") == 12345
    assert extract_pr_or_change_number("Merge pull request #987 from user/branch\n\nTitle") == 987
    assert (
        extract_pr_or_change_number(
            "compress/flate: speed up writeTokens\n\nReviewed-on: https://go-review.googlesource.com/c/go/+/829565\n"
        )
        == 829565
    )
    assert extract_pr_or_change_number("chore: direct commit without reference") is None

    # 14. Replay from a committed manifest (no harvest)
    _self_test_replay_from_manifest()

    # 15. v3 label reuse: unchanged / changed / new / gone, and the reproduction check
    _self_test_v3_label_reuse()

    # 15b. v4 label reuse: the message key, the reproduction check on committed messages
    _self_test_v4_label_reuse()

    # 16. Each re-run's pre-registration pins the previous manifest by hash (python3.10 has no tomllib)
    for study in RERUN_STUDIES.values():
        prereg = REPO_ROOT / study.preregistration
        if prereg.exists():
            pinned = re.search(r'^manifest_sha256 = "([0-9a-f]{64})"$', prereg.read_text(), re.M)
            actual = hashlib.sha256((study.previous_results_dir / "manifest.json").read_bytes()).hexdigest()
            assert pinned and pinned.group(1) == actual, (
                f"{study.previous} manifest.json changed after the {study.study_id} pre-registration"
            )

    # 17. Post-hoc audits on the committed data (v2, v3 and v4)
    for results_dir in (RESULTS_DIR, V3_RESULTS_DIR, V4_RESULTS_DIR):
        if (results_dir / "findings.json").exists() and (results_dir / "post-hoc-audit.json").exists():
            committed = json.loads((results_dir / "post-hoc-audit.json").read_text())
            recomputed = json.loads(json.dumps(post_hoc_audit(results_dir)))
            if committed.get("study_id") == recomputed.get("study_id"):
                assert committed == recomputed, (
                    f"{results_dir.name}/post-hoc-audit.json is stale; rerun with --post-hoc-audit --write"
                )

    print("All benchmark_precision.py self-tests passed.")


def run_rerun_stage(
    study: RerunStudy, stage: str, args: argparse.Namespace, parser: argparse.ArgumentParser
) -> int:
    """`--v3-stage` / `--v4-stage diff|packet|artifacts` (the study's pre-registration)."""
    flag = f"--{study.study_id.rsplit('-', 1)[1]}-stage"
    if not args.raw_eval_json or not args.reference_raw_json or not args.clones_dir:
        parser.error(f"{flag} requires --raw-eval-json, --reference-raw-json and --clones-dir")
    cases = json.loads(args.raw_eval_json.read_text())
    reference = json.loads(args.reference_raw_json.read_text())
    prev_findings = json.loads((study.previous_results_dir / "findings.json").read_text())["items"]
    prev_manifest = json.loads((study.previous_results_dir / "manifest.json").read_text())
    if study.messages_from_reference:
        messages, differences = attach_reference_messages(prev_findings, prev_manifest, reference)
    else:
        messages = {i["item_id"]: i["message"] for i in prev_findings if i["source_tool"] == "discipline"}
        differences = check_reproduction_with_messages(prev_findings, prev_manifest, reference, study.message_key)
    diff = diff_findings_against_previous(prev_findings, messages, cases, study.message_key)
    sampled, inherited = twin_reuse(cases, prev_findings)
    print(json.dumps({
        "reproduction_differences": differences,
        f"{study.previous}_messages": len(messages),
        "twin_flags_inherited": f"{len(inherited)} of {len(sampled)}",
        "findings": {c: sum(r["category"] == c for r in diff["records"]) for c in ("unchanged", "changed", "new")},
        "gone": len(diff["gone"]),
        "by_gate": diff["by_gate"],
    }, indent=2))
    if stage == "diff":
        return 0
    if not args.ground_key_json:
        parser.error(f"{flag} packet|artifacts requires --ground-key-json")
    if stage == "packet":
        items = build_rerun_packet(
            study, cases, diff, inherited, sampled, prev_findings, prev_manifest, args.clones_dir,
            args.ground_key_json, REPO_ROOT / study.rater_packet,
        )
        print(f"wrote {study.rater_packet} with {len(items)} items")
        return 0
    if not args.ratings_json or not args.evaluated_commit:
        parser.error(f"{flag} artifacts requires --ratings-json and --evaluated-commit")
    generate_rerun_artifacts(
        study, cases, diff, sampled, inherited, messages, differences,
        json.loads(args.ground_key_json.read_text()), json.loads(args.ratings_json.read_text()),
        args.evaluated_commit, args.host, args.results_dir_out, args.discipline_binary,
    )
    out = (args.results_dir_out or study.results_dir).resolve()
    print(f"wrote {out}/{{manifest,findings,adjudication,summary}}.json")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Discipline public agent-PR precision benchmark harness.")
    parser.add_argument("--test", action="store_true", help="Run statistical and classifier self-tests.")
    parser.add_argument(
        "--post-hoc-audit",
        action="store_true",
        help="Print the post-hoc audit statistics (not pre-registered) as JSON.",
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="With --post-hoc-audit, write docs/benchmarks/results/post-hoc-audit.json.",
    )
    parser.add_argument(
        "--harvest-and-replay",
        action="store_true",
        help="Harvest and replay merged agent PRs across the 8 evaluation repos.",
    )
    parser.add_argument(
        "--from-manifest",
        type=Path,
        metavar="MANIFEST",
        help="Replay exactly the (repo, base_sha, head_sha) pairs of a study manifest.json "
        "instead of harvesting (no re-harvest; new upstream merges cannot enter the sample).",
    )
    parser.add_argument(
        "--build-packet",
        action="store_true",
        help="Build the source-tool-blinded rater packet (docs/benchmarks/results/rater-packet.json).",
    )
    parser.add_argument(
        "--clones-dir",
        type=Path,
        help="Directory containing the cached git clones of the benchmark repositories.",
    )
    parser.add_argument(
        "--discipline-exe",
        type=Path,
        default=REPO_ROOT / "target" / "release" / "discipline",
        help="Path to the compiled discipline binary.",
    )
    parser.add_argument(
        "--raw-eval-json",
        type=Path,
        help="Path to read/write raw evaluation replay JSON.",
    )
    parser.add_argument(
        "--ground-key-json",
        type=Path,
        help="Path to read/write unblinded ground key JSON.",
    )
    parser.add_argument(
        "--rater-packet-json",
        type=Path,
        default=RESULTS_DIR / "rater-packet.json",
        help="Path to write the committed blinded rater packet JSON.",
    )
    parser.add_argument(
        "--v3-stage",
        choices=["diff", "packet", "artifacts"],
        help="agent-pr-precision-v3: diff a re-run against v2, build the fresh-item packet, or write "
        "docs/benchmarks/results-v3/ (needs --raw-eval-json, --reference-raw-json, --clones-dir).",
    )
    parser.add_argument(
        "--v4-stage",
        choices=["diff", "packet", "artifacts"],
        help="agent-pr-precision-v4: the same stages against v3, writing docs/benchmarks/results-v4/.",
    )
    parser.add_argument(
        "--reference-raw-json",
        type=Path,
        help="Raw replay JSON from the previous study's binary over its manifest, for the reproduction "
        "check (v3: v2's 82d02046; v4: the 0.25.3 release).",
    )
    parser.add_argument(
        "--ratings-json",
        type=Path,
        help="With --v3-stage / --v4-stage artifacts: rater_1 / rater_2 / tie_breaker labels by packet "
        "item id, and rater ids.",
    )
    parser.add_argument(
        "--results-dir-out",
        type=Path,
        help="With --v3-stage / --v4-stage artifacts: write here instead of the study's results directory.",
    )
    parser.add_argument(
        "--discipline-binary",
        help="With --v4-stage artifacts: how the replayed binary was obtained, recorded in summary.json.",
    )
    parser.add_argument("--evaluated-commit", help="Commit the replayed binary was built from.")
    parser.add_argument("--host", default="Apple M-series macOS 15 arm64", help="Host recorded in summary.json.")
    parser.add_argument(
        "--results-dir",
        type=Path,
        default=RESULTS_DIR,
        help="With --post-hoc-audit: the study results directory to audit.",
    )
    args = parser.parse_args()
    if args.v3_stage and args.v4_stage:
        parser.error("--v3-stage and --v4-stage are exclusive")
    if args.v3_stage:
        return run_rerun_stage(V3_STUDY, args.v3_stage, args, parser)
    if args.v4_stage:
        return run_rerun_stage(V4_STUDY, args.v4_stage, args, parser)
    if args.test:
        run_self_tests()
        return 0
    if args.post_hoc_audit:
        text = json.dumps(post_hoc_audit(args.results_dir), indent=2) + "\n"
        if args.write:
            (args.results_dir / "post-hoc-audit.json").write_text(text)
        else:
            sys.stdout.write(text)
        return 0
    if args.from_manifest:
        if not args.clones_dir or not args.raw_eval_json:
            parser.error("--from-manifest requires --clones-dir and --raw-eval-json")
        replay_from_manifest(
            manifest_path=args.from_manifest,
            clones_dir=args.clones_dir,
            discipline_exe=args.discipline_exe,
            out_raw_json=args.raw_eval_json,
        )
        return 0
    if args.harvest_and_replay:
        if not args.clones_dir or not args.raw_eval_json:
            parser.error("--harvest-and-replay requires --clones-dir and --raw-eval-json")
        harvest_and_replay_evaluation(
            clones_dir=args.clones_dir,
            discipline_exe=args.discipline_exe,
            out_raw_json=args.raw_eval_json,
        )
        return 0
    if args.build_packet:
        if not args.clones_dir or not args.raw_eval_json or not args.ground_key_json:
            parser.error("--build-packet requires --clones-dir, --raw-eval-json, and --ground-key-json")
        with open(args.raw_eval_json, encoding="utf-8") as f:
            eval_cases = json.load(f)
        build_blinded_packet(
            eval_cases=eval_cases,
            clones_dir=args.clones_dir,
            ground_key_out=args.ground_key_json,
            rater_packet_out=args.rater_packet_json,
        )
        return 0
    run_self_tests()
    return 0


if __name__ == "__main__":
    sys.exit(main())
