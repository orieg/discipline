---
layout: default
title: Configuration & Integration Reference
permalink: /configuration/
---

# Configuration & Integration Reference

This document provides the complete specification of `discipline`'s configuration layers, schema, GitHub/Gitea/Forgejo Action inputs and outputs, CLI commands, override directives, trust model, and reference adoption configurations.

**Superseded when:** Schema version increases, new configuration layers or action parameters are introduced, or directive syntax changes. Update in place; do not fork.

---

## Configuration Layers & Precedence

Discipline employs a 5-layer configuration hierarchy. With zero configuration, every available gate executes at severity `error`. Every layer merges deterministically; nothing turns off silently.

| Layer | Source | Precedence | Description |
|---|---|---|---|
| **1. Built-in defaults** | Compiled binary | Lowest | Every available gate enabled at severity `"error"`. |
| **2. Repository configuration** | `discipline.toml` | ↑ | Durable, peer-reviewed repository policy. |
| **3. Inline TOML override** | `--config-override`, `DISCIPLINE_CONFIG_OVERRIDE`, action input `config_override` | ↑ | Per-workflow tuning without modifying files. |
| **4. Gate switches** | `--enable` / `--disable`, `DISCIPLINE_ENABLE` / `DISCIPLINE_DISABLE`, action inputs `enable` / `disable` | ↑ | Command-line switches (comma- or newline-separated). |
| **5. Secret denylist** | `DISCIPLINE_HOSTNAME_DENYLIST`, action input `hostname_denylist` | Highest | Sensitive hostnames that must not appear even in repository config. |

### Merge Rules & Asymmetric List Resets

Configuration layers are merged as TOML values under strict typing (F6):
- **Tables** merge recursively key by key.
- **Scalars** replace previous values.
- **Tightening lists** (`hostname_denylist`, `extra_patterns`, `paths`, `include`): Strictly append-only. Higher layers can only add restrictions; they cannot loosen them. Any reset directive on tightening lists is ignored.
- **Loosening lists** (`exempt_paths`, `allow_patterns`, `allowed_users`, `assert_helper_fns`, `extra_assert_macros`): Default to appending across layers. To clear lower-precedence exemptions and enforce stricter rules, use explicit reset syntax:
  ```toml
  # Table reset syntax
  exempt_paths = { reset = true, items = ["tests/legacy/**"] }

  # Or sentinel list reset syntax
  exempt_paths = ["__reset__", "tests/legacy/**"]
  ```

---

## Configuration Schema

Discipline validates `discipline.toml` against JSON Schema (draft 2020-12) with zero tolerance for unknown keys or malformed entries.

<!-- generated:config-schema -->
| Section / Key | Type | Default | Description |
|---|---|---|---|
| `meta.version` | integer | `1` | Configuration schema version (must be 1) |
| `meta.name` | string | `""` | Repository or project name |
| `meta.description` | string | `""` | Optional description of the project |
| `directives.sources` | list | `["pr-body", "commits"]` | Allowed directive source channels |
| `directives.allow_hidden` | boolean | `false` | Allow directives inside HTML comments `<!-- -->` |
| `directives.fail_on_overrides` | boolean | `false` | Treat applied overrides as failures requiring human sign-off |
| `gates.<id>.enabled` | boolean | `true` | Whether this gate is active |
| `gates.<id>.severity` | string | `"error"` | Violation severity: `"error"` (blocking) or `"warning"` (non-blocking) |
| `gates.<id>.exempt_paths` | list | `[]` | File path globs exempted from gate evaluation |
| `gates.assertion-reduction.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions |
| `gates.assertion-reduction.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |
| `gates.vacuous-tests.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions |
| `gates.vacuous-tests.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |
| `gates.unsafe-safety-comment.placeholders` | list | `[]` | Additional placeholder phrases to reject in SAFETY comments |
| `gates.deletion-rationale.paths` | list | `["**"]` | Path globs where file deletions require a rationale |
| `gates.time-estimates.include` | list | `["**/*.md"]` | Markdown file globs swept for duration estimates |
| `gates.time-estimates.extra_patterns` | list | `[]` | Additional custom banned regex patterns |
| `gates.time-estimates.allow_patterns` | list | `[...]` | Regex patterns permitted as operational exceptions |
| `gates.time-estimates.scan_pr_body` | boolean | `true` | Whether to scan the PR description text |
| `gates.pii.home_paths` | boolean | `true` | Check for leaked workstation home directory paths |
| `gates.pii.lan_ips` | boolean | `true` | Check for leaked private RFC 1918 LAN IP addresses |
| `gates.pii.allowed_users` | list | `[...]` | Allowed username tokens in paths |
| `gates.pii.hostname_denylist` | list | `[]` | Whole-token case-insensitive hostnames to reject |
| `gates.pii.extra_patterns` | list | `[]` | Additional regex patterns to reject |
| `gates.pii.allow_patterns` | list | `[]` | Custom regex patterns exempted from rejection |
| `gates.pii.scan_pr_body` | boolean | `true` | Whether to scan the PR description text |
| `gates.agent-scratch.paths` | list | `[...]` | Directory and file globs forbidden from being tracked |
| `gates.golden-output.paths` | list | `[...]` | Committed golden/snapshot globs requiring override to edit |
| `gates.bench-regression.tolerance_pct` | number | `0.5` | Maximum allowed benchmark regression percentage |
| `gates.bench-regression.paths` | list | `[...]` | Benchmark artifact globs tracked across revisions |
| `gates.bench-regression.base_file` | string | `""` | Baseline benchmark output file for dual-file regression checks |
| `gates.bench-regression.head_file` | string | `""` | Current benchmark output file for dual-file regression checks |
| `gates.bench-regression.noise_floor_pct` | number | `0.5` | Multi-arm noise floor threshold percentage |
| `gates.bench-regression.advisory_pct` | number | `0.1` | Advisory threshold percentage for reporting minor regressions |
| `gates.bench-regression.exempt_arms` | list | `[]` | Benchmark arm names exempted from regression checks |
| `gates.bench-regression.require_sourced_override` | boolean | `false` | Require regression overrides to cite CI run URL or committed artifact |
| `gates.bench-regression.provenance` | string | `""` | Expected host or runner provenance tag for benchmark artifacts |
| `gates.bench-regression.allow_cross_host` | boolean | `false` | Allow benchmark comparison across mismatched provenance tags |
| `gates.provenance-tags.include` | list | `["**/*.md"]` | Markdown file globs swept for provenance and hygiene |
| `gates.provenance-tags.check_tables` | boolean | `true` | Verify table unit-bearing numerics carry provenance tags |
| `gates.provenance-tags.check_mechanisms` | boolean | `true` | Verify mechanism claims cite hardware counters or hypothesis qualifiers |
| `gates.provenance-tags.check_intervals` | boolean | `true` | Verify wall-clock ratios cite confidence intervals or provisional markers |
| `gates.provenance-tags.check_paired_figures` | boolean | `true` | Verify paired figures cite shared workload or differentiation markers |
| `gates.provenance-tags.scan_pr_body` | boolean | `true` | Whether to scan the PR description text |
| `gates.shell-secrets.extra_secret_patterns` | list | `[]` | Additional custom regex patterns for sensitive secret variable names |
| `gates.shell-secrets.allow_patterns` | list | `[]` | Custom regex patterns exempted from violation |
| `gates.issue-link.pattern` | string | `""` | Custom regex pattern required in PR title or body |
| `gates.issue-link.require_in_commit_if_no_pr` | boolean | `false` | Require issue link in commit messages when no PR metadata is supplied |
<!-- /generated -->

---

## Action Reference

The composite action (`action.yml`) runs identically in GitHub Actions, Gitea Actions, and Forgejo Actions. It operates with zero Node.js runtime overhead, executing entirely via shell and the static binary.

### Action Inputs

<!-- generated:action-inputs -->
| Input | Default | Description |
|---|---|---|
| `config` | `discipline.toml` | Path to discipline.toml. When the file is absent, built-in defaults apply (every available gate on). |
| `suite` | `all` | Suite to run: all, agent-guard, hygiene, integrity |
| `base_ref` | *(none)* | Branch or commit the change is measured against. Default: PR base branch, else the pushed-from commit, else the default branch. |
| `enable` | *(none)* | Gate ids to force on (comma or newline separated). See `discipline gates`. |
| `disable` | *(none)* | Gate ids to force off (comma or newline separated), e.g. "time-estimates, pii". |
| `config_override` | *(none)* | Inline TOML merged over discipline.toml: tables merge, lists append, scalars replace. |
| `hostname_denylist` | *(none)* | Hostnames the pii gate must reject (comma or newline separated). Pass a secret; matches are never echoed. |
| `fail_on_warnings` | `false` | Treat warnings as failures. |
| `fail_on_overrides` | `false` | Treat applied overrides as failures (requires human sign-off). |
| `directive_sources` | *(none)* | Comma-separated list of allowed directive sources (pr-body, commits). |
| `pr_body` | `${{ github.event.pull_request.body }}` | PR description: carries override directives and is itself scanned by hygiene gates. |
| `pr_title` | `${{ github.event.pull_request.title }}` | PR title: checked by hygiene gates (e.g. issue-link). |
| `working_directory` | `.` | Directory of the repository to check. |
| `version` | *(none)* | Release to download (e.g. v0.1.0). Default: the tag this action was referenced by, else the latest release. |
| `binary_path` | *(none)* | Use this discipline binary instead of downloading one (air-gapped Gitea/Forgejo runners, self-tests). |
| `download_url` | `https://github.com/orieg/discipline/releases` | Base URL of the release store, for mirrors. |
<!-- /generated -->

### Action Outputs

<!-- generated:action-outputs -->
| Output | Description |
|---|---|
| `status` | pass, fail, or error |
| `errors` | Number of blocking violations |
| `warnings` | Number of non-blocking violations |
| `failed_gates` | Comma separated ids of the gates that reported a violation |
| `overrides` | Number of applied override directives |
| `overridden_gates` | Comma separated ids of the gates that had an override applied |
| `passed_gates` | Number of passing gates |
| `examined_items` | Total number of items examined across all enabled gates |
| `report` | Path of the JSON report |
| `install_error` | Why installing the binary failed (empty on success) |
<!-- /generated -->

---

## CLI Reference

Discipline provides a standalone CLI for local developer workflows, pre-commit hooks, and CI scripting:

<!-- generated:cli -->
| Subcommand | Description |
|---|---|
| `check` | Run the configured gates. Exit 0 = pass, 1 = violations, 2 = could not check |
| `diff` | Shorthand for `check --suite agent-guard` against `HEAD~1` |
| `init` | Write a discipline.toml with every available gate at its default |
| `gates` | List every gate: id, suite, availability, and effective state |
| `schema` | Print the JSON Schema for discipline.toml |
| `self-test` | Run the embedded negative / positive controls against this binary |
| `install-hooks` | Install pre-commit hook in the local git repository |
<!-- /generated -->

### Exit Codes

| Code | Status | Meaning |
|---|---|---|
| `0` | **Pass** | Every enabled gate ran and detected no blocking violations. |
| `1` | **Violations** | Gate violations found (blocking errors, or warnings under `--fail-on-warnings`). |
| `2` | **Could not check** | Engine failed to check: missing repository, unresolvable base ref, shallow clone with unreachable merge base, unreadable configuration, or syntax errors. |

---

## Override Directives

Legitimate test refactorings, file deletions, or configuration adjustments are authorized through scoped directives in the PR description or commit messages. Directives never apply globally: they must name the exact subject they cover.

### Syntax & Grammar

```text
<directive>: <subject> <reason>
```

Alternatively, namespaced or HTML-comment syntax is accepted:
```text
discipline: <directive>: <subject> <reason>
<!-- discipline:allow(<gate-id>): <subject> <reason> -->
```

Directives must begin on their own line. Mentions mid-sentence, inside markdown tables, or within code blocks never arm the directive (F8). The reason must be substantive and non-empty; placeholders (`TODO`, `tbd`, `n/a`, `...`) are rejected.

| Directive | Lifts | Subject |
|---|---|---|
| `removes:` / `deletes:` | `deletion-rationale` | File path, directory prefix, or test function name (or unscoped with `require_scope = false`) |
| `allow-assertion-drop:` | `assertion-reduction` | Test function name, file path, or directory prefix |
| `allow-ignore:` | `ignored-tests` | Test function name |
| `allow-gate-weakening:` | `config-integrity` | Gate id |
| `allow-golden-update:` | `golden-output` | Snapshot/fixture file path or directory prefix |
| `allow-regression:` | `bench-regression` | Benchmark name, file stem, or arm, plus non-empty rationale |

### Inline Line Exemptions

Single line exceptions in source code or documentation use inline directives or the `docs-lint: allow` alias:
```rust
// Rust source:
let _ = 1; // discipline:allow(pii)
```
```markdown
<!-- Markdown documentation: -->
<!-- discipline:allow(time-estimates) -->
```
```text
# General text / scripts:
planned for 2 weeks docs-lint: allow
```

Every report records the exact count of lines exempted by inline markers.

---

## Trust Model

Discipline distinguishes between **configurable** and **bypassable**:
1. **Config integrity:** A pull request cannot weaken its own `discipline.toml` without triggering `config-integrity`. If an agent disables a gate or grows an exemption list, the PR is rejected unless an authorized `allow-gate-weakening:` directive is present.
2. **Directive channel enforcement:** Directives are parsed exclusively from trusted channels specified in `directives.sources` (defaulting to `["pr-body", "commits"]`).
3. **Hidden directive policy:** By default, HTML comment-wrapped directives in PR bodies are forbidden (`directives.allow_hidden = false`) to ensure reviewers see all requested waivers.
4. **Machine gate for human sign-off:** When `directives.fail_on_overrides = true` (or `--fail-on-overrides`), any applied override causes Discipline to exit `1`, requiring an authorized human approver to bypass or merge.
5. **Residual gap:** Workflow files (`.github/workflows/*.yml`) are evaluated by CI from the PR head commit; an agent could conceivably edit the workflow step to pass `disable: ...`. Repositories should protect workflow files and `discipline.toml` with `CODEOWNERS` and branch protection rules until the planned `ci-integrity` gate ships.

---

## Adoption Configurations

Reference configurations proven in production repositories:

### `orieg/php-judy` (C Extension & PHP Runtime)

Measured residue against merge base `HEAD~30` with unconfigured defaults:
- `time-estimates`: 1 violation (`BENCHMARK.md:1857`, historical runtime duration `one day`). <!-- discipline:allow(time-estimates) -->
- `assertion-reduction`: 1 violation (`tests/string_to_entry_005.phpt`, newly added NUL-bearing PHP test fixture).
- `pii`: 1 violation (`examples/ip-range-lookup.php`, sample LAN address `192.168.1.50`). <!-- discipline:allow(pii) -->
- `agents-md`: 1 violation (`CLAUDE.md`, unlinked guide diverging from `AGENTS.md`).
- 5 informational warnings (Zend engine C preprocessor macro expansions in `php_judy.c`, `php_judy.h`, `judy_handlers.c`, `judy_iterator.c`, `Judy_arginfo.h`).

Minimal configuration:

```toml
[meta]
version = 1
name = "php-judy"

[gates.pii]
# Sample script demonstrating IP address lookup on Judy arrays
exempt_paths = [
    "examples/ip-range-lookup.php",
]

[gates.time-estimates]
# Historical benchmark documentation references runtime durations
exempt_paths = [
    "BENCHMARK.md",
]

[gates.assertion-reduction]
# PHP binary string entry test intentionally contains NUL bytes
exempt_paths = [
    "tests/string_to_entry_005.phpt",
]
```

---

## Platform Quickstarts

Discipline delivers a single static binary and a composite shell action that runs identically across modern CI/CD engines.

### GitHub Actions

```yaml
name: CI Sentinel

on:
  pull_request:
    types: [opened, synchronize, reopened, edited]

jobs:
  discipline:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0 # merge base must be reachable
      - uses: orieg/discipline@v0
        with:
          fail_on_warnings: true
```

### GitLab CI/CD

Include the remote pipeline template directly:

```yaml
include:
  - remote: 'https://raw.githubusercontent.com/orieg/discipline/main/templates/discipline.gitlab-ci.yml'
```

Or configure a standalone job emitting native GitLab Code Quality diffs:

```yaml
discipline:gate:
  stage: test
  image:
    name: ghcr.io/orieg/discipline:latest
    entrypoint: [""]
  variables:
    GIT_STRATEGY: clone
    GIT_DEPTH: 0
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH
  before_script:
    - git fetch origin $CI_MERGE_REQUEST_TARGET_BRANCH_NAME --depth=100 || true
  script:
    - discipline check
  artifacts:
    reports:
      codequality: gl-codequality.json
      junit: junit.xml
    paths:
      - gl-codequality.json
      - junit.xml
    when: always
```

### Forgejo Actions

Forgejo Actions runs natively via `forgejo-runner` using workflows in `.forgejo/workflows/`. The composite action is shell-only and requires zero JavaScript runtime:

```yaml
      - uses: https://github.com/orieg/discipline@v0
        with:
          binary_path: /opt/discipline/discipline
```

### Gitea Actions

The same composite action runs under Gitea's `act_runner` without modification:

```yaml
      - uses: https://github.com/orieg/discipline@v0
        with:
          binary_path: /opt/discipline/discipline
```

### Argo Workflows

Use [`templates/argo-workflow-template.yaml`](https://github.com/orieg/discipline/blob/main/templates/argo-workflow-template.yaml) in GitOps pipelines to execute pre-merge diff checks:

```yaml
- name: run-discipline-gate
  templateRef:
    name: discipline-sentinel
    template: discipline-gate
  arguments:
    parameters:
      - name: repo-url
        value: "https://github.com/my-org/my-repo.git"
      - name: target-branch
        value: "main"
      - name: source-branch
        value: "feat/my-feature"
```

### pre-commit Hook

```yaml
repos:
  - repo: https://github.com/orieg/discipline
    rev: v0.2.0
    hooks:
      - id: discipline          # compiles via cargo
      # Or: - id: discipline-system # uses pre-installed binary on PATH
```

Or run staged inspection directly in git pre-commit hooks:
```bash
discipline check --staged
```

### Docker Container

```bash
docker run --rm -v "$PWD":/workspace ghcr.io/orieg/discipline:latest check --base origin/main
```

### Standalone CLI

```bash
discipline check --base origin/main   # compare working tree against merge base
discipline check --staged             # check staged index against HEAD
discipline gates                      # print effective gate configuration
discipline self-test                  # execute positive and negative controls
discipline schema                     # output JSON Schema for discipline.toml
```

---

## Report Formats

Discipline produces multi-target reports from a single execution run:

| Format | Option / Artifact | Destination & Use Case |
|---|---|---|
| **Human Terminal (stdout)** | Default stdout | ANSI-colored terminal summary with per-gate examined counts, notes, and file/line locations. |
| **Machine JSON Report** | `--report <path>` | Full JSON outcome with detailed violation records, notes, examined tallies, and applied overrides. |
| **GitHub Step Summary** | `GITHUB_STEP_SUMMARY` | Formatted Markdown table appended to GitHub Actions run summaries. |
| **GitLab Code Quality** | `gl-codequality.json` | JSON format rendered directly in GitLab Merge Request diff widgets. |
| **SARIF** | `--sarif <path>` | OASIS SARIF v2.1.0 report for GitHub Code Scanning, VS Code, and security dashboards. |
| **JUnit XML** | `--junit <path>` | Standard test results XML for CI test summary dashboards and flaky test tracking. |

---

## Override Audit Trail

When an authorized directive is parsed and applied:
1. **Audit Record:** The gate outcome records the override in its result structure, naming the gate, subject, and reason.
2. **Action Outputs:** Outputs `overrides` (total count of applied overrides) and `overridden_gates` (comma-separated list of gate ids) are populated.
3. **Machine Report:** Included in the JSON report under `overrides_applied` for compliance logging.
4. **Enforced Sign-off:** Setting `directives.fail_on_overrides = true` (or passing `--fail-on-overrides`) causes Discipline to exit `1` whenever any override is present. This blocks automated merge and mandates human sign-off while preserving the audit trail.

---

## Recipes & Monorepo Setup

Discipline natively supports polyglot monorepos and multi-tier architectures using path scoping, vocabulary extension, and command gate presets.

### Polyglot Monorepo: Rust Core + TypeScript Frontend + Python Tooling

In a repository containing multiple language stacks (e.g. `crates/` for Rust services, `apps/web/` for Next.js TypeScript frontend, and `scripts/` for Python data pipelines):

```toml
[meta]
version = 1
name = "polyglot-monorepo"

[gates.assertion-reduction]
enabled = true
severity = "error"
extra_assert_macros = ["custom_assert!", "verify_invariant!"]
assert_helper_fns = ["assert_response_ok", "check_bounds"]

[gates.vacuous-tests]
enabled = true
severity = "error"

[gates.unsafe-safety-comment]
enabled = true
severity = "error"
exempt_paths = ["tests/**"]

[gates.golden-output]
enabled = true
paths = ["**/fixtures/**", "apps/web/__snapshots__/**", "**/*.snap"]

[gates.dependency-delta]
enabled = true
severity = "error"
banned_dependencies = ["left-pad", "evil-package"]
forbid_wildcards = true
require_git_pins = true

[gates.command]
enabled = true
commands = [
  { name = "cargo-mutants", preset = "cargo-mutants", timeout_seconds = 300 },
  { name = "frontend-coverage", preset = "lcov", policy_files = ["apps/web/coverage/lcov.info"] }
]
```

### High-Assurance Agent Guard Configuration

For automated coding agents running in iterative development loops, combine `fail_on_overrides = true` with `--format agent-prompt` to enforce that agents repair defects rather than bypassing gates:

```toml
[meta]
version = 1
name = "high-assurance-agent"

[directives]
sources = ["pr-body"]
allow_hidden = false
fail_on_overrides = true

[gates.time-estimates]
enabled = true
severity = "error"

[gates.pii]
enabled = true
severity = "error"
home_paths = true
lan_ips = true

[gates.agent-scratch]
enabled = true
severity = "error"
```

In CI, run:
```bash
discipline check --base origin/main --format agent-prompt
```
