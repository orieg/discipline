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

use crate::config::{GateSettings, ShellSecretsGate};
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
}

impl ShellRuleId {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellRuleId::ArgvEnv => "ARGV-ENV",
            ShellRuleId::ArgvDocker => "ARGV-DOCKER",
            ShellRuleId::ArgvInline => "ARGV-INLINE",
            ShellRuleId::InjectXargs => "INJECT-XARGS",
            ShellRuleId::InjectPipe => "INJECT-PIPE",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ShellRuleId::ArgvEnv => "Unsafe Shell Pattern: ARGV-ENV",
            ShellRuleId::ArgvDocker => "Unsafe Shell Pattern: ARGV-DOCKER",
            ShellRuleId::ArgvInline => "Unsafe Shell Pattern: ARGV-INLINE",
            ShellRuleId::InjectXargs => "Unsafe Shell Pattern: INJECT-XARGS",
            ShellRuleId::InjectPipe => "Unsafe Shell Pattern: INJECT-PIPE",
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
        }
    }
}

pub struct ShellSecretScanner {
    re_env: Regex,
    re_docker: Regex,
    re_inline: Regex,
    re_xargs: Regex,
    re_pipe: Regex,
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

        // ARGV-ENV: env [flags] VAR=$SECRET or env [flags] SECRET=...
        // Matches `\benv\b` where one of the arguments assigns a secret variable or assigns from a secret variable
        let re_env = Regex::new(&format!(
            r#"(?i)\benv\b(?:\s+-[A-Za-z0-9_-]+)*\s+.*?(?:\b[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*\s*=|=[^ \t\n\r]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union}))"#
        ))?;

        // ARGV-DOCKER: docker/podman run ... -e NAME=$SECRET or -e SECRET="literal" or --env ...
        let re_docker = Regex::new(&format!(
            r#"(?i)\b(?:docker|podman)\s+(?:container\s+)?run\b.*?(?:-e|--env(?:=|\s+))\s*['"]?(?:[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*\s*=|[^=\s'"]+=\s*['"]?[^ \t\n\r]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union}))"#
        ))?;

        // ARGV-INLINE: python3 -c "... $SECRET ..." or sh -c "... $SECRET ..."
        let re_inline = Regex::new(&format!(
            r#"(?i)\b(?:python[0-9.]*|bash|sh|zsh|node|ruby|perl)\s+(?:-[A-Za-z0-9_-]*c|-c|-e)\s+(?:"[^"]*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*[}}]?|'[^']*?\$[{{]?[A-Za-z0-9_]*(?:{secret_union})[A-Za-z0-9_]*[}}]?)"#
        ))?;

        // INJECT-XARGS: xargs -I {} sh -c '... {} ...'
        let re_xargs = Regex::new(
            r#"(?i)\bxargs\b.*?-[iI]\s*(\{\}|[A-Za-z0-9_%@]+)\s+.*?\b(?:sh|bash|zsh)\s+-c\s+(.*)"#,
        )?;

        // INJECT-PIPE: curl/wget piped to sh/bash/zsh
        let re_pipe = Regex::new(
            r#"(?i)\b(?:curl|wget)\b[^|;\n\r]*\|\s*(?:sudo\s+)?(?:\/bin\/|\/usr\/bin\/)?(?:sh|bash|zsh)\b"#,
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

        // 1. INJECT-PIPE: Check first (fast and specific)
        if self.re_pipe.is_match(line) {
            return Some(ShellRuleId::InjectPipe);
        }

        // 2. INJECT-XARGS
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

        // 3. ARGV-INLINE
        if self.re_inline.is_match(line) {
            return Some(ShellRuleId::ArgvInline);
        }

        // 4. ARGV-DOCKER
        if self.re_docker.is_match(line) {
            return Some(ShellRuleId::ArgvDocker);
        }

        // 5. ARGV-ENV
        // Ensure shebang lines like `#!/usr/bin/env bash` are never matched
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

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            if let Some(lines) = added_lines {
                if !lines.contains(&line_num) {
                    continue;
                }
            }

            if let Some(rule) = scanner.check_line(line) {
                // Line-level PR body override or inline exemption
                let line_subject = format!("{file}:{line_num}");
                if let Some(record) =
                    ctx.find_override(GATE, tokens::SECRETS_ARGV_OK, &line_subject)
                {
                    out.overrides.push(record);
                    continue;
                }

                if line_has_secrets_argv_ok(line) {
                    out.inline_exemptions += 1;
                    out.overrides.push(OverrideRecord {
                        gate: GATE.to_string(),
                        subject: line_subject,
                        directive: "secrets-argv-ok".to_string(),
                        reason: "inline exemption marker".to_string(),
                        source: OverrideSource::Inline {
                            file: file.clone(),
                            line: line_num,
                        },
                        hidden: true,
                    });
                    continue;
                }

                // Security Invariant: NEVER include matched line content or secret token in message
                out.push(
                    settings.severity(),
                    rule.title(),
                    Some(file),
                    Some(line_num),
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

        // Negative controls
        assert_eq!(
            scanner.check_line("python3 -c \"import sys; print(sys.version)\""),
            None
        );
        assert_eq!(scanner.check_line("sh -c \"echo hello\""), None);
        assert_eq!(scanner.check_line("sh -c \"echo $USER\""), None);
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
        ] {
            assert!(!rule.message().contains("$SECRET"));
            assert!(!rule.message().contains("ghp_"));
            assert!(!rule.remediation().contains("$SECRET"));
        }
    }
}
