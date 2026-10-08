//! A git read that fails must not read as "the file is absent" (#492 item 2). Each test
//! makes one read fail through the real binary while the diff, which never needs that
//! file, still succeeds:
//! the loose blob of a file the change did not touch is removed from the base side.

#![cfg(unix)]

mod common;
use common::{Repo, Run, CONFIG_HEAD};

/// Removes the loose object holding `path` as committed on `main`.
fn lose_base_blob(repo: &Repo, path: &str) {
    let sha = repo.git_output(&["rev-parse", &format!("main:{path}")]);
    let obj = repo.file(&format!(".git/objects/{}/{}", &sha[..2], &sha[2..]));
    assert!(obj.is_file(), "blob {sha} is not loose");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&obj, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(obj).unwrap();
}

fn assert_could_not_check(run: &Run, gate: &str, path: &str) {
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    let (_, g) = run.could_not_check();
    assert_eq!(g.as_deref(), Some(gate), "{}", run.stdout);
    let detail = run.json()["could_not_check"]["detail"].to_string();
    assert!(detail.contains(path), "detail must name `{path}`: {detail}");
}

#[test]
fn test_floor_does_not_count_an_unreadable_base_file_as_absent() {
    let repo = Repo::new();
    lose_base_blob(&repo, "tests/a.rs");
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    let run = repo.check(&[]);
    assert_could_not_check(&run, "test-floor", "tests/a.rs");
}

#[test]
fn command_base_tests_does_not_skip_an_unreadable_base_test_file() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.test-floor]\nenabled = false\n[gates.command]\nenabled = true\npreset = \"base-tests\"\ncommand = \"true\"\n"),
        "chore: configure base-tests",
    );
    lose_base_blob(&repo, "tests/a.rs");
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    let run = repo.check(&[]);
    assert_could_not_check(&run, "command", "tests/a.rs");
}

#[test]
fn an_unreadable_base_manifest_does_not_become_an_empty_runner_collection() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[(
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )],
        "base: manifest",
    );
    lose_base_blob(&repo, "Cargo.toml");
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    let run = repo.check(&[]);
    assert_could_not_check(&run, "assertion-reduction", "Cargo.toml");
}

#[test]
fn ci_integrity_names_a_workflow_whose_head_side_does_not_parse() {
    let repo = Repo::new();
    repo.commit_base(
        ".github/workflows/ci.yml",
        "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n",
        "ci: add",
    );
    repo.write(".github/workflows/ci.yml", "jobs:\n\tbuild: [\n");
    let run = repo.check(&[]);
    let notes = run.outcome("ci-integrity")["notes"].to_string();
    assert!(
        notes.contains(".github/workflows/ci.yml: the head side does not parse as YAML"),
        "{notes}"
    );
}

#[test]
fn a_directory_at_a_configured_path_reads_as_absent_not_as_a_failed_read() {
    let repo = Repo::new();
    repo.commit_base(
        "somedir/inner.txt",
        "inside\n",
        "chore: a directory both sides have",
    );
    repo.write(
        "discipline.toml",
        &format!(
            "{CONFIG_HEAD}[gates.dependency-delta]\nenabled = true\ndeny_file = \"somedir\"\n"
        ),
    );
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// Commits a gitlink (a submodule pointer) at `path` on `main` and rebases `work` onto it.
fn commit_base_gitlink(repo: &Repo, path: &str) {
    let sha = repo.git_output(&["rev-parse", "main"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{sha},{path}"),
    ]);
    repo.git(&["commit", "-q", "-m", "chore: submodule pointer"]);
    repo.git(&["checkout", "-q", "-B", "work", "main"]);
    let entry = repo.git_output(&["ls-tree", "main", path]);
    assert!(entry.starts_with("160000"), "no gitlink on main: {entry}");
}

#[test]
fn a_gitlink_on_the_base_side_reads_as_absent_not_as_a_failed_read() {
    let repo = Repo::new();
    commit_base_gitlink(&repo, "vendor/sub");
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.test-floor]\nenabled = true\nconstant_file = \"vendor/sub\"\nconstant_name = \"MIN_TESTS\"\n"),
    );
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2, more.\n");
    let run = repo.check(&[]);
    // The gate ran and judged the path as not a file on the base side; a failed read would
    // be exit 2 with no outcome.
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let titles = run.titles("test-floor");
    assert_eq!(
        titles,
        vec!["Floor Constant File Missing In Base Ref".to_string()],
        "{}",
        run.stdout
    );
}

#[test]
fn a_staged_gitlink_reads_as_absent_not_as_a_failed_read() {
    let repo = Repo::new();
    let sha = repo.git_output(&["rev-parse", "HEAD"]);
    repo.write(
        "discipline.toml",
        &format!(
            "{CONFIG_HEAD}[gates.dependency-delta]\nenabled = true\ndeny_file = \"vendor/sub\"\n"
        ),
    );
    repo.git(&["add", "discipline.toml"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{sha},vendor/sub"),
    ]);
    let entry = repo.git_output(&["ls-files", "-s", "vendor/sub"]);
    assert!(entry.starts_with("160000"), "no staged gitlink: {entry}");
    let run = repo.check(&["--staged"]);
    assert_ne!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.outcome("dependency-delta")["violations"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "{}",
        run.stdout
    );
}

#[test]
fn a_parse_note_never_echoes_the_yaml() {
    let repo = Repo::new();
    let wf = ".github/workflows/ci.yml";
    repo.commit_base(
        wf,
        "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n",
        "ci: add",
    );
    repo.write(
        wf,
        "name: ci\non: push\njobs:\n  build:\n    env: {TOKEN: *SECRET_SENTINEL_VALUE\n    steps: [\n",
    );
    let run = repo.check(&[]);
    let notes = run.outcome("ci-integrity")["notes"].to_string();
    assert!(
        notes.contains(".github/workflows/ci.yml: the head side does not parse as YAML"),
        "{notes}"
    );
    assert!(
        notes.contains("at line 5, column 18)") || notes.contains("(line 5, column 18)"),
        "{notes}"
    );
    assert!(!notes.contains("SECRET_SENTINEL_VALUE"), "{notes}");
    assert!(!run.stdout.contains("SECRET_SENTINEL_VALUE"));
}
