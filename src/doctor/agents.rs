//! Agent checks: the repository permission of an agent login, the hook files of each
//! agent tool, and the `agent-sandbox` settings.

use super::{access_hint, get, read, Finding, Status};
use crate::forge::{gitlab_project_id, Forge, ForgeApi, ForgeKind};
use std::path::Path;

/// The repository role of `login` and whether it is a site administrator, as far as the
/// token can see. GitHub, Gitea and Forgejo: `collaborators/{login}/permission`; GitLab:
/// the user's membership, inherited ones included.
pub fn agent_permission(
    api: &dyn ForgeApi,
    forge: &Forge,
    login: &str,
) -> Result<(String, bool), String> {
    match forge.kind {
        ForgeKind::GitLab => {
            let users = get(
                api,
                forge,
                &format!("users?username={}", crate::forge::encode_segment(login)),
            )?;
            let Some(user) = users.as_array().and_then(|u| u.first()) else {
                return Ok(("none".into(), false));
            };
            let admin = user.get("is_admin").and_then(|v| v.as_bool()) == Some(true);
            let uid = user.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let role = match api.get(
                forge,
                &format!(
                    "projects/{}/members/all/{uid}",
                    gitlab_project_id(&forge.repo)
                ),
            )? {
                None => "none".to_string(),
                Some(m) => match m.get("access_level").and_then(|v| v.as_u64()) {
                    Some(50..) => "owner",
                    Some(40..=49) => "maintainer",
                    Some(30..=39) => "developer",
                    Some(_) => "reporter",
                    None => "unknown",
                }
                .to_string(),
            };
            Ok((role, admin))
        }
        _ => match api.get(
            forge,
            &format!(
                "repos/{}/collaborators/{}/permission",
                forge.repo,
                crate::forge::encode_segment(login)
            ),
        )? {
            None => Ok(("none".into(), false)),
            Some(v) => {
                let role = v
                    .get("role_name")
                    .and_then(|r| r.as_str())
                    .filter(|r| !r.is_empty())
                    .or_else(|| v.get("permission").and_then(|r| r.as_str()))
                    .unwrap_or("unknown")
                    .to_ascii_lowercase();
                let admin = ["site_admin", "is_admin"].iter().any(|k| {
                    v.pointer(&format!("/user/{k}")).and_then(|b| b.as_bool()) == Some(true)
                });
                Ok((role, admin))
            }
        },
    }
}

/// `agent-permission`: an agent login that is a site administrator or holds admin, owner,
/// maintain or maintainer rights can act as, or rewrite the comments of, the owner whose
/// ratification `ratified-paths` reads.
pub fn agent_permission_findings(
    api: &dyn ForgeApi,
    forge: &Forge,
    agent_logins: &[String],
) -> Vec<Finding> {
    agent_logins
        .iter()
        .map(|login| match agent_permission(api, forge, login) {
            Err(e) => Finding::new(
                "agent-permission",
                Status::Unknown,
                format!("could not read the permissions of agent login `{login}`: {e}"),
            )
            .fix(access_hint(forge.kind, &e)),
            Ok((role, admin)) => {
                let privileged = matches!(role.as_str(), "admin" | "owner" | "maintain" | "maintainer");
                if admin || privileged {
                    Finding::new(
                        "agent-permission",
                        Status::Fail,
                        format!(
                            "agent login `{login}` is {}; it could act as, or edit the comments of, the owner whose ratification `ratified-paths` reads",
                            if admin { "a site administrator".to_string() } else { format!("a repository {role}") }
                        ),
                    )
                    .fix("Give agent accounts write access at most, and never site-administrator rights.")
                } else {
                    Finding::new(
                        "agent-permission",
                        Status::Pass,
                        format!("agent login `{login}` has `{role}` access and is not an administrator"),
                    )
                }
            }
        })
        .collect()
}

/// `copilot-trust`: whether Copilot CLI on this machine runs the repository's Copilot
/// discipline hook, which it skips without a message in a folder it does not trust. `None`
/// without a repository hook or without Copilot CLI configuration (a CI runner, or a
/// machine where Copilot CLI has not run).
/// A generated agent hook file: agent id, path, and whether its text has the pre-tool entry.
type HookFile = (&'static str, &'static str, fn(&str) -> bool);

/// Several agents in one repository (docs/ROADMAP.md, Phase 13 Step 4):
///
/// - `ref-guard`: with more than one worktree, the lease guard (`discipline lease
///   install-guard`) is what stops one session moving another's branch. A warning when
///   it is missing; information with a single worktree.
/// - `leases`: a lease file that does not parse makes the guard refuse every branch
///   update in every worktree (a failure); leases whose holder stopped refreshing them
///   are listed (information).
/// - `pretool-hook`: a generated agent hook file without its pre-tool entry, which a file
///   written before that entry existed lacks (information: the entry needs a discipline
///   release that has `hook run --event pre-tool`).
pub fn multi_agent_findings(root: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    let Ok(repo) = crate::gitctx::discover_repository(root) else {
        return out;
    };
    let linked = git2::Repository::open(repo.commondir())
        .ok()
        .and_then(|m| m.worktrees().ok().map(|w| w.len()))
        .unwrap_or(0);
    let hooks = match repo
        .config()
        .ok()
        .and_then(|c| c.get_path("core.hooksPath").ok())
    {
        Some(p) if p.is_absolute() => p,
        Some(p) => root.join(p),
        None => repo.commondir().join("hooks"),
    };
    let guard = std::fs::read_to_string(hooks.join("reference-transaction"))
        .is_ok_and(|t| t.contains(crate::lease::GUARD_MARKER));
    out.push(match (guard, linked) {
        (true, _) => Finding::new(
            "ref-guard",
            Status::Pass,
            "the lease guard refuses a branch update another worktree's session has leased",
        ),
        (false, 0) => Finding::new(
            "ref-guard",
            Status::Info,
            "one worktree: the lease guard matters once several agents share the repository (optional)",
        )
        .fix("Run `discipline lease install-guard` when agents work in several worktrees."),
        (false, n) => Finding::new(
            "ref-guard",
            Status::Warn,
            format!(
                "{} worktrees and no lease guard: one session can move a branch another is working on (a rebase with --update-refs, branch -f, reset)",
                n + 1
            ),
        )
        .fix("Run `discipline lease install-guard`, and have each session take its branches with `discipline lease take`."),
    });
    if let Ok((store, _)) = crate::lease::open(root) {
        match store.list() {
            Err(e) => out.push(
                Finding::new(
                    "leases",
                    Status::Fail,
                    format!("a lease cannot be read ({e:#}); the lease guard refuses every branch update until it can"),
                )
                .fix(format!("Remove or repair the file under {}.", store.dir.display())),
            ),
            Ok(all) if !all.is_empty() => {
                let now = crate::lease::now();
                let stale: Vec<String> = all
                    .iter()
                    .filter(|(_, l)| !l.is_live(now))
                    .map(|(k, l)| format!("`{k}` ({})", l.agent))
                    .collect();
                out.push(if stale.is_empty() {
                    Finding::new("leases", Status::Pass, format!("{} live lease(s)", all.len()))
                } else {
                    Finding::new(
                        "leases",
                        Status::Info,
                        format!("stale lease(s), holder no longer refreshing: {}", stale.join(", ")),
                    )
                    .fix("Run `discipline lease release` in that worktree, or remove the worktree; a stale lease claims nothing.")
                });
            }
            Ok(_) => {}
        }
    }
    let files: [HookFile; 6] = [
        ("claude-code", ".claude/settings.json", |t| {
            t.contains("\"PreToolUse\"") && t.contains("--event pre-tool")
        }),
        ("copilot", ".github/hooks/discipline.json", |t| {
            t.contains("\"preToolUse\"") && t.contains("--event pre-tool")
        }),
        ("agy", ".agents/hooks.json", |t| {
            t.contains("\"PreToolUse\"") && t.contains("--event pre-tool")
        }),
        ("opencode", ".opencode/plugins/discipline.js", |t| {
            t.contains("tool.execute.before") && t.contains("--event pre-tool")
        }),
        ("qwen", ".qwen/settings.json", |t| {
            t.contains("\"PreToolUse\"") && t.contains("--event pre-tool")
        }),
        ("codex", ".codex/hooks.json", |t| {
            t.contains("\"PreToolUse\"") && t.contains("--event pre-tool")
        }),
    ];
    for (agent, rel, has) in files {
        let Some(text) = read(root, rel) else {
            continue;
        };
        if !text.contains("discipline hook run") {
            continue;
        }
        out.push(if has(&text) {
            Finding::new(
                "pretool-hook",
                Status::Pass,
                format!("`{rel}` refuses an edit outside the session's worktree before it runs"),
            )
        } else {
            Finding::new(
                "pretool-hook",
                Status::Info,
                format!("`{rel}` has no pre-tool entry: {agent} can edit another worktree before any check runs"),
            )
            .fix(format!(
                "Once the installed discipline has `hook run --event pre-tool`, regenerate it: `discipline hook install --agent {agent} --upgrade` (a file with hooks or settings of its own is not rewritten: merge the entry it prints)."
            ))
        });
    }
    out
}

/// `agent-sandbox`: the modes and switches in each agent's settings that run it with less
/// containment than its defaults (`sandbox_config::posture`). The repository's own files
/// warn, since every session in the repository inherits them; the user's files are
/// information, since they are that person's choice. Lists (allowed commands, MCP servers)
/// are not posture and are left to the `sandbox-config` gate, which judges their growth.
pub fn agent_sandbox_findings(root: &Path, home: Option<&Path>) -> Vec<Finding> {
    use crate::guards::sandbox_config::{posture, PROJECT_SETTINGS, USER_SETTINGS};
    let mut out = Vec::new();
    let mut read_any = false;
    let mut scan = |base: &Path,
                    files: &[(&str, &str)],
                    shown: &str,
                    status: Status,
                    out: &mut Vec<Finding>| {
        for (agent, rel) in files {
            let Ok(text) = std::fs::read_to_string(base.join(rel)) else {
                continue;
            };
            read_any = true;
            let path = format!("{shown}{rel}");
            match posture(rel, &text) {
                None => out.push(Finding::new(
                    "agent-sandbox",
                    status,
                    format!("`{path}` ({agent}) does not parse, so its sandbox settings could not be read"),
                ).fix("Fix the file so it parses.")),
                Some(loose) if !loose.is_empty() => out.push(
                    Finding::new(
                        "agent-sandbox",
                        status,
                        format!(
                            "`{path}` runs {agent} with less containment than its defaults: {}",
                            loose.join("; ")
                        ),
                    )
                    .fix(if status == Status::Warn {
                        "Every session in this repository inherits these settings: remove them, or keep them in a user-level file for the person who wants them."
                    } else {
                        "Your own choice; run the agent inside a container or VM with no credentials and a network allow-list when these are on."
                    }),
                ),
                Some(_) => {}
            }
        }
    };
    scan(root, PROJECT_SETTINGS, "", Status::Warn, &mut out);
    if let Some(home) = home {
        scan(home, USER_SETTINGS, "~/", Status::Info, &mut out);
    }
    if out.is_empty() && read_any {
        out.push(Finding::new(
            "agent-sandbox",
            Status::Pass,
            "no agent settings file runs an agent with less containment than its defaults",
        ));
    }
    out
}

pub fn copilot_trust_finding(root: &Path, home: Option<&Path>) -> Option<Finding> {
    let hook = crate::hook::copilot_repo_hook(root)?;
    let hook = hook
        .strip_prefix(root)
        .unwrap_or(&hook)
        .display()
        .to_string();
    let home = home?;
    if crate::hook::copilot_trusts(home, root)? {
        return Some(Finding::new(
            "copilot-trust",
            Status::Pass,
            format!("Copilot CLI trusts this folder, so it runs {hook}"),
        ));
    }
    if let Some(user) = crate::hook::copilot_user_hook(home) {
        return Some(Finding::new(
            "copilot-trust",
            Status::Info,
            format!(
                "Copilot CLI does not trust this folder, so it skips {hook}; the user-level hook {} checks it instead",
                user.display()
            ),
        ));
    }
    let note = crate::hook::copilot_untrusted_note(home, root)?;
    Some(
        Finding::new(
            "copilot-trust",
            Status::Warn,
            format!("Copilot CLI does not trust this folder, so {hook} never runs"),
        )
        .fix(note),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::test_support::*;
    use crate::forge::CannedApi;

    #[test]
    fn agent_sandbox_warns_on_the_repository_and_informs_on_the_user() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let write = |base: &Path, rel: &str, text: &str| {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        // No settings file anywhere: nothing to say.
        assert!(agent_sandbox_findings(root.path(), Some(home.path())).is_empty());
        // Lists only (an allowed command, an MCP server): not posture.
        write(
            root.path(),
            ".claude/settings.json",
            r#"{"permissions": {"allow": ["Bash(cargo test:*)"]}, "hooks": {}}"#,
        );
        let f = agent_sandbox_findings(root.path(), Some(home.path()));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].id, f[0].status), ("agent-sandbox", Status::Pass));
        // The repository runs Qwen Code in yolo; the user runs Codex without a sandbox.
        write(
            root.path(),
            ".qwen/settings.json",
            r#"{"tools": {"approvalMode": "yolo"}}"#,
        );
        write(
            home.path(),
            ".codex/config.toml",
            "sandbox_mode = \"danger-full-access\"\n",
        );
        let f = agent_sandbox_findings(root.path(), Some(home.path()));
        let got: Vec<(Status, bool)> = f
            .iter()
            .map(|x| (x.status, x.summary.contains("tools.approvalMode")))
            .collect();
        assert_eq!(got, [(Status::Warn, true), (Status::Info, false)], "{f:?}");
        assert!(
            f[1].summary
                .starts_with("`~/.codex/config.toml` runs Codex"),
            "{f:?}"
        );
        // A user's file that does not parse is information, never exit 2.
        write(home.path(), ".claude/settings.json", "{");
        let f = agent_sandbox_findings(root.path(), Some(home.path()));
        assert!(f
            .iter()
            .any(|x| x.status == Status::Info && x.summary.contains("does not parse")));
        assert!(f.iter().all(|x| x.status != Status::Unknown));
    }

    #[test]
    fn an_agent_login_with_administrator_rights_fails() {
        let perm = |kind: ForgeKind, body: serde_json::Value| {
            let mut api = CannedApi::default();
            api.responses.insert(
                format!("{}:repos/o/r/collaborators/agent/permission", kind.label()),
                body,
            );
            agent_permission_findings(&api, &forge(kind), &["agent".into()])[0].status
        };
        use serde_json::json;
        assert_eq!(
            perm(
                ForgeKind::GitHub,
                json!({"permission": "admin", "role_name": "admin"})
            ),
            Status::Fail
        );
        assert_eq!(
            perm(
                ForgeKind::GitHub,
                json!({"permission": "write", "role_name": "maintain"})
            ),
            Status::Fail
        );
        assert_eq!(
            perm(
                ForgeKind::GitHub,
                json!({"permission": "write", "role_name": "write"})
            ),
            Status::Pass
        );
        assert_eq!(
            perm(
                ForgeKind::Gitea,
                json!({"permission": "write", "role_name": "write", "user": {"is_admin": true}})
            ),
            Status::Fail,
            "a site administrator can act as any user"
        );
        assert_eq!(
            perm(ForgeKind::Forgejo, serde_json::Value::Null),
            Status::Pass
        );
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/collaborators/agent/permission".into(),
            json!({"__error": "HTTP 403"}),
        );
        assert_eq!(
            agent_permission_findings(&api, &forge(ForgeKind::GitHub), &["agent".into()])[0].status,
            Status::Unknown
        );
        // GitLab: membership access level 40 is a maintainer.
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitlab:users?username=agent".into(),
            json!([{"id": 9, "is_admin": false}]),
        );
        api.responses.insert(
            "gitlab:projects/o%2Fr/members/all/9".into(),
            json!({"access_level": 40}),
        );
        assert_eq!(
            agent_permission_findings(&api, &forge(ForgeKind::GitLab), &["agent".into()])[0].status,
            Status::Fail
        );
    }
}
