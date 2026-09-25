//! `discipline replay` through the real binary: a history of merged changes replayed
//! under a configuration, with and without the pull request body.

mod common;

use common::{FakeForge, Repo};
use serde_json::Value;

const WEAKENED: &str = "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n";

/// `main` gains two squash-merged changes: #2 weakens a test, #3 adds a doc.
fn history() -> (Repo, String) {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("tests/a.rs", WEAKENED);
    repo.commit("test: simplify adds (#2)");
    let weakening = repo.git_output(&["rev-parse", "HEAD"]).trim().to_string();
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes (#3)");
    (repo, weakening)
}

fn replay(repo: &Repo, extra: &[&str], env: &[(&str, &str)]) -> Value {
    let mut args = vec!["replay", "--last", "2", "--ref", "main", "--json"];
    args.extend_from_slice(extra);
    let run = repo.run(&args, env);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    serde_json::from_str(&run.stdout).unwrap()
}

fn verdicts(s: &Value) -> Vec<(u64, String)> {
    s["cases_detail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["pr"].as_u64().unwrap(),
                c["verdict"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn replay_reports_what_the_configuration_would_have_blocked() {
    let (repo, _) = history();
    let status_before = repo.git_output(&["status", "--porcelain"]);
    let head_before = repo.git_output(&["rev-parse", "HEAD"]);
    // Object ids, not `count-objects`: a repack moves loose objects into a pack without
    // adding any, and "no new objects" is what the replay promises.
    let all_objects = |r: &Repo| {
        r.git_output(&[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname)",
        ])
    };
    let objects_before = all_objects(&repo);

    let s = replay(&repo, &[], &[]);
    assert_eq!(
        verdicts(&s),
        [(3, "passed".to_string()), (2, "blocked".to_string())]
    );
    assert_eq!(
        s["errors_by_gate"]["assertion-reduction"],
        serde_json::json!(["#2"])
    );
    assert!(s["cases_detail"][1]["directives_from"]
        .as_str()
        .unwrap()
        .starts_with("commit message only"));

    // A candidate configuration without the gate would have let #2 through.
    let cfg = repo.file("candidate.toml");
    std::fs::write(
        &cfg,
        "[meta]\nversion = 1\nname = \"t\"\n[gates.assertion-reduction]\nenabled = false\n",
    )
    .unwrap();
    let lenient = replay(&repo, &["--config", cfg.to_str().unwrap()], &[]);
    assert_eq!(lenient["blocked"], 0, "{lenient}");
    std::fs::remove_file(&cfg).unwrap();

    // Neither replay touched the source repository: no worktree change, no moved HEAD,
    // no new objects (the candidate configuration's blob lives in the throwaway store).
    assert_eq!(repo.git_output(&["status", "--porcelain"]), status_before);
    assert_eq!(repo.git_output(&["rev-parse", "HEAD"]), head_before);
    assert_eq!(all_objects(&repo), objects_before);

    let text = repo.run(&["replay", "--last", "2", "--ref", "main"], &[]);
    assert!(
        text.stdout
            .contains("2 changes: 1 passed, 1 blocked, 0 could not be checked"),
        "{}",
        text.stdout
    );
    assert!(
        text.stdout
            .contains("directives read from the pull request body for 0 of 2 changes"),
        "{}",
        text.stdout
    );
    assert!(text.stderr.contains("replay 2/2:"), "{}", text.stderr);

    // Asking for more changes than the history holds says so.
    let short = repo.run(&["replay", "--last", "9", "--ref", "main"], &[]);
    assert!(
        short.stderr.contains("fewer than the 9 asked for"),
        "{}",
        short.stderr
    );
}

#[test]
fn replay_reads_the_waiver_in_the_merged_pull_request_body() {
    let (repo, weakening) = history();
    let api = FakeForge::start();
    api.serve(
        &format!("repos/o/r/commits/{weakening}/pulls"),
        serde_json::json!([{
            "number": 12,
            "merged_at": "2026-09-21T00:00:00Z",
            "user": {"login": "dev"},
            "body": "Simplified.\n\nallow-assertion-drop: adds the second check moved to an integration test",
            "head": {"sha": "feedbeef"}
        }]),
    );
    let url = api.url();
    let s = replay(
        &repo,
        &[],
        &[
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            // A replay inside a CI job that comments must not comment on anything.
            ("DISCIPLINE_COMMENT", "1"),
            ("GITHUB_TOKEN", "t"),
        ],
    );
    assert!(api.writes().is_empty(), "{:?}", api.writes());
    let case = &s["cases_detail"][1];
    assert_eq!(case["pr"], 12, "{s}");
    assert_eq!(case["directives_from"], "pull request body");
    assert_eq!(case["verdict"], "passed", "{s}");
}

#[test]
fn an_override_is_judged_against_the_pull_request_author_not_the_replaying_shell() {
    let (repo, weakening) = history();
    let cfg = repo.file("candidate.toml");
    std::fs::write(
        &cfg,
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\nfail_on_overrides = true\nallowed_override_actors = [\"dev\"]\n",
    )
    .unwrap();
    let run = |author: &str, shell_actor: &str| {
        let api = FakeForge::start();
        api.serve(
            &format!("repos/o/r/commits/{weakening}/pulls"),
            serde_json::json!([{
                "number": 12,
                "merged_at": "2026-09-21T00:00:00Z",
                "user": {"login": author},
                "body": "Simplified.\n\nallow-assertion-drop: adds the second check moved to an integration test",
                "head": {"sha": "feedbeef"}
            }]),
        );
        let url = api.url();
        replay(
            &repo,
            &["--config", cfg.to_str().unwrap()],
            &[
                ("GITHUB_REPOSITORY", "o/r"),
                ("DISCIPLINE_FORGE_API_URL", url.as_str()),
                ("GITHUB_TOKEN", "t"),
                ("GITHUB_ACTOR", shell_actor),
                ("DISCIPLINE_ACTOR", shell_actor),
            ],
        )["cases_detail"][1]
            .clone()
    };

    // The author is an allowed actor: the override stands, as it did in the change's CI.
    let allowed = run("dev", "someone-else");
    assert_eq!(allowed["verdict"], "passed", "{allowed}");
    assert_eq!(
        allowed["blocking_gates"],
        serde_json::json!([]),
        "{allowed}"
    );

    // Another author: the override is refused, and the case names the gate and the reason
    // even though no finding is an error. The replaying shell's actor does not rescue it.
    let refused = run("outsider", "dev");
    assert_eq!(refused["verdict"], "blocked", "{refused}");
    assert_eq!(
        refused["blocking_gates"],
        serde_json::json!(["assertion-reduction"]),
        "{refused}"
    );
    let detail = refused["detail"].as_str().unwrap();
    assert!(
        detail.contains("`fail_on_overrides` refused") && detail.contains("`outsider`"),
        "{detail}"
    );

    // No author on the pull request: no actor at all, not the replaying shell's.
    let anonymous = run("", "dev");
    assert_eq!(anonymous["verdict"], "blocked", "{anonymous}");
    assert!(
        anonymous["detail"].as_str().unwrap().contains("no actor"),
        "{anonymous}"
    );
}

#[test]
fn a_blocked_change_whose_pull_request_could_not_be_read_is_not_checked() {
    let (repo, _) = history();
    // The fake forge answers every lookup 403, as a rate-limited API does.
    let api = FakeForge::start();
    let url = api.url();
    let s = replay(
        &repo,
        &[],
        &[
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ],
    );
    assert_eq!(
        verdicts(&s),
        [
            (3, "passed".to_string()),
            (2, "could_not_check".to_string())
        ],
        "{s}"
    );
    assert!(s["errors_by_gate"].as_object().unwrap().is_empty(), "{s}");
    let reasons = s["could_not_check_by_reason"].as_object().unwrap();
    assert_eq!(reasons.len(), 1, "{s}");
    let (reason, changes) = reasons.iter().next().unwrap();
    assert!(
        reason.contains("pull request could not be read"),
        "{reason}"
    );
    assert_eq!(changes, &serde_json::json!(["#2"]));
}

#[test]
fn a_file_the_configuration_names_before_it_existed_skips_its_group_in_replay_only() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    let cfg = "[meta]\nversion = 1\nname = \"t\"\n[gates.version-lockstep]\nenabled = true\ngroups = [{ name = \"v\", sources = [\n  { path = \"pyproject.toml\", regex = 'version = \"([^\"]+)\"' },\n  { path = \"server.json\", regex = '\"version\": \"([^\"]+)\"' },\n]}]\n";
    repo.write("pyproject.toml", "version = \"1.0\"\n");
    repo.commit("build: project (#2)");
    repo.write("server.json", "{\"version\": \"1.0\"}\n");
    repo.commit("build: server manifest (#3)");
    let candidate = repo.file("candidate.toml");
    std::fs::write(&candidate, cfg).unwrap();
    let s = replay(&repo, &["--config", candidate.to_str().unwrap()], &[]);
    assert_eq!(
        verdicts(&s),
        [(3, "passed".to_string()), (2, "passed".to_string())],
        "{s}"
    );

    // Outside replay the same missing file is a configuration error.
    repo.git(&["checkout", "-q", "-B", "old", "HEAD~1"]);
    repo.write("discipline.toml", cfg);
    repo.commit("chore: configuration");
    let live = repo.check(&[]);
    assert_eq!(live.code, 2, "{}\n{}", live.stdout, live.stderr);
    assert!(
        live.stderr.contains("server.json` does not exist"),
        "{}",
        live.stderr
    );
}

/// Since git 2.54, `git commit` ends with `git maintenance run --auto`, detached, and its
/// repack fires when `objects/17/` holds two loose objects: a test repository's objects
/// could be packed in the background mid-test. The harness's git commands never start it,
/// even with a repository configured to run a maintenance task on every command.
#[test]
fn harness_git_commands_never_start_background_maintenance() {
    let repo = Repo::new();
    for kv in [
        ["maintenance.loose-objects.enabled", "true"],
        ["maintenance.loose-objects.auto", "-1"],
        ["maintenance.autoDetach", "false"],
        ["gc.autoDetach", "false"],
    ] {
        repo.git(&["config", kv[0], kv[1]]);
    }
    for i in 0..3 {
        repo.write(&format!("docs/n{i}.md"), &format!("# {i}\n"));
        repo.commit(&format!("docs: note {i}"));
    }
    let counts = repo.git_output(&["count-objects", "-v"]);
    assert!(
        counts.contains("\npacks: 0\n"),
        "a maintenance task packed objects:\n{counts}"
    );
}
