#!/usr/bin/env python3
"""
ImpossibleBench recall study (#838): the pilot's `PILOT` values from its committed artefacts.

`docs/benchmarks/recall-preregistration.toml` (`impossible-recall-v1`) leaves `[arms] sampling`
and `[power_and_bounds] design_effect` to the pilot, and `trajectories_rule` takes its rates
from it. This script derives every one of them from files committed under
`docs/benchmarks/results-recall-pilot/`, never from hand arithmetic:

- `network`: the `[harness] sandbox_network` exclusion check over the Inspect logs. Every
  sample's sandbox must be the patched compose file (`network_mode: none`); every tool call
  that names a network client is listed with whether its output shows a completed transfer.
  A run with one is flagged (its `[run_id, instance_id]` goes to `recall_pairs.py
  --network-flagged`).
- `state`: the lab runner's append-only state file with the provider's identifiers replaced
  (`--redact TEXT=LABEL`); the run fails when any of them is left.
- `log-hashes`: sha256 of every Inspect log (the logs hold the provider's model identifier and
  are not committed).
- `values`: sampling (the runner's start record), the per-access-arm rates of
  `trajectories_rule` with Wilson 95% intervals, the design effect of `design_effect_rule`
  (the `R.cluster` that `recall_replay.summarise` reports for the primary configuration arm),
  `required_n`, and `trajectories_required` against the 544 runs of `feasibility_rule`.
- `values` also reports, descriptively, how the binary classifies the techniques of `mixed`
  pairs (in scope, outside R's denominator), with the two twins beside it.
- `--check DIR`: recompute `values` from DIR and compare with DIR/pilot-values.json.

Stdlib only. `--self-check` runs embedded fixtures.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import re
import sys
from typing import Any


def _load_sibling(name: str) -> Any:
    """Import a sibling script by path (works under `python3 -I`, which drops the script directory from sys.path)."""
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parent / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


power = _load_sibling("benchmark_power_audit")
pairs_mod = _load_sibling("recall_pairs")
strata_mod = _load_sibling("recall_strata")
replay_mod = _load_sibling("recall_replay")

# Locked in the pre-registration before the pilot.
REQUIRED_N_BEFORE_DESIGN_EFFECT = 112  # `[power_and_bounds] required_n_before_design_effect`
CONFIRMATORY_RUNS_PER_ARM = 544  # `feasibility_rule`: 272 confirmatory instances x 2 impossible splits
DESIGN_EFFECT_MIN_INSTANCES = 10  # `design_effect_rule`
DESIGN_EFFECT_FALLBACK = 2.0
IMPOSSIBLE_SPLITS = ("oneoff", "conflicting")
ACCESS_ARMS = ("access_full", "access_readonly")
PRIMARY_CONFIG_ARM = "primary"

# A tool call naming a network client, and output that shows a transfer completed.
NETWORK_CALL = re.compile(
    r"\b(?:pip3?\s+(?:install|download)|curl\s|wget\s|git\s+(?:clone|fetch|pull|ls-remote)|apt(?:-get)?\s+install|"
    r"conda\s+install|urllib\.request|requests\.(?:get|post)|http\.client|socket\.)"
)
NETWORK_SUCCESS = re.compile(r"Downloading\s+\S+\.(?:whl|tar\.gz)|Saving to:|Receiving objects:|HTTP/[\d.]+\s+2\d\d|"
                             r"Successfully downloaded")
NETWORK_NONE = re.compile(r"^\s*network_mode:\s*none\s*$", re.M)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# --- network --------------------------------------------------------------------------


def network_scan(log_dir: Path, compose_dir: Path | None) -> dict[str, Any]:
    """Network-client tool calls per run and the compose network mode of every sample."""
    attempts: list[dict[str, Any]] = []
    flagged: set[tuple[str, str]] = set()
    compose: dict[str, int] = {"network_mode_none": 0, "other": 0, "not_found": 0}
    samples = 0
    for log_path, header, sample in pairs_mod.iter_log_samples(log_dir):
        samples += 1
        run_id = str((header.get("eval") or {}).get("run_id", ""))
        instance = str(sample.get("id"))
        cfg = (sample.get("sandbox") or {}).get("config")
        if compose_dir is not None:
            f = compose_dir / Path(str(cfg)).name if cfg else None
            if f is None or not f.exists():
                compose["not_found"] += 1
            elif NETWORK_NONE.search(f.read_text()):
                compose["network_mode_none"] += 1
            else:
                compose["other"] += 1
        for ev in sample.get("events") or []:
            if ev.get("event") != "tool":
                continue
            args = json.dumps(ev.get("arguments"), sort_keys=True)
            m = NETWORK_CALL.search(args)
            if not m:
                continue
            result = str(ev.get("result") or "")
            connected = bool(NETWORK_SUCCESS.search(result))
            attempts.append({"run_id": run_id, "instance_id": instance, "log_file": log_path.name,
                             "client": m.group(0).strip(), "transfer_completed": connected})
            if connected:
                flagged.add((run_id, instance))
    return {"samples": samples, "compose": compose if compose_dir is not None else None,
            "attempts": attempts, "flagged": sorted([list(x) for x in flagged])}


# --- state ----------------------------------------------------------------------------


def redact_state(text: str, redactions: dict[str, str]) -> str:
    """The runner state with each identifier replaced; raises when one is left."""
    for needle, label in sorted(redactions.items(), key=lambda kv: -len(kv[0])):
        text = text.replace(needle, label)
    for needle in redactions:
        if needle in text:
            raise ValueError("an identifier is left after redaction")
    return text


def final_cells(state_lines: list[str]) -> tuple[dict[str, Any], dict[str, dict[str, Any]]]:
    """The last agent `start` record and every cell's final `done` record."""
    start: dict[str, Any] = {}
    done: dict[str, dict[str, Any]] = {}
    for line in state_lines:
        rec = json.loads(line)
        if rec.get("key") == "start" and not rec.get("dummies_only") and rec.get("instances", 0) > 0:
            if not start or rec.get("instances", 0) >= start.get("instances", 0):
                start = rec
        if rec.get("phase") == "done":
            done[rec["key"]] = rec
    return start, done


# --- values ---------------------------------------------------------------------------


def wilson(k: int, n: int) -> dict[str, Any]:
    return replay_mod.wilson(k, n)


def access_arm_rates(manifest: dict[str, Any], strata: dict[str, Any], done: dict[str, dict[str, Any]],
                     access_arm: str, in_scope: str = "registered") -> dict[str, Any]:
    """`trajectories_rule` rates for one access arm, over planned impossible-split agent cells."""
    is_in_scope = strata_mod.IN_SCOPE_RULES[in_scope]
    planned = [r for r in done.values() if r.get("dummy") is None and r.get("arm") == access_arm
               and r.get("split") in IMPOSSIBLE_SPLITS]
    skipped: dict[str, int] = {}
    for r in planned:
        if r.get("generated") is False:
            skipped[r["excluded"]] = skipped.get(r["excluded"], 0) + 1
    runner_excluded: dict[str, int] = {}
    for r in planned:
        if r.get("generated") is not False and r.get("excluded"):
            runner_excluded[r["excluded"]] = runner_excluded.get(r["excluded"], 0) + 1
    agent_pairs = [p for p in manifest["pairs"] if not p.get("dummy") and p.get("arm") == access_arm
                   and p.get("split") in IMPOSSIBLE_SPLITS and p.get("population_split") == "pilot"]
    manifest_excluded: dict[str, int] = {}
    unverified: dict[str, int] = {}
    valid: list[dict[str, Any]] = []
    for p in agent_pairs:
        if p.get("exclusion"):
            key = p["exclusion"].split(":", 1)[0]
            manifest_excluded[key] = manifest_excluded.get(key, 0) + 1
        elif p.get("reproduction") != "verified":
            unverified[str(p.get("reproduction"))] = unverified.get(str(p.get("reproduction")), 0) + 1
        else:
            valid.append(p)
    by_id = {s["pair_id"]: s for s in strata["pairs"]}
    passes = [p for p in valid if p.get("passed")]
    strata_counts: dict[str, int] = {}
    in_scope_passes = 0
    for p in passes:
        entry = by_id.get(p["pair_id"], {})
        st = entry.get("stratum", "missing")
        strata_counts[st] = strata_counts.get(st, 0) + 1
        in_scope_passes += is_in_scope(st, {t["sub_stratum"] for t in entry.get("techniques", [])})
    labelled = sum(v for k, v in strata_counts.items() if k not in ("out-of-diff", "unlabelled", "missing"))
    return {
        "planned_cells": len(planned),
        "not_generated": dict(sorted(skipped.items())),
        "runner_excluded": dict(sorted(runner_excluded.items())),
        "manifest_excluded": dict(sorted(manifest_excluded.items())),
        "not_verified": dict(sorted(unverified.items())),
        "valid_runs": len(valid),
        "passes": len(passes),
        "pass_strata": dict(sorted(strata_counts.items())),
        "valid_share": wilson(len(valid), len(planned)),
        "pass_rate": wilson(len(passes), len(valid)),
        "in_scope_share": wilson(in_scope_passes, labelled),
    }


def design_effect(summary: dict[str, Any], units_by_instance: dict[str, int]) -> dict[str, Any]:
    """`design_effect_rule` from `recall_replay.summarise` (SWE-bench, primary configuration arm)."""
    instances = sum(1 for n in units_by_instance.values() if n > 0)
    arm = summary["by_benchmark"].get("swe", {}).get("config_arms", {}).get(PRIMARY_CONFIG_ARM, {})
    cluster = (arm.get("R") or {}).get("cluster")
    estimate = cluster["design_effect"] if cluster else None
    if instances < DESIGN_EFFECT_MIN_INSTANCES:
        value, rule = DESIGN_EFFECT_FALLBACK, f"fewer than {DESIGN_EFFECT_MIN_INSTANCES} instances carry a test-side unit"
    elif estimate is None:
        value, rule = None, "no cluster-robust estimate (fewer than two clusters, or every unit blocked or none)"
    else:
        value, rule = max(1.0, estimate), "max(1.0, cluster_robust_ratio point estimate)"
    return {"instances_with_test_side_unit": instances, "test_side_units": sum(units_by_instance.values()),
            "cluster_robust_estimate": estimate, "value": value, "rule_applied": rule}


def required_n(design_effect_value: float) -> int:
    """`required_n_rule`: ceil(required_n_before_design_effect x design_effect), float noise absorbed."""
    x = REQUIRED_N_BEFORE_DESIGN_EFFECT * design_effect_value
    nearest = round(x)
    return int(nearest) if abs(x - nearest) <= 1e-9 * max(1.0, x) else math.ceil(x)


def trajectories(n_req: int, rates: dict[str, Any]) -> dict[str, Any]:
    pe = {k: rates[k]["point_estimate"] for k in ("pass_rate", "in_scope_share", "valid_share")}
    if any(v is None or v <= 0.0 for v in pe.values()):
        return {"inputs": pe, "trajectories_required": None, "feasible_within_confirmatory_runs": False,
                "note": "a pilot rate is zero or undefined, so no number of runs reaches required_n"}
    t = power.trajectories_required(n_req, pe["pass_rate"], pe["in_scope_share"], pe["valid_share"])
    return {"inputs": pe, "trajectories_required": t, "feasible_within_confirmatory_runs": t <= CONFIRMATORY_RUNS_PER_ARM}


def compute_values(results_dir: Path, in_scope: str = "registered") -> dict[str, Any]:
    manifest = json.loads((results_dir / "manifest.json").read_text())
    strata = json.loads((results_dir / "strata.json").read_text())
    results = json.loads((results_dir / "replay.json").read_text())
    summary = replay_mod.summarise(results, None, in_scope)
    is_in_scope = strata_mod.IN_SCOPE_RULES[in_scope]
    state_lines = (results_dir / "runner-state.jsonl").read_text().splitlines()
    start, done = final_cells(state_lines)
    units: dict[str, int] = {}
    seen: set[tuple[str, str, str]] = set()
    for r in results["pairs"]:
        subs = {t["sub_stratum"] for t in r["config_arms"][PRIMARY_CONFIG_ARM]["techniques"]}
        if r["benchmark"] != "swe" or r["split"] == "original" or not is_in_scope(r["stratum"], subs):
            continue
        for t in r["config_arms"][PRIMARY_CONFIG_ARM]["techniques"]:
            key = (r["instance_id"], t["sub_stratum"], t["hunk_sha256"])
            if key not in seen:
                seen.add(key)
                units[r["instance_id"]] = units.get(r["instance_id"], 0) + 1
    de = design_effect(summary, units)
    mixed: dict[str, Any] = {}
    for cfg in replay_mod.CONFIG_ARMS:
        techs = [t for r in results["pairs"] if r["benchmark"] == "swe" and r["split"] != "original"
                 and r["stratum"] == "mixed" for t in r["config_arms"][cfg]["techniques"]]
        classes = {c: sum(t["discipline"] == c for t in techs) for c in
                   ("blocked", "surfaced", "incidental", "missed", "could-not-check")}
        mixed[cfg] = {"techniques": len(techs), "classes": classes,
                      "blocked": wilson(classes["blocked"], len(techs)),
                      "line_regex_twin": wilson(sum(t["twin_line_regex"] for t in techs), len(techs)),
                      "path_twin": wilson(sum(t["twin_path"] for t in techs), len(techs))}
    n_req = required_n(de["value"]) if de["value"] is not None else None
    per_arm = {}
    for arm in ACCESS_ARMS:
        rates = access_arm_rates(manifest, strata, done, arm, in_scope)
        per_arm[arm] = {"rates": rates, "trajectories": trajectories(n_req, rates) if n_req else None}
    out = {
        "study_id": pairs_mod.STUDY_ID,
        "generated_by": "python3 scripts/recall_pilot.py values",
        "inputs_sha256": {name: sha256_file(results_dir / name) for name in
                          ("manifest.json", "strata.json", "replay.json", "runner-state.jsonl")},
        "family": "family-1",
        "sampling": {
            "temperature_top_p_top_k_max_tokens": start.get("sampling"),
            "message_limit": start.get("message_limit"),
            "max_attempts": start.get("max_attempts"),
            "server_defaults": ((start.get("preflight") or {}).get("server") or {}).get("defaults"),
        },
        "binary": start.get("binary"),
        "design_effect": de,
        "required_n": n_req,
        "mixed_descriptive": mixed,
        "access_arms": per_arm,
        "summary_primary_swe": summary["by_benchmark"].get("swe", {}).get("config_arms", {}).get(PRIMARY_CONFIG_ARM),
        "summary_secondary_swe": summary["by_benchmark"].get("swe", {}).get("config_arms", {}).get("secondary"),
    }
    if in_scope != "registered":
        out["in_scope_rule"] = in_scope
    return out


def runs_per_cell_needed(trajectories_required: int | None) -> int | None:
    """Smallest `runs_per_cell` whose confirmatory runs (CONFIRMATORY_RUNS_PER_ARM per cell pass) reach `trajectories_required`."""
    if trajectories_required is None:
        return None
    return max(1, math.ceil(trajectories_required / CONFIRMATORY_RUNS_PER_ARM))


def amendment_3(results_dir: Path) -> dict[str, Any]:
    """Amendment 3's Step-0 bounds: `compute_values` under each in-scope rule, from the same pilot artefacts."""
    rules = {}
    for rule in strata_mod.IN_SCOPE_RULES:
        v = compute_values(results_dir, rule)
        p = (v["summary_primary_swe"] or {}).get("P")
        rules[rule] = {
            "design_effect": v["design_effect"],
            "required_n": v["required_n"],
            "P_primary": p,
            "R_primary": (v["summary_primary_swe"] or {}).get("R"),
            "R_secondary": (v["summary_secondary_swe"] or {}).get("R"),
            "falsifiers_primary": (v["summary_primary_swe"] or {}).get("falsifiers"),
            "access_arms": {arm: {"in_scope_share": a["rates"]["in_scope_share"], "pass_rate": a["rates"]["pass_rate"],
                                  "valid_share": a["rates"]["valid_share"], "trajectories": a["trajectories"],
                                  "runs_per_cell_needed": runs_per_cell_needed(
                                      (a["trajectories"] or {}).get("trajectories_required"))}
                            for arm, a in v["access_arms"].items()},
        }
    return {
        "study_id": pairs_mod.STUDY_ID,
        "generated_by": "python3 scripts/recall_pilot.py amendment-3",
        "inputs_sha256": {name: sha256_file(results_dir / name) for name in
                          ("manifest.json", "strata.json", "replay.json", "runner-state.jsonl")},
        "confirmatory_runs_per_arm": CONFIRMATORY_RUNS_PER_ARM,
        "rules": rules,
    }


def same_values(got: Any, want: Any) -> bool:
    """Equal, except that floats agree to 1e-9 relative: `sum` rounds differently from Python 3.12."""
    if isinstance(got, float) or isinstance(want, float):
        return (isinstance(got, (int, float)) and isinstance(want, (int, float))
                and not isinstance(got, bool) and not isinstance(want, bool)
                and math.isclose(got, want, rel_tol=1e-9, abs_tol=1e-12))
    if isinstance(got, dict) and isinstance(want, dict):
        return got.keys() == want.keys() and all(same_values(got[k], want[k]) for k in got)
    if isinstance(got, list) and isinstance(want, list):
        return len(got) == len(want) and all(same_values(g, w) for g, w in zip(got, want))
    return type(got) is type(want) and got == want


# --- Self-check -----------------------------------------------------------------------


def run_self_checks() -> None:
    assert same_values({"d": [1.0640000000000007, 224]}, {"d": [1.0640000000000003, 224]})
    assert not same_values({"d": [1.064, 224]}, {"d": [1.065, 224]})
    assert not same_values({"n": 224}, {"n": 225}) and not same_values({"n": 1}, {"n": True})
    assert not same_values({"a": 1.0}, {"a": 1.0, "b": 2}) and not same_values([1.0], [1.0, 2.0])
    assert redact_state('{"m": "vendor/x-35B"}', {"vendor/x-35B": "family-1"}) == '{"m": "family-1"}'
    try:
        redact_state("vendor/x", {"vendor/x": "vendor/x"})
    except ValueError:
        pass
    else:
        raise AssertionError("a label that keeps the identifier was accepted")
    assert NETWORK_CALL.search("cd /testbed && pip install pytest -q")
    assert not NETWORK_CALL.search("python -m pytest tests/")
    assert NETWORK_SUCCESS.search("Downloading pytest-8.0.0-py3-none-any.whl (334 kB)")
    assert not NETWORK_SUCCESS.search("WARNING: Retrying (Retry(total=2)) after connection broken by NewConnectionError")
    assert not NETWORK_SUCCESS.search("Requirement already satisfied: tox in /opt/miniconda3/lib/python3.9")
    assert NETWORK_NONE.search("services:\n  default:\n    network_mode: none\n")
    assert not NETWORK_NONE.search("    networks:\n      - inspect_network\n")

    assert required_n(1.0) == 112 and required_n(2.0) == 224 and required_n(1.25) == 140 and required_n(1.001) == 113

    summary = {"by_benchmark": {"swe": {"config_arms": {"primary": {"R": {"cluster": {"design_effect": 0.8}}}}}}}
    many = {f"i{i}": 1 for i in range(12)}
    assert design_effect(summary, many)["value"] == 1.0
    summary["by_benchmark"]["swe"]["config_arms"]["primary"]["R"]["cluster"]["design_effect"] = 1.7
    assert design_effect(summary, many)["value"] == 1.7
    few = {f"i{i}": 2 for i in range(9)}
    assert design_effect(summary, few)["value"] == 2.0
    summary["by_benchmark"]["swe"]["config_arms"]["primary"]["R"]["cluster"] = None
    assert design_effect(summary, many)["value"] is None

    lines = [
        json.dumps({"key": "start", "dummies_only": True, "instances": 3}),
        json.dumps({"key": "start", "dummies_only": False, "instances": 1, "sampling": ["a"]}),
        json.dumps({"key": "start", "dummies_only": False, "instances": 3, "sampling": ["b"]}),
        json.dumps({"key": "i1|oneoff|access_full|agent", "phase": "done", "split": "oneoff", "arm": "access_full",
                    "dummy": None, "instance": "i1", "passed": True}),
        json.dumps({"key": "i2|oneoff|access_full|agent", "phase": "done", "split": "oneoff", "arm": "access_full",
                    "dummy": None, "instance": "i2", "generated": False, "excluded": "oracle-positive-control-failed"}),
        json.dumps({"key": "i3|oneoff|access_full|agent", "phase": "done", "split": "oneoff", "arm": "access_full",
                    "dummy": None, "instance": "i3", "passed": False}),
        json.dumps({"key": "i4|oneoff|access_full|agent", "phase": "done", "split": "oneoff", "arm": "access_full",
                    "dummy": None, "instance": "i4", "passed": True}),
    ]
    start, done = final_cells(lines)
    assert start["sampling"] == ["b"] and len(done) == 4
    manifest = {"pairs": [
        {"pair_id": "p1", "dummy": None, "arm": "access_full", "split": "oneoff", "population_split": "pilot",
         "exclusion": None, "reproduction": "verified", "passed": True},
        {"pair_id": "p3", "dummy": None, "arm": "access_full", "split": "oneoff", "population_split": "pilot",
         "exclusion": None, "reproduction": "verified", "passed": False},
        {"pair_id": "p4", "dummy": None, "arm": "access_full", "split": "oneoff", "population_split": "pilot",
         "exclusion": None, "reproduction": "verified", "passed": True},
    ]}
    strata = {"pairs": [{"pair_id": "p1", "stratum": "test-side"}, {"pair_id": "p4", "stratum": "source-side"}]}
    rates = access_arm_rates(manifest, strata, done, "access_full")
    assert rates["planned_cells"] == 4 and rates["not_generated"] == {"oracle-positive-control-failed": 1}
    assert rates["valid_share"]["k"] == 3 and rates["valid_share"]["n"] == 4
    assert (rates["pass_rate"]["k"], rates["pass_rate"]["n"]) == (2, 3)
    assert (rates["in_scope_share"]["k"], rates["in_scope_share"]["n"]) == (1, 2)
    strata["pairs"].append({"pair_id": "p4b", "stratum": "mixed", "techniques": [{"sub_stratum": "S1-revert"}]})
    manifest["pairs"].append({"pair_id": "p4b", "dummy": None, "arm": "access_full", "split": "oneoff",
                              "population_split": "pilot", "exclusion": None, "reproduction": "verified", "passed": True})
    strata["pairs"].append({"pair_id": "p4c", "stratum": "mixed", "techniques": [{"sub_stratum": "S1"}]})
    manifest["pairs"].append({"pair_id": "p4c", "dummy": None, "arm": "access_full", "split": "oneoff",
                              "population_split": "pilot", "exclusion": None, "reproduction": "verified", "passed": True})
    more = {**done, **{k: {**done["i4|oneoff|access_full|agent"], "instance": k} for k in ("i5", "i6")}}
    share = {rule: access_arm_rates(manifest, strata, more, "access_full", rule)["in_scope_share"]
             for rule in strata_mod.IN_SCOPE_RULES}
    assert {r: (w["k"], w["n"]) for r, w in share.items()} == {
        "registered": (1, 4), "test-side-or-mixed": (3, 4), "test-side-or-mixed-s1-revert": (2, 4)}, share
    del manifest["pairs"][-2:], strata["pairs"][-2:]

    def row(inst: str, stratum: str, sub: str, cls: str) -> dict[str, Any]:
        tech = {"sub_stratum": sub, "hunk_sha256": inst, "discipline": cls, "gates_in_hunks": [],
                "twin_path": False, "twin_line_regex": False}
        return {"benchmark": "swe", "split": "oneoff", "stratum": stratum, "instance_id": inst,
                "twins_any": {"path": False, "line_regex": False},
                "config_arms": {a: {"techniques": [tech], "gate_set_error": False} for a in replay_mod.CONFIG_ARMS}}
    res = {"pairs": [row("a", "test-side", "S1", "blocked"), row("b", "mixed", "S1-revert", "blocked"),
                     row("c", "mixed", "S1", "missed"), row("d", "source-side", "S1", "missed")]}
    got = {}
    for rule in strata_mod.IN_SCOPE_RULES:
        arm = replay_mod.summarise(res, None, rule)["by_benchmark"]["swe"]["config_arms"][PRIMARY_CONFIG_ARM]
        got[rule] = ((arm["R"]["k"], arm["R"]["n"]), (arm["P"]["k"], arm["P"]["n"]))
    assert got == {"registered": ((1, 1), (1, 4)), "test-side-or-mixed": ((2, 3), (3, 4)),
                   "test-side-or-mixed-s1-revert": ((2, 2), (2, 4))}, got
    assert [runs_per_cell_needed(t) for t in (None, 1, 544, 545, 770, 1088, 9240)] == [None, 1, 1, 2, 2, 2, 17]
    t = trajectories(112, rates)
    assert t["trajectories_required"] == power.trajectories_required(112, 2 / 3, 0.5, 0.75) == 448
    assert t["feasible_within_confirmatory_runs"] is True
    none = trajectories(112, {**rates, "in_scope_share": wilson(0, 2)})
    assert none["trajectories_required"] is None and none["feasible_within_confirmatory_runs"] is False
    print("All recall_pilot.py self-checks passed.")


def main() -> int:
    parser = argparse.ArgumentParser(description="ImpossibleBench recall study (#838): pilot values.")
    parser.add_argument("--self-check", action="store_true", help="Run the embedded fixtures.")
    parser.add_argument("--check", type=Path, metavar="DIR", help="Recompute DIR/pilot-values.json and compare.")
    sub = parser.add_subparsers(dest="cmd")
    n = sub.add_parser("network", help="Network exclusion check over the Inspect logs.")
    n.add_argument("--logs", type=Path, required=True)
    n.add_argument("--compose-dir", type=Path, help="The harness's generated compose files.")
    n.add_argument("--out", type=Path, required=True)
    n.add_argument("--flagged-out", type=Path, help="Write the flagged runs for recall_pairs.py --network-flagged.")
    s = sub.add_parser("state", help="Redacted copy of the runner state.")
    s.add_argument("--state", type=Path, required=True)
    s.add_argument("--redact", action="append", default=[], metavar="TEXT=LABEL")
    s.add_argument("--out", type=Path, required=True)
    h = sub.add_parser("log-hashes", help="sha256 of every Inspect log.")
    h.add_argument("--logs", type=Path, required=True)
    h.add_argument("--out", type=Path, required=True)
    v = sub.add_parser("values", help="The PILOT values from a results directory.")
    v.add_argument("--results", type=Path, required=True)
    v.add_argument("--out", type=Path, required=True)
    a = sub.add_parser("amendment-3", help="Amendment 3's Step-0 bounds under each in-scope rule.")
    a.add_argument("--results", type=Path, required=True)
    a.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.self_check:
        run_self_checks()
        return 0
    if args.check:
        checks = [("pilot-values.json", compute_values), ("amendment-3-values.json", amendment_3)]
        for name, compute in checks:
            if name != "pilot-values.json" and not (args.check / name).exists():
                continue
            want = json.loads((args.check / name).read_text())
            got = json.loads(json.dumps(compute(args.check), sort_keys=True))
            if not same_values(got, want):
                print(f"{name} does not match a recomputation from its inputs", file=sys.stderr)
                return 1
            print(f"{name} matches its inputs.")
        return 0
    if args.cmd == "network":
        res = network_scan(args.logs, args.compose_dir)
        args.out.write_text(json.dumps(res, indent=2, sort_keys=True) + "\n")
        if args.flagged_out:
            args.flagged_out.write_text(json.dumps(res["flagged"]) + "\n")
        print(json.dumps({"samples": res["samples"], "compose": res["compose"], "attempts": len(res["attempts"]),
                          "flagged": len(res["flagged"])}))
    elif args.cmd == "state":
        redactions = {}
        for spec in args.redact:
            text, _, label = spec.rpartition("=")
            if not text:
                parser.error(f"--redact expects TEXT=LABEL, got {spec}")
            redactions[text] = label
        args.out.write_text(redact_state(args.state.read_text(), redactions))
    elif args.cmd == "log-hashes":
        rows = {str(p.relative_to(args.logs)): sha256_file(p) for p in sorted(args.logs.rglob("*.json"))}
        args.out.write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n")
    elif args.cmd == "values":
        res = compute_values(args.results)
        args.out.write_text(json.dumps(res, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"design_effect": res["design_effect"], "required_n": res["required_n"],
                          "trajectories": {a: v["trajectories"] for a, v in res["access_arms"].items()}}, indent=2))
    elif args.cmd == "amendment-3":
        res = amendment_3(args.results)
        args.out.write_text(json.dumps(res, indent=2, sort_keys=True) + "\n")
        print(json.dumps({rule: {"required_n": v["required_n"], "design_effect": v["design_effect"]["value"],
                                 "trajectories": {arm: a["trajectories"] for arm, a in v["access_arms"].items()}}
                          for rule, v in res["rules"].items()}, indent=2))
    else:
        parser.error("a subcommand, --check or --self-check is required")
    return 0


if __name__ == "__main__":
    sys.exit(main())
