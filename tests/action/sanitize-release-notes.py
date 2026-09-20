#!/usr/bin/env python3
"""Release notes and PR title mention sanitiser and verification sentinel.

Enforces:
1. Release notes never @-mention arbitrary GitHub accounts. Only the trailing
   attribution `by @<login> in <url>` on PR entries survives as an active mention.
   All other `@name` occurrences are neutralised by enclosing them in code spans (`@name`).
   The job fails closed if any `@` outside an attribution or code span remains.
2. PR titles never contain unquoted `@word` mentions, preventing unintended user
   mentions from entering generated release notes verbatim.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Trailing PR attribution in GitHub generated release notes: ` by @<login> in <url>`
ATTR_RE = re.compile(r'( by @[a-zA-Z0-9_-]+ in https?://\S+)\s*$')

# Code spans in markdown: `...`
CODE_SPAN_RE = re.compile(r'(`[^`]*`)')

# GitHub mention pattern: @name outside word characters or backticks
MENTION_RE = re.compile(r'(?<![\w`])@([a-zA-Z0-9_-]+)')

# PII patterns: home directory paths, agent config paths, private LAN IPs
PII_PATTERNS = [
    (re.compile(r'(?:/Users/|/home/)[a-zA-Z0-9_.-]+'), "home directory path leak"),
    (re.compile(r'(?:~/|\$HOME/)\.(?:claude|gemini|antigravity)\b'), "personal agent config path"),
    (re.compile(r'\b(?:192\.168\.\d{1,3}\.\d{1,3}|10\.\d{1,3}\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3})\b'), "private LAN IP leak"),
]

# Time estimate patterns: clock/calendar durations and sprint projections
TIME_ESTIMATE_PATTERNS = [
    (re.compile(r'\b\d+(?:-\d+)?\s*(?:days?|weeks?|months?|sprints?|engineer-days?)\b', re.IGNORECASE), "time estimate duration"),
    (re.compile(r'\bnext sprint\b', re.IGNORECASE), "sprint projection"),
    (re.compile(r'\bPhase\s+\d+\s*\([^)]*(?:day|week|month)[^)]*\)', re.IGNORECASE), "phase duration estimate"),
]

# Metric claim pattern: e.g. "3x faster", "40% speedup" requiring (measured: ...), (target), or (projected)
METRIC_CLAIM_RE = re.compile(r'\b\d+(?:\.\d+)?\s*(?:x|%)\s*(?:faster|speedup|regression|improvement)\b', re.IGNORECASE)
PROVENANCE_TAG_RE = re.compile(r'\((?:measured:[^)]+|target|projected)\)', re.IGNORECASE)


def sanitize_line(line: str) -> str:
    """Neutralise every @name in line that is NOT in the trailing attribution."""
    attr_match = ATTR_RE.search(line)
    if attr_match:
        prefix = line[: attr_match.start()]
        suffix = line[attr_match.start() :]
    else:
        prefix = line
        suffix = ""

    tokens = CODE_SPAN_RE.split(prefix)
    sanitized_tokens: list[str] = []
    for token in tokens:
        if token.startswith("`") and token.endswith("`") and len(token) >= 2:
            # Already inside a code span; keep untouched
            sanitized_tokens.append(token)
        else:
            # Regular prose: wrap any unquoted @mention in backticks
            sanitized_tokens.append(MENTION_RE.sub(r"`@\1`", token))

    return "".join(sanitized_tokens) + suffix


def sanitize_notes(text: str) -> str:
    """Sanitize all lines of release notes text."""
    return "\n".join(sanitize_line(line) for line in text.splitlines())


def verify_release_notes(text: str) -> list[str]:
    """Return all lines where policy is violated (unverified @, PII, time estimates, unprovenanced metrics)."""
    violations: list[str] = []
    for idx, line in enumerate(text.splitlines(), start=1):
        attr_match = ATTR_RE.search(line)
        prefix = line[: attr_match.start()] if attr_match else line
        without_code = CODE_SPAN_RE.sub("", prefix)
        if "@" in without_code:
            violations.append(f"line {idx}: unverified '@' mention: {line.strip()}")
        for pat, desc in PII_PATTERNS:
            if pat.search(without_code):
                violations.append(f"line {idx}: PII leak ({desc}): {line.strip()}")
        for pat, desc in TIME_ESTIMATE_PATTERNS:
            if pat.search(without_code):
                violations.append(f"line {idx}: time estimate ({desc}): {line.strip()}")
        if METRIC_CLAIM_RE.search(without_code) and not PROVENANCE_TAG_RE.search(line):
            violations.append(f"line {idx}: unprovenanced metric claim without provenance tag: {line.strip()}")
    return violations


def check_pr_title(title: str) -> list[str]:
    """Find all unquoted @word mentions in a PR title."""
    without_code = CODE_SPAN_RE.sub("", title)
    return MENTION_RE.findall(without_code)


def run_tests() -> None:
    """Self-test verifying sanitisation, attributions, edge cases, and PR title checks."""
    print("Running release notes sanitiser test suite...")

    # Required specification fixture:
    # "update quickstart to @v0, thanks @someone by @orieg in https://…" -> only the attribution survives as a mention
    fixture = "update quickstart to @v0, thanks @someone by @orieg in https://github.com/orieg/discipline/pull/48"
    sanitized = sanitize_notes(fixture)
    expected = (
        "update quickstart to `@v0`, thanks `@someone` by @orieg in https://github.com/orieg/discipline/pull/48"
    )
    assert sanitized == expected, f"Fixture failure:\nExpected: {expected}\nGot:      {sanitized}"

    # Verify only the trailing attribution survives unquoted
    violations = verify_release_notes(sanitized)
    assert not violations, f"Fixture produced unverified mentions: {violations}"

    # Verify pre-existing code spans are not double-wrapped
    already_quoted = "update quickstart to `@v0`, thanks `@someone` by @orieg in https://github.com/orieg/discipline/pull/48"
    assert sanitize_notes(already_quoted) == already_quoted

    # Verify New Contributors lines are neutralised
    new_contrib = "* @someone made their first contribution in https://github.com/orieg/discipline/pull/1"
    sanitized_contrib = sanitize_notes(new_contrib)
    expected_contrib = "* `@someone` made their first contribution in https://github.com/orieg/discipline/pull/1"
    assert sanitized_contrib == expected_contrib
    assert not verify_release_notes(sanitized_contrib)

    # Negative control: an unquoted @mention fails verification
    raw_unquoted = "* update quickstart to @v0 by @orieg in https://github.com/orieg/discipline/pull/48"
    bad_violations = verify_release_notes(raw_unquoted)
    assert len(bad_violations) == 1, f"Expected 1 violation, got {bad_violations}"

    # PR title check positive and negative controls
    assert check_pr_title("docs: update quickstart to `@v0` and clean up") == []
    assert check_pr_title("fix(ci): contact support@example.com for assistance") == []
    bad_mentions = check_pr_title("docs: remove pre-release notices, update quickstart to @v0, and clean up")
    assert bad_mentions == ["v0"], f"Expected ['v0'], got {bad_mentions}"
    multi_mentions = check_pr_title("feat: update @foo and @bar")
    assert multi_mentions == ["foo", "bar"], f"Expected ['foo', 'bar'], got {multi_mentions}"

    # PII rejection tests
    home_leak = f"* feat: fix tool at {'/'}{'Users'}{'/'}{'alice'}{'/'}{'repo'} by @orieg in https://github.com/orieg/discipline/pull/50"
    v_home = verify_release_notes(sanitize_notes(home_leak))
    assert any("PII leak" in v for v in v_home), f"Expected PII leak violation, got: {v_home}"

    ip_leak = f"* fix: connect to {'192'}.{'168'}.1.25 by @orieg in https://github.com/orieg/discipline/pull/51"
    v_ip = verify_release_notes(sanitize_notes(ip_leak))
    assert any("PII leak" in v for v in v_ip), f"Expected LAN IP violation, got: {v_ip}"

    agent_cfg_leak = f"* docs: see {'~'}{'/.claude'}/CLAUDE.md by @orieg in https://github.com/orieg/discipline/pull/52"
    v_agent = verify_release_notes(sanitize_notes(agent_cfg_leak))
    assert any("PII leak" in v for v in v_agent), f"Expected agent config violation, got: {v_agent}"

    # Time estimates rejection tests
    time_leak = f"* feat: completed Phase 1 in {1}-{2} {'days'} by @orieg in https://github.com/orieg/discipline/pull/53"
    v_time = verify_release_notes(sanitize_notes(time_leak))
    assert any("time estimate" in v for v in v_time), f"Expected time estimate violation, got: {v_time}"

    sprint_leak = f"* feat: shipping {'next'} {'sprint'} by @orieg in https://github.com/orieg/discipline/pull/54"
    v_sprint = verify_release_notes(sanitize_notes(sprint_leak))
    assert any("time estimate" in v for v in v_sprint), f"Expected sprint projection violation, got: {v_sprint}"

    # Unprovenanced numbers rejection tests
    unprov_claim = f"* perf: achieve {3}x {'faster'} AST parsing by @orieg in https://github.com/orieg/discipline/pull/55"
    v_unprov = verify_release_notes(sanitize_notes(unprov_claim))
    assert any("unprovenanced metric" in v for v in v_unprov), f"Expected unprovenanced metric violation, got: {v_unprov}"

    # Provenanced numbers pass (positive control)
    prov_claim = f"* perf: achieve {3}x {'faster'} AST parsing (measured: linux-x86_64, abc1234) by @orieg in https://github.com/orieg/discipline/pull/55"
    v_prov = verify_release_notes(sanitize_notes(prov_claim))
    assert not v_prov, f"Expected provenanced claim to pass, got: {v_prov}"

    print("OK: All sanitiser and PR title tests passed.")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Neutralise arbitrary @-mentions in release notes and lint PR titles."
    )
    parser.add_argument(
        "--test",
        action="store_true",
        help="Run self-tests and exit.",
    )
    parser.add_argument(
        "--check-pr-title",
        metavar="TITLE",
        help="Check a PR title for unquoted @word mentions.",
    )
    parser.add_argument(
        "input",
        nargs="?",
        help="Path to input release notes markdown file (or '-' for stdin).",
    )
    parser.add_argument(
        "output",
        nargs="?",
        help="Path to output sanitized release notes file (default: stdout).",
    )

    args = parser.parse_args()

    if args.test:
        run_tests()
        sys.exit(0)

    if args.check_pr_title is not None:
        title = args.check_pr_title
        mentions = check_pr_title(title)
        if mentions:
            formatted = ", ".join(f"'@{m}'" for m in mentions)
            first = f"`@{mentions[0]}`"
            print(
                f"::error title=PR Title Mention Policy::PR title contains unquoted mention(s): {formatted}. "
                f"Wrap in backticks (e.g. '{first}') to prevent unintended user mentions in generated release notes.",
                file=sys.stderr,
            )
            print(
                f"ERROR: PR title contains unquoted GitHub mention(s): {formatted}.\n"
                f"PR titles are included verbatim in release notes and will notify arbitrary accounts.\n"
                f"Remediation: Enclose mentions in backticks in the PR title, e.g. '{first}'.",
                file=sys.stderr,
            )
            sys.exit(1)
        sys.exit(0)

    if not args.input:
        parser.print_help(sys.stderr)
        sys.exit(2)

    if args.input == "-":
        content = sys.stdin.read()
    else:
        in_path = Path(args.input)
        if not in_path.is_file():
            print(f"Error: input file '{args.input}' does not exist", file=sys.stderr)
            sys.exit(1)
        content = in_path.read_text(encoding="utf-8")

    sanitized = sanitize_notes(content)
    violations = verify_release_notes(sanitized)
    if violations:
        print(
            f"Error: sanitized release notes still contain {len(violations)} unverified '@' mention(s):",
            file=sys.stderr,
        )
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        sys.exit(1)

    if args.output:
        out_path = Path(args.output)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(sanitized, encoding="utf-8")
    else:
        sys.stdout.write(sanitized)


if __name__ == "__main__":
    main()
