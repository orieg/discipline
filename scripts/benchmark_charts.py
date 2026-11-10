#!/usr/bin/env python3
"""scripts/benchmark_charts.py — SVG charts of the committed benchmark results.

Reads only committed result files and writes one SVG per chart to `docs/assets/img/results/`:

  - `pr-block-rate.svg`: merged coding-agent pull requests blocked, discipline against the
    naive test-path twin (`agent-pr-precision-v6`, `docs/benchmarks/results-v6/summary.json`).
  - `precision-raters.svg`: structural-gate precision under the v5 and the v6 labels of the
    same findings (`docs/benchmarks/results-v5/`, `results-v6/`).
  - `hook-latency.svg`: post-edit hook median latency before and after #994 and #995
    (`docs/benchmarks/results-hook-latency/`).
  - `recall-tradeoff.svg`, `recall-by-release.svg`: test gaming caught on impossible tests
    (`impossible-recall-v1`, #838). Written only when `docs/benchmarks/results-recall-confirmatory/`
    is present.

Every interval is the Wilson 95% score interval (`benchmark_power_audit.wilson_score_interval`).
Each chart states its source and its provenance. `<name>.svg` follows the reader's light or
dark colour scheme (the README); `<name>-light.svg` and `<name>-dark.svg` are fixed, for the
Pages site, whose theme toggle an image cannot see.

  python3 scripts/benchmark_charts.py           write the charts
  python3 scripts/benchmark_charts.py --check   exit 1 if a committed chart differs from its inputs
  python3 scripts/benchmark_charts.py --test    self-checks
"""

import importlib
import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
wilson_score_interval = importlib.import_module("benchmark_power_audit").wilson_score_interval

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "docs" / "benchmarks"
OUT = ROOT / "docs" / "assets" / "img" / "results"
RECALL = BENCH / "results-recall-confirmatory"

WIDTH = 960
FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif'
MONO = "ui-monospace, SFMono-Regular, Menlo, monospace"

# The recall builds in release order: (summary file, row label). A file that is absent is skipped.
RECALL_BUILDS = [
    ("replay-summary.json", "0.26.0 (pre-registered binary)"),
    ("replay-v0.31.0-summary.json", "0.31.0"),
    ("replay-main-9df9b947-summary.json", "main 9df9b947"),
    ("replay-main-87447255-summary.json", "main 87447255 (#1048-#1050)"),
]


LIGHT_CSS = f""".bg{{fill:#fff}} .border{{stroke:#e2e8f0;fill:none}} .grid{{stroke:#f1f5f9;stroke-dasharray:2,3}}
.axis{{stroke:#cbd5e1;stroke-width:1.5px}} .floor{{stroke:#b45309;stroke-width:1.5px;stroke-dasharray:5,3}}
.t-title{{font-family:{FONT};font-size:12px;font-weight:700;letter-spacing:.6px;fill:#0f172a;text-transform:uppercase}}
.t-sub{{font-family:{FONT};font-size:10.5px;font-weight:500;fill:#334155}}
.t-note{{font-family:{FONT};font-size:9.5px;fill:#64748b}}
.t-row{{font-family:{FONT};font-size:11px;font-weight:600;fill:#0f172a}}
.t-axis{{font-family:{MONO};font-size:9.5px;fill:#64748b}}
.t-val{{font-family:{MONO};font-size:10.5px;font-weight:600;fill:#334155}}
.t-floor{{font-family:{MONO};font-size:9.5px;font-weight:700;fill:#b45309}}
.c-disc{{fill:#16a34a;stroke:#16a34a}} .c-twin{{fill:#64748b;stroke:#64748b}} .c-alt{{fill:#2563eb;stroke:#2563eb}}
.ci{{stroke-width:2.5px;stroke-linecap:round}}"""

DARK_CSS = """.bg{fill:#0d1117} .border{stroke:#30363d} .grid{stroke:#21262d} .axis{stroke:#484f58}
.t-title,.t-row{fill:#f0f6fc} .t-sub{fill:#cbd5e1} .t-note,.t-axis{fill:#94a3b8} .t-val{fill:#e2e8f0}
.floor{stroke:#f59e0b} .t-floor{fill:#fbbf24}
.c-disc{fill:#22c55e;stroke:#22c55e} .c-twin{fill:#94a3b8;stroke:#94a3b8} .c-alt{fill:#3b82f6;stroke:#3b82f6}"""

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


def proportion(label: str, k: int, n: int, cls: str) -> tuple:
    """A row for a binomial count: point k/n and its Wilson 95% interval."""
    if not 0 <= k <= n or n <= 0:
        raise ValueError(f"{label}: invalid count {k}/{n}")
    lo, hi = wilson_score_interval(k, n)
    return (label, k / n, lo, hi, cls, f"{k}/{n}  [{lo * 100:.1f}, {hi * 100:.1f}]")


def forest(y0: float, rows: list, *, xmax: float = 1.0, percent: bool = True,
           floor: float | None = None, floor_label: str = "") -> tuple[str, float]:
    """Dot-and-interval rows on one horizontal axis. Returns (svg, height used)."""
    x0, label_w, row_h = 24, 230, 26
    px0 = x0 + label_w
    pw = WIDTH - 48 - label_w - 120

    def x(v: float) -> float:
        return px0 + pw * v / xmax

    h = row_h * len(rows) + 30
    out = []
    for i in range(6):
        v = xmax * i / 5
        out.append(f'<line class="grid" x1="{x(v):.1f}" y1="{y0}" x2="{x(v):.1f}" y2="{y0 + h - 24}"/>')
        tick = f"{v * 100:.0f}%" if percent else f"{v:g}"
        out.append(f'<text class="t-axis" x="{x(v):.1f}" y="{y0 + h - 10}" text-anchor="middle">{tick}</text>')
    out.append(f'<line class="axis" x1="{px0}" y1="{y0 + h - 24}" x2="{px0 + pw}" y2="{y0 + h - 24}"/>')
    if floor is not None:
        out.append(f'<line class="floor" x1="{x(floor):.1f}" y1="{y0 - 6}" x2="{x(floor):.1f}" y2="{y0 + h - 24}"/>')
        out.append(text("t-floor", round(x(floor) + 4, 1), y0 - 8, floor_label))
    for i, (label, v, lo, hi, cls, value) in enumerate(rows):
        y = y0 + 10 + i * row_h
        out.append(text("t-row", x0, y + 4, label))
        out.append(f'<line class="ci {cls}" x1="{x(lo):.1f}" y1="{y}" x2="{x(hi):.1f}" y2="{y}"/>')
        out.append(f'<circle class="{cls}" cx="{x(v):.1f}" cy="{y}" r="4.5"/>')
        out.append(text("t-val", px0 + pw + 10, y + 4, value))
    return "\n".join(out), h


def chart(title: str, subtitle: str, sections: list, notes: list[str]) -> "Chart":
    """sections: (heading or None, rows, forest keyword arguments)."""
    body = [text("t-title", 24, 30, title), text("t-sub", 24, 48, subtitle)]
    y = 74
    for heading, rows, kw in sections:
        if heading:
            y += 6
            body.append(text("t-row", 24, y, heading))
            y += 16
        if kw.get("floor") is not None:
            y += 8
        svg, h = forest(y, rows, **kw)
        body.append(svg)
        y += h + 16
    for note in notes:
        body.append(text("t-note", 24, y, note))
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
    rows = [
        proportion(f"discipline {s['discipline_version']}", b["discipline"]["blocked"], b["discipline"]["n"], "c-disc"),
        proportion("test-path twin", b["naive_sensitive_path_filter"]["blocked"], b["naive_sensitive_path_filter"]["n"], "c-twin"),
    ]
    return chart(
        "Merged coding-agent pull requests blocked",
        "Share of merged agent pull requests each check would have blocked. Lower is less noise. Dot = point, bar = Wilson 95% CI.",
        [(None, rows, {})],
        [
            f"{s['study_id']}: {b['total_pull_requests']} merged coding-agent pull requests in 8 public repositories. The test-path twin flags any change to an existing test file, a CI workflow or a manifest / test configuration.",
            f"Blocked is not wrong: whether each block was warranted is the precision question, which is unresolved (precision-raters chart). (measured: {s['host']}, {s['evaluated_commit'][:8]})",
        ],
    )


def precision_raters() -> Chart:
    v5 = load(BENCH / "results-v5" / "summary.json")
    v6 = load(BENCH / "results-v6" / "summary.json")
    floor = next(c["precision_floor"] for c in v6["class_results"] if c["class"] == "structural-ast-and-deterministic")
    rows = [
        proportion("v5 labels (inherited)", *structural(v5), "c-twin"),
        proportion("v6 labels (fresh)", *structural(v6), "c-twin"),
    ]
    return chart(
        "Structural-gate precision: unresolved",
        "The same pull requests and findings. v6 relabelled every finding with fresh synthetic raters, who disagree with v5 and with each other.",
        [(None, rows, {"floor": floor, "floor_label": f"pre-registered floor {floor * 100:.0f}%"})],
        [
            f"v6 inter-rater kappa {v6['cohens_kappa']:.2f}, under the pre-registered 0.75: the raters do not agree on what the rubric asks. Raters are LLM subagents, not human review.",
            f"An external human labelling pass is the outstanding check. Source: agent-pr-precision-v5, -v6 summary.json. (measured: {v6['host']})",
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
        ("before", "hook-latency.json", "c-twin"),
        ("engine fast path (#994)", "fast-path-hook-latency.json", "c-alt"),
        ("+ event-scoped (#995)", "edit-scoped-main-hook-latency.json", "c-disc"),
    ]
    rows = []
    for repo in ("discipline", "ripgrep"):
        for label, name, cls in stages:
            c = post_edit(name)[repo]
            rows.append((f"{repo} · {label}", c["median_s"], c["min_s"], c["max_s"], cls, f"{c['median_s']:.2f} s"))
    return chart(
        "Per-edit hook latency",
        "Median seconds for the post-edit hook on a one-file change, 7 runs; the bar is min-max. Lower is better.",
        [(None, rows, {"xmax": 50, "percent": False})],
        ["Source: docs/benchmarks/hook-latency.md §4, §4.2, §4.3; release build, DISCIPLINE_NO_NETWORK=1. (measured: Apple M1, macOS)"],
    )


def recall_primary(name: str) -> dict:
    return load(RECALL / name)["by_benchmark"]["swe"]["config_arms"]["primary"]


def recall_tradeoff() -> Chart:
    p = recall_primary("replay-summary.json")
    r, twins = p["R"], p["twins"]
    lr, tp = twins["line-regex"]["recall"], twins["naive-sensitive-path-filter"]["recall"]
    floor = load(RECALL / "replay-summary.json")["recall_floor"]
    rows = [
        proportion("discipline 0.26.0 (pre-registered)", r["k"], r["n"], "c-disc"),
        proportion("line-regex twin", lr["k"], lr["n"], "c-twin"),
        proportion("test-path twin", tp["k"], tp["n"], "c-twin"),
    ]
    v6 = load(BENCH / "results-v6" / "summary.json")["pull_request_block_rates"]
    noise = [
        proportion("discipline 0.28.1", v6["discipline"]["blocked"], v6["discipline"]["n"], "c-disc"),
        proportion("test-path twin", v6["naive_sensitive_path_filter"]["blocked"], v6["naive_sensitive_path_filter"]["n"], "c-twin"),
    ]
    return chart(
        "Catching test gaming vs. blocking good work",
        "Discipline against naive twins: one flags any changed test, CI or configuration file, one any changed test line. Dot = point, bar = Wilson 95% CI.",
        [
            ("A · Test gaming caught on impossible tests (recall, higher is better)", rows,
             {"floor": floor, "floor_label": f"pre-registered floor {floor * 100:.0f}%"}),
            ("B · Merged coding-agent pull requests blocked (noise, lower is better)", noise, {}),
        ],
        [
            f"A: impossible-recall-v1 (#838) confirmatory split, Impossible-SWE-bench, Python, access_full arm, primary configuration; {r['n']} in-scope techniques.",
            "B: agent-pr-precision-v6, 232 merged agent pull requests. A and B are different populations: read them side by side, not as one curve. (measured)",
        ],
    )


def recall_by_release() -> Chart:
    rows, honest = [], set()
    for name, label in RECALL_BUILDS:
        if not (RECALL / name).exists():
            continue
        p = recall_primary(name)
        rows.append(proportion(label, p["R"]["k"], p["R"]["n"], "c-disc"))
        honest.add((p["F"]["k"], p["F"]["n"]))
    if len(honest) == 1:
        k, n = honest.pop()
        lo, hi = wilson_score_interval(k, n)
        f = f"Honest false blocks are {k}/{n} [{lo * 100:.1f}%, {hi * 100:.1f}%] on every build."
    else:
        f = "Honest false blocks differ between builds: " + ", ".join(f"{k}/{n}" for k, n in sorted(honest)) + "."
    return chart(
        "Recall on test gaming, release by release (in-sample)",
        "Later builds replayed on the frozen #838 pairs. " + f,
        [(None, rows, {"xmax": 0.4})],
        [
            "In-sample: the fixes after 0.26.0 were written from misses on these same pairs, so these rows cannot show an improvement;",
            "the pre-registration requires fresh instances for that. Source: impossible-recall-v1 (#838), access_full, primary configuration. (measured: Linux lab replay)",
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
    assert row[5] == "41/232  [13.3, 23.1]", row
    for bad in ((3, 2), (-1, 2), (0, 0)):
        try:
            proportion("x", *bad, "c-disc")
        except ValueError:
            pass
        else:
            raise AssertionError(f"accepted {bad}")
    c = chart("A & B", "s", [(None, [row], {"floor": 0.9, "floor_label": "f"})], ["n"])
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
