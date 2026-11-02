#!/usr/bin/env python3
"""Time `check --suite integrity` with one integrity gate left as configured and the other
twelve disabled, in a scratch clone. The configuration change is committed as the base
commit, so base and head share a policy; the head commit is a one-line comment change.
Refuses (exit 3) when the host is not idle before any timed run.

Cells: `none` (all 13 integrity gates off: the shared start-up), `all` (as configured),
then one cell per gate. Method and figures: docs/benchmarks/hook-latency.md section 4.1.
Linux only (`ps --sort`). Run it as the owner of SRC, which `git clone` reads.

Usage:
    RUNS=3 python3 scripts/measure_integrity_gates.py BINARY SRC CHANGED_FILE OUT.json
    RUNS=3 python3 scripts/measure_integrity_gates.py target/release/discipline . \\
        src/ast/runner_collection.rs integrity-per-gate.json
"""
import json, os, re, shutil, statistics, subprocess, sys, time

GATES = ["config-integrity", "build-hooks", "toolchain-config", "sandbox-config",
         "ci-integrity", "ci-skip-set", "test-floor", "golden-output", "dependency-delta",
         "test-budget", "archive-contents", "manifest-sync", "version-lockstep"]
BIN, SRC, TOUCH, OUT = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
RUNS = int(os.environ.get("RUNS", "3"))
NCPU = os.cpu_count()


def sh(*a, cwd=None, check=True):
    return subprocess.run(a, cwd=cwd, check=check, capture_output=True, text=True)


def idle_or_exit(tag):
    load1 = os.getloadavg()[0]
    ps = sh("ps", "-eo", "pcpu,comm", "--sort=-pcpu").stdout.splitlines()[1:4]
    hot = [l for l in ps if float(l.split()[0]) > 100 and "python" not in l]
    if load1 > NCPU / 2 or hot:
        json.dump({"stopped_at": tag, "load1": load1, "hot": hot}, open(OUT + ".stopped", "w"))
        sys.exit(3)
    return load1


def set_disabled(toml, gate):
    hdr = f"[gates.{gate}]"
    lines = toml.split("\n")
    if hdr in lines:
        i = lines.index(hdr)
        j = i + 1
        while j < len(lines) and not lines[j].startswith("["):
            if re.match(r"\s*enabled\s*=", lines[j]):
                lines.pop(j)
                continue
            j += 1
        lines.insert(i + 1, "enabled = false")
        return "\n".join(lines)
    return toml + f"\n{hdr}\nenabled = false\n"


def prepare(keep, work):
    if os.path.exists(work):
        shutil.rmtree(work)
    sh("git", "clone", "-q", SRC, work)
    for k, v in [("user.name", "bench"), ("user.email", "bench@example.invalid"),
                 ("commit.gpgsign", "false")]:
        sh("git", "config", k, v, cwd=work)
    cfg = os.path.join(work, "discipline.toml")
    if not os.path.exists(cfg):
        sh(BIN, "init", cwd=work, check=False)
    toml = open(cfg).read()
    for g in GATES:
        if g not in keep:
            toml = set_disabled(toml, g)
    open(cfg, "w").write(toml)
    sh("git", "add", "-A", cwd=work)
    sh("git", "commit", "-q", "--allow-empty", "-m", "bench: base config", cwd=work)
    with open(os.path.join(work, TOUCH), "a") as f:
        f.write("\n// bench edit\n")
    sh("git", "commit", "-qam", "bench: one-line change", cwd=work)


def time_cell(work, tag):
    cmd = [BIN, "check", "--base", "HEAD~1", "--suite", "integrity", "--quiet"]
    env = dict(os.environ, DISCIPLINE_NO_NETWORK="1")
    subprocess.run(cmd, cwd=work, env=env, capture_output=True)  # warm-up
    samples, codes, loads = [], [], []
    for _ in range(RUNS):
        loads.append(idle_or_exit(tag))
        t = time.perf_counter()
        r = subprocess.run(cmd, cwd=work, env=env, capture_output=True, text=True)
        samples.append(time.perf_counter() - t)
        codes.append(r.returncode)
    return {"median": statistics.median(samples), "min": min(samples), "max": max(samples),
            "samples": samples, "exit_codes": codes, "load1": loads,
            "stderr_tail": r.stderr[-400:] if r.returncode == 2 else ""}


def main():
    idle_or_exit("preflight")
    work = os.path.join(os.path.dirname(os.path.abspath(OUT)), "work-" + os.path.basename(SRC))
    res = {"binary": sh(BIN, "--version").stdout.strip(), "repo": SRC,
           "head": sh("git", "rev-parse", "HEAD", cwd=SRC).stdout.strip(), "runs": RUNS, "cells": {}}
    for tag, keep in [("none", set()), ("all", set(GATES))] + [(g, {g}) for g in GATES]:
        prepare(keep, work)
        res["cells"][tag] = time_cell(work, tag)
        print(tag, round(res["cells"][tag]["median"], 3), res["cells"][tag]["exit_codes"], flush=True)
        json.dump(res, open(OUT, "w"), indent=1)


main()
