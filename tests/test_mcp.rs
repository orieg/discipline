//! `discipline mcp` over real stdio: an MCP client asks the gates about a change.

mod common;

use common::{Repo, CONFIG_HEAD};
use serde_json::{json, Value};
use std::io::Write;
use std::process::Stdio;

fn session(repo: &Repo, messages: &[Value]) -> Vec<Value> {
    let mut cmd = common::discipline_cmd(repo.path());
    cmd.arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for m in messages {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn call(id: u64, tool: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}})
}

#[test]
fn an_mcp_client_checks_a_weakened_test_and_never_sees_a_waiver() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    let replies = session(
        &repo,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            call(3, "check_diff", json!({})),
            call(4, "list_gates", json!({})),
            call(
                5,
                "explain_finding",
                json!({"query": "### Issue 1 [assertion-reduction]: x"}),
            ),
        ],
    );
    // One reply per request; the notification gets none.
    let ids: Vec<u64> = replies.iter().map(|r| r["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 3);

    let check = &replies[2]["result"];
    assert_eq!(check["structuredContent"]["status"], "findings", "{check}");
    let text = check["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("[assertion-reduction/assertions-reduced]") && text.contains("Repair:"),
        "{text}"
    );
    // The same findings as data: the code, the location and the repair; no remediation,
    // which can name a waiver.
    let findings = check["structuredContent"]["findings"].as_array().unwrap();
    let reduced = findings
        .iter()
        .find(|f| f["code"] == "assertion-reduction/assertions-reduced")
        .unwrap_or_else(|| panic!("{check}"));
    assert_eq!(reduced["file"], "tests/a.rs", "{reduced}");
    assert_eq!(reduced["severity"], "error");
    let repair = reduced["repair"].as_str().unwrap();
    assert!(
        repair.len() > 20 && text.contains(&format!("- Repair: {repair}")),
        "the structured repair is the one the text gives: {reduced}"
    );
    assert!(reduced.get("remediation").is_none(), "{reduced}");
    assert_eq!(check["structuredContent"]["schema_version"], 1);
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools[0]["outputSchema"]["title"], "DisciplineCheckDiff",
        "check_diff declares its structuredContent"
    );

    let gates = replies[3]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(gates.contains("assertion-reduction"), "{gates}");
    let explained = replies[4]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(explained.starts_with("assertion-reduction"), "{explained}");

    for r in &replies {
        let s = r.to_string();
        assert!(
            !s.contains("allow-assertion-drop") && !s.contains("discipline:allow"),
            "waiver syntax reached the client: {s}"
        );
    }
}

/// #362: a directive hidden in an HTML comment in a commit is refused, and its reason, which
/// a reviewer cannot see, never reaches the client: not in the structured content, not in
/// the text.
#[test]
fn check_diff_never_returns_a_hidden_directive_reason() {
    let repo = Repo::new();
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2.\nx\n");
    repo.commit("docs: wording\n\n<!-- no-issue: MCP-MARKER-362 -->");
    let replies = session(&repo, &[call(1, "check_diff", json!({}))]);
    let reply = replies[0].to_string();
    assert!(
        replies[0]["result"]["structuredContent"].is_object(),
        "{reply}"
    );
    assert!(!reply.contains("MCP-MARKER-362"), "{reply}");
}

#[test]
fn a_clean_change_passes_and_a_broken_config_is_an_error_not_a_pass() {
    let repo = Repo::new();
    let pass = session(&repo, &[call(1, "check_diff", json!({}))]);
    assert_eq!(
        pass[0]["result"]["structuredContent"]["status"], "pass",
        "{}",
        pass[0]
    );
    assert_eq!(
        pass[0]["result"]["structuredContent"]["findings"],
        json!([]),
        "{}",
        pass[0]
    );

    repo.write("discipline.toml", "[meta\n");
    let broken = session(&repo, &[call(1, "check_diff", json!({}))]);
    assert_eq!(broken[0]["result"]["isError"], true, "{}", broken[0]);
    assert_eq!(
        broken[0]["result"]["structuredContent"]["status"],
        "could_not_check"
    );
    assert!(
        broken[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("(reason: configuration)"),
        "{}",
        broken[0]
    );
}

#[test]
fn check_diff_ignores_an_agent_chosen_base_and_the_changes_own_config() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.assertion-reduction]\nenabled = false\n"),
    );
    repo.commit("test: simplify");
    let replies = session(
        &repo,
        &[call(
            1,
            "check_diff",
            json!({"base": "HEAD", "staged": true}),
        )],
    );
    assert_eq!(
        replies[0]["result"]["structuredContent"]["status"], "findings",
        "{}",
        replies[0]
    );
}

fn session_at(dir: &std::path::Path, messages: &[Value]) -> Vec<Value> {
    let mut cmd = common::discipline_cmd(dir);
    cmd.arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for m in messages {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// #476: on a shallow clone with committed changes where origin/main does not exist,
/// check_diff answers could_not_check with isError: true, never status: "pass".
#[test]
fn check_diff_on_shallow_clone_refuses_when_base_cannot_measure_change() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n",
    );
    repo.commit("test: drop assertion");

    let tmp = tempfile::tempdir().unwrap();
    let shallow = tmp.path().join("shallow");
    let repo_url = format!("file://{}", repo.path().display());
    let clone_status = common::git_command()
        .args([
            "clone",
            "-q",
            "--depth",
            "1",
            "--branch",
            "work",
            &repo_url,
            shallow.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(clone_status.success());

    let replies = session_at(&shallow, &[call(1, "check_diff", json!({}))]);
    let result = &replies[0]["result"];
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["status"], "could_not_check",
        "{result}"
    );
    assert_eq!(
        result["structuredContent"]["reason"], "repository",
        "{result}"
    );
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("base ref `origin/main` does not resolve"),
        "{text}"
    );
}
