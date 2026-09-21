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

    /// Violations a gate reported as JSON Values.
    pub fn violations(&self, gate: &str) -> Vec<Value> {
        self.outcome(gate)["violations"].as_array().unwrap().clone()
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

pub const ISOLATED_ENV_VARS: &[&str] = &[
    "PR_BODY",
    "PR_TITLE",
    "GITHUB_STEP_SUMMARY",
    "GITHUB_BASE_REF",
    "GITHUB_EVENT_PATH",
    "GITEA_BASE_REF",
    "GITEA_EVENT_PATH",
    "FORGEJO_BASE_REF",
    "FORGEJO_EVENT_PATH",
    "FORGEJO_ACTIONS",
    "DISCIPLINE_CONFIG",
    "DISCIPLINE_CONFIG_OVERRIDE",
    "DISCIPLINE_ENABLE",
    "DISCIPLINE_DISABLE",
    "DISCIPLINE_BASE_REF",
    "DISCIPLINE_FAIL_ON_WARNINGS",
    "DISCIPLINE_FAIL_ON_OVERRIDES",
    "DISCIPLINE_ADVISORY",
    "DISCIPLINE_DIRECTIVE_SOURCES",
    "DISCIPLINE_HOSTNAME_DENYLIST",
    "DISCIPLINE_REPORT_GITLAB",
    "DISCIPLINE_REPORT_JUNIT",
    "DISCIPLINE_REPORT_SARIF",
    "DISCIPLINE_BASELINE",
    "DISCIPLINE_NO_BASELINE",
    "DISCIPLINE_CI_CONTEXT",
    "GITHUB_REPOSITORY",
    "GITHUB_ACTIONS",
    "GITHUB_SERVER_URL",
    "GITEA_ACTIONS",
    "CI_SERVER_URL",
    "CI_PROJECT_PATH",
    "DISCIPLINE_FORGE",
    "DISCIPLINE_FORGE_URL",
    "DISCIPLINE_FORGE_REPO",
    "DISCIPLINE_FORGE_TOKEN",
    "GITEA_TOKEN",
    "FORGEJO_TOKEN",
    "GITLAB_TOKEN",
    "CI_PIPELINE_SOURCE",
    "DISCIPLINE_ALLOW_COMMAND_CHANGE",
    "DISCIPLINE_ALLOW_CROSS_HOST_BENCH",
    "DISCIPLINE_BENCH_BASE_FILE",
    "DISCIPLINE_BENCH_HEAD_FILE",
    "DISCIPLINE_BENCH_PROVENANCE",
    "DISCIPLINE_COMMAND",
    "DISCIPLINE_TRUST_WORKSPACE",
    "DOCS_HOSTNAME_DENYLIST",
    "FORGEJO_EVENT_NAME",
    "GITEA_EVENT_NAME",
    "GITHUB_HEAD_REF",
    "GITHUB_OUTPUT",
    "GITHUB_REF",
    "GITHUB_REF_NAME",
    "GITHUB_REF_TYPE",
    "GITHUB_REPOSITORY_OWNER",
    "DISCIPLINE_FORGE_API_URL",
    "DISCIPLINE_FORGE_ALLOW_HTTP",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "GITLAB_CI",
    "CI_MERGE_REQUEST_TARGET_BRANCH_NAME",
    "CI_MERGE_REQUEST_DIFF_BASE_SHA",
    "CI_DEFAULT_BRANCH",
    "GITHUB_EVENT_NAME",
    "GITHUB_EVENT_BEFORE",
    "GITEA_EVENT_BEFORE",
    "FORGEJO_EVENT_BEFORE",
    "CI_COMMIT_BEFORE_SHA",
    "DISCIPLINE_ACTOR",
    "GITHUB_ACTOR",
    "GITEA_ACTOR",
    "FORGEJO_ACTOR",
    "GITLAB_USER_LOGIN",
];

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

    pub fn git_output(&self, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
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

    /// `discipline check --format json --base main <extra>` with PR_BODY
    pub fn check_with_pr(&self, extra: &[&str], pr_body: &str) -> Run {
        let mut args = vec!["check", "--format", "json"];
        if !extra.contains(&"--staged") && !extra.contains(&"--base") {
            args.extend(["--base", "main"]);
        }
        args.extend(extra);
        self.run(
            &args,
            &[("PR_BODY", pr_body), ("PR_TITLE", "chore: test PR (#101)")],
        )
    }

    /// `discipline check --format json --base main <extra>` with PR_TITLE and/or PR_BODY
    pub fn check_with_pr_metadata(
        &self,
        extra: &[&str],
        pr_title: Option<&str>,
        pr_body: Option<&str>,
    ) -> Run {
        let mut args = vec!["check", "--format", "json"];
        if !extra.contains(&"--staged") && !extra.contains(&"--base") {
            args.extend(["--base", "main"]);
        }
        args.extend(extra);
        let mut env = Vec::new();
        if let Some(t) = pr_title {
            env.push(("PR_TITLE", t));
        }
        if let Some(b) = pr_body {
            env.push(("PR_BODY", b));
        }
        self.run(&args, &env)
    }

    pub fn commit_base(&self, rel: &str, content: &str, message: &str) {
        self.commit_base_files(&[(rel, content)], message);
    }

    pub fn commit_base_files(&self, files: &[(&str, &str)], message: &str) {
        self.git(&["checkout", "-q", "main"]);
        for (rel, content) in files {
            self.write(rel, content);
        }
        self.commit(message);
        self.git(&["checkout", "-q", "-B", "work", "main"]);
    }

    pub fn remove(&self, rel: &str) {
        let p = self.file(rel);
        if p.is_file() {
            std::fs::remove_file(p).unwrap();
        } else if p.is_dir() {
            std::fs::remove_dir_all(p).unwrap();
        }
    }

    pub fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_discipline"));
        cmd.args(args).current_dir(self.path());
        // Inherit nothing that could change the verdict.
        for var in ISOLATED_ENV_VARS {
            cmd.env_remove(var);
        }
        // Prefix-built names too (`DISCIPLINE_COMMAND_<GATE>`, ...).
        for (k, _) in std::env::vars() {
            if k.starts_with("DISCIPLINE_") {
                cmd.env_remove(k);
            }
        }
        // No test reaches a real forge: only a loopback FakeForge is allowed.
        cmd.env("DISCIPLINE_NO_NETWORK", "1");
        let has_pr_body = env.iter().any(|(k, _)| *k == "PR_BODY");
        let has_pr_title = env.iter().any(|(k, _)| *k == "PR_TITLE");
        if has_pr_body && !has_pr_title {
            cmd.env("PR_TITLE", "chore: test PR (#101)");
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
        for var in ISOLATED_ENV_VARS {
            cmd.env_remove(var);
        }
        // Prefix-built names too (`DISCIPLINE_COMMAND_<GATE>`, ...).
        for (k, _) in std::env::vars() {
            if k.starts_with("DISCIPLINE_") {
                cmd.env_remove(k);
            }
        }
        cmd.env("DISCIPLINE_NO_NETWORK", "1");
        cmd.envs(env.iter().copied());
        let out = cmd.output().unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

/// Canned responses by API path: status, extra headers, body.
type Routes = std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<String, (u16, Vec<(String, String)>, String)>>,
>;
/// Recorded requests: path and lower-cased headers.
type Requests = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<(String, String)>)>>>;

/// A forge REST API on a loopback port: canned responses by path, every request recorded.
/// Unknown paths answer 403 with a rate-limit message, like an exhausted anonymous quota.
pub struct FakeForge {
    addr: std::net::SocketAddr,
    routes: Routes,
    requests: Requests,
}

impl FakeForge {
    pub fn start() -> Self {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Routes = Default::default();
        let requests: Requests = Default::default();
        let (r, q) = (routes.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.trim_end().split_once(':') {
                        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                let path = target.trim_start_matches('/').to_string();
                q.lock().unwrap().push((path.clone(), headers));
                let (status, extra, body) = r.lock().unwrap().get(&path).cloned().unwrap_or((
                    403,
                    Vec::new(),
                    r#"{"message":"API rate limit exceeded"}"#.to_string(),
                ));
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in extra {
                    resp.push_str(&format!("{k}: {v}\r\n"));
                }
                resp.push_str("\r\n");
                resp.push_str(&body);
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        Self {
            addr,
            routes,
            requests,
        }
    }

    /// Base URL for `DISCIPLINE_FORGE_API_URL`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Answer `path` (relative to the API base, with any query) with 200 and `body`.
    pub fn serve(&self, path: &str, body: Value) {
        self.serve_raw(path, 200, &[], &body.to_string());
    }

    pub fn serve_raw(&self, path: &str, status: u16, headers: &[(&str, &str)], body: &str) {
        self.routes.lock().unwrap().insert(
            path.trim_start_matches('/').to_string(),
            (
                status,
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body.to_string(),
            ),
        );
    }

    /// Requests received so far: path and lower-cased headers.
    pub fn requests(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.requests.lock().unwrap().clone()
    }
}
