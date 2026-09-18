//! Drives the real `discipline` binary against throwaway git repositories.
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub const GOOD_LIB: &str = "\
pub fn read(p: *const u8) -> u8 {
    // SAFETY: callers pass a pointer that is valid for reads.
    unsafe { *p }
}
";

pub const GOOD_TEST: &str = "\
#[test]
fn adds() {
    let x = 1;
    assert_eq!(x + 1, 2);
    assert_eq!(x + 3, 4);
}

#[test]
fn orders() {
    let x = 1;
    assert!(x < 2);
}
";

pub struct Repo {
    pub dir: TempDir,
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("not JSON ({e}):\n{}\n{}", self.stdout, self.stderr))
    }

    /// Titles of the violations a gate reported.
    pub fn titles(&self, gate: &str) -> Vec<String> {
        self.outcome(gate)["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap().to_string())
            .collect()
    }

    pub fn outcome(&self, gate: &str) -> Value {
        self.json()["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["gate"] == gate)
            .unwrap_or_else(|| panic!("gate {gate} missing from report"))
            .clone()
    }
}

impl Repo {
    /// A clean repository with one commit on `main`, checked out on `work`.
    pub fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.write("AGENTS.md", "# Agent guide\n");
        repo.write("src/lib.rs", GOOD_LIB);
        repo.write("tests/a.rs", GOOD_TEST);
        repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2.\n");
        repo.commit("chore: base");
        repo.git(&["checkout", "-q", "-b", "work"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn file(&self, rel: &str) -> PathBuf {
        self.path().join(rel)
    }

    pub fn write(&self, rel: &str, content: &str) {
        let p = self.file(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    pub fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
    }

    /// `discipline check --format json --base main <extra>`
    pub fn check(&self, extra: &[&str]) -> Run {
        let mut args = vec!["check", "--format", "json"];
        if !extra.contains(&"--staged") && !extra.contains(&"--base") {
            args.extend(["--base", "main"]);
        }
        args.extend(extra);
        self.run(&args, &[])
    }

    pub fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_discipline"));
        cmd.args(args).current_dir(self.path());
        // Inherit nothing that could change the verdict.
        for var in [
            "PR_BODY",
            "GITHUB_STEP_SUMMARY",
            "DISCIPLINE_CONFIG",
            "DISCIPLINE_CONFIG_OVERRIDE",
            "DISCIPLINE_ENABLE",
            "DISCIPLINE_DISABLE",
            "DISCIPLINE_BASE_REF",
            "DISCIPLINE_FAIL_ON_WARNINGS",
            "DISCIPLINE_FAIL_ON_OVERRIDES",
            "DISCIPLINE_DIRECTIVE_SOURCES",
            "DISCIPLINE_HOSTNAME_DENYLIST",
        ] {
            cmd.env_remove(var);
        }
        cmd.envs(env.iter().copied());
        let out = cmd.output().unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    pub fn run_in_dir(&self, rel_dir: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_discipline"));
        cmd.args(args).current_dir(self.file(rel_dir));
        for var in [
            "PR_BODY",
            "GITHUB_STEP_SUMMARY",
            "DISCIPLINE_CONFIG",
            "DISCIPLINE_CONFIG_OVERRIDE",
            "DISCIPLINE_ENABLE",
            "DISCIPLINE_DISABLE",
            "DISCIPLINE_BASE_REF",
            "DISCIPLINE_FAIL_ON_WARNINGS",
            "DISCIPLINE_HOSTNAME_DENYLIST",
        ] {
            cmd.env_remove(var);
        }
        cmd.envs(env.iter().copied());
        let out = cmd.output().unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}
