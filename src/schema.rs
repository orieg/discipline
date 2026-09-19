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
                    "paths": { "$ref": "#/$defs/StringListOrReset" }
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
            }
        }
    })
}
