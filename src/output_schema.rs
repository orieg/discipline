//! JSON Schemas for what discipline prints: the `check --format json` report and the
//! `replay --json` summary.
//!
//! Both are part of the 1.0 interface (docs/ARCHITECTURE.md §3.2). `discipline docs`
//! writes them to `discipline.report.schema.json` and `discipline.replay.schema.json`;
//! `tests/test_output_schemas.rs` pins every field's path and type and validates real
//! output against them, so a renamed, removed or retyped field fails a test.

use serde_json::{json, Value};

const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// `schema_version` of the check report. Adding a field keeps it; renaming, removing or
/// retyping one raises it.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// `schema_version` of the replay summary, under the same rule.
pub const REPLAY_SCHEMA_VERSION: u32 = 1;

/// Version of the `discipline audit --json` output; see [`REPORT_SCHEMA_VERSION`].
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

/// `schema_version` of the MCP `check_diff` tool's `structuredContent`, under the same rule.
pub const MCP_CHECK_SCHEMA_VERSION: u32 = 1;

fn reasons() -> Vec<&'static str> {
    crate::could_not_check::Reason::ALL
        .iter()
        .map(|r| r.as_str())
        .collect()
}

/// Schema of `discipline check --format json` (and of `--json-out`).
pub fn report_schema() -> Value {
    json!({
        "$schema": DRAFT,
        "title": "DisciplineReport",
        "description": "The report `discipline check --format json` prints on stdout and `--json-out` writes. Exit 0 means no finding blocks, 1 that one does, 2 that the check could not run: then the report has no outcomes and `could_not_check` says why.",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "base", "errors", "warnings", "notes", "overrides", "baselined", "outcomes", "planned_gates"],
        "properties": {
            "schema_version": { "const": REPORT_SCHEMA_VERSION, "description": "This schema's version: a field added keeps it, one renamed, removed or retyped raises it" },
            "base": { "type": "string", "description": "The ref the change was measured against, as resolved (e.g. `origin/main (merge base 1a2b3c4d5e)`)" },
            "errors": { "type": "integer", "minimum": 0, "description": "Findings of severity `error` across all gates" },
            "warnings": { "type": "integer", "minimum": 0, "description": "Findings of severity `warning`" },
            "notes": { "type": "integer", "minimum": 0, "description": "Findings of severity `note`" },
            "overrides": { "type": "integer", "minimum": 0, "description": "Directive overrides applied across all gates" },
            "baselined": { "type": "integer", "minimum": 0, "description": "Findings suppressed by the baseline file" },
            "outcomes": { "type": "array", "items": { "$ref": "#/$defs/GateOutcome" }, "description": "One entry per gate, in registry order" },
            "planned_gates": { "type": "array", "items": { "type": "string" }, "description": "Gate ids the roadmap plans but this binary does not ship" },
            "policy_failures": { "type": "array", "items": { "type": "string" }, "description": "Run-level refusals no single gate owns; any entry fails the run. Omitted when empty" },
            "deprecations": { "type": "array", "items": { "type": "string" }, "description": "Deprecated configuration keys this run read, one note each; never fails the run. Omitted when empty" },
            "unused_directives": { "type": "array", "items": { "$ref": "#/$defs/UnusedDirective" }, "description": "Directives this run read that lifted no finding; never fails the run. Computed when every suite ran. Omitted when empty" },
            "could_not_check": { "$ref": "#/$defs/CouldNotCheck", "description": "Why the check could not run (exit 2). Omitted otherwise" }
        },
        "$defs": {
            "CouldNotCheck": {
                "type": "object",
                "additionalProperties": false,
                "required": ["reason", "gate", "detail"],
                "properties": {
                    "reason": { "enum": reasons(), "description": "What stopped the check. A new kind of failure may get a new reason in a minor release; a failure that has one keeps it" },
                    "gate": { "type": ["string", "null"], "description": "The gate that could not run, when one did" },
                    "detail": { "type": "string", "description": "The error, as printed on stderr" }
                }
            },
            "GateOutcome": {
                "type": "object",
                "additionalProperties": false,
                "required": ["gate", "suite", "enabled", "examined", "inline_exemptions", "baselined", "notes", "violations", "overrides"],
                "properties": {
                    "gate": { "type": "string", "description": "Stable kebab-case gate id" },
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
                    "fingerprint": { "type": "string", "pattern": "^([0-9a-f]{64})?$", "description": "The baseline fingerprint (version 2: sha256 of code, path and the source line or message), stable across line moves and title changes" },
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
                    "code": { "type": "string", "pattern": "^[a-z0-9-]+/[a-z0-9-]+$", "description": "The finding the override lifted (`gate/code`, as a violation's `code`): what the run would have reported without the directive. Absent in reports written before it was recorded" },
                    "subject": { "type": "string", "description": "What the directive names: a test, path, rule, dependency, ..." },
                    "directive": { "type": "string" },
                    "reason": { "type": "string" },
                    "source": { "$ref": "#/$defs/OverrideSource" },
                    "hidden": { "type": "boolean", "description": "The directive was inside an HTML comment" }
                }
            },
            "UnusedDirective": {
                "type": "object",
                "additionalProperties": false,
                "required": ["directive", "source", "hidden"],
                "properties": {
                    "directive": { "type": "string" },
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

/// `outputSchema` of the MCP `check_diff` tool: its `structuredContent`. A finding is the
/// report's finding without `remediation`, which can name a waiver, and with `repair`,
/// the fix the `agent-prompt` report gives.
pub fn mcp_check_schema() -> Value {
    json!({
        "$schema": DRAFT,
        "title": "DisciplineCheckDiff",
        "description": "`structuredContent` of the MCP `check_diff` tool. `findings` is present when the check ran (`pass`, `findings`); `reason` and `gate` when it could not.",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "status"],
        "properties": {
            "schema_version": { "const": MCP_CHECK_SCHEMA_VERSION, "description": "This schema's version: a field added keeps it, one renamed, removed or retyped raises it" },
            "status": { "enum": ["pass", "findings", "could_not_check"] },
            "findings": { "type": "array", "items": { "$ref": "#/$defs/Finding" }, "description": "Every finding of the check, in report order" },
            "reason": { "enum": reasons(), "description": "Why the check could not run (`could_not_check` only)" },
            "gate": { "type": ["string", "null"], "description": "The gate that could not run (`could_not_check` only)" }
        },
        "$defs": {
            "Finding": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "severity", "title", "file", "line", "message", "repair", "fingerprint"],
                "properties": {
                    "code": { "type": "string", "pattern": "^[a-z0-9-]+/[a-z0-9-]+$", "description": "`gate/code`, as in the check report" },
                    "severity": { "enum": ["error", "warning", "note"] },
                    "title": { "type": "string" },
                    "file": { "type": ["string", "null"] },
                    "line": { "type": ["integer", "null"], "minimum": 0 },
                    "message": { "type": "string" },
                    "repair": { "type": "string", "description": "The fix; never a waiver" },
                    "fingerprint": { "type": "string", "pattern": "^([0-9a-f]{64})?$" }
                }
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
        "required": ["schema_version", "cases", "passed", "blocked", "could_not_check", "errors_by_gate", "refused_overrides_by_gate", "overrides_by_gate", "warnings_by_gate", "could_not_check_by_reason", "skipped_by_gate", "cases_detail"],
        "properties": {
            "schema_version": { "const": REPLAY_SCHEMA_VERSION, "description": "This schema's version: a field added keeps it, one renamed, removed or retyped raises it" },
            "cases": { "type": "integer", "minimum": 0 },
            "passed": { "type": "integer", "minimum": 0 },
            "blocked": { "type": "integer", "minimum": 0 },
            "could_not_check": { "type": "integer", "minimum": 0 },
            "errors_by_gate": { "description": "Gate id -> the changes it blocked with an error finding", "$ref": "#/$defs/ChangeLists" },
            "refused_overrides_by_gate": { "description": "Gate id -> the changes whose override of that gate `fail_on_overrides` refused", "$ref": "#/$defs/ChangeLists" },
            "overrides_by_gate": { "description": "Gate id -> the changes whose check applied an override of that gate (refused ones included)", "$ref": "#/$defs/ChangeLists" },
            "warnings_by_gate": {
                "description": "Gate id -> the number of changes with a warning from it",
                "type": "object",
                "additionalProperties": { "type": "integer", "minimum": 0 }
            },
            "could_not_check_by_reason": { "description": "The reason a change could not be checked (the report's `could_not_check.reason`) -> the changes", "$ref": "#/$defs/ChangeLists" },
            "skipped_by_gate": { "description": "Gate id -> the changes that skipped part of it because the configuration names a file the change does not have yet (the configuration is newer than the change)", "$ref": "#/$defs/ChangeLists" },
            "cases_detail": { "type": "array", "items": { "$ref": "#/$defs/Case" }, "description": "Newest first" }
        },
        "$defs": {
            "ChangeLists": change_lists,
            "Case": {
                "type": "object",
                "additionalProperties": false,
                "required": ["sha", "pr", "subject", "verdict", "blocking_gates", "refused_overrides", "overrides", "actor", "warning_gates", "findings", "directives_from", "skipped_checks"],
                "properties": {
                    "sha": { "type": "string", "description": "Full commit id of the replayed change" },
                    "pr": { "type": ["integer", "null"], "minimum": 1, "description": "Pull request number, from the forge or the subject's `(#N)`" },
                    "subject": { "type": "string" },
                    "verdict": { "enum": ["passed", "blocked", "could_not_check"] },
                    "blocking_gates": { "type": "array", "items": { "type": "string" }, "description": "Gates with an `error` finding" },
                    "refused_overrides": { "type": "array", "items": { "type": "string" }, "description": "Gates whose overrides `fail_on_overrides` refused" },
                    "overrides": { "type": "array", "items": { "$ref": "#/$defs/CaseOverride" }, "description": "Overrides the change's check applied (each lifted a finding), in report order; empty when the check itself could not run. The reason is never included: it is free text and can echo secret material" },
                    "actor": { "type": ["string", "null"], "description": "The login the change was checked as: its merged pull request's author" },
                    "warning_gates": { "type": "array", "items": { "type": "string" } },
                    "findings": { "type": "array", "items": { "$ref": "#/$defs/CaseFinding" }, "description": "Every error and warning of the change's report, in report order; empty when the check itself could not run. The message is never included: a finding can echo secret material" },
                    "directives_from": { "type": "string", "description": "`pull request body`, or why only the commit message was read" },
                    "skipped_checks": { "type": "array", "items": { "$ref": "#/$defs/SkippedCheck" }, "description": "Parts of gates skipped because the configuration names a file this change does not have yet; empty when the check itself could not run" },
                    "reason": { "enum": reasons(), "description": "Why the change could not be checked: the report's `could_not_check.reason`, or `forge` when its merged pull request could not be read. Omitted otherwise" },
                    "detail": { "type": "string", "description": "Why the change could not be checked. Omitted otherwise" }
                }
            },
            "CaseOverride": {
                "type": "object",
                "additionalProperties": false,
                "required": ["gate", "directive", "subject", "source", "hidden"],
                "properties": {
                    "gate": { "type": "string", "description": "Gate id" },
                    "code": { "type": "string", "pattern": "^[a-z0-9-]+/[a-z0-9-]+$", "description": "The finding the override lifted (`gate/code`); absent when the child report did not record it" },
                    "directive": { "type": "string", "description": "The directive's name, as written" },
                    "subject": { "type": "string", "description": "What the override covers: the path, test or dependency the finding named" },
                    "source": { "type": "string", "description": "Where the directive was read: `PR body` (under replay, the merged pull request's body), `commit <sha>`, `merged pull request #N body` or `inline <file>:<line>`" },
                    "hidden": { "type": "boolean", "description": "The directive was inside an HTML comment" }
                }
            },
            "SkippedCheck": {
                "type": "object",
                "additionalProperties": false,
                "required": ["gate", "what"],
                "properties": {
                    "gate": { "type": "string", "description": "Gate id" },
                    "what": { "type": "string", "description": "What was skipped, as the gate's note names it (for example ``group `versions` ``)" }
                }
            },
            "CaseFinding": {
                "type": "object",
                "additionalProperties": false,
                "required": ["code", "severity", "file", "line"],
                "properties": {
                    "code": { "type": "string", "pattern": "^[a-z0-9-]+/[a-z0-9-]+$", "description": "`gate/code`, as in the check report" },
                    "severity": { "enum": ["error", "warning"] },
                    "file": { "type": ["string", "null"] },
                    "line": { "type": ["integer", "null"], "minimum": 0 }
                }
            }
        }
    })
}

/// Schema of `discipline audit --json`.
pub fn audit_schema() -> Value {
    let counts = json!({
        "type": "object",
        "additionalProperties": { "type": "integer", "minimum": 0 }
    });
    let text = |d: &str| json!({ "type": "string", "description": d });
    json!({
        "$schema": DRAFT,
        "title": "DisciplineAudit",
        "description": "The records `discipline audit --json` prints: one per escape hatch a merged change carried, read from git objects only. A record says what was claimed or applied, not whether a check honoured it.",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "version", "reference", "tip", "links", "changes", "changes_with_records", "by_kind", "by_class", "by_gate", "signals", "checks", "records", "tightenings", "protected_edits", "pulls", "issues", "forge", "identities"],
        "properties": {
            "schema_version": { "const": AUDIT_SCHEMA_VERSION, "description": "This schema's version: a field added keeps it, one renamed, removed or retyped raises it" },
            "version": text("The discipline version that wrote the report"),
            "reference": text("The ref audited, as given or defaulted"),
            "tip": text("The commit the ref resolved to"),
            "links": {
                "description": "Web links from the `origin` remote's forge (GitHub, GitLab, Gitea, Forgejo); null when the remote names none. No request is made",
                "type": ["object", "null"],
                "additionalProperties": false,
                "required": ["repository", "pull", "commit", "file", "file_diff", "issue_comment", "issue"],
                "properties": {
                    "repository": text("The repository's web page"),
                    "pull": text("A pull request's page, `{n}` for its number"),
                    "commit": text("A commit's page, `{sha}` for its id"),
                    "file": text("A file at a commit: `{sha}`, `{path}`, `{line}` (drop `#L{line}` without a line)"),
                    "file_diff": { "type": ["string", "null"], "description": "One file's diff in a commit: `{sha}`, `{path_sha256}`; GitHub only" },
                    "issue_comment": text("A comment on an issue: `{repo}` (`owner/name`), `{n}`, `{id}`"),
                    "issue": text("An issue: `{repo}`, `{n}`")
                }
            },
            "changes": { "type": "integer", "minimum": 0, "description": "First-parent changes audited" },
            "changes_with_records": { "type": "integer", "minimum": 0, "description": "Changes that carried at least one record" },
            "by_kind": { "description": "Kind -> record count", "$ref": "#/$defs/Counts" },
            "by_class": { "description": "Class -> record count", "$ref": "#/$defs/Counts" },
            "by_gate": { "description": "Gate id (or configuration table) -> record count", "$ref": "#/$defs/Counts" },
            "signals": { "type": "array", "items": { "$ref": "#/$defs/Signal" }, "description": "Queries over the records that found something, most urgent first. Review prompts, not verdicts" },
            "checks": { "type": "array", "items": { "$ref": "#/$defs/Check" }, "description": "Every question the audit asks, with its state: a signal is `found` or `clean`; what git alone cannot tell is `not-checked`" },
            "records": { "type": "array", "items": { "$ref": "#/$defs/Record" }, "description": "Newest change first" },
            "tightenings": { "type": "array", "items": { "$ref": "#/$defs/Record" }, "description": "Tightenings of `discipline.toml` (`kind` `config-tightening`), newest first: `before` is the looser value" },
            "pulls": {
                "type": "array",
                "description": "Each change's merged pull request, read with `--forge`; empty otherwise. Logins are not carried",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sha", "pr", "approved_by_other"],
                    "properties": {
                        "sha": text("The change's commit"),
                        "pr": { "type": "integer", "minimum": 1 },
                        "approved_by_other": { "type": "boolean", "description": "A login other than the pull request's author approved its head" },
                        "body_edited_after_merge": { "type": "boolean", "description": "Its body was edited after the merge (GitHub's `lastEditedAt`), so the directives read from it now may not be the ones the gates read; absent when not known" }
                    }
                }
            },
            "identities": {
                "type": "array",
                "description": "For each change with a record or a protected edit, newest first: whether anything records that an agent made it. A missing record is `no-record`, never a person",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["sha", "state", "marker"],
                    "properties": {
                        "sha": text("The change's commit"),
                        "pr": { "type": "integer", "minimum": 1 },
                        "state": { "type": "string", "enum": ["agent-login", "claimed", "no-record"], "description": "`agent-login`: with `--forge`, its pull request was opened by an `agent_logins` or `[bot]` login; `claimed`: a commit carries a `commit-provenance` agent marker, asserted by the commit and verified by nothing; `no-record`: neither" },
                        "marker": { "type": "boolean", "description": "A `commit-provenance` agent marker matched the commit's trailers or author" },
                        "agent_login": { "type": "boolean", "description": "Its pull request's author is an agent login; absent without `--forge` or a pull request" }
                    }
                }
            },
            "issues": {
                "type": "array",
                "description": "The issues waivers cite, read with `--forge`; empty otherwise",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["reference", "repo", "number", "state"],
                    "properties": {
                        "reference": text("The reference as written in a reason"),
                        "repo": text("The repository it was looked up in; empty for another repository"),
                        "number": { "type": "integer", "minimum": 0 },
                        "state": { "enum": ["open", "closed", "not-found", "pull-request", "cross-repo", "not-checked"] },
                        "state_reason": text("GitHub's close reason (`completed`, `not_planned`, ...)"),
                        "closed_at": { "type": "integer", "description": "When it was closed, seconds since the Unix epoch" },
                        "error": text("Why the forge could not answer")
                    }
                }
            },
            "forge": {
                "type": ["object", "null"],
                "description": "What `--forge` read; null without it",
                "additionalProperties": false,
                "required": ["changes", "pulls", "failed"],
                "properties": {
                    "changes": { "type": "integer", "minimum": 0, "description": "Changes whose merged pull request was looked up" },
                    "pulls": { "type": "integer", "minimum": 0, "description": "Of those, the ones that arrived through one" },
                    "failed": { "type": "integer", "minimum": 0, "description": "Changes the forge could not answer for; any makes the forge checks `not-checked`" },
                    "error": text("The first error, when one failed")
                }
            },
            "protected_edits": { "type": "array", "items": { "$ref": "#/$defs/Record" }, "description": "Edits to paths the change's parent configuration protects under `ratified-paths` (`kind` `protected-edit`, `detail` `gate on` or `gate off`); with `--forge` each carries its `ratification`" }
        },
        "$defs": {
            "Counts": counts,
            "Signal": {
                "type": "object",
                "additionalProperties": false,
                "required": ["id", "rank", "count", "changes", "records", "list", "next"],
                "properties": {
                    "id": { "enum": ["guard-gate-loosened", "hidden-directive", "config-unreadable", "loosened-without-pull-request", "loosening-without-waiver", "waived-then-loosened", "loosened-not-restored", "baseline-grew", "protected-edit-unratified", "protected-edit-self-ratified", "waiver-cites-missing-issue", "waiver-cites-issue-closed-before", "waiver-cites-issue-not-planned", "waiver-lifted-nothing"] },
                    "rank": { "enum": ["look-first", "look-soon", "review"] },
                    "count": { "type": "integer", "minimum": 1, "description": "Records the signal is about" },
                    "changes": { "type": "array", "items": { "type": "string" }, "description": "The changes they are in, newest first (`#N`, else a 10-character commit id)" },
                    "records": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "description": "Indexes into the list `list` names" },
                    "list": { "enum": ["records", "protected_edits"], "description": "The list `records` indexes: `protected_edits` for the ratification signals" },
                    "next": text("The next action a reviewer takes")
                }
            },
            "Check": {
                "type": "object",
                "additionalProperties": false,
                "required": ["id", "state", "detail"],
                "properties": {
                    "id": text("A signal id, or a question git alone cannot answer"),
                    "state": { "enum": ["found", "clean", "not-checked"] },
                    "detail": text("What was found, or why it was not checked")
                }
            },
            "Record": {
                "type": "object",
                "additionalProperties": false,
                "required": ["sha", "pr", "time", "change_index", "subject", "kind", "class", "evidence", "tier", "gate"],
                "properties": {
                    "sha": text("Full commit id of the change"),
                    "pr": { "type": ["integer", "null"], "minimum": 1, "description": "Pull request number, from the subject's `(#N)`" },
                    "time": { "type": "integer", "description": "Commit time, seconds since the Unix epoch" },
                    "change_index": { "type": "integer", "minimum": 0, "description": "The change's position, 0 for the newest audited change" },
                    "subject": text("The change's subject line, as its author wrote it"),
                    "kind": { "enum": ["directive", "config", "config-unreadable", "baseline", "inline-marker", "config-tightening", "protected-edit"] },
                    "class": { "enum": ["process", "detector", "config", "baseline", "inline", "protected"], "description": "`process`: a waiver of a process rule (`no-issue`); `detector`: a waiver of a finding" },
                    "evidence": { "enum": ["claimed", "applied"], "description": "`claimed`: text that asks for an exception; `applied`: a tree change that is one" },
                    "tier": { "enum": ["A", "C"], "description": "Who controls the input: `A` git objects on the audited branch, `C` text the change's author wrote" },
                    "gate": { "type": ["string", "null"], "description": "Gate id, or the configuration table a loosening is under; null for a directive this binary does not map to a gate" },
                    "directive": text("The directive's name, as written"),
                    "key": text("The configuration option a loosening changed"),
                    "change": { "enum": ["changed", "removed", "increased", "decreased", "lowered", "gained", "lost", "emptied"] },
                    "before": text("The value before, as the `config-integrity` finding shows it"),
                    "after": text("The value after"),
                    "count": { "type": "integer", "minimum": 0, "description": "List entries gained or lost, or baseline findings added" },
                    "file": text("Where the exception is: the inline marker's file, the baseline, or `discipline.toml`"),
                    "line": { "type": "integer", "minimum": 1 },
                    "hidden": { "type": "boolean", "description": "The directive was inside an HTML comment" },
                    "reason_sha256": { "type": "string", "pattern": "^[0-9a-f]{64}$", "description": "SHA-256 of the directive's reason, to group reuse without the text" },
                    "reason_len": { "type": "integer", "minimum": 0 },
                    "reason": text("The directive's reason text: only under `--reasons`"),
                    "detail": text("Why a configuration or baseline could not be compared"),
                    "source": { "enum": ["commit-message", "pull-request-body"], "description": "Where a directive was read" },
                    "cites": { "type": "array", "items": { "type": "string" }, "description": "Issue references in a directive's reason, as written; omitted when none" },
                    "lifted": { "type": "boolean", "description": "With `--replay`, for a finding waiver: whether the replayed check applied it to lift a finding (then `evidence` is `applied`). Omitted when the replay did not judge the change, and for `no-issue`. An inline marker matches by file and line" },
                    "edited": { "type": "boolean", "description": "A loosening whose own change also tightened the same option: an edited entry, which `config-integrity` counts as lost unless it can prove it tighter. Omitted when false" },
                    "ratification": {
                        "type": "object",
                        "description": "A protected edit's ratification, judged as `ratified-paths` judges it, with `--forge`. The ratifier's login is not carried",
                        "additionalProperties": false,
                        "required": ["state"],
                        "properties": {
                            "state": { "enum": ["ratified", "self-ratified", "unratified", "not-required", "never-ratifiable", "not-checked"] },
                            "issue_repo": text("The repository of the issue carrying the ratifying comment"),
                            "issue": { "type": "integer", "minimum": 1 },
                            "comment_id": text("The ratifying comment's id"),
                            "created": { "type": "integer", "description": "When the ratifying comment was posted, seconds since the Unix epoch" },
                            "why": text("Why it is unratified or not checked")
                        }
                    }
                }
            }
        }
    })
}
