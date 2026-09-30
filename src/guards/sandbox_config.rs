//! `sandbox-config`: a change cannot quietly widen what a coding agent may do, or the
//! isolation of the container it runs in.
//!
//! The same engine as `toolchain-config` (`toolchain_config::diff_trees`) over another rule
//! table: the project-level settings of the coding agents (permission lists, the default
//! approval or sandbox mode, sandbox network allow-lists, MCP server lists, hooks) and the
//! container definitions an agent is commonly run in (Dev Containers, Docker Compose). A file
//! that appears is compared with an empty file, and one that disappears likewise: an agent
//! with no settings file runs on its defaults, so a new file that grants `bypassPermissions`
//! widens as much as an edit that does. Runtime containment itself (system calls, outbound
//! traffic, credential reads) is the sandbox's job, not this gate's (`docs/ARCHITECTURE.md`
//! §1.3).

use super::toolchain_config::{classify_with, diff_trees, load, Classified, Judge, Rule};
use super::{Context, GateOutcome, PathFilter};
use crate::config::{GateSettings, Severity};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;
use serde_json::Value;

const CLAUDE: &[&str] = &[".claude/settings.json", ".claude/settings.local.json"];
const CODEX: &[&str] = &[".codex/config.toml"];
const GEMINI: &[&str] = &[".gemini/settings.json"];
const QWEN: &[&str] = &[".qwen/settings.json"];
const OPENCODE: &[&str] = &["opencode.json", "opencode.jsonc"];
const CURSOR_CLI: &[&str] = &[".cursor/cli.json", ".cursor/cli-config.json"];
const CURSOR_USER: &[&str] = &[".cursor/cli-config.json"];
const CURSOR_SANDBOX: &[&str] = &[".cursor/sandbox.json"];
const COPILOT: &[&str] = &[
    "copilot/settings.json",
    "copilot/settings.local.json",
    ".copilot/settings.json",
];
const COPILOT_USER: &[&str] = &[".copilot/settings.json"];
const AGY_USER: &[&str] = &["antigravity-cli/settings.json"];
const MCP_JSON: &[&str] = &[".mcp.json", ".cursor/mcp.json", ".github/mcp.json"];
const VSCODE_MCP: &[&str] = &[".vscode/mcp.json"];
const DEVCONTAINER: &[&str] = &["devcontainer.json", ".devcontainer.json"];
const COMPOSE: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
    "compose.*.yaml",
    "compose.*.yml",
    "docker-compose.*.yaml",
    "docker-compose.*.yml",
];
/// CI workflows that run jobs in containers: GitHub Actions, and Gitea and Forgejo Actions,
/// which read the same syntax from `.gitea/workflows/` and `.forgejo/workflows/`.
const WORKFLOWS: &[&str] = &["workflows/*.yml", "workflows/*.yaml"];

/// Scripts that set a container's outbound rules: read for a change, not for a delta.
pub const EXECUTABLE: &[&str] = &["*firewall*.sh"];

/// Values of `security_opt` / `securityOpt` / `--security-opt` that switch a confinement off.
const UNCONFINED: &[&str] = &[
    "seccomp=unconfined",
    "seccomp:unconfined",
    "apparmor=unconfined",
    "apparmor:unconfined",
    "label=disable",
    "label:disable",
    "systempaths=unconfined",
];
/// Mount sources that hand a container the host's container engine.
const ENGINE_SOCKETS: &[&str] = &["docker.sock", "podman.sock", "containerd.sock"];
/// `docker create` flags that widen a container: a devcontainer's `runArgs`, a workflow
/// container's `options`.
const LAX_RUN_ARGS: &[&str] = &[
    "--privileged",
    "--network=host",
    "--net=host",
    "--pid=host",
    "--ipc=host",
    "--uts=host",
    "--userns=host",
    "--cgroupns=host",
    "--cap-add",
    "--device",
    "seccomp=unconfined",
    "seccomp:unconfined",
    "apparmor=unconfined",
    "apparmor:unconfined",
    "label=disable",
    "docker.sock",
    "podman.sock",
];
/// Dev Container features that reach the host's Docker engine.
const HOST_ENGINE_FEATURES: &[&str] = &["docker-outside-of-docker", "docker-in-docker"];

const fn r(files: &'static [&'static str], path: &'static str, judge: Judge) -> Rule {
    Rule { files, path, judge }
}

/// `false` < `true`, for a switch whose absent value is `true`.
const OFF_ON: &[&str] = &["false", "true"];
/// `true` < `false`, for a protection whose absent value is `true`.
const ON_OFF: &[&str] = &["true", "false"];
/// A protection set to `"disable"` (the only value) that loosens when removed.
const DISABLED: &[&str] = &["disable", ""];
/// OpenCode and Codex per-rule actions.
const DENY_ASK_ALLOW: &[&str] = &["deny", "ask", "allow"];
const DENY_ALLOW: &[&str] = &["deny", "allow"];
/// Sharing a namespace: private, another container's (`service:` / `container:`), the host's.
const NAMESPACE: &[&str] = &["", "private", "service:", "container:", "host"];

pub const RULES: &[Rule] = &[
    // Claude Code (https://code.claude.com/docs/en/settings-reference). Since 2.1.257
    // `auto` and `bypassPermissions` do not take effect from a project file; earlier
    // releases honour them, so they are still reported.
    r(CLAUDE, "permissions.allow", Judge::Grown),
    r(CLAUDE, "permissions.ask", Judge::Shrunk),
    r(CLAUDE, "permissions.deny", Judge::Shrunk),
    r(CLAUDE, "permissions.additionalDirectories", Judge::Grown),
    r(
        CLAUDE,
        "permissions.defaultMode",
        Judge::Ranked(
            &[
                "dontAsk",
                "plan",
                "default",
                "acceptEdits",
                "auto",
                "bypassPermissions",
            ],
            2,
        ),
    ),
    r(
        CLAUDE,
        "permissions.disableBypassPermissionsMode",
        Judge::Ranked(DISABLED, 1),
    ),
    r(
        CLAUDE,
        "permissions.disableAutoMode",
        Judge::Ranked(DISABLED, 1),
    ),
    r(
        CLAUDE,
        "permissions.blockReadsOutsideWorkingDirectories",
        Judge::LooserWhenFalse,
    ),
    r(CLAUDE, "sandbox.enabled", Judge::LooserWhenFalse),
    r(CLAUDE, "sandbox.failIfUnavailable", Judge::LooserWhenFalse),
    r(
        CLAUDE,
        "sandbox.allowUnsandboxedCommands",
        Judge::Ranked(OFF_ON, 1),
    ),
    r(
        CLAUDE,
        "sandbox.enableWeakerNestedSandbox",
        Judge::LooserWhenTrue,
    ),
    r(
        CLAUDE,
        "sandbox.enableWeakerNetworkIsolation",
        Judge::LooserWhenTrue,
    ),
    r(CLAUDE, "sandbox.excludedCommands", Judge::Grown),
    r(CLAUDE, "sandbox.filesystem.allowWrite", Judge::Grown),
    r(CLAUDE, "sandbox.filesystem.allowRead", Judge::Grown),
    r(CLAUDE, "sandbox.filesystem.denyWrite", Judge::Shrunk),
    r(CLAUDE, "sandbox.filesystem.denyRead", Judge::Shrunk),
    r(CLAUDE, "sandbox.network.allowedDomains", Judge::Grown),
    r(CLAUDE, "sandbox.network.deniedDomains", Judge::Shrunk),
    r(CLAUDE, "sandbox.network.allowUnixSockets", Judge::Grown),
    r(
        CLAUDE,
        "sandbox.network.allowAllUnixSockets",
        Judge::LooserWhenTrue,
    ),
    r(
        CLAUDE,
        "sandbox.network.allowLocalBinding",
        Judge::LooserWhenTrue,
    ),
    r(CLAUDE, "enableAllProjectMcpServers", Judge::LooserWhenTrue),
    r(CLAUDE, "enabledMcpjsonServers", Judge::Grown),
    r(CLAUDE, "disabledMcpjsonServers", Judge::Shrunk),
    r(CLAUDE, "disableAllHooks", Judge::LooserWhenTrue),
    r(CLAUDE, "hooks", Judge::Shrunk),
    // Codex (https://developers.openai.com/codex/config-reference): a project's
    // `.codex/config.toml` loads once the project is trusted, and its sandbox and approval
    // keys take effect there.
    r(
        CODEX,
        "sandbox_mode",
        Judge::Ranked(&["read-only", "workspace-write", "danger-full-access"], 1),
    ),
    r(
        CODEX,
        "approval_policy",
        Judge::Ranked(&["on-request", "never"], 0),
    ),
    r(
        CODEX,
        "default_permissions",
        Judge::Ranked(&[":read-only", ":workspace", ":danger-full-access"], 1),
    ),
    r(
        CODEX,
        "sandbox_workspace_write.network_access",
        Judge::LooserWhenTrue,
    ),
    r(
        CODEX,
        "sandbox_workspace_write.writable_roots",
        Judge::Grown,
    ),
    r(
        CODEX,
        "sandbox_workspace_write.exclude_slash_tmp",
        Judge::LooserWhenFalse,
    ),
    r(
        CODEX,
        "sandbox_workspace_write.exclude_tmpdir_env_var",
        Judge::LooserWhenFalse,
    ),
    r(
        CODEX,
        "permissions.*.filesystem.*",
        Judge::Ranked(&["deny", "read", "write"], 0),
    ),
    r(
        CODEX,
        "permissions.*.network.enabled",
        Judge::LooserWhenTrue,
    ),
    r(
        CODEX,
        "permissions.*.network.mode",
        Judge::Ranked(&["limited", "full"], 0),
    ),
    r(
        CODEX,
        "permissions.*.network.domains.*",
        Judge::Ranked(DENY_ALLOW, 0),
    ),
    r(
        CODEX,
        "permissions.*.network.unix_sockets.*",
        Judge::Ranked(DENY_ALLOW, 0),
    ),
    r(
        CODEX,
        "permissions.*.network.dangerously_allow_all_unix_sockets",
        Judge::LooserWhenTrue,
    ),
    r(
        CODEX,
        "permissions.*.network.allow_local_binding",
        Judge::LooserWhenTrue,
    ),
    r(
        CODEX,
        "permissions.*.network.allow_upstream_proxy",
        Judge::LooserWhenTrue,
    ),
    r(
        CODEX,
        "permissions.*.network.dangerously_allow_non_loopback_proxy",
        Judge::LooserWhenTrue,
    ),
    r(
        CODEX,
        "web_search",
        Judge::Ranked(&["disabled", "cached", "indexed", "live"], 1),
    ),
    r(
        CODEX,
        "shell_environment_policy.inherit",
        Judge::Ranked(&["none", "core", "all"], 2),
    ),
    r(CODEX, "mcp_servers", Judge::Grown),
    r(CODEX, "hooks", Judge::Shrunk),
    // Gemini CLI (settings.schema.json at v0.44.1). `yolo` can only be set on the command line.
    r(
        GEMINI,
        "general.defaultApprovalMode",
        Judge::Ranked(&["plan", "default", "auto_edit", "yolo"], 1),
    ),
    r(GEMINI, "tools.allowed", Judge::Grown),
    r(GEMINI, "tools.confirmationRequired", Judge::Shrunk),
    r(GEMINI, "tools.exclude", Judge::Shrunk),
    r(GEMINI, "tools.sandbox", Judge::LooserWhenFalse),
    r(GEMINI, "tools.sandboxAllowedPaths", Judge::Grown),
    r(GEMINI, "tools.sandboxNetworkAccess", Judge::LooserWhenTrue),
    r(GEMINI, "security.toolSandboxing", Judge::LooserWhenFalse),
    r(GEMINI, "security.disableYoloMode", Judge::LooserWhenFalse),
    r(
        GEMINI,
        "security.disableAlwaysAllow",
        Judge::LooserWhenFalse,
    ),
    r(
        GEMINI,
        "security.enablePermanentToolApproval",
        Judge::LooserWhenTrue,
    ),
    r(
        GEMINI,
        "security.autoAddToPolicyByDefault",
        Judge::LooserWhenTrue,
    ),
    r(
        GEMINI,
        "security.folderTrust.enabled",
        Judge::Ranked(ON_OFF, 0),
    ),
    r(GEMINI, "context.includeDirectories", Judge::Grown),
    r(GEMINI, "mcpServers", Judge::Grown),
    r(GEMINI, "mcp.excluded", Judge::Shrunk),
    r(GEMINI, "hooksConfig.enabled", Judge::Ranked(ON_OFF, 0)),
    r(GEMINI, "hooksConfig.disabled", Judge::Grown),
    r(GEMINI, "hooks", Judge::Shrunk),
    // Qwen Code (settings.schema.json at v0.24.7). Folder trust is off by default, so a
    // repository's `.qwen/settings.json` loads without a prompt.
    r(
        QWEN,
        "tools.approvalMode",
        Judge::Ranked(&["plan", "default", "auto-edit", "auto", "yolo"], 3),
    ),
    r(QWEN, "permissions.allow", Judge::Grown),
    r(QWEN, "permissions.ask", Judge::Shrunk),
    r(QWEN, "permissions.deny", Judge::Shrunk),
    r(QWEN, "permissions.autoMode.hints.allow", Judge::Grown),
    r(QWEN, "permissions.autoMode.hints.softDeny", Judge::Shrunk),
    r(QWEN, "permissions.autoMode.hints.hardDeny", Judge::Shrunk),
    r(
        QWEN,
        "permissions.autoMode.classifyAllShell",
        Judge::LooserWhenFalse,
    ),
    r(QWEN, "tools.allowed", Judge::Grown),
    r(QWEN, "tools.exclude", Judge::Shrunk),
    r(QWEN, "tools.autoAccept", Judge::LooserWhenTrue),
    r(QWEN, "tools.sandbox", Judge::LooserWhenFalse),
    r(QWEN, "context.includeDirectories", Judge::Grown),
    r(QWEN, "mcpServers", Judge::Grown),
    r(QWEN, "mcp.excluded", Judge::Shrunk),
    r(QWEN, "disableAllHooks", Judge::LooserWhenTrue),
    r(
        QWEN,
        "security.allowPrivateNetworkHooks",
        Judge::LooserWhenTrue,
    ),
    r(QWEN, "security.allowedHttpHookUrls", Judge::Grown),
    r(QWEN, "hooks", Judge::Shrunk),
    // OpenCode (https://opencode.ai/config.json): most permissions default to `allow`,
    // `external_directory` and `doom_loop` to `ask`. The specific rules come first: a key
    // two patterns match is judged by the first.
    r(OPENCODE, "permission", Judge::Ranked(DENY_ASK_ALLOW, 2)),
    r(
        OPENCODE,
        "permission.external_directory",
        Judge::Ranked(DENY_ASK_ALLOW, 1),
    ),
    r(
        OPENCODE,
        "permission.doom_loop",
        Judge::Ranked(DENY_ASK_ALLOW, 1),
    ),
    r(
        OPENCODE,
        "permission.external_directory.*",
        Judge::Ranked(DENY_ASK_ALLOW, 1),
    ),
    r(OPENCODE, "permission.*", Judge::Ranked(DENY_ASK_ALLOW, 2)),
    r(OPENCODE, "permission.*.*", Judge::Ranked(DENY_ASK_ALLOW, 2)),
    r(
        OPENCODE,
        "agent.*.permission",
        Judge::Ranked(DENY_ASK_ALLOW, 2),
    ),
    r(
        OPENCODE,
        "agent.*.permission.*",
        Judge::Ranked(DENY_ASK_ALLOW, 2),
    ),
    r(
        OPENCODE,
        "agent.*.permission.*.*",
        Judge::Ranked(DENY_ASK_ALLOW, 2),
    ),
    r(OPENCODE, "tools.*", Judge::Ranked(OFF_ON, 1)),
    r(OPENCODE, "mcp", Judge::Grown),
    // Cursor (https://cursor.com/docs/cli/reference/permissions and /docs/reference/sandbox).
    r(CURSOR_CLI, "permissions.allow", Judge::Grown),
    r(CURSOR_CLI, "permissions.deny", Judge::Shrunk),
    r(
        CURSOR_USER,
        "approvalMode",
        Judge::Ranked(&["allowlist", "auto-review", "unrestricted"], 0),
    ),
    r(
        CURSOR_USER,
        "sandbox.mode",
        Judge::Ranked(&["enabled", "disabled"], 0),
    ),
    r(
        CURSOR_USER,
        "sandbox.networkAccess",
        Judge::Ranked(
            &["user_config_only", "user_config_with_defaults", "allow_all"],
            0,
        ),
    ),
    r(
        CURSOR_SANDBOX,
        "type",
        Judge::Ranked(
            &["workspace_readonly", "workspace_readwrite", "insecure_none"],
            1,
        ),
    ),
    r(CURSOR_SANDBOX, "additionalReadwritePaths", Judge::Grown),
    r(CURSOR_SANDBOX, "additionalReadonlyPaths", Judge::Grown),
    r(
        CURSOR_SANDBOX,
        "networkPolicy.default",
        Judge::Ranked(DENY_ALLOW, 0),
    ),
    r(CURSOR_SANDBOX, "networkPolicy.allow", Judge::Grown),
    r(CURSOR_SANDBOX, "networkPolicy.deny", Judge::Shrunk),
    r(CURSOR_SANDBOX, "disableTmpWrite", Judge::LooserWhenFalse),
    r(
        CURSOR_SANDBOX,
        "networkPolicyStrict",
        Judge::LooserWhenFalse,
    ),
    // Copilot CLI (https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference):
    // a repository's `.github/copilot/settings.json` wins on `disableAllHooks`.
    r(COPILOT, "disableAllHooks", Judge::LooserWhenTrue),
    r(COPILOT, "hooks", Judge::Shrunk),
    r(COPILOT, "enabledPlugins", Judge::Grown),
    r(COPILOT, "extraKnownMarketplaces", Judge::Grown),
    r(COPILOT, "deniedUrls", Judge::Shrunk),
    r(COPILOT, "disabledMcpServers", Judge::Shrunk),
    r(
        COPILOT_USER,
        "defaultPermissionMode",
        Judge::Ranked(&["manual", "assisted", "allow-all"], 0),
    ),
    r(
        COPILOT_USER,
        "defaultMode",
        Judge::Ranked(&["plan", "interactive", "autopilot"], 1),
    ),
    r(COPILOT_USER, "sandbox.enabled", Judge::LooserWhenFalse),
    r(
        COPILOT_USER,
        "sandbox.allowBypass",
        Judge::Ranked(OFF_ON, 1),
    ),
    r(
        COPILOT_USER,
        "sandbox.userPolicy.network.allowLocalNetwork",
        Judge::Ranked(OFF_ON, 1),
    ),
    r(
        COPILOT_USER,
        "sandbox.userPolicy.network.allowedHosts",
        Judge::Grown,
    ),
    r(
        COPILOT_USER,
        "sandbox.userPolicy.network.blockedHosts",
        Judge::Shrunk,
    ),
    r(
        COPILOT_USER,
        "sandbox.userPolicy.seatbelt.keychainAccess",
        Judge::LooserWhenTrue,
    ),
    r(COPILOT_USER, "allowedUrls", Judge::Grown),
    r(COPILOT_USER, "enabledMcpServers", Judge::Grown),
    // agy (https://antigravity.google/docs/sandbox?tab=cli): user-level only; no project
    // settings file is documented.
    r(
        AGY_USER,
        "toolPermission",
        Judge::Ranked(
            &[
                "strict",
                "request-review",
                "proceed-in-sandbox",
                "always-proceed",
            ],
            1,
        ),
    ),
    r(
        AGY_USER,
        "agentMode",
        Judge::Ranked(&["plan", "default", "accept-edits"], 1),
    ),
    r(AGY_USER, "enableTerminalSandbox", Judge::LooserWhenFalse),
    r(AGY_USER, "allowNonWorkspaceAccess", Judge::LooserWhenTrue),
    r(
        AGY_USER,
        "artifactReviewPolicy",
        Judge::Ranked(&["asks-for-review", "agent-decides", "always-proceed"], 0),
    ),
    r(AGY_USER, "permissions.allow", Judge::Grown),
    r(AGY_USER, "permissions.deny", Judge::Shrunk),
    // MCP server lists: a new server is a new tool with its own reach.
    r(MCP_JSON, "mcpServers", Judge::Grown),
    r(VSCODE_MCP, "servers", Judge::Grown),
    // Dev Containers (https://containers.dev/implementors/json_reference/).
    r(DEVCONTAINER, "privileged", Judge::LooserWhenTrue),
    r(DEVCONTAINER, "capAdd", Judge::Grown),
    r(
        DEVCONTAINER,
        "securityOpt",
        Judge::GainedMatching(UNCONFINED),
    ),
    r(DEVCONTAINER, "runArgs", Judge::GainedMatching(LAX_RUN_ARGS)),
    r(
        DEVCONTAINER,
        "mounts",
        Judge::GainedMatching(ENGINE_SOCKETS),
    ),
    r(
        DEVCONTAINER,
        "features",
        Judge::GainedMatching(HOST_ENGINE_FEATURES),
    ),
    r(DEVCONTAINER, "initializeCommand", Judge::Grown),
    // Docker Compose (https://github.com/compose-spec/compose-spec/blob/main/05-services.md).
    r(COMPOSE, "services.*.privileged", Judge::LooserWhenTrue),
    r(
        COMPOSE,
        "services.*.network_mode",
        Judge::Ranked(&["none", "bridge", "service:", "container:", "host"], 1),
    ),
    r(COMPOSE, "services.*.pid", Judge::Ranked(NAMESPACE, 0)),
    r(COMPOSE, "services.*.ipc", Judge::Ranked(NAMESPACE, 1)),
    r(COMPOSE, "services.*.uts", Judge::Ranked(NAMESPACE, 0)),
    r(COMPOSE, "services.*.cgroup", Judge::Ranked(NAMESPACE, 1)),
    r(
        COMPOSE,
        "services.*.userns_mode",
        Judge::Ranked(NAMESPACE, 0),
    ),
    r(COMPOSE, "services.*.cap_add", Judge::Grown),
    r(COMPOSE, "services.*.cap_drop", Judge::Shrunk),
    r(
        COMPOSE,
        "services.*.security_opt",
        Judge::GainedMatching(UNCONFINED),
    ),
    r(COMPOSE, "services.*.devices", Judge::Grown),
    r(
        COMPOSE,
        "services.*.volumes",
        Judge::GainedMatching(ENGINE_SOCKETS),
    ),
    // CI job and service containers
    // (https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax):
    // `options` are `docker create` flags, `volumes` are mounts.
    r(
        WORKFLOWS,
        "jobs.*.container.options",
        Judge::GainedMatching(LAX_RUN_ARGS),
    ),
    r(
        WORKFLOWS,
        "jobs.*.container.volumes",
        Judge::GainedMatching(ENGINE_SOCKETS),
    ),
    r(
        WORKFLOWS,
        "jobs.*.services.*.options",
        Judge::GainedMatching(LAX_RUN_ARGS),
    ),
    r(
        WORKFLOWS,
        "jobs.*.services.*.volumes",
        Judge::GainedMatching(ENGINE_SOCKETS),
    ),
];

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
) -> Result<Vec<super::toolchain_config::Weakening>, &'static str> {
    let tree = |src: Option<&str>, side: &'static str| match src {
        None => Ok(Value::Object(Default::default())),
        Some(s) => load(name, s).ok_or(side),
    };
    Ok(diff_trees(
        &tree(base, "base")?,
        &tree(head, "head")?,
        rules,
    ))
}

pub fn sandbox_config(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "sandbox-config";
    let settings = &ctx.config.gates.sandbox_config;
    let mut out = GateOutcome::new(GATE);
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    for file in ctx.git.changed_files()? {
        if exempt.matches(&file.path) {
            continue;
        }
        let Some(class) = classify(&file.path) else {
            continue;
        };
        out.examined += 1;
        let lift = |lifts: &crate::findings::FindingKind, subject: &str| {
            ctx.find_override(GATE, lifts, tokens::ALLOW_SANDBOX_WIDENING, subject)
        };
        match class {
            Classified::Executable => {
                if file.kind == ChangeKind::Added {
                    continue;
                }
                if let Some(ov) = lift(&crate::findings::SANDBOX_CHANGE_NOT_ANALYSED, &file.path) {
                    out.overrides.push(ov);
                    continue;
                }
                let sev = match settings.severity() {
                    Severity::Error => Severity::Warning,
                    other => other,
                };
                out.push(
                    ctx.overridable(sev),
                    &crate::findings::SANDBOX_CHANGE_NOT_ANALYSED,
                    Some(&file.path),
                    None,
                    format!(
                        "`{}` sets a container's outbound rules in code; whether the change widens them cannot be read from a diff.",
                        file.path
                    ),
                    "Review the change; record it with `allow-sandbox-widening: <path> <reason>` if it is intended.",
                );
            }
            Classified::Data { name, rules } => {
                // An added file has no base side and a deleted one no head side: both read
                // as `None`, which `widenings` compares as an absent file.
                let base = ctx.git.base_content(&file.old_path)?;
                let head = ctx.git.head_content(&file.path)?;
                let found = match widenings(&name, &rules, base.as_deref(), head.as_deref()) {
                    Ok(found) => found,
                    Err(side) => {
                        out.push(
                            settings.severity(),
                            &crate::findings::SANDBOX_CONFIG_UNREADABLE,
                            Some(&file.path),
                            None,
                            format!(
                                "`{}` could not be parsed on the {side} side, so whether it widens the sandbox could not be checked.",
                                file.path
                            ),
                            "Fix the file so it parses.",
                        );
                        continue;
                    }
                };
                for w in found {
                    if let Some(ov) = lift(&crate::findings::SANDBOX_CONFIG_WIDENED, &w.key)
                        .or_else(|| {
                            w.key
                                .rsplit('.')
                                .next()
                                .and_then(|k| lift(&crate::findings::SANDBOX_CONFIG_WIDENED, k))
                        })
                        .or_else(|| lift(&crate::findings::SANDBOX_CONFIG_WIDENED, &file.path))
                    {
                        out.overrides.push(ov);
                        continue;
                    }
                    out.push(
                        ctx.overridable(settings.severity()),
                        &crate::findings::SANDBOX_CONFIG_WIDENED,
                        Some(&file.path),
                        None,
                        format!("`{}` {} in `{}`.", w.key, w.what, file.path),
                        &format!(
                            "Revert it, or justify it on its own line in the PR body or a commit message: `allow-sandbox-widening: {} <reason>`.",
                            w.key
                        ),
                    );
                    out.anchor_last(w.key.clone());
                }
            }
        }
    }
    if out.examined == 0 {
        out.notes
            .push("no agent settings or container definitions modified in this diff".to_string());
    }
    Ok(out)
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
