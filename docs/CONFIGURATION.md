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

Discipline employs a 5-layer configuration hierarchy. With zero configuration, each available gate runs with its built-in default enablement and severity: correctness and integrity gates default to `error`, while heuristic and brownfield-sensitive gates (`time-estimates`, `bench-regression`, `agents-md`, and `suppression-delta`) default to `warning` by design, and gates that need repository-specific input are off until enabled. The *Default* column of the schema table below is generated from the compiled defaults; per-gate rationale is in [`GATES.md`](GATES.md#default-severity-by-gate), and changes to defaults are recorded in the [Default Changes ledger](ROADMAP.md#default-changes-compatibility-ledger). Every layer merges deterministically; nothing turns off silently.

| Layer | Source | Precedence | Description |
|---|---|---|---|
| **1. Built-in defaults** | Compiled binary | Lowest | Per-gate default enablement and severity (correctness/integrity: `"error"`, heuristic/bench/suppression: `"warning"`; input-dependent gates off). |
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
| `directives.allow_hidden` | boolean | `false` | Allow directives hidden inside HTML comments &lt;!-- --&gt; (default: false) |
| `directives.allowed_override_actors` | list | `[]` | Actors authorized to apply overrides even when fail_on_overrides is true (default: []) |
| `directives.fail_on_overrides` | boolean | `false` | Treat applied overrides as failures requiring human sign-off (default: false) |
| `directives.sources` | list | `["pr-body","commits"]` | Allowed directive sources: pr-body, commits (default: ["pr-body", "commits"]) |
| `gates.agent-scratch.enabled` | boolean | `true` | Whether this gate is active |
| `gates.agent-scratch.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.agent-scratch.paths` | list | *(7 entries)* | Directory and file globs that must never be tracked |
| `gates.agent-scratch.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.agents-md.enabled` | boolean | `true` | Whether this gate is active |
| `gates.agents-md.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.agents-md.severity` | string | `"warning"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.archive-contents.archive_path` | string | *(unset)* | Glob pattern matching the built archive file |
| `gates.archive-contents.enabled` | boolean | `false` | Whether this gate is active |
| `gates.archive-contents.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.archive-contents.forbidden_patterns` | list | `[]` | Regex patterns forbidden inside the archive |
| `gates.archive-contents.required_paths` | list | `[]` | Files required to exist inside the archive |
| `gates.archive-contents.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.archive-contents.strip_components` | integer | `0` | Leading directory components to strip from archive paths |
| `gates.assertion-reduction.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |
| `gates.assertion-reduction.enabled` | boolean | `true` | Whether this gate is active |
| `gates.assertion-reduction.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.assertion-reduction.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions |
| `gates.assertion-reduction.min_assertions_per_test` | integer | *(unset)* | Minimum assertions required per test method |
| `gates.assertion-reduction.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.bench-regression.advisory_pct` | number | `0.1` | Advisory review percentage (default: 0.1%) |
| `gates.bench-regression.allow_cross_host` | boolean | `false` | Allow benchmark comparison across mismatched host/runner provenance |
| `gates.bench-regression.base_file` | string | *(unset)* | In-job base benchmark result file path for dual-file regression checks |
| `gates.bench-regression.citation_measurement_jobs` | array of tables | `[]` | CI jobs that produce gated numbers, each with the step that gates them. A cited run that concluded `failure` is admitted only when every listed job it started reached its guard step with every earlier step green. Empty: a cited `failure` run cannot be told from a crashed benchmark and leaves the gate armed. |
| `gates.bench-regression.citation_measurement_jobs[].guard` | string | *(required)* | Name of the step in that job that reads the numbers and enforces the regression guard |
| `gates.bench-regression.citation_measurement_jobs[].job` | string | *(required)* | Job display name as the CI API lists it |
| `gates.bench-regression.citation_source_paths` | list | `[]` | Repo-relative files or directories whose changes can move a gated number. A cited data artifact last committed before the branch's newest change under these paths is stale. Empty: artifact citations cannot be dated and leave the gate armed. |
| `gates.bench-regression.enabled` | boolean | `true` | Whether this gate is active |
| `gates.bench-regression.exempt_arms` | list | `[]` | Benchmark arms exempted from regression checks: exact name, the name as the benchmark prints it (`map_get random` matches `map_get/random`), a glob (`*.heap.*`), a trailing-`*` prefix, or a `::`/`/` path suffix. An entry matching no arm in the run is an error. |
| `gates.bench-regression.exempt_paths` | list | *(6 entries)* | File path globs exempted from this gate |
| `gates.bench-regression.head_file` | string | *(unset)* | In-job head benchmark result file path for dual-file regression checks |
| `gates.bench-regression.max_noise_cv` | number | *(unset)* | Maximum acceptable coefficient of variation (std_dev / mean) |
| `gates.bench-regression.mode` | string | `"version-vs-version"` | Evaluation mode: `version-vs-version` compares base and head artifacts of the same arms (preferred when the old version can be built in the same run); `paired-ratio` compares a ratio of two arms measured in the same interleaved rounds against a committed ratio baseline (when building the old version is impractical). The two are not interchangeable. |
| `gates.bench-regression.noise_floor_pct` | number | `0.5` | Noise floor percentage (default: 0.5%) |
| `gates.bench-regression.noise_margin_pct` | number | *(unset)* | Configurable noise margin added to tolerance_pct |
| `gates.bench-regression.paths` | list | *(6 entries)* | Benchmark artifact globs tracked across revisions |
| `gates.bench-regression.provenance` | string | *(unset)* | Expected host/runner provenance tag for benchmark artifacts |
| `gates.bench-regression.ratio_baseline` | string | *(unset)* | Committed paired-ratio baseline (`discipline-bench-ratio-baseline/v1`), produced by `discipline bench derive`. Read from the base ref, never from head; loosening it needs a scoped `allow-regression: &lt;path&gt;` directive. |
| `gates.bench-regression.ratio_tolerance_pct` | number | *(unset)* | Optional minimum paired-ratio threshold in percent. It only widens a derived floor; configured for an axis with no derived floor, it is a configuration error. |
| `gates.bench-regression.require_sourced_override` | boolean | `false` | Require allow-regression reasons to cite a CI run URL or artifact path and name the arms. Every citation is also checked for freshness: a cited run must have completed, reached its regression guard, and measured a commit reachable from the head; a cited data artifact must post-date the branch's newest change under `citation_source_paths`. A citation that cannot be checked (no `gh`, unauthenticated, rate limited) is reported by name and leaves the gate armed. |
| `gates.bench-regression.severity` | string | `"warning"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.bench-regression.tolerance_pct` | number | `0.5` | Maximum allowed regression percentage |
| `gates.ci-integrity.diff_only` | boolean | `true` | When true, scans only modified workflow files rather than all workflows |
| `gates.ci-integrity.documented_job_count_path` | string | *(unset)* | Path to catalog documentation stating job count |
| `gates.ci-integrity.documented_job_count_pattern` | string | *(unset)* | Regex pattern to extract job count from documentation |
| `gates.ci-integrity.enabled` | boolean | `true` | Whether this gate is active |
| `gates.ci-integrity.excluded_jobs` | list | `["detect-changes"]` | Job names excluded from rollup dependency requirements |
| `gates.ci-integrity.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.ci-integrity.first_party_action_prefixes` | list | `["actions/","github/"]` | Action prefixes considered first-party and excused from commit SHA pinning |
| `gates.ci-integrity.forbid_continue_on_error` | boolean | `true` | Forbid continue-on-error: true in workflow jobs or steps |
| `gates.ci-integrity.forbid_or_true` | boolean | `true` | Forbid \|\| true and set +e error masking in run commands |
| `gates.ci-integrity.pin_actions` | boolean | `true` | Ensure third-party GitHub actions are pinned by 40-character commit SHA |
| `gates.ci-integrity.rollup_job` | string | `"ci-gate"` | Name of the rollup job that must depend on all jobs |
| `gates.ci-integrity.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.ci-integrity.workflows` | list | *(2 entries)* | Workflow file patterns to inspect |
| `gates.ci-skip-set.change_job` | string | `"detect-changes"` | Change-detection job whose outputs gate the conditional jobs; it must have succeeded. Empty string = no such job |
| `gates.ci-skip-set.enabled` | boolean | `true` | Whether this gate is active |
| `gates.ci-skip-set.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.ci-skip-set.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.ci-skip-set.unconditional_jobs` | list | `[]` | Jobs that must never be skipped, whatever their dependencies did |
| `gates.ci-skip-set.workflow` | string | `".github/workflows/ci.yml"` | Repo-relative path of the workflow whose rollup job supplies the runtime needs context (DISCIPLINE_CI_CONTEXT) |
| `gates.command.allow_zero` | boolean | `false` | Whether zero items selected is allowed |
| `gates.command.canary_command` | string | *(unset)* | Optional negative-control canary command |
| `gates.command.canary_expected_diagnostic` | string | *(unset)* | Expected diagnostic string that canary must produce |
| `gates.command.command` | string | *(unset)* | Primary command to execute |
| `gates.command.commands` | array of tables | `[]` | Multi-command suite entries |
| `gates.command.commands[].allow_zero` | boolean | *(per entry)* | Whether zero items selected is allowed |
| `gates.command.commands[].canary_command` | string | *(per entry)* | Optional negative-control canary command |
| `gates.command.commands[].canary_expected_diagnostic` | string | *(per entry)* | Expected diagnostic string that canary must produce |
| `gates.command.commands[].command` | string | *(per entry)* | Command string to execute |
| `gates.command.commands[].count_pattern` | string | *(per entry)* | Regex pattern to extract an integer count |
| `gates.command.commands[].forbid_output` | list | *(per entry)* | Output patterns that must not appear in stdout or stderr |
| `gates.command.commands[].min_count` | integer | *(per entry)* | Minimum count required |
| `gates.command.commands[].name` | string | *(required)* | Name or identifier of the command |
| `gates.command.commands[].preset` | string | *(per entry)* | Predefined turnkey preset name (e.g. cargo-mutants, cargo-deny, loom) |
| `gates.command.commands[].timeout_seconds` | integer | *(per entry)* | Execution timeout in seconds |
| `gates.command.commands[].zero_items_pattern` | string | *(per entry)* | Pattern that indicates zero items were executed |
| `gates.command.count_pattern` | string | *(unset)* | Regex pattern to extract an integer count |
| `gates.command.enabled` | boolean | `true` | Whether this gate is active |
| `gates.command.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.command.forbid_output` | list | `[]` | Output patterns that must not appear in stdout or stderr |
| `gates.command.min_count` | integer | *(unset)* | Minimum count required |
| `gates.command.preset` | string | *(unset)* | Predefined turnkey preset name (e.g. cargo-mutants, cargo-deny, loom) |
| `gates.command.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.command.timeout_seconds` | integer | *(unset)* | Execution timeout in seconds (default: 60s) |
| `gates.command.zero_items_pattern` | string | *(unset)* | Pattern that indicates zero items were executed |
| `gates.config-integrity.enabled` | boolean | `true` | Whether this gate is active |
| `gates.config-integrity.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.config-integrity.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.deletion-rationale.allow_hidden` | boolean or null | *(unset)* | When true, HTML-comment-wrapped directives are accepted for deletions |
| `gates.deletion-rationale.enabled` | boolean | `true` | Whether this gate is active |
| `gates.deletion-rationale.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.deletion-rationale.paths` | list | `["**"]` | Path globs where file deletions require a rationale |
| `gates.deletion-rationale.require_scope` | boolean | `true` | When true, directive must name the deleted file or test |
| `gates.deletion-rationale.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.dependency-delta.allow_dependencies` | list | `[]` | Explicit list of allowed dependency package names |
| `gates.dependency-delta.allow_wildcards` | boolean | `false` | Whether wildcard versions are permitted (default: false) |
| `gates.dependency-delta.deny_dependencies` | list | `[]` | Explicit list of forbidden dependency package names |
| `gates.dependency-delta.deny_file` | string | `"deny.toml"` | Path to deny.toml policy file |
| `gates.dependency-delta.enabled` | boolean | `true` | Whether this gate is active |
| `gates.dependency-delta.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.dependency-delta.manifests` | list | *(9 entries)* | Manifest file globs to inspect |
| `gates.dependency-delta.require_git_pins` | boolean | `true` | Whether git dependencies must specify an immutable commit or tag pin (default: true) |
| `gates.dependency-delta.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.golden-output.allow_updates` | boolean | *(per entry)* | Permit snapshot updates without error |
| `gates.golden-output.enabled` | boolean | `true` | Whether this gate is active |
| `gates.golden-output.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.golden-output.paths` | list | *(4 entries)* | Committed golden/snapshot globs whose edits require a directive |
| `gates.golden-output.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.ignored-tests.approved_predicates` | list | `[]` | Conditional ignore predicates (e.g. miri) approved by policy |
| `gates.ignored-tests.enabled` | boolean | `true` | Whether this gate is active |
| `gates.ignored-tests.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.ignored-tests.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.issue-link.enabled` | boolean | `false` | Whether this gate is active |
| `gates.issue-link.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.issue-link.pattern` | string | *(unset)* | Custom regex pattern required in PR title or body |
| `gates.issue-link.require_in_commit_if_no_pr` | boolean | `false` | Require issue link in commit messages when no PR metadata is supplied |
| `gates.issue-link.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.manifest-sync.enabled` | boolean | `false` | Whether this gate is active |
| `gates.manifest-sync.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.manifest-sync.rules` | array of tables | `[]` | Rules reconciling packaging manifests against git-tracked files |
| `gates.manifest-sync.rules[].exclude_paths` | list | *(per entry)* | Globs excluded from manifest registration requirement |
| `gates.manifest-sync.rules[].extract_regex` | string | *(required)* | Regex to extract relative file paths from manifest |
| `gates.manifest-sync.rules[].manifest` | string | *(required)* | Path to packaging manifest (e.g. package.xml) |
| `gates.manifest-sync.rules[].watched_paths` | list | *(required)* | Git file globs that must be registered in the manifest |
| `gates.manifest-sync.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.miri.args` | list | `[]` | Additional CLI arguments passed to cargo miri test |
| `gates.miri.enabled` | boolean | `false` | Whether this gate is active |
| `gates.miri.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.miri.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.miri.timeout_seconds` | integer | `600` | Maximum execution time in seconds before failing closed (default: 600) |
| `gates.msrv.command` | string or null | *(unset)* | Command to run to verify MSRV compatibility |
| `gates.msrv.enabled` | boolean | `false` | Whether this gate is active |
| `gates.msrv.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.msrv.pinned_version` | string or null | *(unset)* | Explicit MSRV version string (e.g. "1.90.0") |
| `gates.msrv.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.pii.agent_config_refs` | boolean | `true` | When true, flags references to personal agent configuration directories and playbook docs |
| `gates.pii.allow_patterns` | list | `[]` | Regex patterns exempted from rejection |
| `gates.pii.allowed_users` | list | *(8 entries)* | Username tokens permitted inside home-directory paths |
| `gates.pii.diff_only` | boolean | `false` | When true, scans only modified lines in the git diff rather than all tracked files |
| `gates.pii.enabled` | boolean | `true` | Whether this gate is active |
| `gates.pii.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.pii.extra_patterns` | list | `[]` | Additional regex patterns to reject |
| `gates.pii.home_paths` | boolean | `true` | Check for leaked home directory paths |
| `gates.pii.hostname_denylist` | list | `[]` | Whole-token, case-insensitive hostnames that must not appear |
| `gates.pii.lan_ips` | boolean | `true` | Check for leaked private LAN IPs |
| `gates.pii.scan_pr_body` | boolean | `true` | Whether to scan PR description text |
| `gates.pii.secrets` | boolean | `true` | Check for leaked private keys and high-entropy API tokens |
| `gates.pii.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.pr-checklist.enabled` | boolean | `false` | Whether this gate is active |
| `gates.pr-checklist.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.pr-checklist.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.provenance-tags.check_intervals` | boolean | `true` | Check published wall-clock ratios for confidence intervals or explicit qualifiers |
| `gates.provenance-tags.check_mechanisms` | boolean | `true` | Check for mechanism claims without hardware counter evidence or explicit hypothesis qualifiers |
| `gates.provenance-tags.check_paired_figures` | boolean | `true` | Check paired figures for shared workload IDs or differentiation tags |
| `gates.provenance-tags.check_tables` | boolean | `true` | Check markdown tables for unit-bearing numbers without table or caption provenance tags |
| `gates.provenance-tags.enabled` | boolean | `false` | Whether this gate is active |
| `gates.provenance-tags.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.provenance-tags.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.sanitizers.canary` | boolean | `false` | Whether to verify a negative-control race canary before main tests |
| `gates.sanitizers.enabled` | boolean | `false` | Whether this gate is active |
| `gates.sanitizers.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.sanitizers.sanitizer` | string | `"address"` | Sanitizer name to activate (e.g. "address", "thread") |
| `gates.sanitizers.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.sanitizers.timeout_seconds` | integer | `300` | Maximum execution time in seconds (default: 300) |
| `gates.scope-confinement.allowed_paths` | list | `[]` | Glob patterns of paths agents are authorized to modify |
| `gates.scope-confinement.enabled` | boolean | `false` | Whether this gate is active |
| `gates.scope-confinement.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.scope-confinement.forbidden_paths` | list | `[]` | Glob patterns of paths agents are strictly forbidden to touch |
| `gates.scope-confinement.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.shell-secrets.allow_patterns` | list | `[]` | Custom regex patterns exempted from violation |
| `gates.shell-secrets.diff_only` | boolean | `false` | When true, scans only modified lines in the git diff rather than all tracked files |
| `gates.shell-secrets.enabled` | boolean | `true` | Whether this gate is active |
| `gates.shell-secrets.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.shell-secrets.extra_secret_patterns` | list | `[]` | Additional custom regex patterns for sensitive secret variable names |
| `gates.shell-secrets.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.suppression-delta.allowed_suppressions` | list | `[]` | Specific suppression patterns explicitly permitted by policy |
| `gates.suppression-delta.enabled` | boolean | `true` | Whether this gate is active |
| `gates.suppression-delta.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.suppression-delta.max_increase` | integer | `0` | Maximum net increase in suppression annotations permitted (default: 0) |
| `gates.suppression-delta.severity` | string | `"warning"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.test-budget.corpus_dirs` | list | *(3 entries)* | Corpus directory patterns to monitor for seed file shrink |
| `gates.test-budget.enabled` | boolean | `true` | Whether this gate is active |
| `gates.test-budget.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.test-budget.fuzz_targets` | list | `["fuzz/Cargo.toml","fuzz/fuzz_targets/**"]` | Fuzz manifest and harness globs |
| `gates.test-budget.scan_scripts` | boolean | `true` | Whether to scan shell scripts |
| `gates.test-budget.scan_workflows` | boolean | `true` | Whether to scan workflow files |
| `gates.test-budget.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.test-floor.constant_file` | string | *(unset)* | File containing a floor constant |
| `gates.test-floor.constant_name` | string | *(unset)* | Name of the floor constant in constant_file |
| `gates.test-floor.enabled` | boolean | `true` | Whether this gate is active |
| `gates.test-floor.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.test-floor.min_tests` | integer | *(unset)* | Minimum required workspace test count |
| `gates.test-floor.required_suites` | list | `[]` | Required test suite files that must exist |
| `gates.test-floor.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.test-floor.test_command` | string | *(unset)* | Custom command to list or count tests |
| `gates.test-floor.tolerance` | integer | `0` | Allowed test count decrease below floor or base before violation (default: 0) |
| `gates.time-estimates.allow_patterns` | list | `[]` | Regex patterns permitted as operational exceptions; matched per line and across soft-wrapped lines of a paragraph, exempting only the matched text |
| `gates.time-estimates.diff_only` | boolean | `false` | When true, scans only modified lines in the git diff rather than all tracked files |
| `gates.time-estimates.enabled` | boolean | `true` | Whether this gate is active |
| `gates.time-estimates.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.time-estimates.extra_patterns` | list | `[]` | Additional banned regex patterns |
| `gates.time-estimates.include` | list | `["**/*.md"]` | File globs swept for duration estimates |
| `gates.time-estimates.scan_pr_body` | boolean | `true` | Whether to scan PR description text |
| `gates.time-estimates.severity` | string | `"warning"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.unsafe-budget.allow_increase` | boolean | `false` | Whether total unsafe count may increase over base ref without override |
| `gates.unsafe-budget.enabled` | boolean | `false` | Whether this gate is active |
| `gates.unsafe-budget.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.unsafe-budget.max_unsafe` | integer or null | *(unset)* | Maximum total number of unsafe sites allowed in head ref |
| `gates.unsafe-budget.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.unsafe-safety-comment.enabled` | boolean | `true` | Whether this gate is active |
| `gates.unsafe-safety-comment.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.unsafe-safety-comment.placeholders` | list | *(17 entries)* | Additional placeholder words or phrases to reject in SAFETY comments |
| `gates.unsafe-safety-comment.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.vacuous-tests.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |
| `gates.vacuous-tests.enabled` | boolean | `true` | Whether this gate is active |
| `gates.vacuous-tests.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.vacuous-tests.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions |
| `gates.vacuous-tests.min_assertions_per_test` | integer | *(unset)* | Minimum assertions required per test method |
| `gates.vacuous-tests.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.version-lockstep.enabled` | boolean | `false` | Whether this gate is active |
| `gates.version-lockstep.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.version-lockstep.groups` | array of tables | `[]` | Groups of sources that must declare identical version strings |
| `gates.version-lockstep.groups[].name` | string | *(required)* | Name of the version lockstep group |
| `gates.version-lockstep.groups[].sources` | array of tables | *(required)* | Files and capture regexes whose versions must match |
| `gates.version-lockstep.groups[].sources[].path` | string | *(required)* | Source file path |
| `gates.version-lockstep.groups[].sources[].regex` | string | *(required)* | Regex pattern capturing the version string in group 1 |
| `gates.version-lockstep.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `meta.description` | string | *(per entry)* | Optional short description of the project |
| `meta.mode` | string | `"enforcing"` | Operating mode: 'enforcing' exits non-zero on violations; 'advisory' runs all checks and emits reports but exits 0. |
| `meta.name` | string | *(required)* | Repository or project name |
| `meta.version` | integer | `1` | Configuration schema version (must be 1) |
<!-- /generated -->

---

## Action Reference

The composite action (`action.yml`) runs identically in GitHub Actions, Gitea Actions, and Forgejo Actions. It operates with zero Node.js runtime overhead, executing entirely via shell and the static binary.

### Action Inputs

<!-- generated:action-inputs -->
| Input | Default | Description |
|---|---|---|
| `config` | `discipline.toml` | Path to discipline.toml. When the file is absent, built-in defaults apply (each gate at its built-in default enablement and severity). |
| `suite` | `all` | Suite to run: all, agent-guard, hygiene, integrity |
| `base_ref` | *(none)* | Branch or commit the change is measured against. Default: PR base branch, else the pushed-from commit, else the default branch. |
| `enable` | *(none)* | Gate ids to force on (comma or newline separated). See `discipline gates`. |
| `disable` | *(none)* | Gate ids to force off (comma or newline separated), e.g. "time-estimates, pii". |
| `config_override` | *(none)* | Inline TOML merged over discipline.toml: tables merge, lists append, scalars replace. |
| `hostname_denylist` | *(none)* | Hostnames the pii gate must reject (comma or newline separated). Pass a secret; matches are never echoed. |
| `fail_on_warnings` | `false` | Treat warnings as failures. |
| `fail_on_overrides` | `false` | Treat applied overrides as failures (requires human sign-off). |
| `advisory` | `false` | Advisory mode: run all checks and emit reports, but exit code 0 even if violations occur. |
| `actor` | `${{ github.actor }}` | Actor executing the check (defaults to github.actor or forge equivalent; used for allowed_override_actors). |
| `directive_sources` | *(none)* | Comma-separated list of allowed directive sources (pr-body, commits). |
| `pr_body` | `${{ github.event.pull_request.body }}` | PR description: carries override directives and is itself scanned by hygiene gates. |
| `pr_title` | `${{ github.event.pull_request.title }}` | PR title: checked by hygiene gates (e.g. issue-link). |
| `working_directory` | `.` | Directory of the repository to check. |
| `version` | *(none)* | Release to download (e.g. v0.1.0). Default: the tag this action was referenced by, else the latest release. |
| `binary_path` | *(none)* | Use this discipline binary instead of downloading one (air-gapped Gitea/Forgejo runners, self-tests). |
| `download_url` | `https://github.com/orieg/discipline/releases` | Base URL of the release store, for mirrors. |
| `baseline_file` | *(none)* | Path to grandfathering baseline file (defaults to discipline-baseline.toml if present). |
| `no_baseline` | `false` | Ignore grandfathering baseline even if present. |
| `ci_context` | *(none)* | Rollup job only: `toJson(needs)` of the rollup job (inline JSON, or a path to a file holding it) for the ci-skip-set gate. Empty: the gate reports "not evaluated". |
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
| `baselined` | Number of grandfathered findings matching baseline (not blocking) |
| `overridden_gates` | Comma separated ids of the gates that had an override applied |
| `passed_gates` | Number of passing gates |
| `examined_items` | Total number of items examined across all enabled gates |
| `report` | Path of the JSON report |
| `install_error` | Why installing the binary failed (empty on success) |
| `bin` | Path to the installed discipline executable |
<!-- /generated -->

---

## CLI Reference

Discipline provides a standalone CLI for local developer workflows, pre-commit hooks, and CI scripting:

<!-- generated:cli -->
| Subcommand | Description |
|---|---|
| `check` | Run the configured gates. Exit 0 = pass, 1 = violations, 2 = could not check |
| `diff` | Shorthand for checking uncommitted or working tree changes against HEAD |
| `baseline` | Record or manage grandfathered finding baselines |
| `init` | Write a discipline.toml with every available gate at its default |
| `gates` | List every gate: id, suite, availability, and effective state |
| `schema` | Print the JSON Schema for discipline.toml |
| `self-test` | Run the embedded negative / positive controls against this binary |
| `completions` | Generate shell completion script to stdout (bash, zsh, fish, powershell, elvish) |
| `install-hooks` | Install pre-commit hook in the local git repository |
| `bench` | Benchmark tooling for the bench-regression gate |
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
| `removes:` / `deletes:` / `discipline:allow(deletion-rationale)` / `allow(deletion-rationale)` | `deletion-rationale` | File path, directory prefix, or test function name (or unscoped with `require_scope = false`) |
| `allow-assertion-drop:` / `discipline:allow(assertion-reduction)` / `allow(assertion-reduction)` | `assertion-reduction` | Test function name, file path, or directory prefix |
| `allow-ignore:` / `discipline:allow(ignored-tests)` / `allow(ignored-tests)` | `ignored-tests` | Test function name |
| `allow-gate-weakening:` / `discipline:allow(config-integrity)` / `allow(config-integrity)` | `config-integrity` | Gate id |
| `allow-golden-update:` / `discipline:allow(golden-output)` / `allow(golden-output)` | `golden-output` | Snapshot/fixture file path or directory prefix |
| `allow-regression:` / `discipline:allow(bench-regression)` / `allow(bench-regression)` | `bench-regression` | Benchmark name, file stem, or arm, plus non-empty rationale |
| `allow-command:` / `discipline:allow(command)` / `allow(command)` | `command` | Subcommand or command line invocation, plus non-empty rationale |
| `allow-dependency:` / `discipline:allow(dependency-delta)` / `allow(dependency-delta)` | `dependency-delta` | Dependency package name or manifest path |
| `allow-test-shrink:` / `allow-floor-drop:` / `discipline:allow(test-budget)` / `allow(test-budget)` / `discipline:allow(test-floor)` / `allow(test-floor)` | `test-budget`, `test-floor` | Test count delta, budget parameter, or suite name |
| `allow-ci-weakening:` / `allow-unpinned-action:` / `discipline:allow(ci-integrity)` / `allow(ci-integrity)` | `ci-integrity` | Workflow path, job id, or security check rationale |
| `allow-nul:` / `allow-nul-byte:` / `discipline:allow(vacuous-tests)` / `allow(vacuous-tests)` | `assertion-reduction`, `vacuous-tests` | Corrupt or NUL-byte fixture file path |
| `secrets-argv-ok:` / `discipline:allow(shell-secrets)` / `allow(shell-secrets)` | `shell-secrets` | Shell script path or CLI command line |
| `no-issue:` / `discipline:no-issue:` / `discipline:allow(issue-link)` / `allow(issue-link)` | `issue-link` | PR or commit justification for omitted tracking issue |
| `allow-provenance:` / `allow-unpaired-figures:` / `discipline:allow(provenance-tags)` / `allow(provenance-tags)` | `provenance-tags` | Unmeasured figure, claim, or doc file path |
| `allow-archive-leak:` / `discipline:allow(archive-contents)` / `allow(archive-contents)` | `archive-contents` | Archive file path or leaked entry name |
| `allow-manifest-drift:` / `discipline:allow(manifest-sync)` / `allow(manifest-sync)` | `manifest-sync` | Manifest path or package field name |
| `allow-version-mismatch:` / `discipline:allow(version-lockstep)` / `allow(version-lockstep)` | `version-lockstep` | Mismatched crate name or manifest path |
| `allow-scope:` / `allow-scope-confinement:` / `discipline:allow(scope-confinement)` / `allow(scope-confinement)` | `scope-confinement` | Out-of-scope file path or module prefix |
| `allow-suppression:` / `allow-suppression-delta:` / `discipline:allow(suppression-delta)` / `allow(suppression-delta)` | `suppression-delta` | Specific suppression rule (`dead_code`, `noqa`, `type: ignore`) and/or file path |
| `allow-pr-checklist:` / `allow-checklist:` / `discipline:allow(pr-checklist)` / `allow(pr-checklist)` | `pr-checklist` | PR checklist item text or section |
| `allow-unsafe:` / `allow-unsafe-budget:` / `discipline:allow(unsafe-budget)` / `allow(unsafe-budget)` | `unsafe-budget` | Rust file path, function name, or module |
| `allow-msrv:` / `discipline:allow(msrv)` / `allow(msrv)` | `msrv` | Crate name or MSRV error diagnostic |
| `allow-miri:` / `discipline:allow(miri)` / `allow(miri)` | `miri` | Test name or unsupported Miri operation |
| `allow-sanitizers:` / `discipline:allow(sanitizers)` / `allow(sanitizers)` | `sanitizers` | Test or binary name with memory check rationale |

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

Discipline delivers a single static binary and a composite shell action that runs identically across modern CI/CD engines. For in-depth tutorials, multi-architecture container setups, and runner permissions, see the [CI/CD Platform Integration Guide](guides/ci-platforms.md). Quick-reference snippets for each platform follow:

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
    rev: v0.6.0
    hooks:
      - id: discipline          # compiles via cargo
      # Or: - id: discipline-system # uses pre-installed binary on PATH
```

Or run staged inspection directly in git pre-commit hooks:
```bash
discipline check --staged
```

### Docker Container

Official multi-arch (`linux/amd64`, `linux/arm64`) minimal OCI container images are published to GitHub Container Registry:
- `ghcr.io/orieg/discipline:latest`
- `ghcr.io/orieg/discipline:v0`
- `ghcr.io/orieg/discipline:v0.6.0`

Images are built on Alpine Linux with the statically linked musl `discipline` binary and `git` on `PATH`.

#### Non-Root Execution & Out-of-the-Box Ownership Resolution
The container runs under an unprivileged user (`USER 10001:10001`) to comply with strict container security policies (such as CIS Docker Benchmark and Kubernetes restricted PodSecurityStandards).

To ensure volume mounts owned by different host UIDs (including root-owned checkouts created by container runners) work out-of-the-box without requiring host permission changes, the image is built with:
- System-wide `safe.directory '*'` in `/etc/gitconfig` readable by `USER 10001`.
- An entrypoint wrapper (`/usr/local/bin/docker-entrypoint.sh`) dynamically registering the active working directory.
- `ENV DISCIPLINE_TRUST_WORKSPACE=1` by default.

Every container snippet works as written with no `--user` override and no manual `safe.directory` configuration required.

Run directly against any repository mounted to `/workspace`:
```bash
docker run --rm -v "$PWD":/workspace ghcr.io/orieg/discipline:latest check --base origin/main
```

#### Container Runner Environments (Environments Forbidding `uses:`)
The minimal container image contains only the static binary and git; it does **not** include a Node.js runtime.

In GitHub Actions, Gitea Actions (`act_runner`), and Forgejo Actions, `actions/checkout` requires Node.js. As a result, `actions/checkout` cannot run inside a job container using `runs-on: docker://ghcr.io/orieg/discipline:latest`.

**Recommended CI Integration Patterns:**

1. **Host runner using composite action (Standard):**
   ```yaml
   jobs:
     discipline:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v4
           with:
             fetch-depth: 0
         - uses: orieg/discipline@v0
   ```

2. **Container-only runner or environments forbidding `uses:` (`git clone` pattern):**
   When running in strict container execution environments or CI setups that forbid `uses:` actions, run directly inside the container and clone the repository:
   ```yaml
   jobs:
     discipline:
       runs-on: docker://ghcr.io/orieg/discipline:latest
       steps:
         - run: |
             git clone --depth 50 "${REPO_URL}" .
             discipline check --base main
   ```

3. **GitLab CI (`.gitlab-ci.yml`):**
   ```yaml
   discipline:
     image:
       name: ghcr.io/orieg/discipline:latest
       entrypoint: [""]
     variables:
       GIT_STRATEGY: clone
       GIT_DEPTH: 0
       DISCIPLINE_TRUST_WORKSPACE: "1"
     script:
       - discipline check --base origin/${CI_MERGE_REQUEST_TARGET_BRANCH_NAME:-main}
   ```

### Standalone CLI

Discipline is available as a standalone static binary across Linux and macOS.

#### Installation Methods

- **Quick Install (curl | bash)**:
  ```bash
  curl -fsSL https://orieg.github.io/discipline/install.sh | bash
  ```

- **Debian / Ubuntu (APT)**:
  ```bash
  sudo apt update && sudo apt install -y ca-certificates
  echo "deb [trusted=yes] https://orieg.github.io/discipline/apt/ stable main" | sudo tee /etc/apt/sources.list.d/discipline.list
  sudo apt update && sudo apt install -y discipline
  ```

- **Enterprise Linux / Fedora (RPM)**:
  ```bash
  sudo dnf config-manager --add-repo https://orieg.github.io/discipline/rpm/discipline.repo
  sudo dnf install -y discipline
  ```

- **macOS (Homebrew & MacPorts)**:
  ```bash
  # Homebrew
  brew tap orieg/tap && brew install discipline

  # MacPorts
  sudo port install discipline
  ```

- **Cargo**:
  ```bash
  cargo binstall discipline
  # or from source:
  cargo install --git https://github.com/orieg/discipline
  ```

#### CLI Execution

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

## Grandfathering Baseline Mode

When adopting Discipline on existing brownfield repositories, pre-existing code may trigger numerous violations across historical files (measured at v0.4.2 on two consumer repositories: 51 and 26 findings, nearly all pre-existing). Rather than disabling gates or littering inline directives across legacy files, Discipline provides a grandfathering baseline mode.

### 1. Generating a Baseline

There are two modes, and they record different populations. Pick by what you are grandfathering.

| Mode | Command | Records | Use when |
|---|---|---|---|
| **Whole tree** | `discipline baseline --write --whole-tree` | Every pre-existing blocking finding in the repository | **Adopting Discipline on an existing repository.** |
| Diff | `discipline baseline --write --base origin/main` | Only the blocking findings the current change introduced | Deliberately grandfathering findings a specific change adds. |

For brownfield adoption, use **whole-tree** mode:

```bash
discipline baseline --write --whole-tree
```

Whole-tree mode measures against the empty tree, so every tracked file is in scope. That matters because two kinds of gate see a repository differently:

- **Whole-tree gates** such as `pii` and `time-estimates` scan the tree regardless of the diff, so diff mode already reaches their pre-existing findings.
- **Diff-scoped gates** such as `vacuous-tests`, `assertion-reduction`, `ci-integrity` and `provenance-tags` only look at what changed. On a clean branch the diff is empty, so diff mode records **none** of their pre-existing findings, and the gate cannot be enabled without first fixing everything it would report.

`--whole-tree` and `--base` are mutually exclusive: one measures the repository, the other measures a change.

Whole-tree mode is for recording a baseline, not for checking. `discipline check` always measures a change.

#### Which severities are recorded

A baseline exists to let a blocking finding through, so by default it records only findings whose **effective** severity (after `severity` settings and finding-level overrides) would fail `discipline check` under the same configuration:

| Invocation | Records | Skips |
|---|---|---|
| `discipline baseline --write` | `error` | `warning`, `note` |
| `discipline baseline --write --fail-on-warnings` (or `DISCIPLINE_FAIL_ON_WARNINGS=true`, which the CI integrations set from their `fail_on_warnings` input) | `error`, `warning` | `note` |
| `discipline baseline --write --all-severities` | `error`, `warning`, `note` | nothing |

A non-blocking finding gains nothing from being grandfathered: `check` already passes with it, and it stays visible in every report. Recording it only adds bulk and, as the code moves, stale-entry churn. On one consumer repository (built-in defaults plus `provenance-tags`), a whole-tree baseline recorded 651 entries when every severity was recorded; 483 of them were warnings and notes that never block, including 230 C/C++ parse warnings and 231 lint-suppression warnings. The default records the 168 that block. Use `--all-severities` when you intend to raise a gate's severity later and want the existing population grandfathered in advance; run `baseline` with the same `--fail-on-warnings` setting that `check` uses in CI.

Nothing is dropped silently. Both the dry run and `--write` print what was recorded and what was skipped, by severity and gate:

```text
ok: recorded 1 grandfathered finding to discipline-baseline.toml
  recorded: 1 error
  skipped: 1 warning (suppression-delta: 1), 1 note (pii: 1)
  (non-blocking under the current configuration; pass --all-severities to record them, or --fail-on-warnings if `check` runs with it)
```

The file records a line-number-independent SHA-256 fingerprint per finding:

```toml
# Discipline Findings Baseline
# Generated by `discipline baseline --write`
# Do not edit manually; commit alongside configuration changes.

version = 1

[[findings]]
gate = "time-estimates"
rule = "Time Estimate"
path = "docs/old_plan.md"
fingerprint = "5847c9fe7cec2622f5c427937d6a48adebd8e240c027a55d9fdaf8009e8b7100"
```

Committing a new or grown baseline trips `config-integrity` ("Baseline Grew Without Directive"). `baseline --write` prints the exact directive line to add to the commit message.

Because fingerprints are computed from `sha256(gate:rule:path:sha256(trimmed_line))` rather than physical line numbers, line shifts caused by unrelated edits elsewhere in the file do not invalidate or churn the baseline.

### 2. Subsequent CI Execution

Subsequent runs automatically detect `discipline-baseline.toml` if present:
- **Pre-existing findings:** Grandfathered and reported as a non-blocking note: `"N baselined findings not blocking"`. They do not cause non-zero exit codes.
- **New violations:** Fail CI immediately with exit code 1.
- **Transparency:** The baselined count is reported in every output format (terminal, GitHub step summaries, JSON, JUnit, SARIF, GitLab) and exposed to GitHub Actions workflows via the `steps.<id>.outputs.baselined` step output.
- **Bypassing Baseline:** Pass `--no-baseline` (or set `no_baseline: true` in the action) to evaluate the diff without grandfathering.
- **Custom Baseline Path:** Pass `--baseline-file <path>` (or set `baseline_file` in the action).

### 3. Ratchet Protection & Technical Debt Burndown

1. **Ratchet Against Growth:** The baseline is protected by the `config-integrity` gate. Attempting to add new findings to `discipline-baseline.toml` is classified as gate weakening and fails CI unless accompanied by a scoped directive:
   ```text
   allow-gate-weakening: baseline grandfathering legacy modules for migration
   ```
2. **Ratchet Down (Stale Entry Notes):** When a grandfathered finding is fixed in source code, Discipline reports the stale baseline entry as an informational note:
   ```text
   NOTE: baseline entry docs/old_plan.md (time-estimates:duration-estimate) is stale — violation resolved in source. Run `discipline baseline --write` to burn down baseline debt.
   ```
   Maintainers can re-run `discipline baseline --write` to remove the stale entry and lock in the improvement without needing an override.

---

## Recipes & Monorepo Setup

Discipline natively supports polyglot monorepos and multi-tier architectures using path scoping, vocabulary extension, and command gate presets.

### Rollup Skip-Set Check (`ci-skip-set`)

A path-filtered matrix makes its rollup job count `skipped` as passing. The [`ci-skip-set`](GATES.md#ci-skip-set) gate checks that every skip matches the job's own `if:` under the filter outputs the rollup observed, so an all-false filter evaluation cannot render as a green rollup over a run that verified nothing.

The input is runtime data that exists only inside the rollup job, so the workflow supplies it and the binary stays offline:

| Surface | Value |
|---|---|
| `DISCIPLINE_CI_CONTEXT` | `${{ toJson(needs) }}` of the rollup job: inline JSON, or a path to a file holding it. Unset or empty: the gate reports `not evaluated` and examines nothing. |
| Action input `ci_context` | Passed to the binary as `DISCIPLINE_CI_CONTEXT` through `env:`. |
| `GITHUB_EVENT_NAME`, `GITHUB_REF`, `GITHUB_REF_NAME`, `GITHUB_REF_TYPE`, `GITHUB_BASE_REF`, `GITHUB_HEAD_REF`, `GITHUB_REPOSITORY`, `GITHUB_REPOSITORY_OWNER` | Read for `github.*` terms in an `if:`. The runner sets them; an unset one makes a term that reads it unverifiable. |

The rollup must list the change-detection job in its `needs`, because the filter outputs are read from `needs.<change_job>.outputs`. Configure the gate once:

```toml
[gates.ci-skip-set]
workflow = ".github/workflows/ci.yml"   # the file this rollup runs in
change_job = "detect-changes"            # "" when the workflow has none
unconditional_jobs = ["detect-changes", "docs-lint"]
```

Copy-paste rollup job. Every value reaches the shell through `env:`; nothing is interpolated into `run:`.

```yaml
  ci-gate:
    if: always()
    needs: [detect-changes, docs-lint, lint, test]
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - name: No required job failed
        shell: bash
        env:
          NEEDS: ${{ toJson(needs) }}
        run: |
          set -euo pipefail
          bad="$(echo "${NEEDS}" | jq -r '[to_entries[] | select(.value.result != "success" and .value.result != "skipped") | .key] | join(", ")')"
          [ -z "${bad}" ] || { echo "::error::not successful: ${bad}"; exit 1; }
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0 # the other integrity gates diff against the base
      - name: Skip set matches each job's if
        uses: orieg/discipline@v0
        with:
          suite: integrity
          ci_context: ${{ toJson(needs) }}
```

With the standalone binary instead of the action:

```yaml
      - name: Skip set matches each job's if
        env:
          DISCIPLINE_CI_CONTEXT: ${{ toJson(needs) }}
        run: discipline check --suite integrity
```

To verify the gate evaluated rather than passed on a missing context, assert its `examined` count in the JSON report: `jq -e '.outcomes[] | select(.gate == "ci-skip-set") | .examined > 0' report.json`.

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
