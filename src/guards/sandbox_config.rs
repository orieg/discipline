//! `sandbox-config`: a change cannot quietly widen what a coding agent may do, or the
//! isolation of the container it runs in.
//!
//! The same engine as `toolchain-config` (`config_delta::diff_trees`) over another rule
//! table: the project-level settings of the coding agents (permission lists, the default
//! approval or sandbox mode, sandbox network allow-lists, MCP server lists, hooks) and the
//! container definitions an agent is commonly run in (Dev Containers, Docker Compose). A file
//! that appears is compared with an empty file, and one that disappears likewise: an agent
//! with no settings file runs on its defaults, so a new file that grants `bypassPermissions`
//! widens as much as an edit that does. Runtime containment itself (system calls, outbound
//! traffic, credential reads) is the sandbox's job, not this gate's (`docs/ARCHITECTURE.md`
//! §1.3).

mod rules;

pub use rules::{EXECUTABLE, RULES};

use super::config_delta::{
    self, classify_with, diff_trees, load, load_or_empty, AbsentSide, Classified, Judge, ReadOrder,
    Rule, TreeGate, Weakening,
};
use super::{Context, GateOutcome};
use crate::tokens;
use anyhow::Result;
use serde_json::Value;

/// Agent settings files `doctor` reads for `agent-sandbox`, relative to the repository.
pub const PROJECT_SETTINGS: &[(&str, &str)] = &[
    ("Claude Code", ".claude/settings.json"),
    ("Claude Code", ".claude/settings.local.json"),
    ("Codex", ".codex/config.toml"),
    ("Gemini CLI", ".gemini/settings.json"),
    ("Qwen Code", ".qwen/settings.json"),
    ("OpenCode", "opencode.json"),
    ("OpenCode", "opencode.jsonc"),
    ("Cursor", ".cursor/cli.json"),
    ("Cursor", ".cursor/sandbox.json"),
    ("Copilot CLI", ".github/copilot/settings.json"),
    ("Copilot CLI", ".github/copilot/settings.local.json"),
];

/// The same, relative to the user's home directory.
pub const USER_SETTINGS: &[(&str, &str)] = &[
    ("Claude Code", ".claude/settings.json"),
    ("Codex", ".codex/config.toml"),
    ("Gemini CLI", ".gemini/settings.json"),
    ("Qwen Code", ".qwen/settings.json"),
    ("OpenCode", ".config/opencode/opencode.json"),
    ("Cursor", ".cursor/cli-config.json"),
    ("Cursor", ".cursor/sandbox.json"),
    ("Copilot CLI", ".copilot/settings.json"),
    ("agy", ".gemini/antigravity-cli/settings.json"),
];

/// The modes and switches in one settings file that give the agent less containment than
/// an absent file would, as `key what` lines; `None` when the file does not parse. Lists are
/// left out: a permission list is how a team grants what it needs, and the gate judges its
/// growth.
pub fn posture(rel: &str, text: &str) -> Option<Vec<String>> {
    let Some(Classified::Data { name, rules }) = classify(rel) else {
        return Some(Vec::new());
    };
    let rules: Vec<&Rule> = rules
        .into_iter()
        .filter(|r| {
            matches!(
                r.judge,
                Judge::Ranked(..) | Judge::LooserWhenTrue | Judge::LooserWhenFalse
            )
        })
        .collect();
    let tree = load(&name, text)?;
    Some(
        diff_trees(&Value::Object(Default::default()), &tree, &rules)
            .into_iter()
            .map(|w| format!("`{}` {}", w.key, w.what))
            .collect(),
    )
}

/// Whether `path` is a file this gate reads.
pub fn classify(path: &str) -> Option<Classified> {
    classify_with(path, RULES, EXECUTABLE)
}

/// The widenings between two versions of one file; `None` on a side is the file absent.
pub fn widenings(
    name: &str,
    rules: &[&Rule],
    base: Option<&str>,
    head: Option<&str>,
) -> Result<Vec<Weakening>, &'static str> {
    let tree = |src: Option<&str>, side: &'static str| load_or_empty(name, src).ok_or(side);
    Ok(diff_trees(
        &tree(base, "base")?,
        &tree(head, "head")?,
        rules,
    ))
}

fn executable_message(path: &str) -> String {
    format!(
        "`{path}` sets a container's outbound rules in code; whether the change widens them cannot be read from a diff."
    )
}

fn unreadable_message(path: &str, side: &str) -> String {
    format!(
        "`{path}` could not be parsed on the {side} side, so whether it widens the sandbox could not be checked."
    )
}

/// What this gate hands the shared loop ([`config_delta::run`]).
const TREE_GATE: TreeGate = TreeGate {
    id: "sandbox-config",
    directive: tokens::ALLOW_SANDBOX_WIDENING,
    classify,
    changed: &crate::findings::SANDBOX_CONFIG_WIDENED,
    not_analysed: &crate::findings::SANDBOX_CHANGE_NOT_ANALYSED,
    unreadable: &crate::findings::SANDBOX_CONFIG_UNREADABLE,
    executable_message,
    unreadable_message,
    // An added file has no base side and a deleted one no head side: both are compared
    // as an absent file.
    absent_side: AbsentSide::Empty,
    read_order: ReadOrder::BaseThenHead,
    inherited: None,
    other_file: None,
    nothing_examined: "no agent settings or container definitions modified in this diff",
};

pub fn sandbox_config(ctx: &Context) -> Result<GateOutcome> {
    config_delta::run(ctx, &ctx.config.gates.sandbox_config, &TREE_GATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(key, what)` for every widening between `base` and `head` of the file at `path`.
    fn widened(path: &str, base: Option<&str>, head: Option<&str>) -> Vec<(String, String)> {
        let Some(Classified::Data { name, rules }) = classify(path) else {
            panic!("{path} is not a data file of this gate");
        };
        widenings(&name, &rules, base, head)
            .expect("parses")
            .into_iter()
            .map(|w| (w.key, w.what))
            .collect()
    }

    fn keys(v: &[(String, String)]) -> Vec<&str> {
        v.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn claude_code_permissions_modes_sandbox_and_hooks() {
        let base = r#"{
            "permissions": {"allow": ["Bash(cargo test:*)"], "deny": ["Read(./.env)"], "defaultMode": "default"},
            "sandbox": {"enabled": true, "network": {"allowedDomains": ["crates.io"]}},
            "hooks": {"PreToolUse": [], "Stop": []}
        }"#;
        let head = r#"{
            // comments are allowed
            "permissions": {"allow": ["Bash(cargo test:*)", "Bash(curl:*)"], "deny": [], "defaultMode": "bypassPermissions"},
            "sandbox": {"enabled": false, "network": {"allowedDomains": ["crates.io", "*"]}},
            "enableAllProjectMcpServers": true,
            "hooks": {"Stop": []},
        }"#;
        let w = widened(".claude/settings.json", Some(base), Some(head));
        assert_eq!(
            keys(&w),
            [
                "permissions.allow",
                "permissions.deny",
                "permissions.defaultMode",
                "sandbox.enabled",
                "sandbox.network.allowedDomains",
                "enableAllProjectMcpServers",
                "hooks",
            ],
            "{w:?}"
        );
        assert!(w[2].1.contains("`default` to `bypassPermissions`"), "{w:?}");
        // The reverse direction tightens everything: nothing to report.
        assert!(widened(".claude/settings.json", Some(head), Some(base)).is_empty());
        // `plan` is stricter than the default mode; `acceptEdits` is looser.
        let mode = |m: &str| format!(r#"{{"permissions": {{"defaultMode": "{m}"}}}}"#);
        assert!(widened(".claude/settings.json", None, Some(&mode("plan"))).is_empty());
        assert_eq!(
            keys(&widened(
                ".claude/settings.json",
                None,
                Some(&mode("acceptEdits"))
            )),
            ["permissions.defaultMode"]
        );
    }

    #[test]
    fn an_added_or_deleted_file_is_compared_with_an_absent_one() {
        let loose = r#"{"permissions": {"defaultMode": "bypassPermissions"}}"#;
        assert_eq!(
            keys(&widened(".claude/settings.json", None, Some(loose))),
            ["permissions.defaultMode"]
        );
        let strict = r#"{"permissions": {"deny": ["Bash(rm:*)"]}, "sandbox": {"enabled": true}}"#;
        assert_eq!(
            keys(&widened(".claude/settings.json", Some(strict), None)),
            ["permissions.deny", "sandbox.enabled"]
        );
        // A file with nothing loose appears: nothing to report.
        assert!(widened(".claude/settings.json", None, Some(strict)).is_empty());
    }

    #[test]
    fn mcp_server_lists_grow() {
        let w = widened(
            ".mcp.json",
            Some(r#"{"mcpServers": {"docs": {}}}"#),
            Some(r#"{"mcpServers": {"docs": {}, "shell": {"command": "sh"}}}"#),
        );
        assert_eq!(keys(&w), ["mcpServers"]);
        assert!(w[0].1.contains("`shell`"), "{w:?}");
        assert_eq!(
            keys(&widened(
                ".vscode/mcp.json",
                None,
                Some(r#"{"servers": {"x": {}}}"#)
            )),
            ["servers"]
        );
    }

    #[test]
    fn devcontainer_privilege_capabilities_run_args_mounts_and_features() {
        let base = r#"{"image": "x", "runArgs": ["--network=none"], "mounts": ["source=cache,target=/c,type=volume"]}"#;
        let head = r#"{
            "image": "x",
            "privileged": true,
            "capAdd": ["NET_ADMIN"],
            "securityOpt": ["seccomp=unconfined"],
            "runArgs": ["--network", "host", "--init"],
            "mounts": ["source=cache,target=/c,type=volume", {"source": "/var/run/docker.sock", "target": "/var/run/docker.sock", "type": "bind"}],
            "features": {"ghcr.io/devcontainers/features/docker-outside-of-docker:1": {}, "ghcr.io/devcontainers/features/node:1": {}}
        }"#;
        let w = widened(".devcontainer/devcontainer.json", Some(base), Some(head));
        assert_eq!(
            keys(&w),
            [
                "privileged",
                "capAdd",
                "securityOpt",
                "runArgs",
                "mounts",
                "features"
            ],
            "{w:?}"
        );
        assert!(
            w[3].1.contains("--network=host") && !w[3].1.contains("--init"),
            "{w:?}"
        );
        assert!(!w[5].1.contains("node"), "{w:?}");
        // A named configuration in a subfolder is read too.
        assert!(classify(".devcontainer/python/devcontainer.json").is_some());
        assert!(widened(".devcontainer.json", Some(head), Some(base)).is_empty());
    }

    #[test]
    fn workflow_job_and_service_containers_privilege_capabilities_and_engine_socket() {
        let base = "on: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    container:\n      image: rust:1\n      options: --cpus 2\n    services:\n      db:\n        image: postgres\n        options: --health-cmd pg_isready\n    steps:\n      - run: cargo test\n";
        let head = "on: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    container:\n      image: rust:1\n      options: --cpus 2 --privileged --pid=host\n      volumes: ['/var/run/docker.sock:/var/run/docker.sock', 'cache:/cache']\n    services:\n      db:\n        image: postgres\n        options: --health-cmd pg_isready --cap-add NET_ADMIN\n      dind:\n        image: docker:dind\n        options: --security-opt seccomp=unconfined\n        volumes: ['/var/run/docker.sock:/var/run/docker.sock']\n    steps:\n      - run: cargo test\n";
        let w = widened(".github/workflows/ci.yml", Some(base), Some(head));
        assert_eq!(
            keys(&w),
            [
                "jobs.test.container.options",
                "jobs.test.container.volumes",
                "jobs.test.services.db.options",
                "jobs.test.services.dind.options",
                "jobs.test.services.dind.volumes",
            ],
            "{w:?}"
        );
        assert!(
            w[0].1.contains("--privileged") && !w[0].1.contains("--cpus"),
            "{w:?}"
        );
        assert!(!w[1].1.contains("cache:/cache"), "{w:?}");
        // Gitea and Forgejo read the same syntax from their own directories.
        for path in [".gitea/workflows/ci.yaml", ".forgejo/workflows/ci.yml"] {
            assert_eq!(widened(path, Some(base), Some(head)).len(), 5, "{path}");
        }
        // Tightening, an image-only container and unchanged options widen nothing.
        assert!(widened(".github/workflows/ci.yml", Some(head), Some(base)).is_empty());
        let image_only =
            "jobs:\n  test:\n    container: rust:1\n    services:\n      db: postgres\n";
        assert!(widened(".github/workflows/ci.yml", None, Some(image_only)).is_empty());
        assert!(widened(".github/workflows/ci.yml", Some(head), Some(head)).is_empty());
    }

    #[test]
    fn compose_isolation_per_service() {
        let base = "services:\n  app:\n    image: x\n    cap_drop: [ALL]\n    ipc: private\n";
        let head = "services:\n  app:\n    image: x\n    privileged: true\n    network_mode: host\n    pid: host\n    ipc: host\n    cap_add: [SYS_ADMIN]\n    security_opt: ['apparmor:unconfined', 'no-new-privileges:true']\n    volumes: ['./src:/src', '/var/run/docker.sock:/var/run/docker.sock']\n    devices: ['/dev/kvm']\n";
        let w = widened("docker-compose.yml", Some(base), Some(head));
        assert_eq!(
            keys(&w),
            [
                "services.app.privileged",
                "services.app.network_mode",
                "services.app.pid",
                "services.app.ipc",
                "services.app.cap_add",
                "services.app.cap_drop",
                "services.app.security_opt",
                "services.app.devices",
                "services.app.volumes",
            ],
            "{w:?}"
        );
        assert!(!w[6].1.contains("no-new-privileges"), "{w:?}");
        assert!(!w[8].1.contains("./src"), "{w:?}");
        // `network_mode: none` and a new service on its defaults do not widen anything.
        let tight = "services:\n  app:\n    image: x\n    cap_drop: [ALL]\n    ipc: private\n    network_mode: none\n  db:\n    image: y\n";
        assert!(widened("compose.yaml", Some(base), Some(tight)).is_empty());
        // A socket the base already mounted is not gained again by an unrelated new volume.
        let sock =
            "services:\n  app:\n    volumes: ['/var/run/docker.sock:/var/run/docker.sock']\n";
        let more = "services:\n  app:\n    volumes: ['/var/run/docker.sock:/var/run/docker.sock', './src:/src']\n";
        assert!(widened("compose.yaml", Some(sock), Some(more)).is_empty());
        assert!(classify("deploy/compose.prod.yml").is_some());
        assert!(classify("compose.txt").is_none());
    }

    #[test]
    fn firewall_scripts_are_read_for_a_change_and_other_files_are_ignored() {
        assert!(matches!(
            classify(".devcontainer/init-firewall.sh"),
            Some(Classified::Executable)
        ));
        assert!(classify(".devcontainer/post-create.sh").is_none());
        assert!(classify("settings.json").is_none());
        assert!(classify(".vscode/settings.json").is_none());
    }

    #[test]
    fn codex_sandbox_approval_network_and_permission_profiles() {
        let base = "sandbox_mode = \"read-only\"\n[permissions.dev.network]\nenabled = false\n";
        let head = "sandbox_mode = \"danger-full-access\"\napproval_policy = \"never\"\n[sandbox_workspace_write]\nnetwork_access = true\n[permissions.dev.network]\nenabled = true\n[permissions.dev.network.domains]\n\"*.example.com\" = \"allow\"\n[mcp_servers.shell]\ncommand = \"sh\"\n";
        let w = widened(".codex/config.toml", Some(base), Some(head));
        assert_eq!(
            keys(&w),
            [
                "sandbox_mode",
                "approval_policy",
                "sandbox_workspace_write.network_access",
                "permissions.dev.network.enabled",
                "permissions.dev.network.domains.*.example.com",
                "mcp_servers",
            ],
            "{w:?}"
        );
        assert!(
            w[0].1.contains("`read-only` to `danger-full-access`"),
            "{w:?}"
        );
        // Removing `read-only` falls back to `workspace-write`: wider.
        let w = widened(".codex/config.toml", Some(base), Some(""));
        assert!(w[0].1.contains("unset means `workspace-write`"), "{w:?}");
        assert!(widened(".codex/config.toml", Some(head), Some(base)).is_empty());
        // An approval policy the order does not know (`granular`) is not judged.
        assert!(widened(
            ".codex/config.toml",
            None,
            Some("[approval_policy.granular]\nsandbox_approval = true\n")
        )
        .is_empty());
    }

    #[test]
    fn gemini_and_qwen_modes_tools_trust_and_hooks() {
        let w = widened(
            ".gemini/settings.json",
            Some(r#"{"tools": {"sandbox": true}, "hooksConfig": {"enabled": true}}"#),
            Some(
                r#"{"general": {"defaultApprovalMode": "auto_edit"}, "tools": {"sandbox": false, "allowed": ["run_shell_command"]}, "security": {"folderTrust": {"enabled": false}}, "hooksConfig": {"enabled": false}}"#,
            ),
        );
        assert_eq!(
            keys(&w),
            [
                "general.defaultApprovalMode",
                "tools.allowed",
                "tools.sandbox",
                "security.folderTrust.enabled",
                "hooksConfig.enabled",
            ],
            "{w:?}"
        );
        // `plan` is stricter than Gemini's default; folder trust on is its default.
        assert!(widened(
            ".gemini/settings.json",
            None,
            Some(r#"{"general": {"defaultApprovalMode": "plan"}, "security": {"folderTrust": {"enabled": true}}}"#)
        )
        .is_empty());
        let mode = |m: &str| format!(r#"{{"tools": {{"approvalMode": "{m}"}}}}"#);
        assert_eq!(
            keys(&widened(".qwen/settings.json", None, Some(&mode("yolo")))),
            ["tools.approvalMode"]
        );
        // Qwen's default is `auto`: `auto-edit` narrows it.
        assert!(widened(".qwen/settings.json", None, Some(&mode("auto-edit"))).is_empty());
        let w = widened(
            ".qwen/settings.json",
            Some(r#"{"permissions": {"deny": ["run_shell_command(rm)"]}}"#),
            Some(r#"{"permissions": {"allow": ["run_shell_command"]}, "disableAllHooks": true}"#),
        );
        assert_eq!(
            keys(&w),
            ["permissions.allow", "permissions.deny", "disableAllHooks"],
            "{w:?}"
        );
    }

    #[test]
    fn opencode_permission_actions_and_mcp() {
        let base = r#"{"permission": {"bash": {"git push*": "deny", "*": "ask"}, "edit": "ask"}}"#;
        let head = r#"{
            // OpenCode reads JSON with comments
            "permission": {"bash": {"*": "allow"}, "edit": "allow", "external_directory": "allow", "webfetch": "deny"},
            "mcp": {"shell": {"type": "local", "command": ["sh"]}},
        }"#;
        let w = widened("opencode.jsonc", Some(base), Some(head));
        assert_eq!(
            keys(&w),
            [
                "permission.external_directory",
                "permission.edit",
                "permission.bash.*",
                "permission.bash.git push*",
                "mcp",
            ],
            "{w:?}"
        );
        assert!(widened("opencode.json", None, Some(r#"{"permission": "allow"}"#)).is_empty());
        assert_eq!(
            keys(&widened(
                "opencode.json",
                Some(r#"{"permission": "ask"}"#),
                Some("{}")
            )),
            ["permission"]
        );
    }

    #[test]
    fn cursor_copilot_and_agy_files() {
        let w = widened(
            ".cursor/sandbox.json",
            Some(r#"{"networkPolicy": {"deny": ["*"]}}"#),
            Some(
                r#"{"type": "insecure_none", "networkPolicy": {"default": "allow", "allow": ["*"]}}"#,
            ),
        );
        assert_eq!(
            keys(&w),
            [
                "type",
                "networkPolicy.default",
                "networkPolicy.allow",
                "networkPolicy.deny"
            ],
            "{w:?}"
        );
        assert_eq!(
            keys(&widened(
                ".cursor/cli.json",
                None,
                Some(r#"{"permissions": {"allow": ["Shell(curl)"]}}"#)
            )),
            ["permissions.allow"]
        );
        assert_eq!(
            keys(&widened(
                ".github/copilot/settings.json",
                Some(r#"{"hooks": {"preToolUse": []}}"#),
                Some(r#"{"disableAllHooks": true}"#)
            )),
            ["disableAllHooks", "hooks"]
        );
        assert_eq!(
            keys(&widened(
                ".gemini/antigravity-cli/settings.json",
                None,
                Some(r#"{"toolPermission": "always-proceed", "enableTerminalSandbox": false}"#)
            )),
            ["toolPermission"]
        );
    }

    #[test]
    fn compose_namespace_sharing_with_another_container_is_between_private_and_host() {
        let svc = |k: &str, v: &str| format!("services:\n  app:\n    {k}: \"{v}\"\n");
        assert_eq!(
            keys(&widened(
                "compose.yaml",
                None,
                Some(&svc("network_mode", "service:vpn"))
            )),
            ["services.app.network_mode"]
        );
        assert!(widened(
            "compose.yaml",
            Some(&svc("pid", "host")),
            Some(&svc("pid", "container:abc"))
        )
        .is_empty());
    }

    #[test]
    fn posture_reports_modes_and_switches_but_not_lists() {
        let loose = r#"{"permissions": {"allow": ["Bash"], "defaultMode": "bypassPermissions"}, "enableAllProjectMcpServers": true}"#;
        let p = posture(".claude/settings.json", loose).unwrap();
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].contains("permissions.defaultMode"), "{p:?}");
        assert!(posture(
            ".claude/settings.json",
            r#"{"permissions": {"allow": ["Bash"]}}"#
        )
        .unwrap()
        .is_empty());
        assert!(posture(".claude/settings.json", "{").is_none());
    }
}
