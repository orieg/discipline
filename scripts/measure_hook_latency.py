#!/usr/bin/env python3
"""Measure the wall time of the per-edit agent hook (#982).

Every agent edit hook runs a full `discipline check` as a child process
(`src/hook.rs::run_check_under`). This script times that cost on scratch clones,
cell by cell, and refuses to time anything on a host that is not idle.

Cells (method: docs/benchmarks/hook-latency.md):

  check-one-file   `check --base <base>` with one committed one-file change on top of <base>
  check-large      `check --base <base>~N`: the same head against a base N commits back
  hook-post-edit   a Claude Code `PostToolUse` Edit payload piped through
                   `hook run --agent claude-code --observe`, hooks installed, one-file change
  hook-pre-tool    the same payload through `hook run --agent claude-code --event pre-tool --observe`
  git-diff-*       `git diff --name-only` and `git diff` over the same ranges (the floor
                   the check cannot go under)
  suite-*          `check --suite <suite>` per suite on the one-file change (where the time
                   goes; the binary exposes no per-gate timing)

Each cell runs on every repository given (this repository and a second one of a
different size). Results go to a JSON file; a markdown table is printed.

The host check runs before the first run and before every run of every cell: load
average at most half the core count and no foreign process above 100% CPU. A busy host
stops the script with exit 3 and the snapshot that failed; nothing is timed on it.

Usage:
    python3 scripts/measure_hook_latency.py \\
        --binary target/release/discipline \\
        --repo . --repo https://github.com/BurntSushi/ripgrep \\
        --runs 7 --out hook-latency.json
    python3 scripts/measure_hook_latency.py --test
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SUITES = ["agent-guard", "hygiene", "integrity", "quality", "verification", "bench"]
POST_EDIT = '{{"hook_event_name":"PostToolUse","tool_name":"Edit","cwd":{cwd},"tool_input":{{"file_path":{path}}}}}'
PRE_TOOL = '{{"hook_event_name":"PreToolUse","tool_name":"Edit","cwd":{cwd},"tool_input":{{"file_path":{path}}}}}'
EXIT_BUSY = 3


class HostBusy(RuntimeError):
    pass


def host_snapshot(target_pids: frozenset[int] = frozenset()) -> dict:
    """Load, core count and the processes above 100% CPU other than the target."""
    load1, load5, load15 = os.getloadavg()
    cores = os.cpu_count() or 1
    out = subprocess.run(
        ["ps", "-A", "-o", "pid=,%cpu=,command="], capture_output=True, text=True, check=True
    ).stdout
    heavy = []
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) < 3:
            continue
        pid, cpu, cmd = int(parts[0]), float(parts[1]), parts[2]
        if cpu > 100.0 and pid not in target_pids and pid != os.getpid():
            heavy.append({"pid": pid, "cpu": cpu, "command": cmd[:120]})
    return {"load1": load1, "load5": load5, "load15": load15, "cores": cores, "heavy": heavy}


def idle_verdict(snap: dict) -> str | None:
    """None when the host is idle enough to time on, else the reason it is not."""
    limit = snap["cores"] / 2
    if snap["load1"] > limit:
        return f"load1 {snap['load1']:.2f} > {limit:g} ({snap['cores']} cores x 0.5)"
    if snap["heavy"]:
        top = max(snap["heavy"], key=lambda p: p["cpu"])
        return f"foreign process at {top['cpu']:.0f}% CPU: {top['command']}"
    return None


def require_idle() -> dict:
    snap = host_snapshot()
    reason = idle_verdict(snap)
    if reason:
        raise HostBusy(reason)
    return snap


def summarise(samples: list[float]) -> dict:
    """Median and spread (min, max, interquartile range) of wall times in seconds."""
    if not samples:
        raise ValueError("no samples")
    s = sorted(samples)
    if len(s) >= 4:
        q = statistics.quantiles(s, n=4, method="inclusive")
        iqr = q[2] - q[0]
    else:
        iqr = s[-1] - s[0]
    return {
        "n": len(s),
        "median_s": statistics.median(s),
        "min_s": s[0],
        "max_s": s[-1],
        "iqr_s": iqr,
    }


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=True
    ).stdout.strip()


def prepare(source: str, scratch: Path, binary: Path, large_back: int) -> dict:
    """Clone `source` into `scratch`, commit a one-file change, install the hooks."""
    name = Path(source.rstrip("/")).name.removesuffix(".git") or "repo"
    dest = scratch / name
    subprocess.run(["git", "clone", "--quiet", source, str(dest)], check=True)
    base = git(dest, "rev-parse", "HEAD")
    commits = int(git(dest, "rev-list", "--count", "HEAD"))
    back = min(large_back, commits - 1)
    large_base = git(dest, "rev-parse", f"HEAD~{back}")
    # One-file change: a comment line appended to the largest tracked text file under
    # 300 KB, so parsing has real work and no gate is tripped by an empty change.
    files = [f for f in git(dest, "ls-files").splitlines() if (dest / f).is_file()]
    candidates = [f for f in files if f.endswith((".rs", ".py", ".go", ".ts", ".js"))]
    target = max(
        (f for f in candidates if (dest / f).stat().st_size < 300_000),
        key=lambda f: (dest / f).stat().st_size,
    )
    marker = "#" if target.endswith(".py") else "//"
    with open(dest / target, "a", encoding="utf-8") as fh:
        fh.write(f"\n{marker} hook-latency measurement line\n")
    env = dict(os.environ, GIT_AUTHOR_NAME="bench", GIT_AUTHOR_EMAIL="bench@invalid",
               GIT_COMMITTER_NAME="bench", GIT_COMMITTER_EMAIL="bench@invalid")
    subprocess.run(["git", "-C", str(dest), "checkout", "--quiet", "-b", "hook-latency"], check=True)
    subprocess.run(["git", "-C", str(dest), "commit", "--quiet", "--no-gpg-sign", "-am",
                    "bench: one-file change"], check=True, env=env)
    configured = (dest / "discipline.toml").exists()
    if not configured:
        subprocess.run([str(binary), "init"], cwd=dest, check=True, capture_output=True)
    # An existing hook file is never rewritten, so this repository keeps its own.
    subprocess.run([str(binary), "hook", "install", "--agent", "claude-code", "--observe"],
                   cwd=dest, capture_output=True)
    return {
        "source": source,
        "path": dest,
        "head": git(dest, "rev-parse", "HEAD"),
        "base": base,
        "large_base": large_base,
        "large_back": back,
        "files": len(files),
        "commits": commits,
        "changed_file": target,
        "had_discipline_toml": configured,
        "large_changed_files": len(git(dest, "diff", "--name-only", f"{large_base}...HEAD").splitlines()),
    }


def cells(repo: dict, binary: Path) -> list[tuple[str, list[str], str | None]]:
    b, lb = repo["base"], repo["large_base"]
    cwd = json.dumps(str(repo["path"]))
    path = json.dumps(str(repo["path"] / repo["changed_file"]))
    exe = str(binary)
    out = [
        ("git-diff-names-one-file", ["git", "diff", "--name-only", f"{b}...HEAD"], None),
        ("git-diff-one-file", ["git", "diff", f"{b}...HEAD"], None),
        ("git-diff-names-large", ["git", "diff", "--name-only", f"{lb}...HEAD"], None),
        ("git-diff-large", ["git", "diff", f"{lb}...HEAD"], None),
        ("check-one-file", [exe, "check", "--base", b, "--quiet"], None),
        ("check-large", [exe, "check", "--base", lb, "--quiet"], None),
        ("hook-post-edit", [exe, "hook", "run", "--agent", "claude-code", "--base", b, "--observe"],
         POST_EDIT.format(cwd=cwd, path=path)),
        ("hook-pre-tool", [exe, "hook", "run", "--agent", "claude-code", "--base", b,
                           "--event", "pre-tool", "--observe"], PRE_TOOL.format(cwd=cwd, path=path)),
    ]
    out += [(f"suite-{s}", [exe, "check", "--base", b, "--quiet", "--suite", s], None) for s in SUITES]
    return out


def time_cell(argv: list[str], stdin: str | None, cwd: Path, runs: int, warmup: int) -> dict:
    env = dict(os.environ, DISCIPLINE_NO_NETWORK="1", PR_BODY="", NO_COLOR="1")
    samples, codes, loads = [], [], []
    for i in range(warmup + runs):
        snap = require_idle()
        start = time.perf_counter()
        proc = subprocess.run(argv, cwd=cwd, input=stdin, capture_output=True, text=True, env=env)
        elapsed = time.perf_counter() - start
        if i >= warmup:
            samples.append(elapsed)
            codes.append(proc.returncode)
            loads.append(round(snap["load1"], 2))
    # Exit 2 is "could not check": its wall time is not the cost of a check.
    return {**summarise(samples), "samples_s": samples, "exit_codes": sorted(set(codes)),
            "load1_before_each": loads}


def host_info(binary: Path) -> dict:
    version = subprocess.run([str(binary), "--version"], capture_output=True, text=True).stdout.strip()
    info = {"platform": platform.platform(), "machine": platform.machine(),
            "cores": os.cpu_count(), "binary_version": version}
    if sys.platform == "darwin":
        info["cpu"] = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"],
                                     capture_output=True, text=True).stdout.strip()
        info["mem_bytes"] = int(subprocess.run(["sysctl", "-n", "hw.memsize"],
                                               capture_output=True, text=True).stdout.strip() or 0)
    return info


def table(results: dict) -> str:
    lines = ["| repo | cell | n | median s | min s | max s | IQR s | exits |",
             "|---|---|---|---|---|---|---|---|"]
    for repo in results["repos"]:
        for name, c in repo["cells"].items():
            lines.append(f"| {repo['name']} | `{name}` | {c['n']} | {c['median_s']:.3f} | "
                         f"{c['min_s']:.3f} | {c['max_s']:.3f} | {c['iqr_s']:.3f} | {c['exit_codes']} |")
    return "\n".join(lines)


def measure(args: argparse.Namespace) -> int:
    binary = Path(args.binary).resolve()
    try:
        first = require_idle()
    except HostBusy as busy:
        print(f"host not idle, nothing timed: {busy}", file=sys.stderr)
        return EXIT_BUSY
    results = {"host": host_info(binary), "host_before": first, "runs": args.runs,
               "warmup": args.warmup, "repos": []}
    with tempfile.TemporaryDirectory(prefix="hook-latency-") as tmp:
        for source in args.repo:
            src = str(Path(source).resolve()) if Path(source).exists() else source
            repo = prepare(src, Path(tmp), binary, args.large_back)
            entry = {k: v for k, v in repo.items() if k != "path"}
            entry["name"] = Path(src.rstrip("/")).name.removesuffix(".git")
            entry["cells"] = {}
            for name, argv, stdin in cells(repo, binary):
                try:
                    entry["cells"][name] = time_cell(argv, stdin, repo["path"], args.runs, args.warmup)
                except HostBusy as busy:
                    print(f"host went busy during {entry['name']}/{name}, stopping: {busy}",
                          file=sys.stderr)
                    results["stopped"] = {"cell": f"{entry['name']}/{name}", "reason": str(busy)}
                    results["repos"].append(entry)
                    Path(args.out).write_text(json.dumps(results, indent=2) + "\n")
                    return EXIT_BUSY
                print(f"{entry['name']}/{name}: median {entry['cells'][name]['median_s']:.3f} s",
                      file=sys.stderr)
            results["repos"].append(entry)
    results["host_after"] = host_snapshot()
    Path(args.out).write_text(json.dumps(results, indent=2) + "\n")
    print(table(results))
    return 0


def self_test() -> int:
    s = summarise([3.0, 1.0, 2.0, 5.0, 4.0])
    assert s["median_s"] == 3.0 and s["min_s"] == 1.0 and s["max_s"] == 5.0, s
    assert s["iqr_s"] == 2.0, s  # inclusive quartiles of 1..5 are 2 and 4
    assert summarise([1.0, 3.0])["iqr_s"] == 2.0
    try:
        summarise([])
    except ValueError:
        pass
    else:
        raise AssertionError("empty samples must raise")
    idle = {"load1": 3.9, "cores": 8, "heavy": []}
    assert idle_verdict(idle) is None
    assert "load1" in idle_verdict({**idle, "load1": 4.1})
    hot = {**idle, "heavy": [{"pid": 1, "cpu": 180.0, "command": "rustc"}]}
    assert "180% CPU" in idle_verdict(hot)
    print("measure_hook_latency self-test: ok")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--binary", default="target/release/discipline")
    p.add_argument("--repo", action="append", help="local path or clone URL; repeat for each repository")
    p.add_argument("--runs", type=int, default=7)
    p.add_argument("--warmup", type=int, default=1)
    p.add_argument("--large-back", type=int, default=200, help="commits back for the large-branch base")
    p.add_argument("--out", default="hook-latency.json")
    p.add_argument("--test", action="store_true", help="run the self-test and exit")
    args = p.parse_args()
    if args.test:
        return self_test()
    if not args.repo:
        p.error("--repo is required")
    return measure(args)


if __name__ == "__main__":
    sys.exit(main())
