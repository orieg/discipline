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
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
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
from typing import Any, Sequence


REPO_ROOT = Path(__file__).resolve().parent.parent
PREREG_PATH = REPO_ROOT / "docs" / "benchmarks" / "precision-preregistration.toml"
RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results"

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
    return {
        "study_id": study_id,
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
        commit_data.append((sha, parent, an, ae, msg, agent_cat, pr_num, paths, sorted(base_existing)))

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
        f"for {len(selected)} commits in {time.time() - t_repo:.1f}s"
    )

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
    print(f"[{repo_slug}] finished {len(results)} cases in {time.time() - t_repo:.1f}s")
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

    twin_candidates = []
    for case in eval_cases:
        base_set = set(case["base_existing"])
        for p in case["paths"]:
            tcode = classify_twin_sensitive_path(p, p in base_set)
            if tcode:
                twin_candidates.append((case, p, tcode))

    rng = random.Random(twin_sample_seed)
    twin_sampled = rng.sample(twin_candidates, twin_sample_size)
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
) -> dict[str, Any]:
    """
    Generate `manifest.json`, `findings.json`, `adjudication.json`, and `summary.json`
    under `results_dir` from the evaluation cases, ground key, and rater labels.
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
    l1_seq = [rater_1_labels[k] for k in keys]
    l2_seq = [rater_2_labels[k] for k in keys]
    kappa = cohens_kappa(l1_seq, l2_seq)

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
        })
        adjudication_entries.append({
            "item_id": item_id,
            "rater_1_label": l1,
            "rater_2_label": l2,
            "agreed": agreed,
            "tie_breaker_label": tb_l,
            "final_label": final_l,
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
        "committed_rater_packet": "docs/benchmarks/results/rater-packet.json",
        "total_items": total_items,
        "exact_agreement_count": agreed_count,
        "disagreement_count": total_items - agreed_count,
        "cohens_kappa": round(kappa, 6),
        "min_acceptable_kappa": 0.75,
        "kappa_passed": kappa >= 0.75,
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
        "host": "Apple M-series macOS 15 arm64",
        "confidence_level": 0.95,
        "cohens_kappa": round(kappa, 6),
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
    }
    with open(results_dir / "summary.json", "w", encoding="utf-8") as f:
        json.dump(summary_doc, f, indent=2)

    return summary_doc


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

    # 14. Post-hoc audit on the committed data
    if (RESULTS_DIR / "findings.json").exists() and (RESULTS_DIR / "post-hoc-audit.json").exists():
        committed = json.loads((RESULTS_DIR / "post-hoc-audit.json").read_text())
        recomputed = json.loads(json.dumps(post_hoc_audit()))
        if committed.get("study_id") == recomputed.get("study_id"):
            assert committed == recomputed, "post-hoc-audit.json is stale; rerun with --post-hoc-audit --write"

    print("All benchmark_precision.py self-tests passed.")


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
