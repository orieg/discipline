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
