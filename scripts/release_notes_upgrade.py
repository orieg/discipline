#!/usr/bin/env python3
"""Render the "Upgrading" section of a release's notes from docs/ROADMAP.md.

The compatibility ledger (default changes) and the behaviour-changes table in
docs/ROADMAP.md are the single source of truth for what a release changes for
consumers. Generated release notes list pull-request titles only, which is how
past loosenings shipped without a migration note. This script copies the rows
whose Release cell names the version, or an earlier patch of the same minor,
into a Markdown section that release.yml puts above the generated notes. A
patch release is therefore cumulative for its minor: v0.17.2 lists the v0.17.0
and v0.17.1 rows too, because an earlier patch may never have reached a
registry, and then the patch is the first release of that minor a consumer
upgrades to.

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
            # A `\|` inside a cell is a literal pipe (Markdown's escape), not a separator.
            cells = [
                c.strip().replace("\\|", "|")
                for c in re.split(r"(?<!\\)\|", line.strip().strip("|"))
            ]
            if all(re.fullmatch(r":?-+:?", c) for c in cells):
                continue
            rows.append(cells)
        elif in_table:
            break
    return rows[1:]  # drop the header row


def parse_version(text: str) -> tuple[int, int, int] | None:
    """`(major, minor, patch)` of a `vX.Y.Z` at the start of `text`, else None."""
    m = re.match(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?![0-9.])", text)
    return (int(m[1]), int(m[2]), int(m[3])) if m else None


def release_matches(cell: str, version: str) -> bool:
    """Whether a row labelled `cell` belongs in the notes of `version`: the same
    major and minor, and a patch no later than the release's."""
    row, release = parse_version(cell), parse_version(version)
    if row is None or release is None:
        return re.match(rf"{re.escape(version)}(?![0-9.])", cell) is not None
    return row[:2] == release[:2] and row[2] <= release[2]


def earlier_patches(rows: list[list[str]], version: str) -> list[str]:
    """Labels of the earlier patches of the same minor that `rows` carry."""
    release = parse_version(version)
    labels = {r[0].split()[0] for r in rows if parse_version(r[0]) not in (None, release)}
    return sorted(labels, key=lambda label: parse_version(label) or (0, 0, 0))


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
    earlier = earlier_patches(defaults + behaviour, version)
    if earlier:
        out += [
            f"This section is cumulative for the minor: it includes the changes of {', '.join(earlier)}.",
            "",
        ]
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
| v1.2.0 | CLI | New `x --agent <a\\|b>`. | additive | None. |
| v1.2.01 | other | Not this release. | looser | None. |
| v1.3.0 | later | A later minor. | stricter | None. |
| v1.2.2 | later-patch | A later patch. | stricter | None. |
| v1.2.1 | patch | A patch change. | stricter | Do x. |
"""


def self_test() -> None:
    got = render(SAMPLE, "v1.2.0")
    assert "## Upgrading to v1.2.0" in got, got
    assert "`g`: on, `error` → on, `warning` (looser)" in got, got
    assert "**report** (reclassified): Counts changed." in got, got
    assert "**CLI** (additive): New `x --agent <a|b>`." in got, "an escaped pipe splits a cell"

    assert "`h`" not in got, "a row of another release leaked"
    assert "Not this release" not in got, "a prefix-matching version leaked"
    assert "A patch change" not in got, "a later patch leaked into an earlier release"
    assert "cumulative" not in got, "the first release of a minor is not cumulative"

    patch = render(SAMPLE, "v1.2.1")
    assert "## Upgrading to v1.2.1" in patch, patch
    assert "**patch** (stricter): A patch change. Migration: Do x." in patch, patch
    assert "**report** (reclassified): Counts changed." in patch, "an earlier patch of the minor is included"
    assert "`g`: on, `error` → on, `warning` (looser)" in patch, "an earlier patch's default change is included"
    assert "cumulative for the minor: it includes the changes of v1.2.0." in patch, patch
    assert "A later patch" not in patch, "a later patch leaked"
    assert "A later minor" not in patch and "`h`" not in patch, "another minor leaked"
    assert "Not this release" not in patch, "a prefix-matching version leaked"
    assert render(SAMPLE, "v1.2.2").count("- **") == 4, "v1.2.2 lists v1.2.0, v1.2.1 and its own rows"

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
