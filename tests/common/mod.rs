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

    /// `(reason, gate)` of a run that could not check (exit 2, `--format json`).
    pub fn could_not_check(&self) -> (String, Option<String>) {
        let c = &self.json()["could_not_check"];
        assert!(
            c.is_object(),
            "no could_not_check in the report:\n{}\n{}",
            self.stdout,
            self.stderr
        );
        (
            c["reason"].as_str().unwrap().to_string(),
            c["gate"].as_str().map(str::to_string),
        )
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

/// `COPILOT_HOME` for a test that does not set its own: a directory that does not exist.
pub const NO_COPILOT_HOME: &str = "/nonexistent/discipline-test-copilot-home";

/// `HOME` for a test that does not set its own: a directory that does not exist, so
/// neither the binary nor a `git` it runs reads this machine's `~/.gitconfig`,
/// `~/.config/git/ignore` or `~/.ssh/config`.
pub const NO_HOME: &str = "/nonexistent/discipline-test-home";

/// Name families swept whole from a spawned command's environment, whatever follows the
/// prefix: a name the binary builds at run time (`DISCIPLINE_COMMAND_<GATE>`), a variable
/// a runner sets that no list names yet, git's own, and the locale categories (`LC_ALL`,
/// `LC_MESSAGES`, ...).
pub const ISOLATED_ENV_PREFIXES: &[&str] = &[
    "DISCIPLINE_",
    "GIT_",
    "GITHUB_",
    "GITEA_",
    "FORGEJO_",
    "GITLAB_",
    "CI_",
    "RUNNER_",
    "LC_",
];

/// Path of the built `discipline` binary. A test spawns it through [`discipline_cmd`];
/// this is for a test that puts its directory on a `PATH` (a git hook, a stub).
pub fn discipline_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_discipline"))
}

/// Removes from `cmd` every inherited variable that could change what the binary, or a
/// program it runs, does, and sets the few the harness controls. Applied to every
/// `discipline` and every `git` a test spawns.
pub fn isolate_env(cmd: &mut Command) {
    // Inherit nothing that could change the verdict.
    for var in ISOLATED_ENV_VARS.iter().chain(GIT_REPOSITORY_ENV_VARS) {
        cmd.env_remove(var);
    }
    for (k, _) in std::env::vars_os() {
        if k.to_str()
            .is_some_and(|k| ISOLATED_ENV_PREFIXES.iter().any(|p| k.starts_with(p)))
        {
            cmd.env_remove(k);
        }
    }
    // No test reaches a real forge: only a loopback FakeForge is allowed.
    cmd.env("DISCIPLINE_NO_NETWORK", "1");
    // Nor reads this machine's Copilot CLI configuration (trusted folders, hooks).
    cmd.env("COPILOT_HOME", NO_COPILOT_HOME);
    // Nor this user's home directory, nor the machine's git configuration. `HOME` (with
    // `XDG_CONFIG_HOME` removed) is where both the `git` program and the binary's
    // in-process git library look for the user's configuration. `GIT_CONFIG_NOSYSTEM`
    // is read by the `git` program only: the library's system file (`/etc/gitconfig`)
    // has no variable, so a machine that has one is not isolated from it here.
    cmd.env("HOME", NO_HOME);
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
}

/// A `discipline` binary invocation in `dir` with full process isolation
/// ([`isolate_env`]): `DISCIPLINE_NO_NETWORK=1`, a hermetic `HOME` and `COPILOT_HOME`,
/// and no CI, forge, git, proxy, colour or locale variable of the parent process. Every
/// test that spawns the binary builds on this; nothing reaches a real forge.
/// `tests/test_isolation.rs` fails on a spawn that does not.
pub fn discipline_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new(discipline_bin());
    cmd.current_dir(dir);
    isolate_env(&mut cmd);
    cmd
}

/// A program a test runs that is neither the binary nor `git` (`bash`, `python3`,
/// `node`), found through `PATH` and under [`isolate_env`]: a script sees no token, CI
/// marker, home directory or temporary directory of whoever runs the tests. A test that
/// needs one of them sets it on the returned command.
pub fn script_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    isolate_env(&mut cmd);
    cmd
}

/// Caller-supplied environment over an isolated command: a `PR_BODY` without a
/// `PR_TITLE` still gets the default title `check` expects.
fn apply_run_env(cmd: &mut Command, env: &[(&str, &str)]) {
    let has_pr_body = env.iter().any(|(k, _)| *k == "PR_BODY");
    let has_pr_title = env.iter().any(|(k, _)| *k == "PR_TITLE");
    if has_pr_body && !has_pr_title {
        cmd.env("PR_TITLE", "chore: test PR (#101)");
    }
    cmd.envs(env.iter().copied());
}

fn finish_run(out: std::process::Output) -> Run {
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The `[meta]` header every test configuration starts from.
pub const CONFIG_HEAD: &str = "[meta]\nversion = 1\nname = \"t\"\n";

/// Variables that point git at a repository other than the one in the working directory.
/// `git rebase --exec` and git hooks export them; a fixture that inherits `GIT_DIR` would
/// re-initialise or commit to that repository instead of its own.
pub const GIT_REPOSITORY_ENV_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// Every `git` command a test runs, under [`isolate_env`]: [`GIT_REPOSITORY_ENV_VARS`] and
/// every other `GIT_*` are gone, and with [`NO_HOME`] there is no global configuration,
/// ignore file or identity to read.
pub fn git_command() -> Command {
    let mut cmd = Command::new("git");
    isolate_env(&mut cmd);
    cmd
}

pub const ISOLATED_ENV_VARS: &[&str] = &[
    "DISCIPLINE_HOOK_RUN",
    "PR_BODY",
    "DISCIPLINE_PR_BODY_FILE",
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
    "DISCIPLINE_REPLAY_CASE",
    "DISCIPLINE_ENABLE",
    "DISCIPLINE_DISABLE",
    "DISCIPLINE_BASE_REF",
    "DISCIPLINE_FAIL_ON_WARNINGS",
    "DISCIPLINE_FAIL_ON_OVERRIDES",
    "DISCIPLINE_ADVISORY",
    "DISCIPLINE_COMMENT",
    "DISCIPLINE_POLICY_FROM",
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
    "DISCIPLINE_TEST_BASE_REPORT",
    "DISCIPLINE_TEST_HEAD_REPORT",
    "DISCIPLINE_TEST_REPORT",
    "DISCIPLINE_TRUST_WORKSPACE",
    "DOCS_HOSTNAME_DENYLIST",
    "FORGEJO_EVENT_NAME",
    "GITEA_EVENT_NAME",
    "GITHUB_HEAD_REF",
    "GITHUB_OUTPUT",
    "GITHUB_REF",
    "GITHUB_REF_NAME",
    "GITHUB_REF_TYPE",
    "GITHUB_WORKFLOW_REF",
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
    // Named by the CI-condition reader as variables it recognises in the code under
    // review; the binary does not read them, and removing them keeps that true of a test.
    "GITHUB_RUN_ID",
    "CI_JOB_ID",
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
    "CI_MERGE_REQUEST_IID",
    "CI_MERGE_REQUEST_SOURCE_BRANCH_SHA",
    "CI_COMMIT_SHA",
    // Names without a family prefix, which `ISOLATED_ENV_PREFIXES` cannot sweep.
    // A bare `CI` makes a missing pull request a configuration error and allows the
    // base fetch.
    "CI",
    // Colour: the report honours `NO_COLOR`, and the argument parser `CLICOLOR_FORCE`
    // (through `anstyle-query`) even when the output is a pipe.
    "NO_COLOR",
    "CLICOLOR_FORCE",
    // The forge client reads its proxy from the environment, for a loopback address too.
    "HTTP_PROXY",
    "http_proxy",
    // Where the user's configuration lives: `HOME` is set to `NO_HOME`, these are removed.
    "USERPROFILE",
    "XDG_CONFIG_HOME",
    // Locale: `git` words its messages in the user's language, and the binary quotes
    // them. With these and the `LC_` prefix gone, every program runs in the C locale.
    "LANG",
    "LANGUAGE",
    // Where temporary files go: every spawned program uses the platform's default, not a
    // directory the parent process names. A fixture's own directories are created by the
    // test process and passed by path.
    "TMPDIR",
];

/// Configuration every harness git command runs with. No signing and no hooks, and no
/// automatic maintenance: since git 2.54 a commit ends with a detached
/// `git maintenance run --auto` whose repack fires on two loose objects in `objects/17/`,
/// so a test repository could be repacked in the background mid-test.
const HARNESS_GIT_CONFIG: &[&str] = &[
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "maintenance.auto=false",
    "-c",
    "gc.auto=0",
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
        let out = git_command()
            .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
            .args(HARNESS_GIT_CONFIG)
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
        let out = git_command()
            .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
            .args(HARNESS_GIT_CONFIG)
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
        let mut cmd = discipline_cmd(self.path());
        cmd.args(args);
        apply_run_env(&mut cmd, env);
        finish_run(cmd.output().unwrap())
    }

    pub fn run_in_dir(&self, rel_dir: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = discipline_cmd(&self.file(rel_dir));
        cmd.args(args);
        apply_run_env(&mut cmd, env);
        finish_run(cmd.output().unwrap())
    }
}

/// Canned responses by API path: status, extra headers, body.
type Routes = std::sync::Arc<
    std::sync::Mutex<std::collections::HashMap<String, (u16, Vec<(String, String)>, String)>>,
>;
/// Answers given once each, in order, before a path falls back to its route.
type Queued = std::sync::Arc<
    std::sync::Mutex<
        std::collections::HashMap<
            String,
            std::collections::VecDeque<(u16, Vec<(String, String)>, String)>,
        >,
    >,
>;
/// Recorded requests: path and lower-cased headers.
type Requests = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<(String, String)>)>>>;
/// Recorded writes: method, path and body.
type Writes = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

/// The merged pull request lookup of the commit that `path` asks for, when `path` is a
/// forge's "get a single commit" endpoint: Gitea and Forgejo
/// `repos/{owner}/{repo}/git/commits/{sha}`, GitLab
/// `projects/{id}/repository/commits/{sha}`.
fn pulls_route_of(path: &str) -> Option<String> {
    if let Some((repo, sha)) = path.rsplit_once("/git/commits/") {
        return (!sha.contains('/')).then(|| format!("{repo}/commits/{sha}/pull"));
    }
    let (project, sha) = path.rsplit_once("/repository/commits/")?;
    (!sha.contains('/')).then(|| format!("{project}/repository/commits/{sha}/merge_requests"))
}

/// A forge REST API on a loopback port: canned responses by path, every request recorded.
/// Unknown paths answer 403 with a rate-limit message, like an exhausted anonymous quota.
///
/// One exception. Gitea, Forgejo and GitLab answer 404 for the merged pull request of a
/// commit both when no merged pull request carries it and when the forge does not have
/// the commit, so the binary then asks for the commit itself. A test that serves a
/// commit's pull request lookup is about a commit the forge has: its commit endpoint
/// answers 200 with a body that names the commit (`sha` and `id`) unless the test serves
/// that too (a 404 there is "not on the forge").
pub struct FakeForge {
    addr: std::net::SocketAddr,
    routes: Routes,
    queued: Queued,
    requests: Requests,
    writes: Writes,
}

impl FakeForge {
    pub fn start() -> Self {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Routes = Default::default();
        let queued: Queued = Default::default();
        let requests: Requests = Default::default();
        let writes: Writes = Default::default();
        let (r, qd, q, w) = (
            routes.clone(),
            queued.clone(),
            requests.clone(),
            writes.clone(),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let method = line.split_whitespace().next().unwrap_or("GET").to_string();
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
                let length: usize = headers
                    .iter()
                    .find(|(k, _)| k == "content-length")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                if method != "GET" {
                    let mut body = vec![0u8; length];
                    let _ = std::io::Read::read_exact(&mut reader, &mut body);
                    w.lock().unwrap().push((
                        method.clone(),
                        path.clone(),
                        String::from_utf8_lossy(&body).into_owned(),
                    ));
                }
                q.lock().unwrap().push((path.clone(), headers));
                let once = qd
                    .lock()
                    .unwrap()
                    .get_mut(&path)
                    .and_then(|answers| answers.pop_front());
                let (status, extra, body) = once.unwrap_or_else(|| {
                    let routes = r.lock().unwrap();
                    match routes.get(&path) {
                        Some(answer) => answer.clone(),
                        // The commit itself, asked for after its merged pull request
                        // lookup answered 404: the forge has the commit.
                        None if pulls_route_of(&path)
                            .is_some_and(|pulls| routes.contains_key(&pulls)) =>
                        {
                            let sha = path.rsplit('/').next().unwrap_or_default();
                            (
                                200,
                                Vec::new(),
                                serde_json::json!({"sha": sha, "id": sha}).to_string(),
                            )
                        }
                        None => (
                            403,
                            Vec::new(),
                            r#"{"message":"API rate limit exceeded"}"#.to_string(),
                        ),
                    }
                });
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
            queued,
            requests,
            writes,
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

    /// Answer `path` once with `status`, `headers` and `body`, before any earlier queued
    /// answer is used up and the path falls back to its route.
    pub fn queue(&self, path: &str, status: u16, headers: &[(&str, &str)], body: &str) {
        self.queued
            .lock()
            .unwrap()
            .entry(path.trim_start_matches('/').to_string())
            .or_default()
            .push_back((
                status,
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                body.to_string(),
            ));
    }

    /// Writes received so far: method, path and body.
    pub fn writes(&self) -> Vec<(String, String, String)> {
        self.writes.lock().unwrap().clone()
    }

    /// Requests received so far: path and lower-cased headers.
    pub fn requests(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.requests.lock().unwrap().clone()
    }
}
