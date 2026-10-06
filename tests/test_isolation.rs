//! Process isolation of the test harness itself: a spawned binary must not see
//! CI/forge variables set in the parent process.

mod common;

use common::Repo;

/// The parent process forges CI state that would waive the weakened test below:
/// a `PR_BODY` waiver and a `GITHUB_EVENT_PATH` file carrying the same waiver. The
/// isolated helper scrubs both, so the run still reports the dropped assertion with no
/// override.
/// Killed mutants: `PR_BODY` left in the spawned environment by `common::isolate_env`,
/// and `GITHUB_EVENT_PATH` left in it.
#[test]
fn spawned_binary_does_not_inherit_forge_env_from_the_parent_process() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    // Outside the repository, so the event payload itself never enters the diff.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("event.json"),
        serde_json::json!({
            "pull_request": {
                "title": "chore: test PR (#101)",
                "body": "allow-assertion-drop: adds covered elsewhere",
            }
        })
        .to_string(),
    )
    .unwrap();
    let event = tmp.path().join("event.json");
    let _ambient = Ambient::set(&[
        ("PR_BODY", "allow-assertion-drop: adds covered elsewhere"),
        ("GITHUB_EVENT_PATH", event.to_str().unwrap()),
    ]);
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.json()["overrides"], 0, "{}", run.stdout);
    assert!(
        !run.titles("assertion-reduction").is_empty(),
        "the dropped assertion must still be reported: {}",
        run.stdout
    );
}

/// Every token variable the forge client reads, for any forge kind.
const TOKEN_VARIABLES: &[&str] = &[
    "DISCIPLINE_FORGE_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GITLAB_TOKEN",
    "GITEA_TOKEN",
    "FORGEJO_TOKEN",
];

/// The `Authorization` values the loopback forge received from one `doctor` run as
/// `forge`, with `env` given to the spawn by the test itself.
fn authorizations_sent(forge: &str, env: &[(&str, &str)]) -> Vec<String> {
    let repo = Repo::new();
    let api = common::FakeForge::start();
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://forge.example.com/o/r.git",
    ]);
    let url = api.url();
    let mut env = env.to_vec();
    env.push(("DISCIPLINE_FORGE", forge));
    env.push(("DISCIPLINE_FORGE_API_URL", url.as_str()));
    repo.run(&["doctor", "--format", "json"], &env);
    let requests = api.requests();
    assert!(
        !requests.is_empty(),
        "{forge}: the loopback forge was not asked"
    );
    requests
        .into_iter()
        .flat_map(|(_, headers)| headers)
        .filter(|(name, _)| name == "authorization")
        .map(|(_, value)| value)
        .collect()
}

/// A token of the parent process is a credential of whoever runs the tests: no spawn
/// may send it, to the loopback forge or anywhere. Each token variable is set alone, so
/// one that leaks is named, for every forge kind that reads it.
/// Killed mutants: each of the six names left in the spawned environment by
/// `common::isolate_env`, one at a time.
#[test]
fn a_token_of_the_parent_process_is_not_sent_to_the_forge() {
    for name in TOKEN_VARIABLES {
        let _ambient = Ambient::set(&[(name, "parent-process-token")]);
        for forge in ["github", "gitlab", "gitea", "forgejo"] {
            assert_eq!(
                authorizations_sent(forge, &[]),
                Vec::<String>::new(),
                "{name} of the parent process, as {forge}"
            );
        }
    }
    // Control: a token the test itself gives the spawn is sent, so the header would be
    // seen above had a parent's token got through.
    let sent = authorizations_sent("github", &[("GH_TOKEN", "test-token")]);
    assert!(
        !sent.is_empty() && sent.iter().all(|v| v == "Bearer test-token"),
        "{sent:?}"
    );
}

/// What the harness's own spawns do, one `<name>=<value>` line each. Every value is
/// decided by the fixture alone, so a variable of the parent process that changes one is
/// a variable the harness lets through.
fn ambient_environment_probe() -> Vec<String> {
    let mut seen = Vec::new();
    let repo = Repo::new();

    // The harness's own `git`: what it tracks, who it commits as, how it words a message.
    repo.write("notes.log", "kept\n");
    repo.commit("docs: notes");
    seen.push(format!(
        "git-tracks={}",
        repo.git_output(&["ls-files", "notes.log", "src/lib.rs"])
            .replace('\n', ",")
    ));
    seen.push(format!(
        "git-identity={}",
        repo.git_output(&["log", "-1", "--format=%an <%ae> / %cn <%ce>"])
    ));
    seen.push(format!(
        "git-wording={}",
        repo.git_output(&["status"]).lines().next().unwrap_or("")
    ));

    // A base that does not resolve is fetched in CI only, and the fetch is refused and
    // said so under the harness's DISCIPLINE_NO_NETWORK=1.
    let unresolved = repo.run(&["check", "--base", "origin/no-such-branch"], &[]);
    seen.push(format!(
        "ci-base-fetch={} {}",
        unresolved.code,
        unresolved.stderr.contains("git fetch")
    ));
    // With the fetch asked for (no remote, so nothing leaves the machine), the binary
    // quotes git's own message.
    let fetched = repo.run(
        &["check", "--base", "origin/no-such-branch"],
        &[("CI", "true"), ("DISCIPLINE_NO_NETWORK", "0")],
    );
    seen.push(format!(
        "binary-git-wording={}",
        fetched
            .stderr
            .contains("Could not read from remote repository")
    ));

    // Colour: none in a pipe, unless the test itself names a CI log.
    let usage = repo.run(&["check", "--no-such-flag"], &[]);
    seen.push(format!(
        "usage-error-colour={} {}",
        usage.code,
        usage.stderr.contains('\u{1b}')
    ));
    let ci_log = repo.run(&["check", "--base", "main"], &[("GITHUB_ACTIONS", "true")]);
    seen.push(format!(
        "ci-log-colour={} {}",
        ci_log.code,
        ci_log.stdout.contains('\u{1b}')
    ));

    // The loopback forge is reached directly, never through a proxy.
    let api = common::FakeForge::start();
    api.serve("repos/o/r", serde_json::json!({"default_branch": "main"}));
    api.serve_raw("repos/o/r/branch_protections", 200, &[], "[]");
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://gitea.example.com/o/r.git",
    ]);
    let url = api.url();
    repo.run(
        &["doctor", "--format", "json"],
        &[
            ("DISCIPLINE_FORGE", "gitea"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ],
    );
    seen.push(format!(
        "loopback-forge-reached={}",
        !api.requests().is_empty()
    ));

    // An SSH remote is read as written: no `~/.ssh/config` of this machine renames it.
    repo.git(&["remote", "set-url", "origin", "git@forge:o/r.git"]);
    let alias = repo.run(&["doctor"], &[("DISCIPLINE_FORGE", "gitea")]);
    seen.push(format!(
        "ssh-alias-unresolved={}",
        alias.stdout.contains("(https://forge)")
    ));

    // Names of the swept families that would reach a spawned binary or `git`.
    for (label, cmd) in [
        ("binary", common::discipline_cmd(repo.path())),
        ("git", common::git_command()),
    ] {
        let named: Vec<std::ffi::OsString> = cmd.get_envs().map(|(k, _)| k.to_owned()).collect();
        let mut inherited: Vec<String> = std::env::vars_os()
            .filter(|(k, _)| !named.contains(k))
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| PROBED_FAMILIES.iter().any(|p| k.starts_with(p)))
            .collect();
        inherited.sort();
        seen.push(format!("{label}-inherits={}", inherited.join(",")));
    }

    seen
}

/// Families no spawned command may inherit a name of.
const PROBED_FAMILIES: &[&str] = &[
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

/// What [`ambient_environment_probe`] returns, whatever the parent process carries.
const PROBE_EXPECTED: &[&str] = &[
    "git-tracks=notes.log,src/lib.rs",
    "git-identity=t <t@example.invalid> / t <t@example.invalid>",
    "git-wording=On branch work",
    "ci-base-fetch=2 false",
    "binary-git-wording=true",
    "usage-error-colour=2 false",
    "ci-log-colour=0 true",
    "loopback-forge-reached=true",
    "ssh-alias-unresolved=true",
    "binary-inherits=",
    "git-inherits=",
];

/// One test at a time changes this process's environment.
static AMBIENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Variables set in this process, the parent of every spawn, and put back on drop: the
/// only path by which an ambient variable reaches a command the harness builds.
struct Ambient {
    before: Vec<(String, Option<std::ffi::OsString>)>,
    _one_at_a_time: std::sync::MutexGuard<'static, ()>,
}

impl Ambient {
    fn set(env: &[(&str, &str)]) -> Self {
        let one_at_a_time = AMBIENT.lock().unwrap_or_else(|e| e.into_inner());
        let before = env
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var_os(k)))
            .collect();
        for (k, v) in env {
            std::env::set_var(k, v);
        }
        Self {
            before,
            _one_at_a_time: one_at_a_time,
        }
    }
}

impl Drop for Ambient {
    fn drop(&mut self) {
        for (k, v) in &self.before {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}

/// [`ambient_environment_probe`] with `env` set in this process.
fn probe_under(env: &[(&str, &str)]) -> Vec<String> {
    let _ambient = Ambient::set(env);
    ambient_environment_probe()
}

/// A directory laid out as a hostile home and as a hostile `XDG_CONFIG_HOME` at once: an
/// ignore file hiding `*.log` and `*.rs`, an identity, forced signing, and an SSH alias.
fn hostile_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let config = "[user]\n\tname = mallory\n\temail = mallory@example.invalid\n\tsigningkey = 0000000000000000\n[commit]\n\tgpgsign = true\n";
    for (rel, text) in [
        (".gitconfig", config),
        ("git/config", config),
        (".config/git/config", config),
        ("git/ignore", "*.log\n*.rs\n"),
        (".config/git/ignore", "*.log\n*.rs\n"),
        (
            ".ssh/config",
            "Host forge\n    HostName gitea.example.com\n",
        ),
    ] {
        let path = home.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    home
}

/// #572: a bare `CI` made the unresolved base a fetch, and a missing pull request exit 2.
#[test]
fn a_bare_ci_marker_of_the_parent_process_changes_nothing() {
    assert_eq!(probe_under(&[("CI", "true")]), PROBE_EXPECTED);
}

/// #572: runner variables no list names (`GITHUB_RUN_ID`, `RUNNER_OS`, ...) reached the
/// binary, and every CI family reached the harness's `git`.
#[test]
fn runner_variables_no_list_names_are_not_inherited() {
    let runner = [
        ("GITHUB_RUN_ID", "1"),
        ("GITHUB_SHA", "0000000000000000000000000000000000000000"),
        ("GITHUB_API_URL", "http://127.0.0.1:9"),
        ("RUNNER_OS", "Linux"),
        ("GITEA_RUN_NUMBER", "1"),
        ("FORGEJO_RUN_NUMBER", "1"),
        ("GITLAB_USER_ID", "1"),
        ("CI_JOB_ID", "1"),
        ("DISCIPLINE_COMMAND_UNIT_TESTS", "false"),
    ];
    assert_eq!(probe_under(&runner), PROBE_EXPECTED);
}

/// #572: `CLICOLOR_FORCE` coloured a usage error written to a pipe, and `NO_COLOR`
/// removed the colour a test asked for by naming a CI log.
#[test]
fn colour_variables_of_the_parent_process_change_nothing() {
    assert_eq!(probe_under(&[("CLICOLOR_FORCE", "1")]), PROBE_EXPECTED);
    assert_eq!(probe_under(&[("NO_COLOR", "1")]), PROBE_EXPECTED);
}

/// #572: the forge client took `HTTP_PROXY` for the loopback forge too.
#[test]
fn a_proxy_of_the_parent_process_is_not_used_for_the_loopback_forge() {
    // Port 9 (discard) on loopback: nothing listens, nothing leaves the machine.
    for name in ["HTTP_PROXY", "http_proxy", "ALL_PROXY"] {
        assert_eq!(
            probe_under(&[(name, "http://127.0.0.1:9")]),
            PROBE_EXPECTED,
            "{name}"
        );
    }
}

/// #572: `HOME` reached the binary (`~/.ssh/config`) and both it and `XDG_CONFIG_HOME`
/// reached every `git` (the user's ignore file, identity and signing setup).
#[test]
fn the_home_of_the_parent_process_is_never_read() {
    let home = hostile_home();
    let dir = home.path().to_str().unwrap();
    assert_eq!(probe_under(&[("HOME", dir)]), PROBE_EXPECTED, "HOME");
    assert_eq!(
        probe_under(&[("XDG_CONFIG_HOME", dir)]),
        PROBE_EXPECTED,
        "XDG_CONFIG_HOME"
    );
}

/// #572: the harness's `git` dropped only the variables that name a repository, so an
/// inherited identity or an injected configuration entry decided a fixture's commits.
#[test]
fn git_variables_of_the_parent_process_do_not_reach_the_fixture_git() {
    let home = hostile_home();
    let ignore = home.path().join("git/ignore");
    let global = home.path().join(".gitconfig");
    let injected = [
        ("GIT_AUTHOR_NAME", "mallory"),
        ("GIT_COMMITTER_EMAIL", "mallory@example.invalid"),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "core.excludesFile"),
        ("GIT_CONFIG_VALUE_0", ignore.to_str().unwrap()),
    ];
    assert_eq!(probe_under(&injected), PROBE_EXPECTED, "injected");
    assert_eq!(
        probe_under(&[("GIT_CONFIG_GLOBAL", global.to_str().unwrap())]),
        PROBE_EXPECTED,
        "GIT_CONFIG_GLOBAL"
    );
}

/// #572: `git` words its messages in the user's language, and the binary quotes them.
/// Shows a difference only where git's message catalogues are installed.
#[test]
fn the_locale_of_the_parent_process_does_not_reword_git() {
    let french = [
        ("LANGUAGE", "fr"),
        ("LANG", "fr_FR.UTF-8"),
        ("LC_ALL", "fr_FR.UTF-8"),
        ("LC_MESSAGES", "fr_FR.UTF-8"),
    ];
    assert_eq!(probe_under(&french), PROBE_EXPECTED);
}

/// Source text that starts the binary or `git` without the harness, or copies its scrub:
/// what the scan below counts in every test source, by the name it reports.
fn unisolated_spawn_needles() -> [(&'static str, String); 5] {
    // Written in pieces so this file does not match itself.
    [
        (
            "the binary's path",
            ["CARGO_BIN_EXE", "_discipline"].concat(),
        ),
        (
            "the binary's path from the harness",
            ["discipline", "_bin("].concat(),
        ),
        ("a bare git", ["Command::new(", "\"git\")"].concat()),
        (
            "a hand-rolled scrub (the variable list)",
            ["ISOLATED", "_ENV_VARS"].concat(),
        ),
        (
            "a hand-rolled scrub (git's variables)",
            ["GIT_REPOSITORY", "_ENV_VARS"].concat(),
        ),
    ]
}

/// The only occurrences the scan accepts outside `tests/common/mod.rs`: file, what, how
/// many, and why none of them is a spawn that inherits the parent's environment.
const SPAWN_ALLOW_LIST: &[(&str, &str, usize, &str)] = &[
    (
        "tests/test_gates_e2e.rs",
        "the binary's path",
        3,
        "source text of a fixture repository's own test helper, written to disk for the gates to read and never compiled here",
    ),
    (
        "tests/test_lease.rs",
        "the binary's path from the harness",
        1,
        "the directory of the binary goes first on PATH of an isolated `git_command()`, so the installed git hooks find it",
    ),
    (
        "src/gitctx.rs",
        "a bare git",
        1,
        "the binary's own base fetch, not a test; a unit test under src/ that runs git would raise the count",
    ),
    (
        "tests/test_config.rs",
        "a hand-rolled scrub (the variable list)",
        4,
        "the check that every name the binary reads is in the list; it spawns nothing",
    ),
];

/// Every `.rs` file under `dir`, as a path relative to the package root.
fn rust_sources(dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(dir)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                found.push(path.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

/// #572: `test_output_schemas.rs` and `test_stability_contract.rs` started the binary
/// with `Command::new(env!(..))`, `test_lease.rs` with its own looser scrub, and three
/// files ran a bare `git`. A spawn outside the harness inherits the parent's environment,
/// so a new one fails here until it goes through `common::discipline_cmd` /
/// `common::git_command`, or is recorded above with its reason.
#[test]
fn every_spawn_of_the_binary_and_of_git_goes_through_the_harness() {
    let mut found = std::collections::BTreeSet::new();
    for file in rust_sources("tests").into_iter().chain(rust_sources("src")) {
        if file == "tests/common/mod.rs" {
            continue;
        }
        // Without whitespace, so a call the formatter broke over lines still matches.
        let text: String = std::fs::read_to_string(&file)
            .unwrap()
            .split_whitespace()
            .collect();
        for (what, needle) in unisolated_spawn_needles() {
            let count = text.matches(needle.as_str()).count();
            if count > 0 {
                found.insert((file.clone(), what.to_string(), count));
            }
        }
    }
    let allowed: std::collections::BTreeSet<(String, String, usize)> = SPAWN_ALLOW_LIST
        .iter()
        .map(|(file, what, count, _)| (file.to_string(), what.to_string(), *count))
        .collect();
    let unlisted: Vec<_> = found.difference(&allowed).collect();
    let stale: Vec<_> = allowed.difference(&found).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "spawn the binary through common::discipline_cmd and git through common::git_command \
         (file, what, occurrences): {unlisted:#?}\nSPAWN_ALLOW_LIST entries that no longer match: {stale:#?}"
    );
}
