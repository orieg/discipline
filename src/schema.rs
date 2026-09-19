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
                    "assert_helper_fns": { "$ref": "#/$defs/StringListOrReset" }
                }
            },
            "DeletionGate": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "enabled": { "type": "boolean", "description": "Whether this gate is active" },
                    "severity": { "$ref": "#/$defs/Severity" },
                    "exempt_paths": { "$ref": "#/$defs/StringListOrReset" },
                    "paths": { "$ref": "#/$defs/StringListOrReset" }
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
                    "scan_pr_body": { "type": "boolean", "description": "Whether to scan PR description text" }
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
                    "scan_pr_body": { "type": "boolean", "description": "Whether to scan PR description text" }
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
                    "paths": { "$ref": "#/$defs/StringListOrReset" }
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
                    "paths": { "$ref": "#/$defs/StringListOrReset" },
                    "provenance": { "type": "string", "description": "Expected host/runner provenance tag for benchmark artifacts" },
                    "allow_cross_host": { "type": "boolean", "description": "Allow benchmark comparison across mismatched host/runner provenance" }
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
            }
        }
    })
}
