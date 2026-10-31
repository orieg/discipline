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

/// The README's Non-Goals are generated from the gates' "What it does NOT catch" lists in
/// docs/GATES.md: editing a list makes `docs --check` fail, and a gate without a list
/// stops the generation.
#[test]
fn test_docs_check_covers_the_non_goals_generated_from_gate_lists() {
    let repo = setup_docs_repo();
    let gates_md = std::fs::read_to_string("docs/GATES.md").expect("read docs/GATES.md");
    repo.write("docs/GATES.md", &gates_md);
    repo.write(
        "README.md",
        "# Test Repo\n\n## Non-Goals\n<!-- generated:non-goals -->\n<!-- /generated -->\n",
    );
    let write = repo.run(&["docs", "--write"], &[]);
    assert_eq!(write.code, 0, "{}\n{}", write.stdout, write.stderr);
    let readme = std::fs::read_to_string(repo.file("README.md")).unwrap();
    assert!(
        readme.contains("[`out-of-lane-credential-validity`](tests/fixtures/evasion_corpus.json)"),
        "{readme}"
    );
    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(check.code, 0, "{}\n{}", check.stdout, check.stderr);

    // A class dropped from a gate's list: the README no longer agrees.
    let dropped = gates_md
        .lines()
        .filter(|l| !l.starts_with("  - **Out of lane: credential validity**"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(dropped.lines().count(), gates_md.lines().count());
    repo.write("docs/GATES.md", &dropped);
    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(check.code, 1, "{}\n{}", check.stdout, check.stderr);
    assert!(
        check.stderr.contains("-| credential validity |"),
        "the diff names the dropped class: {}",
        check.stderr
    );

    // A gate without a list: the generation stops (exit 2), naming the gate.
    let unlisted = gates_md.replacen(
        "- **What it does NOT catch:**\n  - Non-standard guide names",
        "- **Not caught:**\n  - Non-standard guide names",
        1,
    );
    assert_ne!(unlisted, gates_md);
    repo.write("docs/GATES.md", &unlisted);
    let check = repo.run(&["docs", "--check"], &[]);
    assert_eq!(check.code, 2, "{}\n{}", check.stdout, check.stderr);
    assert!(check.stderr.contains("agents-md"), "{}", check.stderr);
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
    let output = common::script_command("python3")
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

    let output = common::script_command("python3")
        .arg(&script_path)
        .current_dir(temp_dir.path())
        .output()
        .expect("run check-links.py");
    assert!(
        !output.status.success(),
        "check-links.py must fail on broken anchor"
    );
}

fn check_links_on(readme: &str) -> std::process::Output {
    let temp_dir = tempfile::tempdir().unwrap();
    let script_src = std::fs::read_to_string("tests/action/check-links.py").unwrap();
    let action_dir = temp_dir.path().join("tests/action");
    std::fs::create_dir_all(&action_dir).unwrap();
    let script_path = action_dir.join("check-links.py");
    std::fs::write(&script_path, script_src).unwrap();
    std::fs::write(temp_dir.path().join("Cargo.toml"), "version = \"0.0.0\"\n").unwrap();
    std::fs::write(temp_dir.path().join("README.md"), readme).unwrap();
    common::script_command("python3")
        .arg(&script_path)
        .current_dir(temp_dir.path())
        .output()
        .expect("run check-links.py")
}

#[test]
fn test_check_links_script_fails_on_runs_on_image() {
    let output = check_links_on(
        "# Title\n\n```yaml\njobs:\n  gate:\n    runs-on: docker://ghcr.io/example/image:v1\n    steps:\n      - run: true\n```\n",
    );
    assert!(
        !output.status.success(),
        "check-links.py must fail on runs-on: docker://"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("README.md:6: `runs-on:` names an image"),
        "{stderr}"
    );
}

#[test]
fn test_check_links_script_fails_on_git_clone_in_job() {
    let output = check_links_on(
        "# Title\n\n```yaml\njobs:\n  gate:\n    runs-on: ubuntu-latest\n    steps:\n      - run: git clone https://example.invalid/repo.git .\n```\n",
    );
    assert!(
        !output.status.success(),
        "check-links.py must fail on git clone in a job"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("README.md:8: `git clone` in a workflow job"),
        "{stderr}"
    );
}

#[test]
fn test_check_links_script_accepts_git_clone_outside_a_job() {
    // A shell recipe outside a workflow's `steps:` (a local inner loop) is not a job.
    let output = check_links_on(
        "# Title\n\n```bash\ngit clone https://example.invalid/repo.git\n```\n\n```yaml\njobs:\n  gate:\n    runs-on: ubuntu-latest\n    steps:\n      - run: true\n```\n",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
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
    let out = common::discipline_cmd(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        .arg("gates")
        .stdout(writer)
        .stderr(std::process::Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.signal(), Some(libc::SIGPIPE), "{:?}", out.status);
}

/// The work runs on a second thread, and the signal a write to a closed pipe raises may
/// be handed to either. Handed to the waiting one, it was acted on only after the write
/// had failed and `println!` had panicked: the process still ended by the signal, with a
/// panic message first. Which thread gets it varies from run to run, so this runs many.
#[test]
fn a_closed_stdout_pipe_ends_every_run_by_the_signal_with_no_panic_message() {
    use std::os::unix::process::ExitStatusExt;
    for run in 0..40 {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let out = common::discipline_cmd(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .arg("gates")
            .stdout(writer)
            .stderr(std::process::Stdio::piped())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(stderr, "", "run {run}");
        assert_eq!(
            out.status.signal(),
            Some(libc::SIGPIPE),
            "run {run}: {:?}",
            out.status
        );
    }
}

/// The committed completion scripts are exactly what `discipline completions <shell>`
/// emits, so a packaged install and a hand-generated one behave the same.
#[test]
fn committed_completion_scripts_match_the_binary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (shell, file) in [
        ("zsh", "_discipline"),
        ("bash", "discipline.bash"),
        ("fish", "discipline.fish"),
    ] {
        let out = common::discipline_cmd(root)
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(out.status.success(), "{shell}");
        let committed = std::fs::read_to_string(root.join("completions").join(file))
            .unwrap_or_else(|e| panic!("completions/{file}: {e}"));
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            committed,
            "completions/{file} is stale: run `discipline docs --write`"
        );
    }
}

/// The installer the README serves (`docs/install.sh`, via the Pages site) is a
/// byte copy of the tested root `install.sh`: a fix to one without the other
/// would ship an untested installer.
/// Killed mutant: a byte appended to either copy.
#[test]
fn served_installer_is_a_byte_copy_of_the_tested_installer() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let tested = std::fs::read(root.join("install.sh")).expect("install.sh must exist");
    let served = std::fs::read(root.join("docs/install.sh")).expect("docs/install.sh must exist");
    assert_eq!(
        served, tested,
        "docs/install.sh drifted from install.sh: copy install.sh over docs/install.sh"
    );
}

/// `discipline docs --check` over the working tree itself, through the isolated
/// helper: the one drift check of every generated region, schema, man page and
/// completion script, byte for byte. `committed_completion_scripts_match_the_binary` stays
/// for what this does not say, that `discipline completions <shell>` prints the
/// committed script; `schema_does_not_drift` in `tests/test_config.rs` stays for the
/// gate properties it also checks.
/// Killed mutant: any generated file perturbed.
#[test]
fn docs_check_passes_over_the_working_tree() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = common::discipline_cmd(root)
        .args(["docs", "--check"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "docs --check drifted: run `discipline docs --write`\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The rows of the generated gate tables of a committed Markdown file: gate id and the
/// `Languages` cell, read by the column's position in each table's header.
fn published_coverage(rel: &str) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(rel).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut rows = Vec::new();
    let mut column = None;
    for line in text.lines() {
        if !line.starts_with('|') {
            column = None;
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split(" | ").map(str::trim).collect();
        if let Some(i) = cells.iter().position(|c| *c == "Languages") {
            column = Some(i);
            continue;
        }
        let (Some(i), Some(first)) = (column, cells.first()) else {
            continue;
        };
        let Some(id) = first.split('`').nth(1) else {
            continue;
        };
        rows.push((id.to_string(), cells[i].to_string()));
    }
    rows
}

/// Published language coverage is data (#938): the `Languages` cell of every generated
/// gate table is the gate's derived coverage, and a gate that reads a language pack's
/// facts publishes exactly the packs this binary registers for that fact. A hand edit of
/// a cell, or a pack that gains or loses a fact without `discipline docs --write`, fails.
#[test]
fn published_language_coverage_is_the_registered_packs() {
    use discipline::config::{Coverage, GATES};
    for rel in ["README.md", "docs/GATES.md", "docs/ROADMAP.md"] {
        let rows = published_coverage(rel);
        assert!(!rows.is_empty(), "{rel}: no generated gate table found");
        for (id, cell) in rows {
            let gate = GATES
                .iter()
                .find(|g| g.id == id)
                .unwrap_or_else(|| panic!("{rel}: `{id}` is not a registered gate"));
            assert_eq!(
                cell,
                gate.languages(),
                "{rel}: the published coverage of `{id}` is not the derived one; run `discipline docs --write`"
            );
        }
    }
    let registry = discipline::ast::default_registry();
    let mut derived = 0;
    for g in GATES {
        let Coverage::Packs {
            fact,
            before,
            after,
        } = &g.coverage
        else {
            continue;
        };
        derived += 1;
        let mut labels: Vec<&str> = Vec::new();
        for pack in registry.packs().filter(|p| p.supplies(*fact)) {
            if !labels.contains(&pack.label()) {
                labels.push(pack.label());
            }
        }
        assert!(!labels.is_empty(), "{}: no pack supplies {fact:?}", g.id);
        assert_eq!(
            g.languages(),
            format!("{before}{}{after}", labels.join(", ")),
            "{}",
            g.id
        );
    }
    assert!(derived >= 8, "only {derived} gates derive their coverage");
}

/// The text of each item of the numbered list that follows `heading` in `text`.
fn numbered_items(text: &str, heading: &str) -> Vec<String> {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("`{heading}` not found"));
    let mut items: Vec<String> = Vec::new();
    for line in text[start..].lines().skip(1) {
        if line.starts_with('#') {
            break;
        }
        let Some((n, rest)) = line.split_once(". ") else {
            continue;
        };
        if n.parse::<usize>() == Ok(items.len() + 1) {
            items.push(rest.to_string());
        }
    }
    items
}

/// What an item of the contract names: its code spans and bold phrases, or, for an item
/// that has neither, its whole sentence.
fn contract_anchors(item: &str) -> Vec<String> {
    let mut out: Vec<String> = item
        .split('`')
        .skip(1)
        .step_by(2)
        .map(|s| format!("`{s}`"))
        .collect();
    out.extend(
        item.split("**")
            .skip(1)
            .step_by(2)
            .map(|s| s.trim_end_matches(':').to_string()),
    );
    if out.is_empty() {
        out.push(item.to_string());
    }
    out.into_iter().map(|a| a.to_lowercase()).collect()
}

/// CONTRIBUTING.md restates AGENTS.md §3.4 item by item (#938): the same number of items,
/// and item n of CONTRIBUTING names every path, identifier and bold phrase item n of
/// AGENTS.md names. An item added to, dropped from or reordered in either list fails.
#[test]
fn contributing_restates_the_gate_contract_item_by_item() {
    let agents = numbered_items(
        &std::fs::read_to_string("AGENTS.md").unwrap(),
        "### 3.4 Adding or Changing a Gate",
    );
    let contributing = numbered_items(
        &std::fs::read_to_string("CONTRIBUTING.md").unwrap(),
        "## Gate Contract Discipline",
    );
    assert_eq!(agents.len(), 9, "AGENTS.md §3.4: {agents:#?}");
    assert_eq!(
        contributing.len(),
        agents.len(),
        "CONTRIBUTING.md lists {} contract items, AGENTS.md §3.4 lists {}",
        contributing.len(),
        agents.len()
    );
    for (n, (a, c)) in agents.iter().zip(&contributing).enumerate() {
        let c = c.to_lowercase();
        for anchor in contract_anchors(a) {
            assert!(
                c.contains(&anchor),
                "contract item {}: CONTRIBUTING.md does not name {anchor:?} that AGENTS.md §3.4 names\nAGENTS.md: {a}\nCONTRIBUTING.md: {c}",
                n + 1
            );
        }
    }
}
