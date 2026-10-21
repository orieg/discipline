#!/usr/bin/env python3
"""
Independent labelling packet evaluator for `[hooks.premature-stop]` (#779).

Evaluates precision and recall of `src/turn.rs::judge` against human-reviewed labels
recorded in `tests/fixtures/premature_stop_recorded_endings.json`, computing two-sided
Wilson 95% confidence intervals matching `src/guards/perf/bounds.rs::wilson_score_interval`
(Wilson 1927, JASA 22(158):209-212) and checking the pre-registered `0.90` precision floor
(`docs/ROADMAP.md` Phase 17, Open Question 1).
"""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
import re
import sys
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PACKET = REPO_ROOT / "tests" / "fixtures" / "premature_stop_recorded_endings.json"

LEAD = r"(?:(?:ok(?:ay)?|alright|right|good|great|perfect|so|now|next|then|first|finally|and|but|also)[,.!:]?\s+)*"
INTENT = (
    r"(?:let me|let['\u2019]s|lets|i['\u2019]m (?:now )?going to|i am (?:now )?going to|"
    r"time to|now to|(?:now|next),? (?:i['\u2019]ll|i will|i need to|i have to|i can|we)|"
    r"i['\u2019]ll (?:now|next|go ahead and|start by|begin by|proceed to|first)|"
    r"i will (?:now|next|go ahead and|start by|begin by|proceed to|first)|"
    r"proceeding to|moving on to|starting with|"
    r"i need to (?:check|look|read|run|find|fix|see|examine|verify|investigate|inspect|update|add|write))"
)
ANNOUNCE_RE = re.compile(rf"^{LEAD}{INTENT}\b", re.IGNORECASE)
HANDS_OVER_RE = re.compile(
    r"\?\s*$|let me know|\bif you\b|\bif that\b|\bif so\b|\bonce\b|\bwhen\b|"
    r"\bafter (?:you|that|the|it|ci)\b|\buntil\b|\bunless\b|would you like|want me to|"
    r"should i\b|shall i\b|say (?:the word|go|yes)|your call|go-ahead|\bapprov|\bconfirm\b|"
    r"\bwait(?:ing)? for\b|\bnext (?:session|time|step is yours)\b|\blater\b|\btomorrow\b|"
    r"\bin a follow-up\b|\bfollow-up pr\b|\bwhenever\b|\bplease\b|\bfeel free\b|"
    r"\bas soon as\b|\bthen i['\u2019]ll\b",
    re.IGNORECASE,
)
NOT_AN_ACTION_RE = re.compile(
    rf"^{LEAD}(?:let me|i['\u2019]ll|i will|let['\u2019]s)\s+"
    r"(?:know|explain|summari[sz]e|be (?:clear|honest|direct)|recap|clarify|answer|address|"
    r"break (?:this|that|it) down|walk you|note|flag|stop here|leave|hold|wait|pause|stand by|"
    r"keep an eye|be here|assume|say|put it)\b",
    re.IGNORECASE,
)
TOOL_MARKUP_RE = re.compile(
    r"</?(?:tool_call|function_calls?|invoke|parameter)\b[^>]*>\s*$|<function=|<parameter=|<\|tool_call",
    re.IGNORECASE,
)
LIST_MARKER_RE = re.compile(r"^(?:[-*+]|\d+[.)])\s+")


def without_fences(text: str) -> str:
    parts = text.split("```")
    return " ".join(part for i, part in enumerate(parts) if i % 2 == 0)


def tail_sentence(text: str) -> str:
    outside = without_fences(text)
    lines = [l.strip() for l in outside.splitlines() if l.strip()]
    if not lines:
        return ""
    last = LIST_MARKER_RE.sub("", lines[-1])
    last = "".join(c for c in last if c not in "*_`").strip()
    start = 0
    for i, c in enumerate(last):
        if c not in ".!?":
            continue
        nxt = i + 1
        while nxt < len(last) and last[nxt].isspace():
            nxt += 1
        if nxt == i + 1:
            continue
        if nxt < len(last) and (last[nxt].isupper() or last[nxt] in "\"'("):
            start = nxt
    return last[start:].strip()


def judge(text: str, tool_markup: bool = True) -> tuple[str, str] | None:
    """Mirror `src/turn.rs::judge`."""
    trimmed = text.strip()
    tail = trimmed[-300:] if len(trimmed) > 300 else trimmed
    if tool_markup and TOOL_MARKUP_RE.search(tail):
        return ("tool_call_as_text", "")
    sent = tail_sentence(text)
    if (
        sent
        and len(sent.encode("utf-8")) <= 400
        and ANNOUNCE_RE.search(sent)
        and not NOT_AN_ACTION_RE.search(sent)
        and not HANDS_OVER_RE.search(sent)
    ):
        return ("announced_action", sent)
    return None


def wilson_score_interval(k: int, n: int, confidence_level: float = 0.95) -> tuple[float, float]:
    """Two-sided Wilson score interval (Wilson 1927), matching `src/guards/perf/bounds.rs`."""
    if n <= 0:
        raise ValueError("sample size n must be greater than 0")
    if k < 0 or k > n:
        raise ValueError(f"successes k ({k}) must be in [0, n ({n})]")
    z = 1.959_963_984_540_054 if abs(confidence_level - 0.95) < 1e-6 else 1.644_853_626_951_472_2
    p_hat = k / float(n)
    n_f = float(n)
    z2 = z * z
    denom = 1.0 + z2 / n_f
    center = (p_hat + z2 / (2.0 * n_f)) / denom
    radicand = (p_hat * (1.0 - p_hat) / n_f) + (z2 / (4.0 * n_f * n_f))
    margin = (z / denom) * math.sqrt(max(0.0, radicand))
    return (max(0.0, center - margin), min(1.0, center + margin))


def evaluate_packet(packet_path: Path = DEFAULT_PACKET) -> dict[str, Any]:
    doc = json.loads(packet_path.read_text(encoding="utf-8"))
    items = doc["items"]
    unlabelled = [item["id"] for item in items if item.get("reviewer_label") not in ("true-positive", "false-positive", "true-negative", "false-negative")]
    flagged_items = []
    for item in items:
        v = judge(item["text"], True)
        expected_flag = item["detector_flagged"]
        assert (v is not None) == expected_flag, f"detector mismatch on {item['id']}"
        if v is not None:
            flagged_items.append(item)

    labelled_flags = [i for i in flagged_items if i.get("reviewer_label") in ("true-positive", "false-positive")]
    k_all = sum(1 for i in labelled_flags if i["reviewer_label"] == "true-positive")
    n_all = len(labelled_flags)
    ci_all = wilson_score_interval(k_all, n_all, 0.95) if n_all > 0 else (0.0, 0.0)

    announce_flags = [i for i in labelled_flags if i["detector_kind"] == "announced_action"]
    k_ann = sum(1 for i in announce_flags if i["reviewer_label"] == "true-positive")
    n_ann = len(announce_flags)
    ci_ann = wilson_score_interval(k_ann, n_ann, 0.95) if n_ann > 0 else (0.0, 0.0)

    markup_flags = [i for i in labelled_flags if i["detector_kind"] == "tool_call_as_text"]
    k_mk = sum(1 for i in markup_flags if i["reviewer_label"] == "true-positive")
    n_mk = len(markup_flags)
    ci_mk = wilson_score_interval(k_mk, n_mk, 0.95) if n_mk > 0 else (0.0, 0.0)

    floor = 0.90
    if unlabelled:
        verdict = "PENDING_REVIEWER_LABELS"
    elif n_all == 0:
        verdict = "insufficient-n"
    elif ci_all[0] >= floor:
        verdict = "PASS"
    elif (k_all / float(n_all)) >= floor:
        verdict = "INTERMEDIATE_floor_within_ci"
    else:
        verdict = "REJECTED"

    return {
        "study_id": doc.get("study_id", "premature-stop-recorded-endings-v1"),
        "precision_floor": floor,
        "total_items_in_packet": len(items),
        "unlabelled_count": len(unlabelled),
        "flagged_total": {
            "k": k_all,
            "n": n_all,
            "point_estimate": round(k_all / float(n_all), 6) if n_all else None,
            "wilson_95_ci": [round(ci_all[0], 6), round(ci_all[1], 6)] if n_all else None,
            "verdict": verdict,
        },
        "announced_action_only": {
            "k": k_ann,
            "n": n_ann,
            "point_estimate": round(k_ann / float(n_ann), 6) if n_ann else None,
            "wilson_95_ci": [round(ci_ann[0], 6), round(ci_ann[1], 6)] if n_ann else None,
        },
        "tool_call_as_text_only": {
            "k": k_mk,
            "n": n_mk,
            "point_estimate": round(k_mk / float(n_mk), 6) if n_mk else None,
            "wilson_95_ci": [round(ci_mk[0], 6), round(ci_mk[1], 6)] if n_mk else None,
        },
    }


def run_self_tests() -> None:
    lo, hi = wilson_score_interval(17, 19, 0.95)
    assert abs(lo - 0.686059) < 1e-5 and abs(hi - 0.970641) < 1e-5
    lo_18, _ = wilson_score_interval(18, 19, 0.95)
    assert abs(lo_18 - 0.753613) < 1e-5
    lo_19, _ = wilson_score_interval(19, 19, 0.95)
    assert abs(lo_19 - 0.831821) < 1e-5
    # Verify minimum n needed for 100% precision (k=n) to clear 0.90 Wilson 95% lower bound is n=35
    assert wilson_score_interval(34, 34, 0.95)[0] < 0.90
    assert wilson_score_interval(35, 35, 0.95)[0] >= 0.90
    print("evaluate_premature_stop_labels.py: all self-tests passed.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test", action="store_true", help="Run statistical self-tests.")
    parser.add_argument(
        "--packet",
        type=Path,
        default=DEFAULT_PACKET,
        help="Path to the recorded-endings review packet JSON.",
    )
    args = parser.parse_args()
    if args.test:
        run_self_tests()
        return 0
    res = evaluate_packet(args.packet)
    print(json.dumps(res, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
