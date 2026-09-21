//! JSON Schema generation for `discipline.toml`.
//!
//! Provides `generate_schema()` which emits a JSON Schema (draft 2020-12)
//! precisely matching `DisciplineConfig`, all gate tables, and asymmetric list
//! reset structures.

use crate::config::{gate_info, GATES, SCHEMA_VERSION};
use serde_json::{json, Value};

/// Generate JSON Schema for `discipline.toml`.
pub fn generate_schema() -> Value {
    let mut gate_properties = serde_json::Map::new();

    for g in GATES.iter().filter(|g| g.available) {
        let ref_name = match g.id {
            "assertion-reduction" | "vacuous-tests" => "#/$defs/AssertionGate",
            "ignored-tests" => "#/$defs/IgnoredTestsGate",
            "deletion-rationale" => "#/$defs/DeletionGate",
            "time-estimates" => "#/$defs/TimeEstimateGate",
            "pii" => "#/$defs/PiiGate",
            "agent-scratch" => "#/$defs/ScratchGate",
            "golden-output" => "#/$defs/GoldenGate",
            "bench-regression" => "#/$defs/BenchRegressionGate",
            "unsafe-safety-comment" => "#/$defs/UnsafeSafetyCommentGate",
            "command" => "#/$defs/CommandGate",
            "dependency-delta" => "#/$defs/DependencyDeltaGate",
            "test-budget" => "#/$defs/TestBudgetGate",
            "test-floor" => "#/$defs/TestFloorGate",
            "ci-integrity" => "#/$defs/CiIntegrityGate",
            "ci-skip-set" => "#/$defs/CiSkipSetGate",
            "shell-secrets" => "#/$defs/ShellSecretsGate",
            "issue-link" => "#/$defs/IssueLinkGate",
            "provenance-tags" => "#/$defs/ProvenanceTagsGate",
            "archive-contents" => "#/$defs/ArchiveContentsGate",
            "manifest-sync" => "#/$defs/ManifestSyncGate",
            "version-lockstep" => "#/$defs/VersionLockstepGate",
            "scope-confinement" => "#/$defs/ScopeConfinementGate",
            "suppression-delta" => "#/$defs/SuppressionDeltaGate",
            "pr-checklist" => "#/$defs/PrChecklistGate",
            "unsafe-budget" => "#/$defs/UnsafeBudgetGate",
            "msrv" => "#/$defs/MsrvGate",
            "miri" => "#/$defs/MiriGate",
            "sanitizers" => "#/$defs/SanitizersGate",
            _ => "#/$defs/BasicGate",
        };
        let desc = gate_info(g.id).map(|info| info.summary).unwrap_or("");
        gate_properties.insert(
            g.id.to_string(),
            json!({
                "allOf": [{ "$ref": ref_name }],
                "description": desc,
            }),
        );
    }

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "DisciplineConfig",
        "description": "Configuration schema for Discipline CI/CD gatekeeper and agent diff sentinel (discipline.toml)",
        "type": "object",
        "required": ["meta"],
        "additionalProperties": false,
        "properties": {
            "meta": {
                "type": "object",
                "required": ["version", "name"],
                "additionalProperties": false,
                "description": "Repository metadata and configuration format version",
                "properties": {
                    "version": {
                        "type": "integer",
                        "const": SCHEMA_VERSION,
                        "description": format!("Configuration schema version (must be {SCHEMA_VERSION})")
                    },
                    "name": {
                        "type": "string",
                        "description": "Repository or project name"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional short description of the project"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["enforcing", "advisory"],
                        "default": "enforcing",
                        "description": "Operating mode: 'enforcing' exits non-zero on violations; 'advisory' runs all checks and emits reports but exits 0."
                    }
                }
            },
            "directives": {
                "type": "object",
                "additionalProperties": false,
                "description": "Policy controls for override directives (sources and visibility)",
                "properties": {
                    "sources": {
                        "$ref": "#/$defs/StringListOrReset",
                        "description": "Allowed directive sources: pr-body, commits (default: [\"pr-body\", \"commits\"])"
                    },
                    "allow_hidden": {
                        "type": "boolean",
                        "description": "Allow directives hidden inside HTML comments <!-- --> (default: false)"
                    },
                    "fail_on_overrides": {
                        "type": "boolean",
                        "description": "Treat applied overrides as failures requiring human sign-off (default: false)"
                    },
                    "allowed_override_actors": {
                        "$ref": "#/$defs/StringListOrReset",
                        "description": "Actors authorized to apply overrides even when fail_on_overrides is true (default: [])"
                    }
                }
            },
            "gates": {
                "type": "object",
                "additionalProperties": false,
                "description": "Per-gate settings. Each gate has a built-in default enablement and severity; see docs/GATES.md \"Default Severity by Gate\".",
                "properties": Value::Object(gate_properties)
            }
        },
        "$defs": {
            "Severity": {
                "type": "string",
                "enum": ["error", "warning", "note"],
                "description": "Violation severity: error (blocking, exit 1), warning (non-blocking), or note (informational)."
            },
            "StringListOrReset": {
                "description": "A list of strings, or a table with reset = true to clear lower precedence layers.",
                "oneOf": [
                    {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["reset"],
                        "properties": {
                            "reset": { "type": "boolean" },
                            "items": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        }
                    }
                ]
            },
            "BasicGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" }
                }
            },
            "AssertionGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "extra_assert_macros": { "$ref": "#/$defs/StringListOrReset", "description": "Additional macro names treated as assertions" },
                    "assert_helper_fns": { "$ref": "#/$defs/StringListOrReset", "description": "Additional function names treated as assertions" },
                    "min_assertions_per_test": { "type": "integer", "description": "Minimum assertions required per test method" }
                }
            },
            "DeletionGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "paths": { "$ref": "#/$defs/StringListOrReset", "description": "Path globs where file deletions require a rationale" },
                    "require_scope": { "type": "boolean", "description": "When true, directive must name the deleted file or test" },
                    "allow_hidden": { "type": ["boolean", "null"], "description": "When true, HTML-comment-wrapped directives are accepted for deletions" }
                }
            },
            "IgnoredTestsGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "approved_predicates": { "$ref": "#/$defs/StringListOrReset", "description": "Conditional ignore predicates (e.g. miri) approved by policy" }
                }
            },
            "TimeEstimateGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "include": { "$ref": "#/$defs/StringListOrReset", "description": "File globs swept for duration estimates" },
                    "extra_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Additional banned regex patterns" },
                    "allow_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Regex patterns permitted as operational exceptions; matched per line and across soft-wrapped lines of a paragraph, exempting only the matched text" },
                    "scan_pr_body": { "type": "boolean", "description": "Whether to scan PR description text" },
                    "diff_only": { "type": "boolean", "description": "When true, scans only modified lines in the git diff rather than all tracked files" }
                }
            },
            "PiiGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "home_paths": { "type": "boolean", "description": "Check for leaked home directory paths" },
                    "lan_ips": { "type": "boolean", "description": "Check for leaked private LAN IPs" },
                    "secrets": { "type": "boolean", "description": "Check for leaked private keys and high-entropy API tokens" },
                    "allowed_users": { "$ref": "#/$defs/StringListOrReset", "description": "Username tokens permitted inside home-directory paths" },
                    "hostname_denylist": { "$ref": "#/$defs/StringListOrReset", "description": "Whole-token, case-insensitive hostnames that must not appear" },
                    "extra_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Additional regex patterns to reject" },
                    "allow_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Regex patterns exempted from rejection" },
                    "scan_pr_body": { "type": "boolean", "description": "Whether to scan PR description text" },
                    "diff_only": { "type": "boolean", "description": "When true, scans only modified lines in the git diff rather than all tracked files" },
                    "agent_config_refs": { "type": "boolean", "description": "When true, flags references to personal agent configuration directories and playbook docs" }
                }
            },
            "ScratchGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "paths": { "$ref": "#/$defs/StringListOrReset", "description": "Directory and file globs that must never be tracked" }
                }
            },
            "GoldenGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "paths": { "$ref": "#/$defs/StringListOrReset", "description": "Committed golden/snapshot globs whose edits require a directive" },
                    "allow_updates": { "type": "boolean", "description": "Permit snapshot updates without error" }
                }
            },
            "BenchRegressionGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "tolerance_pct": { "type": "number", "description": "Maximum allowed regression percentage" },
                    "noise_margin_pct": { "type": "number", "description": "Configurable noise margin added to tolerance_pct" },
                    "max_noise_cv": { "type": "number", "description": "Maximum acceptable coefficient of variation (std_dev / mean)" },
                    "paths": { "$ref": "#/$defs/StringListOrReset", "description": "Benchmark artifact globs tracked across revisions" },
                    "provenance": { "type": "string", "description": "Expected host/runner provenance tag for benchmark artifacts" },
                    "allow_cross_host": { "type": "boolean", "description": "Allow benchmark comparison across mismatched host/runner provenance" },
                    "base_file": { "type": "string", "description": "In-job base benchmark result file path for dual-file regression checks" },
                    "head_file": { "type": "string", "description": "In-job head benchmark result file path for dual-file regression checks" },
                    "noise_floor_pct": { "type": "number", "description": "Noise floor percentage (default: 0.5%)" },
                    "advisory_pct": { "type": "number", "description": "Advisory review percentage (default: 0.1%)" },
                    "exempt_arms": { "$ref": "#/$defs/StringListOrReset", "description": "Benchmark arms exempted from regression checks: exact name, the name as the benchmark prints it (`map_get random` matches `map_get/random`), a glob (`*.heap.*`), a trailing-`*` prefix, or a `::`/`/` path suffix. An entry matching no arm in the run is an error." },
                    "require_sourced_override": { "type": "boolean", "description": "Require allow-regression reasons to cite a CI run URL or artifact path and name the arms. Every citation is also checked for freshness: a cited run must have completed, reached its regression guard, and measured a commit reachable from the head; a cited data artifact must post-date the branch's newest change under `citation_source_paths`. A citation that cannot be checked (no `gh`, unauthenticated, rate limited) is reported by name and leaves the gate armed." },
                    "citation_source_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Repo-relative files or directories whose changes can move a gated number. A cited data artifact last committed before the branch's newest change under these paths is stale. Empty: artifact citations cannot be dated and leave the gate armed." },
                    "citation_measurement_jobs": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/MeasurementJob" },
                        "description": "CI jobs that produce gated numbers, each with the step that gates them. A cited run that concluded `failure` is admitted only when every listed job it started reached its guard step with every earlier step green. Empty: a cited `failure` run cannot be told from a crashed benchmark and leaves the gate armed."
                    },
                    "mode": { "type": "string", "enum": ["version-vs-version", "paired-ratio"], "description": "Evaluation mode: `version-vs-version` compares base and head artifacts of the same arms (preferred when the old version can be built in the same run); `paired-ratio` compares a ratio of two arms measured in the same interleaved rounds against a committed ratio baseline (when building the old version is impractical). The two are not interchangeable." },
                    "ratio_baseline": { "type": "string", "description": "Committed paired-ratio baseline (`discipline-bench-ratio-baseline/v1`), produced by `discipline bench derive`. Read from the base ref, never from head; loosening it needs a scoped `allow-regression: <path>` directive." },
                    "ratio_tolerance_pct": { "type": "number", "description": "Optional minimum paired-ratio threshold in percent. It only widens a derived floor; configured for an axis with no derived floor, it is a configuration error." }
                }
            },
            "MeasurementJob": {
                "type": "object",
                "additionalProperties": false,
                "required": ["job", "guard"],
                "properties": {
                    "job": { "type": "string", "description": "Job display name as the CI API lists it" },
                    "guard": { "type": "string", "description": "Name of the step in that job that reads the numbers and enforces the regression guard" }
                }
            },
            "ProvenanceTagsGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "check_tables": { "type": "boolean", "description": "Check markdown tables for unit-bearing numbers without table or caption provenance tags" },
                    "check_mechanisms": { "type": "boolean", "description": "Check for mechanism claims without hardware counter evidence or explicit hypothesis qualifiers" },
                    "check_intervals": { "type": "boolean", "description": "Check published wall-clock ratios for confidence intervals or explicit qualifiers" },
                    "check_paired_figures": { "type": "boolean", "description": "Check paired figures for shared workload IDs or differentiation tags" },
                    "superseded_registry": { "type": "string", "description": "Path (read at HEAD) of a JSON registry of withdrawn figures; a registered figure may be republished only next to a retraction marker" },
                    "superseded_json_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Globs of tracked JSON datasets swept for registered figures" },
                    "check_pending_citations": { "type": "boolean", "description": "A pending-measurement statement must cite a tracking issue" },
                    "require_open_pending_issues": { "type": "boolean", "description": "A pending-measurement statement must cite at least one open issue, read from the forge (gh on GitHub, curl on GitLab, Gitea and Forgejo); implies check_pending_citations" },
                    "pending_issue_repos": { "$ref": "#/$defs/StringListOrReset", "description": "Other repositories (owner/name) whose issues a pending statement may cite; by default only this repository's issues count" }
                }
            },
            "UnsafeSafetyCommentGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "placeholders": { "$ref": "#/$defs/StringListOrReset", "description": "Additional placeholder words or phrases to reject in SAFETY comments" }
                }
            },
            "CommandGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "preset": { "type": "string", "description": "Predefined turnkey preset name (e.g. cargo-mutants, cargo-deny, loom)" },
                    "command": { "type": "string", "description": "Primary command to execute" },
                    "timeout_seconds": { "type": "integer", "description": "Execution timeout in seconds (default: 60s)" },
                    "count_pattern": { "type": "string", "description": "Regex pattern to extract an integer count" },
                    "min_count": { "type": "integer", "description": "Minimum count required" },
                    "forbid_output": { "$ref": "#/$defs/StringListOrReset", "description": "Output patterns that must not appear in stdout or stderr" },
                    "zero_items_pattern": { "type": "string", "description": "Pattern that indicates zero items were executed" },
                    "allow_zero": { "type": "boolean", "description": "Whether zero items selected is allowed" },
                    "canary_command": { "type": "string", "description": "Optional negative-control canary command" },
                    "canary_expected_diagnostic": { "type": "string", "description": "Expected diagnostic string that canary must produce" },
                    "commands": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/CommandEntry" },
                        "description": "Multi-command suite entries"
                    }
                }
            },
            "CommandEntry": {
                "type": "object",
                "additionalProperties": false,
                "required": ["name"],
                "properties": {
                    "name": { "type": "string", "description": "Name or identifier of the command" },
                    "preset": { "type": "string", "description": "Predefined turnkey preset name (e.g. cargo-mutants, cargo-deny, loom)" },
                    "command": { "type": "string", "description": "Command string to execute" },
                    "timeout_seconds": { "type": "integer", "description": "Execution timeout in seconds" },
                    "count_pattern": { "type": "string", "description": "Regex pattern to extract an integer count" },
                    "min_count": { "type": "integer", "description": "Minimum count required" },
                    "forbid_output": { "$ref": "#/$defs/StringListOrReset", "description": "Output patterns that must not appear in stdout or stderr" },
                    "zero_items_pattern": { "type": "string", "description": "Pattern that indicates zero items were executed" },
                    "allow_zero": { "type": "boolean", "description": "Whether zero items selected is allowed" },
                    "canary_command": { "type": "string", "description": "Optional negative-control canary command" },
                    "canary_expected_diagnostic": { "type": "string", "description": "Expected diagnostic string that canary must produce" }
                }
            },
            "DependencyDeltaGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "manifests": { "$ref": "#/$defs/StringListOrReset", "description": "Manifest file globs to inspect" },
                    "allow_wildcards": { "type": "boolean", "description": "Whether wildcard versions are permitted (default: false)" },
                    "require_git_pins": { "type": "boolean", "description": "Whether git dependencies must specify an immutable commit or tag pin (default: true)" },
                    "deny_file": { "type": "string", "description": "Path to deny.toml policy file" },
                    "allow_dependencies": { "$ref": "#/$defs/StringListOrReset", "description": "Explicit list of allowed dependency package names" },
                    "deny_dependencies": { "$ref": "#/$defs/StringListOrReset", "description": "Explicit list of forbidden dependency package names" }
                }
            },
            "TestBudgetGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "corpus_dirs": { "$ref": "#/$defs/StringListOrReset", "description": "Corpus directory patterns to monitor for seed file shrink" },
                    "fuzz_targets": { "$ref": "#/$defs/StringListOrReset", "description": "Fuzz manifest and harness globs" },
                    "scan_workflows": { "type": "boolean", "description": "Whether to scan workflow files" },
                    "scan_scripts": { "type": "boolean", "description": "Whether to scan shell scripts" }
                }
            },
            "ShellSecretsGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "extra_secret_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Additional custom regex patterns for sensitive secret variable names" },
                    "allow_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Custom regex patterns exempted from violation" },
                    "diff_only": { "type": "boolean", "description": "When true, scans only modified lines in the git diff rather than all tracked files" }
                }
            },
            "IssueLinkGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "pattern": { "type": "string", "description": "Custom regex pattern required in PR title or body" },
                    "require_in_commit_if_no_pr": { "type": "boolean", "description": "Require issue link in commit messages when no PR metadata is supplied" }
                }
            },
            "TestFloorGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "min_tests": { "type": "integer", "description": "Minimum required workspace test count" },
                    "tolerance": { "type": "integer", "description": "Allowed test count decrease below floor or base before violation (default: 0)" },
                    "constant_file": { "type": "string", "description": "File containing a floor constant" },
                    "constant_name": { "type": "string", "description": "Name of the floor constant in constant_file" },
                    "required_suites": { "$ref": "#/$defs/StringListOrReset", "description": "Required test suite files that must exist" },
                    "test_command": { "type": "string", "description": "Custom command to list or count tests" }
                }
            },
            "CiIntegrityGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "workflows": { "$ref": "#/$defs/StringListOrReset", "description": "Workflow file patterns to inspect" },
                    "rollup_job": { "type": "string", "description": "Name of the rollup job that must depend on all jobs" },
                    "excluded_jobs": { "$ref": "#/$defs/StringListOrReset", "description": "Job names excluded from rollup dependency requirements" },
                    "pin_actions": { "type": "boolean", "description": "Ensure third-party GitHub actions are pinned by 40-character commit SHA" },
                    "forbid_continue_on_error": { "type": "boolean", "description": "Forbid continue-on-error: true in workflow jobs or steps" },
                    "forbid_or_true": { "type": "boolean", "description": "Forbid || true and set +e error masking in run commands" },
                    "diff_only": { "type": "boolean", "description": "When true, scans only modified workflow files rather than all workflows" },
                    "documented_job_count_path": { "type": "string", "description": "Path to catalog documentation stating job count" },
                    "documented_job_count_pattern": { "type": "string", "description": "Regex pattern to extract job count from documentation" },
                    "first_party_action_prefixes": { "$ref": "#/$defs/StringListOrReset", "description": "Action prefixes considered first-party and excused from commit SHA pinning" }
                }
            },
            "CiSkipSetGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "workflow": { "type": "string", "description": "Repo-relative path of the workflow whose rollup job supplies the runtime needs context (DISCIPLINE_CI_CONTEXT)" },
                    "change_job": { "type": "string", "description": "Change-detection job whose outputs gate the conditional jobs; it must have succeeded. Empty string = no such job" },
                    "unconditional_jobs": { "$ref": "#/$defs/StringListOrReset", "description": "Jobs that must never be skipped, whatever their dependencies did" }
                }
            },
            "ArchiveContentsGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "archive_path": { "type": "string", "description": "Glob pattern matching the built archive file" },
                    "required_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Files required to exist inside the archive" },
                    "forbidden_patterns": { "$ref": "#/$defs/StringListOrReset", "description": "Regex patterns forbidden inside the archive" },
                    "strip_components": { "type": "integer", "description": "Leading directory components to strip from archive paths" }
                }
            },
            "ManifestSyncGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "rules": {
                        "type": "array",
                        "description": "Rules reconciling packaging manifests against git-tracked files",
                        "items": {
                            "type": "object",
                            "required": ["manifest", "extract_regex", "watched_paths"],
                            "additionalProperties": false,
                            "properties": {
                                "manifest": { "type": "string", "description": "Path to packaging manifest (e.g. package.xml)" },
                                "extract_regex": { "type": "string", "description": "Regex to extract relative file paths from manifest" },
                                "watched_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Git file globs that must be registered in the manifest" },
                                "exclude_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Globs excluded from manifest registration requirement" }
                            }
                        }
                    }
                }
            },
            "VersionLockstepGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "groups": {
                        "type": "array",
                        "description": "Groups of sources that must declare identical version strings",
                        "items": {
                            "type": "object",
                            "required": ["name", "sources"],
                            "additionalProperties": false,
                            "properties": {
                                "name": { "type": "string", "description": "Name of the version lockstep group" },
                                "sources": {
                                    "type": "array",
                                    "description": "Files and capture regexes whose versions must match",
                                    "items": {
                                        "type": "object",
                                        "required": ["path", "regex"],
                                        "additionalProperties": false,
                                        "properties": {
                                            "path": { "type": "string", "description": "Source file path" },
                                            "regex": { "type": "string", "description": "Regex pattern capturing the version string in group 1" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "ScopeConfinementGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "allowed_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Glob patterns of paths agents are authorized to modify" },
                    "forbidden_paths": { "$ref": "#/$defs/StringListOrReset", "description": "Glob patterns of paths agents are strictly forbidden to touch" }
                }
            },
            "SuppressionDeltaGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "max_increase": { "type": "integer", "description": "Maximum net increase in suppression annotations permitted (default: 0)" },
                    "allowed_suppressions": { "$ref": "#/$defs/StringListOrReset", "description": "Specific suppression patterns explicitly permitted by policy" }
                }
            },
            "PrChecklistGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" }
                }
            },
            "UnsafeBudgetGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "max_unsafe": { "type": ["integer", "null"], "description": "Maximum total number of unsafe sites allowed in head ref" },
                    "allow_increase": { "type": "boolean", "description": "Whether total unsafe count may increase over base ref without override" }
                }
            },
            "MsrvGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "pinned_version": { "type": ["string", "null"], "description": "Explicit MSRV version string (e.g. \"1.90.0\")" },
                    "command": { "type": ["string", "null"], "description": "Command to run to verify MSRV compatibility" }
                }
            },
            "MiriGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "args": { "$ref": "#/$defs/StringListOrReset", "description": "Additional CLI arguments passed to cargo miri test" },
                    "timeout_seconds": { "type": "integer", "description": "Maximum execution time in seconds before failing closed (default: 600)" }
                }
            },
            "SanitizersGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "sanitizer": { "type": "string", "description": "Sanitizer name to activate (e.g. \"address\", \"thread\")" },
                    "canary": { "type": "boolean", "description": "Whether to verify a negative-control race canary before main tests" },
                    "timeout_seconds": { "type": "integer", "description": "Maximum execution time in seconds (default: 300)" }
                }
            }
        }
    })
}
