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
use crate::could_not_check::{tag, Reason};
use crate::guards::presets;
use crate::guards::{Context, GateOutcome};
use crate::tokens;
use anyhow::{anyhow, bail, Context as _, Result};
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

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(tag(
                Reason::ToolMissing,
                anyhow!("tool `{program}` not found in PATH for command `{name}`"),
            ));
        }
        Err(e) => {
            return Err(tag(
                Reason::ToolMissing,
                anyhow!("failed to spawn tool `{program}` for command `{name}`: {e}"),
            ));
        }
    };

    let stdout_pipe = child.stdout.take().expect("piped stdout");
    let stderr_pipe = child.stderr.take().expect("piped stderr");

    const MAX_CAPTURE_BYTES: u64 = 25 * 1024 * 1024; // 25 MiB cap per stream

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = std::io::Read::take(stdout_pipe, MAX_CAPTURE_BYTES);
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = std::io::Read::take(stderr_pipe, MAX_CAPTURE_BYTES);
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let kill_child_group = |child: &mut std::process::Child| {
        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            // SAFETY: -pid sends SIGKILL to the process group created by process_group(0).
            // This terminates all grandchildren/descendant processes that might hold inherited
            // stdout/stderr file descriptors open, preventing pipe read deadlocks.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    };

    let timeout = Duration::from_secs(timeout_secs);
    let start = Instant::now();

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    kill_child_group(&mut child);
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    return Err(tag(
                        Reason::ToolTimeout,
                        anyhow!("command `{name}` timed out after {timeout_secs}s"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                kill_child_group(&mut child);
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
    let base_src = ctx.base_config_text()?;

    let head_cmd = &ctx.config.gates.command;
    let head_has_commands =
        head_cmd.command.is_some() || head_cmd.preset.is_some() || !head_cmd.commands.is_empty();

    let modified = if ctx.git.has_base() {
        match base_src {
            Some(src) => match DisciplineConfig::from_toml_str(&src) {
                Ok(base_cfg) => {
                    let base_cmd = &base_cfg.gates.command;
                    let mut modded = false;
                    if head_cmd.command != base_cmd.command || head_cmd.preset != base_cmd.preset {
                        modded = true;
                    }
                    if head_cmd.commands.len() != base_cmd.commands.len() {
                        modded = true;
                    } else {
                        for (h, b) in head_cmd.commands.iter().zip(base_cmd.commands.iter()) {
                            if h.name != b.name
                                || h.command != b.command
                                || h.preset != b.preset
                                || h.canary_command != b.canary_command
                            {
                                modded = true;
                                break;
                            }
                        }
                    }
                    modded
                }
                Err(_) => head_has_commands,
            },
            None => head_has_commands,
        }
    } else {
        false
    };

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
    let base_src = ctx.base_config_text().ok()??;
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
    is_base_tests: bool,
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
            &crate::findings::UNTRUSTED_COMMAND_MODIFICATION,
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

        let is_base = gate.preset.as_deref() == Some("base-tests");
        resolved.push(ResolvedCommand {
            name: gate.preset.clone().unwrap_or_else(|| "default".to_string()),
            is_base_tests: is_base,
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

        let is_base = entry.preset.as_deref() == Some("base-tests") || entry.name == "base-tests";
        resolved.push(ResolvedCommand {
            name: entry.name.clone(),
            is_base_tests: is_base,
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
        if item.is_base_tests {
            evaluate_base_tests(ctx, &item, gate, &mut outcome)?;
            continue;
        }

        let mut command_violations = Vec::new();

        // 0. Check required policy files for stealth deletion
        for pf in item.policy_files {
            if let Ok(Some(_)) = ctx.git.base_content(pf) {
                let pf_path = ctx.git.root().join(pf);
                if !pf_path.exists() {
                    command_violations.push((
                        &crate::findings::POLICY_FILE_DELETED,
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
                        &crate::findings::CANARY_DIAGNOSTIC_MISSING,
                        format!(
                            "Command `{}` negative-control canary did not produce expected diagnostic `{expected_diag}`.",
                            item.name
                        ),
                        "Ensure negative-control canary produces the expected diagnostic or failure message.",
                    ));
                }
                if canary_res.status.success() {
                    command_violations.push((
                        &crate::findings::CANARY_COMMAND_SUCCEEDED,
                        format!(
                            "Command `{}` negative-control canary exited with status 0 but was expected to fail.",
                            item.name
                        ),
                        "Ensure negative-control canary fails when testing invalid or error conditions.",
                    ));
                }
            } else if canary_res.status.success() {
                command_violations.push((
                    &crate::findings::CANARY_COMMAND_SUCCEEDED,
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
                &crate::findings::COMMAND_FAILED,
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
                    &crate::findings::FORBIDDEN_OUTPUT,
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
                &crate::findings::ZERO_ITEMS_EXECUTED,
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
                        &crate::findings::COUNT_BELOW_RATCHET,
                        format!(
                            "Command `{}` count {cnt} fell below ratchet floor {effective_floor} (enforced from base ref).",
                            item.name
                        ),
                        "Restore missing tests or justify ratchet lowering with an explicit override.",
                    ));
                }
                None if item.count_pattern.is_some() => {
                    command_violations.push((
                        &crate::findings::COUNT_PATTERN_UNMATCHED,
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

        // Apply findings or an override directive covering this command name or
        // "default". One directive lifts every finding of the command; the record names
        // the first, as the report would list it.
        if let Some((first, _, _)) = command_violations.first() {
            let lifts = *first;
            let override_rec = ctx
                .find_override(GATE, lifts, tokens::ALLOW_COMMAND, &item.name)
                .or_else(|| {
                    if item.name != "default" {
                        ctx.find_override(GATE, lifts, tokens::ALLOW_COMMAND, "default")
                    } else {
                        None
                    }
                });
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCaseReport {
    pub id: String,
    pub name: String,
    pub classname: Option<String>,
    pub status: TestStatus,
    pub failure_message: Option<String>,
}

fn unescape_xml(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub fn parse_junit_cases(xml: &str) -> Vec<TestCaseReport> {
    let re_case = regex::Regex::new(r"(?s)<testcase\b([^>]*?)(?:/>|>(.*?)</testcase>)").unwrap();
    let re_attr = regex::Regex::new(r#"([a-zA-Z0-9_:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap();
    let re_failure =
        regex::Regex::new(r"(?s)<(?:failure|error)\b([^>]*?)(?:/>|>(.*?)</(?:failure|error)>)")
            .unwrap();
    let re_skipped = regex::Regex::new(r"(?s)<skipped\b").unwrap();

    let mut cases = Vec::new();
    for cap in re_case.captures_iter(xml) {
        let attr_str = &cap[1];
        let body_opt = cap.get(2).map(|m| m.as_str());

        let mut name = String::new();
        let mut classname = None;
        let mut id_attr = None;

        for attr in re_attr.captures_iter(attr_str) {
            let k = &attr[1];
            let v = attr
                .get(2)
                .or_else(|| attr.get(3))
                .map(|m| m.as_str())
                .unwrap_or("");
            let val = unescape_xml(v);
            match k {
                "name" => name = val,
                "classname" if !val.trim().is_empty() => {
                    classname = Some(val);
                }
                "id" if !val.trim().is_empty() => {
                    id_attr = Some(val);
                }
                _ => {}
            }
        }

        if name.is_empty() {
            if let Some(id) = id_attr.as_ref() {
                name = id.clone();
            } else {
                continue;
            }
        }

        let id = match &classname {
            Some(c) => format!("{c}::{name}"),
            None => name.clone(),
        };

        let mut failure_message = None;
        let status = if let Some(body) = body_opt {
            if let Some(f_cap) = re_failure.captures(body) {
                let msg_attr = re_attr
                    .captures_iter(&f_cap[1])
                    .find(|a| &a[1] == "message")
                    .and_then(|a| a.get(2).or_else(|| a.get(3)))
                    .map(|m| unescape_xml(m.as_str()));
                let body_text = f_cap.get(2).map(|m| unescape_xml(m.as_str().trim()));
                failure_message = msg_attr.or(body_text);
                TestStatus::Failed
            } else if re_skipped.is_match(body) {
                TestStatus::Skipped
            } else {
                TestStatus::Passed
            }
        } else {
            TestStatus::Passed
        };

        cases.push(TestCaseReport {
            id,
            name,
            classname,
            status,
            failure_message,
        });
    }

    cases
}

fn evaluate_base_tests(
    ctx: &Context,
    cmd: &ResolvedCommand,
    gate: &crate::config::CommandGate,
    outcome: &mut GateOutcome,
) -> Result<()> {
    if !ctx.git.has_base() {
        outcome
            .notes
            .push("command: preset 'base-tests' skipped: no base ref".to_string());
        return Ok(());
    }

    let base_files = ctx.git.base_tracked_files()?;
    let base_test_files: Vec<String> = base_files
        .into_iter()
        .filter(|p| {
            crate::ast::functions::declared_test_path(p, &ctx.config.tests.paths)
                || crate::ast::functions::test_path(p)
        })
        .collect();

    if base_test_files.is_empty() {
        outcome
            .notes
            .push("command: preset 'base-tests': no base test files found".to_string());
        return Ok(());
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_name = format!(
        "discipline-base-tests-{}-{}-{}",
        std::process::id(),
        nanos,
        count
    );
    let temp_path = std::env::temp_dir().join(temp_name);
    std::fs::create_dir_all(&temp_path)
        .context("failed to create temporary worktree for base-tests")?;

    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0); // discipline:allow(error-swallowing): best-effort temporary directory cleanup on drop
        }
    }
    let _guard = TempDirGuard(temp_path.clone());

    // Copy all tracked files from head working tree into temp_path
    for file in ctx.git.tracked_files()? {
        let src = ctx.git.root().join(&file);
        let dst = temp_path.join(&file);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if src.is_file() {
            std::fs::copy(&src, &dst)?;
        }
    }

    // Overlay base test files onto head code
    for tf in &base_test_files {
        if let Ok(Some(content)) = ctx.git.base_content(tf) {
            let dst = temp_path.join(tf);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dst, content)?;
        }
    }

    // Remove any test files that were added newly in head (did not exist in base)
    if let Ok(changed) = ctx.git.changed_files() {
        for cf in changed {
            if cf.kind == crate::gitctx::ChangeKind::Added
                && (crate::ast::functions::declared_test_path(&cf.path, &ctx.config.tests.paths)
                    || crate::ast::functions::test_path(&cf.path))
            {
                let dst = temp_path.join(&cf.path);
                if dst.exists() {
                    let _ = std::fs::remove_file(&dst); // discipline:allow(error-swallowing): best-effort removal of newly added head test file
                }
            }
        }
    }

    // Execute the test command bounded in temp_path
    let run_res = run_command_bounded(&cmd.name, &cmd.command, cmd.timeout_seconds, &temp_path)?;
    outcome.examined += 1;

    let combined_output = format!("{}\n{}", run_res.stdout, run_res.stderr);
    let mut cases = parse_junit_cases(&combined_output);

    // Also look for test report XML files generated in temp_path if stdout did not contain JUnit XML
    if cases.is_empty() {
        if let Ok(entries) = std::fs::read_dir(temp_path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().is_some_and(|e| e == "xml") {
                    if let Ok(content) = std::fs::read_to_string(&p) {
                        let f_cases = parse_junit_cases(&content);
                        if !f_cases.is_empty() {
                            cases = f_cases;
                            break;
                        }
                    }
                }
            }
        }
    }

    let failed_cases: Vec<&TestCaseReport> = cases
        .iter()
        .filter(|c| c.status == TestStatus::Failed)
        .collect();

    if run_res.status.success() && failed_cases.is_empty() {
        outcome.notes.push(format!(
            "command: preset 'base-tests': {} base tests passed against head code",
            cases.len()
        ));
        return Ok(());
    }

    if !failed_cases.is_empty() {
        for failed in failed_cases {
            let mut excused = false;
            for subj in &[&failed.id, &failed.name] {
                if let Some(ov) = ctx.find_override(
                    "command",
                    &crate::findings::BASE_TEST_FAILED,
                    tokens::ALLOW_BEHAVIOR_CHANGE,
                    subj,
                ) {
                    outcome.overrides.push(ov);
                    excused = true;
                    break;
                }
            }
            if !excused {
                let msg = format!(
                    "Base test `{}` failed when executed against head code: {}",
                    failed.id,
                    failed
                        .failure_message
                        .as_deref()
                        .unwrap_or("assertion failure")
                );
                let rem = format!(
                    "Restore expected behavior or excuse intentional behavior change with `allow-behavior-change: {} <reason>`.",
                    failed.name
                );
                outcome.push(
                    gate.severity(),
                    &crate::findings::BASE_TEST_FAILED,
                    None,
                    None,
                    msg,
                    &rem,
                );
            }
        }
    } else {
        // Test runner failed (e.g. compilation error or exit failure without parsed JUnit failure)
        let mut excused = false;
        for subj in &["compile", "base-tests"] {
            if let Some(ov) = ctx.find_override(
                "command",
                &crate::findings::BASE_TEST_FAILED,
                tokens::ALLOW_BEHAVIOR_CHANGE,
                subj,
            ) {
                outcome.overrides.push(ov);
                excused = true;
                break;
            }
        }
        if !excused {
            let sample_err = run_res
                .stderr
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(5)
                .collect::<Vec<_>>()
                .join("\n");
            let msg = format!(
                "Base test suite failed against head code (exit code {:?}):\n{}",
                run_res.status.code(),
                if sample_err.is_empty() {
                    run_res
                        .stdout
                        .lines()
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    sample_err
                }
            );
            outcome.push(
                gate.severity(),
                &crate::findings::BASE_TEST_FAILED,
                None,
                None,
                msg,
                "Fix base test compilation error against head API or excuse with `allow-behavior-change: compile <reason>`.",
            );
        }
    }

    Ok(())
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

    #[test]
    fn test_parse_junit_cases_elements_and_statuses() {
        let xml = r#"
            <testsuites>
                <testsuite name="suite1">
                    <testcase name="test_ok" classname="pkg::mod" time="0.01" />
                    <testcase name="test_fail" classname="pkg::mod" time="0.02">
                        <failure message="assertion failed: 1 == 2">details</failure>
                    </testcase>
                    <testcase name="test_skip" classname="pkg::mod" time="0.00">
                        <skipped message="ignored" />
                    </testcase>
                </testsuite>
            </testsuites>
        "#;
        let cases = parse_junit_cases(xml);
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].id, "pkg::mod::test_ok");
        assert_eq!(cases[0].status, TestStatus::Passed);

        assert_eq!(cases[1].id, "pkg::mod::test_fail");
        assert_eq!(cases[1].status, TestStatus::Failed);
        assert_eq!(
            cases[1].failure_message.as_deref(),
            Some("assertion failed: 1 == 2")
        );

        assert_eq!(cases[2].id, "pkg::mod::test_skip");
        assert_eq!(cases[2].status, TestStatus::Skipped);
    }

    #[test]
    fn test_parse_junit_cases_escaped_entities_and_ids() {
        let xml = r#"
            <testcase id="test_custom_id" name="&quot;quoted&quot; &amp; &lt;tagged&gt;">
                <error message="&quot;failed&quot;">error body</error>
            </testcase>
        "#;
        let cases = parse_junit_cases(xml);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "\"quoted\" & <tagged>");
        assert_eq!(cases[0].status, TestStatus::Failed);
        assert_eq!(cases[0].failure_message.as_deref(), Some("\"failed\""));
    }
}
