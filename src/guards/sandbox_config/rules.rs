//! The `sandbox-config` tables: which key of which agent settings file or container
//! definition widens in which direction, and the files that set outbound rules in code.

use crate::guards::config_delta::{Judge, Rule};

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
