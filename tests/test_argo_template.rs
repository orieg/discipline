//! The Argo Workflows template's gate step, run as a script (#891): it resolves a merge
//! base past its shallow fetch depth, judges by the base policy, and reports exit 2 as
//! its own status.
#![cfg(unix)]
mod common;

use common::{git_command, script_command};
use std::path::Path;

fn template() -> serde_yaml::Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    serde_yaml::from_str(
        &std::fs::read_to_string(root.join("templates/argo-workflow-template.yaml")).unwrap(),
    )
    .unwrap()
}

fn gate_step() -> serde_yaml::Value {
    template()["spec"]["templates"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "discipline-gate")
        .unwrap()
        .clone()
}

fn env_value(name: &str) -> Option<String> {
    gate_step()["container"]["env"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .and_then(|e| e["value"].as_str().map(str::to_string))
}

fn git(dir: &Path, args: &[&str]) {
    let out = git_command()
        .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An upstream whose `feat` branch and `main` each have `n` commits past their fork point.
fn diverged_upstream(dir: &Path, n: usize) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "root"]);
    git(dir, &["switch", "-q", "-c", "feat"]);
    for i in 0..n {
        git(
            dir,
            &["commit", "-q", "--allow-empty", "-m", &format!("f{i}")],
        );
    }
    git(dir, &["switch", "-q", "main"]);
    for i in 0..n {
        git(
            dir,
            &["commit", "-q", "--allow-empty", "-m", &format!("m{i}")],
        );
    }
}

/// Run the gate step's script with its parameters filled in, /workspace moved into
/// `work`, and `discipline` replaced by a stub that exits with `stub_rc` once it sees a
/// merge base.
fn run_gate(work: &Path, upstream: &Path, stub_rc: i32) -> (i32, String, String) {
    let script = gate_step()["container"]["args"][0]
        .as_str()
        .unwrap()
        .replace("/workspace", work.to_str().unwrap())
        .replace(
            "{{inputs.parameters.repo-url}}",
            &format!("file://{}", upstream.display()),
        )
        .replace("{{inputs.parameters.target-branch}}", "main")
        .replace("{{inputs.parameters.source-branch}}", "feat");
    let bin = work.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        bin.join("discipline"),
        format!(
            "#!/bin/sh\ngit merge-base origin/main HEAD >/dev/null || exit 2\nexit {stub_rc}\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        bin.join("discipline"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = script_command("sh")
        .args(["-c", &script])
        .env("PATH", path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    let status = std::fs::read_to_string(work.join("reports/status")).unwrap_or_default();
    (
        out.status.code().unwrap_or(-1),
        status.trim().to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn argo_gate_resolves_a_merge_base_past_its_fetch_depth() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = tmp.path().join("up");
    std::fs::create_dir_all(&upstream).unwrap();
    diverged_upstream(&upstream, 60);
    let work = tmp.path().join("ws");
    let (rc, status, stderr) = run_gate(&work, &upstream, 0);
    assert_eq!((rc, status.as_str()), (0, "success"), "{stderr}");
}

#[test]
fn argo_gate_reports_exit_codes_as_distinct_statuses() {
    for (stub_rc, want) in [(1, "failure"), (2, "could-not-check")] {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("up");
        std::fs::create_dir_all(&upstream).unwrap();
        diverged_upstream(&upstream, 2);
        let work = tmp.path().join("ws");
        let (rc, status, stderr) = run_gate(&work, &upstream, stub_rc);
        assert_eq!((rc, status.as_str()), (stub_rc, want), "{stderr}");
    }
}

#[test]
fn argo_gate_reports_an_unreachable_repository_as_could_not_check() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no-such-repo");
    let work = tmp.path().join("ws");
    let (rc, status, stderr) = run_gate(&work, &missing, 0);
    assert_eq!((rc, status.as_str()), (2, "could-not-check"), "{stderr}");
}

#[test]
fn argo_gate_judges_by_the_base_policy_and_caps_its_text_report() {
    assert_eq!(env_value("DISCIPLINE_POLICY_FROM").as_deref(), Some("base"));
    let cap = env_value("DISCIPLINE_MAX_REPORT_LINES").expect("report cap set");
    assert!(cap.parse::<usize>().is_ok(), "{cap}");
    let outputs = &gate_step()["outputs"]["parameters"];
    assert!(
        outputs
            .as_sequence()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "discipline-status"),
        "the status is exported for a commit-status step"
    );
}
