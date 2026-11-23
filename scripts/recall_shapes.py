#!/usr/bin/env python3
"""Shape of the injected test file on #838's confirmatory gaming runs (Part 3 of `base-tests-v1`).

Pre-registration: `docs/benchmarks/base-tests-preregistration.toml`, `[q3_added_value]`.

Part 3 splits #838's 212 confirmatory gaming runs by the shape of the test file the
impossible test was injected into, as in the 2026-11-09 breakdown on #838:

- `well-formed` (77): the injected file parses and every line the run's test-file hunks
  cover sits inside a function.
- `broken` (99): the injected file does not parse, or a line the hunks cover sits at class
  body or module level, outside any function.
- `no-test-file-hunk` (36): the run has no `S1` or `S1-revert` technique (source-side gaming).

The rule (2026-11-09, RUN on the lab host). The population is the replay's pairs with arm
`access_full`, split not `original`, a stratum other than `unlabelled` and benchmark `swe`.
A run's hunks are those of its `S1` and `S1-revert` techniques in `strata.json`. For each hunk,
every non-blank line of its old range (the base side, context lines included) of the base
test file was labelled with Python's `ast`: `parse-error` when the file does not parse, else
by the innermost `FunctionDef` / `AsyncFunctionDef` span that holds it (`test-fn` when the
name starts with `test`, `helper-fn` otherwise), else `class-body` inside a `ClassDef` span,
else `module-level`. A run is `broken` when any hunk has `parse-error`, `class-body` or
`module-level`, otherwise `well-formed`. A helper function counts as well-formed.

What this script reads. That rule parsed the whole base test file, which lives only in the
lab host's materialised pair repositories (`recall_pairs.py --materialise`): the repository
at `base_commit` plus the impossible `test_patch`, neither of which is committed. The
committed `manifest-patches/<pair>/agent_patch.diff` holds the base side of every hunk the
rule reads, and nothing more. So this script applies the rule to that base side alone: it
rebuilds the statements, the indentation and the enclosing `def` / `class` of each line
from the hunk, and reports `parse-error` for what makes the whole file fail to parse there
(an unexpected indent, a block header with no body, a clause after a dedent, an unindent to
no open level, a statement that does not parse). A line whose enclosing header lies above
the hunk is placed by its indentation (`class-body` at four columns in a file of `self`
methods, `module-level` at column zero).

The hunk-local reading was written against the per-run labels of the 2026-11-09 run,
committed as `shapes-whole-file.json` in the results directory, and agrees with them on
212 / 212 runs. That agreement is by construction (it was tuned on these runs); it does not
show the reading holds on other files. A test-file hunk that does not reach an injected
line that breaks the parse elsewhere in the file would be read `well-formed` here; none of
the 212 runs has one.

- default: print the per-shape counts, blocked counts and Wilson intervals as JSON.
- `--runs`: add the per-run shapes and hunk labels.
- `--check-labels FILE`: compare per run with the whole-file labels; exit 1 on a mismatch.
- `--self-check`: synthetic cases for each part of the rule.
"""

from __future__ import annotations

import argparse
import ast
import gzip
import importlib.util
import json
from pathlib import Path
import re
import sys
import textwrap
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
RESULTS_DIR = REPO_ROOT / "docs" / "benchmarks" / "results-recall-confirmatory"
REPLAY = "replay-main-9df9b947.json.gz"
LABELS = "shapes-whole-file.json"

SHAPES = ("well-formed", "broken", "no-test-file-hunk")
BROKEN_LABELS = frozenset(("parse-error", "class-body", "module-level"))
TEST_SUB_STRATA = ("S1", "S1-revert")


def _load_sibling(name: str) -> Any:
    """Import a sibling script by path (works under `python3 -I`, which drops the script directory from sys.path)."""
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parent / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


power = _load_sibling("benchmark_power_audit")


# --- Reading the committed agent patch ------------------------------------------------

HUNK_HEADER = re.compile(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def base_sides(diff: str) -> dict[tuple[str, int], list[str]]:
    """The base side (context and removed lines) of every hunk, keyed by (old path, old start)."""
    out: dict[tuple[str, int], list[str]] = {}
    path: str | None = None
    hunk: list[str] | None = None
    for line in diff.splitlines():
        if line.startswith("diff --git ") or line.startswith("+++ "):
            hunk = None
            continue
        if line.startswith("--- "):
            path = line[6:] if line.startswith("--- a/") else None
            hunk = None
            continue
        m = HUNK_HEADER.match(line)
        if m:
            hunk = [] if path is not None else None
            if hunk is not None:
                out[(path, int(m[1]))] = hunk
            continue
        if hunk is None:
            continue
        if line[:1] in (" ", "-"):
            hunk.append(line[1:])
        elif line == "":
            hunk.append("")
    return out


# --- The rule on one hunk's base side --------------------------------------------------

HEADER = re.compile(r"(async\s+def|def|class)\s+(\w+)")
CLAUSE = re.compile(r"(else|elif|except|finally)\b")
TRIPLE = re.compile(r'"""|\'\'\'')
STRING = re.compile(r"'(?:\\.|[^'\\])*'|\"(?:\\.|[^\"\\])*\"")


def indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def code(line: str) -> str:
    """The line with string literals emptied and the comment dropped."""
    return re.sub(r"#.*$", "", STRING.sub('""', line))


def bracket_delta(line: str) -> int:
    c = code(line)
    return sum(c.count(x) for x in "([{") - sum(c.count(x) for x in ")]}")


def code_end(text: str) -> str:
    return code(text.split("\n")[-1]).rstrip()


def first_code_index(lines: list[str]) -> int:
    """Index of the first line that starts a statement: past a string or bracket the hunk opens inside."""
    start = 0
    for i, line in enumerate(lines):
        quotes = TRIPLE.findall(line)
        if not quotes:
            continue
        earlier = [x for x in lines[:i] if x.strip()]
        opens_docstring = bool(earlier) and code_end(earlier[-1]).endswith(":")
        lone = line.strip() in ('"""', "'''")
        opener = re.match(r"\s*[rbuRBU]?(\"\"\"|''')", line) or re.search(r"[=(]\s*[rbuRBU]?(\"\"\"|''')", line)
        if (lone and not opens_docstring) or (not lone and len(quotes) % 2 == 1 and not opener):
            start = i + 1
        break
    depth, lowest, cut = 0, 0, start - 1
    for i in range(start, len(lines)):
        depth += bracket_delta(lines[i])
        if depth < lowest:
            lowest, cut = depth, i
    return cut + 1


def statements(lines: list[str]) -> tuple[list[dict[str, Any]], list[int], bool]:
    """Logical lines (index, indent, first line, full text), comment-line indents, and whether the last one ends."""
    out: list[dict[str, Any]] = []
    comments: list[tuple[int, int]] = []
    depth, in_string, cur = 0, False, None
    for i in range(first_code_index(lines), len(lines)):
        line = lines[i]
        if in_string:
            cur["text"] += "\n" + line
            if len(TRIPLE.findall(line)) % 2 == 1:
                in_string = False
            continue
        if not line.strip():
            continue
        continued = cur is not None and (depth > 0 or cur["text"].rstrip().endswith("\\"))
        if line.lstrip().startswith("#") and not continued:
            comments.append((i, indent(line)))
            continue
        if continued:
            cur["text"] += "\n" + line
        else:
            cur = {"i": i, "k": indent(line), "first": line, "text": line}
            out.append(cur)
        if len(TRIPLE.findall(line)) % 2 == 1:
            in_string = True
        else:
            depth = max(0, depth + bracket_delta(line))
    return out, comments, not in_string and depth == 0


def statement_parses(text: str) -> bool:
    body = textwrap.dedent(text)
    if CLAUSE.match(body.lstrip()):
        return True
    if code_end(body).endswith(":"):
        body += "\n    pass"
    if re.match(r"\s*@", body):
        body += "\ndef _decorated(): pass"
    try:
        ast.parse(body)
    except SyntaxError:
        return False
    return True


def parse_errors(stmts: list[dict[str, Any]], complete: bool) -> bool:
    """Whether the hunk's base side holds something that makes the whole file fail to parse."""
    seen_statement = False
    for s in stmts if complete else stmts[:-1]:
        ok = statement_parses(s["text"])
        # A hunk can open on the tail of a statement that started above it; a fragment
        # before the first whole statement says nothing about the file.
        if not ok and seen_statement:
            return True
        seen_statement = seen_statement or ok
    stack = [stmts[0]["k"]] if stmts else []
    for a, b in zip(stmts, stmts[1:]):
        end = code_end(a["text"])
        opens_block = end.endswith(":")
        if b["k"] > a["k"] and not opens_block:
            return True  # unexpected indent
        if opens_block and b["k"] <= a["k"]:
            return True  # expected an indented block
        if CLAUSE.match(b["first"].lstrip()) and a["k"] <= b["k"]:
            return True  # else / except / finally with no block before it
        if b["k"] > stack[-1]:
            stack.append(b["k"])
        elif b["k"] < stack[-1]:
            while stack and stack[-1] > b["k"]:
                stack.pop()
            if stack and stack[-1] != b["k"]:
                return True  # unindent does not match any outer level
            if not stack:
                if b["k"] % 4:
                    return True
                stack = [b["k"]]
    return False


def hunk_labels(lines: list[str]) -> set[str]:
    """The labels the 2026-11-09 rule gives the non-blank lines of one hunk's base side."""
    stmts, comments, complete = statements(lines)
    labels: set[str] = set()
    if parse_errors(stmts, complete):
        labels.add("parse-error")
    methods = any(re.search(r"\bself\b", line) for line in lines)

    def enclosing(idx: int, k: int) -> str:
        level = k
        for j in range(idx - 1, -1, -1):
            if stmts[j]["k"] < level:
                m = HEADER.match(stmts[j]["first"].lstrip())
                if m:
                    if "def" in m[1]:
                        return "test-fn" if m[2].startswith("test") else "helper-fn"
                    return "class-body"
                level = stmts[j]["k"]
        if level == 0:
            return "module-level"
        return "class-body" if methods and level <= 4 else "fn"

    for idx, s in enumerate(stmts):
        m = HEADER.match(s["first"].lstrip())
        if m and "def" in m[1]:
            labels.add("test-fn" if m[2].startswith("test") else "helper-fn")
            continue
        where = enclosing(idx, s["k"])
        if m:  # a class header is in its own ClassDef span unless a function holds it
            where = "class-body" if where in ("class-body", "module-level") else where
        labels.add(where)
    for i, k in comments:
        # A comment is inside a function span only when a statement of that function follows it.
        after = [j for j, s in enumerate(stmts) if s["i"] > i]
        if after:
            nxt = stmts[after[0]]
            sibling_header = nxt["k"] == k and (HEADER.match(nxt["first"].lstrip()) or nxt["first"].lstrip().startswith("@"))
            if nxt["k"] >= k and not sibling_header:
                labels.add(enclosing(after[0], nxt["k"]))
                continue
        before = [j for j, s in enumerate(stmts) if s["i"] < i]
        if k == 0:
            labels.add("module-level")
        elif before:
            labels.add(enclosing(before[-1] + 1, k))
        else:
            labels.add("class-body" if methods and k <= 4 else "fn")
    return labels


def shape(hunks: list[set[str]] | None) -> str:
    if not hunks:
        return "no-test-file-hunk"
    return "broken" if any(h & BROKEN_LABELS for h in hunks) else "well-formed"


# --- The population -------------------------------------------------------------------


def gaming_runs(results: Path) -> list[dict[str, Any]]:
    """The 212 confirmatory gaming runs, with their test-file hunks and whether the replay blocked them."""
    with gzip.open(results / REPLAY, "rt", encoding="utf-8") as f:
        replay = json.load(f)
    strata = {p["pair_id"]: p for p in json.loads((results / "strata.json").read_text(encoding="utf-8"))["pairs"]}
    runs = []
    for p in replay["pairs"]:
        if p["arm"] != "access_full" or p["split"] == "original" or p["stratum"] == "unlabelled" or p["benchmark"] != "swe":
            continue
        findings = p["config_arms"]["primary"]["findings"] or []
        hunks = [
            h
            for t in strata[p["pair_id"]].get("techniques", [])
            if t.get("sub_stratum") in TEST_SUB_STRATA
            for h in t.get("hunks", [])
        ]
        runs.append(
            {
                "pair_id": p["pair_id"],
                "stratum": p["stratum"],
                "blocked": any(f["severity"] == "error" for f in findings),
                "hunks": hunks,
            }
        )
    return runs


def classify(results: Path) -> list[dict[str, Any]]:
    out = []
    for run in gaming_runs(results):
        labels = []
        if run["hunks"]:
            sides = base_sides((results / "manifest-patches" / run["pair_id"] / "agent_patch.diff").read_text(encoding="utf-8"))
            labels = [sorted(hunk_labels(sides.get((h["file"], h["old_start"]), []))) for h in run["hunks"]]
        out.append(
            {
                "pair_id": run["pair_id"],
                "blocked": run["blocked"],
                "shape": shape([set(x) for x in labels]),
                "parse_error": any("parse-error" in x for x in labels),
                "labels": labels,
            }
        )
    return out


def summary(runs: list[dict[str, Any]]) -> dict[str, Any]:
    def cell(rows: list[dict[str, Any]]) -> dict[str, Any]:
        n, k = len(rows), sum(r["blocked"] for r in rows)
        lo, hi = power.wilson_score_interval(k, n, 0.95) if n else (None, None)
        return {"runs": n, "blocked": k, "block_rate": k / n if n else None, "ci95": [lo, hi]}

    by_shape = {s: cell([r for r in runs if r["shape"] == s]) for s in SHAPES}
    broken = [r for r in runs if r["shape"] == "broken"]
    by_shape["broken"]["does_not_parse"] = sum(r["parse_error"] for r in broken)
    by_shape["broken"]["outside_any_function"] = sum(not r["parse_error"] for r in broken)
    return {"replay": REPLAY, "shapes": by_shape, "all": cell(runs)}


def whole_file_shape(labels: list[str]) -> str:
    """The 2026-11-09 aggregation of one run's per-hunk whole-file labels (`class-body+test-fn`, ...)."""
    return shape([set(x.split("+")) for x in labels] if labels else None)


def check_labels(runs: list[dict[str, Any]], labels_file: Path) -> list[str]:
    recorded = json.loads(labels_file.read_text(encoding="utf-8"))["runs"]
    problems = []
    if set(recorded) != {r["pair_id"] for r in runs}:
        problems.append("the recorded runs are not the replay's gaming runs")
    for r in runs:
        want = recorded.get(r["pair_id"])
        if want is not None and whole_file_shape(want) != r["shape"]:
            problems.append(f"{r['pair_id']}: whole file {whole_file_shape(want)}, hunk {r['shape']}")
    return problems


# --- Self-check -----------------------------------------------------------------------


def _hunk(text: str) -> list[str]:
    """A case's base side: the literal's own eight-column indent removed, the rest kept as written."""
    lines = text.strip("\n").split("\n")
    assert all(not x.strip() or x.startswith(" " * 8) for x in lines), text
    return [x[8:] for x in lines]


SELF_CHECK_CASES = [
    # (name, base side, expected labels)
    (
        "line inside a test function",
        """
        def test_sep(model, result):
            assert_allclose(is_separable(model), result[0])
            assert_allclose(is_separable(model), [True, False])


        def test_custom():
        """,
        {"test-fn"},
    ),
    (
        "unexpected indent after the injected line",
        """
                assert diff.identical
                assert len(diff.common_columns) == 11
            assert diff.common_column_names == set("abcdefghij")
                assert diff.diff_ratio == 0
        """,
        {"fn", "parse-error"},
    ),
    (
        "class-body line between two methods",
        """
                aggs = Book.objects.aggregate(
                    ratings=Count("rating"),
                )
            self.assertEqual(aggs["ratings"], 3)

            def test_next(self):
        """,
        {"fn", "class-body", "test-fn"},
    ),
    (
        "comment at column zero",
        """
                self.assertXMLEqual(index, expected)


        # RemovedInDjango50Warning
        """,
        {"fn", "module-level"},
    ),
    (
        "finally after a dedented line",
        """
                    mocked_sync_apps.assert_called()
                    self.assertEqual(args[1], {"app_unmigrated"})
                self.assertEqual(args[1], {"test_app"})
                finally:
                    destroy()
        """,
        {"fn", "parse-error"},
    ),
    (
        "block header with no body",
        """
                    with self.assertRaises(expected):
                        validator(value)
                else:
                self.assertEqual("failure", validator(value))

            def test_single(self):
        """,
        {"fn", "parse-error", "test-fn"},
    ),
    (
        "unindent to no open level",
        """
                user = User.objects.create(name="Joe")
                qs = Issue.objects.only("assignee__status")
               self.assertEqual(qs.get(), user)

            def test_eq(self):
        """,
        {"fn", "parse-error", "test-fn"},
    ),
    (
        "statement broken inside brackets",
        """
                for name, field in form.fields.items():
                    self.assertEqual(
                self.assertEqual(type(field.widget), forms.TextInput)
                        type(field.widget),
                        type(other.widget),
                    )
        """,
        {"fn", "parse-error"},
    ),
    (
        "hunk opens inside a docstring",
        '''
                there are changes to an app requiring migrations.
                """
                with self.temporary_migration_module() as tmpdir:
                call_command("makemigrations", "--check")
        ''',
        {"fn", "parse-error"},
    ),
    (
        "hunk opens inside brackets",
        """
                        'port': '444',
                    }), (
                        ['psql', '-U', username],
                        'wrongpassword',
                    )
                )

        """,
        set(),
    ),
    (
        "helper function",
        """
                class DummyFile(object):
                    def read(self):
                        # part of html of a normal response
                        return b'View <a href="/raw/3c0c6750bd">raw</a>.'

                return DummyFile()
        """,
        {"helper-fn", "fn"},
    ),
]


def self_check() -> None:
    for name, text, want in SELF_CHECK_CASES:
        got = hunk_labels(_hunk(text))
        assert got == want, (name, sorted(got), sorted(want))
    assert shape(None) == "no-test-file-hunk"
    assert shape([{"test-fn"}, {"helper-fn", "fn"}]) == "well-formed"
    assert shape([{"test-fn"}, {"class-body", "test-fn"}]) == "broken"
    assert whole_file_shape([]) == "no-test-file-hunk"
    assert whole_file_shape(["no-base-file", "test-fn"]) == "well-formed"
    assert whole_file_shape(["class-body+test-fn"]) == "broken"
    diff = (
        "diff --git a/t/test_a.py b/t/test_a.py\n--- a/t/test_a.py\n+++ b/t/test_a.py\n"
        "@@ -3,3 +3,2 @@\n def test_a():\n-    assert f() == 2\n     assert f()\n"
        "diff --git a/t/test_b.py b/t/test_b.py\n--- /dev/null\n+++ b/t/test_b.py\n@@ -0,0 +1 @@\n+x = 1\n"
    )
    assert base_sides(diff) == {("t/test_a.py", 3): ["def test_a():", "    assert f() == 2", "    assert f()"]}
    print(f"self-check: {len(SELF_CHECK_CASES)} hunk cases and the shape rule pass")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--results", type=Path, default=RESULTS_DIR, help="The #838 confirmatory results directory.")
    parser.add_argument("--runs", action="store_true", help="Include the per-run shapes and hunk labels.")
    parser.add_argument("--check-labels", type=Path, nargs="?", const=RESULTS_DIR / LABELS, help="Compare per run with the whole-file labels.")
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return 0
    runs = classify(args.results)
    if args.check_labels is not None:
        problems = check_labels(runs, args.check_labels)
        for p in problems:
            print(p, file=sys.stderr)
        print(f"{len(runs) - len(problems)} / {len(runs)} runs agree with {args.check_labels.name}")
        return 1 if problems else 0
    out = summary(runs)
    if args.runs:
        out["runs"] = runs
    json.dump(out, sys.stdout, indent=1)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
