//! Universal fail-closed execution wrapper for external verification tools.
//!
//! Enforces:
//! - Direct subprocess execution without shell pipe fragility
//! - Bounded runtime: exceeding `timeout_seconds` triggers exit code 2 (bail)
//! - Missing tool binary in PATH triggers exit code 2 (bail)
//! - Negative-control canary that must produce a declared diagnostic
//! - `forbid_output` patterns that must not appear in stdout or stderr
//! - `zero_items_pattern` and zero-count guard (failure unless `allow_zero = true`)
//! - `count_pattern` + `min_count` ratchet read from BASE ref
//! - Untrusted PR text guard: commands cannot be modified in PR diff without runner authorization

use crate::config::{DisciplineConfig, GateSettings};
use crate::guards::presets;
use crate::guards::{Context, GateOutcome};
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub const GATE: &str = "command";

#[derive(Debug, Clone)]
pub struct CommandRunResult {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Splits a command line into executable and arguments, honoring single and double quotes.
pub fn split_command_line(cmd: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '\\' if in_double => {
                if let Some(&next_c) = chars.peek() {
                    if next_c == '"' || next_c == '\\' || next_c == '$' || next_c == '`' {
                        current.push(next_c);
                        chars.next();
                    } else {
                        current.push('\\');
                    }
                } else {
                    current.push('\\');
                }
            }
            '\\' if !in_single => {
                if let Some(next_c) = chars.next() {
                    current.push(next_c);
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(current);
                    current = String::new();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if in_single || in_double {
        bail!("unclosed quote in command string: `{cmd}`");
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Executes a command directly without shell pipes, enforcing a strict timeout and
/// reading stdout/stderr concurrently to avoid OS pipe deadlocks.
///
/// Missing binary or timeout causes `bail!` (propagating to exit code 2).
pub fn run_command_bounded(
    name: &str,
    cmd_str: &str,
    timeout_secs: u64,
    repo_root: &Path,
) -> Result<CommandRunResult> {
    let tokens = split_command_line(cmd_str)
        .with_context(|| format!("invalid command line for `{name}`"))?;
    if tokens.is_empty() {
        bail!("empty command string for `{name}`");
    }
    let program = &tokens[0];
    let args = &tokens[1..];

    let mut child = match Command::new(program)
        .args(args)
        .current_dir(repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("tool `{program}` not found in PATH for command `{name}`");
        }
        Err(e) => {
            bail!("failed to spawn tool `{program}` for command `{name}`: {e}");
        }
    };

    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let timeout = Duration::from_secs(timeout_secs);
    let start = Instant::now();

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    bail!("command `{name}` timed out after {timeout_secs}s");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                bail!("failed waiting for child `{program}` for command `{name}`: {e}");
            }
        }
    };

    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    Ok(CommandRunResult {
        status,
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
    })
}

/// Checks whether an untrusted PR diff modified command gate definitions without
/// runner environment authorization.
fn check_untrusted_command_tampering(ctx: &Context) -> Result<Option<String>> {
    let base_src = match ctx.git.base_content(ctx.config_path)? {
        None if ctx.config_path != "discipline.toml" => ctx.git.base_content("discipline.toml")?,
        other => other,
    };
    let Some(base_src) = base_src else {
        return Ok(None);
    };
    let base_cfg = match DisciplineConfig::from_toml_str(&base_src) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    let head_cmd = &ctx.config.gates.command;
    let base_cmd = &base_cfg.gates.command;

    let mut modified = false;
    if head_cmd.command != base_cmd.command || head_cmd.preset != base_cmd.preset {
        modified = true;
    }
    if head_cmd.commands.len() != base_cmd.commands.len() {
        modified = true;
    } else {
        for (h, b) in head_cmd.commands.iter().zip(base_cmd.commands.iter()) {
            if h.name != b.name
                || h.command != b.command
                || h.preset != b.preset
                || h.canary_command != b.canary_command
            {
                modified = true;
                break;
            }
        }
    }

    if modified {
        let env_authorized = std::env::var("DISCIPLINE_COMMAND").is_ok()
            || std::env::var("DISCIPLINE_ALLOW_COMMAND_CHANGE").is_ok();
        if !env_authorized {
            return Ok(Some(
                "PR diff modifies command or canary definitions without runner environment authorization; commands cannot be introduced or altered by untrusted PR text"
                    .to_string(),
            ));
        }
    }

    Ok(None)
}

/// Retrieves the base min_count ratchet floor for a named command.
fn get_base_min_count(ctx: &Context, name: &str) -> Option<u64> {
    let base_src = match ctx.git.base_content(ctx.config_path).ok()? {
        None if ctx.config_path != "discipline.toml" => {
            ctx.git.base_content("discipline.toml").ok()?
        }
        other => other,
    }?;
    let base_cfg = DisciplineConfig::from_toml_str(&base_src).ok()?;
    if name == "command" || name == "default" {
        base_cfg.gates.command.min_count
    } else {
        base_cfg
            .gates
            .command
            .commands
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.min_count)
            .or(base_cfg.gates.command.min_count)
    }
}

struct ResolvedCommand {
    name: String,
    command: String,
    timeout_seconds: u64,
    count_pattern: Option<String>,
    min_count: Option<u64>,
    forbid_output: Vec<String>,
    zero_items_pattern: Option<String>,
    allow_zero: bool,
    canary_command: Option<String>,
    canary_expected_diagnostic: Option<String>,
    policy_files: &'static [&'static str],
}

/// Evaluates the `command` verification gate.
pub fn evaluate_command(ctx: &Context) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::new(GATE);
    let gate = &ctx.config.gates.command;

    if !gate.enabled() {
        outcome.enabled = false;
        return Ok(outcome);
    }

    // Security check: Untrusted PR text guard
    if let Some(err_msg) = check_untrusted_command_tampering(ctx)? {
        outcome.push(
            gate.severity(),
            "Untrusted Command Modification",
            Some(ctx.config_path),
            None,
            err_msg,
            "Configure commands in the merge base ref discipline.toml or provide trusted overrides via runner environment (DISCIPLINE_COMMAND).",
        );
        return Ok(outcome);
    }

    // Collect resolved commands (from top-level command/preset and commands list)
    let mut resolved = Vec::new();

    let runner_default = std::env::var("DISCIPLINE_COMMAND").ok();
    let preset_def = match gate.preset.as_deref() {
        Some(name) => match presets::resolve_preset(name) {
            Some(def) => Some(def),
            None => bail!("unknown command preset `{name}` in `[gates.command]`"),
        },
        None => None,
    };

    let primary_cmd = runner_default
        .or_else(|| gate.command.clone())
        .or_else(|| preset_def.map(|d| d.default_command.to_string()));

    if let Some(cmd) = primary_cmd {
        let mut forbid = gate.forbid_output.clone();
        if let Some(def) = preset_def {
            for p in def.forbid_output {
                if !forbid.iter().any(|existing| existing == p) {
                    forbid.push(p.to_string());
                }
            }
        }
        let timeout = gate
            .timeout_seconds
            .or_else(|| preset_def.map(|d| d.default_timeout_seconds))
            .unwrap_or(60);
        let zero_pattern = gate
            .zero_items_pattern
            .clone()
            .or_else(|| preset_def.and_then(|d| d.zero_items_pattern.map(|s| s.to_string())));
        let canary_cmd = gate
            .canary_command
            .clone()
            .or_else(|| preset_def.and_then(|d| d.canary_command.map(|s| s.to_string())));
        let canary_diag = gate.canary_expected_diagnostic.clone().or_else(|| {
            preset_def.and_then(|d| d.canary_expected_diagnostic.map(|s| s.to_string()))
        });
        let policy_files = preset_def.map(|d| d.policy_files).unwrap_or(&[]);

        resolved.push(ResolvedCommand {
            name: gate.preset.clone().unwrap_or_else(|| "default".to_string()),
            command: cmd,
            timeout_seconds: timeout,
            count_pattern: gate.count_pattern.clone(),
            min_count: gate.min_count,
            forbid_output: forbid,
            zero_items_pattern: zero_pattern,
            allow_zero: gate.allow_zero,
            canary_command: canary_cmd,
            canary_expected_diagnostic: canary_diag,
            policy_files,
        });
    }

    for entry in &gate.commands {
        let preset_def = match entry.preset.as_deref() {
            Some(name) => match presets::resolve_preset(name) {
                Some(def) => Some(def),
                None => bail!(
                    "unknown command preset `{name}` in command entry `{}`",
                    entry.name
                ),
            },
            None => None,
        };

        let env_key = format!(
            "DISCIPLINE_COMMAND_{}",
            entry.name.replace('-', "_").to_uppercase()
        );
        let effective_cmd = std::env::var(&env_key)
            .ok()
            .or_else(|| entry.command.clone())
            .or_else(|| preset_def.map(|d| d.default_command.to_string()));

        let effective_cmd = match effective_cmd {
            Some(c) => c,
            None => {
                bail!(
                    "command entry `{}` must specify `command` or a valid `preset`",
                    entry.name
                );
            }
        };

        let mut forbid = gate.forbid_output.clone();
        if let Some(def) = preset_def {
            for p in def.forbid_output {
                if !forbid.iter().any(|e| e == p) {
                    forbid.push(p.to_string());
                }
            }
        }
        for p in &entry.forbid_output {
            if !forbid.iter().any(|e| e == p) {
                forbid.push(p.clone());
            }
        }

        let timeout = entry
            .timeout_seconds
            .or_else(|| preset_def.map(|d| d.default_timeout_seconds))
            .or(gate.timeout_seconds)
            .unwrap_or(60);

        let zero_pattern = entry
            .zero_items_pattern
            .clone()
            .or_else(|| preset_def.and_then(|d| d.zero_items_pattern.map(|s| s.to_string())))
            .or_else(|| gate.zero_items_pattern.clone());

        let canary_cmd = entry
            .canary_command
            .clone()
            .or_else(|| preset_def.and_then(|d| d.canary_command.map(|s| s.to_string())))
            .or_else(|| gate.canary_command.clone());

        let canary_diag = entry
            .canary_expected_diagnostic
            .clone()
            .or_else(|| {
                preset_def.and_then(|d| d.canary_expected_diagnostic.map(|s| s.to_string()))
            })
            .or_else(|| gate.canary_expected_diagnostic.clone());

        let policy_files = preset_def.map(|d| d.policy_files).unwrap_or(&[]);

        resolved.push(ResolvedCommand {
            name: entry.name.clone(),
            command: effective_cmd,
            timeout_seconds: timeout,
            count_pattern: entry
                .count_pattern
                .clone()
                .or_else(|| gate.count_pattern.clone()),
            min_count: entry.min_count.or(gate.min_count),
            forbid_output: forbid,
            zero_items_pattern: zero_pattern,
            allow_zero: entry.allow_zero || gate.allow_zero,
            canary_command: canary_cmd,
            canary_expected_diagnostic: canary_diag,
            policy_files,
        });
    }

    if resolved.is_empty() {
        outcome.notes.push("no commands declared".to_string());
        return Ok(outcome);
    }

    for item in resolved {
        // Check for override directive covering this command name or "default"
        let override_rec = ctx
            .find_override(GATE, tokens::ALLOW_COMMAND, &item.name)
            .or_else(|| {
                if item.name != "default" {
                    ctx.find_override(GATE, tokens::ALLOW_COMMAND, "default")
                } else {
                    None
                }
            });
        let mut command_violations = Vec::new();

        // 0. Check required policy files for stealth deletion
        for pf in item.policy_files {
            if let Ok(Some(_)) = ctx.git.base_content(pf) {
                let pf_path = ctx.git.root().join(pf);
                if !pf_path.exists() {
                    command_violations.push((
                        "Policy File Deleted",
                        format!(
                            "Command `{}` required policy file `{pf}` was deleted in this change.",
                            item.name
                        ),
                        "Restore the policy file or justify its removal.",
                    ));
                }
            }
        }

        // 1. Negative-control canary execution
        if let Some(ref canary_cmd) = item.canary_command {
            let canary_res = run_command_bounded(
                &format!("{}:canary", item.name),
                canary_cmd,
                item.timeout_seconds,
                ctx.git.root(),
            )?;
            let canary_output = format!("{}\n{}", canary_res.stdout, canary_res.stderr);
            if let Some(ref expected_diag) = item.canary_expected_diagnostic {
                let matched = if let Ok(re) = regex::Regex::new(expected_diag) {
                    re.is_match(&canary_output)
                } else {
                    canary_output.contains(expected_diag)
                };
                if !matched {
                    command_violations.push((
                        "Canary Diagnostic Missing",
                        format!(
                            "Command `{}` negative-control canary did not produce expected diagnostic `{expected_diag}`.",
                            item.name
                        ),
                        "Ensure negative-control canary produces the expected diagnostic or failure message.",
                    ));
                }
                if canary_res.status.success() {
                    command_violations.push((
                        "Canary Command Succeeded",
                        format!(
                            "Command `{}` negative-control canary exited with status 0 but was expected to fail.",
                            item.name
                        ),
                        "Ensure negative-control canary fails when testing invalid or error conditions.",
                    ));
                }
            } else if canary_res.status.success() {
                command_violations.push((
                    "Canary Command Succeeded",
                    format!(
                        "Command `{}` negative-control canary exited with status 0 but was expected to fail.",
                        item.name
                    ),
                    "Ensure negative-control canary fails when testing invalid or error conditions.",
                ));
            }
        }

        // 2. Primary command execution
        let run_res = run_command_bounded(
            &item.name,
            &item.command,
            item.timeout_seconds,
            ctx.git.root(),
        )?;
        let combined_output = format!("{}\n{}", run_res.stdout, run_res.stderr);

        // Check exit status
        if !run_res.status.success() {
            let code_str = run_res
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            command_violations.push((
                "Command Exited With Error",
                format!(
                    "Command `{}` failed with exit status {code_str}.",
                    item.name
                ),
                "Fix errors reported by the command execution.",
            ));
        }

        // Check forbid_output patterns
        for pattern in &item.forbid_output {
            let matched = if let Ok(re) = regex::Regex::new(pattern) {
                re.is_match(&combined_output)
            } else {
                combined_output.contains(pattern)
            };
            if matched {
                command_violations.push((
                    "Forbidden Output Detected",
                    format!(
                        "Command `{}` produced forbidden output matching pattern `{pattern}`.",
                        item.name
                    ),
                    "Eliminate forbidden output patterns from verification command execution.",
                ));
            }
        }

        // Check count pattern & zero-items detection
        let mut zero_items = false;
        if let Some(ref zpat) = item.zero_items_pattern {
            let matched = if let Ok(re) = regex::Regex::new(zpat) {
                re.is_match(&combined_output)
            } else {
                combined_output.contains(zpat)
            };
            if matched {
                zero_items = true;
            }
        }

        let extracted_count = if let Some(ref cpat) = item.count_pattern {
            if let Ok(re) = regex::Regex::new(cpat) {
                if let Some(caps) = re.captures(&combined_output) {
                    caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if item.count_pattern.is_some() && extracted_count == Some(0) {
            zero_items = true;
        }

        if !item.allow_zero && zero_items {
            command_violations.push((
                "Zero Items Selected Or Executed",
                format!("Command `{}` selected or executed zero items.", item.name),
                "Ensure test or verification commands select and execute tests.",
            ));
        }

        // Check count ratchet against BASE ref
        let base_min = get_base_min_count(ctx, &item.name);
        let effective_floor = item.min_count.unwrap_or(0).max(base_min.unwrap_or(0));

        if effective_floor > 0 {
            match extracted_count {
                Some(cnt) if cnt < effective_floor => {
                    command_violations.push((
                        "Count Ratchet Regression",
                        format!(
                            "Command `{}` count {cnt} fell below ratchet floor {effective_floor} (enforced from base ref).",
                            item.name
                        ),
                        "Restore missing tests or justify ratchet lowering with an explicit override.",
                    ));
                }
                None if item.count_pattern.is_some() => {
                    command_violations.push((
                        "Count Pattern Did Not Match",
                        format!(
                            "Command `{}` count pattern could not extract count to verify against ratchet floor {effective_floor}.",
                            item.name
                        ),
                        "Ensure count_pattern matches command output format.",
                    ));
                }
                _ => {}
            }
        }

        // Apply findings or override
        if !command_violations.is_empty() {
            if let Some(rec) = override_rec {
                outcome.overrides.push(rec);
            } else {
                for (title, msg, rem) in command_violations {
                    outcome.push(
                        gate.severity(),
                        title,
                        Some(ctx.config_path),
                        None,
                        msg,
                        rem,
                    );
                }
            }
        }

        outcome.examined += 1;
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_command_line_basic() {
        let tokens = split_command_line("cargo test --lib -- --nocapture").unwrap();
        assert_eq!(tokens, vec!["cargo", "test", "--lib", "--", "--nocapture"]);
    }

    #[test]
    fn test_split_command_line_quotes() {
        let tokens = split_command_line("echo \"hello world\" 'single quote' foo\\ bar").unwrap();
        assert_eq!(
            tokens,
            vec!["echo", "hello world", "single quote", "foo bar"]
        );
    }

    #[test]
    fn test_split_command_line_unclosed_quote_fails() {
        assert!(split_command_line("echo \"unclosed").is_err());
        assert!(split_command_line("echo 'unclosed").is_err());
    }

    #[test]
    fn test_run_command_bounded_success() {
        let res = run_command_bounded("test_echo", "echo hello_world", 5, Path::new(".")).unwrap();
        assert!(res.status.success());
        assert!(res.stdout.contains("hello_world"));
    }

    #[test]
    fn test_run_command_bounded_missing_tool_fails_closed() {
        let err = run_command_bounded(
            "test_missing",
            "non_existent_binary_xyz_12345",
            5,
            Path::new("."),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not found in PATH"),
            "expected not found in PATH, got: {msg}"
        );
    }

    #[test]
    fn test_run_command_bounded_timeout_fails_closed() {
        let err = run_command_bounded("test_sleep", "sleep 3", 1, Path::new(".")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("timed out after 1s"),
            "expected timeout message, got: {msg}"
        );
    }

    #[test]
    fn test_preset_catalog_resolution_invariants() {
        let p = presets::resolve_preset("cargo-mutants").expect("cargo-mutants preset exists");
        assert_eq!(p.category, "mutation");
        assert_eq!(p.default_command, "cargo mutants --in-diff");
        assert_eq!(p.zero_items_pattern, Some("0 mutants tested"));
        assert!(p.forbid_output.contains(&"survived"));
        assert_eq!(p.policy_files, &[".cargo/mutants.toml"]);

        let loom = presets::resolve_preset("loom").expect("loom preset exists");
        assert_eq!(loom.category, "concurrency");
        assert_eq!(loom.zero_items_pattern, Some("running 0 tests"));

        let deny = presets::resolve_preset("cargo-deny").expect("cargo-deny preset exists");
        assert_eq!(deny.category, "supply-chain");
        assert_eq!(deny.policy_files, &["deny.toml"]);
    }
}
