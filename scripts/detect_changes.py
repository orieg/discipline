#!/usr/bin/env python3
"""Evaluate changed files against workflow path rules for conditional CI jobs.

Determines which verification suites need to run on a pull request, ensuring
that documentation-only or license-only changes skip heavy compilation, matrix
tests, and fuzzing while preserving complete invariant enforcement.

Usage:
  scripts/detect_changes.py [--event <name>] [--base <ref>] [--files <path1,path2>]
  scripts/detect_changes.py --test
"""

import os
import re
import subprocess
import sys
from typing import Dict, List, Pattern

RULES: Dict[str, List[str]] = {
    "test": [
        "src/**",
        "tests/**",
        "benches/**",
        "Cargo.toml",
        "Cargo.lock",
        "deny.toml",
        "build.rs",
        "discipline.toml",
        ".github/workflows/ci.yml",
        # Files outside the source tree that a Rust test reads. Each is listed
        # because a pull request changing it alone can make `cargo test` fail;
        # a file no test reads stays out, so the rule remains a filter.
        # Compared with each other by a parity test:
        "install.sh",
        "docs/install.sh",
        ".forgejo/workflows/action-selftest.yml",
        ".gitea/workflows/action-selftest.yml",
        # Parsed by tests:
        "action.yml",
        ".github/workflows/release.yml",
        "renovate/discipline.json",
        "CITATION.cff",
        # Generated or checked by `discipline docs --check`, which a test runs
        # over the working tree; several are also read by a test of their own:
        "README.md",
        "docs/index.html",
        "docs/GATES.md",
        "docs/CONFIGURATION.md",
        "docs/ROADMAP.md",
        "man/**",
        "completions/**",
        "discipline.schema.json",
        "discipline.*.schema.json",
    ],
    "msrv": [
        "src/**",
        "tests/**",
        "benches/**",
        "Cargo.toml",
        "Cargo.lock",
        ".github/workflows/ci.yml",
    ],
    "supply-chain": [
        "Cargo.toml",
        "Cargo.lock",
        "deny.toml",
        ".github/workflows/ci.yml",
    ],
    "action-github": [
        "src/**",
        "action.yml",
        "tests/action/**",
        "Cargo.toml",
        "Cargo.lock",
        ".github/workflows/ci.yml",
    ],
    "action-gitea": [
        "src/**",
        "action.yml",
        ".gitea/**",
        ".forgejo/**",
        "Dockerfile*",
        "Cargo.toml",
        "Cargo.lock",
        ".github/workflows/ci.yml",
    ],
    "docker-smoke": [
        "src/**",
        "Dockerfile*",
        "Cargo.toml",
        "Cargo.lock",
        ".github/workflows/ci.yml",
    ],
    "pre-commit": [
        "src/**",
        ".pre-commit-hooks.yaml",
        "Cargo.toml",
        "Cargo.lock",
        ".github/workflows/ci.yml",
    ],
}


def glob_to_regex(pattern: str) -> Pattern[str]:
    """Compile a git-style path pattern into a regex.

    Supports:
      - '**' matching zero or more path segments
      - '*' matching characters within a path segment (excluding '/')
      - '?' matching a single character within a path segment
      - literal characters
    """
    regex_parts = []
    i = 0
    n = len(pattern)
    while i < n:
        if pattern[i:i + 2] == "**":
            if i + 2 < n and pattern[i + 2] == "/":
                regex_parts.append("(?:.*/)?")
                i += 3
            elif i > 0 and pattern[i - 1] == "/":
                regex_parts.append(".*")
                i += 2
            else:
                regex_parts.append(".*")
                i += 2
        elif pattern[i] == "*":
            regex_parts.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            regex_parts.append("[^/]")
            i += 1
        else:
            regex_parts.append(re.escape(pattern[i]))
            i += 1
    return re.compile("^" + "".join(regex_parts) + "$")


_COMPILED_RULES: Dict[str, List[Pattern[str]]] = {
    key: [glob_to_regex(p) for p in patterns] for key, patterns in RULES.items()
}


def evaluate_files(changed_files: List[str], event_name: str) -> Dict[str, bool]:
    """Evaluate whether changed files trigger each verification job.

    On non-pull_request events (e.g. pushes to main), all jobs run unconditionally.
    """
    if event_name != "pull_request":
        return {key: True for key in RULES}

    out: Dict[str, bool] = {}
    for key, compiled_patterns in _COMPILED_RULES.items():
        matched = any(
            any(pat.match(f) for pat in compiled_patterns) for f in changed_files
        )
        out[key] = matched
    return out


def get_changed_files_from_git(base_ref: str) -> List[str]:
    """Read changed file paths from git diff against base_ref."""
    candidates = [f"origin/{base_ref}", base_ref]
    target_ref = None
    for ref in candidates:
        r = subprocess.run(
            ["git", "rev-parse", "--verify", ref], capture_output=True, text=True
        )
        if r.returncode == 0:
            target_ref = ref
            break

    if not target_ref:
        # Attempt fetch if reference not present in shallow / narrow clones
        subprocess.run(["git", "fetch", "origin", base_ref], capture_output=True)
        for ref in candidates:
            r = subprocess.run(
                ["git", "rev-parse", "--verify", ref], capture_output=True, text=True
            )
            if r.returncode == 0:
                target_ref = ref
                break

    if not target_ref:
        raise RuntimeError(f"could not resolve git base ref '{base_ref}'")

    cmd = ["git", "diff", "--name-only", f"{target_ref}...HEAD"]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=True)
    return [line.strip() for line in proc.stdout.splitlines() if line.strip()]


def run_tests() -> None:
    """Execute unit tests for change detection logic."""
    # 1. Pure docs change: files no Rust test reads
    docs_only = [
        "LICENSE",
        "SECURITY.md",
        "CONTRIBUTING.md",
        "docs/ARCHITECTURE.md",
        "docs/guides/ci-platforms.md",
    ]
    res = evaluate_files(docs_only, "pull_request")
    assert all(not v for v in res.values()), f"Expected all False for docs, got {res}"

    # 2. Rust source change
    code_only = ["src/guards/ci_skip_set.rs"]
    res = evaluate_files(code_only, "pull_request")
    assert res["test"] is True
    assert res["msrv"] is True
    assert res["action-github"] is True
    assert res["action-gitea"] is True
    assert res["docker-smoke"] is True
    assert res["pre-commit"] is True
    assert res["supply-chain"] is False

    # 3. Cargo.lock change
    cargo_lock = ["Cargo.lock"]
    res = evaluate_files(cargo_lock, "pull_request")
    assert all(v for v in res.values()), f"Expected all True for Cargo.lock, got {res}"

    # 4. deny.toml change
    deny = ["deny.toml"]
    res = evaluate_files(deny, "pull_request")
    assert res["test"] is True
    assert res["supply-chain"] is True
    assert res["msrv"] is False

    # 5. action.yml change
    action = ["action.yml"]
    res = evaluate_files(action, "pull_request")
    assert res["action-github"] is True
    assert res["action-gitea"] is True
    assert res["test"] is True
    assert res["docker-smoke"] is False

    # 6. Dockerfile change
    docker = ["Dockerfile.release"]
    res = evaluate_files(docker, "pull_request")
    assert res["docker-smoke"] is True
    assert res["action-gitea"] is True
    assert res["test"] is False

    # 7. .pre-commit-hooks.yaml change
    pre = [".pre-commit-hooks.yaml"]
    res = evaluate_files(pre, "pull_request")
    assert res["pre-commit"] is True
    assert res["test"] is False

    # 8. ci.yml workflow change
    ci_wf = [".github/workflows/ci.yml"]
    res = evaluate_files(ci_wf, "pull_request")
    assert all(v for v in res.values()), f"Expected all True for ci.yml, got {res}"

    # 8b. Forgejo workflow change runs the same suite as the Gitea one
    forgejo = [".forgejo/workflows/action-selftest.yml"]
    res = evaluate_files(forgejo, "pull_request")
    assert res["action-gitea"] is True
    gitea = [".gitea/workflows/action-selftest.yml"]
    assert evaluate_files(gitea, "pull_request") == res

    # 8c. A file a Rust test reads runs `cargo test` when it is the only change:
    # the test is the drift check, so skipping it lets the drift land (issue 523).
    read_by_a_rust_test = [
        # tests/test_docs.rs: served_installer_is_a_byte_copy_of_the_tested_installer
        "install.sh",
        "docs/install.sh",
        # tests/test_action_version.rs:
        # forgejo_and_gitea_selftest_workflows_run_the_same_jobs
        ".forgejo/workflows/action-selftest.yml",
        ".gitea/workflows/action-selftest.yml",
        # tests/test_release_workflow.rs
        ".github/workflows/release.yml",
        # tests/test_docs.rs, tests/test_action_version.rs,
        # tests/test_stability_contract.rs, tests/test_threat_model_claims.rs
        "action.yml",
        # tests/test_docs.rs: docs_check_passes_over_the_working_tree
        "README.md",
        "docs/index.html",
        "docs/ROADMAP.md",
        "man/man1/discipline.1",
        "man/man5/discipline.toml.5",
        # the same, and tests/test_adoption.rs (include_str!)
        "docs/GATES.md",
        # the same, and tests/test_config.rs, tests/test_editor_integrations.rs,
        # tests/test_release_recipe.rs, tests/test_stability_contract.rs
        "docs/CONFIGURATION.md",
        # tests/test_docs.rs: committed_completion_scripts_match_the_binary
        "completions/_discipline",
        "completions/discipline.bash",
        "completions/discipline.fish",
        # tests/test_config.rs, tests/test_output_schemas.rs
        "discipline.schema.json",
        "discipline.report.schema.json",
        "discipline.replay.schema.json",
        "discipline.audit.schema.json",
        # tests/test_editor_integrations.rs
        "renovate/discipline.json",
        "CITATION.cff",
    ]
    skipped = [
        f
        for f in read_by_a_rust_test
        if not evaluate_files([f], "pull_request")["test"]
    ]
    assert not skipped, f"`test` is skipped for files a Rust test reads: {skipped}"

    # 8d. Control: the rule stays a filter. A file no Rust test reads, alone,
    # still skips `cargo test`, including a neighbour of each listed file.
    for unread in [
        "SECURITY.md",
        "LICENSE-MIT",
        ".zenodo.json",
        "docs/ARCHITECTURE.md",
        "docs/tutorials/getting-started.md",
        "templates/discipline.gitlab-ci.yml",
        "scripts/tag-release.sh",
        ".github/workflows/pages.yml",
        ".gitea/workflows/other.yml",
        "renovate/other.json",
        "packaging/docker/docker-entrypoint.sh",
    ]:
        assert (
            evaluate_files([unread], "pull_request")["test"] is False
        ), f"`test` runs for {unread}, which no Rust test reads"

    # 9. Push event on main (must run all suites unconditionally)
    res_push = evaluate_files(docs_only, "push")
    assert all(
        v for v in res_push.values()
    ), f"Expected all True on push event, got {res_push}"

    print("detect_changes.py: all unit tests passed successfully.")


def main() -> None:
    if "--test" in sys.argv:
        run_tests()
        return

    event_name = os.environ.get("EVENT_NAME", "pull_request")
    base_ref = os.environ.get("BASE_REF", "main")

    # Command line overrides for local debugging
    for i, arg in enumerate(sys.argv):
        if arg == "--event" and i + 1 < len(sys.argv):
            event_name = sys.argv[i + 1]
        elif arg == "--base" and i + 1 < len(sys.argv):
            base_ref = sys.argv[i + 1]

    if event_name == "pull_request":
        changed_files = get_changed_files_from_git(base_ref)
        print(f"detect-changes: pull_request against '{base_ref}', {len(changed_files)} changed files:")
        for f in changed_files[:20]:
            print(f"  {f}")
        if len(changed_files) > 20:
            print(f"  ... and {len(changed_files) - 20} more")
    else:
        changed_files = []
        print(f"detect-changes: event '{event_name}' (unconditional run of all suites)")

    decisions = evaluate_files(changed_files, event_name)

    print("Job execution decisions:")
    for job, should_run in decisions.items():
        state = "RUN" if should_run else "SKIP"
        print(f"  {job:<15}: {state} ({str(should_run).lower()})")

    # Write to GitHub Actions step outputs if running in Actions environment
    github_output = os.environ.get("GITHUB_OUTPUT")
    if github_output:
        with open(github_output, "a", encoding="utf-8") as f:
            for job, should_run in decisions.items():
                f.write(f"{job}={str(should_run).lower()}\n")


if __name__ == "__main__":
    main()
