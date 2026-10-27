#!/usr/bin/env python3
"""Plan and judge the rotating, sharded cargo-mutants run against the surviving-mutant ratchet.

`.github/workflows/mutants.yml` (docs/ARCHITECTURE.md §8.7) does not test every mutant
of `src/ast/` and `src/guards/` in one run: that does not fit the hosted runners (#869).
Each file belongs to one of `slices` slices, and each run tests one slice, plus the
mutants in the code changed since the previous measured commit (the diff slice).

The ratchet, `tests/fixtures/mutants_ratchet.json`:

  {"description": "...", "slices": 16, "previous_commit": "<sha>" | null,
   "files": {"src/ast/x.rs": {"slice": 3, "missed": 2 | null, "run_id": 123 | null}}}

- `slice` is the file's slice. A file the ratchet does not list, or lists without a
  slice, is given the slice with the fewest mutants (largest file first) by `--plan`;
  the computed ratchet keeps the choice, so a file never moves on its own.
- `missed` is the number of surviving mutants its slice's last run found, and `run_id`
  that run. `null` means not measured yet: such a file cannot fail a run. Bootstrapping
  is complete once every file has a count, that is once every slice has run.
- `previous_commit` is the commit of the last measured run; the diff slice re-tests the
  mutants in `git diff <previous_commit>..HEAD -- src/ast src/guards`.

Modes:

  --sizing                 the slice count derived from the measured cost (#869) and the
                           workflow's limits, as JSON.
  --print-previous-commit  the ratchet's `previous_commit`, or an empty line.
  --plan                   pick this run's slice (by the run's ISO week, or `--slice`),
                           assign new files, size the diff slice, and write `plan.json`
                           and `slice-files.txt` to `--plan-out`.
  --shard-plan PLAN --shard K
                           print "<slice mutants> <diff mutants>" for shard K.
  --outcomes-dir DIR --plan-file PLAN ...
                           judge the run: read each shard's `slice/outcomes.json` and
                           `diff/outcomes.json`, compare with the ratchet, and write the
                           computed ratchet, updated for the slice's files only.
  --validate FILE          check that FILE parses as a ratchet.
  --self-check             run the embedded cases.

Judging: a slice file above its count fails the run, below it can be lowered. A file in
the diff slice whose survivors in the changed code alone exceed its count fails too: a
part of a file cannot have more survivors than the whole. A diff-slice count is never
written to the ratchet, since it covers only part of a file.

Fail-closed: a shard job that did not succeed; a shard whose outcomes are missing,
malformed, unfinished (`end_time` is null), with a failed baseline, from another
cargo-mutants version, or with another number of mutants than the plan expects; or a
plan or ratchet that cannot be read: exit 2. Missing data is never read as zero
survivors. A file whose slice has not run yet is not missing data.

`outcomes.json` is cargo-mutants' `LabOutcome` (src/outcome.rs, src/mutant.rs and
src/span.rs of cargo-mutants; book/src/mutants-out.md): `outcomes[]` holds one entry
per scenario with `scenario` (`"Baseline"`, or `{"Mutant": {"name", "file", "span":
{"start": {"line", "column"}}, ...}}`) and `summary` (`CaughtMutant`, `MissedMutant`,
`Timeout`, `Unviable`, `Success`, `Failure`), beside `total_mutants`, `end_time` and
`cargo_mutants_version`. `cargo mutants --list --json` prints a list of the same
mutant objects.

Exit codes: 0 pass, 1 a file is above the ratchet, 2 the data could not be read.
Stdlib only, so it runs on a bare runner image.
"""

import argparse
import datetime
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Set, Tuple

EXIT_PASS = 0
EXIT_ABOVE = 1
EXIT_UNREADABLE = 2

ARTIFACT_PREFIX = "mutants-shard-"
KINDS = ("slice", "diff")

# Sizing (#869). The first dispatched run (40235664265) found 16 shards of 1,283
# mutants, so about 20,500 in all; its shard 0 logged a 119 s baseline build and a 73 s
# lib-test run, and each shard tested about a tenth of its mutants before the step
# limit, about 150 s per mutant (an incremental build plus the lib tests).
OBSERVED_TOTAL_MUTANTS = 20500
OBSERVED_SECONDS_PER_MUTANT = 150
OBSERVED_BASELINE_SECONDS = 119 + 73
# The workflow's limits: `MUTANTS_SHARDS`, the `timeout-minutes` of the step that runs
# cargo-mutants and of its job, and the job limit of a GitHub-hosted runner.
SHARDS = 16
STEP_TIMEOUT_MINUTES = 320
JOB_TIMEOUT_MINUTES = 340
HOSTED_JOB_LIMIT_MINUTES = 360
# Each shard runs cargo-mutants twice, for the slice and for the diff slice, and each
# run builds and tests the unmutated tree first.
INVOCATIONS_PER_SHARD = 2
# Share of the step budget left unused, against per-mutant cost varying around the mean.
SAFETY_PERCENT = 10
# Share of a shard's capacity kept for the diff slice when the slice count is chosen.
DIFF_RESERVE_PERCENT = 25

# Weeks are counted from this Monday, so a week runs Monday to Sunday (UTC), as an ISO
# week does, without the 52/53-week wrap of ISO week numbers at a year's end.
ROTATION_EPOCH = datetime.date(1970, 1, 5)

COMMIT_RE = re.compile(r"[0-9a-f]{40}")

RATCHET_DESCRIPTION = (
    "Surviving (missed) cargo-mutants mutants per source file under src/ast/ and "
    "src/guards/, written by scripts/mutants_ratchet.py from a run of "
    ".github/workflows/mutants.yml (docs/ARCHITECTURE.md §8.7). Each run tests one "
    "slice of the files: `slice` is the file's slice, `missed` the count its slice's "
    "last run found and `run_id` that run; `missed: null` is not measured yet. "
    "`previous_commit` is the last measured run's commit, the base of the diff slice. "
    "The counts come from unit tests only (`--cargo-test-arg=--lib`): a survivor that "
    "an integration test kills is counted, and is accepted here with that note until "
    "the integration-test mapping (§8.7) re-tests it."
)


class DataError(Exception):
    """The run's data is missing, malformed or incomplete: fail closed."""


# ---------------------------------------------------------------------------------
# Sizing


@dataclass(frozen=True)
class Sizing:
    step_seconds: int
    budget_seconds: int
    usable_seconds: int
    per_shard_capacity: int
    slice_per_shard: int
    slice_capacity: int
    slices: int
    mean_slice_mutants: int
    mean_slice_per_shard: int
    run_capacity: int
    full_sweep_per_shard: int


def ceil_div(a: int, b: int) -> int:
    return -(-a // b)


def sizing(
    total_mutants: int = OBSERVED_TOTAL_MUTANTS,
    seconds_per_mutant: int = OBSERVED_SECONDS_PER_MUTANT,
    baseline_seconds: int = OBSERVED_BASELINE_SECONDS,
    shards: int = SHARDS,
    step_minutes: int = STEP_TIMEOUT_MINUTES,
    invocations: int = INVOCATIONS_PER_SHARD,
    safety_percent: int = SAFETY_PERCENT,
    diff_reserve_percent: int = DIFF_RESERVE_PERCENT,
) -> Sizing:
    """The smallest slice count whose average slice fits a shard's step with the reserves.

    A shard's step has `step_minutes`; `invocations` baselines come off it, then
    `safety_percent`. What is left, divided by the cost of one mutant, is the shard's
    capacity. `diff_reserve_percent` of it is kept for the diff slice; the rest, times
    the shards, is what one slice may hold, and the slice count covers every mutant.
    """
    for name, value in (("total_mutants", total_mutants), ("seconds_per_mutant", seconds_per_mutant),
                        ("shards", shards), ("step_minutes", step_minutes), ("invocations", invocations)):
        if not isinstance(value, int) or value < 1:
            raise ValueError(f"{name} must be a positive integer")
    if not isinstance(baseline_seconds, int) or baseline_seconds < 0:
        raise ValueError("baseline_seconds must be a non-negative integer")
    for name, value in (("safety_percent", safety_percent), ("diff_reserve_percent", diff_reserve_percent)):
        if not isinstance(value, int) or not 0 <= value < 100:
            raise ValueError(f"{name} must be an integer in 0..99")
    step_seconds = step_minutes * 60
    budget = step_seconds - invocations * baseline_seconds
    if budget <= 0:
        raise ValueError("the baselines alone fill the step")
    usable = budget * (100 - safety_percent) // 100
    per_shard = usable // seconds_per_mutant
    slice_per_shard = per_shard * (100 - diff_reserve_percent) // 100
    if slice_per_shard < 1:
        raise ValueError("a shard has no room for a mutant")
    slice_capacity = slice_per_shard * shards
    slices = ceil_div(total_mutants, slice_capacity)
    mean = ceil_div(total_mutants, slices)
    return Sizing(
        step_seconds=step_seconds,
        budget_seconds=budget,
        usable_seconds=usable,
        per_shard_capacity=per_shard,
        slice_per_shard=slice_per_shard,
        slice_capacity=slice_capacity,
        slices=slices,
        mean_slice_mutants=mean,
        mean_slice_per_shard=ceil_div(mean, shards),
        run_capacity=per_shard * shards,
        full_sweep_per_shard=ceil_div(total_mutants, shards),
    )


# ---------------------------------------------------------------------------------
# Ratchet


@dataclass
class Entry:
    slice: Optional[int]
    missed: Optional[int]
    run_id: Optional[int]


@dataclass
class Ratchet:
    slices: int
    previous_commit: Optional[str]
    files: Dict[str, Entry]


def load_json(path: Path) -> object:
    try:
        with path.open(encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        raise DataError(f"{path}: not found") from None
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise DataError(f"{path}: cannot be read as JSON ({e})") from None


def is_count(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def load_ratchet(path: Path) -> Ratchet:
    """The committed ratchet; anything outside the schema in the module docstring is an error."""
    data = load_json(path)
    if not isinstance(data, dict):
        raise DataError(f"{path}: the ratchet is not a JSON object")
    if "bootstrap" in data or "missed" in data:
        raise DataError(f"{path}: a ratchet without slices (top-level `missed` or `bootstrap`); "
                        "the rotating run keeps a count per file under `files`")
    slices = data.get("slices")
    if not is_count(slices) or slices < 1:
        raise DataError(f"{path}: `slices` is not a positive integer")
    previous = data.get("previous_commit")
    if previous is not None and (not isinstance(previous, str) or not COMMIT_RE.fullmatch(previous)):
        raise DataError(f"{path}: `previous_commit` is not null or a 40-character commit id")
    files = data.get("files")
    if not isinstance(files, dict):
        raise DataError(f"{path}: `files` is not an object of file to entry")
    entries: Dict[str, Entry] = {}
    for file, entry in files.items():
        if not isinstance(entry, dict) or set(entry) - {"slice", "missed", "run_id"}:
            raise DataError(f"{path}: the entry of {file} is not an object of `slice`, `missed`, `run_id`")
        s, missed, run_id = entry.get("slice"), entry.get("missed"), entry.get("run_id")
        if s is not None and (not is_count(s) or s >= slices):
            raise DataError(f"{path}: `slice` of {file} is not null or in 0..{slices - 1}")
        if missed is not None and not is_count(missed):
            raise DataError(f"{path}: `missed` of {file} is not null or a non-negative integer")
        if run_id is not None and not is_count(run_id):
            raise DataError(f"{path}: `run_id` of {file} is not null or a non-negative integer")
        if missed is not None and run_id is None:
            raise DataError(f"{path}: {file} has a count but no `run_id`")
        entries[file] = Entry(slice=s, missed=missed, run_id=run_id)
    return Ratchet(slices=slices, previous_commit=previous, files=entries)


def ratchet_json(r: Ratchet) -> str:
    files = {f: {"slice": e.slice, "missed": e.missed, "run_id": e.run_id} for f, e in sorted(r.files.items())}
    data = {"description": RATCHET_DESCRIPTION, "slices": r.slices,
            "previous_commit": r.previous_commit, "files": files}
    return json.dumps(data, indent=2, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------------------------
# Plan


def week_index(created: str) -> int:
    """The week of a UTC timestamp (`2026-10-24T03:31:00Z`), counted from ROTATION_EPOCH."""
    try:
        when = datetime.datetime.strptime(created, "%Y-%m-%dT%H:%M:%SZ")
    except (TypeError, ValueError):
        raise DataError(f"the run's creation time {created!r} is not YYYY-MM-DDTHH:MM:SSZ") from None
    return (when.date() - ROTATION_EPOCH).days // 7


def rotation_slice(week: int, slices: int) -> int:
    """The slice a week tests: consecutive weeks walk every slice once in `slices` weeks."""
    return week % slices


def mutant_files(path: Path, allow_empty: bool) -> Dict[str, int]:
    """Mutants per file of a `cargo mutants --list --json` output.

    cargo-mutants prints nothing, and exits 0, when `--in-diff` gets an empty diff or one
    that touches no mutant; only the diff list may be empty for that reason.
    """
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        raise DataError(f"{path}: not found") from None
    except (OSError, UnicodeDecodeError) as e:
        raise DataError(f"{path}: cannot be read ({e})") from None
    if not text.strip():
        if allow_empty:
            return {}
        raise DataError(f"{path}: the mutant list is empty")
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        raise DataError(f"{path}: cannot be read as JSON ({e})") from None
    if not isinstance(data, list):
        raise DataError(f"{path}: not a list of mutants")
    counts: Dict[str, int] = {}
    for m in data:
        file = m.get("file") if isinstance(m, dict) else None
        if not isinstance(file, str) or not file:
            raise DataError(f"{path}: a mutant has no `file`")
        counts[file] = counts.get(file, 0) + 1
    if not counts and not allow_empty:
        raise DataError(f"{path}: the mutant list is empty")
    return dict(sorted(counts.items()))


def assign_slices(scope: Dict[str, int], ratchet: Ratchet) -> Dict[str, int]:
    """Every file in scope with its slice: the ratchet's, else the lightest slice.

    Files without a slice are placed largest first (by mutants, then by path), each on
    the slice holding the fewest mutants so far (lowest index on a tie), so the result
    depends only on the ratchet and the mutant list.
    """
    assignment: Dict[str, int] = {}
    load = [0] * ratchet.slices
    for file, count in scope.items():
        entry = ratchet.files.get(file)
        if entry is not None and entry.slice is not None:
            assignment[file] = entry.slice
            load[entry.slice] += count
    for file in sorted((f for f in scope if f not in assignment), key=lambda f: (-scope[f], f)):
        s = min(range(ratchet.slices), key=lambda i: (load[i], i))
        assignment[file] = s
        load[s] += scope[file]
    return dict(sorted(assignment.items()))


def shard_sizes(n: int, shards: int) -> List[int]:
    """Mutants per shard under cargo-mutants' default `--sharding slice` (src/shard.rs):
    consecutive chunks of ceil(n / shards)."""
    chunk = ceil_div(n, shards) if n else 0
    return [max(0, min(n, (k + 1) * chunk) - k * chunk) for k in range(shards)]


def make_plan(ratchet: Ratchet, scope: Dict[str, int], slice_index: int, week: Optional[int],
              diff: Optional[Dict[str, int]], diff_skipped: Optional[str], shards: int,
              per_shard_capacity: int) -> dict:
    if shards < 1:
        raise DataError("--shards must be at least 1")
    if not 0 <= slice_index < ratchet.slices:
        raise DataError(f"slice {slice_index} is not in 0..{ratchet.slices - 1}")
    assignment = assign_slices(scope, ratchet)
    slice_files = [f for f, s in assignment.items() if s == slice_index]
    slice_mutants = sum(scope[f] for f in slice_files)
    slice_sizes = shard_sizes(slice_mutants, shards)
    if slice_sizes[0] > per_shard_capacity:
        raise DataError(
            f"slice {slice_index} holds {slice_mutants} mutants, {slice_sizes[0]} per shard, more than a "
            f"shard's capacity of {per_shard_capacity}: raise `slices` in the ratchet (docs/ARCHITECTURE.md §8.7)")

    base = ratchet.previous_commit
    note: Optional[str] = None
    diff_mutants = 0
    if base is None:
        note = "no previous measured commit, so the diff slice is skipped"
    elif diff_skipped is not None:
        note = diff_skipped
    elif diff is None:
        raise DataError(f"the ratchet holds the previous commit {base}, but no diff mutant list was given")
    else:
        in_slice = set(slice_files)
        diff_mutants = sum(n for f, n in diff.items() if f not in in_slice)
        room = per_shard_capacity - slice_sizes[0]
        if diff_mutants == 0:
            note = f"the diff since {base} holds no mutant outside the slice"
        elif ceil_div(diff_mutants, shards) > room:
            note = (f"the diff since {base} holds {diff_mutants} mutants outside the slice, "
                    f"{ceil_div(diff_mutants, shards)} per shard, more than the {room} left per shard; "
                    "the diff slice is skipped and its files are measured by their slices")
            diff_mutants = 0
    run_diff = diff_mutants > 0
    diff_sizes = shard_sizes(diff_mutants, shards)
    return {
        "slices": ratchet.slices,
        "slice": slice_index,
        "week": week,
        "shards": shards,
        "per_shard_capacity": per_shard_capacity,
        "assignment": assignment,
        "slice_files": slice_files,
        "slice_mutants": slice_mutants,
        "diff": {"base": base, "run": run_diff, "mutants": diff_mutants, "note": note},
        "expect": [{"slice": a, "diff": b} for a, b in zip(slice_sizes, diff_sizes)],
    }


def load_plan(path: Path) -> dict:
    plan = load_json(path)
    try:
        assert isinstance(plan, dict)
        shards, slices, s = plan["shards"], plan["slices"], plan["slice"]
        assert is_count(shards) and shards >= 1 and is_count(slices) and slices >= 1
        assert is_count(s) and s < slices
        assignment = plan["assignment"]
        assert isinstance(assignment, dict)
        assert all(isinstance(f, str) and is_count(v) and v < slices for f, v in assignment.items())
        slice_files = plan["slice_files"]
        assert isinstance(slice_files, list)
        assert sorted(slice_files) == sorted(f for f, v in assignment.items() if v == s)
        expect = plan["expect"]
        assert isinstance(expect, list) and len(expect) == shards
        assert all(isinstance(e, dict) and is_count(e.get("slice")) and is_count(e.get("diff")) for e in expect)
        assert sum(e["slice"] for e in expect) == plan["slice_mutants"]
        d = plan["diff"]
        assert isinstance(d, dict) and is_count(d["mutants"]) and isinstance(d["run"], bool)
        assert sum(e["diff"] for e in expect) == d["mutants"]
        assert d["run"] == (d["mutants"] > 0)
        assert d["base"] is None or (isinstance(d["base"], str) and COMMIT_RE.fullmatch(d["base"]))
    except (AssertionError, KeyError, TypeError):
        raise DataError(f"{path}: not a plan written by --plan") from None
    return plan


def render_plan(plan: dict, scope: Dict[str, int]) -> str:
    d = plan["diff"]
    when = f"week {plan['week']}" if plan["week"] is not None else "chosen by the `slice` input"
    out = ["## Mutation run plan", "",
           f"- Slice {plan['slice']} of {plan['slices']} ({when}): {len(plan['slice_files'])} file(s), "
           f"{plan['slice_mutants']} mutants, at most {plan['expect'][0]['slice']} per shard "
           f"(capacity {plan['per_shard_capacity']}).",
           ]
    if d["run"]:
        out.append(f"- Diff slice since `{d['base']}`: {d['mutants']} mutants outside the slice.")
    else:
        out.append(f"- Diff slice skipped: {d['note']}.")
    out += ["", "| File | Mutants |", "|---|---|"]
    out += [f"| `{f}` | {scope[f]} |" for f in plan["slice_files"]]
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------------
# Judging


@dataclass
class Survivor:
    file: str
    line: int
    mutation: str
    kind: str = "slice"


@dataclass
class ShardCounts:
    shard: int
    kind: str
    mutants: int = 0
    caught: int = 0
    missed: int = 0
    timeout: int = 0
    unviable: int = 0


@dataclass
class RunData:
    shards: List[ShardCounts] = field(default_factory=list)
    survivors: List[Survivor] = field(default_factory=list)
    timeouts: List[Survivor] = field(default_factory=list)


@dataclass
class Verdict:
    above: List[Tuple[str, int, int, str]] = field(default_factory=list)
    can_lower: List[Tuple[str, int, int]] = field(default_factory=list)
    equal: List[Tuple[str, int]] = field(default_factory=list)
    first: List[Tuple[str, int]] = field(default_factory=list)
    diff_unmeasured: List[Tuple[str, int]] = field(default_factory=list)

    @property
    def failed(self) -> bool:
        return bool(self.above)


def mutation_text(mutant: dict, file: str) -> str:
    """The mutation from the mutant's `name` (`file:line:col: mutation`) without its location."""
    name = mutant.get("name")
    if not isinstance(name, str) or not name:
        raise DataError(f"a mutant in {file} has no `name`")
    prefix = f"{file}:"
    if name.startswith(prefix):
        rest = name[len(prefix):]
        parts = rest.split(": ", 1)
        if len(parts) == 2:
            return parts[1]
    return name


def read_mutant(entry: dict, where: str, kind: str) -> Survivor:
    mutant = entry["scenario"]["Mutant"]
    if not isinstance(mutant, dict):
        raise DataError(f"{where}: a mutant scenario is not an object")
    file = mutant.get("file")
    try:
        line = mutant["span"]["start"]["line"]
    except (KeyError, TypeError):
        line = None
    if not isinstance(file, str) or not file or not isinstance(line, int):
        raise DataError(f"{where}: a mutant has no `file` or `span.start.line`")
    return Survivor(file=file, line=line, mutation=mutation_text(mutant, file), kind=kind)


def read_shard(path: Path, shard: int, kind: str, expect_version: Optional[str]
               ) -> Tuple[ShardCounts, List[Survivor], List[Survivor], Set[str]]:
    """One shard's counts, survivors, timeouts and mutated files; incomplete data raises."""
    data = load_json(path)
    if not isinstance(data, dict) or not isinstance(data.get("outcomes"), list):
        raise DataError(f"{path}: no `outcomes` list")
    if data.get("end_time") is None:
        raise DataError(f"{path}: the run did not finish (`end_time` is null)")
    version = data.get("cargo_mutants_version")
    if expect_version is not None and version != expect_version:
        raise DataError(f"{path}: written by cargo-mutants {version}, expected {expect_version}")
    total = data.get("total_mutants")
    if not isinstance(total, int):
        raise DataError(f"{path}: no `total_mutants`")

    counts = ShardCounts(shard=shard, kind=kind)
    survivors: List[Survivor] = []
    timeouts: List[Survivor] = []
    files: Set[str] = set()
    for entry in data["outcomes"]:
        if not isinstance(entry, dict) or "scenario" not in entry or "summary" not in entry:
            raise DataError(f"{path}: an outcome has no `scenario` or `summary`")
        scenario, summary = entry["scenario"], entry["summary"]
        if scenario == "Baseline":
            if summary != "Success":
                raise DataError(f"{path}: the baseline did not pass ({summary})")
            continue
        if not isinstance(scenario, dict) or "Mutant" not in scenario:
            raise DataError(f"{path}: unknown scenario {scenario!r}")
        counts.mutants += 1
        mutant = read_mutant(entry, str(path), kind)
        files.add(mutant.file)
        if summary == "CaughtMutant":
            counts.caught += 1
        elif summary == "MissedMutant":
            counts.missed += 1
            survivors.append(mutant)
        elif summary == "Timeout":
            counts.timeout += 1
            timeouts.append(mutant)
        elif summary == "Unviable":
            counts.unviable += 1
        else:
            raise DataError(f"{path}: a mutant has the outcome {summary!r}, not one a test run gives")
    if counts.mutants != total:
        raise DataError(f"{path}: {counts.mutants} mutant outcomes, but `total_mutants` is {total}")
    return counts, survivors, timeouts, files


def read_run(outcomes_dir: Path, plan: dict, expect_version: Optional[str]) -> RunData:
    """Every shard's outcomes the plan expects, from `<dir>/mutants-shard-<k>/<kind>/outcomes.json`."""
    run = RunData()
    problems: List[str] = []
    in_slice = set(plan["slice_files"])
    for k, expect in enumerate(plan["expect"]):
        for kind in KINDS:
            want = expect[kind]
            if want == 0:
                continue
            path = outcomes_dir / f"{ARTIFACT_PREFIX}{k}" / kind / "outcomes.json"
            try:
                counts, survivors, timeouts, files = read_shard(path, k, kind, expect_version)
                if counts.mutants != want:
                    raise DataError(f"{path}: {counts.mutants} mutants, the plan expects {want}")
                stray = sorted(files - in_slice) if kind == "slice" else sorted(files & in_slice)
                if stray:
                    raise DataError(f"{path}: mutants in {', '.join(stray)}, "
                                    f"{'outside' if kind == 'slice' else 'inside'} the slice")
            except DataError as e:
                problems.append(f"shard {k} ({kind}): {e}")
                continue
            run.shards.append(counts)
            run.survivors.extend(survivors)
            run.timeouts.extend(timeouts)
    if problems:
        raise DataError("\n".join(problems))
    return run


def judge(plan: dict, ratchet: Ratchet, run: RunData, run_id: int, commit: Optional[str]
          ) -> Tuple[Verdict, Ratchet]:
    """Compare the slice's and the diff slice's survivors with the ratchet; compute the new one.

    Only the slice's files take a new count and the run id. Every other file keeps its
    entry; a file that left the scope is dropped. A diff-slice count is compared, never
    written.
    """
    verdict = Verdict()
    slice_files = plan["slice_files"]
    slice_counts = {f: 0 for f in slice_files}
    diff_counts: Dict[str, int] = {}
    for s in run.survivors:
        if s.kind == "slice":
            slice_counts[s.file] += 1
        else:
            diff_counts[s.file] = diff_counts.get(s.file, 0) + 1

    def recorded(file: str) -> Optional[int]:
        entry = ratchet.files.get(file)
        return entry.missed if entry is not None else None

    for file in sorted(slice_counts):
        found, allowed = slice_counts[file], recorded(file)
        if allowed is None:
            verdict.first.append((file, found))
        elif found > allowed:
            verdict.above.append((file, found, allowed, "slice"))
        elif found < allowed:
            verdict.can_lower.append((file, found, allowed))
        else:
            verdict.equal.append((file, found))
    for file in sorted(diff_counts):
        found, allowed = diff_counts[file], recorded(file)
        if allowed is None:
            verdict.diff_unmeasured.append((file, found))
        elif found > allowed:
            verdict.above.append((file, found, allowed, "diff"))

    files: Dict[str, Entry] = {}
    for file, s in plan["assignment"].items():
        old = ratchet.files.get(file)
        if file in slice_counts:
            files[file] = Entry(slice=s, missed=slice_counts[file], run_id=run_id)
        else:
            files[file] = Entry(slice=s, missed=old.missed if old else None, run_id=old.run_id if old else None)
    computed = Ratchet(slices=ratchet.slices,
                       previous_commit=commit if commit is not None else ratchet.previous_commit,
                       files=files)
    return verdict, computed


def render(plan: dict, ratchet: Ratchet, run: RunData, verdict: Verdict, computed: Ratchet) -> str:
    """The Markdown report: verdict, plan, per-shard counts, per-file comparison, every survivor."""
    out: List[str] = ["## Surviving-mutant ratchet", ""]
    if verdict.failed:
        out.append(f"**Failed:** {len(verdict.above)} file(s) above the ratchet.")
    else:
        out.append("**Passed:** no file is above the ratchet.")
    out.append("")
    d = plan["diff"]
    out.append(f"Slice {plan['slice']} of {plan['slices']}: {len(plan['slice_files'])} file(s), "
               f"{plan['slice_mutants']} mutants.")
    if d["run"]:
        out.append(f"Diff slice since `{d['base']}`: {d['mutants']} mutants outside the slice.")
    else:
        out.append(f"Diff slice skipped: {d['note']}.")
    out.append("")
    out += ["| Shard | Part | Mutants | Caught | Missed | Timeout | Unviable |", "|---|---|---|---|---|---|---|"]
    for s in run.shards:
        out.append(f"| {s.shard} | {s.kind} | {s.mutants} | {s.caught} | {s.missed} | {s.timeout} | {s.unviable} |")
    out.append("")
    if verdict.above:
        out += ["### Above the ratchet", "", "| File | Missed | Ratchet | Measured by |", "|---|---|---|---|"]
        out += [f"| `{f}` | {n} | {r} | {k} |" for f, n, r, k in verdict.above]
        out.append("")
    if verdict.can_lower:
        out += ["### Below the ratchet (can lower)", "", "| File | Missed | Ratchet |", "|---|---|---|"]
        out += [f"| `{f}` | {n} | {r} |" for f, n, r in verdict.can_lower]
        out.append("")
    if verdict.first:
        out += ["### Measured for the first time", "",
                "Recorded in the computed ratchet; a first count fails nothing.", "",
                "| File | Missed |", "|---|---|"]
        out += [f"| `{f}` | {n} |" for f, n in verdict.first]
        out.append("")
    if verdict.diff_unmeasured:
        out += ["### Diff-slice survivors in files not measured yet", "",
                "Listed, not compared: their slice has not run.", "",
                "| File | Missed in the diff |", "|---|---|"]
        out += [f"| `{f}` | {n} |" for f, n in verdict.diff_unmeasured]
        out.append("")
    out += [f"### Surviving mutants ({len(run.survivors)})", ""]
    for s in sorted(run.survivors, key=lambda s: (s.file, s.line, s.mutation)):
        out.append(f"- `{s.file}:{s.line}`: {s.mutation}" + (" (diff slice)" if s.kind == "diff" else ""))
    if not run.survivors:
        out.append("None.")
    out.append("")
    if run.timeouts:
        out += [f"### Timed out ({len(run.timeouts)}, not counted as surviving)", ""]
        for s in sorted(run.timeouts, key=lambda s: (s.file, s.line, s.mutation)):
            out.append(f"- `{s.file}:{s.line}`: {s.mutation}")
        out.append("")
    unmeasured = sorted(f for f, e in computed.files.items() if e.missed is None)
    out += ["### Bootstrap", ""]
    if unmeasured:
        pending = sorted({computed.files[f].slice for f in unmeasured})
        out.append(f"**Bootstrap:** {len(computed.files) - len(unmeasured)} of {len(computed.files)} file(s) "
                   f"measured; slice(s) not run yet: {', '.join(str(s) for s in pending)}.")
    else:
        out.append(f"Every file ({len(computed.files)}) has a count: the ratchet is bootstrapped.")
    dropped = sorted(set(ratchet.files) - set(computed.files))
    if dropped:
        out.append("")
        out.append("Dropped from the ratchet (no longer in scope): " + ", ".join(f"`{f}`" for f in dropped) + ".")
    out.append("")
    out.append("The computed ratchet is the artifact `mutants-ratchet`; a follow-up pull request "
               "commits it as `tests/fixtures/mutants_ratchet.json`.")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------------
# Commands


def append(path: Optional[str], text: str) -> None:
    if path:
        with open(path, "a", encoding="utf-8") as f:
            f.write(text)


def summary_path(args: argparse.Namespace) -> Optional[str]:
    return args.summary if args.summary is not None else os.environ.get("GITHUB_STEP_SUMMARY")


def report_unreadable(summary: Optional[str], title: str, e: DataError) -> int:
    text = (f"## {title}\n\n**Could not check:** the run's data is "
            f"incomplete, so no count is compared.\n\n```\n{e}\n```\n")
    sys.stdout.write(text)
    append(summary, text)
    print(f"error: {e}", file=sys.stderr)
    return EXIT_UNREADABLE


def run_plan(args: argparse.Namespace) -> int:
    summary = summary_path(args)
    try:
        if args.ratchet is None or args.mutants_list is None or args.plan_out is None or args.shards is None:
            raise DataError("--plan needs --ratchet, --mutants-list, --shards and --plan-out")
        ratchet = load_ratchet(Path(args.ratchet))
        scope = mutant_files(Path(args.mutants_list), allow_empty=False)
        week: Optional[int] = None
        if args.slice is not None and args.slice != "":
            if not re.fullmatch(r"[0-9]+", args.slice):
                raise DataError(f"the slice {args.slice!r} is not a non-negative integer")
            slice_index = int(args.slice)
        elif args.run_created is not None:
            week = week_index(args.run_created)
            slice_index = rotation_slice(week, ratchet.slices)
        else:
            raise DataError("--plan needs --run-created or --slice")
        diff = mutant_files(Path(args.diff_list), allow_empty=True) if args.diff_list else None
        capacity = args.per_shard_capacity if args.per_shard_capacity is not None else sizing().per_shard_capacity
        plan = make_plan(ratchet, scope, slice_index, week, diff, args.diff_skipped, args.shards, capacity)
    except DataError as e:
        return report_unreadable(summary, "Mutation run plan", e)
    out = Path(args.plan_out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "plan.json").write_text(json.dumps(plan, indent=2) + "\n", encoding="utf-8")
    (out / "slice-files.txt").write_text("".join(f + "\n" for f in plan["slice_files"]), encoding="utf-8")
    text = render_plan(plan, scope)
    sys.stdout.write(text)
    append(summary, text)
    return EXIT_PASS


def run_shard_plan(args: argparse.Namespace) -> int:
    try:
        plan = load_plan(Path(args.shard_plan))
        if args.shard is None or not 0 <= args.shard < plan["shards"]:
            raise DataError(f"--shard is not in 0..{plan['shards'] - 1}")
    except DataError as e:
        print(f"error: {e}", file=sys.stderr)
        return EXIT_UNREADABLE
    e = plan["expect"][args.shard]
    print(f"{e['slice']} {e['diff']}")
    return EXIT_PASS


def run_check(args: argparse.Namespace) -> int:
    summary = summary_path(args)
    try:
        if args.shards_result is not None and args.shards_result != "success":
            raise DataError(f"the shard jobs ended `{args.shards_result}`, not `success`")
        if args.plan_file is None or args.ratchet is None or args.run_id is None:
            raise DataError("judging needs --plan-file, --ratchet and --run-id")
        if not re.fullmatch(r"[0-9]+", args.run_id):
            raise DataError(f"the run id {args.run_id!r} is not a number")
        if args.commit is not None and not COMMIT_RE.fullmatch(args.commit):
            raise DataError(f"the commit {args.commit!r} is not a 40-character commit id")
        ratchet = load_ratchet(Path(args.ratchet))
        plan = load_plan(Path(args.plan_file))
        if plan["slices"] != ratchet.slices:
            raise DataError(f"the plan has {plan['slices']} slices, the ratchet {ratchet.slices}")
        run = read_run(Path(args.outcomes_dir), plan, args.expect_version)
    except DataError as e:
        return report_unreadable(summary, "Surviving-mutant ratchet", e)
    verdict, computed = judge(plan, ratchet, run, int(args.run_id), args.commit)
    text = render(plan, ratchet, run, verdict, computed)
    sys.stdout.write(text)
    append(summary, text)
    if args.write_ratchet:
        Path(args.write_ratchet).write_text(ratchet_json(computed), encoding="utf-8")
    return EXIT_ABOVE if verdict.failed else EXIT_PASS


# ---------------------------------------------------------------------------------
# Self-check


def self_check() -> int:
    """Plan and judge small embedded runs; exits non-zero on a wrong verdict."""

    def mutant(file: str, line: int, mutation: str, summary: str) -> dict:
        return {
            "scenario": {"Mutant": {
                "name": f"{file}:{line}:5: {mutation}",
                "package": "discipline",
                "file": file,
                "function": None,
                "span": {"start": {"line": line, "column": 5}, "end": {"line": line, "column": 9}},
                "replacement": "true",
                "genre": "FnValue",
            }},
            "summary": summary,
            "log_path": "log/x.log",
            "diff_path": "diff/x.diff",
            "phase_results": [],
        }

    def lab(outcomes: List[dict], finished: bool = True, baseline: str = "Success") -> dict:
        return {
            "outcomes": [{"scenario": "Baseline", "summary": baseline, "log_path": "log/baseline.log",
                          "diff_path": None, "phase_results": []}] + outcomes,
            "total_mutants": len(outcomes),
            "start_time": "2026-01-01T00:00:00Z",
            "end_time": "2026-01-01T00:00:01Z" if finished else None,
            "cargo_mutants_version": "27.1.0",
        }

    commit = "a" * 40
    head = "b" * 40
    # Two shards: shard 0 holds the slice (src/ast/a.rs), shard 1 the diff slice (src/guards/b.rs).
    plan = {
        "slices": 2, "slice": 0, "week": None, "shards": 2, "per_shard_capacity": 10,
        "assignment": {"src/ast/a.rs": 0, "src/guards/b.rs": 1, "src/guards/c.rs": 1},
        "slice_files": ["src/ast/a.rs"], "slice_mutants": 3,
        "diff": {"base": commit, "run": True, "mutants": 3, "note": None},
        "expect": [{"slice": 3, "diff": 0}, {"slice": 0, "diff": 3}],
    }
    slice0 = lab([
        mutant("src/ast/a.rs", 10, "replace a -> bool with true", "MissedMutant"),
        mutant("src/ast/a.rs", 12, "replace == with != in a", "MissedMutant"),
        mutant("src/ast/a.rs", 14, "replace b -> bool with false", "CaughtMutant"),
    ])
    diff1 = lab([
        mutant("src/guards/b.rs", 7, "delete ! in c", "MissedMutant"),
        mutant("src/guards/b.rs", 9, "replace d with ()", "Timeout"),
        mutant("src/guards/b.rs", 11, "replace e -> u32 with 1", "Unviable"),
    ])

    def ratchet(a: Optional[int], b: Optional[int], previous: Optional[str] = commit) -> dict:
        def entry(s: int, n: Optional[int]) -> dict:
            return {"slice": s, "missed": n, "run_id": None if n is None else 7}
        return {"slices": 2, "previous_commit": previous,
                "files": {"src/ast/a.rs": entry(0, a), "src/guards/b.rs": entry(1, b),
                          "src/guards/c.rs": entry(1, 4)}}

    good = {(0, "slice"): slice0, (1, "diff"): diff1}
    cases = [
        # (name, ratchet, outcomes, shards_result, exit, must appear in the summary,
        #  expected computed entries {file: (missed, run_id)} or None)
        ("equal", ratchet(2, 1), good, None, EXIT_PASS, ["**Passed:**"],
         {"src/ast/a.rs": (2, 99), "src/guards/b.rs": (1, 7), "src/guards/c.rs": (4, 7)}),
        ("slice above", ratchet(1, 1), good, None, EXIT_ABOVE,
         ["Above the ratchet", "| `src/ast/a.rs` | 2 | 1 | slice |", "`src/ast/a.rs:10`: replace a -> bool with true"],
         None),
        ("lower", ratchet(3, 1), good, None, EXIT_PASS, ["can lower", "| `src/ast/a.rs` | 2 | 3 |"], None),
        ("diff above", ratchet(2, 0), good, None, EXIT_ABOVE, ["| `src/guards/b.rs` | 1 | 0 | diff |"],
         {"src/guards/b.rs": (0, 7)}),
        ("not measured yet", ratchet(None, None), good, None, EXIT_PASS,
         ["Measured for the first time", "not measured yet"],
         {"src/ast/a.rs": (2, 99), "src/guards/b.rs": (None, None)}),
        ("measured at zero", ratchet(0, 0), good, None, EXIT_ABOVE, ["Above the ratchet"], None),
        ("missing shard", ratchet(2, 1), {(0, "slice"): slice0}, None, EXIT_UNREADABLE,
         ["shard 1 (diff):", "not found"], None),
        ("unfinished shard", ratchet(2, 1), {(0, "slice"): slice0, (1, "diff"): lab(diff1["outcomes"][1:], False)},
         None, EXIT_UNREADABLE, ["did not finish"], None),
        ("failing baseline", ratchet(2, 1), {(0, "slice"): slice0, (1, "diff"): lab(diff1["outcomes"][1:], True, "Failure")},
         None, EXIT_UNREADABLE, ["baseline did not pass"], None),
        ("failed shard job", ratchet(2, 1), good, "failure", EXIT_UNREADABLE, ["ended `failure`"], None),
    ]
    failures = 0
    for name, ratchet_data, outcomes, shards_result, want, needles, entries in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for (k, kind), data in outcomes.items():
                d = root / "outcomes" / f"{ARTIFACT_PREFIX}{k}" / kind
                d.mkdir(parents=True)
                (d / "outcomes.json").write_text(json.dumps(data), encoding="utf-8")
            (root / "outcomes").mkdir(exist_ok=True)
            (root / "ratchet.json").write_text(json.dumps(ratchet_data), encoding="utf-8")
            (root / "plan.json").write_text(json.dumps(plan), encoding="utf-8")
            summary = root / "summary.md"
            computed_path = root / "computed.json"
            args = argparse.Namespace(
                outcomes_dir=str(root / "outcomes"), plan_file=str(root / "plan.json"),
                ratchet=str(root / "ratchet.json"), shards_result=shards_result, expect_version="27.1.0",
                run_id="99", commit=head, write_ratchet=str(computed_path), summary=str(summary),
            )
            saved = sys.stdout, sys.stderr
            sys.stdout = sys.stderr = open(os.devnull, "w", encoding="utf-8")
            try:
                got = run_check(args)
            finally:
                sys.stdout.close()
                sys.stdout, sys.stderr = saved
            text = summary.read_text(encoding="utf-8") if summary.exists() else ""
            problems = [f"missing {n!r}" for n in needles if n not in text]
            if got != want:
                problems.append(f"exit {got}, want {want}")
            if entries is not None:
                computed = json.loads(computed_path.read_text(encoding="utf-8")) if computed_path.exists() else {}
                for file, (missed, run_id) in entries.items():
                    e = computed.get("files", {}).get(file, {})
                    if (e.get("missed"), e.get("run_id")) != (missed, run_id):
                        problems.append(f"{file}: computed {e}, want missed {missed}, run_id {run_id}")
                if computed.get("previous_commit") != head:
                    problems.append("the computed ratchet does not hold this run's commit")
            if problems:
                print(f"FAIL {name}: {'; '.join(problems)}\n{text}", file=sys.stderr)
                failures += 1
            else:
                print(f"ok   {name}")

    # Planning: three weeks in a row walk three slices, each file once.
    scope = {"src/ast/a.rs": 5, "src/ast/b.rs": 4, "src/guards/c.rs": 3, "src/guards/d.rs": 1}
    r = Ratchet(slices=3, previous_commit=None, files={})
    seen: List[str] = []
    for week in (100, 101, 102):
        p = make_plan(r, scope, rotation_slice(week, 3), week, None, None, 2, 10)
        seen += p["slice_files"]
    if sorted(seen) != sorted(scope):
        print(f"FAIL rotation: slices over three weeks hold {sorted(seen)}", file=sys.stderr)
        failures += 1
    else:
        print("ok   rotation")
    p = make_plan(r, scope, 0, None, {"src/ast/a.rs": 2}, None, 2, 10)
    if p["diff"]["run"] or "no previous measured commit" not in (p["diff"]["note"] or ""):
        print(f"FAIL diff without a previous commit: {p['diff']}", file=sys.stderr)
        failures += 1
    else:
        print("ok   diff without a previous commit")
    total = len(cases) + 2
    if failures:
        print(f"{failures} self-check case(s) failed", file=sys.stderr)
        return 1
    print(f"{total} self-check cases passed")
    return 0


def main(argv: List[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    p.add_argument("--self-check", action="store_true", help="run the embedded cases and exit")
    p.add_argument("--validate", metavar="FILE", help="check that FILE parses as a ratchet and exit")
    p.add_argument("--sizing", action="store_true", help="print the derived slice count as JSON and exit")
    p.add_argument("--print-previous-commit", metavar="RATCHET",
                   help="print the ratchet's previous measured commit, or an empty line")
    p.add_argument("--plan", action="store_true", help="plan this run's slice and diff slice")
    p.add_argument("--mutants-list", help="`cargo mutants --list --json` over the whole scope")
    p.add_argument("--diff-list", help="`cargo mutants --list --json --in-diff` over the whole scope")
    p.add_argument("--diff-skipped", metavar="REASON", help="skip the diff slice, noting REASON")
    p.add_argument("--run-created", metavar="TIMESTAMP", help="the run's creation time; picks the slice by week")
    p.add_argument("--slice", help="test this slice instead of the week's")
    p.add_argument("--per-shard-capacity", type=int, help="override the derived mutants per shard")
    p.add_argument("--plan-out", metavar="DIR", help="write plan.json and slice-files.txt here")
    p.add_argument("--shard-plan", metavar="PLAN", help="print shard K's slice and diff mutant counts")
    p.add_argument("--shard", type=int, help="the shard K of --shard-plan")
    p.add_argument("--outcomes-dir", help="directory holding mutants-shard-<k>/<slice|diff>/outcomes.json")
    p.add_argument("--plan-file", help="the plan.json --plan wrote for this run")
    p.add_argument("--shards", type=int, help="the shard count N of `--shard k/N` (for --plan)")
    p.add_argument("--ratchet", help="the committed ratchet JSON")
    p.add_argument("--shards-result", help="the shard jobs' result; anything but `success` fails")
    p.add_argument("--expect-version", help="the cargo-mutants version every shard must report")
    p.add_argument("--run-id", help="this run's id, recorded beside each new count")
    p.add_argument("--commit", help="this run's commit, recorded as `previous_commit`")
    p.add_argument("--write-ratchet", metavar="FILE", help="write the computed ratchet here")
    p.add_argument("--summary", metavar="FILE",
                   help="append the report here (default: $GITHUB_STEP_SUMMARY when set)")
    args = p.parse_args(argv)
    if args.self_check:
        return self_check()
    if args.sizing:
        s = sizing()
        print(json.dumps({**s.__dict__, "total_mutants": OBSERVED_TOTAL_MUTANTS,
                          "seconds_per_mutant": OBSERVED_SECONDS_PER_MUTANT,
                          "baseline_seconds": OBSERVED_BASELINE_SECONDS, "shards": SHARDS,
                          "step_timeout_minutes": STEP_TIMEOUT_MINUTES,
                          "job_timeout_minutes": JOB_TIMEOUT_MINUTES,
                          "hosted_job_limit_minutes": HOSTED_JOB_LIMIT_MINUTES,
                          "safety_percent": SAFETY_PERCENT, "diff_reserve_percent": DIFF_RESERVE_PERCENT},
                         indent=2))
        return EXIT_PASS
    if args.validate or args.print_previous_commit:
        path = args.validate or args.print_previous_commit
        try:
            r = load_ratchet(Path(path))
        except DataError as e:
            print(f"error: {e}", file=sys.stderr)
            return EXIT_UNREADABLE
        if args.print_previous_commit:
            print(r.previous_commit or "")
            return EXIT_PASS
        measured = [e.missed for e in r.files.values() if e.missed is not None]
        print(f"{path}: valid ratchet ({r.slices} slices, {len(r.files)} file(s), "
              f"{len(measured)} measured, {sum(measured)} missed)")
        return EXIT_PASS
    if args.plan:
        return run_plan(args)
    if args.shard_plan:
        return run_shard_plan(args)
    if args.outcomes_dir is None:
        p.error("choose a mode: --plan, --shard-plan, --outcomes-dir, --validate, --sizing or --self-check")
    return run_check(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
