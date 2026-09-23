//! `discipline mcp` over real stdio: an MCP client asks the gates about a change.

mod common;

use common::Repo;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn session(repo: &Repo, messages: &[Value]) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_discipline"));
    cmd.arg("mcp")
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("DISCIPLINE_NO_NETWORK", "1");
    for var in common::ISOLATED_ENV_VARS {
        cmd.env_remove(var);
    }
    for (k, _) in std::env::vars() {
        if k.starts_with("DISCIPLINE_") && k != "DISCIPLINE_NO_NETWORK" {
            cmd.env_remove(k);
        }
    }
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
        text.contains("[assertion-reduction]") && text.contains("Repair:"),
        "{text}"
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

#[test]
fn a_clean_change_passes_and_a_broken_config_is_an_error_not_a_pass() {
    let repo = Repo::new();
    let pass = session(&repo, &[call(1, "check_diff", json!({}))]);
    assert_eq!(
        pass[0]["result"]["structuredContent"]["status"], "pass",
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
}
