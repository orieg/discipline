//! The red-team attack scripts rewrite the global git configuration and delete directories
//! under `/tmp`. Run on a host, one changes the owner's commit identity: a merge commit
//! authored as `lab <lab@lab.invalid>` that GitHub could not verify. Each script therefore
//! sources `lab-guard.sh` before its first command, and the guard refuses anywhere but a
//! container started by `tests/red_team/lab/run.sh`.

use std::path::{Path, PathBuf};
use std::process::Command;

const GUARD_LINE: &str = ". /work/lab-guard.sh || exit 99";

fn attacks() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/red_team/attacks")
}

fn scripts() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in std::fs::read_dir(attacks()).unwrap() {
        let dir = dir.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&dir).unwrap() {
            let f = f.unwrap().path();
            if f.extension().is_some_and(|e| e == "sh") {
                out.push(f);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_attack_script_sources_the_lab_guard_before_its_first_command() {
    let all = scripts();
    assert!(all.len() >= 30, "found only {} attack scripts", all.len());
    let unguarded: Vec<String> = all
        .iter()
        .filter(|p| {
            let text = std::fs::read_to_string(p).unwrap();
            let first = text
                .lines()
                .skip(1)
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'));
            first != Some(GUARD_LINE)
        })
        .map(|p| p.strip_prefix(attacks()).unwrap().display().to_string())
        .collect();
    assert!(
        unguarded.is_empty(),
        "attack scripts whose first command is not `{GUARD_LINE}`: {unguarded:?}"
    );
}

#[test]
fn the_lab_runner_marks_its_containers_for_the_guard() {
    let runner = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/red_team/lab/run.sh"),
    )
    .unwrap();
    assert!(
        runner.contains("-e DISCIPLINE_RT_IN_CONTAINER=1"),
        "{runner}"
    );
    assert!(runner.contains("-e HOME=/tmp"), "{runner}");
}

#[test]
fn the_guard_refuses_a_host_shell_even_with_the_lab_variables_set() {
    if Path::new("/.dockerenv").exists() {
        // Inside a container the guard is meant to pass; nothing to refuse here.
        return;
    }
    let guard = attacks().join("lab-guard.sh");
    let script = format!(". '{}'; echo continued", guard.display());
    for env in [
        vec![],
        vec![("DISCIPLINE_RT_IN_CONTAINER", "1")],
        vec![("DISCIPLINE_RT_IN_CONTAINER", "1"), ("HOME", "/tmp")],
    ] {
        let out = Command::new("bash")
            .args(["-c", &script])
            .env_remove("DISCIPLINE_RT_IN_CONTAINER")
            .envs(env.iter().copied())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(99), "{env:?}");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("continued"),
            "{env:?}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("tests/red_team/lab/run.sh"),
            "{env:?}"
        );
    }
}
