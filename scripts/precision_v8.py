#!/usr/bin/env python3
"""scripts/precision_v8.py — the stages of `agent-pr-precision-v8`
(`docs/benchmarks/precision-preregistration-v8.toml`).

  packet   build the source-tool-blinded packet from a raw replay of the v6 manifest:
           every blocking finding and the 150 twin flags of v6's seeded sample, with v6's
           packet fields. An item matching a v6 packet item on (repo, commit, file, line,
           rule summary) keeps its id; the others get `PKT-1nnn` ids. Writes the packet,
           the ground key (kept out of the raters' reach until grading ends) and the
           rater batches of 20.
  report   combine the two rater slots and the owner's answers into final labels; write
           adjudication.json, findings.json and summary.json (block rates, rule-fidelity
           and actionability shares per class and gate with Wilson and cluster-bootstrap
           intervals, kappa per question, degenerate slots, owner-final counts, verdicts).

The replay, the twin rule, the rule summaries, the diff excerpts and the redaction are
`scripts/benchmark_precision.py`'s, imported, so the packet is built as v6's was.

  python3 scripts/precision_v8.py packet --raw raw.json --clones-dir <clones> \\
      --v6-packet docs/benchmarks/results-v6/rater-packet.json --out-dir <dir>
  python3 scripts/precision_v8.py --test
"""

import argparse
import importlib
import json
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
bp = importlib.import_module("benchmark_precision")

SHUFFLE_SEED = 80804
BATCH_SIZE = 20
TWIN_SAMPLE = 150
TWIN_SEED = 482
BOOTSTRAP_SEED = 20261110
BOOTSTRAP_RESAMPLES = 20000
KAPPA_BAR = 0.75
FLOORS = {"structural-ast-and-deterministic": 0.90, "heuristic-and-prose": 0.80}
REQUIRED_N = {"structural-ast-and-deterministic": 153, "heuristic-and-prose": 184}
QUESTIONS = {"rule_fidelity": ("true-positive", "false-positive", "cannot-tell"),
             "actionability": ("would-block", "would-not-block", "cannot-tell")}
SUCCESS = {"rule_fidelity": "true-positive", "actionability": "would-block"}


def match_key(repo: str, commit12: str, file: str, line, rule: str) -> tuple:
    return (repo, commit12, file, line, rule)


def raw_items(cases: list[dict], clones_dir: Path) -> list[dict]:
    """Every blocking finding and the seeded twin flags, unblinded, in case order."""
    repo_map = dict(bp.EVALUATION_REPOS)
    file_cache: dict = {}
    items = []
    for case in cases:
        for v in case["violations"]:
            if v["code"] not in bp.RULE_SUMMARIES:
                raise SystemExit(f"no rule summary for {v['code']}: write one from docs/GATES.md first")
            items.append({
                "source_tool": "discipline", "gate": v["gate"], "code": v["code"],
                "repo": case["repo"], "sha": case["sha"], "parent": case["parent"],
                "pr_number": case["pr_number"], "subject": case["subject"],
                "file": v["file"], "line": v["line"], "rule_summary": bp.RULE_SUMMARIES[v["code"]],
                "diff_excerpt": bp._get_diff_excerpt(clones_dir, repo_map, case["repo"], case["parent"], case["sha"], v["file"], v["line"]),
                "line_context": bp._get_line_context(clones_dir, repo_map, file_cache, case["repo"], case["sha"], v["file"], v["line"]),
            })
    for case, path, tcode in bp.twin_sample(cases, TWIN_SAMPLE, TWIN_SEED):
        items.append({
            "source_tool": "naive-sensitive-path-filter", "gate": "naive-sensitive-path-filter", "code": tcode,
            "repo": case["repo"], "sha": case["sha"], "parent": case["parent"],
            "pr_number": case["pr_number"], "subject": case["subject"],
            "file": path, "line": None, "rule_summary": bp.RULE_SUMMARIES[tcode],
            "diff_excerpt": bp._get_diff_excerpt(clones_dir, repo_map, case["repo"], case["parent"], case["sha"], path, None),
            "line_context": None,
        })
    return items


def assign_ids(items: list[dict], v6_items: list[dict]) -> None:
    """Keep the v6 id of a matching item; number the rest PKT-1001 up, in shuffled order."""
    v6 = {}
    for it in v6_items:
        v6.setdefault(match_key(it["repo"], it["commit_sha"], it["file"], it["line"], it["rule_summary"]), []).append(it["item_id"])
    random.Random(SHUFFLE_SEED).shuffle(items)
    fresh = 1001
    for it in items:
        ids = v6.get(match_key(it["repo"], it["sha"][:12], it["file"], it["line"], it["rule_summary"]))
        if ids:
            it["item_id"] = ids.pop(0)
        else:
            it["item_id"] = f"PKT-{fresh}"
            fresh += 1


def blinded(it: dict) -> dict:
    return {
        "item_id": it["item_id"], "repo": it["repo"], "commit_sha": it["sha"][:12],
        "commit_subject": it["subject"], "file": it["file"], "line": it["line"],
        "rule_summary": it["rule_summary"],
        "diff_excerpt": bp._redact_packet_pii_literals(it["diff_excerpt"]),
        "line_context": bp._redact_packet_pii_literals(it["line_context"]),
    }


def packet(args) -> int:
    cases = json.loads(Path(args.raw).read_text())
    items = raw_items(cases, Path(args.clones_dir))
    assign_ids(items, json.loads(Path(args.v6_packet).read_text())["items"])
    out = Path(args.out_dir)
    (out / "batches").mkdir(parents=True, exist_ok=True)
    pkt = [blinded(it) for it in items]
    (out / "rater-packet.json").write_text(json.dumps({
        "study_id": "agent-pr-precision-v8", "blind_to_source_tool": True, "blind_to_gate_name": True,
        "blind_to_previous_labels": True, "twin_sample_seed": TWIN_SEED, "packet_shuffle_seed": SHUFFLE_SEED,
        "batch_size": BATCH_SIZE, "total_items": len(pkt), "items": pkt}, indent=2) + "\n")
    (out / "ground-key.json").write_text(json.dumps({it["item_id"]: {k: it[k] for k in (
        "source_tool", "gate", "code", "repo", "sha", "pr_number", "file", "line")} for it in items}, indent=2) + "\n")
    for i in range(0, len(pkt), BATCH_SIZE):
        (out / "batches" / f"batch-{i // BATCH_SIZE + 1:02d}.json").write_text(
            json.dumps({"items": pkt[i:i + BATCH_SIZE]}, indent=2) + "\n")
    kept = sum(1 for it in items if not it["item_id"].startswith("PKT-1"))
    print(f"{len(pkt)} items ({sum(it['source_tool'] == 'discipline' for it in items)} discipline, "
          f"{sum(it['source_tool'] != 'discipline' for it in items)} twin), {kept} keep a v6 id; "
          f"{-(-len(pkt) // BATCH_SIZE)} batches")
    return 0


def load_slot(d: Path, nested: bool) -> dict:
    out = {}
    for f in sorted(d.glob("batch-??.json")):
        doc = json.loads(f.read_text())
        out.update(doc["labels"] if nested else doc)
    return out


def share(k: int, n: int) -> dict:
    lo, hi = bp.wilson_score_interval(k, n) if n else (None, None)
    return {"k": k, "n": n, "point": (k / n) if n else None, "wilson_95": [lo, hi]}


def cluster_ci(items: list, key: str, q: str) -> list:
    groups: dict = {}
    for it in items:
        k, n = groups.get(it[key], (0, 0))
        groups[it[key]] = (k + (it["final"][q] == SUCCESS[q]), n + 1)
    if len(groups) < 2:
        return [None, None]
    return list(bp.pr_cluster_bootstrap_ci(list(groups.values()), BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED))


def kappa_ci(a: list, b: list) -> list:
    rng = random.Random(BOOTSTRAP_SEED)
    n, ks = len(a), []
    for _ in range(2000):
        idx = [rng.randrange(n) for _ in range(n)]
        ks.append(bp.cohens_kappa([a[i] for i in idx], [b[i] for i in idx]))
    ks.sort()
    return [ks[int(0.025 * len(ks))], ks[int(0.975 * len(ks)) - 1]]


def degenerate(labels: list) -> bool:
    """v6's rule as v7 applies it: one slot answering a single label on (nearly) every item."""
    top = max(labels.count(x) for x in set(labels))
    return top / len(labels) >= 0.95 or len(set(labels)) == 1


def report(args) -> int:
    d = Path(args.dir)
    gk = json.loads((d / "ground-key.json").read_text())
    r1, r2 = load_slot(d / "r1", False), load_slot(d / "r2", True)
    owner = json.loads(Path(args.owner).read_text())
    ids = sorted(gk)
    if set(r1) != set(ids) or set(r2) != set(ids):
        raise SystemExit("a rater slot does not cover the packet")
    items, owner_final = [], {q: 0 for q in QUESTIONS}
    for i in ids:
        disagree = r1[i] != r2[i]
        if disagree and i not in owner:
            raise SystemExit(f"{i}: the slots disagree and the owner has not answered")
        final = {}
        for q in QUESTIONS:
            if disagree:
                final[q] = owner[i][q]
                owner_final[q] += 1
            else:
                final[q] = r1[i][q]
        g = gk[i]
        cls = "twin" if g["source_tool"] != "discipline" else bp.gate_class_of(g["gate"])
        items.append({"item_id": i, **g, "gate_class": cls, "pr": f"{g['repo']}#{g['pr_number']}",
                      "rater_1": r1[i], "rater_2": r2[i], "owner": owner.get(i), "final": final})
    raw = json.loads(Path(args.raw).read_text())
    blocked = sum(1 for c in raw if c["violations"])
    twin_blocked = sum(1 for c in raw if any(bp.classify_twin_sensitive_path(p, p in set(c["base_existing"])) for p in c["paths"]))
    classes = {}
    for cls in ("structural-ast-and-deterministic", "heuristic-and-prose", "twin"):
        sub = [it for it in items if it["gate_class"] == cls]
        entry = {}
        for q in QUESTIONS:
            k = sum(it["final"][q] == SUCCESS[q] for it in sub)
            entry[q] = {**share(k, len(sub)), "pr_cluster_bootstrap_95": cluster_ci(sub, "pr", q),
                        "repository_cluster_bootstrap_95": cluster_ci(sub, "repo", q),
                        "labels": {lab: sum(it["final"][q] == lab for it in sub) for lab in QUESTIONS[q]}}
        if cls in FLOORS:
            rf = entry["rule_fidelity"]
            if rf["n"] < REQUIRED_N[cls]:
                would = "insufficient-n"
            elif rf["wilson_95"][0] >= FLOORS[cls] and rf["repository_cluster_bootstrap_95"][0] >= FLOORS[cls]:
                would = "PASS"
            else:
                would = "INTERMEDIATE_floor_within_ci" if rf["point"] >= FLOORS[cls] else "REJECTED"
            entry["verdict_labels_would_give"] = would
            entry["verdict"] = would if would == "insufficient-n" else "INTERMEDIATE_unreliable_labels"
            entry["verdict_qualifier"] = "no-human-labels"
            entry["floor"], entry["required_n"] = FLOORS[cls], REQUIRED_N[cls]
        classes[cls] = entry
    per_gate = {}
    for it in items:
        if it["gate_class"] == "twin":
            continue
        e = per_gate.setdefault(it["gate"], {"class": it["gate_class"], "n": 0, "true_positive": 0, "false_positive": 0, "cannot_tell": 0})
        e["n"] += 1
        e[it["final"]["rule_fidelity"].replace("-", "_")] += 1
    reliability = {}
    for q in QUESTIONS:
        a, b = [r1[i][q] for i in ids], [r2[i][q] for i in ids]
        po = sum(x == y for x, y in zip(a, b)) / len(ids)
        k = bp.cohens_kappa(a, b)
        reliability[q] = {"kappa": k, "kappa_95": kappa_ci(a, b), "observed_agreement": po,
                          "prevalence_adjusted_agreement": 2 * po - 1,
                          "majority_label_share": max(a.count(x) + b.count(x) for x in set(a) | set(b)) / (2 * len(ids)),
                          "rater_1_degenerate": degenerate(a), "rater_2_degenerate": degenerate(b),
                          "meets_bar": k >= KAPPA_BAR}
    summary = {
        "study_id": "agent-pr-precision-v8", "code_commit": "87447255f2d0f4d44644e6f801c2d49cc9e6d87c",
        "binary_sha256": args.binary_sha256, "host": args.host,
        "rater_kind": "llm-subagent and llm-cli, owner adjudication",
        "synthetic_rater_disclosure": "Rater 1 (Claude subagents) and Rater 2 (Gemini through the agy CLI) are AI models, not people. The owner, who wrote discipline, settled every item the two answered differently. No independent human labelled any item.",
        "pull_request_block_rates": {"total_pull_requests": len(raw), "discipline": share(blocked, len(raw)),
                                     "naive_sensitive_path_filter": share(twin_blocked, len(raw))},
        "reliability": reliability, "owner_final_labels": owner_final, "classes": classes, "per_gate": per_gate,
        "bootstrap_seed": BOOTSTRAP_SEED, "bootstrap_resamples": BOOTSTRAP_RESAMPLES,
    }
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    (out / "adjudication.json").write_text(json.dumps({"items": [{k: it[k] for k in (
        "item_id", "rater_1", "rater_2", "owner", "final")} for it in items]}, indent=2) + "\n")
    (out / "findings.json").write_text(json.dumps({"items": [{k: it[k] for k in (
        "item_id", "source_tool", "gate", "code", "gate_class", "repo", "sha", "pr_number", "file", "line")} for it in items]}, indent=2) + "\n")
    s = classes["structural-ast-and-deterministic"]["rule_fidelity"]
    print(f"blocked {blocked}/{len(raw)}; structural rule fidelity {s['k']}/{s['n']}; kappa "
          f"{reliability['rule_fidelity']['kappa']:.3f}; owner-final {owner_final}")
    return 0


def self_test() -> int:
    v6 = [{"item_id": "PKT-0007", "repo": "a/b", "commit_sha": "0123456789ab", "file": "f", "line": 3, "rule_summary": "r"}]
    items = [{"repo": "a/b", "sha": "0123456789abcdef", "file": "f", "line": 3, "rule_summary": "r"},
             {"repo": "a/b", "sha": "0123456789abcdef", "file": "g", "line": 3, "rule_summary": "r"}]
    assign_ids(items, v6)
    assert sorted(it["item_id"] for it in items) == ["PKT-0007", "PKT-1001"], items
    print("precision_v8 self-test: ok")
    return 0


def main() -> int:
    if "--test" in sys.argv:
        return self_test()
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="stage", required=True)
    pk = sub.add_parser("packet")
    pk.add_argument("--raw", required=True)
    pk.add_argument("--clones-dir", required=True)
    pk.add_argument("--v6-packet", required=True)
    pk.add_argument("--out-dir", required=True)
    rp = sub.add_parser("report")
    rp.add_argument("--dir", required=True, help="the packet stage's directory, with r1/ and r2/ batch answers")
    rp.add_argument("--owner", required=True, help="JSON {item_id: {rule_fidelity, actionability}} of the owner's answers")
    rp.add_argument("--raw", required=True)
    rp.add_argument("--binary-sha256", required=True)
    rp.add_argument("--host", required=True)
    rp.add_argument("--out-dir", required=True)
    a = p.parse_args()
    return packet(a) if a.stage == "packet" else report(a)


if __name__ == "__main__":
    sys.exit(main())
