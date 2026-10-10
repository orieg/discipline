//! `doctor::run`: reads the local files and the platform settings and assembles the
//! report.

use super::{
    agent_permission_findings, agent_sandbox_findings, analyse_gitlab_ci, analyse_workflows,
    codeowners_finding, copilot_trust_finding, default_branch, gitea_protection, github_protection,
    gitlab_protection, merge_methods, multi_agent_findings, mutation_preset_finding,
    protection_findings, push_covers, read, security_policy_finding, test_report_finding,
    DisciplineJob, Finding, Report, Status, CODEOWNERS_PATHS, SECURITY_POLICY_PATHS, WORKFLOW_DIRS,
};
use crate::forge::{Forge, ForgeApi, ForgeKind};
use std::path::Path;

// ---------------------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------------------

/// Inputs to one doctor run, gathered by the caller so the logic is testable offline.
pub struct DoctorInput<'a> {
    pub root: &'a Path,
    /// The forge hosting the repository, or why it is unknown.
    pub forge: Result<Forge, String>,
    pub branch: Option<String>,
    pub local_only: bool,
    pub api: &'a dyn ForgeApi,
    /// Copilot CLI's home directory (its `config.json` lists the folders it trusts).
    pub copilot_home: Option<std::path::PathBuf>,
    /// The user's home directory, where each agent keeps its user-level settings.
    pub home: Option<std::path::PathBuf>,
}

/// The files whose `uses:` references `doctor_pins` reads: the workflows, the repository's
/// own action metadata (`action.yml` / `action.yaml`) and local actions under
/// `.github/actions/<name>/`.
fn pin_sources(root: &Path, workflows: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = workflows.to_vec();
    let mut add = |rel: String| {
        if let Some(c) = read(root, &rel) {
            out.push((rel, c));
        }
    };
    add("action.yml".into());
    add("action.yaml".into());
    if let Ok(entries) = std::fs::read_dir(root.join(".github/actions")) {
        let mut dirs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        dirs.sort();
        for d in dirs {
            add(format!(".github/actions/{d}/action.yml"));
            add(format!(".github/actions/{d}/action.yaml"));
        }
    }
    out
}

/// `push-trigger` when `merged-pr-body` is on and every push-run discipline job is granted
/// `pull-requests: read` (or `write`): the push run reads the merged pull request's body.
fn pr_read_granted(jobs: &str, when: &str) -> Finding {
    Finding::new(
        "push-trigger",
        Status::Pass,
        format!("{jobs} run(s) discipline {when}; `merged-pr-body` is enabled and the workflow grants `pull-requests: read`, so the push run reads a merged pull request's body"),
    )
}

/// Run every check.
pub fn run(input: &DoctorInput) -> Report {
    let root = input.root;
    let kind = input.forge.as_ref().ok().map(|f| f.kind);
    let gitlab =
        kind == Some(ForgeKind::GitLab) || (kind.is_none() && root.join(".gitlab-ci.yml").exists());

    // Actions workflow files (path, content); empty on GitLab.
    let mut files = Vec::new();
    let local = if gitlab {
        analyse_gitlab_ci(&read(root, ".gitlab-ci.yml").unwrap_or_default())
    } else {
        for dir in WORKFLOW_DIRS {
            let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
                continue;
            };
            let mut names: Vec<_> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
                .collect();
            names.sort();
            for n in names {
                let rel = format!("{dir}/{n}");
                if let Some(content) = read(root, &rel) {
                    files.push((rel, content));
                }
            }
        }
        let self_action = read(root, "action.yml").is_some_and(|a| {
            a.lines()
                .any(|l| l.trim_start().starts_with("name:") && l.contains("Discipline"))
        });
        analyse_workflows(&files, self_action)
    };
    let mut findings = local.findings.clone();
    // Whether the push run can read a merged pull request's body (`merged-pr-body` in
    // `directives.sources`, on by default): then a squash or rebase merge is a token
    // question, not a lost review record.
    let repo_config = read(root, "discipline.toml")
        .and_then(|c| crate::config::DisciplineConfig::from_toml_str(&c).ok());
    let merged_source_on = repo_config
        .as_ref()
        .map(|cfg| cfg.directives.sources.iter().any(|s| s == "merged-pr-body"))
        // No file: the built-in default keeps the source on.
        .unwrap_or(true);

    let mut targets = vec!["discipline.toml".to_string()];
    if root.join("discipline-baseline.toml").exists() {
        targets.push("discipline-baseline.toml".to_string());
    }
    let mut wf_targets: Vec<String> = local.jobs.iter().map(|j| j.workflow.clone()).collect();
    wf_targets.dedup();
    targets.extend(wf_targets);
    let owner_paths: &[&str] = if gitlab {
        &[".gitlab/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"]
    } else {
        match kind {
            Some(ForgeKind::Gitea) | Some(ForgeKind::Forgejo) => &[
                ".gitea/CODEOWNERS",
                ".forgejo/CODEOWNERS",
                "CODEOWNERS",
                "docs/CODEOWNERS",
            ],
            _ => CODEOWNERS_PATHS,
        }
    };
    let codeowners = owner_paths
        .iter()
        .find_map(|p| read(root, p).map(|c| (p.to_string(), c)));
    findings.push(codeowners_finding(
        codeowners.as_ref().map(|(p, c)| (p.as_str(), c.as_str())),
        &targets,
    ));
    findings.push(if root.join("discipline.toml").exists() {
        Finding::new("config", Status::Pass, "discipline.toml is present")
    } else {
        Finding::new(
            "config",
            Status::Info,
            "no discipline.toml: the built-in defaults apply",
        )
        .fix("Run `discipline init` to pin the configuration in the repository.")
    });
    findings.push(security_policy_finding(
        SECURITY_POLICY_PATHS
            .iter()
            .copied()
            .find(|p| root.join(p).is_file()),
    ));

    if let Some(f) = test_report_finding(root, repo_config.as_ref()) {
        findings.push(f);
    }

    if let Some(f) = mutation_preset_finding(root, repo_config.as_ref()) {
        findings.push(f);
    }

    if let Some(f) = copilot_trust_finding(root, input.copilot_home.as_deref()) {
        findings.push(f);
    }
    findings.extend(multi_agent_findings(root));
    findings.extend(agent_sandbox_findings(root, input.home.as_deref()));

    let mut platform_name = "local".to_string();
    let mut gitea_version: Option<String> = None;
    let mut repository = None;
    let mut branch = input.branch.clone();
    if input.local_only {
        let push_jobs: Vec<&DisciplineJob> = local
            .jobs
            .iter()
            .filter(|j| j.push_branches.is_some())
            .collect();
        let on_push: Vec<String> = push_jobs
            .iter()
            .map(|j| format!("{} job `{}`", j.workflow, j.job_id))
            .collect();
        if !on_push.is_empty() {
            findings.push(if merged_source_on && push_jobs.iter().all(|j| j.reads_pull_requests) {
                pr_read_granted(&on_push.join(", "), "on push events")
            } else if merged_source_on {
                Finding::new(
                    "push-trigger",
                    Status::Info,
                    format!("{} run(s) discipline on push events; the merge method was not checked (--local-only), and `merged-pr-body` is enabled, so a merged pull request's body reaches the push run when its token can read pull requests", on_push.join(", ")),
                )
                .fix("Give the push run a token that can read pull requests (a `contents: read` token cannot on a private repository), or restrict the discipline step to pull_request.")
            } else {
                Finding::new(
                    "push-trigger",
                    Status::Warn,
                    format!("{} run(s) discipline on push events; whether the merge method drops pull-request bodies was not checked (--local-only), and `merged-pr-body` is not in `directives.sources`", on_push.join(", ")),
                )
                .fix("If squash or rebase merges are allowed, restrict the discipline step to pull_request, put `merged-pr-body` back in `directives.sources` with a token that can read pull requests, or put directives in commit messages.")
            });
        }
    }
    if !input.local_only {
        match &input.forge {
            Err(e) => {
                platform_name = "unknown".to_string();
                findings.push(
                    Finding::new("platform", Status::Unknown, format!("cannot identify the forge: {e}"))
                        .fix("Set DISCIPLINE_FORGE (github, gitlab, gitea, forgejo) and DISCIPLINE_FORGE_URL, or use --local-only."),
                );
            }
            Ok(forge) => {
                platform_name = forge.kind.label().to_string();
                if forge.kind == ForgeKind::Gitea {
                    gitea_version = input
                        .api
                        .get(forge, "version")
                        .ok()
                        .flatten()
                        .and_then(|v| v.get("version")?.as_str().map(str::to_string));
                }
                repository = Some(forge.repo.clone());
                if branch.is_none() {
                    match default_branch(input.api, forge) {
                        Ok(b) => branch = Some(b),
                        Err(e) => findings.push(
                            Finding::new(
                                "platform",
                                Status::Unknown,
                                format!("could not read the default branch of {}: {e}", forge.repo),
                            )
                            .fix(access_hint(forge.kind, &e)),
                        ),
                    }
                }
                if let Some(b) = &branch {
                    let on_push: Vec<&DisciplineJob> = local
                        .jobs
                        .iter()
                        .filter(|j| {
                            j.push_branches
                                .as_deref()
                                .is_some_and(|br| push_covers(br, b))
                        })
                        .collect();
                    if !on_push.is_empty() {
                        let jobs: Vec<String> = on_push
                            .iter()
                            .map(|j| format!("{} job `{}`", j.workflow, j.job_id))
                            .collect();
                        let jobs = jobs.join(", ");
                        let fix = "Restrict the discipline step to pull_request, or keep the `merged-pr-body` directive source with a token that can read pull requests, or put every directive in a commit message as well.";
                        // The workflow grants the token the push run needs: the review
                        // record reaches it whatever the merge method.
                        let granted =
                            merged_source_on && on_push.iter().all(|j| j.reads_pull_requests);
                        findings.push(match merge_methods(input.api, forge) {
                            Ok(m) => match m.drops_pr_body() {
                                Some(true) if granted => pr_read_granted(&jobs, &format!("on push to `{b}`")),
                                Some(true) if merged_source_on => Finding::new(
                                    "push-trigger",
                                    Status::Info,
                                    format!("{jobs} run(s) discipline on push to `{b}`; the repository allows squash or rebase merges, which drop a pull request's body from the merge commit, and `merged-pr-body` is enabled: the push run reads the merged pull request's body when its token can read pull requests"),
                                )
                                .fix("Give the push run a token that can read pull requests (a `contents: read` token cannot on a private repository), or restrict the discipline step to pull_request."),
                                Some(true) => Finding::new(
                                    "push-trigger",
                                    Status::Warn,
                                    format!("{jobs} run(s) discipline on push to `{b}`, and the repository allows squash or rebase merges: a waiver written in a pull request's body is not in the merge commit, so the push run fails on changes the pull request had passed; `merged-pr-body` is not in `directives.sources`"),
                                )
                                .fix(fix),
                                Some(false) => Finding::new(
                                    "push-trigger",
                                    Status::Pass,
                                    format!("{jobs} run(s) discipline on push to `{b}`; only merge commits are allowed, so the pull request's body reaches the push run through the merge commit message"),
                                ),
                                // GitHub shows the merge methods to push or admin tokens only.
                                // With `merged-pr-body` on, the method does not decide whether
                                // the review record reaches the push run; the token does.
                                None if granted => pr_read_granted(&jobs, &format!("on push to `{b}`")),
                                None if merged_source_on => Finding::new(
                                    "push-trigger",
                                    Status::Info,
                                    format!("{jobs} run(s) discipline on push to `{b}`; the repository does not show this token which merge methods it allows, and `merged-pr-body` is enabled: the push run reads the merged pull request's body when its token can read pull requests"),
                                )
                                .fix("Give the push run a token that can read pull requests (a `contents: read` token cannot on a private repository), or restrict the discipline step to pull_request."),
                                None => Finding::new(
                                    "push-trigger",
                                    Status::Unknown,
                                    format!("{jobs} run(s) discipline on push to `{b}`; the repository does not show this token which merge methods it allows (GitHub shows them to push or admin tokens), and `merged-pr-body` is not in `directives.sources`"),
                                )
                                .fix(fix),
                            },
                            Err(e) => Finding::new(
                                "push-trigger",
                                Status::Unknown,
                                format!("{jobs} run(s) discipline on push to `{b}`; could not read the repository's merge methods: {e}"),
                            )
                            .fix(access_hint(forge.kind, &e)),
                        });
                    }
                    let protection = match forge.kind {
                        ForgeKind::GitHub => github_protection(input.api, forge, b),
                        ForgeKind::Gitea | ForgeKind::Forgejo => {
                            gitea_protection(input.api, forge, b)
                        }
                        ForgeKind::GitLab => gitlab_protection(input.api, forge, b),
                    };
                    let reviews = protection.as_ref().ok().and_then(|p| p.required_approvals);
                    match protection {
                        Ok(p) => findings.extend(protection_findings(forge.kind, &p, &local.jobs)),
                        Err(e) => findings.push(
                            Finding::new(
                                "platform",
                                Status::Unknown,
                                format!("could not read the protection of `{b}`: {e}"),
                            )
                            .fix(access_hint(forge.kind, &e)),
                        ),
                    }
                    // Repository settings that decide whether mutable action refs,
                    // moved release tags or replaced release assets can run.
                    findings.extend(crate::doctor_settings::findings(
                        input.api, forge, &files, reviews,
                    ));
                    // What a SHA pin does not prove: that the commit belongs to the pinned
                    // repository, and that the action pins what it runs in turn.
                    findings.extend(crate::doctor_pins::findings(
                        input.api,
                        forge,
                        &pin_sources(root, &files),
                    ));
                    // `ratified-paths` trusts comment authorship: an agent login with
                    // administrator rights could act as, or edit the comments of, the owner.
                    if let Some(rp) = repo_config
                        .as_ref()
                        .map(|c| &c.gates.ratified_paths)
                        .filter(|rp| rp.enabled && !rp.agent_logins.is_empty())
                    {
                        findings.extend(agent_permission_findings(
                            input.api,
                            forge,
                            &rp.agent_logins,
                        ));
                    }
                    let visibility = match forge.kind {
                        ForgeKind::GitHub | ForgeKind::Gitea | ForgeKind::Forgejo => input
                            .api
                            .fetch(forge, &format!("repos/{}", forge.repo))
                            .ok()
                            .and_then(|v| {
                                v.get("visibility")
                                    .and_then(|s| s.as_str())
                                    .map(str::to_string)
                                    .or_else(|| {
                                        v.get("private").and_then(|p| p.as_bool()).map(|p| {
                                            if p {
                                                "private".to_string()
                                            } else {
                                                "public".to_string()
                                            }
                                        })
                                    })
                            }),
                        ForgeKind::GitLab => {
                            let id = crate::forge::gitlab_project_id(&forge.repo);
                            input
                                .api
                                .fetch(forge, &format!("projects/{id}"))
                                .ok()
                                .and_then(|v| {
                                    v.get("visibility")
                                        .and_then(|s| s.as_str())
                                        .map(str::to_string)
                                })
                        }
                    };

                    let has_terms = repo_config
                        .as_ref()
                        .map(|c| !c.gates.pii.term_denylist.is_empty())
                        .unwrap_or(false);

                    findings.push(match (visibility.as_deref(), has_terms) {
                        (Some("public"), true) => Finding::new(
                            "term-denylist",
                            Status::Warn,
                            "public repository with committed `term_denylist` in discipline.toml: the list of private terms is exposed to anyone who can view the repository",
                        )
                        .fix("Move private terms to DISCIPLINE_TERM_DENYLIST in CI secrets or action inputs so the denylist itself is not public."),
                        (Some(vis), true) => Finding::new(
                            "term-denylist",
                            Status::Pass,
                            format!("{vis} repository: committed `term_denylist` is not exposed to the public"),
                        ),
                        (None, true) => Finding::new(
                            "term-denylist",
                            Status::Info,
                            "`term_denylist` is configured in discipline.toml; repository visibility could not be verified",
                        )
                        .fix("Ensure the repository is private, or pass terms via DISCIPLINE_TERM_DENYLIST in CI secrets."),
                        _ => Finding::new(
                            "term-denylist",
                            Status::Pass,
                            "no `term_denylist` committed in discipline.toml (pass private terms through DISCIPLINE_TERM_DENYLIST in CI secrets)",
                        ),
                    });
                }
            }
        }
    }

    let gitea_workflows = input
        .forge
        .as_ref()
        .is_ok_and(|f| f.kind == ForgeKind::Gitea);
    qualify_token_findings(&mut findings, gitea_workflows, gitea_version.as_deref());

    Report {
        platform: platform_name,
        forge_url: input.forge.as_ref().ok().map(|f| f.url.clone()),
        repository,
        branch,
        findings,
    }
}

/// First Gitea release whose Actions token honours a workflow's `permissions:`
/// (go-gitea/gitea#36173). Earlier versions ignore the key.
pub const GITEA_PERMISSIONS_SINCE: (u64, u64, u64) = (1, 26, 0);

/// `major.minor.patch` of a version string such as `1.24.6` or `1.26.0+dev-12-gabc`.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v
        .trim()
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

/// A `permissions:` fix only helps where the runner honours it. On Gitea below 1.26 the
/// key is ignored, so the token warning becomes information; when the version is
/// unknown (or the workflow is a Gitea one), the remediation says which version it needs.
fn qualify_token_findings(findings: &mut [Finding], gitea: bool, version: Option<&str>) {
    let (major, minor, patch) = GITEA_PERMISSIONS_SINCE;
    let since = format!("{major}.{minor}.{patch}");
    for f in findings
        .iter_mut()
        .filter(|f| f.id == "token" && f.status == Status::Warn)
    {
        let gitea_file = f.summary.starts_with(".gitea/");
        if !(gitea || gitea_file) {
            continue;
        }
        match version.and_then(parse_version) {
            Some(v) if v < GITEA_PERMISSIONS_SINCE => {
                let shown = version.unwrap_or_default();
                f.status = Status::Info;
                f.summary = format!(
                    "{} (Gitea {shown} ignores `permissions:`; the instance scopes the token)",
                    f.summary
                );
                f.remediation = Some(format!(
                    "Upgrade to Gitea {since} or later, then set `permissions: contents: read`."
                ));
            }
            Some(_) => {}
            None => {
                f.remediation = Some(format!(
                    "Set `permissions: contents: read` on the workflow or job (honoured by Gitea {since} and later; older versions ignore it)."
                ));
            }
        }
    }
}

pub(crate) fn access_hint(kind: ForgeKind, error: &str) -> &'static str {
    // A transport failure is about the address, not the credentials.
    if error.contains("request to ") && error.contains(" failed") {
        return "Check the forge's web address: set DISCIPLINE_FORGE_URL (for example https://git.example.com) when the remote's host is not where the API is served, or use --local-only.";
    }
    match kind {
        ForgeKind::GitHub => "Set GH_TOKEN or GITHUB_TOKEN (or DISCIPLINE_FORGE_TOKEN) with read access, or use --local-only.",
        ForgeKind::GitLab => "Set GITLAB_TOKEN (or DISCIPLINE_FORGE_TOKEN), or use --local-only.",
        ForgeKind::Gitea => "Set GITEA_TOKEN (or DISCIPLINE_FORGE_TOKEN), or use --local-only.",
        ForgeKind::Forgejo => "Set FORGEJO_TOKEN (or DISCIPLINE_FORGE_TOKEN), or use --local-only.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::test_support::*;
    use crate::forge::{CannedApi, NoApi};

    #[test]
    fn an_unreachable_host_points_at_the_address_not_the_token() {
        let dns = "request to gitea failed: io: failed to lookup address information";
        assert!(access_hint(ForgeKind::Gitea, dns).contains("DISCIPLINE_FORGE_URL"));
        let auth =
            "gitea.example.com answered HTTP 403 Only signed in user is allowed to call APIs.";
        assert!(access_hint(ForgeKind::Gitea, auth).contains("GITEA_TOKEN"));
    }

    #[test]
    fn gitea_token_findings_follow_the_instance_version() {
        let warn = || {
            vec![Finding::new(
                "token",
                Status::Warn,
                ".gitea/workflows/ci.yml: discipline jobs inherit the repository's default token permissions",
            )
            .fix("Set `permissions: contents: read` on the workflow or job.")]
        };
        let mut old = warn();
        qualify_token_findings(&mut old, true, Some("1.24.6"));
        assert_eq!(old[0].status, Status::Info);
        assert!(
            old[0].summary.contains("Gitea 1.24.6 ignores"),
            "{}",
            old[0].summary
        );
        assert!(old[0].remediation.as_deref().unwrap().contains("1.26.0"));

        let mut new = warn();
        qualify_token_findings(&mut new, true, Some("1.26.1+dev-3-gabc"));
        assert_eq!(new[0].status, Status::Warn);
        assert_eq!(
            new[0].remediation.as_deref(),
            Some("Set `permissions: contents: read` on the workflow or job.")
        );

        let mut unknown = warn();
        qualify_token_findings(&mut unknown, false, None);
        assert_eq!(unknown[0].status, Status::Warn);
        assert!(unknown[0]
            .remediation
            .as_deref()
            .unwrap()
            .contains("Gitea 1.26.0 and later"));

        let mut github =
            vec![Finding::new("token", Status::Warn, ".github/workflows/ci.yml: x").fix("f")];
        qualify_token_findings(&mut github, false, None);
        assert_eq!(
            github[0].remediation.as_deref(),
            Some("f"),
            "GitHub workflows are untouched"
        );

        assert_eq!(parse_version("1.24.6"), Some((1, 24, 6)));
        assert_eq!(parse_version("v1.26"), Some((1, 26, 0)));
        assert_eq!(parse_version("12.0.4+gitea-1.22.0"), Some((12, 0, 4)));
        assert_eq!(parse_version("dev"), None);
    }

    #[test]
    fn unreachable_platform_is_unknown_not_healthy() {
        let dir = std::env::temp_dir().join(format!("discipline-doctor-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".github/workflows")).unwrap();
        std::fs::write(dir.join(".github/workflows/ci.yml"), WF).unwrap();
        let input = DoctorInput {
            copilot_home: None,
            home: None,
            root: &dir,
            forge: Ok(forge(ForgeKind::GitHub)),
            branch: None,
            local_only: false,
            api: &NoApi,
        };
        let r = run(&input);
        assert_eq!(r.exit_code(false), 2, "{r:?}");
        let local = run(&DoctorInput {
            copilot_home: None,
            home: None,
            local_only: true,
            ..input
        });
        assert!(local.findings.iter().all(|f| f.id != "platform"));
        let other = run(&DoctorInput {
            copilot_home: None,
            home: None,
            root: &dir,
            forge: Err("set DISCIPLINE_FORGE".into()),
            branch: None,
            local_only: false,
            api: &NoApi,
        });
        assert_eq!(other.exit_code(false), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_granted_pull_requests_permission_satisfies_the_push_trigger_check() {
        let dir =
            std::env::temp_dir().join(format!("discipline-doctor-prread-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".github/workflows")).unwrap();
        let mut squash = CannedApi::default();
        squash.responses.insert(
            "github:repos/o/r".into(),
            serde_json::json!({"allow_squash_merge": true, "allow_rebase_merge": false}),
        );
        let mut hidden = CannedApi::default();
        hidden.responses.insert(
            "github:repos/o/r".into(),
            serde_json::json!({"default_branch": "main"}),
        );
        // The push-trigger status locally, on a squash-merge forge, and on a forge that
        // hides its merge methods from the token.
        let statuses = |wf_perm: &str, job_perm: &str| -> Vec<Status> {
            std::fs::write(
                dir.join(".github/workflows/ci.yml"),
                format!(
                    "on:\n  push:\n    branches: [main]\n  pull_request:\n    types: [opened, synchronize, reopened, edited]\n{wf_perm}jobs:\n  gate:\n{job_perm}    runs-on: ubuntu-latest\n    steps:\n      - uses: orieg/discipline@v0\n"
                ),
            )
            .unwrap();
            let status = |local_only: bool, api: &dyn ForgeApi| {
                run(&DoctorInput {
                    copilot_home: None,
                    home: None,
                    root: &dir,
                    forge: Ok(forge(ForgeKind::GitHub)),
                    branch: Some("main".into()),
                    local_only,
                    api,
                })
                .findings
                .iter()
                .find(|f| f.id == "push-trigger")
                .map(|f| f.status)
                .unwrap()
            };
            vec![
                status(true, &NoApi),
                status(false, &squash),
                status(false, &hidden),
            ]
        };
        let info = vec![Status::Info; 3];
        let pass = vec![Status::Pass; 3];
        // Without the permission the push run's token may not read pull requests.
        assert_eq!(statuses("permissions:\n  contents: read\n", ""), info);
        assert_eq!(statuses("", ""), info);
        // Granted at workflow level, at job level, or by `read-all`.
        assert_eq!(
            statuses(
                "permissions:\n  contents: read\n  pull-requests: read\n",
                ""
            ),
            pass
        );
        assert_eq!(
            statuses("", "    permissions:\n      pull-requests: write\n"),
            pass
        );
        assert_eq!(statuses("permissions: read-all\n", ""), pass);
        // A job's `permissions:` replaces the workflow's: the grant no longer applies.
        assert_eq!(
            statuses(
                "permissions:\n  pull-requests: read\n",
                "    permissions:\n      contents: read\n"
            ),
            info
        );
        // `pull-requests: none` grants nothing.
        assert_eq!(statuses("permissions:\n  pull-requests: none\n", ""), info);
        std::fs::remove_dir_all(&dir).ok();
    }
}
