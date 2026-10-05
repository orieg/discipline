//! #530: `hook run` and the MCP `check_diff` tool measure the change against the
//! repository's default branch, whatever it is called, and still refuse a clone whose
//! only candidate is the change's own branch (#476).

mod common;

use common::{Repo, GOOD_LIB, GOOD_TEST};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

const POST_EDIT: &str = r#"{"hook_event_name":"PostToolUse","tool_name":"Edit"}"#;
const REDUCED: &str = "[assertion-reduction/assertions-reduced]";
const WEAK_TEST: &str = "\
#[test]
fn adds() {
    let x = 1;
    let _ = x + 1;
}

#[test]
fn orders() {
    let x = 1;
    assert!(x < 2);
}
";

struct HookRun {
    code: i32,
    stdout: String,
    stderr: String,
}

fn hook_at(dir: &Path, extra: &[&str], env: &[(&str, &str)]) -> HookRun {
    let mut cmd = common::discipline_cmd(dir);
    cmd.args(["hook", "run", "--agent", "claude-code"])
        .args(extra)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(POST_EDIT.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    HookRun {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The `structuredContent` of one MCP `check_diff` call in `dir`.
fn check_diff(dir: &Path) -> Value {
    let mut cmd = common::discipline_cmd(dir);
    cmd.arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"check_diff","arguments":{}}})
    )
    .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reply: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    reply["result"].clone()
}

/// The files the findings of a `check_diff` result name, sorted.
fn finding_files(result: &Value) -> Vec<String> {
    let mut files: Vec<String> = result["structuredContent"]["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {result}"))
        .iter()
        .map(|f| f["file"].as_str().unwrap_or("").to_string())
        .collect();
    files.sort();
    files.dedup();
    files
}

/// One commit on `branch`, checked out on it: no remote, no other branch.
fn repo_on(branch: &str) -> Repo {
    let repo = Repo {
        dir: tempfile::tempdir().unwrap(),
    };
    repo.git(&["init", "-q", "-b", branch]);
    repo.write("AGENTS.md", "# Agent guide\n");
    repo.write("src/lib.rs", GOOD_LIB);
    repo.write("tests/a.rs", GOOD_TEST);
    repo.write("tests/b.rs", GOOD_TEST);
    repo.commit("chore: base");
    repo
}

/// `origin/main` is a release branch one commit behind `origin/develop`, which
/// `origin/HEAD` names as the default branch. The commit only `origin/develop` has
/// weakens `tests/a.rs`: it is merged work, not the agent's change. HEAD is on `work`,
/// cut from `origin/develop`, and the uncommitted change weakens `tests/b.rs`.
fn develop_default_repo() -> Repo {
    let repo = repo_on("seed");
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.write("tests/a.rs", WEAK_TEST);
    repo.commit("test: simplify a");
    repo.git(&["update-ref", "refs/remotes/origin/develop", "HEAD"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/develop",
    ]);
    repo.git(&["checkout", "-q", "-b", "work"]);
    repo.git(&["branch", "-q", "-D", "seed"]);
    repo.write("tests/b.rs", WEAK_TEST);
    repo
}

fn assert_reports_the_edit(run: &HookRun) {
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(
        !run.stderr.contains("could not check"),
        "the hook refused instead of measuring the edit: {}",
        run.stderr
    );
    assert!(run.stderr.contains(REDUCED), "{}", run.stderr);
}

/// The issue's reproduction: HEAD on `master`, no remote, one uncommitted edit.
#[test]
fn hook_on_a_master_repository_measures_the_uncommitted_edit() {
    let repo = repo_on("master");
    let clean = hook_at(repo.path(), &[], &[]);
    assert_eq!(
        (clean.code, clean.stderr.as_str()),
        (0, ""),
        "{}",
        clean.stdout
    );
    repo.write("tests/a.rs", WEAK_TEST);
    assert_reports_the_edit(&hook_at(repo.path(), &[], &[]));
}

/// A branch cut from `master` and not yet committed to: `master` holds HEAD, and is
/// still the base.
#[test]
fn hook_on_a_fresh_branch_of_a_master_repository_measures_the_uncommitted_edit() {
    let repo = repo_on("master");
    repo.git(&["checkout", "-q", "-b", "work"]);
    repo.write("tests/a.rs", WEAK_TEST);
    assert_reports_the_edit(&hook_at(repo.path(), &[], &[]));
}

/// A weakening committed on a branch of a `trunk` repository is seen too.
#[test]
fn hook_on_a_trunk_repository_sees_a_committed_weakening() {
    let repo = repo_on("trunk");
    repo.git(&["checkout", "-q", "-b", "work"]);
    repo.write("tests/a.rs", WEAK_TEST);
    repo.commit("test: simplify");
    assert_reports_the_edit(&hook_at(repo.path(), &[], &[]));
}

#[test]
fn check_diff_on_a_master_repository_measures_the_uncommitted_edit() {
    let repo = repo_on("master");
    repo.write("tests/a.rs", WEAK_TEST);
    let result = check_diff(repo.path());
    assert_eq!(
        result["structuredContent"]["status"], "findings",
        "{result}"
    );
    assert_eq!(finding_files(&result), ["tests/a.rs"], "{result}");
}

/// The base is `origin/HEAD`'s branch, not `origin/main`: what `origin/develop` already
/// holds is not the agent's change.
#[test]
fn hook_measures_against_origin_head_not_a_release_branch_named_main() {
    let repo = develop_default_repo();
    let run = hook_at(repo.path(), &[], &[]);
    assert_reports_the_edit(&run);
    assert!(run.stderr.contains("tests/b.rs"), "{}", run.stderr);
    assert!(
        !run.stderr.contains("tests/a.rs"),
        "measured against origin/main, not origin/develop: {}",
        run.stderr
    );
}

#[test]
fn check_diff_measures_against_origin_head_not_a_release_branch_named_main() {
    let repo = develop_default_repo();
    let result = check_diff(repo.path());
    assert_eq!(
        result["structuredContent"]["status"], "findings",
        "{result}"
    );
    assert_eq!(finding_files(&result), ["tests/b.rs"], "{result}");
}

/// Control: `--base`, `DISCIPLINE_BASE_REF` and a CI base variable each still decide.
/// Against `origin/main` the weakening `origin/develop` holds is part of the change.
#[test]
fn an_explicit_base_and_the_environment_still_win_over_the_default_branch() {
    let repo = develop_default_repo();
    for (extra, env) in [
        (&["--base", "origin/main"][..], &[][..]),
        (&[][..], &[("DISCIPLINE_BASE_REF", "origin/main")][..]),
        (&[][..], &[("GITHUB_BASE_REF", "main")][..]),
    ] {
        let run = hook_at(repo.path(), extra, env);
        assert_reports_the_edit(&run);
        assert!(
            run.stderr.contains("tests/a.rs") && run.stderr.contains("tests/b.rs"),
            "{extra:?} {env:?}: {}",
            run.stderr
        );
    }
}

/// Control: a `main` repository behaves as before, on a branch and on `main` itself.
#[test]
fn a_main_repository_is_measured_against_main_as_before() {
    let repo = Repo::new();
    repo.write("tests/a.rs", WEAK_TEST);
    assert_reports_the_edit(&hook_at(repo.path(), &[], &[]));
    let result = check_diff(repo.path());
    assert_eq!(finding_files(&result), ["tests/a.rs"], "{result}");

    let on_main = repo_on("main");
    on_main.write("tests/a.rs", WEAK_TEST);
    assert_reports_the_edit(&hook_at(on_main.path(), &[], &[]));
}

/// #476 with full history: in a clone of one branch, `origin/HEAD` names that branch (the
/// test writes it, as `git remote set-head` does: git versions differ on whether the
/// clone does). It is the change's own branch, not a base: the weakening it carries
/// would read as an empty diff, so the run refuses.
#[test]
fn a_single_branch_clone_of_the_change_is_refused_not_passed() {
    let repo = Repo::new();
    repo.write("tests/a.rs", WEAK_TEST);
    repo.commit("test: drop assertion");
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("clone");
    let url = format!("file://{}", repo.path().display());
    let status = common::git_command()
        .args(["clone", "-q", "--single-branch", "--branch", "work", &url])
        .arg(&clone)
        .status()
        .unwrap();
    assert!(status.success());
    let status = common::git_command()
        .current_dir(&clone)
        .args([
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/work",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let run = hook_at(&clone, &[], &[]);
    assert_eq!(run.code, 2, "{}", run.stdout);
    assert!(
        run.stderr
            .contains("could not check this change (reason: repository)"),
        "{}",
        run.stderr
    );
    let result = check_diff(&clone);
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["structuredContent"]["status"], "could_not_check",
        "{result}"
    );
}
