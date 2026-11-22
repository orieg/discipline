#!/usr/bin/env python3
"""Head tests on base code (#1071): population scan for `head-tests-on-base-v1`.

Pre-registration: `docs/benchmarks/head-tests-on-base-preregistration.toml`.

The study runs the test files a merged agent pull request added or changed against the
code as it was before the change, and counts the tests that pass there. This script holds
the steps that run before any test does:

- `population`: reads a precision-study manifest (the v6 manifest fixes the 232 pull
  requests and their base and head commits), lists each pull request's changed files
  through the forge API (`gh api`), and writes which pull requests are in the population:
  those that change at least one test source file (kept, not removed) and at least one
  non-test source file.
- `--test`: self-checks: parity cases for the test-path rule, and the verdict rule
  (`majority_threshold`, `verdict`) pinned against the committed Wilson interval.

The test-path rule mirrors `crate::ast::functions::test_path` in `src/ast/functions.rs`,
which is the set of files the `base-tests` preset overlays. The two are kept equal by the
parity cases below, copied from that module's unit tests.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess
import sys
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = REPO_ROOT / "docs" / "benchmarks" / "results-v6" / "manifest.json"
RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results-head-tests-on-base"

JS_EXTENSIONS = ("js", "ts", "mjs", "cjs", "mts", "cts", "jsx", "tsx")

# Source files whose change the study counts, as test code or as the code under test.
# Fixtures, snapshots, expected-output files and documentation are neither.
SOURCE_EXTENSIONS = frozenset(
    ("rs", "py", "go", "java", "kt", "vue", "svelte") + JS_EXTENSIONS
)


def file_name(path: str) -> str:
    return path.rsplit("/", 1)[-1]


def has_dir(path: str, directory: str) -> bool:
    """A directory component of `path` (never the file name) named `directory`, any case."""
    return any(c.lower() == directory.lower() for c in path.split("/")[:-1])


def names_file(p: str, name: str) -> bool:
    if not p.endswith(name):
        return False
    before = p[: len(p) - len(name)]
    return not before or not (before[-1].isascii() and before[-1].isalnum())


def ends_with_word(stem: str, word: str) -> bool:
    """Mirror of `ends_with_word` in `src/ast/functions.rs`."""
    if len(stem) < len(word):
        return False
    before, tail = stem[: len(stem) - len(word)], stem[len(stem) - len(word) :]
    if not before:
        return tail.lower() == word.lower()
    prev = before[-1]
    if not prev.isalnum():
        return tail.lower() == word.lower()
    if tail != word:
        return False
    capitalised = len(word) > 1 and word[1].islower()
    return capitalised or prev.islower() or prev.isnumeric()


def is_test_support_path(path: str) -> bool:
    if any(has_dir(path, d) for d in ("tests", "test", "__tests__", "testutil", "testutils")):
        return True
    p = path.lower()
    name = file_name(path)
    if "." in name:
        stem, ext = name.rsplit(".", 1)
        ext = ext.lower()
    else:
        stem, ext = name, ""
    return (
        p == "conftest.py"
        or p.endswith("/conftest.py")
        or p.endswith("_test.go")
        or names_file(p, "testutil.go")
        or names_file(p, "testutils.go")
        or p.endswith("_test.py")
        or (ext in JS_EXTENSIONS and any(stem.lower().endswith(w) for w in (".test", ".spec")))
        or names_file(p, "test-utils.ts")
        or names_file(p, "test-utils.js")
        or names_file(p, "test_utils.ts")
        or names_file(p, "test_utils.js")
        or (ext == "java" and ends_with_word(stem, "Test"))
        or (ext == "cs" and ends_with_word(stem, "Tests"))
        or p.endswith("_spec.rb")
        or p.endswith("_test.rb")
        or (ext == "php" and ends_with_word(stem, "Test"))
    )


def is_test_path(path: str) -> bool:
    """Mirror of `test_path` in `src/ast/functions.rs`."""
    return is_test_support_path(path) or has_dir(path, "benches") or has_dir(path, "examples")


def extension(path: str) -> str:
    name = file_name(path)
    return name.rsplit(".", 1)[-1].lower() if "." in name else ""


def classify(files: list[dict[str, Any]]) -> dict[str, Any]:
    """Which changed files are head test sources to overlay and which are code under test.

    A test source is kept when the change adds, modifies or renames it (its head path);
    a removed test file has no head version to run. A renamed file whose old path is a
    test file and whose new path is not counts on the side of its new path.
    """
    tests, code = [], []
    for f in files:
        path, status = f["filename"], f["status"]
        if extension(path) not in SOURCE_EXTENSIONS:
            continue
        if is_test_path(path):
            if status != "removed":
                tests.append({"path": path, "status": status})
        else:
            code.append(path)
    return {
        "head_test_sources": tests,
        "code_sources_changed": len(code),
        "in_population": bool(tests) and bool(code),
    }


def wilson_interval(k: int, n: int) -> tuple[float, float]:
    """95% Wilson score interval, from `scripts/benchmark_power_audit.py` (Wilson 1927)."""
    sys.path.insert(0, str(REPO_ROOT / "scripts"))
    from benchmark_power_audit import wilson_score_interval

    return wilson_score_interval(k, n)


def majority_threshold(n: int, floor: float = 0.5) -> int | None:
    """The smallest count k of n whose 95% Wilson lower bound is above `floor`.

    The verdict rule of the pre-registration: a class is "most reports" only when its
    Wilson lower bound clears 0.5, not when its point estimate does. None when even n of
    n does not clear it.
    """
    if n < 1:
        raise ValueError("n must be at least 1")
    return next((k for k in range(n + 1) if wilson_interval(k, n)[0] > floor), None)


def verdict(counts: dict[str, int]) -> str:
    """The pre-registered verdict from the labelled classes of the reported pull requests."""
    reported = sum(counts.values())
    if reported < 5:
        return "INSUFFICIENT_N"
    by_design = counts.get("backfill", 0) + counts.get("refactor", 0)
    if wilson_interval(by_design, reported)[0] > 0.5:
        return "DROP"
    if wilson_interval(counts.get("non-discriminating", 0), reported)[0] > 0.5:
        return "DESIGN_PRESET"
    return "INTERMEDIATE"


def pr_outcome(h: dict[str, Any] | None, b: dict[str, Any] | None, m: dict[str, Any] | None) -> dict[str, Any]:
    """The pre-registered per-pull-request outcome from its H, B and M results.

    A side result (from `scripts/head_tests_on_base_lab.py`) carries `units` (what ran),
    `failed_step` (`deps`, `build`, or None when every non-test step passed) and `cases`
    (case id -> {status, file}, only cases of the changed test files). In-scope cases pass
    in H and are absent from B. A failure of M's dependency step is a run-error; a failure
    of its build, or in-scope cases missing from its results, is a build-failure.
    """
    if h is None or not h.get("units"):
        return {"outcome": "no-runnable-case", "cause": "no runnable test file"}
    if h.get("failed_step"):
        return {"outcome": "run-error", "cause": f"H {h['failed_step']}"}
    if b is None or m is None:
        return {"outcome": "pending", "cause": "B or M not run"}
    if b.get("failed_step"):
        return {"outcome": "run-error", "cause": f"B {b['failed_step']}"}
    new = [c for c in h["cases"] if c not in b["cases"]]
    in_scope = sorted(c for c in new if h["cases"][c]["status"] == "passed")
    excluded = sorted(c for c in new if h["cases"][c]["status"] != "passed")
    base = {"in_scope": in_scope, "excluded_in_h": excluded}
    if not in_scope:
        return {"outcome": "no-runnable-case", "cause": "no in-scope case", **base}
    if m.get("failed_step") == "deps":
        return {"outcome": "run-error", "cause": "M deps", **base}
    if m.get("failed_step"):
        return {"outcome": "build-failure", "cause": f"M {m['failed_step']}", **base}
    present = [c for c in in_scope if c in m["cases"]]
    if not present:
        return {"outcome": "build-failure", "cause": "in-scope cases not collected in M", **base}
    passed = [c for c in in_scope if m["cases"].get(c, {}).get("status") == "passed"]
    return {
        "outcome": "reported" if passed else "not-reported",
        "cause": "",
        "passed_in_m": passed,
        **base,
    }


CLASSES = ("backfill", "refactor", "non-discriminating", "other")


def summarize(runs: dict[str, Any], labels: dict[str, Any]) -> dict[str, Any]:
    """`summary.json` from `runs.json` and `labels.json`: per-pull-request outcomes, the
    counts and 95% Wilson intervals the pre-registration names, and the verdict.

    Fails (ValueError) unless the labels cover exactly the reported pull requests, each
    with one of the pre-registered classes.
    """
    rows = []
    for e in runs["runs"]:
        o = pr_outcome(e["H"], e["B"], e["M"])
        rows.append(
            {
                "repo": e["repo"],
                "pr_number": e["pr_number"],
                "outcome": o["outcome"],
                "cause": o.get("cause", ""),
                "in_scope_cases": len(o.get("in_scope", [])),
                "excluded_in_h_cases": len(o.get("excluded_in_h", [])),
                "in_scope_passing_in_m": len(o.get("passed_in_m", [])),
            }
        )
    reported = {(r["repo"], r["pr_number"]) for r in rows if r["outcome"] == "reported"}
    labelled = {(x["repo"], x["pr_number"]): x["class"] for x in labels["labels"]}
    if set(labelled) != reported:
        raise ValueError(f"labels cover {sorted(labelled)}, reported are {sorted(reported)}")
    bad = sorted(c for c in labelled.values() if c not in CLASSES)
    if bad:
        raise ValueError(f"labels outside the pre-registered classes: {bad}")
    outcomes = {k: sum(r["outcome"] == k for r in rows) for k in ("reported", "not-reported", "build-failure", "no-runnable-case", "run-error")}
    n = outcomes["reported"]
    classes = {c: sum(v == c for v in labelled.values()) for c in CLASSES}

    def share(k: int, d: int) -> dict[str, Any]:
        lo, hi = wilson_interval(k, d) if d else (None, None)
        return {"k": k, "n": d, "wilson_95": [lo, hi]}

    per_repo: dict[str, dict[str, int]] = {}
    for r in rows:
        t = per_repo.setdefault(r["repo"], {"in_scope_cases": 0, "in_scope_passing_in_m": 0})
        t["in_scope_cases"] += r["in_scope_cases"]
        t["in_scope_passing_in_m"] += r["in_scope_passing_in_m"]
    return {
        "study_id": "head-tests-on-base-v1",
        "pull_requests": len(rows),
        "outcomes": outcomes,
        "report_rate": {
            "build_failure_as_not_reported": share(n, n + outcomes["not-reported"] + outcomes["build-failure"]),
            "build_failure_out_of_denominator": share(n, n + outcomes["not-reported"]),
        },
        "classes": {c: share(k, n) for c, k in classes.items()},
        "by_design": share(classes["backfill"] + classes["refactor"], n),
        "verdict": verdict(classes),
        "excluded_in_h_cases": sum(r["excluded_in_h_cases"] for r in rows),
        "in_scope_cases_per_repo": per_repo,
        "run_errors": [{"repo": r["repo"], "pr_number": r["pr_number"], "cause": r["cause"]} for r in rows if r["outcome"] == "run-error"],
        "pull_request_outcomes": rows,
    }


def changed_files(pr: dict[str, Any]) -> tuple[list[dict[str, Any]], str]:
    """The pull request's changed files from the forge, then the base...head comparison.

    Some manifest numbers are not pull requests on the forge (golang/go changes are
    Gerrit change numbers); the comparison of the manifest's own commits covers them.
    """
    jq = ".[]|{filename,status,previous_filename}"
    out = subprocess.run(
        ["gh", "api", "--paginate", f"repos/{pr['repo']}/pulls/{pr['pr_number']}/files", "--jq", jq],
        capture_output=True,
        text=True,
    )
    source = "pulls"
    if out.returncode != 0:
        out = subprocess.run(
            [
                "gh",
                "api",
                f"repos/{pr['repo']}/compare/{pr['base_sha']}...{pr['head_sha']}",
                "--jq",
                ".files" + jq,
            ],
            capture_output=True,
            text=True,
        )
        source = "compare"
    if out.returncode != 0:
        raise RuntimeError(f"{pr['repo']}#{pr['pr_number']}: {out.stderr.strip()}")
    return [json.loads(line) for line in out.stdout.splitlines() if line.strip()], source


def population(manifest_path: Path, out_path: Path) -> dict[str, Any]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    prs = manifest["pull_requests"]
    with ThreadPoolExecutor(8) as pool:
        listed = list(pool.map(changed_files, prs))
    rows = []
    for pr, (files, source) in zip(prs, listed):
        rows.append(
            {
                "repo": pr["repo"],
                "pr_number": pr["pr_number"],
                "base_sha": pr["base_sha"],
                "head_sha": pr["head_sha"],
                "files_source": source,
                **classify(files),
            }
        )
    per_repo: dict[str, dict[str, int]] = {}
    for row in rows:
        counts = per_repo.setdefault(row["repo"], {"pull_requests": 0, "in_population": 0})
        counts["pull_requests"] += 1
        counts["in_population"] += int(row["in_population"])
    result = {
        "study_id": "head-tests-on-base-v1",
        "source_manifest": manifest_path.relative_to(REPO_ROOT).as_posix(),
        "source_study": manifest["study_id"],
        "total_pull_requests": len(rows),
        "in_population": sum(r["in_population"] for r in rows),
        "per_repo": per_repo,
        "pull_requests": rows,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def self_test() -> int:
    # Parity cases from the unit tests of `src/ast/functions.rs`.
    yes = [
        "__tests__/a.js",
        "web/__tests__/a.js",
        "crates/a/benches/x.rs",
        "crates/a/tests/it.rs",
        "pkg/foo_test.go",
        "app/conftest.py",
        "conftest.py",
        "a/util.test.ts",
        "a/util.spec.tsx",
        "src/test/java/RepoTest.java",
        "src/HTTPTest.java",
        "lib/foo_spec.rb",
        "pkg/testutil.go",
        "web/test-utils.ts",
    ]
    no = [
        "src/mybenches/x.rs",
        "my__tests__/a.js",
        "web/util.test.json",
        "web/util.test.py",
        "src/Latest.java",
        "src/Contests.cs",
        "pkg/mytestutil.go",
        "src/lib.rs",
        "__tests__",
    ]
    failures = [p for p in yes if not is_test_path(p)] + [p for p in no if is_test_path(p)]
    for p in failures:
        print(f"FAIL test_path parity: {p}")

    def f(path: str, status: str = "modified") -> dict[str, str]:
        return {"filename": path, "status": status}

    cases = [
        ([f("src/a.ts"), f("src/a.test.ts", "added")], True),
        ([f("src/a.ts"), f("src/a.test.ts", "removed")], False),
        ([f("src/a.test.ts")], False),
        ([f("src/a.ts"), f("tests/fixtures/out.json")], False),
        ([f("src/a.rs"), f("src/snapshots/a.snap")], False),
        ([f("docs/a.md"), f("tests/a.py")], False),
        ([f("crates/x/src/lib.rs"), f("crates/x/tests/it.rs", "renamed")], True),
    ]
    for files, expected in cases:
        got = classify(files)["in_population"]
        if got != expected:
            failures.append(str(files))
            print(f"FAIL classify {[x['filename'] for x in files]}: {got} != {expected}")
    # Pinned against `wilson_score_interval`: the smallest majority that clears 0.5.
    pinned = {5: 5, 10: 9, 20: 15, 30: 21, 60: 38}
    for n, k in pinned.items():
        if majority_threshold(n) != k:
            failures.append(f"majority_threshold({n})")
            print(f"FAIL majority_threshold({n}) = {majority_threshold(n)}, expected {k}")
    lo, hi = wilson_interval(15, 20)
    if not (abs(lo - 0.5313) < 1e-3 and abs(hi - 0.8881) < 1e-3):
        failures.append("wilson_interval(15, 20)")
        print(f"FAIL wilson_interval(15, 20) = {lo, hi}")
    verdicts = [
        ({"backfill": 3, "non-discriminating": 1}, "INSUFFICIENT_N"),
        ({"backfill": 10, "refactor": 5, "non-discriminating": 5}, "DROP"),
        ({"backfill": 9, "refactor": 5, "non-discriminating": 6}, "INTERMEDIATE"),
        ({"backfill": 5, "non-discriminating": 15}, "DESIGN_PRESET"),
        ({"other": 20}, "INTERMEDIATE"),
    ]
    for counts, expected in verdicts:
        if verdict(counts) != expected:
            failures.append(f"verdict({counts})")
            print(f"FAIL verdict({counts}) = {verdict(counts)}, expected {expected}")
    def side(cases: dict[str, str], failed: str | None = None, units: bool = True) -> dict[str, Any]:
        return {
            "units": [{"kind": "x"}] if units else [],
            "failed_step": failed,
            "cases": {c: {"status": st, "file": "f"} for c, st in cases.items()},
        }

    outcomes = [
        ((side({}, units=False), None, None), "no-runnable-case"),
        ((side({}, "build"), side({}), side({})), "run-error"),
        ((side({"a": "passed"}), side({}, "build"), side({})), "run-error"),
        ((side({"a": "passed"}), side({"a": "passed"}), side({"a": "passed"})), "no-runnable-case"),
        ((side({"a": "failed"}), side({}), side({"a": "passed"})), "no-runnable-case"),
        ((side({"a": "passed"}), side({}), side({}, "deps")), "run-error"),
        ((side({"a": "passed"}), side({}), side({}, "build")), "build-failure"),
        ((side({"a": "passed"}), side({}), side({"z": "passed"})), "build-failure"),
        ((side({"a": "passed", "b": "passed"}), side({}), side({"a": "failed", "b": "passed"})), "reported"),
        ((side({"a": "passed"}), side({}), side({"a": "failed"})), "not-reported"),
        ((side({"a": "passed"}), side({}), side({"a": "skipped"})), "not-reported"),
    ]
    for (h, b, m), expected in outcomes:
        got = pr_outcome(h, b, m)["outcome"]
        if got != expected:
            failures.append(f"pr_outcome {expected}")
            print(f"FAIL pr_outcome: {got} != {expected} for {h, b, m}")
    # summarize: two reported, one not-reported, one build-failure.
    runs = {
        "runs": [
            {"repo": "r", "pr_number": i, "H": h, "B": b, "M": m}
            for i, (h, b, m) in enumerate(
                [
                    (side({"a": "passed"}), side({}), side({"a": "passed"})),
                    (side({"a": "passed"}), side({}), side({"a": "passed"})),
                    (side({"a": "passed"}), side({}), side({"a": "failed"})),
                    (side({"a": "passed"}), side({}), side({}, "build")),
                ]
            )
        ]
    }
    lab = {"labels": [{"repo": "r", "pr_number": 0, "class": "backfill"}, {"repo": "r", "pr_number": 1, "class": "refactor"}]}
    sm = summarize(runs, lab)
    got_sm = (
        sm["outcomes"]["reported"],
        sm["report_rate"]["build_failure_as_not_reported"]["n"],
        sm["report_rate"]["build_failure_out_of_denominator"]["n"],
        sm["by_design"]["k"],
        sm["verdict"],
    )
    summary_checks = [(got_sm, (2, 4, 3, 2, "INSUFFICIENT_N"))]
    for bad_labels in ({"labels": lab["labels"][:1]}, {"labels": [lab["labels"][0], {"repo": "r", "pr_number": 1, "class": "noise"}]}):
        try:
            summarize(runs, bad_labels)
            summary_checks.append(("accepted", "refused"))
        except ValueError:
            summary_checks.append(("refused", "refused"))
    for got, want in summary_checks:
        if got != want:
            failures.append("summarize")
            print(f"FAIL summarize: {got} != {want}")
    total = len(yes) + len(no) + len(cases) + len(pinned) + 1 + len(verdicts) + len(outcomes) + len(summary_checks)
    print(f"{'FAILED' if failures else 'ok'}: {total} checks")
    return 1 if failures else 0


def main() -> int:
    if "--test" in sys.argv:
        return self_test()
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    pop = sub.add_parser("population", help="write the study population from a manifest")
    pop.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    pop.add_argument("--out", type=Path, default=RESULTS_DIR / "population.json")
    sm = sub.add_parser("summarize", help="write summary.json from runs.json and labels.json")
    sm.add_argument("--runs", type=Path, default=RESULTS_DIR / "runs.json")
    sm.add_argument("--labels", type=Path, default=RESULTS_DIR / "labels.json")
    sm.add_argument("--out", type=Path, default=RESULTS_DIR / "summary.json")
    args = parser.parse_args()
    if args.cmd == "summarize":
        result = summarize(json.loads(args.runs.read_text(encoding="utf-8")), json.loads(args.labels.read_text(encoding="utf-8")))
        args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(f"{result['outcomes']}; classes {dict((c, v['k']) for c, v in result['classes'].items())}; verdict {result['verdict']}")
        return 0
    if args.cmd == "population":
        result = population(args.manifest.resolve(), args.out.resolve())
        print(f"{result['in_population']} of {result['total_pull_requests']} in population")
        for repo, counts in result["per_repo"].items():
            print(f"  {repo}: {counts['in_population']} of {counts['pull_requests']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
