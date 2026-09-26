//! High-assurance shell secrets and script injection sentinel (`shell-secrets`).
//!
//! Inspects shell scripts (`*.sh`, `*.bash`, `*.zsh`), Dockerfiles, and CI workflow files
//! for dangerous command-line argument secrets and unverified script execution:
//! - `ARGV-ENV`: Passing secrets via `env NAME=$SECRET` or `env NAME="$TOKEN"`
//! - `ARGV-DOCKER`: Passing secrets via `docker run -e NAME=$SECRET` or `-e NAME="literal"`
//! - `ARGV-INLINE`: Expanding secrets inside inline script arguments `python3 -c "... $SECRET ..."`
//! - `INJECT-XARGS`: Command injection via `xargs -I {} sh -c '... {} ...'`
//! - `INJECT-PIPE`: Piping remote downloads directly into shell interpreters `curl ... | sh`
//!
//! Security Invariant: Matched lines containing secret tokens MUST NEVER be echoed in reports.

use crate::config::{GateSettings, Severity, ShellSecretsGate};
use crate::guards::{exempt_filter, line_allows, Context, GateOutcome};
use crate::tokens::{self, OverrideRecord, OverrideSource};
use anyhow::Result;
use regex::Regex;

pub const GATE: &str = "shell-secrets";

const DEFAULT_SECRET_VAR_PATTERN: &str =
    r"(?i)(?:SECRET|TOKEN|PASSWORD|PASSWD|PASSPHRASE|CREDENTIAL|API_KEY|ACCESS_KEY|PRIVATE_KEY)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellRuleId {
    ArgvEnv,
    ArgvDocker,
    ArgvInline,
    InjectXargs,
    InjectPipe,
    TokenGitHub,
    TokenAws,
    TokenSlack,
    TokenOpenAi,
    PrivateKeyBlock,
    LiteralBearer,
    LiteralPassword,
    LiteralSecretEnv,
}

impl ShellRuleId {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellRuleId::ArgvEnv => "ARGV-ENV",
            ShellRuleId::ArgvDocker => "ARGV-DOCKER",
            ShellRuleId::ArgvInline => "ARGV-INLINE",
            ShellRuleId::InjectXargs => "INJECT-XARGS",
            ShellRuleId::InjectPipe => "INJECT-PIPE",
            ShellRuleId::TokenGitHub => "SECRET-TOKEN-GITHUB",
            ShellRuleId::TokenAws => "SECRET-TOKEN-AWS",
            ShellRuleId::TokenSlack => "SECRET-TOKEN-SLACK",
            ShellRuleId::TokenOpenAi => "SECRET-TOKEN-OPENAI",
            ShellRuleId::PrivateKeyBlock => "SECRET-KEY-BLOCK",
            ShellRuleId::LiteralBearer => "SECRET-LITERAL-BEARER",
            ShellRuleId::LiteralPassword => "SECRET-ARGV-PASSWORD",
            ShellRuleId::LiteralSecretEnv => "SECRET-LITERAL-ENV",
        }
    }

    pub fn kind(self) -> &'static crate::findings::FindingKind {
        match self {
            ShellRuleId::ArgvEnv => &crate::findings::SHELL_ARGV_ENV,
            ShellRuleId::ArgvDocker => &crate::findings::SHELL_ARGV_DOCKER,
            ShellRuleId::ArgvInline => &crate::findings::SHELL_ARGV_INLINE,
            ShellRuleId::InjectXargs => &crate::findings::SHELL_INJECT_XARGS,
            ShellRuleId::InjectPipe => &crate::findings::SHELL_INJECT_PIPE,
            ShellRuleId::TokenGitHub => &crate::findings::SECRET_GITHUB_TOKEN,
            ShellRuleId::TokenAws => &crate::findings::SECRET_AWS_ACCESS_KEY,
            ShellRuleId::TokenSlack => &crate::findings::SECRET_SLACK_TOKEN,
            ShellRuleId::TokenOpenAi => &crate::findings::SECRET_LLM_API_TOKEN,
            ShellRuleId::PrivateKeyBlock => &crate::findings::SECRET_PRIVATE_KEY_BLOCK,
            ShellRuleId::LiteralBearer => &crate::findings::SECRET_BEARER_TOKEN,
            ShellRuleId::LiteralPassword => &crate::findings::SECRET_PASSWORD_FLAG,
            ShellRuleId::LiteralSecretEnv => &crate::findings::SECRET_CREDENTIAL_ASSIGNMENT,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            ShellRuleId::ArgvEnv => {
                "Detected command-line secret passing via env (ARGV-ENV). Arguments are visible in process listings (ps / /proc)."
            }
            ShellRuleId::ArgvDocker => {
                "Detected command-line secret passing via docker run -e/--env (ARGV-DOCKER). Arguments are visible in process listings."
            }
            ShellRuleId::ArgvInline => {
                "Detected secret variable expansion in inline command argument (ARGV-INLINE). Expanded secrets are visible in process listings."
            }
            ShellRuleId::InjectXargs => {
                "Detected unsafe placeholder substitution into shell script (INJECT-XARGS). Metacharacters in inputs can cause command injection."
            }
            ShellRuleId::InjectPipe => {
                "Detected piped remote download into shell interpreter (INJECT-PIPE). Unverified remote script execution is vulnerable to supply-chain tampering."
            }
            ShellRuleId::TokenGitHub => {
                "Detected literal GitHub personal access token or app token in script/command. Hardcoded credentials leak in repository history and process arguments."
            }
            ShellRuleId::TokenAws => {
                "Detected literal AWS access key ID or secret access key in script/command. Hardcoded credentials leak in repository history and process arguments."
            }
            ShellRuleId::TokenSlack => {
                "Detected literal Slack API token in script/command. Hardcoded credentials leak in repository history and process arguments."
            }
            ShellRuleId::TokenOpenAi => {
                "Detected literal OpenAI or Anthropic API key in script/command. Hardcoded credentials leak in repository history and process arguments."
            }
            ShellRuleId::PrivateKeyBlock => {
                "Detected cryptographic private key block in script/command. Private keys must never be committed or passed in scripts."
            }
            ShellRuleId::LiteralBearer => {
                "Detected literal bearer token in Authorization header. Tokens should be passed via environment variables."
            }
            ShellRuleId::LiteralPassword => {
                "Detected literal password passed on command-line argument. Arguments are visible in process tables; use stdin or environment variables."
            }
            ShellRuleId::LiteralSecretEnv => {
                "Detected literal secret or API key assignment in script/command. Inject secrets dynamically rather than committing literals."
            }
        }
    }

    pub fn remediation(self) -> &'static str {
        match self {
            ShellRuleId::ArgvEnv => {
                "Export secrets into the environment before invoking the command, or invoke the script directly without env."
            }
            ShellRuleId::ArgvDocker => {
                "Pass environment variables by name without values (e.g. docker run -e VAR) to inherit from runner environment, or use --env-file."
            }
            ShellRuleId::ArgvInline => {
                "Pass secrets via standard input or environment variables rather than expanding them in inline -c arguments."
            }
            ShellRuleId::InjectXargs => {
                "Pass arguments safely as positional parameters: xargs -I {} sh -c 'cmd \"$1\"' _ {}"
            }
            ShellRuleId::InjectPipe => {
                "Download scripts to a temporary file, verify their checksum or signature, and then execute."
            }
            ShellRuleId::TokenGitHub
            | ShellRuleId::TokenAws
            | ShellRuleId::TokenSlack
            | ShellRuleId::TokenOpenAi => {
                "Inject tokens from environment variables or CI secret store (e.g. ${{ secrets.TOKEN }}), never commit literal values."
            }
            ShellRuleId::PrivateKeyBlock => {
                "Load private keys from secure secrets storage or files mounted into the environment; never embed key blocks in scripts."
            }
            ShellRuleId::LiteralBearer => {
                "Use Authorization: Bearer $TOKEN or ${{ secrets.TOKEN }} instead of a literal string."
            }
            ShellRuleId::LiteralPassword => {
                "Pass passwords via standard input (--password-stdin) or environment variables to keep them out of process listings."
            }
            ShellRuleId::LiteralSecretEnv => {
                "Reference environment variables (e.g. export KEY=$ENV_KEY) or fetch secrets dynamically at runtime."
            }
        }
    }
}

fn is_placeholder_or_var(val: &str) -> bool {
    let s = val.trim().trim_matches(|c| c == '\'' || c == '"');
    if s.is_empty() || s == "--password-stdin" {
        return true;
    }
    if s.starts_with('$') || s.starts_with("${{") || s.contains("${{ secrets.") {
        return true;
    }
    if s.starts_with("$(") || s.starts_with('`') {
        return true;
    }
    if s.starts_with('<') && s.ends_with('>') {
        return true;
    }
    if s.len() >= 4 && s.chars().all(|c| c == 'x' || c == 'X') {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    let known_placeholders = [
        "changeme",
        "dummy",
        "example",
        "sample",
        "test",
        "foo",
        "bar",
        "placeholder",
        "your_api_key",
        "your-api-key",
        "your_token",
        "your-token",
        "password",
        "secret",
        "token",
    ];
    if known_placeholders
        .iter()
        .any(|&p| lower == p || lower.starts_with("todo") || lower.starts_with("fixme"))
    {
        return true;
    }
    false
}

pub struct ShellSecretScanner {
    re_env: Regex,
    re_docker: Regex,
    re_inline: Regex,
    re_xargs: Regex,
    re_pipe: Regex,
    re_token_github: Regex,
    re_token_aws: Regex,
    re_token_slack: Regex,
    re_token_openai: Regex,
    re_private_key: Regex,
    re_bearer: Regex,
    re_password_flag: Regex,
    re_short_password: Regex,
    re_aws_secret: Regex,
    re_generic_secret: Regex,
    allow_patterns: Vec<Regex>,
}

impl ShellSecretScanner {
    pub fn new(settings: &ShellSecretsGate) -> Result<Self> {
        let mut secret_parts = vec![DEFAULT_SECRET_VAR_PATTERN.to_string()];
        for p in &settings.extra_secret_patterns {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                secret_parts.push(format!("(?:{trimmed})"));
            }
        }
        let secret_union = secret_parts.join("|");

        let re_env = Regex::new(&format!(
            r#"(?i)\benv\b(?:\s+-[A-Za-z0-9_-]+)*\s+.*?(?:\b[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*\s*=|=[^ \t\n\r]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union}))"#
        ))?;

        let re_docker = Regex::new(&format!(
            r#"(?i)\b(?:docker|podman)\s+(?:container\s+)?run\b.*?(?:-e|--env(?:=|\s+))\s*['"]?(?:[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*\s*=|[^=\s'"]+=\s*['"]?[^ \t\n\r]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union}))"#
        ))?;

        let re_inline = Regex::new(&format!(
            r#"(?i)\b(?:python[0-9.]*|bash|sh|zsh|node|ruby|perl)\b(?:\s+-[A-Za-z0-9_-]+)*\s+(?:-[A-Za-z0-9_-]*c|-c|-e)\s+(?:(?:"[^"]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*[}}]?|'[^']*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*[}}]?)|(?:'[^']*'|"[^"]*"|[^\s]+)\s+[^;\n\r|&]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union}))"#
        ))?;

        let re_xargs = Regex::new(
            r#"(?i)\bxargs\b.*?-[iI]\s*(\{\}|[A-Za-z0-9_%@]+)\s+.*?\b(?:sh|bash|zsh)\s+-c\s+(.*)"#,
        )?;

        let re_pipe = Regex::new(
            r#"(?i)\b(?:curl|wget)\b[^|;\n\r]*\|\s*(?:sudo\s+)?(?:\/bin\/|\/usr\/bin\/)?(?:sh|bash|zsh)\b"#,
        )?;

        let re_token_github =
            Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9_]{36,}|github_pat_[A-Za-z0-9_]{82})\b")?;

        let re_token_aws = Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b")?;

        let re_token_slack = Regex::new(r"\bxox[baprs]-[0-9a-zA-Z-]{10,}\b")?;

        let re_token_openai = Regex::new(r"\bsk-(?:proj-|ant-api[0-9]{2}-)?[a-zA-Z0-9_-]{20,}\b")?;

        let re_private_key = Regex::new(r"-----BEGIN (?:[A-Z0-9 ]+)?PRIVATE KEY(?: BLOCK)?-----")?;

        let re_bearer =
            Regex::new(r#"(?i)\bAuthorization:\s*Bearer\s+['"]?([A-Za-z0-9_\-\.]{12,})['"]?"#)?;

        let re_password_flag =
            Regex::new(r#"(?i)(?:--password|--passwd)(?:=|\s+)(?:['"]([^'"]+)['"]|([^\s'"]+))"#)?;

        let re_short_password = Regex::new(
            r#"(?i)\b(?:mysql|mariadb|docker\s+login|podman\s+login)\b.*?(?:(?:\s|^)-p(?:['"]([^'"]+)['"]|([A-Za-z0-9_!@#$%^&*+=/]+))|(?:\s|^)-p\s+(?:['"]([^'"]+)['"]|([^\s'"-]+)))"#,
        )?;

        let re_aws_secret = Regex::new(
            r#"(?i)\bAWS_SECRET_ACCESS_KEY\s*=\s*(?:['"]([A-Za-z0-9/+=]{20,})['"]|([A-Za-z0-9/+=]{20,}))"#,
        )?;

        let re_generic_secret = Regex::new(
            r#"(?i)\b(?:export\s+)?([A-Za-z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_KEY|ACCESS_KEY|PRIVATE_KEY)[A-Za-z0-9_]*)\s*=\s*(?:['"]([A-Za-z0-9/+=_-]{16,})['"]|([A-Za-z0-9/+=_-]{16,}))"#,
        )?;

        let mut allow_patterns = Vec::new();
        for p in &settings.allow_patterns {
            allow_patterns.push(Regex::new(p)?);
        }

        Ok(Self {
            re_env,
            re_docker,
            re_inline,
            re_xargs,
            re_pipe,
            re_token_github,
            re_token_aws,
            re_token_slack,
            re_token_openai,
            re_private_key,
            re_bearer,
            re_password_flag,
            re_short_password,
            re_aws_secret,
            re_generic_secret,
            allow_patterns,
        })
    }

    /// Check if line is exempt via allow patterns
    pub fn is_line_allowed(&self, line: &str) -> bool {
        self.allow_patterns.iter().any(|re| re.is_match(line))
    }

    /// Scans a single command line for unsafe patterns.
    /// Returns the matched ShellRuleId if unsafe.
    pub fn check_line(&self, line: &str) -> Option<ShellRuleId> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }

        if self.is_line_allowed(line) {
            return None;
        }

        // 1. Specific Provider Tokens (high confidence)
        if self.re_token_github.is_match(line) {
            return Some(ShellRuleId::TokenGitHub);
        }
        if self.re_token_aws.is_match(line) {
            return Some(ShellRuleId::TokenAws);
        }
        if self.re_token_slack.is_match(line) {
            return Some(ShellRuleId::TokenSlack);
        }
        if self.re_token_openai.is_match(line) {
            return Some(ShellRuleId::TokenOpenAi);
        }
        if self.re_private_key.is_match(line) {
            return Some(ShellRuleId::PrivateKeyBlock);
        }

        // 2. Authorization Bearer literal
        if let Some(caps) = self.re_bearer.captures(line) {
            if let Some(val) = caps.get(1) {
                if !is_placeholder_or_var(val.as_str()) {
                    return Some(ShellRuleId::LiteralBearer);
                }
            }
        }

        // 3. Command-line password flags
        if let Some(caps) = self.re_password_flag.captures(line) {
            let val = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            if !val.is_empty() && !is_placeholder_or_var(val) {
                return Some(ShellRuleId::LiteralPassword);
            }
        }
        if let Some(caps) = self.re_short_password.captures(line) {
            let val = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(3))
                .or_else(|| caps.get(4))
                .map(|m| m.as_str())
                .unwrap_or("");
            if !val.is_empty() && !val.starts_with('-') && !is_placeholder_or_var(val) {
                return Some(ShellRuleId::LiteralPassword);
            }
        }

        // 4. AWS Secret Access Key
        if let Some(caps) = self.re_aws_secret.captures(line) {
            let val = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            if !val.is_empty() && !is_placeholder_or_var(val) {
                return Some(ShellRuleId::TokenAws);
            }
        }

        // 5. Generic literal secret assignment
        if let Some(caps) = self.re_generic_secret.captures(line) {
            let val = caps
                .get(2)
                .or_else(|| caps.get(3))
                .map(|m| m.as_str())
                .unwrap_or("");
            if !val.is_empty() && !is_placeholder_or_var(val) {
                return Some(ShellRuleId::LiteralSecretEnv);
            }
        }

        // 6. INJECT-PIPE: Check first (fast and specific)
        if self.re_pipe.is_match(line) {
            return Some(ShellRuleId::InjectPipe);
        }

        // 7. INJECT-XARGS
        if let Some(caps) = self.re_xargs.captures(line) {
            let placeholder = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let rest = caps.get(2).map(|m| m.as_str()).unwrap_or("").trim();
            if !placeholder.is_empty() {
                let script_token = if let Some(stripped) = rest.strip_prefix('\'') {
                    stripped.split('\'').next().unwrap_or("")
                } else if let Some(stripped) = rest.strip_prefix('"') {
                    stripped.split('"').next().unwrap_or("")
                } else {
                    rest.split_whitespace().next().unwrap_or("")
                };
                if script_token.contains(placeholder) {
                    return Some(ShellRuleId::InjectXargs);
                }
            }
        }

        // 8. ARGV-INLINE
        if self.re_inline.is_match(line) {
            return Some(ShellRuleId::ArgvInline);
        }

        // 9. ARGV-DOCKER
        if self.re_docker.is_match(line) {
            return Some(ShellRuleId::ArgvDocker);
        }

        // 10. ARGV-ENV
        if !trimmed.starts_with("#!") && self.re_env.is_match(line) {
            return Some(ShellRuleId::ArgvEnv);
        }

        None
    }
}

pub fn is_shell_secret_target(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();

    // 1. Shell scripts
    if lower.ends_with(".sh") || lower.ends_with(".bash") || lower.ends_with(".zsh") {
        return true;
    }

    // 2. Dockerfiles
    let filename = lower.rsplit('/').next().unwrap_or(&lower);
    if filename == "dockerfile"
        || filename.starts_with("dockerfile.")
        || filename.ends_with(".dockerfile")
    {
        return true;
    }

    // 3. CI Workflows
    if (lower.contains(".github/workflows/")
        || lower.contains(".gitea/workflows/")
        || lower.contains(".forgejo/workflows/")
        || lower.contains("workflows/"))
        && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
    {
        return true;
    }

    if lower.ends_with(".gitlab-ci.yml")
        || (lower.contains("templates/") && lower.ends_with(".gitlab-ci.yml"))
    {
        return true;
    }

    false
}

/// Checks if a line contains an inline directive for shell-secrets.
fn line_has_secrets_argv_ok(line: &str) -> bool {
    if line_allows(line, GATE) {
        return true;
    }
    const MARKER: &str = "secrets-argv-ok:";
    if let Some(pos) = line.find(MARKER) {
        let reason = line[pos + MARKER.len()..].trim();
        let cleaned = reason
            .trim_end_matches("-->")
            .trim_end_matches("--!>")
            .trim();
        return !cleaned.is_empty() && !cleaned.starts_with('<');
    }
    false
}

#[derive(Debug, Clone)]
pub struct LogicalLine {
    pub content: String,
    pub physical_lines: Vec<usize>,
    pub primary_line: usize,
    pub has_inline_allow: bool,
}

pub fn parse_logical_lines(content: &str) -> Vec<LogicalLine> {
    let mut logical_lines = Vec::new();
    let mut current_parts: Vec<(usize, String)> = Vec::new();
    let raw_lines: Vec<&str> = content.lines().collect();

    for (idx, line) in raw_lines.iter().enumerate() {
        let physical_line = idx + 1;
        let mut trimmed_end = line.trim_end();

        // Support comments following a continuation backslash (e.g. `cmd \ # comment`)
        if let Some(hash_pos) = trimmed_end.find('#') {
            let before_hash = trimmed_end[..hash_pos].trim_end();
            if before_hash.ends_with('\\') && !before_hash.ends_with(r"\\") {
                trimmed_end = before_hash;
            }
        }

        let trailing_backslashes = trimmed_end.chars().rev().take_while(|&c| c == '\\').count();

        let is_continuation = trailing_backslashes % 2 == 1;

        if is_continuation {
            let without_slash = &trimmed_end[..trimmed_end.len() - 1];
            current_parts.push((physical_line, without_slash.to_string()));
        } else {
            current_parts.push((physical_line, line.to_string()));

            let mut combined = String::new();
            let mut phys = Vec::with_capacity(current_parts.len());
            let mut has_inline_allow = false;

            for (p_line, part) in &current_parts {
                phys.push(*p_line);
                let orig_line = raw_lines.get(p_line - 1).copied().unwrap_or("");
                if line_has_secrets_argv_ok(orig_line) {
                    has_inline_allow = true;
                }
                if !combined.is_empty() {
                    combined.push(' ');
                }
                combined.push_str(part);
            }

            if line_has_secrets_argv_ok(&combined) {
                has_inline_allow = true;
            }

            let primary_line = phys
                .iter()
                .copied()
                .find(|&p| {
                    let orig = raw_lines.get(p - 1).copied().unwrap_or("");
                    let lower = orig.to_ascii_lowercase();
                    lower.contains("secret")
                        || lower.contains("token")
                        || lower.contains("password")
                        || lower.contains("passwd")
                        || lower.contains("passphrase")
                        || lower.contains("key")
                        || lower.contains("-e")
                        || lower.contains("--env")
                        || lower.contains("bearer")
                        || lower.contains("sk-")
                })
                .unwrap_or(phys[0]);

            logical_lines.push(LogicalLine {
                content: combined,
                physical_lines: phys,
                primary_line,
                has_inline_allow,
            });

            current_parts.clear();
        }
    }

    if !current_parts.is_empty() {
        let mut combined = String::new();
        let mut phys = Vec::with_capacity(current_parts.len());
        let mut has_inline_allow = false;

        for (p_line, part) in &current_parts {
            phys.push(*p_line);
            let orig_line = raw_lines.get(p_line - 1).copied().unwrap_or("");
            if line_has_secrets_argv_ok(orig_line) {
                has_inline_allow = true;
            }
            if !combined.is_empty() {
                combined.push(' ');
            }
            combined.push_str(part);
        }

        if line_has_secrets_argv_ok(&combined) {
            has_inline_allow = true;
        }

        let primary_line = phys
            .iter()
            .copied()
            .find(|&p| {
                let orig = raw_lines.get(p - 1).copied().unwrap_or("");
                let lower = orig.to_ascii_lowercase();
                lower.contains("secret")
                    || lower.contains("token")
                    || lower.contains("password")
                    || lower.contains("passwd")
                    || lower.contains("passphrase")
                    || lower.contains("key")
                    || lower.contains("-e")
                    || lower.contains("--env")
                    || lower.contains("bearer")
                    || lower.contains("sk-")
            })
            .unwrap_or(phys[0]);

        logical_lines.push(LogicalLine {
            content: combined,
            physical_lines: phys,
            primary_line,
            has_inline_allow,
        });
    }

    logical_lines
}

pub fn evaluate_shell_secrets(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.shell_secrets;
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);

    let scanner = ShellSecretScanner::new(settings)?;

    let target_files: Vec<(String, Option<std::collections::BTreeSet<usize>>)> =
        if settings.diff_only {
            let changed = ctx.git.changed_files()?;
            changed
                .into_iter()
                .filter(|f| {
                    !f.is_deleted() && !exempt.matches(&f.path) && is_shell_secret_target(&f.path)
                })
                .map(|f| (f.path, Some(f.added_lines)))
                .collect()
        } else {
            let tracked = ctx.git.tracked_files()?;
            tracked
                .into_iter()
                .filter(|file| !exempt.matches(file) && is_shell_secret_target(file))
                .map(|file| (file, None))
                .collect()
        };

    if target_files.is_empty() {
        out.examined = 0;
        out.notes.push(
            "no shell scripts, Dockerfiles, or CI workflow files found; shell-secrets check examined 0 files"
                .to_string(),
        );
        return Ok(out);
    }

    out.examined = target_files.len();

    for (file, added_lines) in &target_files {
        // Scoped file-level PR body override
        if let Some(record) = ctx.find_override(GATE, tokens::SECRETS_ARGV_OK, file) {
            out.overrides.push(record);
            continue;
        }

        let content = match ctx.git.head_content(file) {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(e) => {
                out.notes
                    .push(format!("could not read content of {file}: {e}"));
                continue;
            }
        };

        let logical_lines = parse_logical_lines(&content);

        for log_line in logical_lines {
            if let Some(lines) = added_lines {
                if !log_line.physical_lines.iter().any(|p| lines.contains(p)) {
                    continue;
                }
            }

            if let Some(rule) = scanner.check_line(&log_line.content) {
                // Check PR body overrides across all physical lines in this logical line
                let mut overridden = false;
                for &p_line in &log_line.physical_lines {
                    let line_subject = format!("{file}:{p_line}");
                    if let Some(record) =
                        ctx.find_override(GATE, tokens::SECRETS_ARGV_OK, &line_subject)
                    {
                        out.overrides.push(record);
                        overridden = true;
                        break;
                    }
                }
                if overridden {
                    continue;
                }

                if log_line.has_inline_allow {
                    out.inline_exemptions += 1;
                    out.overrides.push(OverrideRecord {
                        gate: GATE.to_string(),
                        subject: format!("{file}:{}", log_line.primary_line),
                        directive: "secrets-argv-ok".to_string(),
                        reason: "inline exemption marker".to_string(),
                        source: OverrideSource::Inline {
                            file: file.clone(),
                            line: log_line.primary_line,
                        },
                        hidden: true,
                    });
                    continue;
                }

                // Security Invariant: NEVER include matched line content or secret token in message
                let rule_sev = match rule {
                    ShellRuleId::TokenGitHub
                    | ShellRuleId::TokenAws
                    | ShellRuleId::TokenSlack
                    | ShellRuleId::TokenOpenAi
                    | ShellRuleId::PrivateKeyBlock => settings.severity(),
                    _ => Severity::Warning,
                };
                out.push(
                    rule_sev,
                    rule.kind(),
                    Some(file),
                    Some(log_line.primary_line),
                    rule.message().to_string(),
                    rule.remediation(),
                );
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_file_detection_discriminates() {
        assert!(is_shell_secret_target("scripts/deploy.sh"));
        assert!(is_shell_secret_target("bin/run.bash"));
        assert!(is_shell_secret_target("tools/check.zsh"));
        assert!(is_shell_secret_target("Dockerfile"));
        assert!(is_shell_secret_target("docker/Dockerfile.dev"));
        assert!(is_shell_secret_target("app.dockerfile"));
        assert!(is_shell_secret_target(".github/workflows/ci.yml"));
        assert!(is_shell_secret_target(".gitea/workflows/build.yaml"));
        assert!(is_shell_secret_target(".forgejo/workflows/test.yml"));
        assert!(is_shell_secret_target(".gitlab-ci.yml"));
        assert!(is_shell_secret_target("templates/discipline.gitlab-ci.yml"));

        assert!(!is_shell_secret_target("src/main.rs"));
        assert!(!is_shell_secret_target("README.md"));
        assert!(!is_shell_secret_target("Cargo.toml"));
        assert!(!is_shell_secret_target("docs/index.html"));
    }

    #[test]
    fn argv_env_detector_discriminates() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        // Positive controls
        assert_eq!(
            scanner.check_line("env API_KEY=$SECRET ./deploy.sh"),
            Some(ShellRuleId::ArgvEnv)
        );
        assert_eq!(
            scanner.check_line("env TOKEN=\"ghp_12345\" ./script"),
            Some(ShellRuleId::ArgvEnv)
        );
        assert_eq!(
            scanner.check_line("env -i MY_PASSWORD=secret ./cmd"),
            Some(ShellRuleId::ArgvEnv)
        );
        assert_eq!(
            scanner.check_line("env FOO=${AUTH_TOKEN} ./run"),
            Some(ShellRuleId::ArgvEnv)
        );

        // Negative controls
        assert_eq!(scanner.check_line("#!/usr/bin/env bash"), None);
        assert_eq!(scanner.check_line("env PATH=$PATH ./cmd"), None);
        assert_eq!(scanner.check_line("env FOO=bar ./test"), None);
        assert_eq!(scanner.check_line("API_KEY=$SECRET ./deploy.sh"), None);
    }

    #[test]
    fn argv_docker_detector_discriminates() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        // Positive controls
        assert_eq!(
            scanner.check_line("docker run -e DB_PASSWORD=$PASSWORD myimage"),
            Some(ShellRuleId::ArgvDocker)
        );
        assert_eq!(
            scanner.check_line("docker run -e TOKEN=\"xyz\" myimage"),
            Some(ShellRuleId::ArgvDocker)
        );
        assert_eq!(
            scanner.check_line("docker run --env API_KEY=$KEY myimage"),
            Some(ShellRuleId::ArgvDocker)
        );
        assert_eq!(
            scanner.check_line("docker run --env=PASSWORD=$PASS myimage"),
            Some(ShellRuleId::ArgvDocker)
        );
        assert_eq!(
            scanner.check_line("podman run -e SECRET=val myimage"),
            Some(ShellRuleId::ArgvDocker)
        );

        // Negative controls
        assert_eq!(scanner.check_line("docker run -e PORT=8080 myimage"), None);
        assert_eq!(scanner.check_line("docker run -e SECRET myimage"), None);
        assert_eq!(scanner.check_line("docker run -e API_KEY myimage"), None);
        assert_eq!(scanner.check_line("docker build -t test ."), None);
    }

    #[test]
    fn argv_inline_detector_discriminates() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        // Positive controls
        assert_eq!(
            scanner.check_line("python3 -c \"import os; print('$API_KEY')\""),
            Some(ShellRuleId::ArgvInline)
        );
        assert_eq!(
            scanner.check_line("sh -c \"echo $SECRET\""),
            Some(ShellRuleId::ArgvInline)
        );
        assert_eq!(
            scanner.check_line("bash -c \"curl -H 'Authorization: $TOKEN' https://api.com\""),
            Some(ShellRuleId::ArgvInline)
        );

        // Positional argument positive controls (Issue #66)
        assert_eq!(
            scanner.check_line("calc_hmac=$(python3 -c 'import hmac,sys; print(sys.argv[1])' \"$SECRETS_PASSPHRASE\" \"$enc\")"),
            Some(ShellRuleId::ArgvInline)
        );
        assert_eq!(
            scanner.check_line("python3 -c 'import sys; print(sys.argv[1])' \"$SECRET\""),
            Some(ShellRuleId::ArgvInline)
        );
        assert_eq!(
            scanner.check_line("ruby -e 'puts ARGV[0]' \"$API_KEY\""),
            Some(ShellRuleId::ArgvInline)
        );
        assert_eq!(
            scanner.check_line("node -e 'console.log(process.argv[1])' \"$TOKEN\""),
            Some(ShellRuleId::ArgvInline)
        );
        assert_eq!(
            scanner.check_line("sh -c 'echo \"$1\"' _ \"$MY_SECRET\""),
            Some(ShellRuleId::ArgvInline)
        );

        // Negative controls
        assert_eq!(
            scanner.check_line("python3 -c \"import sys; print(sys.version)\""),
            None
        );
        assert_eq!(scanner.check_line("sh -c \"echo hello\""), None);
        assert_eq!(scanner.check_line("sh -c \"echo $USER\""), None);
        assert_eq!(
            scanner
                .check_line("SECRET=\"$s\" python3 -c 'import os; print(os.environ[\"SECRET\"])'"),
            None
        );
        assert_eq!(
            scanner.check_line("python3 -c 'import sys; print(sys.argv[1])' \"$input_file\""),
            None
        );
        assert_eq!(
            scanner.check_line("python3 -c 'print(\"done\")' ; echo \"$SECRET\""),
            None
        );
    }

    #[test]
    fn parse_logical_lines_and_multiline_continuation_discriminates() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        let multiline_docker = r#"docker run \
  -e AWS_SECRET_ACCESS_KEY="$AWS_SECRET_ACCESS_KEY" \
  amazon/aws-cli:latest"#;

        let logical = parse_logical_lines(multiline_docker);
        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].physical_lines, vec![1, 2, 3]);
        assert_eq!(logical[0].primary_line, 2);
        assert_eq!(
            scanner.check_line(&logical[0].content),
            Some(ShellRuleId::ArgvDocker)
        );

        let split_env_docker = r#"docker run \
  -e \
  AWS_SECRET_ACCESS_KEY="$AWS_SECRET_ACCESS_KEY" \
  amazon/aws-cli:latest"#;

        let logical_split = parse_logical_lines(split_env_docker);
        assert_eq!(logical_split.len(), 1);
        assert_eq!(
            scanner.check_line(&logical_split[0].content),
            Some(ShellRuleId::ArgvDocker)
        );

        let allowed_multiline = r#"docker run \
  -e AWS_SECRET_ACCESS_KEY="$AWS_SECRET_ACCESS_KEY" \ # secrets-argv-ok: test fixture
  amazon/aws-cli:latest"#;

        let logical_allowed = parse_logical_lines(allowed_multiline);
        assert_eq!(logical_allowed.len(), 1);
        assert!(logical_allowed[0].has_inline_allow);
    }

    #[test]
    fn inject_xargs_detector_discriminates() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        // Positive controls
        assert_eq!(
            scanner.check_line("xargs -I {} sh -c 'echo {}'"),
            Some(ShellRuleId::InjectXargs)
        );
        assert_eq!(
            scanner.check_line("xargs -I % bash -c 'rm %'"),
            Some(ShellRuleId::InjectXargs)
        );

        // Negative controls
        assert_eq!(scanner.check_line("xargs -I {} rm {}"), None);
        assert_eq!(
            scanner.check_line("xargs -I {} sh -c 'echo \"$1\"' _ {}"),
            None
        );
    }

    #[test]
    fn inject_pipe_detector_discriminates() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        // Positive controls
        assert_eq!(
            scanner.check_line("curl -fsSL https://get.docker.com | sh"),
            Some(ShellRuleId::InjectPipe)
        );
        assert_eq!(
            scanner.check_line("curl https://example.com/install.sh | bash"),
            Some(ShellRuleId::InjectPipe)
        );
        assert_eq!(
            scanner.check_line("wget -O - https://example.com/install.sh | sudo bash"),
            Some(ShellRuleId::InjectPipe)
        );

        // Negative controls
        assert_eq!(
            scanner.check_line("curl https://example.com/data.json | jq ."),
            None
        );
        assert_eq!(
            scanner.check_line("wget https://example.com/archive.tar.gz -O archive.tar.gz"),
            None
        );
    }

    #[test]
    fn inline_directive_and_redaction_enforced() {
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();
        let bad_line = "curl https://example.com/install.sh | bash";
        assert_eq!(scanner.check_line(bad_line), Some(ShellRuleId::InjectPipe));

        let waived_line = "curl https://example.com/install.sh | bash # secrets-argv-ok: verified vendor installer";
        assert!(line_has_secrets_argv_ok(waived_line));

        let discipline_allow =
            "curl https://example.com/install.sh | bash # discipline:allow(shell-secrets)";
        assert!(line_has_secrets_argv_ok(discipline_allow));

        // Ensure security invariant: Rule message must never echo secret tokens
        for rule in [
            ShellRuleId::ArgvEnv,
            ShellRuleId::ArgvDocker,
            ShellRuleId::ArgvInline,
            ShellRuleId::InjectXargs,
            ShellRuleId::InjectPipe,
            ShellRuleId::TokenGitHub,
            ShellRuleId::TokenAws,
            ShellRuleId::TokenSlack,
            ShellRuleId::TokenOpenAi,
            ShellRuleId::PrivateKeyBlock,
            ShellRuleId::LiteralBearer,
            ShellRuleId::LiteralPassword,
            ShellRuleId::LiteralSecretEnv,
        ] {
            assert!(!rule.message().contains("$SECRET"));
            assert!(!rule.message().contains("ghp_"));
            assert!(!rule.message().contains("AKIA"));
            assert!(!rule.remediation().contains("$SECRET"));
        }
    }

    #[derive(serde::Deserialize)]
    struct CorpusCase {
        line: String,
        is_secret: bool,
        category: String,
    }

    #[test]
    fn shell_secrets_corpus_evaluation() {
        let corpus_raw = include_str!("../../tests/fixtures/shell_secrets_corpus.json");
        let cases: Vec<CorpusCase> = serde_json::from_str(corpus_raw).unwrap();
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

        let mut false_negatives = 0;
        let mut false_positives = 0;
        let mut total_positives = 0;
        let mut total_negatives = 0;

        for case in &cases {
            let hit = scanner.check_line(&case.line);
            if case.is_secret {
                total_positives += 1;
                if hit.is_none() {
                    false_negatives += 1;
                    eprintln!("FN: [{}] {}", case.category, case.line);
                }
            } else {
                total_negatives += 1;
                if hit.is_some() {
                    false_positives += 1;
                    eprintln!("FP: [{}] {}", case.category, case.line);
                }
            }
        }
        println!(
            "Corpus stats: Positives={total_positives} (FN={false_negatives}), Negatives={total_negatives} (FP={false_positives})"
        );
        assert_eq!(
            false_negatives, 0,
            "All secrets in corpus must be detected (0 false negatives)"
        );
        assert_eq!(
            false_positives, 0,
            "No safe lines in corpus may be flagged (0 false positives)"
        );
    }
}
