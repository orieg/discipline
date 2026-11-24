#!/usr/bin/env python3
"""Lab runner for `base-tests-v1` (#1052): the Part 1 and Part 2 runs of the `base-tests` preset.

Pre-registration: `docs/benchmarks/base-tests-preregistration.toml` (`locked-pre-data`; Part 2
estimate-only, `[q2_friction] label`).

Each run starts a fresh container from the instance's SWE-bench image (no network, the
`container_memory` constant), copies the pinned static Linux discipline binary into it, and
builds two commits in `/testbed`:

- B: the base tree plus a `discipline.toml` that turns on the `command` gate's `base-tests`
  preset with the repository's runner command, restricted to the base test files the run
  covers (`{test_files}` / `{test_modules}` in the command template).
- H: B plus the change under test.

`discipline check --format json --base B` then runs on H, so the preset puts B's test files
over H's code, as it does in CI. The runs:

- Part 1 (`q1_feasibility`), one per sampled pair: B = `base_commit` + the pair's impossible
  `test_patch`, H = B + the agent's patch (`manifest-patches/<pair_id>/agent_patch.diff`).
  Sampled: up to two pairs per repository owner among the `mixed` and `test-side` pairs of
  `results-recall-confirmatory/strata.json`, the two with the smallest
  sha256(q1_salt || pair_id) as hex text (`q1_sample`).
  In pilot mode the pairs are every `mixed` or `test-side` pair of the #838 pilot's
  counterfactual records (`results-recall-pilot/strata.json`, `pilot_part1`).
- Part 2 (`q2_friction`), per confirmatory and pilot instance, two runs in this order:
  `control`, the runner command on B alone (base tests on base code, JUnit read by this
  script), then `gold`, H = B + gold patch + test patch. B = `base_commit`. Only tests that
  pass in the control count (`instance_outcome`); eligibility is `eligibility`.
- Part 3 is not run here: it needs the shape classification the `[lock]` preconditions name.

`/testbed` in the SWE-bench images is one `SWE-bench` setup commit past `base_commit`; B is
built on it (as the harness evaluates on it) and the files it touches are recorded
(`image_setup_commit`). A counted test is failed in the gold run when the preset names it, names
its module (a `collection failure`), or reports the whole suite failed (`covered`). The django
and sympy runner commands write JUnit through `base_tests_junit.py`, copied into each
container under `HELPER_DIR`. `pilot-values` writes `[pilot] writes_at_lock` from the pilot's
`summary.json` (`pilot_values`).

Constants. The `[constants]` values marked `LOCK` in the pre-registration are the pilot's to
fix. `--constants <json>` overlays candidate values in pilot mode. Confirmatory mode reads the
pre-registration alone and refuses (exit 2, before any Docker call) while the study is not
`locked-pre-data`, while any `[constants]` or `[binary]` value is still `LOCK`, or when
`--constants` is given (`confirmatory_refusal`). Pilot mode needs the global constants and
runs only repositories whose per-repository constants are set; the others are recorded as
`not-configured`.

Subcommands:

- `plan [--dry-run] [--json]`: the Part 1 sample and the Part 2 run plan, with the constants
  still `LOCK` and whether confirmatory mode would refuse. Reads committed files only; no
  Docker, no network. `--dry-run` is accepted on `run` too and prints the same plan.
- `run --mode pilot|confirmatory --part 1|2 --dataset <jsonl> --binary <path>`: the runs,
  sequentially, under the host-wide benchmark lock (`LOCK_FILE`, taken in-process with
  flock(2), which flock(1) shares). One result file per instance or pair under
  `--root/results/`; an existing result is kept unless `--force`.
- `collect`: `runs.json` and `blocks.json` (each blocked gold pull request, `class` empty
  for the rater) from the result files.
- `summarize`: `summary.json` from `runs.json` and `blocks.json`.
- `pilot-values`: `pilot-values.json` from the pilot's `summary.json`.
- `--test`: self-checks, no host, Docker or network.

`--dataset` is a JSON-lines export of the pinned ImpossibleBench SWE-bench revision
(`dataset_swe_revision` in `scripts/recall_lab/run_confirmatory.py`): one object per
(instance_id, split) with `base_commit`, `patch`, `test_patch` and `repo`. Part 2 reads the
`original` split. A test patch whose sha256 differs from the #838 manifest's record for the
same instance and split stops the run.
"""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time
import tomllib
from typing import Any
import xml.etree.ElementTree as ET


def _load_sibling(name: str) -> Any:
    """Import a sibling script by path (works under `python3 -I`)."""
    if name in sys.modules:
        return sys.modules[name]
    here = Path(__file__).resolve().parent
    if str(here) not in sys.path:
        sys.path.insert(0, str(here))
    spec = importlib.util.spec_from_file_location(name, here / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


htob = _load_sibling("head_tests_on_base")  # test-path rule mirror, Wilson interval
htob_lab = _load_sibling("head_tests_on_base_lab")  # JUnit parser, lock path
cf = _load_sibling("recall_counterfactual")  # image name, pull, tar, quoting
pairs_mod = _load_sibling("recall_pairs")  # patch parser, sha256_text

REPO_ROOT = Path(__file__).resolve().parent.parent
PREREG = REPO_ROOT / "docs" / "benchmarks" / "base-tests-preregistration.toml"
RECALL = REPO_ROOT / "docs" / "benchmarks" / "results-recall-confirmatory"
STRATA = RECALL / "strata.json"
MANIFEST = RECALL / "manifest.json"
# The #838 pilot: its counterfactual records cover 40 pilot pairs; the `mixed` and `test-side`
# ones are the pilot's Part 1 pairs (all of them, not a sample: the pilot fixes the runners).
RECALL_PILOT = REPO_ROOT / "docs" / "benchmarks" / "results-recall-pilot"
STRATA_PILOT = RECALL_PILOT / "strata.json"
MANIFEST_PILOT = RECALL_PILOT / "manifest.json"
RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results-base-tests"
STUDY_ID = "base-tests-v1"
LOCK = "LOCK"
LOCK_FILE = htob_lab.LOCK
ELIGIBLE_STRATA = ("mixed", "test-side")
PER_REPOSITORY = ("runner_command_per_repository", "junit_options_per_repository", "tests_paths_globs_per_repository")
GLOBAL_CONSTANTS = ("timeout_seconds", "flaky_rerun_policy", "image_digests", "container_memory")
# SWE-bench images install each repository into the `testbed` conda environment.
ENV_PREFIX = "source /opt/miniconda3/bin/activate && conda activate testbed && "
HELPER_DIR = "/opt/base-tests"  # outside /testbed: never in the commits B and H
BASE_TEST_FAILED = "command/base-test-failed"
BASE_TESTS_NO_CASES = "command/base-tests-no-cases"
FAILED_ID = re.compile(r"^Base test `(.+?)` failed when executed against head code")
PASSED_NOTE = re.compile(r"preset 'base-tests': (\d+) base tests passed against head code")
BLOCK_CLASSES = ("behaviour-change", "environment", "other")
NOT_ELIGIBLE = (
    "no-base-test-file-modified",  # the test patch modifies no test file that exists at base_commit
    "control-no-case",  # the control read no JUnit case
    "control-no-passing-test",  # no test of those files passes in the control
    "not-configured",  # pilot only: the repository has no candidate constants yet
    "run-error",  # a step outside the tests failed (image, patch, binary); cause recorded
)


# --- Pre-registration and constants -----------------------------------------------------


def load_prereg(path: Path = PREREG) -> dict[str, Any]:
    with path.open("rb") as f:
        return tomllib.load(f)


def owner(instance_id: str) -> str:
    """`django__django-11099` -> `django`."""
    return instance_id.split("__", 1)[0]


def read_ids(rel: str) -> list[str]:
    return [line.strip() for line in (REPO_ROOT / rel).read_text(encoding="utf-8").splitlines() if line.strip()]


def effective_constants(prereg: dict[str, Any], overlay: dict[str, Any] | None) -> dict[str, Any]:
    out = dict(prereg["constants"])
    for k, v in (overlay or {}).items():
        if k not in out:
            raise ValueError(f"--constants names {k!r}, which is not a [constants] key")
        out[k] = v
    return out


def lock_values(prereg: dict[str, Any], constants: dict[str, Any]) -> list[str]:
    """Every `[constants]` and `[binary]` key whose value is still `LOCK`."""
    left = [f"constants.{k}" for k, v in constants.items() if v == LOCK]
    left += [f"binary.{k}" for k, v in prereg["binary"].items() if v == LOCK]
    return sorted(left)


def confirmatory_refusal(prereg: dict[str, Any], overlay: dict[str, Any] | None) -> str | None:
    """Why confirmatory mode must not run, or None. Fail-closed: any doubt is a refusal."""
    if overlay is not None:
        return "--constants is a pilot overlay; confirmatory runs read the locked pre-registration only"
    status = prereg.get("study", {}).get("status")
    left = lock_values(prereg, dict(prereg.get("constants", {})))
    if left:
        return f"values still LOCK: {', '.join(left)}"
    if status != "locked-pre-data":
        return f"study status is {status!r}, not 'locked-pre-data'"
    return None


def pilot_refusal(constants: dict[str, Any]) -> str | None:
    left = [k for k in GLOBAL_CONSTANTS if constants.get(k) == LOCK]
    return f"pilot needs the global constants: {', '.join(left)} still LOCK" if left else None


def repo_constants(constants: dict[str, Any], repo_owner: str) -> dict[str, Any] | None:
    """The per-repository constants for `repo_owner`, or None when any is unset."""
    out: dict[str, Any] = {}
    for key in PER_REPOSITORY:
        table = constants.get(key)
        if not isinstance(table, dict) or repo_owner not in table:
            return None
        out[key] = table[repo_owner]
    return out


# --- Part 1 sample -----------------------------------------------------------------------


def eligible_pairs(strata: dict[str, Any]) -> list[dict[str, Any]]:
    return [p for p in strata["counterfactual"]["pairs"] if p.get("stratum") in ELIGIBLE_STRATA]


def q1_rank(salt: str, pair_id: str) -> str:
    return hashlib.sha256((salt + pair_id).encode("utf-8")).hexdigest()


def q1_sample(strata: dict[str, Any], salt: str, per_repository: int = 2) -> list[dict[str, Any]]:
    """Up to `per_repository` eligible pairs per owner: the smallest sha256(salt || pair_id) as hex text."""
    by_owner: dict[str, list[dict[str, Any]]] = {}
    for p in eligible_pairs(strata):
        by_owner.setdefault(owner(p["instance_id"]), []).append(
            {"pair_id": p["pair_id"], "instance_id": p["instance_id"], "stratum": p["stratum"], "rank": q1_rank(salt, p["pair_id"])}
        )
    out = []
    for o in sorted(by_owner):
        for p in sorted(by_owner[o], key=lambda x: x["rank"])[:per_repository]:
            out.append({"owner": o, **p})
    return out


def pilot_part1(pilot_strata: dict[str, Any]) -> list[dict[str, Any]]:
    """The pilot's Part 1 pairs: every `mixed` or `test-side` pilot pair with a counterfactual
    record, in the confirmatory sample's order (owner, then sha256(q1_salt || pair_id))."""
    salt = load_prereg()["q1_feasibility"]["q1_salt"]
    return q1_sample(pilot_strata, salt, per_repository=len(pilot_strata["counterfactual"]["pairs"]))


def eligible_per_owner(strata: dict[str, Any]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for p in eligible_pairs(strata):
        counts[owner(p["instance_id"])] = counts.get(owner(p["instance_id"]), 0) + 1
    return dict(sorted(counts.items()))


# --- Test files, commands and configuration ----------------------------------------------


def glob_match(path: str, pattern: str) -> bool:
    """A `[tests] paths` glob: `**` spans directories, `*` and `?` stay within one."""
    rx, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            rx, i = rx + "(?:.*/)?", i + 3
        elif pattern.startswith("**", i):
            rx, i = rx + ".*", i + 2
        elif pattern[i] == "*":
            rx, i = rx + "[^/]*", i + 1
        elif pattern[i] == "?":
            rx, i = rx + "[^/]", i + 1
        else:
            rx, i = rx + re.escape(pattern[i]), i + 1
    return re.fullmatch(rx, path) is not None


def is_test_file(path: str, globs: list[str]) -> bool:
    return htob.is_test_path(path) or any(glob_match(path, g) for g in globs)


def base_test_files(patch: str, globs: list[str]) -> list[str]:
    """Test files the patch modifies, renames or deletes: those that exist on its base side."""
    return sorted({fp.old_path for fp in pairs_mod.parse_patch(patch) if fp.old_path is not None and is_test_file(fp.old_path, globs)})


def head_test_files(patch: str, globs: list[str]) -> list[str]:
    return sorted({fp.new_path for fp in pairs_mod.parse_patch(patch) if fp.new_path is not None and is_test_file(fp.new_path, globs)})


def modules(files: list[str], root: str = "") -> list[str]:
    prefix = root.rstrip("/") + "/" if root else ""
    out = []
    for f in files:
        if f.endswith(".py") and f.startswith(prefix):
            m = f[len(prefix) : -3].replace("/", ".")
            out.append(m.removesuffix(".__init__"))
    return out


MODULES_IN = re.compile(r"\{test_modules:([^}]+)\}")


def expand_command(template: str, junit: str, files: list[str]) -> str:
    """The runner command for `files`. Placeholders: `{junit}` (the JUnit options),
    `{test_files}`, `{test_modules}` (dotted, from the repository root) and
    `{test_modules:DIR}` (dotted, relative to DIR, files outside DIR left out)."""
    if not files:
        raise ValueError("no test file to run")
    cmd = template.replace("{junit}", junit)
    cmd = MODULES_IN.sub(lambda m: " ".join(shlex.quote(x) for x in modules(files, m.group(1))), cmd)
    cmd = cmd.replace("{test_modules}", " ".join(shlex.quote(x) for x in modules(files)))
    cmd = cmd.replace("{test_files}", " ".join(shlex.quote(f) for f in files))
    leftover = re.findall(r"\{test_[a-z_]+(?::[^}]*)?\}|\{junit\}", cmd)
    if leftover:
        raise ValueError(f"unexpanded placeholders {leftover}")
    return cmd


def toml_str(s: str) -> str:
    return json.dumps(s)  # a JSON string is a valid TOML basic string


def discipline_config(command: str, globs: list[str], timeout: int) -> str:
    """The `discipline.toml` committed in B: the `base-tests` preset with the run's command."""
    lines = ["[meta]", "version = 1", f"name = {toml_str(STUDY_ID)}", ""]
    if globs:
        lines += ["[tests]", f"paths = [{', '.join(toml_str(g) for g in globs)}]", ""]
    lines += [
        "[gates.command]",
        "enabled = true",
        'preset = "base-tests"',
        f"command = {toml_str('bash -c ' + shlex.quote(ENV_PREFIX + command))}",
        f"timeout_seconds = {int(timeout)}",
    ]
    return "\n".join(lines) + "\n"


# --- Reading results ---------------------------------------------------------------------


JUNIT_DOC = re.compile(r"<testsuites\b.*?</testsuites>|<testsuite\b.*?</testsuite>", re.S)


def junit_cases(*texts: str) -> tuple[dict[str, str], int]:
    """Every JUnit case in the texts, ids as the preset forms them (`classname::name`, or
    `name` with no class), and how many JUnit documents did not parse (recorded, and read
    as no case)."""
    cases: dict[str, str] = {}
    unparsed = 0
    for text in texts:
        for doc in JUNIT_DOC.findall(text or ""):
            try:
                parsed = htob_lab.parse_junit(doc)
            except ET.ParseError:
                unparsed += 1
                parsed = {}
            for cid, status in parsed.items():
                cases[cid.removeprefix("::")] = status
    return cases, unparsed


def read_report(report: dict[str, Any] | None) -> dict[str, Any]:
    """What the `base-tests` preset said in one `discipline check` JSON report."""
    out: dict[str, Any] = {"failed_ids": [], "suite_failed": False, "no_cases": False, "passed_count": None, "other_findings": []}
    if report is None:
        return out
    for o in report.get("outcomes", []):
        for v in o.get("violations", []):
            code = v.get("code")
            if code == BASE_TEST_FAILED:
                m = FAILED_ID.match(v.get("message", ""))
                if m:
                    out["failed_ids"].append(m.group(1))
                else:
                    out["suite_failed"] = True
            elif code == BASE_TESTS_NO_CASES:
                out["no_cases"] = True
            else:
                out["other_findings"].append(code)
        if o.get("gate") == "command":
            for n in o.get("notes", []):
                m = PASSED_NOTE.search(n)
                if m:
                    out["passed_count"] = int(m.group(1))
    out["failed_ids"].sort()
    out["other_findings"].sort()
    return out


def covered(counted: list[str], report: dict[str, Any]) -> set[str]:
    """The counted tests one `read_report` result fails: by id, under a failed module id
    (`pkg.tests.test_x` covers `pkg.tests.test_x::Case::test` and `pkg.tests.test_x.Case::test`),
    or all of them when the suite failed."""
    if report["suite_failed"]:
        return set(counted)
    out = set()
    for f in report["failed_ids"]:
        out |= {c for c in counted if c == f or c.startswith(f + "::") or c.startswith(f + ".")}
    return out


def eligibility(files: list[str], control: dict[str, Any] | None) -> tuple[bool, str | None]:
    """`[q2_friction].population`: eligible when the test patch modifies a base test file and
    at least one test of those files passes in the control."""
    if not files:
        return False, "no-base-test-file-modified"
    if control is None or control.get("error"):
        return False, "run-error"
    cases = control.get("cases", {})
    if not cases:
        return False, "control-no-case"
    if not any(s == "passed" for s in cases.values()):
        return False, "control-no-passing-test"
    return True, None


def instance_outcome(files: list[str], control: dict[str, Any] | None, gold: dict[str, Any] | None) -> dict[str, Any]:
    """The Part 2 outcome of one instance from its control and gold runs."""
    ok, reason = eligibility(files, control)
    if not ok:
        return {"eligible": False, "reason": reason}
    counted = sorted(c for c, s in control["cases"].items() if s == "passed")
    excluded = sorted(c for c, s in control["cases"].items() if s != "passed")
    base = {"counted_tests": len(counted), "excluded_tests": excluded}
    if gold is None or gold.get("error"):
        return {"eligible": False, "reason": "run-error", **base}
    attempts = [read_report(a.get("report")) for a in gold.get("attempts", [])] or [read_report(None)]
    # A block stands when a counted test fails in every attempt (`flaky_rerun_policy`). A
    # failure the preset reports for a whole file (`collection failure`, id = the module) or
    # for the whole suite (the command failed with no case) fails every counted test it
    # covers: they did not run and the preset blocks the change.
    failing = set(counted)
    for a in attempts:
        failing &= covered(counted, a)
    first = attempts[0]
    return {
        "eligible": True,
        "reason": None,
        **base,
        "blocked": bool(failing),
        "failing_counted": sorted(failing),
        "failing_uncounted": sorted(set(first["failed_ids"]) - set(counted)),
        "suite_failed": first["suite_failed"],
        "preset_failed_every_attempt": all(a["failed_ids"] or a["suite_failed"] for a in attempts),
        "attempts": len(attempts),
    }


def q2_verdict(k: int, n: int, ceiling: float, label: str | None = None) -> str:
    """The `[verdict] q2_rule`. Under `label = "estimate-only"` (set at the lock) Part 2 reports
    k / n and its interval and no ceiling verdict."""
    if label == "estimate-only":
        return "ESTIMATE_ONLY"
    if n == 0:
        return "INTERMEDIATE"
    lo, hi = htob.wilson_interval(k, n)
    if hi <= ceiling:
        return "LOW_FRICTION"
    if lo > ceiling:
        return "HIGH_FRICTION"
    return "INTERMEDIATE"


def pair_outcome(result: dict[str, Any] | None) -> dict[str, Any]:
    """Part 1: whether the preset read cases, and whether it blocked (the stratum's expectation)."""
    if result is None or result.get("error"):
        return {"runs": False, "cause": (result or {}).get("error", "not run"), "blocked": None}
    r = read_report(result.get("report"))
    read_cases = bool(r["failed_ids"]) or bool(r["passed_count"])
    cause = None if read_cases else ("no JUnit case" if r["no_cases"] else "suite failed" if r["suite_failed"] else "no base-tests outcome")
    return {"runs": read_cases, "cause": cause, "blocked": bool(r["failed_ids"]) or r["suite_failed"]}


def share(k: int, n: int) -> dict[str, Any]:
    return {"k": k, "n": n, "wilson_95": list(htob.wilson_interval(k, n)) if n else [None, None]}


def summarize(runs: dict[str, Any], blocks: dict[str, Any] | None, prereg: dict[str, Any]) -> dict[str, Any]:
    ceiling = prereg["power_and_bounds"]["q2_ceiling"]
    label = prereg.get("q2_friction", {}).get("label")
    out: dict[str, Any] = {"study_id": STUDY_ID, "part1": {}, "part2": {}, "part3": "not run by this runner (needs the shape classification)"}
    # Part 1, per repository owner.
    per_owner: dict[str, list[dict[str, Any]]] = {}
    for p in runs.get("part1", []):
        per_owner.setdefault(p["owner"], []).append(p)
    for o, rows in sorted(per_owner.items()):
        outs = [pair_outcome(r.get("result")) for r in rows]
        if all(x["runs"] for x in outs):
            v = "RUNS_WITH_PATHS" if any(r.get("tests_paths_globs") for r in rows) else "RUNS"
        else:
            v = "DOES_NOT_RUN"
        out["part1"][o] = {
            "verdict": v,
            "pairs": len(rows),
            "agree_with_stratum": sum(bool(x["blocked"]) for x in outs),
            "causes": sorted({x["cause"] for x in outs if x["cause"]}),
            "label": "PASS_categorical_by_design",
        }
    # Part 2, confirmatory and pilot apart.
    classes = {(b["population"], b["instance_id"]): b.get("class") for b in (blocks or {}).get("blocks", [])}
    bad = sorted({c for c in classes.values() if c is not None and c not in BLOCK_CLASSES})
    if bad:
        raise ValueError(f"block classes outside the pre-registered ones: {bad}")
    for pop in ("confirmatory", "pilot"):
        rows = [r for r in runs.get("part2", []) if r["population"] == pop]
        outs = [r["outcome"] for r in rows]
        eligible = [o for o in outs if o["eligible"]]
        k = sum(o["blocked"] for o in eligible)
        reasons = {why: sum(o["reason"] == why for o in outs) for why in NOT_ELIGIBLE}
        counted = sum(o.get("counted_tests", 0) for o in outs)
        excluded = sum(len(o.get("excluded_tests", [])) for o in outs)
        by_class = {c: sum(1 for r in rows if r["outcome"].get("blocked") and classes.get((pop, r["instance_id"])) == c) for c in BLOCK_CLASSES}
        per_repo: dict[str, dict[str, int]] = {}
        for r in rows:
            t = per_repo.setdefault(r["owner"], {"instances": 0, "eligible": 0, "blocked": 0})
            t["instances"] += 1
            t["eligible"] += int(r["outcome"]["eligible"])
            t["blocked"] += int(bool(r["outcome"].get("blocked")))
        out["part2"][pop] = {
            "instances": len(rows),
            "eligible": len(eligible),
            "not_eligible": reasons,
            "eligible_share": share(len(eligible), len(rows)),
            "friction": share(k, len(eligible)),
            "verdict": q2_verdict(k, len(eligible), ceiling, label) if pop == "confirmatory" else "pilot: no verdict",
            "control_excluded_tests": {"excluded": excluded, "of": counted + excluded},
            "control_excluded_instances": sum(1 for o in outs if o.get("excluded_tests")),
            "block_classes": by_class,
            "unclassified_blocks": sum(1 for r in rows if r["outcome"].get("blocked") and classes.get((pop, r["instance_id"])) is None),
            "per_repository": dict(sorted(per_repo.items())),
        }
    return out


# --- Plan ----------------------------------------------------------------------------------


def plan(prereg: dict[str, Any], strata: dict[str, Any], overlay: dict[str, Any] | None = None) -> dict[str, Any]:
    q1 = prereg["q1_feasibility"]
    pop = prereg["population"]
    constants = effective_constants(prereg, overlay)
    sample = q1_sample(strata, q1["q1_salt"])
    part2 = []
    for population, rel in (("confirmatory", pop["confirmatory_ids"]), ("pilot", pop["pilot_ids"])):
        for iid in read_ids(rel):
            part2.append({"population": population, "instance_id": iid, "owner": owner(iid), "runs": ["control", "gold"]})
    sampled_per_owner: dict[str, int] = {}
    for p in sample:
        sampled_per_owner[p["owner"]] = sampled_per_owner.get(p["owner"], 0) + 1
    refusal = confirmatory_refusal(prereg, overlay)
    return {
        "study_id": STUDY_ID,
        "status": prereg["study"]["status"],
        "part1": {
            "eligible_pairs": len(eligible_pairs(strata)),
            "eligible_per_repository": eligible_per_owner(strata),
            "sampled_pairs": len(sample),
            "sampled_per_repository": dict(sorted(sampled_per_owner.items())),
            "sample": sample,
        },
        "part2": {
            "instances": {k: sum(r["population"] == k for r in part2) for k in ("confirmatory", "pilot")},
            "runs_per_instance": ["control", "gold"],
            "instances_plan": part2,
        },
        "lock_values_remaining": lock_values(prereg, constants),
        "confirmatory_refusal": refusal,
        "pilot_refusal": pilot_refusal(constants),
    }


def print_plan(p: dict[str, Any], as_json: bool) -> None:
    if as_json:
        print(json.dumps(p, indent=2))
        return
    q1, q2 = p["part1"], p["part2"]
    print(f"{p['study_id']} ({p['status']}), dry run: no Docker, no network")
    print(f"Part 1: {q1['sampled_pairs']} of {q1['eligible_pairs']} eligible pairs")
    for s in q1["sample"]:
        print(f"  {s['owner']:<14} {s['pair_id']}  {s['instance_id']:<32} {s['stratum']:<9} {s['rank'][:12]}")
    print(f"Part 2: {q2['instances']['confirmatory']} confirmatory + {q2['instances']['pilot']} pilot instances, runs {' -> '.join(q2['runs_per_instance'])}")
    print(f"LOCK values remaining: {', '.join(p['lock_values_remaining']) or 'none'}")
    print(f"confirmatory mode: {'refused: ' + p['confirmatory_refusal'] if p['confirmatory_refusal'] else 'allowed'}")


# --- Lab execution (Docker) --------------------------------------------------------------


def take_lock(path: str) -> Any:
    import fcntl

    fh = open(path, "a")  # held for the process lifetime, closed by the caller
    try:
        fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print(f"waiting for the benchmark lock {path}", file=sys.stderr)
        fcntl.flock(fh, fcntl.LOCK_EX)
    return fh


def load_dataset(path: Path) -> dict[tuple[str, str], dict[str, Any]]:
    rows = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            r = json.loads(line)
            rows[(r["instance_id"], r.get("split", "original"))] = r
    return rows


def manifest_test_patch_sha(manifest: dict[str, Any]) -> dict[tuple[str, str], str]:
    """(instance, split) -> sha256 of the test patch the #838 manifest recorded."""
    out = {}
    for p in manifest["pairs"]:
        sha = p.get("patch_sha256") or {}
        if sha.get("test_patch"):
            out[(p["instance_id"], p["split"])] = sha["test_patch"]
        if sha.get("original_test_patch"):
            out[(p["instance_id"], "original")] = sha["original_test_patch"]
    return out


def check_patch_sha(known: dict[tuple[str, str], str], iid: str, split: str, text: str) -> None:
    want = known.get((iid, split))
    if want is not None and pairs_mod.sha256_text(text) != want:
        raise RuntimeError(f"{iid} {split}: test_patch sha256 differs from the #838 manifest")


class Container:
    """One fresh container from the instance image, removed on exit."""

    def __init__(self, image: str, memory: str, binary: Path):
        self.image, self.memory, self.binary, self.cid = image, memory, binary, ""
        self.setup_commit: dict[str, Any] | None = None

    def __enter__(self) -> "Container":
        try:
            return self._start()
        except BaseException:
            self.__exit__()
            raise

    def _start(self) -> "Container":
        res = cf._run(["docker", "run", "-d", "--network", "none", "--memory", self.memory, "-w", "/testbed", self.image, "sleep", "infinity"])
        self.cid = res.stdout.decode().strip()
        if not self.cid:
            raise RuntimeError(f"could not start a container from {self.image}")
        put = cf._run(["docker", "cp", str(self.binary), f"{self.cid}:/usr/local/bin/discipline"])
        if put.returncode != 0:
            raise RuntimeError("could not copy the discipline binary")
        # The JUnit shims the django and sympy runner commands use (`base_tests_junit.py`).
        helper = Path(__file__).resolve().parent / "base_tests_junit.py"
        self.must(f"mkdir -p {HELPER_DIR}", "make the helper directory")
        if cf._run(["docker", "cp", str(helper), f"{self.cid}:{HELPER_DIR}/base_tests_junit.py"]).returncode != 0:
            raise RuntimeError("could not copy the JUnit helper")
        self.must(f"{ENV_PREFIX}python {HELPER_DIR}/base_tests_junit.py --write-settings", "write the helper settings")
        return self

    def __exit__(self, *_: Any) -> None:
        if self.cid:
            cf._run(["docker", "rm", "-f", self.cid])

    def sh(self, cmd: str, timeout: int = 600, stdin: bytes | None = None) -> subprocess.CompletedProcess:
        args = ["docker", "exec"] + (["-i"] if stdin is not None else []) + [self.cid, "bash", "-c", cmd]
        return cf._run(args, timeout=timeout, input_bytes=stdin)

    def must(self, cmd: str, what: str, stdin: bytes | None = None) -> str:
        res = self.sh(cmd, stdin=stdin)
        if res.returncode != 0:
            raise RuntimeError(f"{what}: {res.stderr.decode(errors='replace')[-400:]}")
        return res.stdout.decode(errors="replace").strip()

    def apply(self, patch: str, what: str) -> None:
        if patch.strip():
            self.must("cd /testbed && git apply --whitespace=nowarn -", what, stdin=patch.encode())

    def commit(self, msg: str) -> str:
        return self.must(
            f"cd /testbed && git add -A && git -c user.name=bt -c user.email=bt@invalid commit -q --no-verify --allow-empty -m {cf._shell_quote(msg)} && git rev-parse HEAD",
            f"commit {msg}",
        )

    def setup_base(self, base_commit: str, base_patch: str, config: str) -> str:
        head = self.must("cd /testbed && git rev-parse HEAD", "read HEAD")
        setup_commit = None
        if head != base_commit:
            # The SWE-bench images commit their environment setup as one `SWE-bench` commit
            # on top of `base_commit`; the harness evaluates on it, so B is built on it too and
            # the files it touches are recorded. Anything else stops the run.
            parent = self.must("cd /testbed && git rev-parse HEAD^", "read HEAD^")
            subject = self.must("cd /testbed && git log -1 --format=%s", "read HEAD subject")
            if parent != base_commit or subject != "SWE-bench":
                raise RuntimeError(f"/testbed is at {head} ({subject!r}), neither base_commit {base_commit} nor its SWE-bench setup commit")
            files = self.must(f"cd /testbed && git diff --name-only {base_commit} HEAD", "read the setup commit")
            names = sorted(f for f in files.splitlines() if f)
            setup_commit = {"sha": head, "files_changed": len(names), "first_files": names[:20]}
        self.setup_commit = setup_commit
        self.must("cd /testbed && git status --porcelain --untracked-files=no", "status")
        self.apply(base_patch, "base patch does not apply")
        self.must("cat > /testbed/discipline.toml", "write discipline.toml", stdin=config.encode())
        return self.commit("base")

    def run_tests(self, command: str, timeout: int) -> dict[str, Any]:
        t0 = time.time()
        try:
            res = self.sh(f"cd /testbed && {ENV_PREFIX}{command}", timeout=timeout)
            code, timed_out = res.returncode, False
            out, err = res.stdout.decode(errors="replace"), res.stderr.decode(errors="replace")
        except subprocess.TimeoutExpired:
            code, timed_out, out, err = None, True, "", ""
        cases, unparsed = junit_cases(out, err)
        if not cases:
            # As the preset: a report file the command left at the top of the tree, first in
            # name order, never a committed one.
            listed = self.sh("cd /testbed && git ls-files --others --directory -z").stdout.decode(errors="replace")
            for name in sorted(n for n in listed.split("\0") if n.endswith(".xml") and "/" not in n):
                found, bad = junit_cases(self.sh(f"cat -- /testbed/{shlex.quote(name)}").stdout.decode(errors="replace"))
                unparsed += bad
                if found:
                    cases = found
                    break
        return {"exit_code": code, "timed_out": timed_out, "seconds": round(time.time() - t0), "cases": cases, "unparsed_junit_documents": unparsed}

    def check(self, base_sha: str, timeout: int) -> dict[str, Any]:
        self.must("printf '' > /tmp/pr_body", "write PR body")
        env = "env -u GITHUB_TOKEN DISCIPLINE_NO_NETWORK=1 HOME=/tmp"
        try:
            res = self.sh(
                f"cd /testbed && {env} discipline check --format json --base {base_sha} --pr-body-file /tmp/pr_body",
                timeout=timeout + 300,
            )
        except subprocess.TimeoutExpired:
            return {"exit_code": None, "timed_out": True, "report": None}
        text = res.stdout.decode(errors="replace")
        report = json.loads(text) if text.strip() else None
        return {"exit_code": res.returncode, "timed_out": False, "report": report}


def image_for(iid: str, constants: dict[str, Any], strata: dict[str, Any], mode: str, more: tuple[dict[str, Any], ...] = ()) -> str:
    """`image_digests`: the #838 counterfactual digest where the instance has one (`strata`,
    then each of `more`), else the constant's table, else (pilot only) the tag. A
    confirmatory run with no digest stops."""
    for s in (strata, *more):
        for p in s["counterfactual"]["pairs"]:
            if p["instance_id"] == iid and p.get("image_digest"):
                return p["image_digest"]
    table = constants.get("image_digests")
    if isinstance(table, dict) and iid in table:
        return table[iid]
    if mode == "confirmatory":
        raise RuntimeError(f"{iid}: no image digest in the counterfactual records or `image_digests`")
    return cf.image_name(iid)


def run_part2(iid: str, row: dict[str, Any], rc: dict[str, Any], constants: dict[str, Any], image: str, binary: Path) -> dict[str, Any]:
    globs = list(rc["tests_paths_globs_per_repository"])
    files = base_test_files(row["test_patch"], globs)
    result: dict[str, Any] = {"base_test_files": files, "tests_paths_globs": globs, "image": image}
    if not files:
        result["outcome"] = instance_outcome(files, None, None)
        return result
    timeout = int(constants["timeout_seconds"])
    command = expand_command(rc["runner_command_per_repository"], rc["junit_options_per_repository"], files)
    config = discipline_config(command, globs, timeout)
    result["command"] = command
    control = gold = None
    try:
        with Container(image, constants["container_memory"], binary) as c:
            c.setup_base(row["base_commit"], "", config)
            result["image_setup_commit"] = c.setup_commit
            control = c.run_tests(command, timeout + 60)
        result["control"] = control
        ok, _ = eligibility(files, control)
        if ok:
            reruns = int((constants["flaky_rerun_policy"] or {}).get("head_reruns", 0))
            attempts = []
            for _ in range(1 + reruns):
                with Container(image, constants["container_memory"], binary) as c:
                    b = c.setup_base(row["base_commit"], "", config)
                    c.apply(row["patch"], "gold patch does not apply")
                    c.apply(row["test_patch"], "test patch does not apply")
                    c.commit("gold")
                    attempts.append(c.check(b, timeout))
                last = read_report(attempts[-1]["report"])
                if not last["failed_ids"] and not last["suite_failed"]:
                    break
            gold = {"attempts": attempts}
            result["gold"] = gold
    except (RuntimeError, subprocess.TimeoutExpired) as e:
        result["error"] = str(e)[-600:]
        result["outcome"] = {"eligible": False, "reason": "run-error"}
        return result
    result["outcome"] = instance_outcome(files, control, gold)
    return result


def run_part1(pair: dict[str, Any], row: dict[str, Any], agent_patch: str, rc: dict[str, Any], constants: dict[str, Any], image: str, binary: Path) -> dict[str, Any]:
    globs = list(rc["tests_paths_globs_per_repository"])
    files = sorted(set(head_test_files(row["test_patch"], globs)) | set(base_test_files(agent_patch, globs)))
    timeout = int(constants["timeout_seconds"])
    out: dict[str, Any] = {"test_files": files, "tests_paths_globs": globs, "image": image}
    try:
        command = expand_command(rc["runner_command_per_repository"], rc["junit_options_per_repository"], files)
        out["command"] = command
        with Container(image, constants["container_memory"], binary) as c:
            b = c.setup_base(row["base_commit"], row["test_patch"], discipline_config(command, globs, timeout))
            out["image_setup_commit"] = c.setup_commit
            c.apply(agent_patch, "agent patch does not apply")
            c.commit("agent")
            out.update(c.check(b, timeout))
    except (RuntimeError, ValueError, subprocess.TimeoutExpired) as e:
        out["error"] = str(e)[-600:]
    return out


def cmd_run(args: argparse.Namespace) -> int:
    prereg = load_prereg()
    overlay = json.loads(args.constants.read_text(encoding="utf-8")) if args.constants else None
    strata = json.loads(STRATA.read_text(encoding="utf-8"))
    if args.dry_run:
        print_plan(plan(prereg, strata, overlay), args.json)
        return 0
    # Fail closed before anything touches Docker or the lab.
    if args.mode == "confirmatory":
        why = confirmatory_refusal(prereg, overlay)
        if why:
            print(f"refusing confirmatory run: {why}", file=sys.stderr)
            return 2
    constants = effective_constants(prereg, overlay)
    if args.mode == "pilot" and (why := pilot_refusal(constants)):
        print(f"refusing pilot run: {why}", file=sys.stderr)
        return 2
    if args.dataset is None or args.binary is None:
        print("run needs --dataset and --binary", file=sys.stderr)
        return 2
    binary_sha = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    if args.mode == "confirmatory" and binary_sha != prereg["binary"]["sha256"]:
        print(f"refusing confirmatory run: binary sha256 {binary_sha} is not the locked one", file=sys.stderr)
        return 2
    dataset = load_dataset(args.dataset)
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    known = manifest_test_patch_sha(manifest)
    more: tuple[dict[str, Any], ...] = ()
    recall_dir = RECALL
    if args.mode == "pilot":
        # Pilot instances are recorded in the #838 pilot files: their test patches, their
        # counterfactual image digests and, for Part 1, their pairs and agent patches.
        manifest = json.loads(MANIFEST_PILOT.read_text(encoding="utf-8"))
        known = {**known, **manifest_test_patch_sha(manifest)}
        pilot_strata = json.loads(STRATA_PILOT.read_text(encoding="utf-8"))
        more = (pilot_strata,)
        recall_dir = RECALL_PILOT
    lock = take_lock(args.lock_file)
    results = args.root / "results" / f"part{args.part}"
    results.mkdir(parents=True, exist_ok=True)
    p = plan(prereg, strata, overlay)
    meta = {"mode": args.mode, "binary_sha256": binary_sha, "constants": constants}
    if args.part == 1:
        pairs = {x["pair_id"]: x for x in manifest["pairs"]}
        sample = pilot_part1(more[0]) if args.mode == "pilot" else p["part1"]["sample"]
        todo = [s for s in sample if not args.only or s["pair_id"] in args.only]
        for s in todo:
            dest = results / f"{s['pair_id']}.json"
            if dest.exists() and not args.force:
                continue
            m = pairs[s["pair_id"]]
            rc = repo_constants(constants, s["owner"])
            if rc is None:
                res: dict[str, Any] = {"error": "not-configured"}
            else:
                row = dataset[(s["instance_id"], m["split"])]
                check_patch_sha(known, s["instance_id"], m["split"], row["test_patch"])
                agent = (recall_dir / manifest["patches_dir"] / s["pair_id"] / "agent_patch.diff").read_text(encoding="utf-8")
                if pairs_mod.sha256_text(agent) != m["patch_sha256"]["agent_patch"]:
                    raise RuntimeError(f"{s['pair_id']}: agent patch sha256 differs from the manifest")
                res = run_part1(s, row, agent, rc, constants, image_for(s["instance_id"], constants, strata, args.mode, more), args.binary)
            dest.write_text(json.dumps({**s, **meta, "result": res}, indent=2) + "\n", encoding="utf-8")
            print(f"{s['pair_id']} {s['instance_id']}: {pair_outcome(res)}")
    else:
        todo = [r for r in p["part2"]["instances_plan"] if (args.mode == "pilot") == (r["population"] == "pilot")]
        todo = [r for r in todo if not args.only or r["instance_id"] in args.only]
        for r in todo:
            dest = results / f"{r['instance_id']}.json"
            if dest.exists() and not args.force:
                continue
            rc = repo_constants(constants, r["owner"])
            if rc is None:
                res = {"outcome": {"eligible": False, "reason": "not-configured"}}
            else:
                row = dataset[(r["instance_id"], "original")]
                check_patch_sha(known, r["instance_id"], "original", row["test_patch"])
                res = run_part2(r["instance_id"], row, rc, constants, image_for(r["instance_id"], constants, strata, args.mode, more), args.binary)
            dest.write_text(json.dumps({**r, **meta, **res}, indent=2) + "\n", encoding="utf-8")
            print(f"{r['instance_id']}: {res['outcome']}")
    lock.close()
    return 0


def collect(root: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    def rows(part: str) -> list[dict[str, Any]]:
        d = root / "results" / part
        return [json.loads(f.read_text(encoding="utf-8")) for f in sorted(d.glob("*.json"))] if d.is_dir() else []

    runs = {"study_id": STUDY_ID, "part1": rows("part1"), "part2": rows("part2")}
    blocks = {
        "study_id": STUDY_ID,
        "blocks": [
            {"population": r["population"], "instance_id": r["instance_id"], "failing_tests": r["outcome"]["failing_counted"], "class": None, "reason": ""}
            for r in runs["part2"]
            if r["outcome"].get("blocked")
        ],
    }
    return runs, blocks


def cmd_collect(args: argparse.Namespace) -> int:
    runs, blocks = collect(args.root)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "runs.json").write_text(json.dumps(runs, indent=2) + "\n", encoding="utf-8")
    dest = args.out / "blocks.json"
    if dest.exists() and not args.force:
        print(f"{dest} exists (the rater's classes); not overwritten without --force", file=sys.stderr)
    else:
        dest.write_text(json.dumps(blocks, indent=2) + "\n", encoding="utf-8")
    print(f"{len(runs['part1'])} Part 1 pairs, {len(runs['part2'])} Part 2 instances, {len(blocks['blocks'])} blocks")
    return 0


def cmd_summarize(args: argparse.Namespace) -> int:
    runs = json.loads((args.out / "runs.json").read_text(encoding="utf-8"))
    bpath = args.out / "blocks.json"
    blocks = json.loads(bpath.read_text(encoding="utf-8")) if bpath.exists() else None
    s = summarize(runs, blocks, load_prereg())
    (args.out / "summary.json").write_text(json.dumps(s, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: v.get("verdict") for k, v in s["part2"].items()}))
    return 0


def pilot_values(summary: dict[str, Any], prereg: dict[str, Any]) -> dict[str, Any]:
    """`[pilot] writes_at_lock` from the pilot's Part 2 counts, through the bound functions of
    `benchmark_power_audit.py` (the Python mirror of `src/guards/perf/bounds.rs`).

    The eligible share and the friction get 95% Wilson intervals. The Part 2 requirement is
    re-derived at the Wilson upper bound of the friction against the ceiling; the Step-0
    verdict compares it with the eligible gold pull requests expected at the observed share
    (floor(share x confirmatory instances)). An upper bound at or above the ceiling, or a
    requirement past `upper_bound_n_max`, is REJECT_UNDETECTABLE."""
    bpa = _load_sibling("benchmark_power_audit")
    pb = prereg["power_and_bounds"]
    conf, power, n_max = pb["confidence_level"], pb["statistical_power"], pb["upper_bound_n_max"]
    ceiling, available = pb["q2_ceiling"], pb["q2_available"]
    pilot = summary["part2"]["pilot"]
    n, eligible = pilot["instances"], pilot["eligible"]
    k = pilot["friction"]["k"]
    share_ci = list(bpa.wilson_score_interval(eligible, n, conf)) if n else [None, None]
    friction_ci = list(bpa.wilson_score_interval(k, eligible, conf)) if eligible else [None, None]
    expected_eligible = int(eligible * available // n) if n else 0
    out: dict[str, Any] = {
        "study_id": STUDY_ID,
        "source": "summary.json (pilot), counts recomputed from runs.json by tests/test_benchmark_power_bounds_base_tests.rs",
        "confidence_level": conf,
        "statistical_power": power,
        "observed_eligible_share": {"k": eligible, "n": n, "point": eligible / n if n else None, "wilson_95": share_ci},
        "observed_friction": {"k": k, "n": eligible, "point": k / eligible if eligible else None, "wilson_95": friction_ci},
        "q2_ceiling": ceiling,
        "q2_available": available,
        "q2_expected_eligible_at_observed_share": expected_eligible,
    }
    rate = friction_ci[1]
    out["q2_planning_rate_at_lock"] = rate
    if rate is None:
        out.update(q2_required_gold_prs=None, q2_step0_verdict="REJECT_UNDETECTABLE", q2_step0_reason="no eligible pilot gold pull request")
    elif rate >= ceiling:
        out.update(q2_required_gold_prs=None, q2_step0_verdict="REJECT_UNDETECTABLE", q2_step0_reason=f"the friction upper bound {rate:.4f} is not below the ceiling {ceiling}")
    else:
        try:
            required = bpa.sample_size_for_wilson_upper_bound_power(rate, ceiling, conf, power, n_max)
        except ValueError as e:
            out.update(q2_required_gold_prs=None, q2_step0_verdict="REJECT_UNDETECTABLE", q2_step0_reason=str(e))
        else:
            ok = required <= expected_eligible
            out.update(
                q2_required_gold_prs=required,
                q2_power_at_expected_eligible=bpa.wilson_upper_pass_probability(expected_eligible, rate, ceiling, conf) if expected_eligible else 0.0,
                q2_step0_verdict="PROCEED" if ok else "REJECT_UNDETECTABLE",
                q2_step0_reason=f"{required} required, {expected_eligible} expected eligible",
            )
    return out


def cmd_pilot_values(args: argparse.Namespace) -> int:
    summary = json.loads((args.out / "summary.json").read_text(encoding="utf-8"))
    v = pilot_values(summary, load_prereg())
    (args.out / "pilot-values.json").write_text(json.dumps(v, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: v[k] for k in ("observed_eligible_share", "observed_friction", "q2_required_gold_prs", "q2_step0_verdict")}))
    return 0


# --- Self-checks ---------------------------------------------------------------------------


def self_test() -> int:
    failures: list[str] = []
    ran: list[str] = []

    def check(name: str, got: Any, want: Any) -> None:
        ran.append(name)
        if got != want:
            failures.append(name)
            print(f"FAIL {name}: {got!r} != {want!r}")

    prereg = load_prereg()
    strata = json.loads(STRATA.read_text(encoding="utf-8"))
    q1 = prereg["q1_feasibility"]
    # The committed population reproduces the pre-registration's counts.
    check("eligible pairs", len(eligible_pairs(strata)), q1["eligible_pairs"])
    check("eligible per repository", eligible_per_owner(strata), dict(sorted(q1["eligible_pairs_per_repository"].items())))
    sample = q1_sample(strata, q1["q1_salt"])
    check("sampled pairs", len(sample), q1["sampled_pairs"])
    # Selection: the smallest ranks, at most two per owner, ties impossible (distinct pair ids).
    synthetic = {"counterfactual": {"pairs": [{"pair_id": f"p{i}", "instance_id": f"o{i % 2}__r-{i}", "stratum": s} for i, s in enumerate(["mixed", "test-side", "mixed", "source-side", "mixed", "out-of-diff", "test-side"])]}}
    want = []
    for o in ("o0", "o1"):
        ranked = sorted((q1_rank("s", f"p{i}"), f"p{i}") for i, s in enumerate(["mixed", "test-side", "mixed", "source-side", "mixed", "out-of-diff", "test-side"]) if f"o{i % 2}" == o and s in ELIGIBLE_STRATA)
        want += [pid for _, pid in ranked[:2]]
    check("q1_sample smallest two", [p["pair_id"] for p in q1_sample(synthetic, "s")], want)
    check("q1_sample one per owner", [p["pair_id"] for p in q1_sample(synthetic, "s", 1)], [want[0], want[2]])
    check("q1_rank is sha256(salt||id)", q1_rank("ab", "cd"), hashlib.sha256(b"abcd").hexdigest())
    # LOCK guard.
    draft = {"study": {"status": "draft"}, "binary": {"sha256": "LOCK", "release": "LOCK"}, "constants": {"timeout_seconds": "LOCK", "container_memory": "4g"}}
    r = confirmatory_refusal(draft, None)
    check("refuses while LOCK", r is not None and "constants.timeout_seconds" in r and "binary.sha256" in r, True)
    locked = {"study": {"status": "locked-pre-data"}, "binary": {"sha256": "ab", "release": "1"}, "constants": {"timeout_seconds": 900, "container_memory": "4g"}}
    check("allowed when locked", confirmatory_refusal(locked, None), None)
    check("refuses an overlay", confirmatory_refusal(locked, {}) is not None, True)
    check("refuses a draft without LOCK", confirmatory_refusal({**locked, "study": {"status": "draft"}}, None) is not None, True)
    check("refuses one LOCK binary value", confirmatory_refusal({**locked, "binary": {"sha256": "LOCK", "release": "1"}}, None) is not None, True)
    check("the committed lock is accepted", confirmatory_refusal(prereg, None), None)
    try:
        effective_constants(prereg, {"timeout": 1})
        check("overlay key checked", "accepted", "refused")
    except ValueError:
        check("overlay key checked", "refused", "refused")
    check("pilot needs globals", pilot_refusal({"timeout_seconds": 1, "flaky_rerun_policy": {}, "image_digests": "counterfactual", "container_memory": LOCK}) is not None, True)
    filled = {k: {"django": "x"} for k in PER_REPOSITORY}
    check("repo constants set", repo_constants(filled, "django"), {k: "x" for k in PER_REPOSITORY})
    check("repo constants unset", repo_constants(filled, "sympy"), None)
    # Globs and test files.
    check("glob **", glob_match("django/contrib/auth/tests.py", "**/tests.py"), True)
    check("glob * stays in one dir", glob_match("a/b/tests.py", "*/tests.py"), False)
    check("glob */tests.py", glob_match("auth/tests.py", "*/tests.py"), True)
    patch = (
        "diff --git a/tests/test_a.py b/tests/test_a.py\n--- a/tests/test_a.py\n+++ b/tests/test_a.py\n@@ -1 +1 @@\n-x\n+y\n"
        "diff --git a/tests/test_new.py b/tests/test_new.py\nnew file mode 100644\n--- /dev/null\n+++ b/tests/test_new.py\n@@ -0,0 +1 @@\n+z\n"
        "diff --git a/pkg/mod.py b/pkg/mod.py\n--- a/pkg/mod.py\n+++ b/pkg/mod.py\n@@ -1 +1 @@\n-a\n+b\n"
        "diff --git a/pkg/checks.py b/pkg/checks.py\n--- a/pkg/checks.py\n+++ b/pkg/checks.py\n@@ -1 +1 @@\n-a\n+b\n"
    )
    check("base test files", base_test_files(patch, []), ["tests/test_a.py"])
    check("base test files with globs", base_test_files(patch, ["pkg/checks.py"]), ["pkg/checks.py", "tests/test_a.py"])
    check("head test files", head_test_files(patch, []), ["tests/test_a.py", "tests/test_new.py"])
    added_only = "diff --git a/tests/test_new.py b/tests/test_new.py\nnew file mode 100644\n--- /dev/null\n+++ b/tests/test_new.py\n@@ -0,0 +1 @@\n+z\n"
    check("only added tests", base_test_files(added_only, []), [])
    # Commands and configuration.
    files = ["tests/auth_tests/test_a.py", "tests/x/__init__.py"]
    check("expand files", expand_command("pytest {junit} {test_files}", "--junitxml=j.xml", files), "pytest --junitxml=j.xml tests/auth_tests/test_a.py tests/x/__init__.py")
    check("expand modules in", expand_command("./runtests.py {test_modules:tests}", "", files), "./runtests.py auth_tests.test_a x")
    check("expand modules", expand_command("r {test_modules}", "", files[:1]), "r tests.auth_tests.test_a")
    for bad in ("r {test_file}", "r {test_modules:}"):
        try:
            expand_command(bad, "", files)
            check(f"unexpanded {bad}", "accepted", "refused")
        except ValueError:
            check(f"unexpanded {bad}", "refused", "refused")
    try:
        expand_command("r {test_files}", "", [])
        check("no files", "accepted", "refused")
    except ValueError:
        check("no files", "refused", "refused")
    cfg = tomllib.loads(discipline_config("pytest 'a b'", ["*/tests.py"], 900))
    check("config preset", (cfg["gates"]["command"]["preset"], cfg["gates"]["command"]["timeout_seconds"], cfg["tests"]["paths"]), ("base-tests", 900, ["*/tests.py"]))
    check("config command", cfg["gates"]["command"]["command"], "bash -c " + shlex.quote(ENV_PREFIX + "pytest 'a b'"))
    check("config no globs", "tests" in tomllib.loads(discipline_config("x", [], 1)), False)
    # JUnit and reports.
    junit = 'noise <testsuite><testcase classname="t.A" name="x"/><testcase name="y"><failure/></testcase></testsuite> tail'
    check("junit in noise", junit_cases(junit), ({"t.A::x": "passed", "y": "failed"}, 0))
    check("junit broken is counted", junit_cases("<testsuite><testcase></testsuite>"), ({}, 1))

    def report(*violations: tuple[str, str], notes: tuple[str, ...] = ()) -> dict[str, Any]:
        return {"outcomes": [{"gate": "command", "notes": list(notes), "violations": [{"code": c, "message": m} for c, m in violations]}]}

    rep = report((BASE_TEST_FAILED, "Base test `t.A::x` failed when executed against head code: boom"), ("assertion-reduction/assertions-reduced", "m"))
    check("report failed ids", read_report(rep)["failed_ids"], ["t.A::x"])
    check("report other findings", read_report(rep)["other_findings"], ["assertion-reduction/assertions-reduced"])
    check("report suite failed", read_report(report((BASE_TEST_FAILED, "Base test suite failed against head code (exit code Some(1)):\nE")))["suite_failed"], True)
    check("report passed note", read_report(report(notes=("command: preset 'base-tests': 7 base tests passed against head code",)))["passed_count"], 7)
    check("report no cases", read_report(report((BASE_TESTS_NO_CASES, "m")))["no_cases"], True)
    # Eligibility and outcomes.
    ctl = {"cases": {"a": "passed", "b": "failed", "c": "passed"}}
    check("eligible", eligibility(["t.py"], ctl), (True, None))
    check("no base test file", eligibility([], ctl), (False, "no-base-test-file-modified"))
    check("control no case", eligibility(["t.py"], {"cases": {}}), (False, "control-no-case"))
    check("control none passing", eligibility(["t.py"], {"cases": {"a": "failed", "b": "skipped"}}), (False, "control-no-passing-test"))
    check("control error", eligibility(["t.py"], {"error": "x"}), (False, "run-error"))
    gold_fail = {"attempts": [{"report": report((BASE_TEST_FAILED, "Base test `a` failed when executed against head code: m"), (BASE_TEST_FAILED, "Base test `b` failed when executed against head code: m"))}]}
    o = instance_outcome(["t.py"], ctl, gold_fail)
    check("blocked on a counted test", (o["blocked"], o["failing_counted"], o["failing_uncounted"], o["excluded_tests"], o["counted_tests"]), (True, ["a"], ["b"], ["b"], 2))
    only_uncounted = {"attempts": [{"report": report((BASE_TEST_FAILED, "Base test `b` failed when executed against head code: m"))}]}
    check("uncounted failure does not block", instance_outcome(["t.py"], ctl, only_uncounted)["blocked"], False)
    flaky = {"attempts": [gold_fail["attempts"][0], {"report": report(notes=("command: preset 'base-tests': 2 base tests passed against head code",))}]}
    module_fail = {"attempts": [{"report": report((BASE_TEST_FAILED, "Base test `t` failed when executed against head code: collection failure"))}]}
    ctl_mod = {"cases": {"t::A::x": "passed", "t.A::y": "passed", "tx::z": "passed", "u::w": "failed"}}
    check("a module failure covers its counted tests", instance_outcome(["t.py"], ctl_mod, module_fail)["failing_counted"], ["t.A::y", "t::A::x"])
    suite_fail = {"attempts": [{"report": report((BASE_TEST_FAILED, "Base test suite failed against head code (exit code Some(4)):\nE"))}]}
    check("a suite failure covers every counted test", instance_outcome(["t.py"], ctl_mod, suite_fail)["failing_counted"], ["t.A::y", "t::A::x", "tx::z"])
    check("a rerun that passes lifts the block", instance_outcome(["t.py"], ctl, flaky)["blocked"], False)
    check("gold error", instance_outcome(["t.py"], ctl, {"error": "x"})["reason"], "run-error")
    # Verdict rule, against the ceiling 0.15.
    check("LOW_FRICTION", q2_verdict(2, 100, 0.15), "LOW_FRICTION")
    check("HIGH_FRICTION", q2_verdict(40, 100, 0.15), "HIGH_FRICTION")
    check("INTERMEDIATE", q2_verdict(12, 100, 0.15), "INTERMEDIATE")
    check("no eligible", q2_verdict(0, 0, 0.15), "INTERMEDIATE")
    check("estimate-only has no ceiling verdict", q2_verdict(2, 100, 0.15, "estimate-only"), "ESTIMATE_ONLY")
    # Part 1 outcome and the summary shape.
    ran_pair = {"report": rep}
    check("pair runs and blocks", pair_outcome(ran_pair), {"runs": True, "cause": None, "blocked": True})
    check("pair no cases", pair_outcome({"report": report((BASE_TESTS_NO_CASES, "m"))})["runs"], False)
    check("pair error", pair_outcome({"error": "patch"})["runs"], False)
    runs = {
        "part1": [
            {"owner": "astropy", "result": ran_pair, "tests_paths_globs": []},
            {"owner": "django", "result": ran_pair, "tests_paths_globs": ["*/tests.py"]},
            {"owner": "sympy", "result": {"error": "x"}, "tests_paths_globs": []},
        ],
        "part2": [
            {"population": "confirmatory", "instance_id": "a__a-1", "owner": "a", "outcome": instance_outcome(["t.py"], ctl, gold_fail)},
            {"population": "confirmatory", "instance_id": "a__a-2", "owner": "a", "outcome": instance_outcome(["t.py"], ctl, only_uncounted)},
            {"population": "confirmatory", "instance_id": "b__b-1", "owner": "b", "outcome": instance_outcome([], None, None)},
            {"population": "pilot", "instance_id": "a__a-3", "owner": "a", "outcome": instance_outcome(["t.py"], ctl, gold_fail)},
        ],
    }
    blocks = {"blocks": [{"population": "confirmatory", "instance_id": "a__a-1", "class": "behaviour-change"}]}
    s = summarize(runs, blocks, prereg)
    check("summary part1", {o: v["verdict"] for o, v in s["part1"].items()}, {"astropy": "RUNS", "django": "RUNS_WITH_PATHS", "sympy": "DOES_NOT_RUN"})
    c2 = s["part2"]["confirmatory"]
    check("summary part2", (c2["instances"], c2["eligible"], c2["friction"]["k"], c2["not_eligible"]["no-base-test-file-modified"], c2["block_classes"]["behaviour-change"], c2["unclassified_blocks"]), (3, 2, 1, 1, 0 + 1, 0))
    check("summary confirmatory is estimate-only", c2["verdict"], "ESTIMATE_ONLY")
    check("summary pilot apart", (s["part2"]["pilot"]["instances"], s["part2"]["pilot"]["verdict"]), (1, "pilot: no verdict"))
    try:
        summarize(runs, {"blocks": [{"population": "confirmatory", "instance_id": "a__a-1", "class": "noise"}]}, prereg)
        check("summary refuses a bad class", "accepted", "refused")
    except ValueError:
        check("summary refuses a bad class", "refused", "refused")
    # The plan from the committed files.
    p = plan(prereg, strata)
    check("plan part2 instances", p["part2"]["instances"], {"confirmatory": prereg["population"]["confirmatory_instances"], "pilot": prereg["population"]["pilot_instances"]})
    check("plan accepts confirmatory once locked", p["confirmatory_refusal"], None)
    digest = strata["counterfactual"]["pairs"][0]
    check("image from the counterfactual record", image_for(digest["instance_id"], {}, strata, "confirmatory"), digest["image_digest"])
    check("pilot image falls back to the tag", image_for("x__y-1", {}, strata, "pilot"), cf.image_name("x__y-1"))
    try:
        image_for("x__y-1", {}, strata, "confirmatory")
        check("confirmatory needs a digest", "accepted", "refused")
    except RuntimeError:
        check("confirmatory needs a digest", "refused", "refused")
    # Pilot values: the re-derivation at the friction's Wilson upper bound.
    def fake(n: int, e: int, k: int) -> dict[str, Any]:
        return {"part2": {"pilot": {"instances": n, "eligible": e, "friction": {"k": k}}}}

    pv = pilot_values(fake(77, 60, 0), prereg)
    check("pilot k=0 upper bound", round(pv["observed_friction"]["wilson_95"][1], 4), 0.0602)
    check("pilot k=0 proceeds", pv["q2_step0_verdict"], "PROCEED")
    check("pilot expected eligible", pv["q2_expected_eligible_at_observed_share"], 60 * 272 // 77)
    check("pilot upper bound past the ceiling", pilot_values(fake(77, 60, 6), prereg)["q2_step0_verdict"], "REJECT_UNDETECTABLE")
    check("pilot no eligible", pilot_values(fake(77, 0, 0), prereg)["q2_step0_verdict"], "REJECT_UNDETECTABLE")
    print(f"{'FAILED' if failures else 'ok'}: {len(ran)} checks")
    return 1 if failures else 0


def main() -> int:
    if "--test" in sys.argv:
        return self_test()
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    pl = sub.add_parser("plan", help="print the Part 1 sample and the Part 2 plan (no Docker)")
    pl.add_argument("--dry-run", action="store_true", help="accepted for symmetry with `run --dry-run`")
    pl.add_argument("--json", action="store_true")
    pl.add_argument("--constants", type=Path)
    rn = sub.add_parser("run", help="run Part 1 or Part 2 on the lab host")
    rn.add_argument("--mode", choices=("pilot", "confirmatory"), required=True)
    rn.add_argument("--part", type=int, choices=(1, 2), default=2)
    rn.add_argument("--dry-run", action="store_true", help="print the plan and exit; no Docker")
    rn.add_argument("--json", action="store_true")
    rn.add_argument("--constants", type=Path, help="pilot only: candidate [constants] values (JSON)")
    rn.add_argument("--dataset", type=Path)
    rn.add_argument("--binary", type=Path, help="the static Linux discipline binary")
    rn.add_argument("--root", type=Path, default=Path("/data/base-tests"))
    rn.add_argument("--lock-file", default=LOCK_FILE)
    rn.add_argument("--only", nargs="*", default=[])
    rn.add_argument("--force", action="store_true")
    co = sub.add_parser("collect", help="write runs.json and blocks.json from the result files")
    co.add_argument("--root", type=Path, default=Path("/data/base-tests"))
    co.add_argument("--out", type=Path, default=RESULTS_DIR)
    co.add_argument("--force", action="store_true", help="overwrite blocks.json")
    sm = sub.add_parser("summarize", help="write summary.json from runs.json and blocks.json")
    sm.add_argument("--out", type=Path, default=RESULTS_DIR)
    pv = sub.add_parser("pilot-values", help="write pilot-values.json (`[pilot] writes_at_lock`) from summary.json")
    pv.add_argument("--out", type=Path, default=RESULTS_DIR / "pilot")
    args = ap.parse_args()
    if args.cmd == "plan":
        overlay = json.loads(args.constants.read_text(encoding="utf-8")) if args.constants else None
        print_plan(plan(load_prereg(), json.loads(STRATA.read_text(encoding="utf-8")), overlay), args.json)
        return 0
    return {"run": cmd_run, "collect": cmd_collect, "summarize": cmd_summarize, "pilot-values": cmd_pilot_values}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
