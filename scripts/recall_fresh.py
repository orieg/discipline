#!/usr/bin/env python3
"""Fresh-instance recall re-measurement (#838): population scan and Step-0 bounds.

Pre-registration: `docs/benchmarks/recall-fresh-preregistration.toml` (status `draft`).

The `impossible-recall-v1` pre-registration (`docs/benchmarks/recall-preregistration.toml`,
`[integrity] regression_loop`) requires a re-measurement after fixes to use fresh, unseen
instances. Its pilot (77) and confirmatory (272) ids are every instance of the pinned
Impossible-SWE-bench dataset, so no fresh instance exists there: fresh ones must be generated
from SWE-bench instances the dataset was not built from. This script holds the steps that run
before any instance is generated or any agent runs:

- `population`: reads SWE-bench's `test` split, SWE-bench Verified and Impossible-SWE-bench at
  pinned revisions (Hugging Face datasets-server, read only), checks that the pinned
  Impossible-SWE-bench ids are exactly the v1 pilot and confirmatory ids, and writes the
  fresh pool (SWE-bench `test` minus Verified) in its pre-registered generation order to
  `docs/benchmarks/results-recall-fresh/population.json`.
- `--bounds-json`: the Step-0 values the pre-registration records, computed by the bound
  functions in `scripts/benchmark_power_audit.py` (mirrored in `src/guards/perf/bounds.rs`).
- `--check-population`: re-runs `check_population` on the committed `population.json`
  against the committed v1 id lists (no network).
- `--test`: self-checks (patch file parsing, the generation order, the disjointness check,
  the Step-0 values pinned).
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import math
from pathlib import Path
import sys
import time
from typing import Any, Iterable
import unittest
import urllib.parse
import urllib.request

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))
# The bound functions; imported after the path insert, so by name.
b = importlib.import_module("benchmark_power_audit")

LAB_DIR = REPO_ROOT / "scripts" / "recall_lab"
RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results-recall-fresh"
STUDY_ID = "impossible-recall-fresh-v1"

# Dataset revisions the scan reads. datasets-server serves the current revision only, so the
# scan reads each dataset's revision before and after its rows and refuses when either is
# not the pinned one.
SWEBENCH = {"dataset": "princeton-nlp/SWE-bench", "revision": "e48e2bd1e9fecd5bbd641e9414ac59da9f2e69f6", "split": "test"}
VERIFIED = {"dataset": "princeton-nlp/SWE-bench_Verified", "revision": "c104f840cc67f8b6eec6f759ebc8b2693d585d4a", "split": "test"}
IMPOSSIBLE = {"dataset": "fjzzq2002/impossible_swebench", "revision": "9c2d34f364b7229e8c0ff807c646100bdc18bbb5"}
IMPOSSIBLE_SPLITS = ("original", "oneoff", "conflicting")

# `secrets.token_hex(16)`, committed with the draft before any instance is generated. The
# generation order is sha256(salt || instance_id), ascending.
GENERATION_SALT = "3aae8647ed567535e520aef34229b4c0"

ROWS_URL = "https://datasets-server.huggingface.co/rows"
INFO_URL = "https://huggingface.co/api/datasets/"
PAGE = 100
USER_AGENT = "recall-fresh-scan/1"
PACE_SECONDS = 1.0  # between requests; datasets-server rate-limits bursts

# ---- Step-0 inputs -----------------------------------------------------------------
#
# Each planning value names where it comes from; none is hand-computed downstream.
FRESH_CONFIDENCE = 0.95
FRESH_POWER = 0.80
FRESH_HALF_WIDTH = 0.10  # target Wilson 95% half-width of R on the well-formed stratum
FRESH_PLANNING_P = 0.5  # worst case for the half-width: no unit-level well-formed rate at the frozen binary is measured yet
FRESH_DESIGN_EFFECT = 1.064  # v1 locked value (pilot); the v1 confirmatory estimate is 1.023
# v1 confirmatory, primary arm: 66 of the 138 in-scope units have a well-formed injected test
# file by the registered `injection_shape` rule (`[strata.v1_units]`, recomputed offline by
# `recall_strata.py --v1-units-injection`). The unit and its stratum do not depend on the
# binary, only on the agent arm, which is unchanged.
V1_WELL_FORMED_UNITS = 66
V1_CONFIRMATORY_INSTANCES = 272
# Instances of the pinned Impossible-SWE-bench (349) over SWE-bench Verified (500): the
# upstream generator's retention after its own checks, the only measured generation yield.
GENERATION_YIELD = (349, 500)
SPLITS_PER_INSTANCE = 2  # oneoff, conflicting
RUNS_PER_CELL = 2  # access_full, as v1 amendment 3
RECALL_FLOOR = 0.80  # v1's floor, kept for continuity
# Planning recall on the well-formed stratum: 18/66 = 0.273 is the measured reused-unit R_wf of
# the 0.32.0 tree on those 66 units (`[generalisation.reused_baseline_committed_reading]`);
# 0.5 is a deliberately generous ceiling. Both give the floor verdict's pass probability.
V1_WELL_FORMED_BLOCKED = 18
FLOOR_PLANNING_RATES = (V1_WELL_FORMED_BLOCKED / V1_WELL_FORMED_UNITS, 0.5)
REUSED_BASELINE_P = V1_WELL_FORMED_BLOCKED / V1_WELL_FORMED_UNITS
FRESH_N_MAX = 2000


def fresh_bound_values() -> dict[str, Any]:
    """The Step-0 values `recall-fresh-preregistration.toml` records."""
    n_wf = b.sample_size_for_interval(FRESH_PLANNING_P, FRESH_HALF_WIDTH, FRESH_CONFIDENCE)
    units = math.ceil(n_wf * FRESH_DESIGN_EFFECT)
    per_instance = V1_WELL_FORMED_UNITS / V1_CONFIRMATORY_INSTANCES
    yield_ = GENERATION_YIELD[0] / GENERATION_YIELD[1]
    # `trajectories_required(n, rate, 1, valid)` = ceil(n / (rate * valid)): here the "runs"
    # are instances, the rate is well-formed in-scope units per validated instance and the
    # valid share is the generation yield.
    validated = b.trajectories_required(units, per_instance, 1.0, 1.0)
    to_generate = b.trajectories_required(units, per_instance, 1.0, yield_)
    return {
        "precision": {
            "planning_p": FRESH_PLANNING_P,
            "half_width": FRESH_HALF_WIDTH,
            "confidence_level": FRESH_CONFIDENCE,
            "n_well_formed_units": n_wf,
            "design_effect": FRESH_DESIGN_EFFECT,
            "required_units": units,
        },
        "yield": {
            "well_formed_units_per_instance": [V1_WELL_FORMED_UNITS, V1_CONFIRMATORY_INSTANCES],
            "generation_yield": list(GENERATION_YIELD),
            "validated_instances_required": validated,
            "instances_to_generate": to_generate,
            "agent_runs": validated * SPLITS_PER_INSTANCE * RUNS_PER_CELL,
        },
        "floor": {
            "recall_floor": RECALL_FLOOR,
            "pass_probability": [
                {"planning_recall": p, "n": units, "value": b.wilson_pass_probability(units, p, RECALL_FLOOR, FRESH_CONFIDENCE)}
                for p in FLOOR_PLANNING_RATES
            ],
            "verdict": "REJECT_INFEASIBLE",
        },
        "generalisation": {
            "reused_units": V1_WELL_FORMED_UNITS,
            "fresh_units": units,
            "baseline_p": REUSED_BASELINE_P,
            "power": FRESH_POWER,
            "minimum_detectable_drop": b.minimum_detectable_difference(
                V1_WELL_FORMED_UNITS, units, REUSED_BASELINE_P, FRESH_CONFIDENCE, FRESH_POWER
            ),
        },
    }


# ---- Population --------------------------------------------------------------------


def files_of_patch(patch: str) -> list[str]:
    """The files a unified diff touches, from its `diff --git a/X b/Y` headers (the `b` side), sorted."""
    files = set()
    for line in patch.splitlines():
        if line.startswith("diff --git "):
            parts = line.split(" ")
            if len(parts) >= 4 and parts[3].startswith("b/"):
                files.add(parts[3][2:])
    return sorted(files)


def order_key(instance_id: str, salt: str = GENERATION_SALT) -> str:
    return hashlib.sha256((salt + instance_id).encode()).hexdigest()


def generation_order(ids: Iterable[str], salt: str = GENERATION_SALT) -> list[str]:
    """Pool ids in the pre-registered generation order: sha256(salt || id) ascending."""
    return sorted(ids, key=lambda i: (order_key(i, salt), i))


def build_population(
    swebench_rows: list[dict[str, Any]],
    verified_ids: set[str],
    impossible_ids: set[str],
    pilot_ids: set[str],
    confirmatory_ids: set[str],
    v1_test_files: dict[str, set[str]],
) -> dict[str, Any]:
    """The fresh pool and the checks that tie it to v1."""
    swe_ids = {r["instance_id"] for r in swebench_rows}
    if len(swe_ids) != len(swebench_rows):
        raise ValueError("duplicate instance ids in the SWE-bench rows")
    v1 = pilot_ids | confirmatory_ids
    pool_ids = swe_ids - verified_ids
    by_id = {r["instance_id"]: r for r in swebench_rows}
    entries = []
    for rank, iid in enumerate(generation_order(pool_ids)):
        row = by_id[iid]
        files = files_of_patch(row["test_patch"])
        shared = sorted(set(files) & v1_test_files.get(row["repo"], set()))
        entries.append({"rank": rank, "instance_id": iid, "repo": row["repo"], "test_files": files, "shares_v1_test_file": bool(shared)})
    per_repo: dict[str, int] = {}
    for e in entries:
        per_repo[e["repo"]] = per_repo.get(e["repo"], 0) + 1
    pop = {
        "study_id": STUDY_ID,
        "generated_by": "python3 scripts/recall_fresh.py population",
        "generation_salt": GENERATION_SALT,
        "sources": {"swebench": SWEBENCH, "verified": VERIFIED, "impossible": IMPOSSIBLE},
        "counts": {
            "swebench_test": len(swe_ids),
            "verified": len(verified_ids),
            "impossible": len(impossible_ids),
            "v1_pilot": len(pilot_ids),
            "v1_confirmatory": len(confirmatory_ids),
            "pool": len(entries),
            "pool_sharing_a_v1_test_file": sum(e["shares_v1_test_file"] for e in entries),
        },
        "checks": {
            "impossible_equals_v1_ids": impossible_ids == v1,
            "v1_pilot_confirmatory_disjoint": not (pilot_ids & confirmatory_ids),
            "verified_within_swebench_test": verified_ids <= swe_ids,
            "pool_disjoint_from_v1": not (pool_ids & v1),
            "pool_disjoint_from_verified": not (pool_ids & verified_ids),
        },
        "pool_per_repo": dict(sorted(per_repo.items())),
        "pool": entries,
    }
    check_population(pop, pilot_ids, confirmatory_ids)
    return pop


def check_population(pop: dict[str, Any], pilot_ids: set[str], confirmatory_ids: set[str]) -> None:
    """Raise ValueError unless the pool is disjoint from v1 and every recorded check holds."""
    failed = [k for k, v in pop["checks"].items() if v is not True]
    if failed:
        raise ValueError(f"population checks failed: {failed}")
    ids = [e["instance_id"] for e in pop["pool"]]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate ids in the pool")
    overlap = set(ids) & (pilot_ids | confirmatory_ids)
    if overlap:
        raise ValueError(f"{len(overlap)} pool ids are v1 instances")
    if ids != generation_order(ids, pop["generation_salt"]):
        raise ValueError("pool is not in generation order")
    if [e["rank"] for e in pop["pool"]] != list(range(len(ids))):
        raise ValueError("pool ranks are not 0..n-1")
    if pop["counts"]["pool"] != len(ids):
        raise ValueError("recorded pool count differs from the pool")


def read_ids(path: Path) -> set[str]:
    return {line.strip() for line in path.read_text().splitlines() if line.strip()}


def _get_json(url: str) -> Any:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    for attempt in range(8):
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                data = json.load(resp)
            time.sleep(PACE_SECONDS)
            return data
        except OSError as e:
            if attempt == 7:
                raise
            retry_after = getattr(e, "headers", None) and e.headers.get("Retry-After")
            time.sleep(float(retry_after) if retry_after and retry_after.isdigit() else min(60, 2 ** (attempt + 2)))
    raise AssertionError("unreachable")


def _revision(dataset: str) -> str:
    return _get_json(INFO_URL + dataset)["sha"]


def fetch_rows(dataset: str, revision: str, split: str, fields: tuple[str, ...]) -> list[dict[str, Any]]:
    """Every row of `split`, keeping `fields`; refuses unless the dataset is at `revision` before and after."""
    if _revision(dataset) != revision:
        raise SystemExit(f"{dataset} is not at the pinned revision {revision}")
    rows: list[dict[str, Any]] = []
    offset = 0
    while True:
        q = urllib.parse.urlencode({"dataset": dataset, "config": "default", "split": split, "offset": offset, "length": PAGE})
        page = _get_json(f"{ROWS_URL}?{q}")
        rows.extend({f: r["row"][f] for f in fields} for r in page["rows"])
        offset += PAGE
        if offset >= page["num_rows_total"]:
            break
    if _revision(dataset) != revision:
        raise SystemExit(f"{dataset} moved off {revision} during the scan")
    return rows


def population(out: Path) -> None:
    swe = fetch_rows(SWEBENCH["dataset"], SWEBENCH["revision"], SWEBENCH["split"], ("instance_id", "repo", "test_patch"))
    ver = fetch_rows(VERIFIED["dataset"], VERIFIED["revision"], VERIFIED["split"], ("instance_id", "repo", "test_patch"))
    imp: set[str] = set()
    for split in IMPOSSIBLE_SPLITS:
        ids = {r["instance_id"] for r in fetch_rows(IMPOSSIBLE["dataset"], IMPOSSIBLE["revision"], split, ("instance_id",))}
        if imp and ids != imp:
            raise SystemExit(f"Impossible-SWE-bench split {split} has different ids")
        imp = ids
    pilot = read_ids(LAB_DIR / "pilot-ids-swe.txt")
    conf = read_ids(LAB_DIR / "confirmatory-ids-swe.txt")
    v1_files: dict[str, set[str]] = {}
    for r in ver:
        if r["instance_id"] in pilot | conf:
            v1_files.setdefault(r["repo"], set()).update(files_of_patch(r["test_patch"]))
    pop = build_population(swe, {r["instance_id"] for r in ver}, imp, pilot, conf, v1_files)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(pop, indent=1) + "\n")
    print(json.dumps(pop["counts"], indent=1))


# ---- Self-checks -------------------------------------------------------------------


class SelfCheck(unittest.TestCase):
    def test_files_of_patch_reads_the_b_side(self) -> None:
        patch = (
            "diff --git a/tests/test_a.py b/tests/test_a.py\n--- a/tests/test_a.py\n+++ b/tests/test_a.py\n@@ -1 +1 @@\n-x\n+y\n"
            "diff --git a/old.py b/new.py\nrename from old.py\nrename to new.py\n"
            "diff --git a/tests/test_a.py b/tests/test_a.py\n"
        )
        self.assertEqual(files_of_patch(patch), ["new.py", "tests/test_a.py"])
        self.assertEqual(files_of_patch(""), [])

    def test_generation_order_is_salted_sha256(self) -> None:
        ids = ["django__django-1", "sympy__sympy-2", "astropy__astropy-3"]
        expected = sorted(ids, key=lambda i: hashlib.sha256(("s" + i).encode()).hexdigest())
        self.assertEqual(generation_order(ids, "s"), expected)
        self.assertEqual(order_key("x", "ab"), hashlib.sha256(b"abx").hexdigest())

    def _rows(self, ids: list[str]) -> list[dict[str, Any]]:
        return [{"instance_id": i, "repo": i.split("__")[0], "test_patch": f"diff --git a/t_{i}.py b/t_{i}.py\n"} for i in ids]

    def test_population_disjoint_and_ordered(self) -> None:
        rows = self._rows(["a__1", "a__2", "a__3", "b__4", "b__5"])
        pop = build_population(rows, {"a__1", "a__2"}, {"a__1", "a__2"}, {"a__1"}, {"a__2"}, {"a": {"t_a__3.py"}})
        ids = [e["instance_id"] for e in pop["pool"]]
        self.assertEqual(sorted(ids), ["a__3", "b__4", "b__5"])
        self.assertEqual(ids, generation_order(ids))
        self.assertEqual(pop["counts"]["pool_sharing_a_v1_test_file"], 1)
        self.assertEqual(pop["pool_per_repo"], {"a": 1, "b": 2})

    def test_population_refuses_a_v1_id_in_the_pool(self) -> None:
        rows = self._rows(["a__1", "a__2", "a__3"])
        # a__3 is a v1 confirmatory id but not in Verified: the pool would contain it.
        with self.assertRaises(ValueError):
            build_population(rows, {"a__1", "a__2"}, {"a__1", "a__2", "a__3"}, {"a__1"}, {"a__2", "a__3"}, {})

    def test_population_refuses_impossible_ids_other_than_v1(self) -> None:
        rows = self._rows(["a__1", "a__2", "a__3"])
        with self.assertRaises(ValueError):
            build_population(rows, {"a__1", "a__2"}, {"a__1"}, {"a__1"}, {"a__2"}, {})

    def test_check_population_refuses_a_reordered_pool(self) -> None:
        rows = self._rows(["a__3", "b__4", "b__5"])
        pop = build_population(rows, set(), set(), set(), set(), {})
        pop["pool"] = list(reversed(pop["pool"]))
        with self.assertRaises(ValueError):
            check_population(pop, set(), set())

    def test_step0_values_are_pinned(self) -> None:
        v = fresh_bound_values()
        self.assertEqual(v["precision"]["n_well_formed_units"], 93)
        self.assertEqual(v["precision"]["required_units"], 99)
        self.assertEqual(v["yield"]["validated_instances_required"], 408)
        self.assertEqual(v["yield"]["instances_to_generate"], 585)
        self.assertEqual(v["yield"]["agent_runs"], 1632)
        for row in v["floor"]["pass_probability"]:
            self.assertLess(row["value"], 1e-12)
        self.assertAlmostEqual(v["generalisation"]["minimum_detectable_drop"], 0.2083, places=4)


def main() -> None:
    if "--test" in sys.argv or "--self-check" in sys.argv:
        sys.argv = [a for a in sys.argv if a not in ("--test", "--self-check")]
        unittest.main()
        return
    if "--bounds-json" in sys.argv:
        print(json.dumps(fresh_bound_values(), indent=2))
        return
    if "--check-population" in sys.argv:
        pop = json.loads((RESULTS_DIR / "population.json").read_text())
        check_population(pop, read_ids(LAB_DIR / "pilot-ids-swe.txt"), read_ids(LAB_DIR / "confirmatory-ids-swe.txt"))
        print("population.json: checks hold")
        return
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("population", help="scan the datasets and write population.json (network, read only)")
    p.add_argument("--out", type=Path, default=RESULTS_DIR / "population.json")
    args = ap.parse_args()
    if args.cmd == "population":
        population(args.out)


if __name__ == "__main__":
    main()
