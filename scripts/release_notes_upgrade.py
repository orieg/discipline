#!/usr/bin/env python3
"""Render the "Upgrading" section of a release's notes from docs/ROADMAP.md.

The compatibility ledger (default changes) and the behaviour-changes table in
docs/ROADMAP.md are the single source of truth for what a release changes for
consumers. Generated release notes list pull-request titles only, which is how
past loosenings shipped without a migration note. This script copies the rows
whose Release cell names the version into a Markdown section that release.yml
puts above the generated notes.

Usage:
  release_notes_upgrade.py --version v0.7.0 [--roadmap docs/ROADMAP.md]
  release_notes_upgrade.py --test
"""

import argparse
import re
import sys
from pathlib import Path


def table_rows(text: str, heading: str) -> list[list[str]]:
    """Data rows of the first Markdown table after `heading` (a line prefix)."""
    lines = text.splitlines()
    start = next((i for i, l in enumerate(lines) if l.startswith(heading)), None)
    if start is None:
        raise SystemExit(f"error: heading {heading!r} not found")
    rows: list[list[str]] = []
    in_table = False
    for line in lines[start + 1:]:
        if line.startswith("|"):
            in_table = True
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if all(re.fullmatch(r":?-+:?", c) for c in cells):
                continue
            rows.append(cells)
        elif in_table:
            break
    return rows[1:]  # drop the header row


def release_matches(cell: str, version: str) -> bool:
    return re.match(rf"{re.escape(version)}(?![0-9.])", cell) is not None


def render(text: str, version: str) -> str:
    defaults = [
        r for r in table_rows(text, "## Default Changes") if r and release_matches(r[0], version)
    ]
    behaviour = [
        r for r in table_rows(text, "### Behaviour Changes") if r and release_matches(r[0], version)
    ]
    if not defaults and not behaviour:
        return ""
    out = [f"## Upgrading to {version}", ""]
    if defaults:
        out += ["### Default changes", ""]
        for _, gate, old, new, direction, reason, restore in defaults:
            out.append(f"- {gate}: {old} → {new} ({direction}). {reason} Restore: {restore}.")
        out.append("")
    if behaviour:
        out += ["### Behaviour changes", ""]
        for _, area, change, direction, migration in behaviour:
            out.append(f"- **{area}** ({direction}): {change} Migration: {migration}")
        out.append("")
    out.append(
        "The full ledger is in [docs/ROADMAP.md](https://github.com/orieg/discipline/blob/main/docs/ROADMAP.md#default-changes-compatibility-ledger)."
    )
    return "\n".join(out) + "\n"


SAMPLE = """## Default Changes (Compatibility Ledger)

| Release | Gate | Old default | New default | Direction | Reason | Restore previous behaviour |
|---|---|---|---|---|---|---|
| v1.2.0 | `g` | on, `error` | on, `warning` | looser | Noisy. | `severity = "error"` |
| v1.1.0 | `h` | off | on | stricter | Useful. | `enabled = false` |

### Behaviour Changes

| Release | Area | Change | Direction | Migration |
|---|---|---|---|---|
| v1.2.0 | report | Counts changed. | reclassified | Read status. |
| v1.2.01 | other | Not this release. | looser | None. |
"""


def self_test() -> None:
    got = render(SAMPLE, "v1.2.0")
    assert "## Upgrading to v1.2.0" in got, got
    assert "`g`: on, `error` → on, `warning` (looser)" in got, got
    assert "**report** (reclassified): Counts changed." in got, got
    assert "`h`" not in got, "a row of another release leaked"
    assert "Not this release" not in got, "a prefix-matching version leaked"
    assert render(SAMPLE, "v9.9.9") == "", "no rows means no section"
    try:
        render("# nothing\n", "v1.2.0")
    except SystemExit:
        pass
    else:
        raise AssertionError("a missing ledger must be an error, not an empty section")
    print("release_notes_upgrade: self-test passed")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", help="release tag, e.g. v0.7.0")
    parser.add_argument("--roadmap", default="docs/ROADMAP.md")
    parser.add_argument("--test", action="store_true", help="run the self-test")
    args = parser.parse_args()
    if args.test:
        self_test()
        return
    if not args.version:
        parser.error("--version is required")
    sys.stdout.write(render(Path(args.roadmap).read_text(encoding="utf-8"), args.version))


if __name__ == "__main__":
    main()
