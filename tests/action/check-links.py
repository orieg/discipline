#!/usr/bin/env python3
"""Validate relative documentation links, anchors, and ensure zero obsolete references.

Checks:
  1. Zero occurrences of the obsolete specification token across the repository
     (excluding .git, target, and this validator script itself).
  2. All relative links and #anchors in markdown files resolve to existing files
     and valid heading anchors.
"""
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
FORBIDDEN_TOKEN = "P" + "RD"


def check_no_prd_references():
    """Ensure grep for obsolete token outside .git and target returns 0 hits."""
    print(f"Checking for obsolete '{FORBIDDEN_TOKEN}' references across repository...")
    violations = []
    ignore_dirs = {".git", "target", ".cargo", "node_modules"}
    this_file = Path(__file__).resolve()

    for dirpath, dirnames, filenames in os.walk(ROOT):
        # Prune ignored directories
        dirnames[:] = [d for d in dirnames if d not in ignore_dirs]
        for fname in filenames:
            # Skip binary artifacts, packages, archives or compiled objects per Rule 1.11
            if fname.endswith((
                ".tar.gz", ".tgz", ".zip", ".bin", ".pyc", ".png", ".ico",
                ".deb", ".rpm", ".gz", ".xz", ".bz2", ".woff", ".woff2",
                ".dylib", ".so", ".a", ".o"
            )):
                continue
            fpath = Path(dirpath) / fname
            if fpath.resolve() == this_file:
                continue
            try:
                with open(fpath, "r", encoding="utf-8", errors="ignore") as f:
                    for lno, line in enumerate(f, 1):
                        if FORBIDDEN_TOKEN in line:
                            violations.append(f"{fpath.relative_to(ROOT)}:{lno}: {line.strip()}")
            except Exception as e:
                print(f"Warning: could not read {fpath}: {e}", file=sys.stderr)

    if violations:
        print(f"FAILED: Found {len(violations)} reference(s) to '{FORBIDDEN_TOKEN}':", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        return False

    print(f"OK: Zero references to '{FORBIDDEN_TOKEN}' found.")
    return True


def _make_slug(s):
    slug = s.strip().lower()
    slug = re.sub(r'[\s/]+', '-', slug)
    slug = re.sub(r'[^a-z0-9\-]', '', slug)
    slug = re.sub(r'-+', '-', slug).strip('-')
    return slug


def _github_slug(s):
    """GitHub's own anchor rule: drop punctuation, keep repeated hyphens
    ("3. Forgejo & Gitea Actions" -> "3-forgejo--gitea-actions")."""
    slug = s.strip().lower()
    slug = re.sub(r'[^\w\- ]', '', slug)
    return slug.replace(' ', '-')


def slugify_heading(text):
    """Generate slug candidates for a markdown heading."""
    # Strip markdown links: [text](url) -> text
    text = re.sub(r'\[([^\]]+)\]\([^\)]+\)', r'\1', text)
    # Strip HTML tags
    text = re.sub(r'<[^>]+>', '', text)
    # Strip styling formatting
    for ch in ['`', '*', '_', '~']:
        text = text.replace(ch, '')

    slugs = set()
    gh = _github_slug(text)
    if gh:
        slugs.add(gh)
    s1 = _make_slug(text)
    if s1:
        slugs.add(s1)
        # Strip leading numbers like "1-", "81-", "82-"
        s1_nonum = re.sub(r'^[0-9]+(-[0-9]+)*-', '', s1)
        if s1_nonum:
            slugs.add(s1_nonum)

    # Also try without parenthesized text: "8.2 Release (file.yml)" -> "8.2 Release"
    text_noparen = re.sub(r'\(.*?\)', '', text).strip()
    if text_noparen != text:
        s2 = _make_slug(text_noparen)
        if s2:
            slugs.add(s2)
            s2_nonum = re.sub(r'^[0-9]+(-[0-9]+)*-', '', s2)
            if s2_nonum:
                slugs.add(s2_nonum)

    return slugs


def extract_headings(file_path):
    """Extract all heading anchors and HTML ID anchors from a markdown file."""
    anchors = set()
    try:
        with open(file_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                m = re.match(r'^(#{1,6})\s+(.+)$', line)
                if m:
                    heading_text = m.group(2)
                    anchors.update(slugify_heading(heading_text))
                # Match <a id="..." or <a name="..."
                for am in re.finditer(r'<a\s+[^>]*(?:id|name)=["\']([^"\']+)["\']', line):
                    anchors.add(am.group(1))
    except Exception as e:
        print(f"Warning: could not extract headings from {file_path}: {e}", file=sys.stderr)
    return anchors


def check_markdown_links():
    """Verify all relative links and anchors in tracked markdown files."""
    print("Checking markdown relative links and anchors...")
    md_files = [ROOT / "README.md", ROOT / "AGENTS.md"]
    docs_dir = ROOT / "docs"
    if docs_dir.exists():
        md_files.extend(docs_dir.rglob("*.md"))

    link_pattern = re.compile(r'(?<!\!)\[([^\]]+)\]\(([^)]+)\)')

    headings_cache = {}
    violations = []

    for md_file in md_files:
        if not md_file.exists():
            continue
        rel_path = md_file.relative_to(ROOT)
        with open(md_file, "r", encoding="utf-8") as f:
            content = f.read()

        for lno, line in enumerate(content.splitlines(), 1):
            for match in link_pattern.finditer(line):
                target = match.group(2).strip()

                # Ignore external, mailto, or template schemes
                if target.startswith(("http://", "https://", "mailto:", "#")):
                    if target.startswith("#"):
                        # Local document anchor
                        anchor = target[1:]
                        if md_file not in headings_cache:
                            headings_cache[md_file] = extract_headings(md_file)
                        if anchor not in headings_cache[md_file]:
                            violations.append(
                                f"{rel_path}:{lno}: broken anchor '{target}' (not found in {rel_path})"
                            )
                    continue

                # Relative link with optional anchor: "path" or "path#anchor"
                if "#" in target:
                    file_part, anchor_part = target.split("#", 1)
                else:
                    file_part, anchor_part = target, None

                # Resolve file_part relative to md_file directory
                target_file = (md_file.parent / file_part).resolve()

                if not target_file.exists():
                    violations.append(
                        f"{rel_path}:{lno}: broken link '{target}' -> '{target_file}' does not exist"
                    )
                    continue

                if anchor_part and target_file.suffix == ".md":
                    if target_file not in headings_cache:
                        headings_cache[target_file] = extract_headings(target_file)
                    if anchor_part not in headings_cache[target_file]:
                        violations.append(
                            f"{rel_path}:{lno}: broken anchor '{anchor_part}' in target '{file_part}'"
                        )

    if violations:
        print(f"FAILED: Found {len(violations)} broken link(s):", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        return False

    print(f"OK: All markdown links and anchors resolved across {len(md_files)} files.")
    return True


def check_container_tags():
    """Verify that all ghcr.io/orieg/discipline container tags referenced in docs are valid."""
    print("Checking container image tags in documentation...")
    cargo_path = ROOT / "Cargo.toml"
    current_version = None
    with open(cargo_path, "r", encoding="utf-8") as f:
        for line in f:
            m = re.match(r'^version\s*=\s*"([^"]+)"', line.strip())
            if m:
                current_version = m.group(1)
                break

    if not current_version:
        print("FAILED: Could not determine current version from Cargo.toml", file=sys.stderr)
        return False

    major = current_version.split(".")[0]
    allowed_tags = {"latest", f"v{major}", f"v{current_version}", "test"}

    check_files = [ROOT / "README.md", ROOT / "AGENTS.md"]
    docs_dir = ROOT / "docs"
    if docs_dir.exists():
        check_files.extend(docs_dir.glob("*.md"))
        check_files.extend(docs_dir.glob("*.html"))
    templates_dir = ROOT / "templates"
    if templates_dir.exists():
        check_files.extend(templates_dir.glob("*"))

    tag_pattern = re.compile(r'ghcr\.io/orieg/discipline:([a-zA-Z0-9_\.-]+)')
    violations = []

    for fpath in check_files:
        if not fpath.is_file():
            continue
        rel_path = fpath.relative_to(ROOT)
        with open(fpath, "r", encoding="utf-8", errors="ignore") as f:
            for lno, line in enumerate(f, 1):
                for match in tag_pattern.finditer(line):
                    tag = match.group(1).rstrip('`"\'.,;:)<>')
                    tag = re.sub(r'[<>/].*$', '', tag)
                    if tag not in allowed_tags and not tag.startswith("${{"):
                        violations.append(
                            f"{rel_path}:{lno}: invalid or obsolete container tag '{tag}' (allowed: {sorted(allowed_tags)})"
                        )

    if violations:
        print(f"FAILED: Found {len(violations)} invalid container tag(s):", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        return False

    print(f"OK: All container image tags valid across documentation (allowed: {sorted(allowed_tags)}).")
    return True


def check_ci_recipes():
    """Reject two workflow-recipe defects that make a gate never run or never fail.

    `runs-on: docker://image` matches no runner label on act_runner or
    forgejo-runner (the job queues forever); a `git clone` inside a job's
    steps lands on the default branch, so a pull-request gate compares the
    base with itself and passes every change. The correct recipe is the one
    executed by tests/action/test-container-recipe.sh.
    """
    print("Checking CI recipes in documentation and templates...")
    check_files = [ROOT / "README.md", ROOT / "AGENTS.md"]
    for sub in ("docs", "templates"):
        d = ROOT / sub
        if d.exists():
            check_files.extend(p for p in d.rglob("*") if p.is_file()
                               and p.suffix in {".md", ".html", ".yml", ".yaml"})
    runs_on_image = re.compile(r"runs-on:\s*docker://")
    steps_key = re.compile(r"^\s*steps:\s*$")
    fence = re.compile(r"^\s*```")
    violations = []
    for fpath in sorted(check_files):
        if not fpath.is_file():
            continue
        rel_path = fpath.relative_to(ROOT)
        in_workflow_block = False
        with open(fpath, "r", encoding="utf-8", errors="ignore") as f:
            for lno, line in enumerate(f, 1):
                if runs_on_image.search(line):
                    violations.append(
                        f"{rel_path}:{lno}: `runs-on:` names an image; it must name a runner label "
                        f"(put the image under `container: image:`)"
                    )
                if fpath.suffix in {".yml", ".yaml"}:
                    in_workflow_block = in_workflow_block or bool(steps_key.match(line))
                elif fence.match(line):
                    in_workflow_block = False
                elif steps_key.match(line):
                    in_workflow_block = True
                if in_workflow_block and "git clone" in line:
                    violations.append(
                        f"{rel_path}:{lno}: `git clone` in a workflow job lands on the default branch; "
                        f"fetch the pull request head (see the container recipe in docs/CONFIGURATION.md)"
                    )
    if violations:
        print(f"FAILED: Found {len(violations)} CI recipe defect(s):", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        return False
    print("OK: No CI recipe names an image as a runner label or clones the default branch in a job.")
    return True


def main():
    prd_ok = check_no_prd_references()
    links_ok = check_markdown_links()
    tags_ok = check_container_tags()
    recipes_ok = check_ci_recipes()

    if not (prd_ok and links_ok and tags_ok and recipes_ok):
        sys.exit(1)
    print("All link, reference, and tag checks passed successfully.")
    sys.exit(0)


if __name__ == "__main__":
    main()
