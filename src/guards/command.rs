//! Universal fail-closed execution wrapper for external verification tools.
//!
//! Enforces:
//! - Direct subprocess execution without shell pipe fragility
//! - Bounded runtime: exceeding `timeout_seconds` triggers exit code 2 (bail)
//! - Missing tool binary in PATH triggers exit code 2 (bail)
//! - Negative-control canary that must produce a declared diagnostic
//! - `forbid_output` patterns that must not appear in stdout or stderr (regular expressions,
//!   like `zero_items_pattern` and `canary_expected_diagnostic`; one that does not compile is
//!   a configuration error, never matched as literal text)
//! - `zero_items_pattern` and zero-count guard (failure unless `allow_zero = true`)
//! - `count_pattern` + `min_count` ratchet read from BASE ref
//! - Untrusted PR text guard: commands cannot be modified in PR diff without runner authorization
//! - Bounded capture: output past [`MAX_CAPTURE_BYTES`] is exit 2 for every check that reads it
//! - `snapshot`: stdout must match a committed file, line by line, read before any command
//!   runs (exit 2 when the output cannot be compared: cut off at the capture limit, not
//!   UTF-8, empty, no file, a symbolic link, or a file past the capture limit)

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

/// The dotted name of `key` in `[gates.command]`, or in the `commands` entry named `entry`.
pub fn entry_key(entry: Option<&str>, key: &str) -> String {
    match entry {
        Some(name) => format!("gates.command.commands[{name}].{key}"),
        None => format!("gates.command.{key}"),
    }
}

/// The configuration key of a `count_pattern`: the gate's own, or that of the `commands`
/// entry named `entry`.
pub fn count_pattern_key(entry: Option<&str>) -> String {
    entry_key(entry, "count_pattern")
}

/// Compiles a value of `forbid_output`, `zero_items_pattern` or
/// `canary_expected_diagnostic`. Each is a regular expression and nothing else: a value
/// that does not compile is a configuration error naming `key`, never matched as literal
/// text. A pattern its author wrote as an expression would, as text, match nothing the
/// expression was written for, so the guard would silently never fire.
pub fn output_pattern(pattern: &str, key: &str) -> Result<regex::Regex> {
    regex::Regex::new(pattern).map_err(|e| {
        tag(
            Reason::Configuration,
            anyhow!(
                "`{key}` pattern `{pattern}` is not a valid regular expression: {e}. The value is matched as a regular expression, never as literal text: escape the characters meant literally (`\\(` for `(`)"
            ),
        )
    })
}

#[derive(Debug, Clone)]
pub struct CommandRunResult {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    /// stdout is incomplete: it went past [`MAX_CAPTURE_BYTES`] and was cut there, or a
    /// read failed.
    pub stdout_truncated: bool,
    /// stderr is incomplete, as for `stdout_truncated`.
    pub stderr_truncated: bool,
    /// stdout was not valid UTF-8; `stdout` holds a lossy conversion.
    pub stdout_lossy: bool,
}

/// How much of each output stream is kept.
pub const MAX_CAPTURE_BYTES: u64 = 25 * 1024 * 1024;

/// Reads a stream up to [`MAX_CAPTURE_BYTES`], and whether the capture is incomplete: the
/// stream went past the limit, or a read failed. Past the limit the rest is drained so a
/// child writing more is not blocked on a full pipe.
fn read_capped(pipe: impl Read) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut reader = pipe.take(MAX_CAPTURE_BYTES + 1);
    let read_failed = reader.read_to_end(&mut buf).is_err();
    let over_limit = buf.len() as u64 > MAX_CAPTURE_BYTES;
    if over_limit {
        buf.truncate(MAX_CAPTURE_BYTES as usize);
        // Draining only unblocks the child; the capture is already marked incomplete.
        let _ = std::io::copy(&mut reader.into_inner(), &mut std::io::sink()); // discipline:allow(error-swallowing): the capture is already reported incomplete
    }
    (buf, read_failed || over_limit)
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
    run_argv_bounded(name, &tokens, timeout_secs, repo_root)
}

/// [`run_command_bounded`] for a command already split into program and arguments. Each
/// element reaches the process as exactly one argument: nothing here splits it again, so
/// a configured value placed in one element cannot become several.
pub fn run_argv_bounded(
    name: &str,
    tokens: &[String],
    timeout_secs: u64,
    repo_root: &Path,
) -> Result<CommandRunResult> {
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

    let stdout_handle = std::thread::spawn(move || read_capped(stdout_pipe));
    let stderr_handle = std::thread::spawn(move || read_capped(stderr_pipe));

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

    let (stdout_bytes, stdout_truncated) = stdout_handle.join().unwrap_or_default();
    let (stderr_bytes, stderr_truncated) = stderr_handle.join().unwrap_or_default();
    let stdout_lossy = std::str::from_utf8(&stdout_bytes).is_err();

    Ok(CommandRunResult {
        status,
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        stdout_truncated,
        stderr_truncated,
        stdout_lossy,
    })
}

/// Whether the runner's environment authorises a change to the commands a gate executes:
/// `DISCIPLINE_ALLOW_COMMAND_CHANGE`, which the change under review cannot set. Every
/// gate that executes configured text shares this one switch: `command`, `test-floor`'s
/// `test_command`, and `msrv`, `miri` and `sanitizers` ([`configured_execution`]).
///
/// `DISCIPLINE_COMMAND` and `DISCIPLINE_COMMAND_<NAME>` are not this switch. Each
/// supplies one command in place of a configured one ([`runner_supplies_command`]) and
/// authorises nothing else.
pub(crate) fn runner_authorises_command_change() -> bool {
    std::env::var("DISCIPLINE_ALLOW_COMMAND_CHANGE").is_ok()
}

/// The variable that supplies the command of the `commands` entry named `name`.
pub(crate) fn entry_command_variable(name: &str) -> String {
    format!(
        "DISCIPLINE_COMMAND_{}",
        name.replace('-', "_").to_uppercase()
    )
}

/// The command the runner's environment supplies in place of the table's own `command`
/// (`entry` is `None`: `DISCIPLINE_COMMAND`) or of the `command` of the entry named
/// `entry` (`DISCIPLINE_COMMAND_<NAME>`).
fn runner_command(entry: Option<&str>) -> Option<String> {
    match entry {
        None => std::env::var("DISCIPLINE_COMMAND").ok(),
        Some(name) => {
            let env_key = entry_command_variable(name);
            std::env::var(&env_key).ok()
        }
    }
}

/// Whether the runner supplies that command. The configured value is then never run, so
/// a change to it is moot; nothing else about the table or the entry is.
pub(crate) fn runner_supplies_command(entry: Option<&str>) -> bool {
    runner_command(entry).is_some()
}

/// The keys of one gate's table whose value reaches a process invocation, each with its
/// value rendered so that two sides compare equal exactly when they run the same thing.
pub(crate) type ExecutedKeys = Vec<(&'static str, String)>;

/// The keys of `head` whose value is not the one `base` has. Both lists come from the
/// same gate, so a key one side lacks differs as well.
pub(crate) fn executed_keys_that_differ(
    head: &[(&'static str, String)],
    base: &[(&'static str, String)],
) -> Vec<&'static str> {
    head.iter()
        .filter(|(key, value)| {
            base.iter()
                .find(|(base_key, _)| base_key == key)
                .is_none_or(|(_, base_value)| base_value != value)
        })
        .map(|(key, _)| *key)
        .collect()
}

/// What the report tells the reader of a refused execution to do.
pub(crate) const EXECUTED_KEY_REMEDIATION: &str = "Configure the key in the merge base ref's discipline.toml, or set DISCIPLINE_ALLOW_COMMAND_CHANGE on the runner to accept the change. DISCIPLINE_COMMAND supplies the `command` gate's command and authorises nothing here.";

/// What a gate that executes configured text may do, decided before it runs anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfiguredExecution {
    /// Nothing the gate would execute is text the change supplies, or the runner
    /// authorised it. `enabling_note` is set when the base side does not enable the
    /// gate: that it runs at all is this change's doing, which the report should say.
    Vouched { enabling_note: Option<String> },
    /// The change supplies what the gate would execute, and the runner did not authorise
    /// it: the gate runs nothing and reports `message`.
    Refused { message: String },
}

/// The guard of every gate that executes configured text other than `command` and
/// `test-floor`, which have their own: `msrv`, `miri` and `sanitizers`.
///
/// `executed` lists the keys of `gate`'s table that reach a process invocation. Under
/// the default policy side the configuration in force is the change's own copy, so each
/// is compared with the merge base copy: a key that differs is text the change supplies.
/// A base configuration that is absent, or does not load, vouches for nothing, so the
/// head is compared with the gate's defaults. Under `--policy-from base` the copy in
/// force is the base's and the two are equal.
///
/// A run with no base at all (`--staged` before the first commit) has nothing to compare
/// with. On a developer's machine the configuration is the developer's own and it runs;
/// on a CI runner it is the change's, so it is compared with the defaults.
///
/// [`runner_authorises_command_change`] accepts every difference. `DISCIPLINE_COMMAND`
/// does not: it supplies a command of the `command` gate.
pub(crate) fn configured_execution(
    ctx: &Context,
    gate: &str,
    executed: &dyn Fn(&crate::config::Gates) -> ExecutedKeys,
) -> Result<ConfiguredExecution> {
    let head = executed(&ctx.config.gates);
    let unvouched = executed(&crate::config::Gates::default());
    let enabled = |gates: &crate::config::Gates| gates.settings(gate).is_some_and(|s| s.enabled());
    let has_base = ctx.git.has_base();
    let (keys, enabled_here) = if has_base {
        let base = ctx
            .base_config_text()?
            .and_then(|src| DisciplineConfig::from_toml_str(&src).ok());
        let enabled_here = match &base {
            Some(base) if enabled(&base.gates) => None,
            Some(_) => Some(format!(
                "`[gates.{gate}]` is disabled on the base side and this change enables it"
            )),
            None => Some(format!(
                "the base side has no configuration that loads and this change enables `[gates.{gate}]`"
            )),
        };
        let base_keys = base.map_or(unvouched, |base| executed(&base.gates));
        (executed_keys_that_differ(&head, &base_keys), enabled_here)
    } else if crate::gitctx::is_ci_environment() {
        (executed_keys_that_differ(&head, &unvouched), None)
    } else {
        (Vec::new(), None)
    };
    let named = keys
        .iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if keys.is_empty() {
        let enabling_note = enabled_here.map(|said| {
            format!("{said}: what ran is what the base side vouches for (the values it configures, else the gate's built-in command)")
        });
        return Ok(ConfiguredExecution::Vouched { enabling_note });
    }
    if runner_authorises_command_change() {
        let enabling_note = enabled_here
            .map(|said| format!("{said}: the runner authorised what the change set in {named}"));
        return Ok(ConfiguredExecution::Vouched { enabling_note });
    }
    let message = if has_base {
        format!(
            "The change adds or alters {named} in `[gates.{gate}]`, which the gate executes, without runner environment authorization; text a gate executes cannot be introduced or altered by the change it judges, so nothing was run."
        )
    } else {
        format!(
            "This CI run has no base ref to compare {named} in `[gates.{gate}]` with, and no runner environment authorization; text a gate executes cannot be introduced by the change it judges, so nothing was run."
        )
    };
    Ok(ConfiguredExecution::Refused { message })
}

/// [`configured_execution`] for the gate `out` reports for. `None` when the execution
/// is refused: `kind` is reported at the configuration file and the gate must return
/// without running anything. Otherwise the notes to record once the gate has run
/// something.
pub(crate) fn vouched_execution(
    ctx: &Context,
    out: &mut GateOutcome,
    severity: crate::config::Severity,
    kind: &crate::findings::FindingKind,
    executed: &dyn Fn(&crate::config::Gates) -> ExecutedKeys,
) -> Result<Option<Vec<String>>> {
    Ok(match configured_execution(ctx, out.gate, executed)? {
        ConfiguredExecution::Vouched { enabling_note } => Some(enabling_note.into_iter().collect()),
        ConfiguredExecution::Refused { message } => {
            out.examined = 0;
            out.push(
                severity,
                kind,
                Some(ctx.config_path),
                None,
                message,
                EXECUTED_KEY_REMEDIATION,
            );
            None
        }
    })
}

/// Whether two `[gates.command]` tables differ in anything the gate executes.
///
/// A value reaches a process invocation through these keys and no others:
/// - on the table: `command`, `preset` (its default command and default canary) and
///   `canary_command` (run for the table's own command, and inherited by every entry
///   that declares neither a canary nor a preset that has one);
/// - on a `[[gates.command.commands]]` entry: `command`, `preset`, `canary_command`, and
///   `name` (it selects the `DISCIPLINE_COMMAND_<NAME>` override and the `base-tests`
///   mode).
///
/// Entries are compared by position, so an inserted, removed or reordered entry differs.
/// Keys that are only matched against output (`forbid_output`, `zero_items_pattern`,
/// `canary_expected_diagnostic`, `snapshot`, `count_pattern`, `min_count`) execute
/// nothing and are `config-integrity`'s to judge.
pub(crate) fn executed_definitions_differ(
    head: &crate::config::CommandGate,
    base: &crate::config::CommandGate,
) -> bool {
    executed_definitions_differ_beyond(head, base, &|_| false)
}

/// [`executed_definitions_differ`], leaving out each `command` the runner supplies:
/// `supplied(None)` for the table's, `supplied(Some(name))` for an entry's. A supplied
/// command replaces the configured one, so the two sides run the same thing whatever
/// they wrote. A preset, a canary, an entry's name, and the number and order of entries
/// are compared regardless.
pub(crate) fn executed_definitions_differ_beyond(
    head: &crate::config::CommandGate,
    base: &crate::config::CommandGate,
    supplied: &dyn Fn(Option<&str>) -> bool,
) -> bool {
    (head.command != base.command && !supplied(None))
        || head.preset != base.preset
        || head.canary_command != base.canary_command
        || head.commands.len() != base.commands.len()
        || head.commands.iter().zip(&base.commands).any(|(h, b)| {
            h.name != b.name
                || (h.command != b.command && !supplied(Some(&h.name)))
                || h.preset != b.preset
                || h.canary_command != b.canary_command
        })
}

/// Whether the commands `gate` would run are ones nothing vouches for: the change under
/// review altered them (compared with the merge base copy), or wrote them where the base
/// has no configuration, or one that does not load.
///
/// A run with no base at all (`--staged` before the first commit) has nothing to compare
/// with. On a developer's machine that is the developer's own configuration and it runs.
/// On a CI runner it is the change's, so it is treated as a base with no configuration.
pub(crate) fn commands_supplied_by_change(
    ctx: &Context,
    supplied: &dyn Fn(Option<&str>) -> bool,
) -> Result<bool> {
    let head_cmd = &ctx.config.gates.command;
    // With no base-side table to vouch for it, every command the head declares is the
    // change's, except a table `command` the runner replaces.
    let unvouched = || {
        executed_definitions_differ_beyond(
            head_cmd,
            &crate::config::CommandGate {
                canary_command: head_cmd.canary_command.clone(),
                ..Default::default()
            },
            supplied,
        )
    };
    if !ctx.git.has_base() {
        return Ok(crate::gitctx::is_ci_environment() && unvouched());
    }
    Ok(match ctx.base_config_text()? {
        Some(src) => match DisciplineConfig::from_toml_str(&src) {
            Ok(base_cfg) => {
                executed_definitions_differ_beyond(head_cmd, &base_cfg.gates.command, supplied)
            }
            Err(_) => unvouched(),
        },
        None => unvouched(),
    })
}

/// Checks whether an untrusted PR diff modified command gate definitions without
/// runner environment authorization.
fn check_untrusted_command_tampering(ctx: &Context) -> Result<Option<String>> {
    if runner_authorises_command_change() {
        return Ok(None);
    }
    if commands_supplied_by_change(ctx, &runner_supplies_command)? {
        let why = if ctx.git.has_base() {
            "PR diff modifies command or canary definitions without runner environment authorization; commands cannot be introduced or altered by untrusted PR text"
        } else {
            "this CI run has no base ref to compare the command or canary definitions with, and no runner environment authorization; commands cannot be introduced by the change they judge"
        };
        return Ok(Some(why.to_string()));
    }
    Ok(None)
}

/// Whether the base side has a configuration that loads and switches this gate off.
fn base_disables_gate(ctx: &Context) -> Result<bool> {
    Ok(ctx
        .base_config_text()?
        .and_then(|src| DisciplineConfig::from_toml_str(&src).ok())
        .is_some_and(|base| !base.gates.command.enabled()))
}

/// Whether a base-side configuration exists and does not load with this binary.
fn base_config_does_not_load(ctx: &Context) -> Result<bool> {
    Ok(ctx
        .base_config_text()?
        .is_some_and(|src| DisciplineConfig::from_toml_str(&src).is_err()))
}

/// Retrieves the base min_count ratchet floor for a named command. A base configuration
/// that does not load has no floor to give; `evaluate_command` notes it.
fn get_base_min_count(ctx: &Context, name: &str) -> Result<Option<u64>> {
    let Some(base_src) = ctx.base_config_text()? else {
        return Ok(None);
    };
    let Ok(base_cfg) = DisciplineConfig::from_toml_str(&base_src) else {
        return Ok(None);
    };
    Ok(if name == "command" || name == "default" {
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
    })
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
    policy_files: Vec<String>,
    snapshot: Option<Snapshot>,
    /// What `snapshot` held before any command of this run started.
    committed_snapshot: Option<CommittedSnapshot>,
}

/// A committed file the command's stdout must match.
struct Snapshot {
    path: String,
    ignore: Vec<regex::Regex>,
}

/// What the head side holds at a snapshot's path, read before any command runs so that
/// no command of the run can write the file it is compared with.
enum CommittedSnapshot {
    Text(String),
    Missing,
    /// Present, and not a file the output can be compared with; the sentence says why.
    Unusable(String),
}

/// Reads the snapshot at `path` from the head side (the index under `--staged`, else the
/// working tree), the side every gate reads the change from. A symbolic link is refused:
/// it would compare the output with whatever the link names. A file larger than
/// [`MAX_CAPTURE_BYTES`] is refused unread: no captured output can equal it.
fn read_committed_snapshot(ctx: &Context, path: &str) -> Result<CommittedSnapshot> {
    let unusable = |why: String| Ok(CommittedSnapshot::Unusable(why));
    let on_disk = std::fs::symlink_metadata(ctx.git.root().join(path)).ok();
    if ctx.git.is_symlink(path)? || on_disk.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
        return unusable(format!(
            "snapshot `{path}` is a symbolic link; commit the command's output as a regular file"
        ));
    }
    let too_large = |len: u64| {
        format!(
            "snapshot `{path}` is {len} bytes, past the {MAX_CAPTURE_BYTES}-byte capture limit, so no captured output can match it"
        )
    };
    if !ctx.staged {
        if let Some(len) = on_disk
            .map(|m| m.len())
            .filter(|len| *len > MAX_CAPTURE_BYTES)
        {
            return unusable(too_large(len));
        }
    }
    let bytes = match ctx.git.head_bytes(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(CommittedSnapshot::Missing),
        Err(e) => return unusable(format!("snapshot `{path}` could not be read: {e:#}")),
    };
    if bytes.len() as u64 > MAX_CAPTURE_BYTES {
        return unusable(too_large(bytes.len() as u64));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(CommittedSnapshot::Text(text)),
        Err(_) => unusable(format!("snapshot `{path}` is not valid UTF-8")),
    }
}

/// [`read_committed_snapshot`] for a command's snapshot, when it has one.
fn committed_snapshot(
    ctx: &Context,
    snapshot: Option<&Snapshot>,
) -> Result<Option<CommittedSnapshot>> {
    snapshot
        .map(|s| read_committed_snapshot(ctx, &s.path))
        .transpose()
}

/// The error (exit 2, reason `gate`) of a check that reads a command's output when the
/// capture of that output is incomplete. A check answered from the captured part would
/// pass whenever the line it looks for was printed after the limit, and a command can
/// always print more.
fn incomplete_capture(name: &str, what: &str) -> anyhow::Error {
    tag(
        Reason::Gate,
        anyhow!(
            "command `{name}`: output went past the {MAX_CAPTURE_BYTES}-byte capture limit or could not be read, so {what} cannot be checked against all of it; make the command print less (a summary, not a log)"
        ),
    )
}

/// A `base-tests` command runs the base branch's tests and never reaches the snapshot
/// comparison, so a snapshot on it would be configured and never checked: a
/// configuration error (exit 2) rather than a silent skip.
fn refuse_snapshot_on_base_tests(
    owner: &str,
    is_base_tests: bool,
    snapshot: Option<&Snapshot>,
) -> Result<()> {
    match snapshot {
        Some(snap) if is_base_tests => Err(tag(
            Reason::Configuration,
            anyhow!(
                "{owner} runs the base tests, whose output is not compared: remove `snapshot = \"{}\"` or move it to its own command",
                snap.path
            ),
        )),
        _ => Ok(()),
    }
}

/// Resolves the snapshot of one command table: its own `snapshot` key, else the preset's,
/// with the preset's ignore patterns and the table's own. Configuration errors are exit 2.
fn resolve_snapshot(
    owner: &str,
    snapshot: Option<&String>,
    snapshot_ignore: &[String],
    preset_def: Option<&presets::PresetDefinition>,
) -> Result<Option<Snapshot>> {
    let config_error = |msg: String| tag(Reason::Configuration, anyhow!(msg));
    let path = snapshot
        .cloned()
        .or_else(|| preset_def.and_then(|d| d.snapshot.map(str::to_string)));
    let Some(path) = path else {
        if !snapshot_ignore.is_empty() {
            return Err(config_error(format!(
                "{owner} sets `snapshot_ignore` without `snapshot`: there is no snapshot to compare"
            )));
        }
        return Ok(None);
    };
    let as_path = Path::new(&path);
    let escapes = as_path.components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if path.trim().is_empty() || escapes {
        return Err(config_error(format!(
            "{owner} `snapshot = \"{path}\"` must be a path relative to the repository root, without `..`"
        )));
    }
    let preset_ignore = preset_def.map(|d| d.snapshot_ignore).unwrap_or(&[]);
    let mut ignore = Vec::new();
    for p in preset_ignore
        .iter()
        .copied()
        .chain(snapshot_ignore.iter().map(String::as_str))
    {
        let re = regex::Regex::new(p).map_err(|e| {
            config_error(format!(
                "{owner} `snapshot_ignore` pattern `{p}` is not a valid regex: {e}"
            ))
        })?;
        ignore.push(re);
    }
    Ok(Some(Snapshot { path, ignore }))
}

/// The policy files of a command: the preset's, with the preset's own snapshot replaced by
/// the snapshot the command actually compares against.
fn policy_files_for(
    preset_def: Option<&presets::PresetDefinition>,
    snapshot: Option<&Snapshot>,
) -> Vec<String> {
    let mut files: Vec<String> = preset_def
        .map(|d| d.policy_files)
        .unwrap_or(&[])
        .iter()
        .filter(|f| preset_def.and_then(|d| d.snapshot) != Some(**f))
        .map(|f| f.to_string())
        .collect();
    if let Some(s) = snapshot {
        if !files.contains(&s.path) {
            files.push(s.path.clone());
        }
    }
    files
}

/// How a command's output differs from its snapshot. Line numbers are 1-based in each text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDiff {
    /// Lines in the output with no counterpart in the snapshot.
    pub only_in_output: usize,
    /// Lines in the snapshot with no counterpart in the output.
    pub only_in_snapshot: usize,
    /// The first compared line that differs: (snapshot line, output line). `None` on a side
    /// that ran out of lines.
    pub first_difference: (Option<usize>, Option<usize>),
}

/// The lines of `text` kept for comparison, with their 1-based line numbers. A trailing
/// `\r` is dropped from every line, so a CRLF checkout compares equal to LF output, and a
/// final newline is not significant.
pub fn snapshot_lines<'t>(text: &'t str, ignore: &[regex::Regex]) -> Vec<(usize, &'t str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.strip_suffix('\r').unwrap_or(l)))
        .filter(|(_, l)| !ignore.iter().any(|re| re.is_match(l)))
        .collect()
}

/// Compares a snapshot with a command's output, both already reduced by
/// [`snapshot_lines`]. `None` when they match.
pub fn compare_snapshot(
    snapshot: &[(usize, &str)],
    output: &[(usize, &str)],
) -> Option<SnapshotDiff> {
    let first = snapshot
        .iter()
        .map(Some)
        .chain(std::iter::repeat(None))
        .zip(output.iter().map(Some).chain(std::iter::repeat(None)))
        .take(snapshot.len().max(output.len()))
        .find(|(s, o)| s.map(|x| x.1) != o.map(|x| x.1))?;
    let mut counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for (_, l) in snapshot {
        *counts.entry(l).or_default() += 1;
    }
    for (_, l) in output {
        *counts.entry(l).or_default() -= 1;
    }
    let only_in_snapshot = counts
        .values()
        .filter(|c| **c > 0)
        .map(|c| *c as usize)
        .sum();
    let only_in_output = counts
        .values()
        .filter(|c| **c < 0)
        .map(|c| (-*c) as usize)
        .sum();
    Some(SnapshotDiff {
        only_in_output,
        only_in_snapshot,
        first_difference: (first.0.map(|x| x.0), first.1.map(|x| x.0)),
    })
}

/// A finding of one command, before the override check.
struct Violation {
    kind: &'static crate::findings::FindingKind,
    message: String,
    remediation: String,
    file: Option<String>,
    line: Option<usize>,
    /// What tells this finding from another of its kind in the same command: the policy
    /// file or the configured pattern it is about. Never command output.
    detail: Option<String>,
}

impl Violation {
    fn about(mut self, detail: &str) -> Self {
        self.detail = Some(detail.to_string());
        self
    }

    /// A finding located at the configuration file.
    fn new(
        kind: &'static crate::findings::FindingKind,
        message: String,
        remediation: &str,
    ) -> Self {
        Self {
            kind,
            message,
            remediation: remediation.to_string(),
            file: None,
            line: None,
            detail: None,
        }
    }
}

/// Evaluates the `command` verification gate.
pub fn evaluate_command(ctx: &Context) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::new(GATE);
    let gate = &ctx.config.gates.command;

    // Security check: Untrusted PR text guard
    if let Some(err_msg) = check_untrusted_command_tampering(ctx)? {
        outcome.push(
            gate.severity(),
            &crate::findings::UNTRUSTED_COMMAND_MODIFICATION,
            Some(ctx.config_path),
            None,
            err_msg,
            "Configure commands in the merge base ref discipline.toml, or set DISCIPLINE_ALLOW_COMMAND_CHANGE on the runner to accept the change. DISCIPLINE_COMMAND (and DISCIPLINE_COMMAND_<NAME> for an entry) replaces one configured `command` and accepts no other change.",
        );
        return Ok(outcome);
    }

    // Collect resolved commands (from top-level command/preset and commands list)
    let mut resolved = Vec::new();

    let runner_default = runner_command(None);
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
        let snapshot = resolve_snapshot(
            "`[gates.command]`",
            gate.snapshot.as_ref(),
            &gate.snapshot_ignore,
            preset_def,
        )?;
        let policy_files = policy_files_for(preset_def, snapshot.as_ref());

        let is_base = gate.preset.as_deref() == Some("base-tests");
        refuse_snapshot_on_base_tests("`[gates.command]`", is_base, snapshot.as_ref())?;
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
            committed_snapshot: committed_snapshot(ctx, snapshot.as_ref())?,
            snapshot,
        });
    } else if gate.snapshot.is_some() || !gate.snapshot_ignore.is_empty() {
        // Nothing would compare it: the table runs no command of its own, and an entry
        // does not inherit the table's snapshot.
        let key = if gate.snapshot.is_some() {
            "snapshot"
        } else {
            "snapshot_ignore"
        };
        return Err(tag(
            Reason::Configuration,
            anyhow!(
                "`[gates.command]` sets `{key}` but declares no `command` or `preset` of its own, so no output is compared with it: entries do not inherit the table's snapshot; set `snapshot` on the `[[gates.command.commands]]` entry it is for"
            ),
        ));
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

        let effective_cmd = runner_command(Some(&entry.name))
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

        let snapshot = resolve_snapshot(
            &format!("command entry `{}`", entry.name),
            entry.snapshot.as_ref(),
            &entry.snapshot_ignore,
            preset_def,
        )?;
        let policy_files = policy_files_for(preset_def, snapshot.as_ref());

        let is_base = entry.preset.as_deref() == Some("base-tests") || entry.name == "base-tests";
        refuse_snapshot_on_base_tests(
            &format!("command entry `{}`", entry.name),
            is_base,
            snapshot.as_ref(),
        )?;
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
            committed_snapshot: committed_snapshot(ctx, snapshot.as_ref())?,
            snapshot,
        });
    }

    if resolved.is_empty() {
        outcome.notes.push("no commands declared".to_string());
        return Ok(outcome);
    }

    // The commands are the base's own text, so the guard above lets them run; that they
    // run at all is this change's doing, which the report should say.
    if base_disables_gate(ctx)? {
        outcome.notes.push(
            "`[gates.command]` is disabled on the base side and this change enables it: the commands the base configuration declares were run"
                .to_string(),
        );
    }

    for item in resolved {
        if item.is_base_tests {
            evaluate_base_tests(ctx, &item, gate, &mut outcome)?;
            continue;
        }

        let mut command_violations: Vec<Violation> = Vec::new();

        // 0. Check required policy files for stealth deletion
        for pf in &item.policy_files {
            if ctx.git.base_content(pf)?.is_some() {
                let pf_path = ctx.git.root().join(pf);
                if !pf_path.exists() {
                    command_violations.push(
                        Violation::new(
                            &crate::findings::POLICY_FILE_DELETED,
                            format!(
                            "Command `{}` required policy file `{pf}` was deleted in this change.",
                            item.name
                        ),
                            "Restore the policy file or justify its removal.",
                        )
                        .about(pf),
                    );
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
                let expected = output_pattern(
                    expected_diag,
                    &entry_key(None, "canary_expected_diagnostic"),
                )?;
                if !expected.is_match(&canary_output) {
                    command_violations.push(Violation::new(
                        &crate::findings::CANARY_DIAGNOSTIC_MISSING,
                        format!(
                            "Command `{}` negative-control canary did not produce expected diagnostic `{expected_diag}`.",
                            item.name
                        ),
                        "Ensure negative-control canary produces the expected diagnostic or failure message.",
                    ));
                }
                if canary_res.status.success() {
                    command_violations.push(Violation::new(
                        &crate::findings::CANARY_COMMAND_SUCCEEDED,
                        format!(
                            "Command `{}` negative-control canary exited with status 0 but was expected to fail.",
                            item.name
                        ),
                        "Ensure negative-control canary fails when testing invalid or error conditions.",
                    ));
                }
            } else if canary_res.status.success() {
                command_violations.push(Violation::new(
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
        if run_res.stdout_truncated || run_res.stderr_truncated {
            // Every pattern is matched against stdout and stderr together.
            let reads_output = if !item.forbid_output.is_empty() {
                Some("`forbid_output`")
            } else if item.zero_items_pattern.is_some() {
                Some("`zero_items_pattern`")
            } else if item.count_pattern.is_some() {
                Some("`count_pattern`")
            } else {
                None
            };
            match reads_output {
                Some(what) => return Err(incomplete_capture(&item.name, what)),
                // The snapshot comparison reads stdout alone and refuses a cut one itself.
                None => outcome.notes.push(format!(
                    "command `{}`: output went past the {MAX_CAPTURE_BYTES}-byte capture limit or could not be read; no output pattern is configured, so only the exit status was judged",
                    item.name
                )),
            }
        }

        // Check exit status
        if !run_res.status.success() {
            let code_str = run_res
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            command_violations.push(Violation::new(
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
            // Checked with the configuration before any gate runs; compiled the same way
            // here so a caller that skips that check gets the error, not a literal match.
            if output_pattern(pattern, &entry_key(None, "forbid_output"))?
                .is_match(&combined_output)
            {
                command_violations.push(
                    Violation::new(
                        &crate::findings::FORBIDDEN_OUTPUT,
                        format!(
                            "Command `{}` produced forbidden output matching pattern `{pattern}`.",
                            item.name
                        ),
                        "Eliminate forbidden output patterns from verification command execution.",
                    )
                    .about(pattern),
                );
            }
        }

        // Check count pattern & zero-items detection
        let mut zero_items = false;
        if let Some(ref zpat) = item.zero_items_pattern {
            zero_items = output_pattern(zpat, &entry_key(None, "zero_items_pattern"))?
                .is_match(&combined_output);
        }

        // Checked with the configuration before any gate runs; compiled the same way here
        // so a caller that skips that check gets the error, not a count that is never read.
        let extracted_count = match item.count_pattern {
            Some(ref cpat) => super::capture_pattern(cpat, &count_pattern_key(None))?
                .captures(&combined_output)
                .and_then(|caps| caps.get(1))
                .and_then(|m| m.as_str().parse::<u64>().ok()),
            None => None,
        };

        if item.count_pattern.is_some() && extracted_count == Some(0) {
            zero_items = true;
        }

        if !item.allow_zero && zero_items {
            command_violations.push(Violation::new(
                &crate::findings::ZERO_ITEMS_EXECUTED,
                format!("Command `{}` selected or executed zero items.", item.name),
                "Ensure test or verification commands select and execute tests.",
            ));
        }

        // Compare stdout with the committed snapshot. A failed run's output is not compared.
        if let Some(ref snap) = item.snapshot {
            if run_res.status.success() {
                if let Some(v) = check_snapshot(ctx, &item, snap, &run_res)? {
                    command_violations.push(v);
                }
            } else {
                outcome.notes.push(format!(
                    "command `{}`: snapshot `{}` not compared: the command failed",
                    item.name, snap.path
                ));
            }
        }

        // Check count ratchet against BASE ref
        let base_min = get_base_min_count(ctx, &item.name)?;
        if base_config_does_not_load(ctx)? {
            let note = "the base-side configuration does not load with this binary; the base `min_count` ratchet was not checked";
            if !outcome.notes.iter().any(|n| n == note) {
                outcome.notes.push(note.to_string());
            }
        }
        let effective_floor = item.min_count.unwrap_or(0).max(base_min.unwrap_or(0));

        if effective_floor > 0 {
            match extracted_count {
                Some(cnt) if cnt < effective_floor => {
                    command_violations.push(Violation::new(
                        &crate::findings::COUNT_BELOW_RATCHET,
                        format!(
                            "Command `{}` count {cnt} fell below ratchet floor {effective_floor} (enforced from base ref).",
                            item.name
                        ),
                        "Restore missing tests or justify ratchet lowering with an explicit override.",
                    ));
                }
                None if item.count_pattern.is_some() => {
                    command_violations.push(Violation::new(
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
        if let Some(first) = command_violations.first() {
            let lifts = first.kind;
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
                for v in command_violations {
                    outcome.push(
                        gate.severity(),
                        v.kind,
                        Some(v.file.as_deref().unwrap_or(ctx.config_path)),
                        v.line,
                        v.message,
                        &v.remediation,
                    );
                    // Located at the configuration file, which every command shares: the
                    // command's name tells its findings from another command's.
                    if v.file.is_none() {
                        outcome.anchor_last(match &v.detail {
                            Some(d) => format!("command:{}:{d}", item.name),
                            None => format!("command:{}", item.name),
                        });
                    }
                }
            }
        }

        outcome.examined += 1;
    }

    Ok(outcome)
}

/// Compares one command's stdout with its snapshot. Output that cannot be compared is exit 2;
/// a snapshot deleted in this change is left to the policy-file check.
fn check_snapshot(
    ctx: &Context,
    item: &ResolvedCommand,
    snap: &Snapshot,
    run: &CommandRunResult,
) -> Result<Option<Violation>> {
    let cannot = |msg: String| Err(tag(Reason::Gate, anyhow!(msg)));
    let name = &item.name;
    if run.stdout_truncated {
        return cannot(format!(
            "command `{name}`: stdout went past the {MAX_CAPTURE_BYTES}-byte capture limit or could not be read, so it cannot be compared with snapshot `{}`",
            snap.path
        ));
    }
    if run.stdout_lossy {
        return cannot(format!(
            "command `{name}`: stdout is not valid UTF-8, so it cannot be compared with snapshot `{}`",
            snap.path
        ));
    }
    let text = match item
        .committed_snapshot
        .as_ref()
        .unwrap_or(&CommittedSnapshot::Missing)
    {
        CommittedSnapshot::Text(text) => text,
        CommittedSnapshot::Missing => {
            if ctx.git.base_content(&snap.path)?.is_some() {
                // Reported as `policy-file-deleted`.
                return Ok(None);
            }
            return cannot(format!(
                "command `{name}`: snapshot `{}` does not exist; commit the command's output there",
                snap.path
            ));
        }
        CommittedSnapshot::Unusable(why) => return cannot(format!("command `{name}`: {why}")),
    };
    let output = snapshot_lines(&run.stdout, &snap.ignore);
    if output.is_empty() && !item.allow_zero {
        return cannot(format!(
            "command `{name}`: stdout has no lines to compare with snapshot `{}`; an empty output never matches (set `allow_zero = true` if the snapshot is meant to be empty)",
            snap.path
        ));
    }
    let snapshot = snapshot_lines(text, &snap.ignore);
    let Some(diff) = compare_snapshot(&snapshot, &output) else {
        return Ok(None);
    };
    let at = |n: Option<usize>| n.map_or("its end".to_string(), |n| format!("line {n}"));
    let order = if diff.only_in_output == 0 && diff.only_in_snapshot == 0 {
        " (the same lines in a different order)"
    } else {
        ""
    };
    // Output lines are not echoed: the command may print anything, a secret included.
    let mut v = Violation::new(
        &crate::findings::SNAPSHOT_MISMATCH,
        format!(
            "Command `{name}` output differs from snapshot `{}`: {} line(s) only in the output, {} only in the snapshot{order}; first difference at snapshot {}, output {}.",
            snap.path,
            diff.only_in_output,
            diff.only_in_snapshot,
            at(diff.first_difference.0),
            at(diff.first_difference.1),
        ),
        "",
    );
    v.remediation = format!(
        "Run `{}`, review the difference, and commit its output as `{}`.",
        item.command, snap.path
    );
    v.file = Some(snap.path.clone());
    v.line = diff.first_difference.0;
    Ok(Some(v))
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
    let re_case =
        regex::Regex::new(r"(?s)<testcase\b([^>]*?)(?:/>|>(.*?)</testcase>)").expect("valid regex");
    let re_attr = regex::Regex::new(r#"([a-zA-Z0-9_:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
        .expect("valid regex");
    let re_failure =
        regex::Regex::new(r"(?s)<(?:failure|error)\b([^>]*?)(?:/>|>(.*?)</(?:failure|error)>)")
            .unwrap();
    let re_skipped = regex::Regex::new(r"(?s)<skipped\b").expect("valid regex");

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
        if let Some(content) = ctx.git.base_content(tf)? {
            let dst = temp_path.join(tf);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dst, content)?;
        }
    }

    // Remove any test files that were added newly in head (did not exist in base)
    for cf in ctx.git.changed_files()? {
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

    // Execute the test command bounded in temp_path
    let run_res = run_command_bounded(&cmd.name, &cmd.command, cmd.timeout_seconds, &temp_path)?;
    outcome.examined += 1;
    // The failed cases are read from the output: a report cut off may have lost them.
    if run_res.stdout_truncated || run_res.stderr_truncated {
        return Err(incomplete_capture(&cmd.name, "the base tests' report"));
    }

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
                // The message quotes the failure text, which can change run to run.
                outcome.anchor_last(format!("test:{}", failed.id));
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
    fn an_output_pattern_is_a_regular_expression_or_a_configuration_error() {
        let e = output_pattern("(FAILED", "gates.command.forbid_output").unwrap_err();
        let (reason, _) = crate::could_not_check::classify(&e);
        assert_eq!(reason, Reason::Configuration);
        let shown = format!("{e:#}");
        assert!(shown.contains("`gates.command.forbid_output`"), "{shown}");
        assert!(shown.contains("never as literal text"), "{shown}");
        // The escaped form matches the text; the expression form matches what it describes.
        let escaped = output_pattern(r"\(FAILED", "k").unwrap();
        assert!(escaped.is_match("1 test (FAILED)"));
        assert!(output_pattern(r"FAILED\d", "k")
            .unwrap()
            .is_match("FAILED7"));
        assert!(!output_pattern(r"FAILED\d", "k").unwrap().is_match("FAILED"));
        assert_eq!(
            entry_key(Some("unit"), "forbid_output"),
            "gates.command.commands[unit].forbid_output"
        );
        assert_eq!(
            entry_key(None, "zero_items_pattern"),
            "gates.command.zero_items_pattern"
        );
    }

    fn gate(body: &str) -> crate::config::CommandGate {
        DisciplineConfig::from_toml_str(&format!("[gates.command]\n{body}"))
            .unwrap()
            .gates
            .command
    }

    /// Every key whose value reaches a process invocation, on the table and on an entry.
    #[test]
    fn a_difference_in_any_executed_key_is_a_modification() {
        let base = gate("command = \"true\"\n\n[[gates.command.commands]]\nname = \"unit\"\ncommand = \"true\"\n");
        assert!(!executed_definitions_differ(&base, &base));
        let entry = "\n[[gates.command.commands]]\nname = \"unit\"\ncommand = \"true\"\n";
        for (what, head) in [
            ("table command", format!("command = \"false\"\n{entry}")),
            ("table command removed", entry.to_string()),
            ("table preset", format!("command = \"true\"\npreset = \"loom\"\n{entry}")),
            (
                "table canary_command",
                format!("command = \"true\"\ncanary_command = \"false\"\n{entry}"),
            ),
            (
                "entry command",
                "command = \"true\"\n\n[[gates.command.commands]]\nname = \"unit\"\ncommand = \"false\"\n".to_string(),
            ),
            (
                "entry name",
                "command = \"true\"\n\n[[gates.command.commands]]\nname = \"base-tests\"\ncommand = \"true\"\n".to_string(),
            ),
            (
                "entry preset",
                format!("command = \"true\"\n{entry}preset = \"loom\"\n"),
            ),
            (
                "entry canary_command",
                format!("command = \"true\"\n{entry}canary_command = \"false\"\n"),
            ),
            ("entry added", format!("command = \"true\"\n{entry}{entry}")),
            ("entry removed", "command = \"true\"\n".to_string()),
        ] {
            assert!(executed_definitions_differ(&gate(&head), &base), "{what}");
            assert!(executed_definitions_differ(&base, &gate(&head)), "{what}, reversed");
        }
    }

    /// The table-level canary counts whatever it overrides: nothing, an entry's
    /// inherited canary, a preset's default, or a canary the base already had.
    #[test]
    fn a_table_canary_is_an_executed_key_whatever_it_overrides() {
        for base in [
            "command = \"true\"\n",
            "\n[[gates.command.commands]]\nname = \"unit\"\ncommand = \"true\"\n",
            "preset = \"sanitizers\"\n",
            "command = \"true\"\ncanary_command = \"false\"\n",
        ] {
            let head = format!("canary_command = \"sh -c 'exit 1'\"\n{base}");
            let head = if base.contains("canary_command") {
                base.replace("\"false\"", "\"sh -c 'exit 1'\"")
            } else {
                head
            };
            assert!(
                executed_definitions_differ(&gate(&head), &gate(base)),
                "{base}"
            );
        }
    }

    /// Keys that are matched against output, or bound a run, execute nothing.
    #[test]
    fn a_compared_or_neutral_key_is_not_an_executed_one() {
        let base = gate("command = \"true\"\n\n[[gates.command.commands]]\nname = \"unit\"\ncommand = \"true\"\n");
        let head = gate(
            "command = \"true\"\nenabled = false\nseverity = \"warning\"\nexempt_paths = [\"a\"]\n\
             timeout_seconds = 5\ncount_pattern = \"(\\\\d+)\"\nmin_count = 1\nforbid_output = [\"x\"]\n\
             zero_items_pattern = \"0 tests\"\nallow_zero = true\n\
             canary_expected_diagnostic = \"boom\"\nsnapshot = \"api.txt\"\nsnapshot_ignore = [\"^#\"]\n\n\
             [[gates.command.commands]]\nname = \"unit\"\ncommand = \"true\"\ntimeout_seconds = 5\n\
             count_pattern = \"(\\\\d+)\"\nmin_count = 1\nforbid_output = [\"x\"]\nzero_items_pattern = \"0 tests\"\n\
             allow_zero = true\ncanary_expected_diagnostic = \"boom\"\nsnapshot = \"api.txt\"\n\
             snapshot_ignore = [\"^#\"]\n",
        );
        assert!(!executed_definitions_differ(&head, &base));
    }

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
    fn a_key_differs_when_its_value_does_or_the_other_side_lacks_it() {
        let side = |pairs: &[(&'static str, &str)]| -> ExecutedKeys {
            pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
        };
        let base = side(&[("sanitizer", "address"), ("canary", "false")]);
        assert_eq!(executed_keys_that_differ(&base, &base), Vec::<&str>::new());
        assert_eq!(
            executed_keys_that_differ(
                &side(&[("sanitizer", "thread"), ("canary", "false")]),
                &base
            ),
            ["sanitizer"]
        );
        assert_eq!(
            executed_keys_that_differ(&side(&[("sanitizer", "thread"), ("canary", "true")]), &base),
            ["sanitizer", "canary"]
        );
        assert_eq!(
            executed_keys_that_differ(&base, &side(&[("sanitizer", "address")])),
            ["canary"]
        );
    }

    #[test]
    fn an_argv_element_is_one_argument_whatever_it_holds() {
        let argv: Vec<String> = ["sh", "-c", "printf '%s|' \"$@\"", "sh", "a b", "c;d"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let res = run_argv_bounded("argv", &argv, 5, Path::new(".")).unwrap();
        assert_eq!(res.stdout, "a b|c;d|");
        assert!(run_argv_bounded("argv", &[], 5, Path::new(".")).is_err());
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

    fn re(p: &str) -> regex::Regex {
        regex::Regex::new(p).expect("valid regex")
    }

    #[test]
    fn snapshot_lines_drop_cr_ignored_lines_and_keep_line_numbers() {
        let lines = snapshot_lines("# v1\r\npub fn a()\r\n\r\npub fn b()", &[re("^#")]);
        assert_eq!(lines, vec![(2, "pub fn a()"), (3, ""), (4, "pub fn b()")]);
        // Negative control: nothing ignored keeps the header.
        assert_eq!(snapshot_lines("# v1\n", &[]), vec![(1, "# v1")]);
    }

    #[test]
    fn compare_snapshot_matches_equal_lines_and_locates_the_first_difference() {
        let snap = snapshot_lines("# v1\npub fn a()\npub fn b()\n", &[re("^#")]);
        let same = snapshot_lines("# v2\r\npub fn a()\r\npub fn b()", &[re("^#")]);
        assert_eq!(compare_snapshot(&snap, &same), None);

        let changed = snapshot_lines("pub fn a()\npub fn c()\n", &[]);
        assert_eq!(
            compare_snapshot(&snap, &changed),
            Some(SnapshotDiff {
                only_in_output: 1,
                only_in_snapshot: 1,
                first_difference: (Some(3), Some(2)),
            })
        );

        let longer = snapshot_lines("pub fn a()\npub fn b()\npub fn c()\n", &[]);
        assert_eq!(
            compare_snapshot(&snap, &longer),
            Some(SnapshotDiff {
                only_in_output: 1,
                only_in_snapshot: 0,
                first_difference: (None, Some(3)),
            })
        );

        let reordered = snapshot_lines("pub fn b()\npub fn a()\n", &[]);
        let d = compare_snapshot(&snap, &reordered).expect("order is compared");
        assert_eq!((d.only_in_output, d.only_in_snapshot), (0, 0));
    }

    #[test]
    fn resolve_snapshot_refuses_bad_paths_bad_patterns_and_orphan_ignores() {
        let s = |p: &str| Some(p.to_string());
        let err = |r: Result<Option<Snapshot>>| format!("{:#}", r.err().expect("refused"));
        assert!(err(resolve_snapshot("t", s("../x").as_ref(), &[], None)).contains("relative"));
        assert!(err(resolve_snapshot("t", s("/etc/x").as_ref(), &[], None)).contains("relative"));
        assert!(err(resolve_snapshot("t", s("").as_ref(), &[], None)).contains("relative"));
        assert!(err(resolve_snapshot(
            "t",
            s("a.txt").as_ref(),
            &["(".into()],
            None
        ))
        .contains("not a valid regex"));
        assert!(err(resolve_snapshot("t", None, &["^#".into()], None)).contains("without"));

        let ok = resolve_snapshot("t", s("api/a.txt").as_ref(), &["^#".into()], None)
            .unwrap()
            .unwrap();
        assert_eq!(ok.path, "api/a.txt");
        assert_eq!(ok.ignore.len(), 1);
        assert!(resolve_snapshot("t", None, &[], None).unwrap().is_none());
    }

    #[test]
    fn cargo_public_api_preset_renders_and_compares_with_its_policy_file() {
        let preset = presets::resolve_preset("cargo-public-api").unwrap();
        assert_eq!(preset.default_command, "cargo public-api --simplified");
        assert_eq!(preset.snapshot, Some("public-api.txt"));

        let snap = resolve_snapshot("t", None, &[], Some(preset))
            .unwrap()
            .unwrap();
        assert_eq!(snap.path, "public-api.txt");
        assert_eq!(
            policy_files_for(Some(preset), Some(&snap)),
            ["public-api.txt"]
        );

        // A configured path replaces the preset's file as the protected policy file.
        let moved = resolve_snapshot("t", Some(&"api/surface.txt".to_string()), &[], Some(preset))
            .unwrap()
            .unwrap();
        assert_eq!(
            policy_files_for(Some(preset), Some(&moved)),
            ["api/surface.txt"]
        );

        // Presets without a snapshot keep their policy files and compare nothing.
        let deny = presets::resolve_preset("cargo-deny").unwrap();
        assert!(resolve_snapshot("t", None, &[], Some(deny))
            .unwrap()
            .is_none());
        assert_eq!(policy_files_for(Some(deny), None), ["deny.toml"]);
    }

    #[test]
    fn read_capped_reports_output_past_the_limit() {
        let limit = MAX_CAPTURE_BYTES as usize;
        let (buf, cut) = read_capped(std::io::repeat(b'x').take(MAX_CAPTURE_BYTES));
        assert_eq!((buf.len(), cut), (limit, false));
        let (buf, cut) = read_capped(std::io::repeat(b'x').take(MAX_CAPTURE_BYTES + 1));
        assert_eq!((buf.len(), cut), (limit, true));
    }

    #[test]
    fn run_command_bounded_flags_stdout_that_is_not_utf8() {
        let bad = run_command_bounded("t", "printf '\\377'", 5, Path::new(".")).unwrap();
        assert!(bad.stdout_lossy);
        let good = run_command_bounded("t", "printf 'ok'", 5, Path::new(".")).unwrap();
        assert!(!good.stdout_lossy && !good.stdout_truncated);
    }

    // ---- #592: what the runner's variables make moot ----

    const TABLE_TRUE: &str = "command = \"true\"\n";
    const TWO_ENTRIES: &str =
        "[[gates.command.commands]]\nname = \"unit-tests\"\ncommand = \"true\"\n\
         [[gates.command.commands]]\nname = \"lint\"\ncommand = \"true\"\n";
    const TWO_ENTRIES_FIRST_REPOINTED: &str =
        "[[gates.command.commands]]\nname = \"unit-tests\"\ncommand = \"false\"\n\
         [[gates.command.commands]]\nname = \"lint\"\ncommand = \"true\"\n";
    const TWO_ENTRIES_FIRST_CANARY: &str = "[[gates.command.commands]]\nname = \"unit-tests\"\ncommand = \"true\"\ncanary_command = \"false\"\n\
         [[gates.command.commands]]\nname = \"lint\"\ncommand = \"true\"\n";

    fn table(body: &str) -> crate::config::CommandGate {
        DisciplineConfig::from_toml_str(&format!("[gates.command]\n{body}"))
            .unwrap()
            .gates
            .command
    }

    #[test]
    fn a_supplied_table_command_makes_only_that_command_moot() {
        let table_only = |entry: Option<&str>| entry.is_none();
        let nothing = |_: Option<&str>| false;
        let base = table(TABLE_TRUE);
        let repointed = table("command = \"false\"\n");
        assert!(executed_definitions_differ_beyond(
            &repointed, &base, &nothing
        ));
        assert!(!executed_definitions_differ_beyond(
            &repointed,
            &base,
            &table_only
        ));
        // Everything else the table executes is compared whatever the runner supplies.
        for body in [
            "command = \"true\"\ncanary_command = \"false\"\n",
            "command = \"true\"\npreset = \"cargo-deny\"\n",
            "command = \"true\"\n[[gates.command.commands]]\nname = \"extra\"\ncommand = \"true\"\n",
        ] {
            assert!(
                executed_definitions_differ_beyond(&table(body), &base, &table_only),
                "{body}"
            );
        }
        // The two-argument form supplies nothing.
        assert!(executed_definitions_differ(&repointed, &base));
    }

    #[test]
    fn a_supplied_entry_command_makes_only_that_entry_command_moot() {
        let unit_only = |entry: Option<&str>| entry == Some("unit-tests");
        let lint_only = |entry: Option<&str>| entry == Some("lint");
        let base = table(TWO_ENTRIES);
        let repointed = table(TWO_ENTRIES_FIRST_REPOINTED);
        assert!(!executed_definitions_differ_beyond(
            &repointed, &base, &unit_only
        ));
        assert!(executed_definitions_differ_beyond(
            &repointed, &base, &lint_only
        ));
        assert!(executed_definitions_differ_beyond(
            &table(TWO_ENTRIES_FIRST_CANARY),
            &base,
            &unit_only
        ));
        // Neither does it cover the table's own command.
        assert!(executed_definitions_differ_beyond(
            &table(&format!("command = \"false\"\n{TWO_ENTRIES}")),
            &table(&format!("{TABLE_TRUE}{TWO_ENTRIES}")),
            &unit_only
        ));
    }

    #[test]
    fn an_entry_command_variable_is_named_after_the_entry() {
        // Spelled in two parts: a whole name in quotes would read as a variable the test
        // harness has to isolate.
        let named = |suffix: &str| format!("{}_{suffix}", "DISCIPLINE_COMMAND");
        assert_eq!(entry_command_variable("unit-tests"), named("UNIT_TESTS"));
        assert_eq!(entry_command_variable("api"), named("API"));
    }
}
