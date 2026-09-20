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
            "shell-secrets" => "#/$defs/ShellSecretsGate",
            "issue-link" => "#/$defs/IssueLinkGate",
            "provenance-tags" => "#/$defs/ProvenanceTagsGate",
            "archive-contents" => "#/$defs/ArchiveContentsGate",
            "manifest-sync" => "#/$defs/ManifestSyncGate",
            "version-lockstep" => "#/$defs/VersionLockstepGate",
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
                "description": "Per-gate settings. By default, every available gate is enabled at severity \"error\".",
                "properties": Value::Object(gate_properties)
            }
        },
        "$defs": {
            "Severity": {
                "type": "string",
                "enum": ["error", "warning", "info"],
                "description": "Violation severity: error (blocking, exit 1), warning (non-blocking), or info."
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
                    "extra_assert_macros": { "$ref": "#/$defs/StringListOrReset" },
                    "assert_helper_fns": { "$ref": "#/$defs/StringListOrReset" },
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
                    "paths": { "$ref": "#/$defs/StringListOrReset" },
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
                    "include": { "$ref": "#/$defs/StringListOrReset" },
                    "extra_patterns": { "$ref": "#/$defs/StringListOrReset" },
                    "allow_patterns": { "$ref": "#/$defs/StringListOrReset" },
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
                    "allowed_users": { "$ref": "#/$defs/StringListOrReset" },
                    "hostname_denylist": { "$ref": "#/$defs/StringListOrReset" },
                    "extra_patterns": { "$ref": "#/$defs/StringListOrReset" },
                    "allow_patterns": { "$ref": "#/$defs/StringListOrReset" },
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
                    "paths": { "$ref": "#/$defs/StringListOrReset" }
                }
            },
            "GoldenGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "paths": { "$ref": "#/$defs/StringListOrReset" },
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
                    "paths": { "$ref": "#/$defs/StringListOrReset" },
                    "provenance": { "type": "string", "description": "Expected host/runner provenance tag for benchmark artifacts" },
                    "allow_cross_host": { "type": "boolean", "description": "Allow benchmark comparison across mismatched host/runner provenance" },
                    "base_file": { "type": "string", "description": "In-job base benchmark result file path for dual-file regression checks" },
                    "head_file": { "type": "string", "description": "In-job head benchmark result file path for dual-file regression checks" },
                    "noise_floor_pct": { "type": "number", "description": "Noise floor percentage (default: 0.5%)" },
                    "advisory_pct": { "type": "number", "description": "Advisory review percentage (default: 0.1%)" },
                    "exempt_arms": { "$ref": "#/$defs/StringListOrReset", "description": "Declared exempt benchmark arms" },
                    "require_sourced_override": { "type": "boolean", "description": "Require allow-regression reasons to cite a CI run URL or artifact path and name the arms" }
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
                    "check_paired_figures": { "type": "boolean", "description": "Check paired figures for shared workload IDs or differentiation tags" }
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
            }
        }
    })
}
