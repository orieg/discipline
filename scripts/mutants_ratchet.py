#!/usr/bin/env python3
"""Compare a sharded cargo-mutants run with the committed surviving-mutant ratchet.

`.github/workflows/mutants.yml` runs `cargo mutants --shard k/N` over `src/ast/` and
`src/guards/` and keeps each shard's `mutants.out/outcomes.json` as the artifact
`mutants-shard-<k>`. This script reads all N of them, counts the surviving
(`MissedMutant`) mutants per source file and compares the counts with the ratchet,
`tests/fixtures/mutants_ratchet.json` (docs/ARCHITECTURE.md §8.7):

  {"description": "...", "missed": {"src/ast/x.rs": 3, ...}}

- a file above its count fails the run; a file absent from the ratchet counts as 0;
- a file below its count passes and is listed as one whose count can be lowered;
- `"bootstrap": true` passes whatever the counts, so the first run can record them.

The computed ratchet is printed and written to `--write-ratchet` for a follow-up pull
request to commit. Every surviving mutant is listed with its file, line and mutation.

Fail-closed: a shard whose job did not succeed, whose outcomes are missing or malformed,
whose run did not finish (`end_time` is null) or whose baseline failed makes the run fail
with exit 2. Missing data is never read as zero survivors.

`outcomes.json` is cargo-mutants' `LabOutcome` (src/outcome.rs, src/mutant.rs and
src/span.rs of cargo-mutants; book/src/mutants-out.md): `outcomes[]` holds one entry
per scenario with `scenario` (`"Baseline"`, or `{"Mutant": {"name", "file", "span":
{"start": {"line", "column"}}, ...}}`) and `summary` (`CaughtMutant`, `MissedMutant`,
`Timeout`, `Unviable`, `Success`, `Failure`), beside `total_mutants`, `end_time` and
`cargo_mutants_version`.

Exit codes: 0 pass, 1 a file is above the ratchet, 2 the data could not be read.
Stdlib only, so it runs on a bare runner image.

Usage:
  scripts/mutants_ratchet.py --outcomes-dir DIR --shards N --ratchet FILE
      [--shards-result RESULT] [--expect-version V] [--write-ratchet FILE] [--summary FILE]
  scripts/mutants_ratchet.py --validate FILE
  scripts/mutants_ratchet.py --self-check
"""

import argparse
import json
import os
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Tuple

EXIT_PASS = 0
EXIT_ABOVE = 1
EXIT_UNREADABLE = 2

ARTIFACT_PREFIX = "mutants-shard-"
RATCHET_DESCRIPTION = (
    "Surviving (missed) cargo-mutants mutants per source file under src/ast/ and "
    "src/guards/, written by scripts/mutants_ratchet.py from a run of "
    ".github/workflows/mutants.yml (docs/ARCHITECTURE.md §8.7). A file absent here "
    "counts as 0. A run that finds more in a file fails; lower a count when a test "
    "kills a survivor."
)


class DataError(Exception):
    """The run's data is missing, malformed or incomplete: fail closed."""


@dataclass
class Survivor:
    file: str
    line: int
    mutation: str


@dataclass
class ShardCounts:
    shard: int
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
class Ratchet:
    bootstrap: bool
    missed: Dict[str, int]


@dataclass
class Verdict:
    above: List[Tuple[str, int, int]] = field(default_factory=list)
    can_lower: List[Tuple[str, int, int]] = field(default_factory=list)
    equal: List[Tuple[str, int]] = field(default_factory=list)

    @property
    def failed(self) -> bool:
        return bool(self.above)


def load_json(path: Path) -> object:
    try:
        with path.open(encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        raise DataError(f"{path}: not found") from None
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise DataError(f"{path}: cannot be read as JSON ({e})") from None


def load_ratchet(path: Path) -> Ratchet:
    """The committed ratchet; anything but a map of file to non-negative count is an error."""
    data = load_json(path)
    if not isinstance(data, dict):
        raise DataError(f"{path}: the ratchet is not a JSON object")
    missed = data.get("missed")
    if not isinstance(missed, dict):
        raise DataError(f"{path}: `missed` is not an object of file to count")
    for file, count in missed.items():
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            raise DataError(f"{path}: `missed` of {file} is not a non-negative integer")
    bootstrap = data.get("bootstrap", False)
    if not isinstance(bootstrap, bool):
        raise DataError(f"{path}: `bootstrap` is not true or false")
    if bootstrap and missed:
        raise DataError(f"{path}: `bootstrap` is true but `missed` already holds counts")
    return Ratchet(bootstrap=bootstrap, missed=dict(missed))


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


def read_mutant(entry: dict, where: str) -> Survivor:
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
    return Survivor(file=file, line=line, mutation=mutation_text(mutant, file))


def read_shard(path: Path, shard: int, expect_version: Optional[str]) -> Tuple[ShardCounts, List[Survivor], List[Survivor]]:
    """One shard's counts, survivors and timeouts; incomplete data raises `DataError`."""
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

    counts = ShardCounts(shard=shard)
    survivors: List[Survivor] = []
    timeouts: List[Survivor] = []
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
        if summary == "CaughtMutant":
            counts.caught += 1
        elif summary == "MissedMutant":
            counts.missed += 1
            survivors.append(read_mutant(entry, str(path)))
        elif summary == "Timeout":
            counts.timeout += 1
            timeouts.append(read_mutant(entry, str(path)))
        elif summary == "Unviable":
            counts.unviable += 1
        else:
            raise DataError(f"{path}: a mutant has the outcome {summary!r}, not one a test run gives")
    if counts.mutants != total:
        raise DataError(f"{path}: {counts.mutants} mutant outcomes, but `total_mutants` is {total}")
    return counts, survivors, timeouts


def read_run(outcomes_dir: Path, shards: int, expect_version: Optional[str]) -> RunData:
    """Every shard's outcomes, from `<dir>/mutants-shard-<k>/outcomes.json` for k in 0..shards."""
    if shards < 1:
        raise DataError("--shards must be at least 1")
    run = RunData()
    problems: List[str] = []
    for k in range(shards):
        path = outcomes_dir / f"{ARTIFACT_PREFIX}{k}" / "outcomes.json"
        try:
            counts, survivors, timeouts = read_shard(path, k, expect_version)
        except DataError as e:
            problems.append(f"shard {k}: {e}")
            continue
        run.shards.append(counts)
        run.survivors.extend(survivors)
        run.timeouts.extend(timeouts)
    if problems:
        raise DataError("\n".join(problems))
    return run


def missed_per_file(survivors: List[Survivor]) -> Dict[str, int]:
    counts: Dict[str, int] = {}
    for s in survivors:
        counts[s.file] = counts.get(s.file, 0) + 1
    return dict(sorted(counts.items()))


def compare(counts: Dict[str, int], ratchet: Ratchet) -> Verdict:
    """Each file's survivors against its ratchet count (0 when absent)."""
    verdict = Verdict()
    if ratchet.bootstrap:
        return verdict
    for file in sorted(set(counts) | set(ratchet.missed)):
        found, allowed = counts.get(file, 0), ratchet.missed.get(file, 0)
        if found > allowed:
            verdict.above.append((file, found, allowed))
        elif found < allowed:
            verdict.can_lower.append((file, found, allowed))
        else:
            verdict.equal.append((file, found))
    return verdict


def ratchet_json(counts: Dict[str, int]) -> str:
    return json.dumps({"description": RATCHET_DESCRIPTION, "missed": counts}, indent=2, ensure_ascii=False) + "\n"


def render(run: RunData, ratchet: Ratchet, verdict: Verdict, counts: Dict[str, int]) -> str:
    """The Markdown report: verdict, per-shard counts, per-file comparison, every survivor."""
    out: List[str] = ["## Surviving-mutant ratchet", ""]
    if ratchet.bootstrap:
        out.append("**Bootstrap:** the ratchet holds no counts yet, so this run passes. "
                   "Commit the computed ratchet below as `tests/fixtures/mutants_ratchet.json`.")
    elif verdict.failed:
        out.append(f"**Failed:** {len(verdict.above)} file(s) above the ratchet.")
    else:
        out.append("**Passed:** no file is above the ratchet.")
    out.append("")
    out += ["| Shard | Mutants | Caught | Missed | Timeout | Unviable |", "|---|---|---|---|---|---|"]
    for s in run.shards:
        out.append(f"| {s.shard} | {s.mutants} | {s.caught} | {s.missed} | {s.timeout} | {s.unviable} |")
    out.append("")
    if verdict.above:
        out += ["### Above the ratchet", "", "| File | Missed | Ratchet |", "|---|---|---|"]
        out += [f"| `{f}` | {n} | {r} |" for f, n, r in verdict.above]
        out.append("")
    if verdict.can_lower:
        out += ["### Below the ratchet (can lower)", "", "| File | Missed | Ratchet |", "|---|---|---|"]
        out += [f"| `{f}` | {n} | {r} |" for f, n, r in verdict.can_lower]
        out.append("")
    out += [f"### Surviving mutants ({len(run.survivors)})", ""]
    for s in sorted(run.survivors, key=lambda s: (s.file, s.line, s.mutation)):
        out.append(f"- `{s.file}:{s.line}`: {s.mutation}")
    if not run.survivors:
        out.append("None.")
    out.append("")
    if run.timeouts:
        out += [f"### Timed out ({len(run.timeouts)}, not counted as surviving)", ""]
        for s in sorted(run.timeouts, key=lambda s: (s.file, s.line, s.mutation)):
            out.append(f"- `{s.file}:{s.line}`: {s.mutation}")
        out.append("")
    if ratchet.bootstrap or verdict.can_lower or verdict.above:
        out += ["### Computed ratchet", "", "```json", ratchet_json(counts).rstrip("\n"), "```", ""]
    return "\n".join(out) + "\n"


def append(path: Optional[str], text: str) -> None:
    if path:
        with open(path, "a", encoding="utf-8") as f:
            f.write(text)


def run_check(args: argparse.Namespace) -> int:
    summary = args.summary if args.summary is not None else os.environ.get("GITHUB_STEP_SUMMARY")
    try:
        if args.shards_result is not None and args.shards_result != "success":
            raise DataError(f"the shard jobs ended `{args.shards_result}`, not `success`")
        ratchet = load_ratchet(Path(args.ratchet))
        run = read_run(Path(args.outcomes_dir), args.shards, args.expect_version)
    except DataError as e:
        text = ("## Surviving-mutant ratchet\n\n**Could not check:** the run's data is "
                f"incomplete, so no count is compared.\n\n```\n{e}\n```\n")
        sys.stdout.write(text)
        append(summary, text)
        print(f"error: {e}", file=sys.stderr)
        return EXIT_UNREADABLE
    counts = missed_per_file(run.survivors)
    verdict = compare(counts, ratchet)
    text = render(run, ratchet, verdict, counts)
    sys.stdout.write(text)
    append(summary, text)
    if args.write_ratchet:
        Path(args.write_ratchet).write_text(ratchet_json(counts), encoding="utf-8")
    return EXIT_ABOVE if verdict.failed else EXIT_PASS


def self_check() -> int:
    """Run the comparison over small embedded outcomes; exits non-zero on a wrong verdict."""

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
        mutants = [o for o in outcomes if o["scenario"] != "Baseline"]
        return {
            "outcomes": [{"scenario": "Baseline", "summary": baseline, "log_path": "log/baseline.log",
                          "diff_path": None, "phase_results": []}] + outcomes,
            "total_mutants": len(mutants),
            "missed": sum(o["summary"] == "MissedMutant" for o in mutants),
            "caught": sum(o["summary"] == "CaughtMutant" for o in mutants),
            "timeout": sum(o["summary"] == "Timeout" for o in mutants),
            "unviable": sum(o["summary"] == "Unviable" for o in mutants),
            "success": 0,
            "start_time": "2026-01-01T00:00:00Z",
            "end_time": "2026-01-01T00:00:01Z" if finished else None,
            "cargo_mutants_version": "27.1.0",
        }

    shard0 = lab([
        mutant("src/ast/a.rs", 10, "replace a -> bool with true", "MissedMutant"),
        mutant("src/ast/a.rs", 12, "replace == with != in a", "MissedMutant"),
        mutant("src/ast/a.rs", 14, "replace b -> bool with false", "CaughtMutant"),
    ])
    shard1 = lab([
        mutant("src/guards/b.rs", 7, "delete ! in c", "MissedMutant"),
        mutant("src/guards/b.rs", 9, "replace d with ()", "Timeout"),
        mutant("src/guards/b.rs", 11, "replace e -> u32 with 1", "Unviable"),
    ])
    cases = [
        # (name, ratchet, shards, shard files, shards_result, exit, must appear in output)
        ("above", {"missed": {"src/ast/a.rs": 1, "src/guards/b.rs": 1}}, 2, [shard0, shard1],
         None, EXIT_ABOVE, ["Above the ratchet", "`src/ast/a.rs:10`: replace a -> bool with true"]),
        ("equal", {"missed": {"src/ast/a.rs": 2, "src/guards/b.rs": 1}}, 2, [shard0, shard1],
         None, EXIT_PASS, ["**Passed:**"]),
        ("lower", {"missed": {"src/ast/a.rs": 3, "src/guards/b.rs": 1}}, 2, [shard0, shard1],
         None, EXIT_PASS, ["can lower", "| `src/ast/a.rs` | 2 | 3 |"]),
        ("absent file", {"missed": {"src/ast/a.rs": 2}}, 2, [shard0, shard1],
         None, EXIT_ABOVE, ["| `src/guards/b.rs` | 1 | 0 |"]),
        ("all at zero", {"missed": {}}, 2, [shard0, shard1],
         None, EXIT_ABOVE, ["Above the ratchet"]),
        ("bootstrap", {"bootstrap": True, "missed": {}}, 2, [shard0, shard1],
         None, EXIT_PASS, ["**Bootstrap:**", '"src/ast/a.rs": 2']),
        ("missing shard", {"bootstrap": True, "missed": {}}, 3, [shard0, shard1],
         None, EXIT_UNREADABLE, ["shard 2:", "not found"]),
        ("unfinished shard", {"bootstrap": True, "missed": {}}, 2, [shard0, lab([], finished=False)],
         None, EXIT_UNREADABLE, ["did not finish"]),
        ("failing baseline", {"bootstrap": True, "missed": {}}, 2, [shard0, lab([], baseline="Failure")],
         None, EXIT_UNREADABLE, ["baseline did not pass"]),
        ("failed shard job", {"bootstrap": True, "missed": {}}, 2, [shard0, shard1],
         "failure", EXIT_UNREADABLE, ["ended `failure`"]),
    ]
    failures = 0
    for name, ratchet, shards, labs, shards_result, want, needles in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for k, data in enumerate(labs):
                d = root / "outcomes" / f"{ARTIFACT_PREFIX}{k}"
                d.mkdir(parents=True)
                (d / "outcomes.json").write_text(json.dumps(data), encoding="utf-8")
            (root / "ratchet.json").write_text(json.dumps(ratchet), encoding="utf-8")
            summary = root / "summary.md"
            args = argparse.Namespace(
                outcomes_dir=str(root / "outcomes"), shards=shards, ratchet=str(root / "ratchet.json"),
                shards_result=shards_result, expect_version="27.1.0",
                write_ratchet=str(root / "computed.json"), summary=str(summary),
            )
            saved = sys.stdout, sys.stderr
            sys.stdout = sys.stderr = open(os.devnull, "w", encoding="utf-8")
            try:
                got = run_check(args)
            finally:
                sys.stdout.close()
                sys.stdout, sys.stderr = saved
            text = summary.read_text(encoding="utf-8") if summary.exists() else ""
            missing = [n for n in needles if n not in text]
            if got != want or missing:
                print(f"FAIL {name}: exit {got}, want {want}; missing {missing}\n{text}", file=sys.stderr)
                failures += 1
            else:
                print(f"ok   {name}")
    if failures:
        print(f"{failures} self-check case(s) failed", file=sys.stderr)
        return 1
    print(f"{len(cases)} self-check cases passed")
    return 0


def main(argv: List[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    p.add_argument("--self-check", action="store_true", help="run the embedded cases and exit")
    p.add_argument("--validate", metavar="FILE", help="check that FILE parses as a ratchet and exit")
    p.add_argument("--outcomes-dir", help="directory holding mutants-shard-<k>/outcomes.json")
    p.add_argument("--shards", type=int, help="the shard count N of `--shard k/N`")
    p.add_argument("--ratchet", help="the committed ratchet JSON")
    p.add_argument("--shards-result", help="the shard jobs' result; anything but `success` fails")
    p.add_argument("--expect-version", help="the cargo-mutants version every shard must report")
    p.add_argument("--write-ratchet", metavar="FILE", help="write the computed ratchet here")
    p.add_argument("--summary", metavar="FILE",
                   help="append the report here (default: $GITHUB_STEP_SUMMARY when set)")
    args = p.parse_args(argv)
    if args.self_check:
        return self_check()
    if args.validate:
        try:
            r = load_ratchet(Path(args.validate))
        except DataError as e:
            print(f"error: {e}", file=sys.stderr)
            return EXIT_UNREADABLE
        state = "bootstrap" if r.bootstrap else f"{len(r.missed)} file(s), {sum(r.missed.values())} missed"
        print(f"{args.validate}: valid ratchet ({state})")
        return EXIT_PASS
    if args.outcomes_dir is None or args.shards is None or args.ratchet is None:
        p.error("--outcomes-dir, --shards and --ratchet are required")
    return run_check(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
