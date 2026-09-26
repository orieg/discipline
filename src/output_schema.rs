//! JSON Schemas for what discipline prints: the `check --format json` report and the
//! `replay --json` summary.
//!
//! Both are part of the 1.0 interface (docs/ARCHITECTURE.md §3.2). `discipline docs`
//! writes them to `discipline.report.schema.json` and `discipline.replay.schema.json`;
//! `tests/test_output_schemas.rs` pins every field's path and type and validates real
//! output against them, so a renamed, removed or retyped field fails a test.

use serde_json::{json, Value};

const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// Schema of `discipline check --format json` (and of `--json-out`).
pub fn report_schema() -> Value {
    json!({
        "$schema": DRAFT,
        "title": "DisciplineReport",
        "description": "The report `discipline check --format json` prints on stdout and `--json-out` writes. Exit 0 means no finding blocks, 1 that one does, 2 that the check could not run: then stdout carries no report, and `--json-out` gets this shape with a single `engine` outcome whose violation holds the error.",
        "type": "object",
        "additionalProperties": false,
        "required": ["base", "errors", "warnings", "notes", "overrides", "baselined", "outcomes", "planned_gates"],
        "properties": {
            "base": { "type": "string", "description": "The ref the change was measured against, as resolved (e.g. `origin/main (merge base 1a2b3c4d5e)`)" },
            "errors": { "type": "integer", "minimum": 0, "description": "Findings of severity `error` across all gates" },
            "warnings": { "type": "integer", "minimum": 0, "description": "Findings of severity `warning`" },
            "notes": { "type": "integer", "minimum": 0, "description": "Findings of severity `note`" },
            "overrides": { "type": "integer", "minimum": 0, "description": "Directive overrides applied across all gates" },
            "baselined": { "type": "integer", "minimum": 0, "description": "Findings suppressed by the baseline file" },
            "outcomes": { "type": "array", "items": { "$ref": "#/$defs/GateOutcome" }, "description": "One entry per gate, in registry order" },
            "planned_gates": { "type": "array", "items": { "type": "string" }, "description": "Gate ids the roadmap plans but this binary does not ship" },
            "policy_failures": { "type": "array", "items": { "type": "string" }, "description": "Run-level refusals no single gate owns; any entry fails the run. Omitted when empty" },
            "deprecations": { "type": "array", "items": { "type": "string" }, "description": "Deprecated configuration keys this run read, one note each; never fails the run. Omitted when empty" }
        },
        "$defs": {
            "GateOutcome": {
                "type": "object",
                "additionalProperties": false,
                "required": ["gate", "suite", "enabled", "examined", "inline_exemptions", "baselined", "notes", "violations", "overrides"],
                "properties": {
                    "gate": { "type": "string", "description": "Stable kebab-case gate id (`engine` for a run that could not start)" },
                    "suite": { "type": "string", "description": "The suite the gate belongs to" },
                    "enabled": { "type": "boolean" },
                    "examined": { "type": "integer", "minimum": 0, "description": "Items the gate inspected" },
                    "inline_exemptions": { "type": "integer", "minimum": 0 },
                    "baselined": { "type": "integer", "minimum": 0 },
                    "notes": { "type": "array", "items": { "type": "string" }, "description": "What the gate could not verify, and other context" },
                    "violations": { "type": "array", "items": { "$ref": "#/$defs/Violation" } },
                    "overrides": { "type": "array", "items": { "$ref": "#/$defs/Override" } }
                }
            },
            "Violation": {
                "type": "object",
                "additionalProperties": false,
                "required": ["gate", "code", "fingerprint", "severity", "title", "file", "line", "message", "remediation"],
                "properties": {
                    "gate": { "type": "string" },
                    "code": { "type": "string", "pattern": "^[a-z0-9-]+/[a-z0-9-]+$", "description": "`gate/code`: the finding's stable identity, frozen from 1.0 (the registry in `src/findings.rs`)" },
                    "fingerprint": { "type": "string", "pattern": "^([0-9a-f]{64})?$", "description": "The baseline fingerprint (version 2: sha256 of code, path and the source line or message), stable across line moves and title changes; empty for the `engine` finding of a run that could not complete" },
                    "severity": { "enum": ["error", "warning", "note"] },
                    "title": { "type": "string", "description": "Display text, free to change" },
                    "file": { "type": ["string", "null"] },
                    "line": { "type": ["integer", "null"], "minimum": 0 },
                    "message": { "type": "string" },
                    "remediation": { "type": ["string", "null"] }
                }
            },
            "Override": {
                "type": "object",
                "additionalProperties": false,
                "required": ["gate", "subject", "directive", "reason", "source", "hidden"],
                "properties": {
                    "gate": { "type": "string" },
                    "subject": { "type": "string", "description": "What the directive names: a test, path, rule, dependency, ..." },
                    "directive": { "type": "string" },
                    "reason": { "type": "string" },
                    "source": { "$ref": "#/$defs/OverrideSource" },
                    "hidden": { "type": "boolean", "description": "The directive was inside an HTML comment" }
                }
            },
            "OverrideSource": {
                "oneOf": [
                    {
                        "type": "object", "additionalProperties": false, "required": ["type"],
                        "properties": { "type": { "const": "PrBody" } }
                    },
                    {
                        "type": "object", "additionalProperties": false, "required": ["type", "detail"],
                        "properties": { "type": { "const": "Commit" }, "detail": { "type": "string", "description": "Commit id" } }
                    },
                    {
                        "type": "object", "additionalProperties": false, "required": ["type", "detail"],
                        "properties": {
                            "type": { "const": "Inline" },
                            "detail": {
                                "type": "object", "additionalProperties": false, "required": ["file", "line"],
                                "properties": { "file": { "type": "string" }, "line": { "type": "integer", "minimum": 0 } }
                            }
                        }
                    },
                    {
                        "type": "object", "additionalProperties": false, "required": ["type", "detail"],
                        "properties": { "type": { "const": "MergedPrBody" }, "detail": { "type": "integer", "minimum": 1, "description": "Pull request number" } }
                    }
                ]
            }
        }
    })
}

/// Schema of `discipline replay --json`.
pub fn replay_schema() -> Value {
    let change_lists = json!({
        "type": "object",
        "additionalProperties": { "type": "array", "items": { "type": "string" } }
    });
    json!({
        "$schema": DRAFT,
        "title": "DisciplineReplay",
        "description": "The summary `discipline replay --json` prints: each replayed change's verdict under the configuration, and per-gate counts. Changes are labelled `#N` (pull request) or by a 10-character commit id.",
        "type": "object",
        "additionalProperties": false,
        "required": ["cases", "passed", "blocked", "could_not_check", "errors_by_gate", "refused_overrides_by_gate", "warnings_by_gate", "could_not_check_by_reason", "cases_detail"],
        "properties": {
            "cases": { "type": "integer", "minimum": 0 },
            "passed": { "type": "integer", "minimum": 0 },
            "blocked": { "type": "integer", "minimum": 0 },
            "could_not_check": { "type": "integer", "minimum": 0 },
            "errors_by_gate": { "description": "Gate id -> the changes it blocked with an error finding", "$ref": "#/$defs/ChangeLists" },
            "refused_overrides_by_gate": { "description": "Gate id -> the changes whose override of that gate `fail_on_overrides` refused", "$ref": "#/$defs/ChangeLists" },
            "warnings_by_gate": {
                "description": "Gate id -> the number of changes with a warning from it",
                "type": "object",
                "additionalProperties": { "type": "integer", "minimum": 0 }
            },
            "could_not_check_by_reason": { "description": "The error a change stopped on -> the changes", "$ref": "#/$defs/ChangeLists" },
            "cases_detail": { "type": "array", "items": { "$ref": "#/$defs/Case" }, "description": "Newest first" }
        },
        "$defs": {
            "ChangeLists": change_lists,
            "Case": {
                "type": "object",
                "additionalProperties": false,
                "required": ["sha", "pr", "subject", "verdict", "blocking_gates", "refused_overrides", "actor", "warning_gates", "directives_from"],
                "properties": {
                    "sha": { "type": "string", "description": "Full commit id of the replayed change" },
                    "pr": { "type": ["integer", "null"], "minimum": 1, "description": "Pull request number, from the forge or the subject's `(#N)`" },
                    "subject": { "type": "string" },
                    "verdict": { "enum": ["passed", "blocked", "could_not_check"] },
                    "blocking_gates": { "type": "array", "items": { "type": "string" }, "description": "Gates with an `error` finding" },
                    "refused_overrides": { "type": "array", "items": { "type": "string" }, "description": "Gates whose overrides `fail_on_overrides` refused" },
                    "actor": { "type": ["string", "null"], "description": "The login the change was checked as: its merged pull request's author" },
                    "warning_gates": { "type": "array", "items": { "type": "string" } },
                    "directives_from": { "type": "string", "description": "`pull request body`, or why only the commit message was read" },
                    "detail": { "type": "string", "description": "Why the change could not be checked. Omitted otherwise" }
                }
            }
        }
    })
}
