//! `discipline replay --last N`: what a configuration would have blocked.
//!
//! Each of the last N first-parent commits of a branch is one merged change. For each
//! commit C with parent P the change is rebuilt in a throwaway repository whose object
//! store borrows the source repository's (`objects/info/alternates`), so nothing is
//! written to the source repository:
//!
//! * B' = P's tree with the configuration under test as `discipline.toml`;
//! * H' = C's tree with the same configuration, C's message and author (a change to
//!   `discipline.toml` itself is not replayed: the configuration is what is measured);
//! * H' is checked out and this binary's `check --base B'` runs against it, with the
//!   body of the pull request C arrived through when the forge can be read (the same
//!   lookup as the `merged-pr-body` directive source), else the commit message only.
//!
//! The throwaway repository is removed when the replay ends, whatever the outcome.

use anyhow::{anyhow, bail, Context, Result};
use git2::{Commit, Oid, Repository};
use std::collections::BTreeMap;
use std::path::PathBuf;

const CONFIG_NAME: &str = "discipline.toml";

/// One replayed change.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Case {
    pub sha: String,
    /// The pull request number, from the forge or the subject's `(#N)`.
    pub pr: Option<u64>,
    pub subject: String,
    /// `passed`, `blocked` or `could_not_check`.
    pub verdict: &'static str,
    /// Gates with an `error` finding; when there is none, the gates whose overrides
    /// `fail_on_overrides` refused.
    pub blocking_gates: Vec<String>,
    /// Gates with a `warning` finding.
    pub warning_gates: Vec<String>,
    /// Where the directives came from: `pull request body`, or why they did not.
    pub directives_from: String,
    /// The child's stderr when it could not check; for a change blocked with no `error`
    /// finding, which overrides were refused and for which actor.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Summary {
    pub cases: usize,
    pub passed: usize,
    pub blocked: usize,
    pub could_not_check: usize,
    /// Gate id → the changes it blocked (`#N`, else the short sha).
    pub errors_by_gate: BTreeMap<String, Vec<String>>,
    /// Gate id → the number of changes with a warning from it.
    pub warnings_by_gate: BTreeMap<String, usize>,
    /// Why a change could not be checked → the changes (`#N`, else the short sha).
    pub could_not_check_by_reason: BTreeMap<String, Vec<String>>,
    pub cases_detail: Vec<Case>,
}

impl Case {
    pub fn label(&self) -> String {
        match self.pr {
            Some(n) => format!("#{n}"),
            None => self.sha.chars().take(10).collect(),
        }
    }
}

impl Summary {
    pub fn from_cases(cases: Vec<Case>) -> Self {
        let mut s = Summary {
            cases: cases.len(),
            ..Default::default()
        };
        for c in &cases {
            match c.verdict {
                "passed" => s.passed += 1,
                "blocked" => s.blocked += 1,
                _ => {
                    s.could_not_check += 1;
                    s.could_not_check_by_reason
                        .entry(reason(&c.detail))
                        .or_default()
                        .push(c.label());
                }
            }
            // A change that could not be checked keeps its gates in its detail only.
            for g in c.blocking_gates.iter().filter(|_| c.verdict == "blocked") {
                s.errors_by_gate
                    .entry(g.clone())
                    .or_default()
                    .push(c.label());
            }
            for g in &c.warning_gates {
                *s.warnings_by_gate.entry(g.clone()).or_default() += 1;
            }
        }
        s.cases_detail = cases;
        s
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for c in &self.cases_detail {
            let gates = if c.blocking_gates.is_empty() {
                String::new()
            } else {
                format!("  {}", c.blocking_gates.join(", "))
            };
            out.push_str(&format!(
                "{:<9} {:<16} {}{gates}\n",
                c.label(),
                c.verdict,
                c.subject.chars().take(72).collect::<String>()
            ));
        }
        out.push_str(&format!(
            "\n{} changes: {} passed, {} blocked, {} could not be checked\n",
            self.cases, self.passed, self.blocked, self.could_not_check
        ));
        let from_body = self
            .cases_detail
            .iter()
            .filter(|c| c.directives_from == "pull request body")
            .count();
        out.push_str(&format!(
            "directives read from the pull request body for {from_body} of {} changes; the rest used the commit message only\n",
            self.cases
        ));
        for (g, changes) in &self.errors_by_gate {
            out.push_str(&format!(
                "  error    {g:<24} {} change(s): {}\n",
                changes.len(),
                changes.join(" ")
            ));
        }
        for (g, n) in &self.warnings_by_gate {
            out.push_str(&format!("  warning  {g:<24} {n} change(s)\n"));
        }
        for (r, changes) in &self.could_not_check_by_reason {
            out.push_str(&format!(
                "  could not check {} change(s): {r}\n    {}\n",
                changes.len(),
                changes.join(" ")
            ));
        }
        out
    }
}

/// The reason a case could not be checked: the last line of its detail, without the
/// `discipline check: error: ` prefix, so identical failures group together.
fn reason(detail: &str) -> String {
    let last = detail
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let r = last.trim().trim_start_matches("discipline check: error: ");
    if r.is_empty() {
        "no reason given".to_string()
    } else {
        r.to_string()
    }
}

/// The verdict and gates of one `check --format json` run.
pub fn read_verdict(code: i32, json: &str) -> (&'static str, Vec<String>, Vec<String>) {
    let verdict = match code {
        0 => "passed",
        1 => "blocked",
        _ => return ("could_not_check", Vec::new(), Vec::new()),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        for o in v["outcomes"].as_array().into_iter().flatten() {
            let gate = o["gate"].as_str().unwrap_or("").to_string();
            let sevs: Vec<&str> = o["violations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x["severity"].as_str())
                .collect();
            if sevs.contains(&"error") {
                errors.push(gate.clone());
            }
            if sevs.contains(&"warning") {
                warnings.push(gate);
            }
        }
    }
    (verdict, errors, warnings)
}

/// Gates that applied an override, in report order.
pub fn gates_with_overrides(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    v["outcomes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["overrides"].as_array().is_some_and(|a| !a.is_empty()))
        .filter_map(|o| o["gate"].as_str().map(str::to_string))
        .collect()
}

/// Why a change with no `error` finding was blocked: `fail_on_overrides` refused its
/// overrides, since its actor is not in `allowed_override_actors`.
pub fn refused_overrides(json: &str, actor: Option<&str>) -> String {
    let gates = gates_with_overrides(json);
    if gates.is_empty() {
        return String::new();
    }
    let by = actor.map_or(
        "no actor (no merged pull request author)".to_string(),
        |a| format!("actor `{a}`"),
    );
    format!(
        "no error finding: `fail_on_overrides` refused the override(s) of {} applied by {by}, who is not in `allowed_override_actors`",
        gates.join(", ")
    )
}

/// `(#123)` at the end of a squash-merge subject.
pub fn pr_from_subject(subject: &str) -> Option<u64> {
    let open = subject.rfind("(#")?;
    let rest = &subject[open + 2..];
    let close = rest.find(')')?;
    rest[..close].parse().ok()
}

/// A temp directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let base = std::env::temp_dir();
        for i in 0..100u32 {
            let p = base.join(format!("discipline-replay-{}-{i}", std::process::id()));
            if std::fs::create_dir(&p).is_ok() {
                return Ok(TempDir(p));
            }
        }
        bail!(
            "cannot create a temporary directory under {}",
            base.display()
        )
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "discipline replay: could not remove {}: {e}",
                self.0.display()
            );
        }
    }
}

/// `tree` with `discipline.toml` at the root set to `config` (or removed when `None`).
fn with_config(repo: &Repository, tree: &git2::Tree, config: Option<Oid>) -> Result<Oid> {
    let mut b = repo.treebuilder(Some(tree))?;
    match config {
        Some(blob) => {
            b.insert(CONFIG_NAME, blob, 0o100644)?;
        }
        None => {
            if b.get(CONFIG_NAME)?.is_some() {
                b.remove(CONFIG_NAME)?;
            }
        }
    }
    Ok(b.write()?)
}

pub struct Options {
    pub last: usize,
    /// The branch whose history is replayed (default: the default branch).
    pub reference: Option<String>,
    /// The configuration under test (default: `discipline.toml` in the working tree).
    pub config: Option<PathBuf>,
}

/// The first-parent commits to replay, newest first.
pub fn commits_to_replay<'r>(
    repo: &'r Repository,
    tip: Oid,
    last: usize,
) -> Result<Vec<Commit<'r>>> {
    let mut out = Vec::new();
    let mut cur = repo.find_commit(tip)?;
    while out.len() < last {
        let Ok(parent) = cur.parent(0) else {
            break; // the root commit has no change to replay
        };
        out.push(cur);
        cur = parent;
    }
    Ok(out)
}

pub fn run(opts: &Options) -> Result<Summary> {
    if opts.last == 0 {
        bail!("--last must be at least 1");
    }
    let src = crate::gitctx::discover_repository(".")?;
    let workdir = src
        .workdir()
        .ok_or_else(|| anyhow!("bare repositories are not supported"))?
        .to_path_buf();
    let reference = match &opts.reference {
        Some(r) => r.clone(),
        None => crate::hook::default_base(&src).unwrap_or_else(|| "HEAD".to_string()),
    };
    let tip = src
        .revparse_single(&reference)
        .with_context(|| format!("`{reference}` does not resolve"))?
        .peel_to_commit()?
        .id();
    let config_bytes: Option<Vec<u8>> = match &opts.config {
        Some(p) => Some(std::fs::read(p).with_context(|| format!("cannot read {}", p.display()))?),
        None => std::fs::read(workdir.join(CONFIG_NAME)).ok(),
    };

    let tmp = TempDir::new()?;
    let scratch = Repository::init(tmp.0.join("tree"))?;
    let alternates = scratch.path().join("objects/info/alternates");
    std::fs::write(
        &alternates,
        format!("{}\n", src.commondir().join("objects").display()),
    )?;
    let scratch = Repository::open(tmp.0.join("tree"))?;
    if let Some(url) = src
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().ok().map(str::to_string))
    {
        scratch.remote("origin", &url)?;
    }
    let config_blob = config_bytes
        .as_deref()
        .map(|b| scratch.blob(b))
        .transpose()?;

    let forge = {
        let origin = src
            .find_remote("origin")
            .ok()
            .and_then(|r| r.url().ok().map(str::to_string))
            .map(|o| {
                crate::forge::resolve_ssh_alias(&o, &|a| crate::forge::ssh_hostname_from_home(a))
            });
        crate::forge::detect(&|k| std::env::var(k).ok(), origin.as_deref())
    };
    let api = crate::forge::HttpApi::from_env();
    let exe = std::env::current_exe().context("cannot locate the discipline binary")?;

    let mut cases = Vec::new();
    let commits = commits_to_replay(&src, tip, opts.last)?;
    if commits.len() < opts.last {
        eprintln!(
            "replay: {} change(s) before the root commit, fewer than the {} asked for (the root commit has no parent to compare with)",
            commits.len(),
            opts.last
        );
    }
    for c in commits {
        let parent = c.parent(0)?;
        let subject = c.summary().ok().flatten().unwrap_or("").to_string();
        let sig = git2::Signature::now("discipline replay", "replay@discipline.invalid")?;
        let base_tree = scratch.find_tree(with_config(&scratch, &parent.tree()?, config_blob)?)?;
        let base = scratch.commit(None, &sig, &sig, "replay base", &base_tree, &[])?;
        let head_tree = scratch.find_tree(with_config(&scratch, &c.tree()?, config_blob)?)?;
        let base_commit = scratch.find_commit(base)?;
        let head = scratch.commit(
            None,
            &c.author(),
            &c.committer(),
            c.message().unwrap_or(""),
            &head_tree,
            &[&base_commit],
        )?;
        scratch.set_head_detached(head)?;
        scratch.checkout_head(Some(
            git2::build::CheckoutBuilder::new()
                .force()
                .remove_untracked(true),
        ))?;

        let mut lookup_error = None;
        let (pr, body, author, directives_from) = match &forge {
            Ok(f) => match crate::forge::merged_pull_for_commit(&api, f, &c.id().to_string()) {
                Ok(Some(m)) => (
                    Some(m.number),
                    Some(m.body),
                    Some(m.author).filter(|a| !a.trim().is_empty()),
                    "pull request body".to_string(),
                ),
                Ok(None) => (
                    None,
                    None,
                    None,
                    "commit message only: no merged pull request".to_string(),
                ),
                Err(e) => {
                    lookup_error = Some(e.to_string());
                    (None, None, None, format!("commit message only: {e}"))
                }
            },
            Err(e) => (None, None, None, format!("commit message only: {e}")),
        };
        let body_file = tmp.0.join("body.txt");
        std::fs::write(&body_file, body.as_deref().unwrap_or(""))?;

        let mut cmd = std::process::Command::new(&exe);
        cmd.env(crate::guards::REPLAY_CASE_ENV, "1")
            .current_dir(tmp.0.join("tree"))
            .args(["check", "--format", "json", "--base"])
            .arg(base.to_string())
            .arg("--pr-body-file")
            .arg(&body_file);
        // The pull request's author stands in for the CI actor that ran its check, so
        // `allowed_override_actors` is judged per change, never as whoever runs the replay.
        if let Some(a) = &author {
            cmd.arg("--actor").arg(a);
        }
        for (k, _) in std::env::vars() {
            let event = k.ends_with("_EVENT_NAME")
                || k.ends_with("_EVENT_BEFORE")
                || k.ends_with("_EVENT_PATH");
            if event
                || matches!(
                    k.as_str(),
                    "PR_BODY"
                        | "PR_TITLE"
                        | "DISCIPLINE_BASE_REF"
                        | "DISCIPLINE_COMMENT"
                        | "GITLAB_CI"
                        | "CI_PIPELINE_SOURCE"
                        | "CI_COMMIT_BEFORE_SHA"
                        | "DISCIPLINE_ACTOR"
                        | "GITHUB_ACTOR"
                        | "GITEA_ACTOR"
                        | "FORGEJO_ACTOR"
                        | "GITLAB_USER_LOGIN"
                )
            {
                cmd.env_remove(&k);
            }
        }
        let out = cmd.output().context("cannot run discipline check")?;
        let code = out.status.code().unwrap_or(2);
        let (mut verdict, blocking, warning) =
            read_verdict(code, &String::from_utf8_lossy(&out.stdout));
        let mut detail = if verdict == "could_not_check" {
            String::from_utf8_lossy(&out.stderr).trim().to_string()
        } else if verdict == "blocked" && blocking.is_empty() {
            refused_overrides(&String::from_utf8_lossy(&out.stdout), author.as_deref())
        } else {
            String::new()
        };
        let blocking = if verdict == "blocked" && blocking.is_empty() {
            gates_with_overrides(&String::from_utf8_lossy(&out.stdout))
        } else {
            blocking
        };
        // A forge that was found but could not answer may hold a merged pull request whose
        // body lifts these findings: the change's verdict is unknown, not blocked.
        if let (Some(e), "blocked") = (&lookup_error, verdict) {
            verdict = "could_not_check";
            detail = format!(
                "its merged pull request could not be read, and its body may carry directives: {e} (a forge token in DISCIPLINE_FORGE_TOKEN raises the API rate limit)"
            );
        }
        eprintln!(
            "replay {}/{}: {} {}",
            cases.len() + 1,
            opts.last,
            &c.id().to_string()[..10],
            verdict
        );
        cases.push(Case {
            sha: c.id().to_string(),
            pr: pr.or_else(|| pr_from_subject(&subject)),
            subject,
            verdict,
            blocking_gates: blocking,
            warning_gates: warning,
            directives_from,
            detail,
        });
    }
    Ok(Summary::from_cases(cases))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_and_gates_are_read_from_the_report() {
        let json = r#"{"outcomes":[
            {"gate":"assertion-reduction","violations":[{"severity":"error"}]},
            {"gate":"pr-checklist","violations":[{"severity":"warning"}]},
            {"gate":"pii","violations":[]}]}"#;
        let (v, e, w) = read_verdict(1, json);
        assert_eq!(
            (v, e, w),
            (
                "blocked",
                vec!["assertion-reduction".to_string()],
                vec!["pr-checklist".to_string()]
            )
        );
        assert_eq!(read_verdict(0, "{}").0, "passed");
        assert_eq!(read_verdict(2, json), ("could_not_check", vec![], vec![]));
    }

    #[test]
    fn a_pull_request_number_comes_from_the_squash_subject() {
        assert_eq!(pr_from_subject("fix(x): y (#1028)"), Some(1028));
        assert_eq!(pr_from_subject("no number"), None);
        assert_eq!(pr_from_subject("a (#12) b (#34)"), Some(34));
    }

    #[test]
    fn the_summary_counts_verdicts_and_names_blocked_changes_per_gate() {
        let case = |pr, verdict, e: &[&str], w: &[&str]| Case {
            sha: "0123456789abcdef".into(),
            pr,
            subject: "s".into(),
            verdict,
            blocking_gates: e.iter().map(|s| s.to_string()).collect(),
            warning_gates: w.iter().map(|s| s.to_string()).collect(),
            directives_from: String::new(),
            detail: String::new(),
        };
        let s = Summary::from_cases(vec![
            case(Some(1), "blocked", &["pii"], &[]),
            case(None, "blocked", &["pii", "ci-integrity"], &["pr-checklist"]),
            case(Some(3), "passed", &[], &["pr-checklist"]),
            case(Some(4), "could_not_check", &[], &[]),
        ]);
        assert_eq!(
            (s.cases, s.passed, s.blocked, s.could_not_check),
            (4, 1, 2, 1)
        );
        assert_eq!(s.errors_by_gate["pii"], vec!["#1", "0123456789"]);
        assert_eq!(s.warnings_by_gate["pr-checklist"], 2);
        assert_eq!(s.could_not_check_by_reason["no reason given"], vec!["#4"]);
        let lockstep = |pr| Case {
            detail: "replay noise\ndiscipline check: error: gate version-lockstep could not run\n"
                .into(),
            ..case(Some(pr), "could_not_check", &[], &[])
        };
        let grouped = Summary::from_cases(vec![lockstep(5), lockstep(6)]);
        assert_eq!(
            grouped.could_not_check_by_reason["gate version-lockstep could not run"],
            vec!["#5", "#6"]
        );
        assert!(grouped.render().contains(
            "could not check 2 change(s): gate version-lockstep could not run\n    #5 #6"
        ));
        assert!(s
            .render()
            .contains("4 changes: 1 passed, 2 blocked, 1 could not be checked"));
    }
}
