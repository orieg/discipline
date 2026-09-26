//! The 1.0 interface (docs/ARCHITECTURE.md §3.2) as a test, not a promise.
//!
//! `tests/fixtures/v1_surface.json` lists every frozen name: subcommands and flags, the
//! `DISCIPLINE_*` variables, configuration keys, directives, gate ids, finding codes,
//! could-not-check reasons, the MCP tools and their arguments, hook agents, and the
//! action's inputs and outputs. Each is collected here from the code itself. A name the
//! fixture holds that the code no longer has (removed or renamed) fails the test; a new
//! name passes and is printed, and `DISCIPLINE_BLESS_SURFACE=1` adds it to the fixture.
//! The JSON output fields are pinned field by field in `tests/test_output_schemas.rs`.

use clap::CommandFactory;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// `DISCIPLINE_*` names the documentation mentions that are not an interface: set by
/// discipline for its own child processes.
const INTERNAL_ENV: &[&str] = &["DISCIPLINE_REPLAY_CASE"];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn cli(out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
    fn walk(cmd: &clap::Command, prefix: &str, out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
        for sub in cmd.get_subcommands() {
            let path = format!("{prefix}{}", sub.get_name()).trim().to_string();
            out.entry("cli.commands").or_default().insert(path.clone());
            for arg in sub.get_arguments() {
                if let Some(long) = arg.get_long() {
                    out.entry("cli.flags")
                        .or_default()
                        .insert(format!("{path} --{long}"));
                }
                if let Some(short) = arg.get_short() {
                    out.entry("cli.flags")
                        .or_default()
                        .insert(format!("{path} -{short}"));
                }
                for v in arg.get_possible_values() {
                    // A switch's `true` / `false` is not a name of its own.
                    if !v.is_hide_set() && !matches!(v.get_name(), "true" | "false") {
                        out.entry("cli.values").or_default().insert(format!(
                            "{path} --{}={}",
                            arg.get_long().unwrap_or(arg.get_id().as_str()),
                            v.get_name()
                        ));
                    }
                }
            }
            walk(sub, &format!("{path} "), out);
        }
    }
    walk(&discipline::cli::Cli::command(), "", out);
}

fn env(out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
    let docs = std::fs::read_to_string(root().join("docs/CONFIGURATION.md")).unwrap();
    let re = regex::Regex::new(r"DISCIPLINE_[A-Z_]+[A-Z]").unwrap();
    let mut src = String::new();
    for entry in walkdir(root().join("src")) {
        src.push_str(&std::fs::read_to_string(entry).unwrap());
    }
    for m in re.find_iter(&docs) {
        let name = m.as_str();
        // Documented and read by the code: an interface.
        if !INTERNAL_ENV.contains(&name) && src.contains(name) {
            out.entry("env").or_default().insert(name.to_string());
        }
    }
}

fn walkdir(dir: std::path::PathBuf) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files.extend(walkdir(p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            files.push(p);
        }
    }
    files
}

fn config(out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
    let schema = discipline::schema::generate_schema();
    fn walk(root: &Value, node: &Value, path: &str, out: &mut BTreeSet<String>, depth: usize) {
        if depth > 8 {
            return;
        }
        let node = match node.get("$ref").and_then(Value::as_str) {
            Some(r) => &root["$defs"][r.trim_start_matches("#/$defs/")],
            None => node,
        };
        for key in ["anyOf", "oneOf", "allOf"] {
            for alt in node
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(root, alt, path, out, depth + 1);
            }
        }
        if let Some(props) = node.get("properties").and_then(Value::as_object) {
            for (k, v) in props {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                out.insert(p.clone());
                walk(root, v, &p, out, depth + 1);
            }
        }
    }
    walk(
        &schema,
        &schema,
        "",
        out.entry("config.keys").or_default(),
        0,
    );
}

fn registries(out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
    for d in discipline::tokens::ALL_DIRECTIVE_NAMES {
        out.entry("directives").or_default().insert(d.to_string());
    }
    for g in discipline::config::GATES.iter().filter(|g| g.available) {
        out.entry("gates").or_default().insert(g.id.to_string());
    }
    for k in discipline::findings::FINDINGS {
        out.entry("finding_codes")
            .or_default()
            .insert(discipline::findings::full_code(k.gates[0], k));
    }
    for r in discipline::could_not_check::Reason::ALL {
        out.entry("could_not_check_reasons")
            .or_default()
            .insert(r.as_str().to_string());
    }
}

fn mcp(out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
    struct NoChild;
    impl discipline::mcp::Runner for NoChild {
        fn check(
            &self,
            _: &discipline::hook::CheckSide,
        ) -> anyhow::Result<discipline::hook::CheckRun> {
            anyhow::bail!("not run")
        }
        fn gates(&self) -> anyhow::Result<String> {
            Ok(String::new())
        }
    }
    let list = discipline::mcp::handle(
        &NoChild,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .unwrap();
    for t in list["result"]["tools"].as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        out.entry("mcp.tools").or_default().insert(name.to_string());
        for arg in t["inputSchema"]["properties"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
        {
            out.entry("mcp.arguments")
                .or_default()
                .insert(format!("{name}.{arg}"));
        }
    }
}

fn action(out: &mut BTreeMap<&'static str, BTreeSet<String>>) {
    let text = std::fs::read_to_string(root().join("action.yml")).unwrap();
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    for (section, key) in [("inputs", "action.inputs"), ("outputs", "action.outputs")] {
        for (k, _) in doc[section].as_mapping().into_iter().flatten() {
            out.entry(key)
                .or_default()
                .insert(k.as_str().unwrap().to_string());
        }
    }
}

fn current() -> BTreeMap<&'static str, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    cli(&mut out);
    env(&mut out);
    config(&mut out);
    registries(&mut out);
    mcp(&mut out);
    action(&mut out);
    out.insert(
        "exit_codes",
        ["0 pass", "1 findings", "2 could not check"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    out
}

#[test]
fn nothing_frozen_is_removed_or_renamed() {
    let path = root().join("tests/fixtures/v1_surface.json");
    let now = current();
    if std::env::var("DISCIPLINE_BLESS_SURFACE").is_ok_and(|v| v == "1") {
        let mut fixture: BTreeMap<String, BTreeSet<String>> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| serde_json::from_value(v["surface"].clone()).ok())
            .unwrap_or_default();
        for (k, v) in &now {
            fixture
                .entry(k.to_string())
                .or_default()
                .extend(v.iter().cloned());
        }
        let doc = json!({
            "description": "The names discipline 1.0 freezes (docs/ARCHITECTURE.md §3.2). tests/test_stability_contract.rs fails when one is removed or renamed; DISCIPLINE_BLESS_SURFACE=1 cargo test --test test_stability_contract adds new ones.",
            "surface": fixture,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
    }
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let fixture: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["surface"].clone()).unwrap();

    let mut removed = Vec::new();
    let mut added = Vec::new();
    for (section, names) in &fixture {
        let have = now.get(section.as_str()).cloned().unwrap_or_default();
        removed.extend(names.difference(&have).map(|n| format!("{section}: {n}")));
    }
    for (section, names) in &now {
        let had = fixture.get(*section).cloned().unwrap_or_default();
        added.extend(names.difference(&had).map(|n| format!("{section}: {n}")));
    }
    if !added.is_empty() {
        eprintln!(
            "new interface names (allowed; run with DISCIPLINE_BLESS_SURFACE=1 to record them):\n  {}",
            added.join("\n  ")
        );
    }
    assert!(
        removed.is_empty(),
        "names frozen by docs/ARCHITECTURE.md §3.2 are gone. A rename keeps the old name as \
         an alias until the next major version; a removal waits for it:\n  {}",
        removed.join("\n  ")
    );
}

/// The fixture is not allowed to go stale: every name the code has is recorded, so the
/// next removal is caught against the full surface.
#[test]
fn the_fixture_records_every_current_name() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/fixtures/v1_surface.json")).unwrap(),
    )
    .unwrap();
    let fixture: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["surface"].clone()).unwrap();
    let mut missing = Vec::new();
    for (section, names) in current() {
        let had = fixture.get(section).cloned().unwrap_or_default();
        missing.extend(names.difference(&had).map(|n| format!("{section}: {n}")));
    }
    assert!(
        missing.is_empty(),
        "record the new names: DISCIPLINE_BLESS_SURFACE=1 cargo test --test test_stability_contract\n  {}",
        missing.join("\n  ")
    );
}

/// A7: a run stopped by a signal ends by that signal (the shell's 128 + n), never with
/// exit 2, which would read as "could not check".
#[cfg(unix)]
#[test]
fn a_signal_ends_the_run_by_the_signal_not_exit_2() {
    use std::io::{BufRead, Write};
    use std::os::unix::process::ExitStatusExt;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"))
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The server is running once it answers; the stdin stays open, so it waits for more.
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"ping"}}"#).unwrap();
    let mut reply = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut reply)
        .unwrap();
    assert!(reply.contains(r#""id":1"#), "{reply}");
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = child.wait().unwrap();
    drop(stdin);
    assert_eq!(status.code(), None, "{status:?}");
    assert_eq!(status.signal(), Some(15), "{status:?}");
}
