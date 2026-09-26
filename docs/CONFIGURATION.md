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
| **5. Secret denylist** | `DISCIPLINE_HOSTNAME_DENYLIST` (legacy fallback `DOCS_HOSTNAME_DENYLIST`), action input `hostname_denylist` | Highest | Sensitive hostnames that must not appear even in repository config. |

Two `check` switches apply after layer 5: `--directive-sources` (`DISCIPLINE_DIRECTIVE_SOURCES`, action input `directive_sources`) replaces `directives.sources`, and `--fail-on-overrides` can only turn `fail_on_overrides` on.

### Merge Rules & Asymmetric List Resets

`discipline.toml` is read over the built-in defaults key by key: a key it leaves out keeps its default, and a list it sets replaces the built-in list (`config-integrity` reports the edit). From layer 3 up, values merge onto the file's as TOML under strict typing (F6); a list key the file left out is inserted whole, so it replaces the built-in default:
- **Tables** merge recursively key by key.
- **Scalars** replace previous values.
- **Tightening lists** (`hostname_denylist`, `extra_patterns`, `paths`, `include`, `deny_dependencies`, `manifests`, ...): append-only; a reset marker on them is ignored.
- **Loosening lists** (`exempt_paths`, `allow_patterns`, `allowed_users`, `assert_helper_fns`, `extra_assert_macros`, `allow_dependencies`, `directives.sources`): append by default; a reset clears the lower layers' list first. Reset works only in `--config-override` / `config_override`; in `discipline.toml` the table form fails schema validation.
  ```toml
  [gates.pii]
  exempt_paths = { reset = true, items = ["tests/legacy/**"] }
  # or the sentinel form: exempt_paths = ["__reset__", "tests/legacy/**"]
  ```

---

## Configuration Schema

Discipline deserializes `discipline.toml` strictly: an unknown key, an unknown or unshipped gate id, or a malformed entry is exit `2`. `discipline schema` prints a JSON Schema (draft 2020-12) of the same shape for editors.

<!-- generated:config-schema -->
| Section / Key | Type | Default | Description |
|---|---|---|---|
| `directives.allow_hidden` | boolean | `false` | Allow directives hidden inside HTML comments &lt;!-- --&gt; (default: false) |
| `directives.allowed_override_actors` | list | `[]` | Actors authorized to apply overrides even when fail_on_overrides is true, and the reviewers whose approval require_approval accepts (default: []) |
| `directives.degrade_offline` | boolean | `true` | On a push event, continue with a named note when the merged-pr-body lookup fails, the finding it might have lifted standing, instead of stopping with exit 2 (default: true; set false to make the review record a hard requirement) |
| `directives.fail_on_overrides` | boolean | `false` | Treat applied overrides as failures requiring human sign-off (default: false) |
| `directives.max_overrides` | integer | *(per entry)* | Most PR-body / commit-body overrides one change may apply; inline markers are not counted (default: unset, no cap) |
| `directives.require_approval` | boolean | `false` | PR-body / commit-body overrides fail the run until the forge shows an approving review of the head commit by an allowed_override_actors member other than the author (default: false) |
| `directives.sources` | list | `["pr-body","commits","merged-pr-body"]` | Allowed directive sources: pr-body, commits, merged-pr-body (default: ["pr-body", "commits", "merged-pr-body"]). merged-pr-body reads, on a push event, the body of the merged pull request each pushed commit arrived through |
| `gates.agent-scratch.enabled` | boolean | `true` | Whether this gate is active |
| `gates.agent-scratch.exempt_paths` | list | *(4 entries)* | File path globs exempted from this gate |
| `gates.agent-scratch.paths` | list | *(7 entries)* | Directory and file globs that must never be tracked |
| `gates.agent-scratch.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.agents-md.enabled` | boolean | `true` | Whether this gate is active |
| `gates.agents-md.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.agents-md.severity` | string | `"warning"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.archive-contents.archive_path` | string | *(unset)* | Glob pattern matching the built archive file |
| `gates.archive-contents.enabled` | boolean | `false` | Whether this gate is active |
| `gates.archive-contents.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.archive-contents.forbidden_patterns` | list | `[]` | Regex patterns forbidden inside the archive |
| `gates.archive-contents.max_entry_bytes` | integer | `16777216` | Entries larger than this many bytes are not scanned and are named in a note (default 16 MiB) |
| `gates.archive-contents.preset` | string | *(unset)* | Named forbidden_patterns list merged with forbidden_patterns; no-source also turns scan_contents on |
| `gates.archive-contents.required_paths` | list | `[]` | Files required to exist inside the archive |
| `gates.archive-contents.scan_contents` | boolean | `false` | Read each entry and report source maps whose sourcesContent embeds the original source, as .map entries or inline base64 sourceMappingURL comments |
| `gates.archive-contents.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.archive-contents.strip_components` | integer | `0` | Leading directory components to strip from archive paths |
| `gates.assertion-reduction.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |
| `gates.assertion-reduction.enabled` | boolean | `true` | Whether this gate is active |
| `gates.assertion-reduction.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.assertion-reduction.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions (a trailing `!` is optional) |
| `gates.assertion-reduction.min_assertions_per_test` | integer | *(unset)* | Minimum assertions required per test method |
| `gates.assertion-reduction.mock_assert_fns` | list | `[]` | Callee fragments that assert on a test double's interactions, beyond the built-in vocabulary |
| `gates.assertion-reduction.mock_setup_fns` | list | `[]` | Callee fragments that construct or program a test double, beyond the built-in vocabulary |
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
| `gates.build-hooks.enabled` | boolean | `true` | Whether this gate is active |
| `gates.build-hooks.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.build-hooks.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
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
| `gates.ci-integrity.workflows` | list | *(9 entries)* | Workflow file patterns to inspect |
| `gates.ci-skip-set.change_job` | string | `"detect-changes"` | Change-detection job whose outputs gate the conditional jobs; it must have succeeded. Empty string = no such job |
| `gates.ci-skip-set.enabled` | boolean | `true` | Whether this gate is active |
| `gates.ci-skip-set.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.ci-skip-set.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.ci-skip-set.unconditional_jobs` | list | `[]` | Jobs that must never be skipped, whatever their dependencies did |
| `gates.ci-skip-set.workflow` | string | `".github/workflows/ci.yml"` | Repo-relative path of the workflow whose rollup job supplies the runtime needs context (DISCIPLINE_CI_CONTEXT). Left at the default, the running workflow (GITHUB_WORKFLOW_REF) or the first ci.yml under .github/, .gitea/ or .forgejo/workflows/ is used |
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
| `gates.commit-provenance.agent_markers` | list | *(12 entries)* | Substrings of a trailer line, author name or author email that identify an agent-produced commit |
| `gates.commit-provenance.enabled` | boolean | `false` | Whether this gate is active |
| `gates.commit-provenance.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.commit-provenance.required_trailers` | list | `[]` | Trailer keys every commit in the change must carry |
| `gates.commit-provenance.review_trailer` | string | `"Reviewed-by"` | Trailer an agent-produced commit must carry, naming someone other than its author; empty switches the rule off (default: Reviewed-by) |
| `gates.commit-provenance.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
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
| `gates.error-swallowing.enabled` | boolean | `true` | Whether this gate is active |
| `gates.error-swallowing.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.error-swallowing.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.golden-output.enabled` | boolean | `true` | Whether this gate is active |
| `gates.golden-output.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.golden-output.paths` | list | *(8 entries)* | Committed golden/snapshot globs whose edits require a directive |
| `gates.golden-output.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.ignored-tests.approved_predicates` | list | `[]` | Conditional ignore predicates (e.g. miri) approved by policy |
| `gates.ignored-tests.enabled` | boolean | `true` | Whether this gate is active |
| `gates.ignored-tests.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.ignored-tests.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.instruction-smuggling.enabled` | boolean | `true` | Whether this gate is active |
| `gates.instruction-smuggling.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.instruction-smuggling.instruction_files` | list | `[]` | Globs of the repository's own agent-instruction files (a prompt an MCP server loads, a runtime context file), reported like AGENTS.md (default: []) |
| `gates.instruction-smuggling.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
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
| `gates.pii.redact_lan_ips` | boolean | `false` | Mask a matched LAN IP in the report instead of echoing it (default: false) |
| `gates.pii.scan_pr_body` | boolean | `true` | Whether to scan PR description text |
| `gates.pii.secrets` | boolean | `true` | Check for leaked private keys and high-entropy API tokens |
| `gates.pii.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.pr-checklist.enabled` | boolean | `false` | Whether this gate is active |
| `gates.pr-checklist.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.pr-checklist.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.provenance-tags.check_intervals` | boolean | `true` | Check published wall-clock ratios for confidence intervals or explicit qualifiers |
| `gates.provenance-tags.check_mechanisms` | boolean | `true` | Check for mechanism claims without hardware counter evidence or explicit hypothesis qualifiers |
| `gates.provenance-tags.check_paired_figures` | boolean | `true` | Check paired figures for shared workload IDs or differentiation tags |
| `gates.provenance-tags.check_pending_citations` | boolean | `false` | A pending-measurement statement must cite a tracking issue |
| `gates.provenance-tags.check_tables` | boolean | `true` | Check markdown tables for unit-bearing numbers without table or caption provenance tags |
| `gates.provenance-tags.deterministic_units` | list | `[]` | Units whose figures are deterministic and exempt from the interval requirement, added to the built-in list |
| `gates.provenance-tags.diff_only` | boolean | `false` | Judge only paragraphs that contain an added line (default: false, the whole changed file) |
| `gates.provenance-tags.enabled` | boolean | `false` | Whether this gate is active |
| `gates.provenance-tags.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.provenance-tags.pending_issue_repos` | list | `[]` | Other repositories (owner/name) whose issues a pending statement may cite; by default only this repository's issues count |
| `gates.provenance-tags.ratio_satisfied_by` | list | `[]` | What satisfies a published wall-clock ratio, replacing the built-in list when set: interval, marker:&lt;word&gt;, artifact:&lt;glob&gt;, regex:&lt;pattern&gt; (paragraph-scoped) |
| `gates.provenance-tags.require_open_pending_issues` | boolean | `false` | A pending-measurement statement must cite at least one open issue, read from the forge (gh on GitHub, curl on GitLab, Gitea and Forgejo); implies check_pending_citations |
| `gates.provenance-tags.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.provenance-tags.superseded_json_paths` | list | `[]` | Globs of tracked JSON datasets swept for registered figures |
| `gates.provenance-tags.superseded_registry` | string | *(unset)* | Path (read at HEAD) of a JSON registry of withdrawn figures; a registered figure may be republished only next to a retraction marker |
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
| `gates.stub-bodies.enabled` | boolean | `true` | Whether this gate is active |
| `gates.stub-bodies.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.stub-bodies.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
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
| `gates.toolchain-config.enabled` | boolean | `true` | Whether this gate is active |
| `gates.toolchain-config.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.toolchain-config.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
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
| `gates.vacuous-tests.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions (a trailing `!` is optional) |
| `gates.vacuous-tests.min_assertions_per_test` | integer | *(unset)* | Minimum assertions required per test method |
| `gates.vacuous-tests.mock_assert_fns` | list | `[]` | Callee fragments that assert on a test double's interactions, beyond the built-in vocabulary |
| `gates.vacuous-tests.mock_setup_fns` | list | `[]` | Callee fragments that construct or program a test double, beyond the built-in vocabulary |
| `gates.vacuous-tests.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `gates.version-lockstep.enabled` | boolean | `false` | Whether this gate is active |
| `gates.version-lockstep.exempt_paths` | list | `[]` | File path globs exempted from this gate |
| `gates.version-lockstep.groups` | array of tables | `[]` | Groups of sources that must declare identical version strings |
| `gates.version-lockstep.groups[].name` | string | *(required)* | Name of the version lockstep group |
| `gates.version-lockstep.groups[].sources` | array of tables | *(required)* | Files and capture regexes whose versions must match |
| `gates.version-lockstep.groups[].sources[].path` | string | *(required)* | Source file path |
| `gates.version-lockstep.groups[].sources[].regex` | string | *(required)* | Regex pattern capturing the version string in group 1 |
| `gates.version-lockstep.severity` | string | `"error"` | Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational). |
| `languages.c.function_macros` | list | `[]` | Macros that expand to a function head, e.g. MYEXT_METHOD(Class, name) { ... } (default: []) |
| `languages.c.macros` | list | `[]` | Macros blanked with their arguments: statement or declaration macros written without a semicolon, list entries without a comma, attribute-like prefixes (default: []) |
| `meta.description` | string | *(per entry)* | Optional short description of the project |
| `meta.mode` | string | `"enforcing"` | Operating mode: 'enforcing' exits non-zero on violations; 'advisory' runs all checks and emits reports but exits 0. |
| `meta.name` | string | *(required)* | Repository or project name |
| `meta.version` | integer | `1` | Configuration schema version (must be 1) |
| `tests.functions` | list | `[]` | Function names (leaf) that are test entry points wherever they appear, e.g. a script's self_test (default: []) |
| `tests.paths` | list | `[]` | Path globs whose every line is test scope (default: []) |
<!-- /generated -->

---

## Action Reference

The composite action (`action.yml`) runs identically in GitHub Actions, Gitea Actions, and Forgejo Actions. It operates with zero Node.js runtime overhead, executing entirely via shell and the static binary.

The action runs on `pull_request`, `merge_group` and `push` events (the base is `github.base_ref`, the merge group's base, or `github.event.before`). On `push` there is no pull request body to read directives from; see [Override Directives](#override-directives) for what each event sees and why a squash or rebase merge drops PR-body waivers.

### Action Inputs

<!-- generated:action-inputs -->
| Input | Default | Description |
|---|---|---|
| `config` | `discipline.toml` | Path to discipline.toml. When the default discipline.toml is absent, built-in defaults apply (each gate at its built-in default enablement and severity); any other path that does not exist is an error (exit 2). |
| `suite` | `all` | Suite to run: all, agent-guard, hygiene, integrity, quality, verification, bench |
| `base_ref` | *(none)* | Branch or commit the change is measured against. Default: PR base branch, else the pushed-from commit, else the default branch. |
| `enable` | *(none)* | Gate ids to force on (comma or newline separated). See `discipline gates`. |
| `disable` | *(none)* | Gate ids to force off (comma or newline separated), e.g. "time-estimates, pii". |
| `config_override` | *(none)* | Inline TOML merged over discipline.toml: tables merge, lists append, scalars replace. |
| `hostname_denylist` | *(none)* | Hostnames the pii gate must reject (comma or newline separated). Pass a secret; matches are never echoed. |
| `fail_on_warnings` | `false` | Treat warnings as failures. |
| `fail_on_overrides` | `false` | Treat applied overrides as failures (requires human sign-off). |
| `advisory` | `false` | Advisory mode: run all checks and emit reports, but exit code 0 even if violations occur. |
| `comment` | `false` | Post the report as one pull-request comment, edited on every run. Needs `pull-requests: write` (GitHub) or a token that can comment; a fork's read-only token is reported, not failed. |
| `token` | `${{ github.token }}` | Token used to post the comment. Passed to discipline only when `comment` is true. |
| `policy_from` | `head` | Which side's discipline.toml judges the change: 'head' (the change's own copy) or 'base' (the base ref's, so a policy edit takes effect once merged; config-integrity still reports it). |
| `actor` | `${{ github.event.pull_request.user.login || github.actor }}` | Login judged against allowed_override_actors. Default: the pull request author (github.event.pull_request.user.login), which the server sets; otherwise github.actor or the forge equivalent. The triggering login is not used on a pull request, since whoever edits the description must not be able to authorize their own override. |
| `directive_sources` | *(none)* | Comma-separated list of allowed directive sources (pr-body, commits, merged-pr-body). merged-pr-body reads, on a push event, the body of the merged pull request each pushed commit arrived through. |
| `pr_body` | `${{ github.event.pull_request.body }}` | PR description: carries override directives and is itself scanned by hygiene gates. |
| `pr_title` | `${{ github.event.pull_request.title }}` | PR title: checked by hygiene gates (e.g. issue-link). |
| `working_directory` | `.` | Directory of the repository to check. |
| `version` | *(none)* | Release to download (e.g. v0.1.0). Default: the tag this action was referenced by (`@vX.Y.Z`); for a major tag, a commit SHA or a branch, the release in the action''s own Cargo.toml; the latest release only when neither is available. |
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
| `init` | Write a minimal discipline.toml: gates run at their built-in defaults; commented examples show what to change |
| `gates` | List every gate: id, suite, availability, and effective state |
| `schema` | Print the JSON Schema for discipline.toml |
| `self-test` | Run the embedded negative / positive controls against this binary |
| `completions` | Generate shell completion script to stdout (bash, zsh, fish, powershell, elvish) |
| `install-hooks` | Install pre-commit hook in the local git repository |
| `hook` | Run the gates inside a coding agent's edit loop (Claude Code, Codex, Cursor, Aider, Copilot CLI, agy, Qwen Code, OpenCode) |
| `explain` | Explain a gate: what it checks, its state here, and the directive that lifts a finding |
| `replay` | Replay the last N merged changes through a configuration: what it would have blocked |
| `mcp` | Serve the gates to an MCP client over stdio (read-only tools: check_diff, list_gates, explain_finding) |
| `bench` | Benchmark tooling for the bench-regression gate |
| `doctor` | Check that the repository and its platform enforce discipline: workflows, CODEOWNERS, branch protection. Exit 0 = healthy, 1 = a failing check, 2 = could not check |
<!-- /generated -->

### Options

Every option of every subcommand, generated from the binary's own definitions (`discipline <subcommand> --help` and `man discipline` show the same):

<!-- generated:cli-options -->

**`discipline check`**

| Option | Env | Default | Description |
|---|---|---|---|
| `-c`, `--config` | `DISCIPLINE_CONFIG` | `discipline.toml` | Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2) |
| `--config-override` | `DISCIPLINE_CONFIG_OVERRIDE` |  | Inline TOML merged over the file (tables merge, lists append, scalars replace) |
| `--enable` | `DISCIPLINE_ENABLE` |  | Gate ids to force on (comma separated) |
| `--disable` | `DISCIPLINE_DISABLE` |  | Gate ids to force off (comma separated) |
| `-s`, `--suite` |  | `all` | Which check suite to run |
| `-b`, `--base` | `DISCIPLINE_BASE_REF` |  | Base branch or commit to measure the change against (auto-detected in CI if omitted) |
| `--commit` |  |  | Specific commit to inspect, against its parent &lt;sha&gt;~1; it must be the commit checked out (exit 2 otherwise) |
| `--commit-range` |  |  | Commit range to inspect (&lt;before&gt;..&lt;after&gt; or &lt;before&gt;...&lt;after&gt;); &lt;after&gt;, when given, must be the commit checked out (exit 2 otherwise) |
| `--staged` |  |  | Inspect the index against HEAD instead (pre-commit hook mode) |
| `--pr-body-file` |  |  | File holding the PR body or commit message (override directives, hygiene scanning). Falls back to the PR_BODY environment variable |
| `--pr-title` | `PR_TITLE` |  | PR title for PR-level hygiene checks (e.g. issue-link). Falls back to the PR_TITLE environment variable |
| `--fail-on-warnings` | `DISCIPLINE_FAIL_ON_WARNINGS` |  | Treat warnings as failures |
| `--fail-on-overrides` | `DISCIPLINE_FAIL_ON_OVERRIDES` |  | Treat applied overrides as failures (requires human sign-off) |
| `--advisory` | `DISCIPLINE_ADVISORY` |  | Advisory mode: run all checks and emit reports, but exit code 0 even if violations occur |
| `--comment` | `DISCIPLINE_COMMENT` |  | Post the report as one pull-request comment, edited on every run (needs a token that can write comments; off by default) |
| `--policy-from` | `DISCIPLINE_POLICY_FROM` | `head` | Which side's discipline.toml judges the change. `base` reads it from the base ref, so a policy edit takes effect once merged; `config-integrity` still reports it |
| `--actor` | `DISCIPLINE_ACTOR` |  | Actor executing the check (for actor-aware override authorization). Falls back to DISCIPLINE_ACTOR, GITHUB_ACTOR, GITEA_ACTOR, FORGEJO_ACTOR, GITLAB_USER_LOGIN |
| `--directive-sources` | `DISCIPLINE_DIRECTIVE_SOURCES` |  | Comma-separated list of allowed directive sources (pr-body, commits, merged-pr-body) |
| `--trust-workspace` |  |  | Trust the workspace and disable libgit2 repository owner validation (off by default, or set DISCIPLINE_TRUST_WORKSPACE=1) |
| `-q`, `--quiet` |  |  | Suppress output on success (only print output when violations are found) |
| `-f`, `--format` |  | `terminal` | Output format |
| `--json-out` |  |  | Also write the JSON report to this path, whatever --format is |
| `-o`, `--output-file` |  |  | Write the formatted report to this path |
| `--report-gitlab` | `DISCIPLINE_REPORT_GITLAB` |  | Write GitLab Code Quality JSON report to this path |
| `--report-junit` | `DISCIPLINE_REPORT_JUNIT` |  | Write JUnit XML report to this path |
| `--report-sarif` | `DISCIPLINE_REPORT_SARIF` |  | Write SARIF report to this path |
| `--bench-provenance` | `DISCIPLINE_BENCH_PROVENANCE` |  | Expected host or runner provenance tag for benchmark artifacts |
| `--allow-cross-host-bench` | `DISCIPLINE_ALLOW_CROSS_HOST_BENCH` |  | Allow benchmark comparison across mismatched host/runner provenance tags |
| `--bench-base-file` | `DISCIPLINE_BENCH_BASE_FILE` |  | In-job base benchmark result file for bench-regression dual-mode |
| `--bench-head-file` | `DISCIPLINE_BENCH_HEAD_FILE` |  | In-job head benchmark result file for bench-regression dual-mode |
| `--baseline-file` |  |  | Path to grandfathering baseline file (defaults to discipline-baseline.toml if present) |
| `--no-baseline` |  |  | Ignore grandfathering baseline even if present |

**`discipline diff`**

| Option | Env | Default | Description |
|---|---|---|---|
| `-c`, `--config` | `DISCIPLINE_CONFIG` | `discipline.toml` | Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2) |
| `--config-override` | `DISCIPLINE_CONFIG_OVERRIDE` |  | Inline TOML merged over the file (tables merge, lists append, scalars replace) |
| `--enable` | `DISCIPLINE_ENABLE` |  | Gate ids to force on (comma separated) |
| `--disable` | `DISCIPLINE_DISABLE` |  | Gate ids to force off (comma separated) |
| `-s`, `--suite` |  | `all` | Which check suite to run |
| `-b`, `--base` |  |  | Base branch or commit ref to compare against (defaults to HEAD for uncommitted changes) |
| `-f`, `--format` |  | `terminal` | Output format |
| `--json-out` |  |  | Also write the JSON report to this path, whatever --format is |
| `-o`, `--output-file` |  |  | Write the formatted report to this path |
| `--report-gitlab` | `DISCIPLINE_REPORT_GITLAB` |  | Write GitLab Code Quality JSON report to this path |
| `--report-junit` | `DISCIPLINE_REPORT_JUNIT` |  | Write JUnit XML report to this path |
| `--report-sarif` | `DISCIPLINE_REPORT_SARIF` |  | Write SARIF report to this path |
| `--baseline-file` |  |  | Path to grandfathering baseline file (defaults to discipline-baseline.toml if present) |
| `--no-baseline` |  |  | Ignore grandfathering baseline even if present |
| `--trust-workspace` |  |  | Trust the workspace and disable libgit2 repository owner validation (off by default, or set DISCIPLINE_TRUST_WORKSPACE=1) |
| `--advisory` | `DISCIPLINE_ADVISORY` |  | Advisory mode: run checks and emit reports, but exit 0 even if violations are found |

**`discipline baseline`**

| Option | Env | Default | Description |
|---|---|---|---|
| `-c`, `--config` | `DISCIPLINE_CONFIG` | `discipline.toml` | Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2) |
| `--config-override` | `DISCIPLINE_CONFIG_OVERRIDE` |  | Inline TOML merged over the file (tables merge, lists append, scalars replace) |
| `--enable` | `DISCIPLINE_ENABLE` |  | Gate ids to force on (comma separated) |
| `--disable` | `DISCIPLINE_DISABLE` |  | Gate ids to force off (comma separated) |
| `--write` |  |  | Record current findings to the baseline file |
| `--migrate` |  |  | Rewrite a fingerprint-version-1 baseline to version 2: every entry a current finding still matches is kept under its finding code, and stale entries are dropped. Commit the result in a change of its own, which `config-integrity` accepts without a directive |
| `--baseline-file` |  | `discipline-baseline.toml` | Path to grandfathering baseline file (defaults to discipline-baseline.toml) |
| `-b`, `--base` | `DISCIPLINE_BASE_REF` |  | Base branch or commit ref to compare against |
| `-s`, `--suite` |  | `all` | Specific suite to run: all, agent-guard, hygiene, integrity ... |
| `--whole-tree` |  |  | Record every pre-existing finding in the tree, not just the diff. Use when adopting discipline on an existing repository; conflicts with --base |
| `--all-severities` |  |  | Also record warnings and notes. By default only findings that would block under the current configuration are recorded: `error`, plus `warning` under --fail-on-warnings |
| `--fail-on-warnings` | `DISCIPLINE_FAIL_ON_WARNINGS` |  | Treat warnings as blocking when choosing what to record (same switch as `check --fail-on-warnings`) |
| `--trust-workspace` |  |  | Trust the workspace and disable libgit2 repository owner validation |

**`discipline init`**

| Option | Env | Default | Description |
|---|---|---|---|
| `-n`, `--name` |  |  | Name of the project (defaults to current directory name) |

**`discipline gates`**

| Option | Env | Default | Description |
|---|---|---|---|
| `-c`, `--config` | `DISCIPLINE_CONFIG` | `discipline.toml` | Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2) |
| `--config-override` | `DISCIPLINE_CONFIG_OVERRIDE` |  | Inline TOML merged over the file (tables merge, lists append, scalars replace) |
| `--enable` | `DISCIPLINE_ENABLE` |  | Gate ids to force on (comma separated) |
| `--disable` | `DISCIPLINE_DISABLE` |  | Gate ids to force off (comma separated) |

**`discipline completions`**

| Option | Env | Default | Description |
|---|---|---|---|
| `<SHELL>` |  |  | Target shell for completion script |

**`discipline install-hooks`**

| Option | Env | Default | Description |
|---|---|---|---|
| `-f`, `--force` |  |  | Overwrite existing pre-commit hook if present |

**`discipline hook run`**

| Option | Env | Default | Description |
|---|---|---|---|
| `--agent` |  |  | The agent whose hook contract to answer in |
| `-b`, `--base` |  |  | Base to measure the change against (default: the merge base with origin's default branch, else main / master) |

**`discipline hook install`**

| Option | Env | Default | Description |
|---|---|---|---|
| `--agent` |  |  | The agent to configure |

**`discipline explain`**

| Option | Env | Default | Description |
|---|---|---|---|
| `<QUERY>` |  |  | A gate id (`assertion-reduction`) or a finding line naming `[gate-id]` |
| `-c`, `--config` | `DISCIPLINE_CONFIG` | `discipline.toml` | Path to discipline.toml. An absent default file = built-in defaults (`discipline gates` lists them); any other path that does not exist is an error (exit 2) |
| `--config-override` | `DISCIPLINE_CONFIG_OVERRIDE` |  | Inline TOML merged over the file (tables merge, lists append, scalars replace) |
| `--enable` | `DISCIPLINE_ENABLE` |  | Gate ids to force on (comma separated) |
| `--disable` | `DISCIPLINE_DISABLE` |  | Gate ids to force off (comma separated) |

**`discipline replay`**

| Option | Env | Default | Description |
|---|---|---|---|
| `--last` |  |  | Number of first-parent commits (merged changes) to replay, newest first |
| `--ref` |  |  | Branch whose history is replayed (default: origin's default branch, else main / master) |
| `-c`, `--config` |  |  | Configuration under test (default: discipline.toml in the working tree) |
| `--json` |  |  | Print the summary as JSON |

**`discipline bench derive`**

| Option | Env | Default | Description |
|---|---|---|---|
| `<RUNS>` |  |  | `discipline-bench-ratio/v1` run files of the same code (at least two) |
| `--baseline` |  |  | Merge the derived platform entry into this ratio baseline file (created if absent) |
| `--allow-mixed-commits` |  |  | Accept runs of different commits (recorded in the baseline); only when the differences cannot move a number |
| `--ceiling-pct` |  | `50` | Derived cell floors above this percentage are reported but not gated |

**`discipline doctor`**

| Option | Env | Default | Description |
|---|---|---|---|
| `--branch` |  |  | Branch whose protection is checked (default: the repository's default branch) |
| `--repo` |  |  | Repository path on the forge, e.g. OWNER/NAME (default: from the CI environment or the `origin` remote; set DISCIPLINE_FORGE for a self-hosted forge) |
| `--local-only` |  |  | Check only local files; skip the platform API |
| `--strict` |  |  | Treat warnings as failures |
| `-f`, `--format` |  | `text` | Output format |
<!-- /generated -->

### Exit Codes

| Code | Status | Meaning |
|---|---|---|
| `0` | **Pass** | Every enabled gate ran and detected no blocking violations; also a run with violations in advisory mode (`--advisory`, or `meta.mode = "advisory"` once merged). |
| `1` | **Violations** | Gate violations found (blocking errors, or warnings under `--fail-on-warnings`), an applied override under `fail_on_overrides`, or a `max_overrides` / `require_approval` refusal (`policy_failures`). |
| `2` | **Could not check** | Engine failed to check: missing repository, unresolvable base ref, shallow clone with unreachable merge base, unreadable configuration, or syntax errors. |

---

## Override Directives

Legitimate test refactorings, file deletions, or configuration adjustments are authorized through scoped directives in the PR description or commit messages. Directives never apply globally: they must name the exact subject they cover.

**Which sources each event sees.** A `pull_request` run reads the pull request's body and the branch's commit messages. A `push` run (and `--commit` / `--commit-range`) reads only the pushed commits' messages: there is no pull request in its payload. A squash merge builds the commit message from the branch's commits, a rebase merge keeps them as they were, and a merge commit's default message carries neither — so a waiver written only in the PR body is not seen by the push run on the default branch that follows the merge, and that run fails on a change the pull request had already passed. Options, in the order to prefer them: gate on `pull_request` (the event whose body is the review record) and do not run the gate on `push` to the default branch; or put the directive in a commit message as well; or keep the `merged-pr-body` source (on by default) and give the push run a token that can read pull requests (`GH_TOKEN` or `DISCIPLINE_FORGE_TOKEN` in the step's `env:`; the action passes its `token` input only when `comment` is true): it resolves each pushed commit's merged pull request through the forge and reads its body under the same trust rules, and the gate notes name the pull request. When that lookup fails (forge unreachable, token without permission — a least-privilege `contents: read` token cannot read pull requests on a private repository), the run continues with a named note and the finding the body might have lifted stands (`directives.degrade_offline`, default `true`: fail-closed, never a silent pass); set it to `false` to stop with `could not check` (exit 2) naming the commit when the review record must be readable. `DISCIPLINE_NO_NETWORK` and an unidentifiable forge are notes. On a push, a finding that a PR-body directive would have lifted says so in its remediation and in the gate's notes (naming the merged pull request whose body was read, when one was), and `doctor` reports a workflow that runs the gate on `push` to a default branch whose merge method allows squash or rebase.

| Event | Sources read (`directives.sources` default `["pr-body", "commits", "merged-pr-body"]`) |
|---|---|
| `pull_request` | PR body (`pr-body`), branch commit messages (`commits`) |
| `merge_group` | the queued commits' messages (`commits`); the payload has no PR body |
| `push` to any branch | pushed commit messages (`commits`); with `merged-pr-body` (on by default), the body of the merged pull request each pushed commit arrived through, resolved through the forge (GitHub `commits/{sha}/pulls`, Gitea / Forgejo `commits/{sha}/pull`, GitLab `repository/commits/:sha/merge_requests`) and trusted like a PR body: line-anchored, `allow_hidden`, scoped subjects, `allowed_override_actors` against the pull request's author, `max_overrides`, `require_approval` (with that pull request as the context when exactly one was merged). A direct push has none. A push of more than 20 commits looks up none and says so in a note. |
| `--commit`, `--commit-range` (local) | the named commits' messages (`commits`) only |
| `--staged` (local) | `--pr-body-file` if given, else none; staged changes have no commits |

### Syntax & Grammar

```text
<directive>: <subject> <reason>
```

Alternatively, namespaced or HTML-comment syntax is accepted (`discipline:allow(<gate-id>)` takes a colon or a space before the subject); the HTML-comment form counts only with `directives.allow_hidden = true` (for `removes:`, also `gates.deletion-rationale.allow_hidden = true`):
```text
discipline: <directive>: <subject> <reason>
<!-- discipline:allow(<gate-id>): <subject> <reason> -->
```

Directives must begin on their own line. Mentions mid-sentence or inside markdown tables never arm the directive (F8). The text after the colon must name the subject and must not be empty or a bare placeholder (`TODO`, `tbd`, `none`, `n/a`, `...`, `<reason>`). A subject with no reason is accepted, so write the reason for the reviewer. Lines inside fenced (```` ``` ```` / `~~~`) code blocks are ignored; indented code blocks are not. A commit's subject line is never read.

| Directive | Lifts | Subject |
|---|---|---|
| `removes:` / `deletes:` / `discipline:allow(deletion-rationale)` / `allow(deletion-rationale)` | `deletion-rationale` | File path, directory prefix, or test function name (or unscoped with `require_scope = false`) |
| `allow-assertion-drop:` / `discipline:allow(assertion-reduction)` / `allow(assertion-reduction)` | `assertion-reduction` | Test function name, file path, or directory prefix |
| `allow-ignore:` / `discipline:allow(ignored-tests)` / `allow(ignored-tests)` | `ignored-tests` | Test function name |
| `allow-gate-weakening:` / `discipline:allow(config-integrity)` / `allow(config-integrity)` | `config-integrity`; with subject `ci-integrity` or `test-floor`, that gate too | Gate id, or `directives`, `tests`, `languages`, `meta`, `baseline` |
| `allow-golden-update:` / `discipline:allow(golden-output)` / `allow(golden-output)` | `golden-output` | Snapshot/fixture file path or directory prefix |
| `allow-toolchain-weakening:` / `discipline:allow(toolchain-config)` / `allow(toolchain-config)` | `toolchain-config` | Option key path (`compilerOptions.strict`), its last segment, or the configuration file path |
| `allow-stub:` / `discipline:allow(stub-bodies)` / `allow(stub-bodies)` | `stub-bodies` | Function name, or the file path |
| `allow-swallow:` / `discipline:allow(error-swallowing)` / `allow(error-swallowing)` | `error-swallowing` | File path, or `path:line` of the handler |
| `allow-agent-instructions:` / `discipline:allow(instruction-smuggling)` / `allow(instruction-smuggling)` | `instruction-smuggling` | File path, or `path:line` |
| `allow-commit-provenance:` / `discipline:allow(commit-provenance)` / `allow(commit-provenance)` | `commit-provenance` | Commit SHA (7 or 40 characters) |
| `allow-build-hook:` / `discipline:allow(build-hooks)` / `allow(build-hooks)` | `build-hooks` | Hook name (`postinstall`) or file path |
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
| `allow-vacuous-test:` / `discipline:allow(vacuous-tests)` / `allow(vacuous-tests)` | `vacuous-tests` | Name of the new test the vacuous-tests finding is on |

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

`docs-lint: allow` exempts only `time-estimates` and `pii`. Where a gate counts the lines its inline markers exempted, the report shows the count (`inline_exemptions`); not every gate counts them (`suppression-delta` lists each marked line as an override instead).

---

## Declared test scope (`[tests]`)

Each language pack knows its conventions for test code (`#[test]`, pytest collection, `it(` callbacks, `tests/` directories). A repository can widen that:

```toml
[tests]
functions = ["self_test"]      # leaf function names that are test entry points wherever they appear
paths = ["scripts/fixtures/**"] # globs whose every line is test scope
```

`paths` is honoured by every language pack; `functions` by the Python and Rust packs and by `pii`. A declared name wins over the language's naming rules: `_self_test` is a test once listed, although Python treats a leading underscore as private. The gates that separate test code from production code read the declaration through the packs: in Python and Rust the assertion gates collect a declared function as a test and `error-swallowing`, `stub-bodies` and `suppression-delta` treat its body as test scope; in every language those three treat a declared file as test scope; `pii` does not report fixture strings inside either. Widening either list is reported by `config-integrity` as a weakening, including the first change that sets it: that change carries `allow-gate-weakening: tests <reason>` (the subject is `tests`, not a gate id). Growing `assert_helper_fns` in the same change needs one more line per gate, named by its id (`allow-gate-weakening: assertion-reduction <reason>`, `allow-gate-weakening: vacuous-tests <reason>`); see `config-integrity` in [GATES.md](GATES.md).

## C and C++ extension macros (`[languages.c]`)

tree-sitter parses C without expanding macros, so the idioms of a PHP, Python or Ruby C extension become parse error regions (`file `php_ext.c` had N skipped C/C++ parse error region(s)`), and the stubs, discards and assertions inside them go unread. Before parsing, the C and C++ packs rewrite the listed macros byte for byte: the file keeps its length and its newlines, so every finding's line is the original's.

- **Blanked** (`macros`): the name, and its balanced `(...)` when called, become spaces. For a statement or declaration macro written without a semicolon (`ZEND_PARSE_PARAMETERS_START(1, 1)`, `ZEND_DECLARE_MODULE_GLOBALS(ext)`), a table entry written without a comma (`PHP_ME(...)`, `PHP_FE_END`), or an attribute-like prefix (`PHPAPI`, `zend_always_inline`).
- **Function heads** (`function_macros`): `PHP_METHOD(Judy, size)` becomes `int Judy_size()` padded with spaces, so the body after it is a function the gates read, named `Judy_size` in findings (`PHP_MINIT_FUNCTION(ext)` is `minit_ext`).

Built in: the Zend argument-info, fast-parameter-parsing, module-globals, function-table, ini, hash-iteration and module-dependency macros, `PHP_FUNCTION` / `PHP_METHOD` / the module hooks (and their `ZEND_` forms); CPython `PyObject_HEAD`, `PyObject_HEAD_INIT` and the thread-state macros; the Ruby C API export markers (full list: `src/ast/c_macros.rs`). A repository adds its own:

```toml
[languages.c]
macros = ["MYEXT_GET_OBJECT", "MYEXT_API", "JSLN"]   # an entry ending in `*` is a prefix
function_macros = ["MYEXT_METHOD"]
```

The C pack also blanks, inside an `#ifdef __cplusplus` guard, the `extern "C" {` line and its lone `}` (same bytes, same newlines). A `.h` header the C grammar leaves with parse errors is re-read as C++, and the reading with fewer error regions is kept; a `.c` file is never re-read.

Comments, string literals and preprocessor lines are never rewritten. A blanked macro is code no gate reads, so adding an entry to either list is a `config-integrity` weakening (`allow-gate-weakening: languages <reason>`).

## Trust Model

Discipline distinguishes between **configurable** and **bypassable**:
0. **Who wrote the policy:** By default a change is judged by its own copy of `discipline.toml`. With `--policy-from base` (action input `policy_from: base`, env `DISCIPLINE_POLICY_FROM`) it is judged by the base ref's copy: a policy edit, looser or stricter, takes effect once merged, and `config-integrity` still reports the edit for review. A base ref without the file is judged by the built-in defaults, never by the change's copy. The change's own file must still parse (exit `2` otherwise). A `--config` given as an absolute path outside the repository is on neither side of the change, so `config-integrity` compares nothing for it, and under `--policy-from base` it is not used: the base ref's `discipline.toml` (else the built-in defaults) judges. A relative path that leaves the repository (`../outside.toml`) is exit `2`.
1. **Config integrity:** A pull request cannot weaken its own `discipline.toml` without triggering `config-integrity`. If an agent disables a gate or grows an exemption list, the PR is rejected unless an authorized `allow-gate-weakening:` directive is present.
2. **Directive channel enforcement:** Directives are parsed exclusively from trusted channels specified in `directives.sources` (defaulting to `["pr-body", "commits", "merged-pr-body"]`).
3. **Hidden directive policy:** By default, HTML comment-wrapped directives in PR bodies, commit messages and merged PR bodies are ignored (`directives.allow_hidden = false`; `gates.deletion-rationale.allow_hidden` can admit them for deletions) to ensure reviewers see all requested waivers.
4. **Machine gate for human sign-off:** When `directives.fail_on_overrides = true` (or `--fail-on-overrides`), any applied override causes Discipline to exit `1`, requiring an authorized human approver to bypass or merge.
   A directive in the PR body or a commit body is written by the author of the change it excuses. Two options sit between accepting every such override and refusing them all; both count directive overrides only (inline `discipline:allow(...)` markers are part of the reviewed tree):
   - `directives.max_overrides = N`: a change applying more than `N` fails, with the refusal listed under `policy_failures` in the JSON report.
   - `directives.require_approval = true`: directive overrides fail the run until the forge shows an **approving review of the pull request's head commit** by a login in `allowed_override_actors` **other than the pull request's author**. An approval of an earlier commit does not count, and a later "changes requested" by the same reviewer withdraws it. The pull request is read from the Actions event payload (GitHub, Gitea, Forgejo) or, on GitLab, from `CI_MERGE_REQUEST_IID` and `CI_MERGE_REQUEST_SOURCE_BRANCH_SHA`, and the reviews from the forge API (read-only token; on GitLab the merge request's `sha` must be the head being checked and the author is never an approver). When a directive override applies, no pull-request context, a forge that cannot be identified or reached, `DISCIPLINE_NO_NETWORK=1`, or an empty `allowed_override_actors` is exit `2`, never a pass. Re-run the check after the review (trigger the workflow on `pull_request_review`), since the first run precedes it.
   Raising or removing `max_overrides`, switching `require_approval` off, and growing `allowed_override_actors` are themselves weakenings reported by `config-integrity`.
5. **Residual gap:** Workflow files (`.github/workflows/*.yml`) are evaluated by CI from the PR head commit; an agent could conceivably edit the workflow step to pass `disable: ...` or `advisory: true`. The `ci-integrity` gate catches the common forms of this in modified workflows: those two inputs, masked failures (`continue-on-error`, `|| true`, `set +e`), unpinned actions, deleted verification steps, and a rollup job whose `needs` no longer covers every verification job. Repositories should still protect workflow files and `discipline.toml` with `CODEOWNERS` and branch protection, because a workflow can be rewritten in ways no static check anticipates; see [Repository Protection](guides/ci-platforms.md#8-repository-protection).

---

## Adoption Configurations

Reference configurations measured on real consumer repositories. Paths are illustrative; adapt them to your layout.

### C Extension with a PHP Runtime

Measured residue against merge base `HEAD~30` with unconfigured defaults on a reference consumer:
- `time-estimates`: 1 violation (a benchmark document describing a historical runtime duration).
- `assertion-reduction`: 1 violation (a newly added PHP test fixture that intentionally contains NUL bytes).
- `pii`: 1 violation (an example script whose sample data is a private LAN address).
- `agents-md`: 1 violation (a `CLAUDE.md` that diverged from `AGENTS.md` instead of symlinking it).
- 5 informational warnings (Zend engine C preprocessor macros that the C grammar cannot fully parse). The built-in Zend macro list now reads those; the extension's own statement macros go in `[languages.c] macros`, see [C and C++ extension macros](#c-and-c-extension-macros-languagesc).

Minimal configuration:

```toml
[meta]
version = 1
name = "example-ext"

[gates.pii]
# Example script whose sample data is a private LAN address
exempt_paths = [
    "examples/ip-range-lookup.php",
]

[gates.time-estimates]
# Benchmark documentation references historical runtime durations
exempt_paths = [
    "BENCHMARK.md",
]

[gates.assertion-reduction]
# Binary-safe string test intentionally contains NUL bytes
exempt_paths = [
    "tests/binary_safe_005.phpt",
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

A repository that runs the default `ci-integrity` gate reports a tag ref (`@v0`, `@v0.13.1`) as an unpinned action. Pin the action to a commit SHA (`uses: orieg/discipline@<commit-sha> # v0.13.1`): a SHA ref runs the binary of the release that commit's `Cargo.toml` names, and `version:` picks another release.

### GitLab CI/CD

Include the remote pipeline template directly:

```yaml
include:
  - remote: 'https://raw.githubusercontent.com/orieg/discipline/v0.13.1/templates/discipline.gitlab-ci.yml'
```

Or configure a standalone job emitting native GitLab Code Quality diffs:

```yaml
discipline:gate:
  stage: test
  image:
    name: ghcr.io/orieg/discipline:v0
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
          binary_path: /opt/discipline/discipline # optional: a binary already on the runner (air-gapped); omit to download
```

### Gitea Actions

The same composite action runs under Gitea's `act_runner` without modification:

```yaml
      - uses: https://github.com/orieg/discipline@v0
        with:
          binary_path: /opt/discipline/discipline # optional: a binary already on the runner (air-gapped); omit to download
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
      - name: pr-body
        value: ""
      - name: discipline-image
        value: "ghcr.io/orieg/discipline:v0"
```

### pre-commit Hook

```yaml
repos:
  - repo: https://github.com/orieg/discipline
    rev: v0.13.1
    hooks:
      - id: discipline          # compiles via cargo
      # Or: - id: discipline-system # uses pre-installed binary on PATH
```

Or run staged inspection directly in git pre-commit hooks:
```bash
discipline check --staged
```

### Agent Hooks

Run the gates inside a coding agent's edit loop, so an agent that weakens a test is told while it is still editing rather than when CI fails:

```bash
discipline hook install --agent claude-code   # or: codex, cursor, aider, copilot, agy, qwen, opencode
```

`hook install` writes the agent's configuration at the repository root when that file does not exist (exit 0). When the file already runs discipline for that agent it says so and changes nothing (exit 0). When the file exists without the hook it changes nothing and prints the snippet to merge (exit 1); for Claude Code, merge the two `hooks` entries into the existing `hooks` object, appending to an event's array if the file already has one:

```json
{
  "permissions": { "allow": ["Bash(cargo test:*)"] },
  "hooks": {
    "PostToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "./scripts/lint.sh" }] },
      { "matcher": "Edit|Write|MultiEdit|NotebookEdit",
        "hooks": [{ "type": "command", "command": "discipline hook run --agent claude-code" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "discipline hook run --agent claude-code" }] }
    ]
  }
}
```

The hook file is project configuration: commit it so every contributor's agent runs the same check. `agent-scratch` does not report the files `hook install` writes (`.claude/settings.json`, `.cursor/hooks.json`, `.aider.conf.yml` are in its default `exempt_paths`); `instruction-smuggling` does report a change to them, because it changes what the agent is made to do, so the pull request that adds the hook carries `allow-agent-instructions: <file> <reason>`. Each configuration runs `discipline hook run --agent <name>`, which checks the change so far (committed on the branch and uncommitted, though a file never `git add`ed is not seen, against the merge base with `origin`'s default branch, else `main` / `master`; `--base` or `DISCIPLINE_BASE_REF` overrides it) and answers in that agent's hook contract:

| Agent | File | Runs on | A finding |
|---|---|---|---|
| Claude Code | `.claude/settings.json` | `PostToolUse` (`Edit`, `Write`, `MultiEdit`, `NotebookEdit`) and `Stop` | exit 2, the report on stderr, which the agent reads |
| Codex CLI | `.codex/hooks.json` | `PostToolUse` (`apply_patch`, `Edit`, `Write`) and `Stop` | exit 2, the report on stderr |
| Cursor | `.cursor/hooks.json` | `stop` (`loop_limit: 3`) | `{"followup_message": <report>}` on stdout, sent as the next message |
| Aider | `.aider.conf.yml` | `lint-cmd` after each edit (`auto-lint: true`) | exit 1, the report on stdout |
| GitHub Copilot CLI | `.github/hooks/discipline.json` | `postToolUse` (`create`, `edit`, `str_replace_editor`) and `agentStop` | after an edit, exit 0 with `{"additionalContext": <report>}`, appended to the tool result the model reads; at the end of a turn, `{"decision": "block", "reason": <report>}`, which forces another turn |
| Antigravity CLI (`agy`) | `.agents/hooks.json` | `Stop` only (a `PostToolUse` hook's output does not reach agy's model) | `{"decision": "continue", "reason": <report>}`, which re-enters the loop with the report as a system message |
| Qwen Code | `.qwen/settings.json` | `PostToolUse` (`write_file`, `edit`) and `Stop` | exit 2, the report on stderr (Claude Code's contract) |
| OpenCode | `.opencode/plugins/discipline.js` | a plugin on `tool.execute.after` for `edit`, `write`, `apply_patch` | the plugin appends the report to the tool's output; `hook run --agent opencode` exits 1 with the report on stdout |

Loop guards at the end of a turn: Claude Code, Codex, Copilot CLI and Qwen Code send `stop_hook_active` on a turn a hook already continued, and the hook lets it through (Copilot CLI and Qwen Code also stop after eight continuations); Cursor's `loop_limit` is 3. agy documents no guard, so discipline counts consecutive blocks for each conversation in `<git dir>/discipline/agy-stop-<id>` (never tracked) and lets the stop through after three; a pass resets the count. OpenCode's plugin runs after edit tools only. The Copilot, agy, Qwen and OpenCode contracts are read from each tool's documentation (the module header of `src/hook.rs` cites the pages); the OpenCode plugin and the agy file have not been run against a live session of those tools.

The report is the `agent-prompt` format: each finding with its location and the repair, never the directive that would waive it. For a weakened test, a Claude Code agent reads on stderr, with exit 2:

```text
Discipline gatekeeper detected violations in your changes. Please fix each issue:

### Issue 1 [assertion-reduction]: Assertion Reduction In Existing Test
- Location: tests/a.rs:2
- Problem: Test `adds`: effective assertions dropped from 2 to 0.
- Repair: Restore the assertions that were removed or weakened ...
```

**What a change cannot do to the check that judges it.** The hook (and `discipline mcp`) judges the change by the base ref's `discipline.toml` (`--policy-from base`), so an agent that edits the configuration does not switch its own gates off, and it reads no directive (a waiver in a commit message does not lift a finding here; the reviewed PR body lifts it in CI). Leaving waiver syntax out of the report is a convenience, not the control: an agent can run `discipline explain` like anyone else. The control is CI with `policy_from: base`, directives read from the PR body only, and `fail_on_overrides` or `require_approval` (see [High-Assurance Agent Guard Configuration](#high-assurance-agent-guard-configuration)). Findings a repository already has, such as a missing `AGENTS.md`, appear in every hook report too; record them with `discipline baseline --write` before installing the hook.

A check that cannot run (configuration that does not parse, a base that does not resolve) blocks with the reason; it never reads as a pass. An event that this hook already continued (`stop_hook_active`, sent by Claude Code, Codex, Copilot CLI and Qwen Code) is let through, so a finding the agent cannot fix returns control to the person instead of looping; CI still gates the change. `discipline` must be on the agent's `PATH`.

### Pull-Request Comments

```yaml
permissions:
  contents: read
  pull-requests: write        # GitHub: lets the token comment
steps:
  - uses: orieg/discipline@v0
    with:
      comment: true
```

`--comment` (`DISCIPLINE_COMMENT=1`, the action's `comment` input) posts the report as one comment on the pull request and edits that comment on every later run, so a pull request carries one report however often it is checked. It is the reviewer-visible surface on Gitea and Forgejo, which have no code-scanning view. The comment lists each finding with its repair, the overrides that lifted findings and any policy refusal; it never carries directive syntax (reviewers run `discipline explain <gate>`).

The token comes from `DISCIPLINE_FORGE_TOKEN` or the forge's own variable (`GH_TOKEN` or `GITHUB_TOKEN`, `GITEA_TOKEN`, `FORGEJO_TOKEN`, `GITLAB_TOKEN`); the action passes its `token` input only when `comment` is true. The pull request is read from the event payload (`GITHUB_EVENT_PATH` and the Gitea / Forgejo equivalents) or GitLab's `CI_MERGE_REQUEST_IID`; a run without one posts nothing. A token that cannot write, as on a pull request from a fork, is reported and the gates' verdict stands; a forge that cannot be identified or reached stops the run (exit 2). The check's status, not the comment, is the verdict.

### Previewing Adoption: `discipline replay`

```bash
discipline replay --last 50                          # the working tree's discipline.toml
discipline replay --last 100 --config candidate.toml --ref origin/main --json
```

Replays the last N first-parent commits of a branch (default: `origin`'s default branch, else `main` / `master`), one merged change each, through a configuration, and prints which would have been blocked and by which gate. Each change is rebuilt in a throwaway repository that borrows the source repository's objects: its parent with the configuration under test as the base, and the change on top with the same configuration (a change to `discipline.toml` itself is not replayed), then `discipline check` runs on it. Nothing is written to the source repository, and the throwaway repository is removed when the replay ends.

The directives each change carried are read from the body of the pull request it was merged through, by the same forge lookup as the `merged-pr-body` source (a token that can read pull requests: `DISCIPLINE_FORGE_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, `GITEA_TOKEN`, ...; unauthenticated lookups hit the forge's rate limit after a few dozen changes). With no forge (no `origin` on a known forge) or no merged pull request, only the commit message is read, and each case says so (`directives_from`). When the forge is found but cannot be read (a rate limit, a private repository without a token, or `DISCIPLINE_NO_NETWORK=1`), a change that would be blocked is `could_not_check` instead: its pull request body may hold the directive that lifts the finding.

Each change is checked with its pull request's author as the actor (`--actor`), the login the action judges against `directives.allowed_override_actors` by default; an actor set in the replaying shell (`DISCIPLINE_ACTOR`, `GITHUB_ACTOR`, `GITEA_ACTOR`, ...) is ignored, and a change with no merged pull request has no actor. Under `fail_on_overrides = true`, every override applied by an author outside `allowed_override_actors` is refused and blocks the change: the case lists those gates in `refused_overrides` and the login in `actor`, while `blocking_gates` keeps only the gates with an `error` finding.

The configuration under test is usually newer than the history it replays. A file it names for `version-lockstep` or `manifest-sync` that neither side of a replayed change has yet skips that group or rule with a note, rather than failing the whole change; outside replay, the same missing file is a configuration error (exit 2). Replay marks its cases with `DISCIPLINE_REPLAY_CASE`, which is honoured only when the base is the parentless commit replay builds for the case; set on any other `check`, it changes nothing.

`--json` prints the per-change verdicts and the per-gate counts (`errors_by_gate` names the changes each gate blocked with an error finding; `refused_overrides_by_gate` the changes whose override of a gate was refused; `could_not_check_by_reason` groups the changes that could not be checked by the error they stopped on). The command exits 0 when the replay ran, whatever it found; 2 when it could not run. The shape is defined by `discipline.replay.schema.json`, and the `check --format json` report's by `discipline.report.schema.json`, both at the repository root.

### VS Code Problems Panel

Run discipline as a VS Code task and its findings land in the **Problems** panel, each with its file, line and gate. Add this to `.vscode/tasks.json`:

<!-- vscode-problem-matcher -->
```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "discipline",
      "type": "shell",
      "command": "discipline diff",
      "options": { "env": { "NO_COLOR": "1" } },
      "problemMatcher": {
        "owner": "discipline",
        "source": "discipline",
        "fileLocation": ["relative", "${workspaceFolder}"],
        "pattern": [
          {
            "regexp": "^(error|warning|note) \\[([a-z0-9-]+)\\] .+ \\[(.+?)(?::(\\d+))?\\]$",
            "severity": 1,
            "code": 2,
            "file": 3,
            "line": 4
          },
          {
            "regexp": "^   (.+)$",
            "message": 1
          }
        ]
      }
    }
  ]
}
```
<!-- /vscode-problem-matcher -->

`discipline diff` checks the working tree against `HEAD` (a file never `git add`ed is not seen); use `discipline check --base main` to check the whole branch. `NO_COLOR` keeps the output plain, which the pattern needs. A finding with no line (a whole-file or gate-wide finding, such as a missing `AGENTS.md`) points at the file's first line or stays in the terminal.

### Keeping Pins Current with Renovate

A repository pins discipline in up to four places: the action (`uses: orieg/discipline@v…`), the container image (`ghcr.io/orieg/discipline` by tag and digest, moved together), the pre-commit `rev:` and the GitLab CI `include:` URL. The preset in this repository moves all of them in one pull request per release. In the repository's `renovate.json`:

```json
{ "extends": ["github>orieg/discipline//renovate/discipline"] }
```

It groups every discipline dependency under one `discipline` update, pins digests (the image's, and the action to a commit SHA; it does not set the action's `version:` input), turns on Renovate's `pre-commit` manager (off by default in Renovate; this enables it for every hook in the repository), and adds a regex manager for the GitLab `include: remote:` URL, which no built-in manager reads. Renovate's own `github-actions` manager already covers `.github`, `.gitea` and `.forgejo` workflows, including `container: image:` with a digest.

### Explaining a Gate

```bash
discipline explain assertion-reduction
discipline explain "error [error-swallowing] Result Discarded"   # a finding line works too
```

Prints what the gate checks, its languages, its state under this repository's configuration, the directive that lifts a finding (with the subject it takes) and the reference link. An unknown query exits 2 and suggests gate ids that share a word with it. This is the human-facing explanation: the agent-facing surfaces (`agent-prompt`, `discipline hook run`, `discipline mcp`'s `explain_finding`) leave the directive out.

### MCP Server

`discipline mcp` serves the gates to any MCP-capable agent over stdio (newline-delimited JSON-RPC; no socket, no network). Register it with the agent's MCP configuration:

| Client | Registration |
|---|---|
| Claude Code | `claude mcp add discipline -- discipline mcp`, or a project `.mcp.json` with the JSON below |
| Cursor | `.cursor/mcp.json` with the JSON below |
| Codex CLI | `config.toml` in the Codex home directory: `[mcp_servers.discipline]` with `command = "discipline"` and `args = ["mcp"]` |
| GitHub Copilot CLI | `copilot mcp add discipline -- discipline mcp`, or `.github/mcp.json` with the JSON below plus `"type": "local"` |
| Antigravity CLI (`agy`) | `.agents/mcp_config.json` with the JSON below |
| Qwen Code | `qwen mcp add --scope project discipline discipline mcp`, or the `mcpServers` key in `.qwen/settings.json` |
| OpenCode | `opencode.json`: `"mcp": { "discipline": { "type": "local", "command": ["discipline", "mcp"] } }` |
| Any other client | its server list, command `discipline`, argument `mcp` |

```json
{ "mcpServers": { "discipline": { "command": "discipline", "args": ["mcp"] } } }
```

The server checks the repository it is started in: a client that starts servers outside the project gets `could_not_check` ("not inside a git repository"), never a pass.

**Hooks or MCP.** A hook pushes the findings to the agent after every edit and before it stops, whether or not the agent asks; MCP lets the agent ask when it chooses. The hook is the enforcement; MCP is a tool the agent can use to check before it commits. Use the hook, and add MCP where the agent should be able to ask.

| Tool | Arguments | Returns |
|---|---|---|
| `check_diff` | none | The `agent-prompt` report for the change so far (committed and uncommitted, not files never `git add`ed, against the merge base with the default branch, under the default branch's configuration, reading no directive); `structuredContent.status` is `pass`, `findings` or `could_not_check` (the last also `isError`). The agent cannot choose the base: naming `HEAD` would judge a committed change by its own configuration |
| `list_gates` | none | The `discipline gates` table under the repository's configuration |
| `explain_finding` | `query`: a gate id or a finding line naming `[gate-id]` | The gate's suite, what it checks, its languages and its reference link; an unknown query suggests gate ids |

Every tool is read-only (`readOnlyHint`): none writes a file, a directive or a baseline, and no output carries waiver syntax, so an agent is told how to repair a finding, never how to excuse it. The server runs in the directory the agent starts it in; `discipline` must be on the agent's `PATH`.

### Docker Container

Official multi-arch (`linux/amd64`, `linux/arm64`) minimal OCI container images are published to GitHub Container Registry:
- `ghcr.io/orieg/discipline:latest`
- `ghcr.io/orieg/discipline:v0`
- `ghcr.io/orieg/discipline:v0.13.1`

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
The container image holds the static binary, `git` and `sh`; it has no Node.js and no `bash`. `actions/checkout` is a JavaScript action, so on GitHub Actions, Gitea Actions (`act_runner`) and Forgejo Actions it cannot run inside this image. A job that runs in the image checks out with `git` itself.

Two things a job container recipe must get right:

- `runs-on` names a **label** the runner registered (`ubuntu-latest`, `docker`, ...), never an image. `docker://image` is the value a label maps to when the runner is registered (`python:docker://python:3.12-bookworm`); written under `runs-on`, it matches no label and the job queues forever. On a required check, every pull request then waits on it. The image goes under `container: image:`.
- The checkout must land on the **pull request's own commit**. `git clone` lands on the default branch, so a check against that branch compares it with itself, examines none of the change, and passes every pull request. Fetch `refs/pull/<n>/head` (served by GitHub, Gitea and Forgejo) and check out the head commit the event names, then verify the two agree.

The job also has to hand the binary what the composite action would: the base branch, the pull request title and body (override directives, PR-body hygiene) and the actor for `allowed_override_actors`. The actor is the pull request's **author**, which the server sets, not the login that triggered the run: on an `edited` event the trigger is whoever changed the description, and an allow-listed editor must not be able to approve their own override.

**Recommended CI Integration Patterns:**

1. **Host runner using composite action (Standard):** the runner label maps to an image with Node.js (the default `act_runner` and `forgejo-runner` images, and every GitHub-hosted runner), so `actions/checkout` runs and the action fetches the binary:
   ```yaml
   jobs:
     discipline:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
           with:
             fetch-depth: 0
         - uses: orieg/discipline@v0
   ```

2. **Job container, no `uses:` (Gitea Actions, Forgejo Actions, GitHub Actions):**
   The job runs inside the pinned image on a registered label and performs its own checkout. Pin the image by tag **and** digest: when a reference carries both, the digest is what runs and the tag is only a comment, so the tag must name the release the digest is. A line reading `:latest@sha256:...` runs whatever the digest was when it was written, not the latest release, and reports nothing. Never pair a digest with `latest`. Read the digest of a release with `docker buildx imagetools inspect ghcr.io/orieg/discipline:v0.13.1`.
   <!-- snippet: gitea-container-recipe (executed in CI by tests/action/test-container-recipe.sh) -->
   ```yaml
   name: CI Sentinel

   on:
     pull_request:
       types: [opened, synchronize, reopened, edited]

   jobs:
     discipline:
       runs-on: ubuntu-latest # a label the runner registers, never an image
       container:
         image: ghcr.io/orieg/discipline:v0.13.1@sha256:<digest of that release>
       defaults:
         run:
           shell: sh # the image has no bash
       steps:
         - name: Check out the pull request's own commit
           env:
             SERVER_URL: ${{ github.server_url }}
             REPOSITORY: ${{ github.repository }}
             TOKEN: ${{ github.token }} # read access for a private repository
             PR_NUMBER: ${{ github.event.pull_request.number }}
             HEAD_SHA: ${{ github.event.pull_request.head.sha }}
             BASE_REF: ${{ github.event.pull_request.base.ref }}
           run: |
             set -eu
             git init -q .
             git remote add origin "${SERVER_URL}/${REPOSITORY}.git"
             auth="$(printf 'x-access-token:%s' "${TOKEN}" | base64 | tr -d '\n')"
             git -c "http.extraHeader=Authorization: Basic ${auth}" fetch -q --no-tags origin \
               "+refs/pull/${PR_NUMBER}/head:refs/remotes/origin/pr-${PR_NUMBER}" \
               "+refs/heads/${BASE_REF}:refs/remotes/origin/${BASE_REF}"
             git checkout -q "${HEAD_SHA}"
             echo "checked out $(git rev-parse HEAD); base ${BASE_REF} is $(git rev-parse "origin/${BASE_REF}")"
             [ "$(git rev-parse HEAD)" = "${HEAD_SHA}" ] || { echo "HEAD is not the pull request head"; exit 1; }
         - name: Run discipline
           env:
             DISCIPLINE_BASE_REF: origin/${{ github.event.pull_request.base.ref }}
             DISCIPLINE_ACTOR: ${{ github.event.pull_request.user.login }} # the author, not the trigger
             PR_TITLE: ${{ github.event.pull_request.title }}
             PR_BODY: ${{ github.event.pull_request.body }}
           run: discipline check --fail-on-warnings
   ```
   The checkout step prints the commit it landed on next to the base branch's commit; on a pull request they differ. Its last line fails the job if the checkout is not the event's head commit, so a runner that serves the wrong ref cannot pass silently. `github.token` is the job's own read token on all three forges; on a public repository the header is harmless. `directives.require_approval` still applies on top of the actor: an override then also needs an approving review of the head commit by someone other than the author (see [Trust Model](#trust-model)).

   This recipe is executed in this repository's CI under nektos/act (the engine inside `act_runner`) on every change: the fenced block above is extracted verbatim, the image placeholder is filled with the image built in that run, and the job must schedule, land on the pull request head, pass a clean change and reject a change that trips known gates (`tests/action/test-container-recipe.sh`).

3. **GitLab CI (`.gitlab-ci.yml`):**
   ```yaml
   discipline:
     image:
       name: ghcr.io/orieg/discipline:v0
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
  curl -fsSL https://orieg.github.io/discipline/apt/discipline-archive-keyring.gpg | sudo tee /usr/share/keyrings/discipline-archive-keyring.gpg >/dev/null
  echo "deb [signed-by=/usr/share/keyrings/discipline-archive-keyring.gpg] https://orieg.github.io/discipline/apt/ stable main" | sudo tee /etc/apt/sources.list.d/discipline.list
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
  brew install orieg/tap/discipline

  # MacPorts: from the Portfile attached to each release, via a local ports tree
  mkdir -p ~/ports/devel/discipline
  curl -fsSL -o ~/ports/devel/discipline/Portfile https://github.com/orieg/discipline/releases/latest/download/Portfile
  (cd ~/ports && portindex) && sudo port install discipline   # after adding file:///Users/<you>/ports to sources.conf
  ```

- **Cargo**:
  ```bash
  cargo install --git https://github.com/orieg/discipline   # from source; not published on crates.io
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
| **Machine JSON Report** | `--format json` (stdout) or `--json-out <path>` | Full JSON outcome with detailed violation records, notes, examined tallies, and applied overrides. |
| **GitHub Step Summary** | `--format github-summary` (the action's format) | Terminal report plus annotations; when set, appends a Markdown table to `GITHUB_STEP_SUMMARY` and writes the counts to `GITHUB_OUTPUT`. |
| **GitLab Code Quality** | `--report-gitlab <path>` (written to `gl-codequality.json` by default in GitLab CI) | JSON format rendered directly in GitLab Merge Request diff widgets. |
| **SARIF** | `--report-sarif <path>` | OASIS SARIF v2.1.0 report for GitHub Code Scanning, VS Code, and security dashboards. |
| **JUnit XML** | `--report-junit <path>` (written to `junit.xml` by default in GitLab CI) | Standard test results XML for CI test summary dashboards and flaky test tracking. |

---

## Override Audit Trail

When an authorized directive is parsed and applied:
1. **Audit Record:** The gate outcome records the override in its result structure, naming the gate, subject, and reason.
2. **Action Outputs:** Outputs `overrides` (total count of applied overrides) and `overridden_gates` (comma-separated list of gate ids) are populated.
3. **Machine Report:** The JSON report carries the total as top-level `overrides` and each record under `outcomes[].overrides[]` (`gate`, `subject`, `directive`, `reason`, `source`, `hidden`) for compliance logging.
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
- **Diff-scoped gates** such as `vacuous-tests`, `unsafe-safety-comment` and `provenance-tags` only look at what changed. On a clean branch the diff is empty, so diff mode records **none** of their pre-existing findings, and the gate cannot be enabled without first fixing everything it would report.

A whole-tree baseline diffs against the empty tree, so every file is "added". Gates whose rule describes a change (`dependency-delta`, `ignored-tests`, `config-integrity`, `ci-integrity`, `build-hooks`, `error-swallowing`, `stub-bodies`, `suppression-delta`, and the other delta rules) are reported as not evaluated in that mode and record nothing: a dependency that exists is not debt. Rules describing a state (`pii`, `time-estimates`, `vacuous-tests`, `unsafe-safety-comment`, invisible characters) are recorded. A symlink is its target, enumerated once.

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

version = 2

[[findings]]
gate = "time-estimates"
rule = "time-estimates/time-estimate"
path = "docs/old_plan.md"
fingerprint = "5847c9fe7cec2622f5c427937d6a48adebd8e240c027a55d9fdaf8009e8b7100"
```

Committing a new or grown baseline trips `config-integrity` ("Baseline Grew Without Directive"). `baseline --write` prints the exact directive line to add to the commit message.

Fingerprints (version 2) are `sha256("v2:" + gate/code + ":" + path + ":" + sha256(trimmed_line))`, with the finding's message in place of the line for a finding that has none. They use the finding's code (`rule`), never its title, and no line number, so neither a reworded title nor a line shift caused by an unrelated edit churns the baseline. The same fingerprint is the `fingerprint` field of each finding in `check --format json` and the SARIF `partialFingerprints`.

**Version 1 files.** Releases before the finding codes wrote `version = 1`, fingerprinted on the finding's title. Such a file still grandfathers its findings, and each run reports a `deprecated:` line. `discipline baseline --migrate` rewrites it to version 2: every entry a current finding still matches is kept, under its code, and stale entries are dropped, so the file never grows. Commit the migration in a change of its own: `config-integrity` accepts a change that touches nothing but the baseline, does not grow it and keeps every entry's gate and path, without a directive. Mixed with other changes it is reported as `config-integrity/baseline-migration-not-alone`. A `--write --suite <suite>` over a version-1 file is refused until it is migrated, since the entries of the gates it does not run could not be rewritten.

### 2. Subsequent CI Execution

Subsequent runs automatically detect `discipline-baseline.toml` if present:
- **Pre-existing findings:** Grandfathered and reported per gate as a non-blocking note (`N finding(s) grandfathered by baseline in this gate (not blocking)`) and as `baselined: N` in the summary line. They do not cause non-zero exit codes.
- **New violations:** Fail CI immediately with exit code 1.
- **Transparency:** The baselined count is reported in the terminal, the GitHub step summary and JSON (not in JUnit, SARIF or GitLab reports) and exposed to GitHub Actions workflows via the `steps.<id>.outputs.baselined` step output.
- **Bypassing Baseline:** Pass `--no-baseline` (or set `no_baseline: true` in the action) to evaluate the diff without grandfathering.
- **Custom Baseline Path:** Pass `--baseline-file <path>` (or set `baseline_file` in the action).

### 3. Ratchet Protection & Technical Debt Burndown

1. **Ratchet Against Growth:** The baseline is protected by the `config-integrity` gate. Attempting to add new findings to `discipline-baseline.toml` is classified as gate weakening and fails CI unless accompanied by a scoped directive:
   ```text
   allow-gate-weakening: baseline grandfathering legacy modules for migration
   ```
   A baseline that keeps its size but swaps grandfathered findings for new ones is refused the same way ("Baseline Contains New Findings Without Directive").
2. **Ratchet Down (Stale Entry Notes):** When a grandfathered finding is fixed in source code, Discipline reports the stale baseline entry as a note on its gate:
   ```text
   1 stale baseline entry (resolved findings) (`Time Estimate` in `docs/old_plan.md`): run `discipline baseline --write` to ratchet down
   ```
   Maintainers can re-run `discipline baseline --write` to remove the stale entry and lock in the improvement without needing an override.

---

## Recipes & Monorepo Setup

Discipline natively supports polyglot monorepos and multi-tier architectures using path scoping, vocabulary extension, and command gate presets.

### Release Gate: No Source in the Published Package (`archive-contents`)

A release job can check the exact file it is about to publish and stop the publish when [`archive-contents`](GATES.md#archive-contents) fails. The pattern, on a tag push:

1. Build and **pack to a file** (`npm pack --pack-destination release`, not `npm pack --dry-run --json`), so the check reads the bytes that get uploaded.
2. Run `discipline check` with the gate forced on and its settings supplied inline (`DISCIPLINE_CONFIG_OVERRIDE`), measured against the tagged commit itself (`--base HEAD`), so the change-based gates see no change and only the package is judged.
3. Publish that file, in a step or job that runs only when the check passed.

The snippets below are read by `tests/test_release_recipe.rs`, which runs their discipline command and configuration through the binary against a leaking and a clean npm tarball. For another ecosystem, change the pack command, `archive_path` and `preset`:

| Ecosystem | Pack to a file | `archive_path` | `preset` |
|---|---|---|---|
| npm | `npm pack --pack-destination release` | `release/*.tgz` | `no-source-npm` |
| Python | `python -m build --wheel --outdir release` | `release/*.whl` | `no-source-python` |
| Rust | `cargo package` | `target/package/*.crate` | `no-source-rust` |
| .NET | `dotnet pack -o release` | `release/*.nupkg` | `no-source-dotnet` |
| JVM | `mvn package` | `target/*.jar` | `no-source-jvm` |
| Go binaries | your release archive (`goreleaser`, `tar`) | `dist/*.tar.gz` | `no-source-go` |

`preset = "no-source"` takes the union of the binary-ecosystem lists and turns the content scan on; the ecosystem presets leave `scan_contents` to you, so the snippets set it.

GitHub Actions (the `publish` step runs only when the discipline step succeeded):

<!-- release-recipe-github -->
```yaml
name: Release

on:
  push:
    tags: ['v*']

permissions:
  contents: read

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7.0.0
        with:
          node-version: 22
          registry-url: https://registry.npmjs.org
      - run: npm ci && npm run build
      - run: mkdir -p release && npm pack --pack-destination release
      - name: Check the packed tarball
        uses: orieg/discipline@v0
        with:
          suite: integrity
          base_ref: HEAD
          enable: archive-contents
          config_override: |
            [gates.archive-contents]
            archive_path = "release/*.tgz"
            preset = "no-source-npm"
            scan_contents = true
      - name: Publish
        run: npm publish release/*.tgz
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```
<!-- /release-recipe-github -->

GitLab CI/CD (three jobs; `publish` needs `check-package`, so a failed check stops it):

<!-- release-recipe-gitlab -->
```yaml
stages: [package, verify, publish]

package:
  stage: package
  image: node:22
  rules:
    - if: $CI_COMMIT_TAG
  script:
    - npm ci
    - npm run build
    - mkdir -p release && npm pack --pack-destination release
  artifacts:
    paths: [release/]

check-package:
  stage: verify
  image:
    name: ghcr.io/orieg/discipline:v0
    entrypoint: [""]
  rules:
    - if: $CI_COMMIT_TAG
  needs: [package]
  variables:
    DISCIPLINE_ENABLE: archive-contents
    DISCIPLINE_CONFIG_OVERRIDE: |
      [gates.archive-contents]
      archive_path = "release/*.tgz"
      preset = "no-source-npm"
      scan_contents = true
  script:
    - discipline check --suite integrity --base HEAD

publish:
  stage: publish
  image: node:22
  rules:
    - if: $CI_COMMIT_TAG
  needs: [package, check-package]
  script:
    - npm config set //registry.npmjs.org/:_authToken "${NPM_TOKEN}"
    - npm publish release/*.tgz
```
<!-- /release-recipe-gitlab -->

With `--base HEAD` there is no commit range and no pull request, so no directive is read from either. A finding the release is meant to carry is lifted by passing `allow-archive-leak: <entry> <reason>` explicitly: the action's `pr_body` input, or `PR_BODY` / `--pr-body-file` on the CLI. The test runs that too.

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
extra_assert_macros = ["custom_assert", "verify_invariant"] # macro names; a trailing `!` is optional
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
deny_dependencies = ["left-pad", "evil-package"]
allow_wildcards = false
require_git_pins = true

[gates.command]
enabled = true
commands = [
  { name = "cargo-mutants", preset = "cargo-mutants", timeout_seconds = 300 },
  { name = "frontend-coverage", preset = "lcov", command = "lcov --summary apps/web/coverage/lcov.info" }
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
