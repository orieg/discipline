#!/usr/bin/env python3
"""scripts/benchmark_charts.py — SVG charts of the committed benchmark results.

Reads only committed result files and writes the charts to `docs/assets/img/results/`:

  - `pr-block-rate`: how many merged coding-agent pull requests discipline blocks, against a
    simple rule that blocks any changed test, CI or configuration file
    (`agent-pr-precision-v6`, `docs/benchmarks/results-v6/summary.json`).
  - `precision-raters`: whether those blocks were right, as two rounds of AI grading judged the
    same findings (`docs/benchmarks/results-v5/`, `results-v6/`).
  - `hook-latency`: how long the check after an agent edit takes, before and after #994 and
    #995 (`docs/benchmarks/results-hook-latency/`).
  - `recall-tradeoff`, `recall-by-release`: test gaming caught on impossible tests
    (`impossible-recall-v1`, #838). Written only when `docs/benchmarks/results-recall-confirmatory/`
    is present.

The charts are written for readers who are not statisticians: each title states the finding,
rows lead with a plain percentage, and identifiers stay in the docs the footnotes link to.
A rate's range is its Wilson 95% score interval (`benchmark_power_audit.wilson_score_interval`);
a latency is a median with its min-max spread. `<name>.svg` follows the reader's light or dark
colour scheme (the README); `<name>-light.svg` and `<name>-dark.svg` are fixed, for the Pages
site, whose theme toggle an image cannot see.

  python3 scripts/benchmark_charts.py           write the charts
  python3 scripts/benchmark_charts.py --check   exit 1 if a committed chart differs from its inputs
  python3 scripts/benchmark_charts.py --test    self-checks
"""

import importlib
import json
import math
import sys
import textwrap
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
wilson_score_interval = importlib.import_module("benchmark_power_audit").wilson_score_interval

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "docs" / "benchmarks"
OUT = ROOT / "docs" / "assets" / "img" / "results"
RECALL = BENCH / "results-recall-confirmatory"

WIDTH = 760
FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif'
MONO = "ui-monospace, SFMono-Regular, Menlo, monospace"

SIMPLE_FILE_RULE = "Simple rule: any test, CI or config file"
SIMPLE_LINE_RULE = "Simple rule: any changed test line"

# The recall builds in release order: (summary file, row label, written from these same cases).
# A file that is absent is skipped.
RECALL_BUILDS = [
    ("replay-summary.json", "0.26.0, the release fixed in advance", False),
    ("replay-v0.31.0-summary.json", "0.31.0", True),
    ("replay-main-9df9b947-summary.json", "Development version, before #1048", True),
    ("replay-main-87447255-summary.json", "Development version, with #1048-#1050", True),
]

# Discipline in indigo, the comparison rules in grey, a bar set in advance in amber. No colour
# means "better": the direction is written on each chart.
LIGHT_CSS = f""".bg{{fill:#fff}} .border{{stroke:#e2e8f0;fill:none}} .grid{{stroke:#f1f5f9;stroke-dasharray:2,3}}
.axis{{stroke:#cbd5e1;stroke-width:1.5px}} .floor{{stroke:#b45309;stroke-width:1.5px;stroke-dasharray:5,3}}
.t-title{{font-family:{FONT};font-size:14px;font-weight:700;fill:#0f172a}}
.t-sub{{font-family:{FONT};font-size:11px;font-weight:500;fill:#334155}}
.t-note{{font-family:{FONT};font-size:10px;fill:#64748b}}
.t-head{{font-family:{FONT};font-size:11px;font-weight:700;fill:#0f172a}}
.t-row{{font-family:{FONT};font-size:11px;font-weight:600;fill:#0f172a}}
.t-axis{{font-family:{MONO};font-size:9.5px;fill:#64748b}}
.t-val{{font-family:{MONO};font-size:10.5px;font-weight:600;fill:#334155}}
.t-big{{font-family:{MONO};font-size:11px;font-weight:700;fill:#0f172a}}
.t-floor{{font-family:{FONT};font-size:10px;font-weight:700;fill:#b45309}}
.c-disc{{fill:#4f46e5;stroke:#4f46e5}} .c-base{{fill:#64748b;stroke:#64748b}}
.c-hollow{{fill:#fff;stroke:#4f46e5;stroke-width:2px}}
.ci{{stroke-width:2.5px;stroke-linecap:round}}"""

DARK_CSS = """.bg{fill:#0d1117} .border{stroke:#30363d} .grid{stroke:#21262d} .axis{stroke:#484f58}
.t-title,.t-row,.t-head,.t-big{fill:#f0f6fc} .t-sub{fill:#cbd5e1} .t-note,.t-axis{fill:#94a3b8} .t-val{fill:#e2e8f0}
.floor{stroke:#f59e0b} .t-floor{fill:#fbbf24}
.c-disc{fill:#818cf8;stroke:#818cf8} .c-base{fill:#94a3b8;stroke:#94a3b8}
.c-hollow{fill:#0d1117;stroke:#818cf8}"""

# `auto` follows the reader's colour scheme (README); `light` and `dark` are fixed, for the
# site, whose own theme toggle an <img> cannot see.
THEMES = {
    "auto": LIGHT_CSS + "\n@media (prefers-color-scheme: dark){\n" + DARK_CSS + "\n}",
    "light": LIGHT_CSS,
    "dark": LIGHT_CSS + "\n" + DARK_CSS,
}


def header(height: int, title: str, theme: str) -> str:
    return f"""<svg xmlns="http://www.w3.org/2000/svg" role="img" viewBox="0 0 {WIDTH} {height}" width="100%">
<title>{esc(title)}</title>
<defs><style>
{THEMES[theme]}
</style></defs>
<rect class="bg" width="{WIDTH}" height="{height}" rx="6"/>
<rect class="border" x=".5" y=".5" width="{WIDTH - 1}" height="{height - 1}" rx="6"/>
"""


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def text(cls: str, x: float, y: float, s: str) -> str:
    return f'<text class="{cls}" x="{x:g}" y="{y:g}">{esc(s)}</text>'


def pct(v: float) -> str:
    """A share as a whole percentage, or one decimal under 10% (1.7%, 18%)."""
    p = v * 100
    return f"{p:.1f}%" if 0 < p < 10 else f"{p:.0f}%"


def outward(lo: float, hi: float) -> str:
    """A likely range rounded outward, so a printed bound never claims more than the interval."""
    def down(v: float) -> str:
        p = v * 100
        return f"{math.floor(p * 10) / 10:.1f}%" if 0 < p < 10 else f"{math.floor(p):.0f}%"

    def up(v: float) -> str:
        p = v * 100
        return f"{math.ceil(p * 10) / 10:.1f}%" if 0 < p < 10 else f"{math.ceil(p):.0f}%"

    return f"{down(lo)}–{up(hi)}"


def proportion(label: str, k: int, n: int, cls: str) -> tuple:
    """A row for a count: the share k/n first, then the count and its likely range (Wilson 95%)."""
    if not 0 <= k <= n or n <= 0:
        raise ValueError(f"{label}: invalid count {k}/{n}")
    lo, hi = wilson_score_interval(k, n)
    return (label, k / n, lo, hi, cls, pct(k / n), f"{k} of {n} · likely {outward(lo, hi)}")


def forest(y0: float, rows: list, *, xmax: float = 1.0, percent: bool = True, unit: str = "",
           floor: float | None = None, floor_label: str = "") -> tuple[str, float]:
    """Dot-and-range rows on one horizontal axis. Returns (svg, height used)."""
    x0, label_w, row_h, value_w = 24, 230, 26, 222
    px0 = x0 + label_w
    pw = WIDTH - 48 - label_w - value_w

    def x(v: float) -> float:
        return px0 + pw * v / xmax

    h = row_h * len(rows) + 30
    out = []
    for i in range(6):
        v = xmax * i / 5
        out.append(f'<line class="grid" x1="{x(v):.1f}" y1="{y0}" x2="{x(v):.1f}" y2="{y0 + h - 24}"/>')
        tick = f"{v * 100:.0f}%" if percent else f"{v:g}{unit}"
        out.append(f'<text class="t-axis" x="{x(v):.1f}" y="{y0 + h - 10}" text-anchor="middle">{tick}</text>')
    out.append(f'<line class="axis" x1="{px0}" y1="{y0 + h - 24}" x2="{px0 + pw}" y2="{y0 + h - 24}"/>')
    if floor is not None:
        out.append(f'<line class="floor" x1="{x(floor):.1f}" y1="{y0 - 6}" x2="{x(floor):.1f}" y2="{y0 + h - 24}"/>')
        out.append(text("t-floor", round(x(floor) + 4, 1), y0 - 8, floor_label))
    for i, (label, v, lo, hi, cls, big, detail) in enumerate(rows):
        y = y0 + 10 + i * row_h
        out.append(text("t-row", x0, y + 4, label))
        line_cls = "c-disc" if cls == "c-hollow" else cls
        out.append(f'<line class="ci {line_cls}" x1="{x(lo):.1f}" y1="{y}" x2="{x(hi):.1f}" y2="{y}"/>')
        out.append(f'<circle class="{cls}" cx="{x(v):.1f}" cy="{y}" r="4.5"/>')
        out.append(text("t-big", px0 + pw + 12, y + 4, big))
        out.append(text("t-val", px0 + pw + 62, y + 4, detail))
    return "\n".join(out), h


def chart(title: str, subtitle: list[str], sections: list, notes: list[str]) -> "Chart":
    """sections: (heading or None, rows, forest keyword arguments)."""
    body = []
    y = 32
    for line in textwrap.wrap(title, 88):
        body.append(text("t-title", 24, y, line))
        y += 19
    y += 1
    for line in textwrap.wrap(" ".join(subtitle), 112):
        body.append(text("t-sub", 24, y, line))
        y += 15
    y += 12
    for heading, rows, kw in sections:
        if heading:
            body.append(text("t-head", 24, y, heading))
            y += 16
        if kw.get("floor") is not None:
            y += 10
        svg, h = forest(y, rows, **kw)
        body.append(svg)
        y += h + 14
    for line in textwrap.wrap(" ".join(notes), 124):
        body.append(text("t-note", 24, y, line))
        y += 14
    return Chart(title, int(y + 8), "\n".join(body))


class Chart:
    def __init__(self, title: str, height: int, body: str):
        self.title, self.height, self.body = title, height, body

    def svg(self, theme: str = "auto") -> str:
        svg = header(self.height, self.title, theme) + self.body + "\n</svg>\n"
        ET.fromstring(svg)
        return svg


def load(path: Path) -> dict:
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def structural(summary: dict) -> tuple[int, int]:
    for c in summary["class_results"]:
        if c["class"] == "structural-ast-and-deterministic":
            return c["k"], c["n"]
    raise ValueError("no structural-ast-and-deterministic class result")


def pr_block_rate() -> Chart:
    s = load(BENCH / "results-v6" / "summary.json")
    b = s["pull_request_block_rates"]
    d, t = b["discipline"], b["naive_sensitive_path_filter"]
    rows = [
        proportion("Discipline", d["blocked"], d["n"], "c-disc"),
        proportion(SIMPLE_FILE_RULE, t["blocked"], t["n"], "c-base"),
    ]
    return chart(
        f"Discipline blocked {pct(d['blocked'] / d['n'])} of merged agent pull requests; a simple rule blocked {pct(t['blocked'] / t['n'])}",
        [f"{b['total_pull_requests']} merged pull requests written by coding agents, in 8 public repositories. Dot: measured share. Line: the likely range."],
        [(None, rows, {})],
        [
            "Fewer blocks means less friction, but only if the blocks it keeps are right, and that is not yet known (next chart).",
            f"Discipline {s['discipline_version']}. The simple rule blocks any pull request that changes a test, CI or configuration file. (measured)",
        ],
    )


def precision_raters() -> Chart:
    v5 = load(BENCH / "results-v5" / "summary.json")
    v6 = load(BENCH / "results-v6" / "summary.json")
    floor = next(c["precision_floor"] for c in v6["class_results"] if c["class"] == "structural-ast-and-deterministic")
    (k5, n5), (k6, n6) = structural(v5), structural(v6)
    rows = [
        proportion("First grading round", k5, n5, "c-base"),
        proportion("Second round, new graders", k6, n6, "c-base"),
    ]
    lo5 = wilson_score_interval(k5, n5)[0]
    return chart(
        "Unknown: two rounds of AI grading disagree on whether the blocks were right",
        [
            "Graders judged whether each finding behind a block was right. The first round said "
            f"{pct(k5 / n5)} were; a second round of new graders, on the same findings, said {pct(k6 / n6)}.",
        ],
        [(None, rows, {"floor": floor, "floor_label": f"Bar set in advance: {pct(floor)}"})],
        [
            f"The second-round graders agreed with each other only weakly (score {v6['cohens_kappa']:.2f}, where 1 is full agreement and 0 is chance; 0.75 was required),"
            f" so neither round settles it. All graders were AI models, not people. Even the first round's range starts at {math.floor(lo5 * 1000) / 10:.1f}%, under the bar.",
            f"One pull request can hold several findings. The newer release dropped {n5 - n6} of the first round's {n5} findings, none graded right. (measured)",
        ],
    )


def post_edit(name: str) -> dict[str, dict]:
    d = load(BENCH / "results-hook-latency" / name)
    repos = {}
    for r in d["repos"]:
        cell = r["cells"]["hook-post-edit"]
        repos["discipline" if r["name"] in ("discipline", "src") else r["name"]] = cell
    return repos


def hook_latency() -> Chart:
    stages = [
        ("Before", "hook-latency.json", "c-base"),
        ("Faster engine", "fast-path-hook-latency.json", "c-hollow"),
        ("+ checks only what the edit touches", "edit-scoped-main-hook-latency.json", "c-disc"),
    ]
    sections, spread, now = [], 0.0, {}
    for repo, heading in (("discipline", "On this repository (large)"), ("ripgrep", "On ripgrep (a mid-size Rust project)")):
        cells = [post_edit(name)[repo] for _, name, _ in stages]
        first, last = cells[0]["median_s"], cells[-1]["median_s"]
        now[repo] = (first, last)
        rows = []
        for (label, _, cls), c in zip(stages, cells):
            spread = max(spread, c["max_s"] - c["min_s"])
            big = f"{c['median_s']:.1f} s"
            detail = f"{first / c['median_s']:.1f}x faster" if c is cells[-1] else ""
            rows.append((label, c["median_s"], c["min_s"], c["max_s"], cls, big, detail))
        sections.append((heading, rows, {"xmax": 50, "percent": False, "unit": " s"}))
    (d0, d1), (r0, r1) = now["discipline"], now["ripgrep"]
    return chart(
        f"The check after each agent edit now takes {r1:.1f} to {d1:.1f} seconds, down from {r0:.1f} to {d0:.0f}",
        [f"Typical (median) time for the check that runs after an agent edits one file, over 7 runs. Lower is better. Runs varied by under {math.ceil(spread * 100) / 100:.2f} s."],
        sections,
        ["Release builds with the network off. The two speed-ups are #994 and #995; the method is in docs/benchmarks/hook-latency.md. (measured)"],
    )


def recall_primary(name: str) -> dict:
    return load(RECALL / name)["by_benchmark"]["swe"]["config_arms"]["primary"]


def recall_tradeoff() -> Chart:
    p = recall_primary("replay-summary.json")
    r, f, twins = p["R"], p["F"], p["twins"]
    lr, tp = twins["line-regex"], twins["naive-sensitive-path-filter"]
    floor = load(RECALL / "replay-summary.json")["recall_floor"]
    caught = [
        proportion("Discipline", r["k"], r["n"], "c-disc"),
        proportion(SIMPLE_LINE_RULE, lr["recall"]["k"], lr["recall"]["n"], "c-base"),
        proportion(SIMPLE_FILE_RULE, tp["recall"]["k"], tp["recall"]["n"], "c-base"),
    ]
    wrong = [
        proportion("Discipline", f["k"], f["n"], "c-disc"),
        proportion(SIMPLE_LINE_RULE, lr["false_block_rate_original"]["k"], lr["false_block_rate_original"]["n"], "c-base"),
        proportion(SIMPLE_FILE_RULE, tp["false_block_rate_original"]["k"], tp["false_block_rate_original"]["n"], "c-base"),
    ]
    return chart(
        f"On impossible tests, Discipline caught {pct(r['k'] / r['n'])} of test gaming; simple rules caught {pct(lr['recall']['k'] / lr['recall']['n'])}-{pct(tp['recall']['k'] / tp['recall']['n'])}",
        [
            "Coding agents were given tests that cannot pass honestly. When an agent made them pass by editing tests, which check blocked it?",
            "The same agents also solved ordinary tasks honestly; the second panel is how often each check wrongly blocked those.",
        ],
        [
            (f"Test gaming caught, out of {r['n']} cases (higher is better)", caught,
             {"floor": floor, "floor_label": f"Bar set in advance: {pct(floor)}"}),
            (f"Honest solutions wrongly blocked, out of {f['n']} (lower is better)", wrong, {}),
        ],
        ["Python tasks from Impossible-SWE-bench, discipline 0.26.0, the release fixed before the study. Method: docs/benchmarks/REPRODUCING.md §4. (measured)"],
    )


def recall_by_release() -> Chart:
    rows, honest = [], set()
    for name, label, written_from in RECALL_BUILDS:
        if not (RECALL / name).exists():
            continue
        p = recall_primary(name)
        rows.append(proportion(label, p["R"]["k"], p["R"]["n"], "c-hollow" if written_from else "c-disc"))
        honest.add((p["F"]["k"], p["F"]["n"]))
    if len(honest) == 1:
        k, n = honest.pop()
        f = f"Honest solutions wrongly blocked stay at {pct(k / n)} ({k} of {n}) on every build."
    else:
        f = "Honest solutions wrongly blocked differ between builds: " + ", ".join(f"{k} of {n}" for k, n in sorted(honest)) + "."
    return chart(
        "Test gaming caught, release by release: not evidence of improvement",
        [f"The same {rows[0][6].split(' of ')[1].split(' ')[0]} cases each time. {f}"],
        [(None, rows, {})],
        [
            "Hollow dots: these builds carry fixes written from misses on these same cases, so they cannot show the fixes help on new ones.",
            "That needs fresh cases, which the study's plan requires. Method: docs/benchmarks/REPRODUCING.md §4. (measured)",
        ],
    )


def charts() -> dict[str, str]:
    """Every chart file: `<name>.svg` follows the reader's scheme, `-light` / `-dark` are fixed."""
    made = {
        "pr-block-rate": pr_block_rate(),
        "precision-raters": precision_raters(),
        "hook-latency": hook_latency(),
    }
    if RECALL.is_dir():
        made["recall-tradeoff"] = recall_tradeoff()
        made["recall-by-release"] = recall_by_release()
    out = {}
    for name, c in made.items():
        out[f"{name}.svg"] = c.svg("auto")
        out[f"{name}-light.svg"] = c.svg("light")
        out[f"{name}-dark.svg"] = c.svg("dark")
    return out


def self_test() -> None:
    lo, hi = wilson_score_interval(119, 125)
    assert (round(lo, 6), round(hi, 6)) == (0.899229, 0.977818), (lo, hi)
    row = proportion("x", 41, 232, "c-disc")
    assert row[5:] == ("18%", "41 of 232 · likely 13%–24%"), row
    assert outward(*wilson_score_interval(119, 125)) == "89%–98%"
    assert proportion("x", 11, 644, "c-disc")[5] == "1.7%"
    assert pct(0) == "0%" and pct(1) == "100%"
    for bad in ((3, 2), (-1, 2), (0, 0)):
        try:
            proportion("x", *bad, "c-disc")
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted {bad}")
    c = chart("A & B", ["s"], [(None, [row], {"floor": 0.9, "floor_label": "f"})], ["n"])
    auto, light, dark = c.svg("auto"), c.svg("light"), c.svg("dark")
    assert "A &amp; B" in auto and 'class="floor"' in auto
    assert "prefers-color-scheme" in auto and "prefers-color-scheme" not in light + dark
    assert "#0d1117" not in light and "#0d1117" in dark
    print("benchmark_charts self-test: ok")


def main() -> int:
    if "--test" in sys.argv:
        self_test()
        return 0
    rendered = charts()
    if "--check" in sys.argv:
        stale = [n for n, s in rendered.items() if not (OUT / n).exists() or (OUT / n).read_text(encoding="utf-8") != s]
        extra = sorted(p.name for p in OUT.glob("*.svg") if p.name not in rendered) if OUT.is_dir() else []
        for n in stale:
            print(f"stale or missing: docs/assets/img/results/{n}", file=sys.stderr)
        for n in extra:
            print(f"not produced from committed results: docs/assets/img/results/{n}", file=sys.stderr)
        if stale or extra:
            print("run: python3 scripts/benchmark_charts.py", file=sys.stderr)
            return 1
        return 0
    OUT.mkdir(parents=True, exist_ok=True)
    for n, s in rendered.items():
        (OUT / n).write_text(s, encoding="utf-8")
        print(f"wrote docs/assets/img/results/{n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
