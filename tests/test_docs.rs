//! Tests for reference documentation and schema generator sentinel (`discipline docs`).

mod common;
use common::Repo;

fn setup_docs_repo() -> Repo {
    let repo = Repo::new();
    // Copy real action.yml into test repo
    let action_yml = std::fs::read_to_string("action.yml").expect("read action.yml");
    repo.write("action.yml", &action_yml);

    // Write a README with generated markers
    repo.write(
        "README.md",
        "# Test Repo\n\n## Gates\n<!-- generated:gates -->\n<!-- /generated -->\n\n## Inputs\n<!-- generated:action-inputs -->\n<!-- /generated -->\n",
    );

    // Write docs/index.html with markers
    repo.write(
        "docs/index.html",
        "<!DOCTYPE html><html><body>\n<table><tbody>\n<!-- generated:gates -->\n<!-- /generated -->\n</tbody></table>\n<!-- generated:action-inputs -->\n<!-- /generated -->\n</body></html>\n",
    );

    // Initial write
    let run = repo.run(&["docs", "--write"], &[]);
    assert_eq!(
        run.code, 0,
        "docs --write failed: {}\n{}",
        run.stdout, run.stderr
    );

    repo
}

#[test]
fn test_docs_write_and_check_roundtrip() {
    let repo = setup_docs_repo();

    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(
        check.code, 0,
        "check must pass after write: {}\n{}",
        check.stdout, check.stderr
    );
    assert!(check
        .stdout
        .contains("All reference documentation and schemas are up to date."));
}

#[test]
fn test_docs_check_detects_action_yml_input_drift() {
    let repo = setup_docs_repo();

    // Mutate action.yml by appending a fake input
    let action_yml = std::fs::read_to_string(repo.file("action.yml")).unwrap();
    let mutated_action_yml = action_yml.replace(
        "inputs:\n",
        "inputs:\n  fake_test_input:\n    description: 'Drift detection test input'\n    required: false\n    default: ''\n",
    );
    assert_ne!(action_yml, mutated_action_yml);
    repo.write("action.yml", &mutated_action_yml);

    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(check.code, 1, "check must fail with exit code 1 on drift");
    let combined = format!("{}\n{}", check.stdout, check.stderr);
    assert!(
        combined.contains("fake_test_input"),
        "diff must show mutated input:\n{combined}"
    );
    assert!(
        combined.contains("cargo run -- docs --write"),
        "must show remediation:\n{combined}"
    );
}

#[test]
fn test_docs_check_detects_gate_status_drift() {
    let repo = setup_docs_repo();

    // Mutate README.md inside generated region
    let readme = std::fs::read_to_string(repo.file("README.md")).unwrap();
    let tampered = readme.replace("assertion-reduction", "tampered-gate-name");
    assert_ne!(readme, tampered, "replacement must have occurred");
    repo.write("README.md", &tampered);

    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(check.code, 1, "check must fail on tampered gate table");
    let combined = format!("{}\n{}", check.stdout, check.stderr);
    assert!(
        combined.contains("tampered-gate-name"),
        "diff must show tampered gate"
    );
}

#[test]
fn test_docs_check_detects_schema_drift() {
    let repo = setup_docs_repo();

    // Tamper with discipline.schema.json
    let schema_path = repo.file("discipline.schema.json");
    let schema_content = std::fs::read_to_string(&schema_path).unwrap();
    let tampered = schema_content.replace("DisciplineConfig", "TamperedConfig");
    assert_ne!(schema_content, tampered);
    repo.write("discipline.schema.json", &tampered);

    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(check.code, 1, "check must fail on tampered schema");
    let combined = format!("{}\n{}", check.stdout, check.stderr);
    assert!(combined.contains("TamperedConfig"));
}

#[test]
fn test_docs_check_fails_on_unclosed_marker() {
    let repo = Repo::new();
    repo.write("action.yml", "name: 'Test'\n");
    repo.write(
        "README.md",
        "# Broken\n<!-- generated:gates -->\nNo closing marker here!\n",
    );

    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(
        check.code, 2,
        "unclosed marker must fail closed with exit code 2"
    );
    assert!(check.stderr.contains("unclosed generated marker 'gates'"));
}

#[test]
fn test_docs_check_fails_on_duplicate_marker() {
    let repo = Repo::new();
    repo.write("action.yml", "name: 'Test'\n");
    repo.write(
        "README.md",
        "# Duplicate\n<!-- generated:gates -->\n<!-- /generated -->\n<!-- generated:gates -->\n<!-- /generated -->\n",
    );

    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(
        check.code, 2,
        "duplicate marker must fail closed with exit code 2"
    );
    assert!(check.stderr.contains("duplicate generated marker 'gates'"));
}

#[test]
fn test_mutant_check_always_pass_killed() {
    let repo = setup_docs_repo();

    // Introduce drift
    let readme = std::fs::read_to_string(repo.file("README.md")).unwrap();
    repo.write(
        "README.md",
        &format!("{readme}\n<!-- generated:cli -->\n<!-- /generated -->\n"),
    );

    let check = repo.run(&["docs", "--check"], &[]);
    // A mutant where run_docs_check_or_write always returns Ok(true) is killed by this assertion
    assert_ne!(check.code, 0, "mutant that always returns 0 must be killed");
    assert_eq!(check.code, 1);
}

#[test]
fn test_check_links_script_succeeds_on_repo() {
    let output = std::process::Command::new("python3")
        .arg("tests/action/check-links.py")
        .output()
        .expect("run check-links.py");
    assert!(
        output.status.success(),
        "check-links.py failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_check_links_script_fails_on_broken_anchor() {
    let temp_dir = tempfile::tempdir().unwrap();
    let script_src = std::fs::read_to_string("tests/action/check-links.py").unwrap();
    let action_dir = temp_dir.path().join("tests/action");
    std::fs::create_dir_all(&action_dir).unwrap();
    let script_path = action_dir.join("check-links.py");
    std::fs::write(&script_path, script_src).unwrap();

    // Create a README with broken anchor
    std::fs::write(
        temp_dir.path().join("README.md"),
        "# Title\n\n[Broken](#does-not-exist)\n",
    )
    .unwrap();

    let output = std::process::Command::new("python3")
        .arg(&script_path)
        .current_dir(temp_dir.path())
        .output()
        .expect("run check-links.py");
    assert!(
        !output.status.success(),
        "check-links.py must fail on broken anchor"
    );
}

/// Every property key the JSON Schema exposes, collected by walking the raw
/// schema JSON (independently of the renderer under test).
fn all_schema_property_names() -> std::collections::BTreeSet<String> {
    fn walk(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::Object(props)) = map.get("properties") {
                    out.extend(props.keys().cloned());
                }
                for child in map.values() {
                    walk(child, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(&discipline::schema::generate_schema(), &mut out);
    // `reset` / `items` belong to the list-reset helper, not to a config key.
    out.remove("reset");
    out.remove("items");
    out
}

/// Every `gates.<id>.<key>` path the schema accepts, resolved through `$ref`.
fn all_gate_key_paths() -> Vec<String> {
    let schema = discipline::schema::generate_schema();
    let defs = &schema["$defs"];
    let mut paths = Vec::new();
    let gates = schema["properties"]["gates"]["properties"]
        .as_object()
        .expect("gates.properties is an object");
    for (id, entry) in gates {
        let reference = entry["allOf"][0]["$ref"]
            .as_str()
            .expect("gate entry references a $def");
        let def_name = reference.trim_start_matches("#/$defs/");
        let props = defs[def_name]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{def_name} has properties"));
        for key in props.keys() {
            paths.push(format!("gates.{id}.{key}"));
        }
    }
    paths
}

#[test]
fn test_config_reference_lists_every_schema_key() {
    let table = discipline::docs::render_config_schema_markdown();
    let names = all_schema_property_names();
    assert!(names.len() > 100, "schema walk found {} keys", names.len());
    let missing: Vec<&String> = names
        .iter()
        .filter(|k| {
            // A key is present when it is a segment of some row's dotted path.
            let segment = [
                format!("`{k}."),
                format!(".{k}."),
                format!(".{k}`"),
                format!(".{k}[]"),
            ];
            !segment.iter().any(|s| table.contains(s.as_str()))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "{} schema keys absent from the generated config table: {missing:?}",
        missing.len()
    );
    let missing_paths: Vec<String> = all_gate_key_paths()
        .into_iter()
        .filter(|p| !table.contains(&format!("`{p}`")))
        .collect();
    assert!(
        missing_paths.is_empty(),
        "gate key paths absent from the generated config table: {missing_paths:?}"
    );
}

#[test]
fn test_config_reference_rows_are_complete() {
    let table = discipline::docs::render_config_schema_markdown();
    for row in table.lines().skip(2) {
        let cells: Vec<&str> = row.trim_matches('|').split(" | ").collect();
        assert_eq!(cells.len(), 4, "malformed row: {row}");
        for cell in &cells {
            assert!(!cell.trim().is_empty(), "empty cell in row: {row}");
        }
    }
    // The Default column is derived from the compiled defaults, not hand-typed.
    assert!(
        table.contains("| `gates.suppression-delta.severity` | string | `\"warning\"` |"),
        "suppression-delta severity row does not show the compiled default"
    );
    assert!(
        table.contains("| `gates.miri.enabled` | boolean | `false` |"),
        "miri enabled row does not show the compiled default"
    );
}

#[test]
fn test_committed_configuration_md_lists_every_schema_key() {
    let doc = std::fs::read_to_string("docs/CONFIGURATION.md").expect("read CONFIGURATION.md");
    let missing: Vec<String> = all_gate_key_paths()
        .into_iter()
        .filter(|p| !doc.contains(&format!("`{p}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "{} gate keys absent from docs/CONFIGURATION.md (run `discipline docs --write`): {missing:?}",
        missing.len()
    );
}

#[test]
fn test_gates_html_severity_badge_reflects_compiled_default() {
    let html = discipline::docs::render_gates_html(discipline::config::GATES);
    let row = |id: &str| -> String {
        let start = html
            .find(&format!("<td class=\"gate-id\">{id}</td>"))
            .unwrap_or_else(|| panic!("{id} row missing"));
        let end = html[start..].find("</tr>").unwrap() + start;
        html[start..end].to_string()
    };
    assert!(
        row("suppression-delta").contains("badge-warn\">Warning<"),
        "{}",
        row("suppression-delta")
    );
    assert!(
        row("assertion-reduction").contains("badge-error\">Error<"),
        "{}",
        row("assertion-reduction")
    );
    assert!(row("miri").contains(">Off<"), "{}", row("miri"));
}

/// `discipline gates | head -1` must not panic when the reader goes away: the process
/// ends the way other Unix tools do (SIGPIPE), never with a panic backtrace.
#[cfg(unix)]
#[test]
fn closed_stdout_pipe_does_not_panic() {
    use std::os::unix::process::ExitStatusExt;
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_discipline"))
        .arg("gates")
        .stdout(writer)
        .stderr(std::process::Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.signal(), Some(libc::SIGPIPE), "{:?}", out.status);
}
