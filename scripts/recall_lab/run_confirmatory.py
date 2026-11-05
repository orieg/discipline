#!/usr/bin/env python3
"""#838 confirmatory runner (lab only, uncommitted; run_pilot.py plus RUNS_PER_CELL): per instance, dummies -> agent cells -> image removal.

Launched by start_pilot.sh, which holds the bench lock and exports LOCAL_API_KEY / LOCAL_BASE_URL
(never written to disk).

  start_pilot.sh --dummies-only [--limit N]          model-free cells only; no binary needed
  start_pilot.sh --binary-dir bin/v0.26.0 [--limit N]  dummies and agent cells

State: pilot-state.jsonl, append-only. A cell is final once a `done` record exists for its key.
A `started` record without `done` means the run was interrupted: its log directory is moved to
quarantine/ and the cell runs again (counted as `interrupted`). Infrastructure failures (model
endpoint, docker) are retried and counted separately; any other agent run without a final
state is excluded and counted, never replaced.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
HARNESS = ROOT / "impossiblebench"
UV = Path(os.path.expanduser("~/.local/bin/uv"))
TASK = "impossiblebench/src/impossiblebench/swebench_tasks.py@impossible_swebench"
DATASET = "fjzzq2002/impossible_swebench"
PIN = {
    "harness_commit": "d4f9a149db2b9cec0fed7bab01d91c07ee10aca2",
    "lock_sha256": "f1038a8e3f2726d8266873ec0af6a37adb55ada1380d185ba00123adb4dd0ed3",
    "dataset_swe_revision": "9c2d34f364b7229e8c0ff807c646100bdc18bbb5",
    "binary_version": "0.26.0",
    "binary_asset": "discipline-x86_64-unknown-linux-musl.tar.gz",
}
# `[arms] sampling` (PILOT): the server's own defaults, sent explicitly. The OpenAI-compatible
# provider drops --top-k, so top_k goes in the request body.
SAMPLING = ["--temperature", "1.0", "--top-p", "0.95", "--extra-body", '{"top_k": 20}', "--max-tokens", "16384"]
MESSAGE_LIMIT = 100
MAX_ATTEMPTS = 10
MIN_FREE_GB = 20
# `[arms] runs_per_cell`, amendment 3: 2 for access_full in the confirmatory split, 1 for access_readonly.
RUNS_PER_CELL = {"access_full": 2, "access_readonly": 1}
DUMMY_TRIES = 3
INFRA_TRIES = 3
CELL_TIMEOUT = {"dummy": 3600, "agent": 6 * 3600}
INFRA_RE = re.compile(r"APIConnectionError|APITimeoutError|Connection refused|ConnectError|RemoteProtocolError|"
                      r"docker|sandbox.*(start|fail)|No space left", re.I)
PROPS_KEYS = ("model_path", "chat_template")
STATE = ROOT / "confirmatory-state.jsonl"
PILOT_STATE = ROOT / "pilot-state.jsonl"
QUARANTINE = ROOT / "quarantine"

STOP = threading.Event()
LOCK = threading.Lock()
CHILDREN: set[subprocess.Popen] = set()


class Halt(Exception):
    """Stop the whole run without recording the current cell."""


def sh(*args: str, cwd: Path = ROOT, check: bool = True) -> str:
    return subprocess.run(args, cwd=cwd, check=check, capture_output=True, text=True).stdout


def free_gb() -> float:
    return shutil.disk_usage(ROOT).free / 1e9


def hub_revision() -> str:
    with urllib.request.urlopen(f"https://huggingface.co/api/datasets/{DATASET}", timeout=30) as r:
        return json.load(r)["sha"]


def endpoint_get(path: str) -> dict:
    req = urllib.request.Request(os.environ["LOCAL_BASE_URL"].removesuffix("/v1") + path,
                                 headers={"Authorization": f"Bearer {os.environ['LOCAL_API_KEY']}"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def props_fingerprint() -> dict:
    p = endpoint_get("/props")
    g = p.get("default_generation_settings", {})
    params = g.get("params", {})
    fp = {k: p.get(k) for k in PROPS_KEYS}
    fp["chat_template"] = hashlib.sha256((fp["chat_template"] or "").encode()).hexdigest()
    fp["n_ctx"] = g.get("n_ctx")
    fp["defaults"] = {k: params.get(k) for k in ("temperature", "top_p", "top_k", "min_p", "repeat_penalty",
                                                 "presence_penalty", "frequency_penalty")}
    return fp


def verify_binary(bin_dir: Path) -> dict:
    sums = (bin_dir / "SHA256SUMS").read_text().splitlines()
    want = next(l.split()[0] for l in sums if l.endswith(" " + PIN["binary_asset"]))
    got = hashlib.sha256((bin_dir / PIN["binary_asset"]).read_bytes()).hexdigest()
    if got != want:
        sys.exit(f"binary tarball sha {got} != SHA256SUMS {want}")
    exe = next(p for p in bin_dir.rglob("discipline") if p.is_file() and os.access(p, os.X_OK))
    ver = sh(str(exe), "--version").strip()
    if ver != f"discipline {PIN['binary_version']}":
        sys.exit(f"binary reports {ver!r}, pinned {PIN['binary_version']}")
    return {"version": PIN["binary_version"], "tarball_sha256": got,
            "binary_sha256": hashlib.sha256(exe.read_bytes()).hexdigest()}


def preflight(applied_diff_sha: str, need_model: bool) -> dict:
    head = sh("git", "rev-parse", "HEAD", cwd=HARNESS).strip()
    if head != PIN["harness_commit"]:
        sys.exit(f"harness HEAD {head} != pinned")
    diff_sha = hashlib.sha256(sh("git", "diff", cwd=HARNESS).encode()).hexdigest()
    if diff_sha != applied_diff_sha:
        sys.exit(f"harness working-tree diff sha {diff_sha} != recorded {applied_diff_sha}")
    freeze = sh(str(UV), "pip", "freeze", "--python", str(ROOT / "venv/bin/python"))
    # The committed lock omits the editable install of the harness (its line is a local path).
    freeze = "".join(l for l in freeze.splitlines(keepends=True) if not l.startswith("-e "))
    lock_sha = hashlib.sha256(freeze.encode()).hexdigest()
    if lock_sha != PIN["lock_sha256"]:
        sys.exit(f"environment freeze sha {lock_sha} != pinned lock")
    # Warm Inspect's dataset cache between two revision checks; children then run offline.
    if hub_revision() != PIN["dataset_swe_revision"]:
        sys.exit("dataset revision on the Hub != pinned")
    warm = ("from inspect_ai.dataset import hf_dataset, FieldSpec\n"
            "for s in ('original','oneoff','conflicting'):\n"
            f"    hf_dataset(path='{DATASET}', split=s, sample_fields=FieldSpec(input='problem_statement', id='instance_id'))\n")
    subprocess.run([str(ROOT / "venv/bin/python"), "-c", warm], cwd=ROOT, check=True, capture_output=True)
    if hub_revision() != PIN["dataset_swe_revision"]:
        sys.exit("dataset revision on the Hub moved during cache warm-up")
    if free_gb() < MIN_FREE_GB:
        sys.exit(f"free disk {free_gb():.1f} GB < {MIN_FREE_GB} GB")
    out = {"harness_commit": head, "harness_diff_sha256": diff_sha, "lock_sha256": lock_sha,
           "dataset_swe_revision": PIN["dataset_swe_revision"]}
    if need_model:
        for var in ("LOCAL_API_KEY", "LOCAL_BASE_URL"):
            if not os.environ.get(var):
                sys.exit(f"{var} not set")
        out["server"] = props_fingerprint()
    return out


def cells(instance: str, dummies_only: bool) -> list[dict]:
    out = [{"instance": instance, "split": "original", "arm": "access_full", "dummy": "oracle"}]  # positive control
    for split in ("oneoff", "conflicting"):
        for dummy in ("oracle", "nochange"):
            out.append({"instance": instance, "split": split, "arm": "access_full", "dummy": dummy})
    if not dummies_only:
        for split in ("oneoff", "conflicting", "original"):
            for arm in ("access_full", "access_readonly"):
                for run in range(1, RUNS_PER_CELL[arm] + 1):
                    out.append({"instance": instance, "split": split, "arm": arm, "dummy": None, "run": run})
    return out


def cell_key(c: dict) -> str:
    k = f"{c['instance']}|{c['split']}|{c['arm']}|{c['dummy'] or 'agent'}"
    return k if c.get("run", 1) == 1 else f"{k}|r{c['run']}"


def cell_dir(log_root: Path, c: dict) -> Path:
    d = f"{c['split']}__{c['arm']}__{c['dummy'] or 'agent'}"
    return log_root / c["instance"] / (d if c.get("run", 1) == 1 else f"{d}__r{c['run']}")


def load_state() -> dict[str, list[dict]]:
    recs: dict[str, list[dict]] = {}
    if STATE.exists():
        for line in STATE.read_text().splitlines():
            rec = json.loads(line)
            recs.setdefault(rec.get("key", ""), []).append(rec)
    return recs


def record(rec: dict) -> None:
    rec = {"t": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), **rec}
    with LOCK, STATE.open("a") as f:
        f.write(json.dumps(rec, sort_keys=True) + "\n")


def quarantine(d: Path, why: str) -> None:
    if d.exists() and any(d.iterdir()):
        dest = QUARANTINE / f"{d.parent.name}__{d.name}__{int(time.time())}"
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(d), dest)
        record({"key": "quarantine", "from": str(d.relative_to(ROOT)), "to": str(dest.relative_to(ROOT)), "why": why})


def read_log(log: Path, instance: str) -> dict:
    """Status, the swe_bench_scorer verdict, the sample error and oracle-application evidence."""
    # JSON logs: `.eval` archives use a compression the stdlib zipfile cannot read, and
    # `recall_pairs.py` reads the same logs with the stdlib.
    data = json.loads(log.read_text(encoding="utf-8"))
    samples = data.get("samples") or []
    out = {"status": data.get("status"), "n_samples": len(samples), "passed": None, "error": None,
           "oracle_applied": None}
    if len(samples) != 1:
        return out
    s = samples[0]
    if s.get("id") != instance:
        out["error"] = f"sample id {s.get('id')!r} != {instance!r}"
        return out
    if s.get("error"):
        out["error"] = str(s["error"].get("message", s["error"]))[:500]
    sc = (s.get("scores") or {}).get("swe_bench_scorer")
    if isinstance(sc, dict) and not s.get("error"):
        out["passed"] = sc.get("value") in (1, 1.0, "C", True)
    # Mirrors `oracle_applied` in recall_pairs.py: the exit code of the oracle's `git apply`.
    for ev in s.get("events") or []:
        if ev.get("event") == "sandbox" and ev.get("action") == "exec" and \
                "git apply --check /tmp/oracle_patch.diff" in str(ev.get("cmd")):
            out["oracle_applied"] = ev.get("result") == 0
            break
    return out


def run_cell(c: dict, model: str, log_root: Path) -> dict:
    d = cell_dir(log_root, c)
    quarantine(d, "stale files before a fresh run")
    d.mkdir(parents=True, exist_ok=True)
    kind = "dummy" if c["dummy"] else "agent"
    args = [str(ROOT / "venv/bin/inspect"), "eval", TASK,
            "-T", f"split={c['split']}", "-T", f"instance_ids=[{c['instance']}]", "-T", "agent_type=tools",
            "-T", f"message_limit={MESSAGE_LIMIT}", "-T", f"max_attempts={MAX_ATTEMPTS}",
            "--log-dir", str(d), "--log-format", "json", "--max-connections", "1", "--no-fail-on-error",
            "--display", "plain"]
    if c["arm"] == "access_readonly":
        args += ["-T", "reset_tests=true"]
    if c["dummy"]:
        args += ["-T", f"dummy={c['dummy']}", "--model", "mockllm/model"]
    else:
        args += ["--model", model, "--timeout", "1800", *SAMPLING]
    env = {**os.environ, "HF_HUB_OFFLINE": "1", "HF_DATASETS_OFFLINE": "1"}
    t0 = time.time()
    p = subprocess.Popen(args, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                         start_new_session=True)
    with LOCK:
        CHILDREN.add(p)
    try:
        out, _ = p.communicate(timeout=CELL_TIMEOUT[kind])
        timed_out = False
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGTERM)
        out, _ = p.communicate()
        timed_out = True
    finally:
        with LOCK:
            CHILDREN.discard(p)
    (d / "inspect-output.txt").write_text(out[-40000:])
    logs = sorted(d.glob("*.json"))
    res = {"rc": p.returncode, "seconds": round(time.time() - t0), "timed_out": timed_out,
           "logs": [str(x.relative_to(ROOT)) for x in logs]}
    if STOP.is_set() or (p.returncode is not None and p.returncode < 0 and not timed_out):
        raise Halt("interrupted")
    if len(logs) == 1:
        res.update(read_log(logs[0], c["instance"]))
    else:
        res.update({"status": None, "passed": None, "error": f"{len(logs)} logs in the cell directory"})
    if res.get("status") == "cancelled":
        raise Halt("cancelled")
    blob = f"{res.get('error') or ''}\n{out[-8000:]}" if res.get("passed") is None else ""
    res["infra"] = bool(blob and INFRA_RE.search(blob))
    return res


def ensure_endpoint(fp0: dict) -> None:
    try:
        fp = props_fingerprint()
    except Exception as e:  # noqa: BLE001 - any failure to reach the server halts the run
        raise Halt(f"model endpoint unreachable: {e}") from e
    if fp != fp0:
        raise Halt(f"model server changed: {json.dumps(fp, sort_keys=True)}")


def run_instance(instance: str, model: str, log_root: Path, dummies_only: bool, fp0: dict | None) -> None:
    state = load_state()
    done: dict[str, dict] = {}
    for c in cells(instance, dummies_only):
        k = cell_key(c)
        recs = state.get(k, [])
        finals = [r for r in recs if r.get("phase") == "done"]
        if finals:
            done[k] = finals[-1]
            continue
        if any(r.get("phase") == "started" for r in recs):
            record({"key": k, "phase": "interrupted"})
            quarantine(cell_dir(log_root, c), "interrupted run")
    try:
        for c in cells(instance, dummies_only):
            if STOP.is_set():
                return
            k = cell_key(c)
            if k in done:
                continue
            if free_gb() < MIN_FREE_GB:
                raise Halt(f"free disk {free_gb():.1f} GB < {MIN_FREE_GB} GB")
            skip = validity_skip(instance, c, done)
            if skip:
                done[k] = {"key": k, **c, "phase": "done", "excluded": skip, "generated": False}
                record(done[k])
                continue
            tries = DUMMY_TRIES if c["dummy"] else INFRA_TRIES
            for attempt in range(1, tries + 1):
                if not c["dummy"]:
                    ensure_endpoint(fp0)
                record({"key": k, **c, "phase": "started", "attempt": attempt})
                res = run_cell(c, model, log_root)
                retry = res["passed"] is None and (c["dummy"] or res["infra"]) and attempt < tries
                if retry:
                    record({"key": k, **c, "phase": "retry", "attempt": attempt, **res})
                    quarantine(cell_dir(log_root, c), f"retry after attempt {attempt}")
                    continue
                excluded = None
                if res["passed"] is None:
                    excluded = ("dummy-error" if c["dummy"] else
                                "infrastructure-error" if res["infra"] else "no-final-state")
                done[k] = {"key": k, **c, "phase": "done", "attempt": attempt, "excluded": excluded, **res}
                record(done[k])
                break
    finally:
        remove_images(instance)


def validity_skip(instance: str, c: dict, done: dict[str, dict]) -> str | None:
    """`[population].validity_filter` (amendment 2): impossible-split agent cells are generated only when
    the oracle applied and passed `original`, and both dummies ran and failed that split."""
    if c["dummy"] or c["split"] == "original":
        return None
    pos = done.get(f"{instance}|original|access_full|oracle", {})
    if pos.get("passed") is not True or pos.get("oracle_applied") is not True:
        return "oracle-positive-control-failed"
    ds = [done.get(f"{instance}|{c['split']}|access_full|{x}", {}) for x in ("oracle", "nochange")]
    if any(r.get("passed") is None for r in ds):
        return "validity-filter-not-run"
    if ds[0].get("oracle_applied") is not True:
        return "oracle-not-applied"
    if any(r["passed"] for r in ds):
        return "validity-filter-dummy-passes"
    return None


def remove_images(instance: str) -> None:
    tag = instance.replace("__", "_1776_").lower()
    pat = re.compile(rf"^(?:[\w.-]+/)?sweb\.eval\.x86_64\.{re.escape(tag)}:[\w.-]+$")
    out = sh("docker", "images", "--format", "{{.Repository}}:{{.Tag}}", check=False)
    for img in out.split():
        if not pat.match(img):
            continue
        # A cell stopped mid-run leaves its exited container behind, which pins the image. This
        # instance's cells are all finished here, so none of its containers is still in use.
        stale = sh("docker", "ps", "-a", "-q", "--filter", f"ancestor={img}", "--filter", "status=exited",
                   "--filter", "status=created", "--filter", "status=dead", check=False).split()
        if stale:
            sh("docker", "rm", *stale, check=False)
        if sh("docker", "images", "-q", img, check=False).strip():
            subprocess.run(["docker", "rmi", img], capture_output=True)
        if sh("docker", "images", "-q", img, check=False).strip():
            record({"key": "image-not-removed", "instance": instance, "image": img})


def on_signal(signum, _frame) -> None:
    STOP.set()
    with LOCK:
        for p in list(CHILDREN):
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ids", default=str(ROOT / "confirmatory-ids-swe.txt"))
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--log-root", default=str(ROOT / "logs/confirmatory/f1"))
    ap.add_argument("--dummies-only", action="store_true")
    ap.add_argument("--binary-dir", default=None, help="verified release directory; required for agent cells")
    args = ap.parse_args()
    signal.signal(signal.SIGINT, on_signal)
    signal.signal(signal.SIGTERM, on_signal)
    manifest = json.loads((ROOT / "manifest-local.json").read_text())
    model = "openai-api/local/" + manifest["families"]["family-1"]["identifier"]
    binary = None
    if not args.dummies_only:
        if not args.binary_dir:
            sys.exit("agent cells need --binary-dir with the verified pinned release")
        binary = verify_binary(Path(args.binary_dir).resolve())
    pre = preflight(manifest["harness_applied_diff_sha256"], need_model=not args.dummies_only)
    # The locked arm: the same model server and binary the pilot ran against.
    pilot_starts = [r for r in (json.loads(l) for l in PILOT_STATE.read_text().splitlines())
                    if r.get("key") == "start" and not r.get("dummies_only")]
    if not args.dummies_only:
        if pre["server"] != pilot_starts[-1]["preflight"]["server"]:
            sys.exit(f"model server differs from the pilot: {json.dumps(pre['server'], sort_keys=True)}")
        if binary != pilot_starts[-1]["binary"]:
            sys.exit("binary differs from the pilot")
    pilot_ids = set((ROOT / "pilot-ids-swe.txt").read_text().split())
    log_root = Path(args.log_root).resolve()
    if not log_root.is_relative_to(ROOT):
        sys.exit("--log-root must be under the run directory")
    # Hash order, not alphabetical, so any prefix of the run is a mix of repositories.
    ids = sorted((i for i in Path(args.ids).read_text().split() if i),
                 key=lambda i: hashlib.sha256(i.encode()).hexdigest())
    if pilot_ids & set(ids):
        sys.exit("a pilot instance is in the confirmatory id list")
    if args.limit:
        ids = ids[: args.limit]
    record({"key": "start", "preflight": pre, "binary": binary, "dummies_only": args.dummies_only,
            "sampling": SAMPLING, "message_limit": MESSAGE_LIMIT, "max_attempts": MAX_ATTEMPTS,
            "instances": len(ids), "workers": args.workers,
            "population_split": "confirmatory", "runs_per_cell": RUNS_PER_CELL})
    halted = None
    with cf.ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = {ex.submit(run_instance, i, model, log_root, args.dummies_only, pre.get("server")): i for i in ids}
        for f in cf.as_completed(futs):
            if f.cancelled():
                continue
            try:
                f.result()
            except Halt as h:
                halted = halted or f"{futs[f]}: {h}"
                on_signal(None, None)
                ex.shutdown(wait=True, cancel_futures=True)
            except Exception as e:  # noqa: BLE001 - surface at once, stop the rest
                halted = halted or f"{futs[f]}: {type(e).__name__}: {e}"
                on_signal(None, None)
                ex.shutdown(wait=True, cancel_futures=True)
    record({"key": "end", "halted": halted, "stopped": STOP.is_set()})
    if halted or STOP.is_set():
        sys.exit(f"halted: {halted or 'signal'}")


if __name__ == "__main__":
    main()
