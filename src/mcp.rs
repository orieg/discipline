//! `discipline mcp`: a Model Context Protocol server over stdio.
//!
//! Newline-delimited JSON-RPC 2.0 on stdin / stdout, no socket and no network. Three
//! read-only tools let an MCP-capable agent ask the gates before it commits:
//!
//! * `check_diff`: the change so far, as the `agent-prompt` report (repair text,
//!   never waiver syntax), with the verdict in `structuredContent`.
//! * `list_gates`: every gate, its state and severity under the repository's
//!   configuration (the `gates` subcommand's table).
//! * `explain_finding`: what a gate checks and why, from the gate registry.
//!
//! No tool writes a file, a directive or a baseline. The checks run this binary as a
//! child, exactly as `discipline hook run` does.

use crate::hook::CheckSide;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

/// Protocol versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The child processes a tool call needs; swapped for a fake in unit tests.
pub trait Runner {
    /// A check of the change.
    fn check(&self, side: &CheckSide) -> Result<crate::hook::CheckRun>;
    /// The `gates` table.
    fn gates(&self) -> Result<String>;
}

/// Runs this binary in the current directory.
pub struct ChildRunner;

impl Runner for ChildRunner {
    fn check(&self, side: &CheckSide) -> Result<crate::hook::CheckRun> {
        let side = match side {
            CheckSide::Default => crate::gitctx::discover_repository(".")
                .ok()
                .and_then(|r| crate::hook::default_base(&r))
                .map(CheckSide::Base)
                .unwrap_or(CheckSide::Default),
            other => other.clone(),
        };
        crate::hook::run_check(Path::new("."), &side)
    }

    fn gates(&self) -> Result<String> {
        let exe = std::env::current_exe()?;
        let out = std::process::Command::new(exe).arg("gates").output()?;
        if !out.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn tools() -> Value {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    json!([
        {
            "name": "check_diff",
            "title": "Check the change",
            "description": "Run discipline's gates on the change so far and return each finding with its location and the repair. The working tree (committed on the branch and uncommitted) is measured against the merge base with the default branch, under the default branch's configuration. Call it before committing; fix every finding it reports.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "outputSchema": crate::output_schema::mcp_check_schema(),
            "annotations": read_only
        },
        {
            "name": "list_gates",
            "title": "List the gates",
            "description": "List every discipline gate with its suite, whether it is on in this repository, its severity and a one-line summary.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only
        },
        {
            "name": "explain_finding",
            "title": "Explain a gate",
            "description": "Explain what a discipline gate checks: pass a gate id (`assertion-reduction`) or a finding line containing `[gate-id]`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "A gate id, or a finding line that names one in brackets." }
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "annotations": read_only
        }
    ])
}

/// The gate a query names (shared with `discipline explain`).
pub fn gate_for(query: &str) -> Option<&'static crate::config::GateInfo> {
    crate::explain::gate_for(query)
}

fn explain(query: &str) -> (String, bool) {
    match gate_for(query) {
        Some(g) => (
            format!(
                "{id} ({suite}{planned})\nChecks: {summary}.\nLanguages: {langs}.\nReference: https://orieg.github.io/discipline/gates/#{id}\nTo resolve a finding, change the code so the rule holds; `check_diff` names the repair for each one.\n",
                id = g.id,
                suite = g.suite.label(),
                planned = if g.available { "" } else { ", planned, not in this binary" },
                summary = g.summary,
                langs = g.languages,
            ),
            false,
        ),
        None => {
            let near = crate::explain::suggestions(query);
            let hint = if near.is_empty() {
                String::new()
            } else {
                format!(" Did you mean: {}?", near.join(", "))
            };
            (
                format!("No discipline gate matches `{query}`.{hint} `list_gates` lists every gate id."),
                true,
            )
        }
    }
}

fn text_result(text: String, is_error: bool, structured: Option<Value>) -> Value {
    // Agent-facing output never carries waiver syntax.
    let text = crate::report::scrub_override_directives(&text);
    let mut r = json!({ "content": [{ "type": "text", "text": text }], "isError": is_error });
    if let Some(s) = structured {
        r["structuredContent"] = s;
    }
    r
}

/// The findings of a check's JSON report as `check_diff` returns them: no remediation
/// (it can name a waiver), the repair instead, every text scrubbed of waiver syntax.
fn findings(report: Option<&Value>) -> Vec<Value> {
    let scrub = |v: &Value| crate::report::scrub_override_directives(v.as_str().unwrap_or(""));
    report
        .and_then(|r| r["outcomes"].as_array())
        .into_iter()
        .flatten()
        .flat_map(|o| o["violations"].as_array().into_iter().flatten())
        .map(|v| {
            let code = v["code"].as_str().unwrap_or("");
            let gate = v["gate"].as_str().unwrap_or("");
            json!({
                "code": code,
                "severity": v["severity"],
                "title": scrub(&v["title"]),
                "file": v["file"],
                "line": v["line"],
                "message": scrub(&v["message"]),
                "repair": crate::report::repair_for(code, gate, v["remediation"].as_str()),
                "fingerprint": v["fingerprint"],
            })
        })
        .collect()
}

fn check_content(status: &str, run: Option<&crate::hook::CheckRun>) -> Value {
    json!({
        "schema_version": crate::output_schema::MCP_CHECK_SCHEMA_VERSION,
        "status": status,
        "findings": findings(run.and_then(|r| r.json.as_ref())),
    })
}

fn call_tool(runner: &dyn Runner, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match name {
        "check_diff" => {
            // No argument chooses what is compared: an agent that could name `HEAD` as the
            // base would judge its committed change by its own configuration.
            Ok(match runner.check(&CheckSide::Default) {
                Ok(run) if run.code == 0 => {
                    let content = check_content("pass", Some(&run));
                    let text = if run.report.trim().is_empty() {
                        "No discipline findings in this change.".to_string()
                    } else {
                        run.report
                    };
                    text_result(text, false, Some(content))
                }
                Ok(run) if run.code == 1 => {
                    let content = check_content("findings", Some(&run));
                    text_result(run.report, false, Some(content))
                }
                Ok(run) => {
                    // The reason is the report's `could_not_check.reason`; a child that
                    // wrote no report did not start.
                    let reason = run
                        .could_not_check()
                        .and_then(|c| c.get("reason"))
                        .and_then(Value::as_str)
                        .unwrap_or(crate::could_not_check::Reason::Internal.as_str())
                        .to_string();
                    let gate = run
                        .could_not_check()
                        .and_then(|c| c.get("gate"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    text_result(
                        format!(
                            "discipline could not check this change, so it is not known to be safe:\n{}",
                            run.stderr.trim()
                        ),
                        true,
                        Some(json!({
                            "schema_version": crate::output_schema::MCP_CHECK_SCHEMA_VERSION,
                            "status": "could_not_check",
                            "reason": reason,
                            "gate": gate
                        })),
                    )
                }
                Err(e) => text_result(
                    format!("discipline could not check this change: {e:#}"),
                    true,
                    Some(json!({
                        "schema_version": crate::output_schema::MCP_CHECK_SCHEMA_VERSION,
                        "status": "could_not_check",
                        "reason": crate::could_not_check::Reason::Internal.as_str(),
                        "gate": null
                    })),
                ),
            })
        }
        "list_gates" => Ok(match runner.gates() {
            Ok(t) => text_result(t, false, None),
            Err(e) => text_result(format!("cannot list gates: {e:#}"), true, None),
        }),
        "explain_finding" => {
            let q = args.get("query").and_then(Value::as_str).unwrap_or("");
            let (t, err) = explain(q);
            Ok(text_result(t, err, None))
        }
        other => Err((-32602, format!("unknown tool `{other}`"))),
    }
}

/// Answers one JSON-RPC message; `None` for a notification.
pub fn handle(runner: &dyn Runner, line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } }),
            )
        }
    };
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?; // notifications (`notifications/initialized`, ...) get no reply
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|v| PROTOCOL_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "discipline", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Call check_diff before committing and fix every finding it reports."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(runner, &params),
        other => Err((-32601, format!("method not found: `{other}`"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

/// Serves until `input` closes.
pub fn serve(runner: &dyn Runner, input: impl BufRead, mut output: impl Write) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(runner, &line) {
            writeln!(output, "{}", serde_json::to_string(&reply)?)?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(i32, &'static str);
    impl Runner for Fake {
        fn check(&self, _: &CheckSide) -> Result<crate::hook::CheckRun> {
            Ok(crate::hook::CheckRun {
                code: self.0,
                report: self.1.to_string(),
                stderr: "config does not parse".to_string(),
                json: (self.0 == 2).then(|| {
                    json!({ "could_not_check": { "reason": "configuration", "gate": null, "detail": "config does not parse" } })
                }),
            })
        }
        fn gates(&self) -> Result<String> {
            Ok("GATE SUITE\nassertion-reduction agent-guard\n".to_string())
        }
    }

    fn call(r: &dyn Runner, tool: &str, args: Value) -> Value {
        let line = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":tool,"arguments":args}});
        handle(r, &line.to_string()).unwrap()
    }

    #[test]
    fn initialize_negotiates_and_notifications_get_no_reply() {
        let r = Fake(0, "");
        let init = handle(&r, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let newer = handle(&r, r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#).unwrap();
        assert_eq!(newer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        assert!(handle(
            &r,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
        assert_eq!(
            handle(&r, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#).unwrap()["error"]["code"],
            -32601
        );
        assert_eq!(handle(&r, "{not json").unwrap()["error"]["code"], -32700);
    }

    #[test]
    fn three_read_only_tools_are_listed() {
        let list = handle(
            &Fake(0, ""),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        )
        .unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["check_diff", "list_gates", "explain_finding"]);
        assert!(tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true));
    }

    #[test]
    fn check_diff_reports_findings_and_could_not_check_is_an_error() {
        let report = "### Issue 1 [assertion-reduction]: x\n- Repair: restore it\n";
        let found = call(&Fake(1, report), "check_diff", json!({}));
        assert_eq!(found["result"]["structuredContent"]["status"], "findings");
        assert_eq!(found["result"]["isError"], false);
        assert!(found["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Repair:"));
        let pass = call(
            &Fake(0, "No discipline violations found"),
            "check_diff",
            json!({}),
        );
        assert_eq!(pass["result"]["structuredContent"]["status"], "pass");
        let broken = call(&Fake(2, ""), "check_diff", json!({"staged": true}));
        assert_eq!(broken["result"]["isError"], true);
        assert_eq!(
            broken["result"]["structuredContent"]["status"],
            "could_not_check"
        );
        assert_eq!(
            broken["result"]["structuredContent"]["reason"],
            "configuration"
        );
        assert!(broken["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("config does not parse"));
    }

    #[test]
    fn findings_carry_the_repair_and_no_waiver_syntax_in_any_field() {
        // A finding's own text can quote a directive (a smuggled one, or the source line).
        let report = json!({"outcomes": [{"gate": "assertion-reduction", "violations": [{
            "gate": "assertion-reduction",
            "code": "assertion-reduction/assertions-reduced",
            "fingerprint": "",
            "severity": "error",
            "title": "Assertions Reduced near allow-assertion-drop:",
            "file": "tests/a.rs",
            "line": 3,
            "message": "the body says `allow-assertion-drop: t flaky` and `discipline:allow(x)`",
            "remediation": "Restore it, or justify with `allow-assertion-drop: <test> <reason>`"
        }]}]});
        let got = findings(Some(&report));
        assert_eq!(got.len(), 1);
        let f = &got[0];
        assert_eq!(f["code"], "assertion-reduction/assertions-reduced");
        assert_eq!(f["line"], 3);
        assert!(f.get("remediation").is_none(), "{f}");
        assert!(f["repair"].as_str().unwrap().starts_with("Restore"), "{f}");
        let text = f.to_string();
        assert!(
            !text.contains("allow-assertion-drop") && !text.contains("discipline:allow"),
            "{text}"
        );
        assert!(findings(None).is_empty());
    }

    #[test]
    fn explain_resolves_an_id_or_a_finding_line_and_never_names_a_waiver() {
        assert_eq!(
            gate_for("assertion-reduction").unwrap().id,
            "assertion-reduction"
        );
        assert_eq!(
            gate_for("### Issue 1 [error-swallowing]: Result Discarded")
                .unwrap()
                .id,
            "error-swallowing"
        );
        assert!(gate_for("no such thing").is_none());
        let r = Fake(0, "");
        for g in crate::config::GATES {
            let out = call(&r, "explain_finding", json!({"query": g.id}));
            let text = out["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.starts_with(g.id), "{text}");
            assert!(
                !text.contains("allow-") && !text.contains("discipline:allow"),
                "{text}"
            );
        }
        let missing = call(&r, "explain_finding", json!({"query": "zzz"}));
        assert_eq!(missing["result"]["isError"], true);
        assert_eq!(call(&r, "nope", json!({}))["error"]["code"], -32602);
    }

    #[test]
    fn a_waiver_in_a_report_is_scrubbed_before_it_reaches_the_agent() {
        let leaky = "### Issue 1 [x]: y\n- Repair: allow-assertion-drop: t reason\n";
        let out = call(&Fake(1, leaky), "check_diff", json!({}));
        let text = out["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("allow-assertion-drop"), "{text}");
    }
}
