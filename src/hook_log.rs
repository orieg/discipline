//! The hook's durable record: `<git dir>/discipline/hook-observe.log`, one JSON line per
//! hook run, in observe and in enforcing mode (#937).
//!
//! A change check (`hook run`, after an edit or at the end of a turn) appends one line
//! whatever it decided, so a repository whose hooks are configured and whose log has no
//! line for a session is a repository where the hook did not run. A pre-tool call appends
//! a line when it is refused (enforcing) or would be (observe); the calls it lets through
//! are not logged, since every tool call of a session passes through it. The end-of-turn
//! checks (`turn/<code>`) keep their own lines beside these.
//!
//! The file name predates the enforcing records and is kept: readers of the observe-mode
//! log keep working, and a line written by an earlier release (no `record` field) reads as
//! an observe-mode record. `discipline hook log` prints the lines; the same fields, for
//! the change check only, are the report's `hook` object when the check runs under a hook.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The log file's name under `<git dir>/discipline/`.
pub const LOG_NAME: &str = "hook-observe.log";

/// `record` of a line this release writes. A field added keeps it; a field renamed,
/// removed or retyped raises it.
pub const RECORD_VERSION: u32 = 1;

/// How the hook was configured to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// `--observe`: never blocks, logs what it would have blocked.
    Observe,
    /// Blocks what the check reports.
    Enforce,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Observe => "observe",
            Mode::Enforce => "enforce",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "observe" => Some(Mode::Observe),
            "enforce" => Some(Mode::Enforce),
            _ => None,
        }
    }
}

/// What the hook answered the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// Nothing blocked: the agent's pass.
    Allowed,
    /// Enforcing mode refused the change, the stop or the tool call.
    Blocked,
    /// Observe mode: it would have blocked, and let the agent through.
    Observed,
    /// Enforcing mode let a stop through that this hook had already continued once, so it
    /// cannot loop; what is still wrong was said.
    LetThrough,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allowed => "allowed",
            Decision::Blocked => "blocked",
            Decision::Observed => "observed",
            Decision::LetThrough => "let-through",
        }
    }

    /// The decision of a check that exited `check_exit` (0 pass) in `mode`.
    pub fn of_check(mode: Mode, passed: bool) -> Self {
        match (passed, mode) {
            (true, _) => Decision::Allowed,
            (false, Mode::Observe) => Decision::Observed,
            (false, Mode::Enforce) => Decision::Blocked,
        }
    }
}

/// The report's `hook` object: the check ran under an agent hook. Absent from a report
/// the check wrote for anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookContext {
    /// The agent whose hook ran the check (`claude-code`, `copilot`, ...).
    pub agent: String,
    /// `edit` (after an edit) or `stop` (at the end of a turn).
    pub event: String,
    pub mode: Mode,
    /// The version of the discipline binary that ran the check.
    pub binary_version: String,
    /// What the hook answers on this report alone: `allowed` when it passes, otherwise
    /// `blocked` (enforcing) or `observed` (observe mode). An end-of-turn check the hook
    /// adds after a passing report is in the hook log, not here.
    pub decision: Decision,
}

impl HookContext {
    /// The value of [`crate::hook::HOOK_RUN_ENV`] a hook gives the check it runs. A value
    /// this does not write (an earlier release's `1`) gives no `hook` object.
    pub fn env_value(agent: &str, event: &str, mode: Mode) -> String {
        format!("agent={agent} event={event} mode={}", mode.as_str())
    }

    /// The context the hook handed this run, with the decision for a run that `passed`.
    pub fn from_env(passed: bool) -> Option<Self> {
        Self::from_value(&std::env::var(crate::hook::HOOK_RUN_ENV).ok()?, passed)
    }

    /// [`Self::from_env`] on the variable's value.
    pub fn from_value(raw: &str, passed: bool) -> Option<Self> {
        let mut agent = None;
        let mut event = None;
        let mut mode = None;
        for part in raw.split_whitespace() {
            match part.split_once('=')? {
                ("agent", v) if is_name(v) => agent = Some(v.to_string()),
                ("event", v) if is_name(v) => event = Some(v.to_string()),
                ("mode", v) => mode = Mode::parse(v),
                _ => return None,
            }
        }
        let mode = mode?;
        Some(Self {
            agent: agent?,
            event: event?,
            mode,
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
            decision: Decision::of_check(mode, passed),
        })
    }
}

/// A kebab-case word: what an agent id and an event name are. Anything else in the
/// variable is not ours, and the report carries no `hook` object for it.
fn is_name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<git dir>/discipline/hook-observe.log` of the repository holding `dir`.
pub fn path(dir: &Path) -> Result<PathBuf> {
    Ok(crate::gitctx::discover_repository(dir)?
        .path()
        .join("discipline")
        .join(LOG_NAME))
}

/// Appends `entry` as one line to the log of the repository holding `dir`.
pub fn append(dir: &Path, entry: &Value) -> Result<PathBuf> {
    use std::io::Write as _;
    let path = path(dir)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    writeln!(f, "{entry}").with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// The fields every record this release writes carries, beside the event's own.
pub fn record(
    agent: &str,
    event: &str,
    mode: Mode,
    decision: Decision,
    exit_code: i32,
) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("record".into(), RECORD_VERSION.into());
    m.insert("time".into(), crate::lease::now().into());
    m.insert("agent".into(), agent.into());
    m.insert("event".into(), event.into());
    m.insert("mode".into(), mode.as_str().into());
    m.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    m.insert("decision".into(), decision.as_str().into());
    m.insert("exit_code".into(), exit_code.into());
    m
}

/// The column names above `hook log`'s text output.
pub const HEADER: &str =
    "time                  agent        event     mode     version   decision     outcome";

/// One line of the log as `hook log` prints it. A field a line does not carry is `None`;
/// a line an earlier release wrote is an observe-mode record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub time: Option<i64>,
    pub agent: Option<String>,
    pub event: Option<String>,
    pub mode: Option<String>,
    pub version: Option<String>,
    pub decision: Option<String>,
    pub exit_code: Option<i64>,
    pub errors: Option<u64>,
    pub warnings: Option<u64>,
    pub verdict: Option<String>,
    pub codes: Vec<String>,
}

impl Entry {
    /// `None` for a line that is not a JSON object.
    pub fn parse(line: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(line).ok()?;
        let o = v.as_object()?;
        let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
        let legacy = o.get("record").is_none();
        let verdict = s("verdict");
        Some(Self {
            time: o.get("time").and_then(Value::as_i64),
            agent: s("agent"),
            event: s("event"),
            mode: s("mode").or_else(|| legacy.then(|| "observe".to_string())),
            version: s("version"),
            decision: s("decision").or_else(|| {
                // Before records, a line was written only for what would have blocked.
                legacy.then(|| match verdict.as_deref() {
                    Some("skipped") => "allowed".to_string(),
                    _ => "observed".to_string(),
                })
            }),
            exit_code: o.get("exit_code").and_then(Value::as_i64),
            errors: o.get("errors").and_then(Value::as_u64),
            warnings: o.get("warnings").and_then(Value::as_u64),
            verdict,
            codes: o
                .get("codes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect(),
        })
    }

    /// One line of `hook log`'s text output.
    pub fn render(&self) -> String {
        let dash = |o: &Option<String>| o.clone().unwrap_or_else(|| "-".into());
        let num = |o: Option<u64>| o.map_or_else(|| "-".into(), |n| n.to_string());
        let mut line = format!(
            "{}  {:<12} {:<9} {:<8} {:<9} {:<12} exit={} errors={} warnings={}",
            self.time.map_or_else(|| "-".into(), timestamp),
            dash(&self.agent),
            dash(&self.event),
            dash(&self.mode),
            dash(&self.version),
            dash(&self.decision),
            self.exit_code.map_or_else(|| "-".into(), |n| n.to_string()),
            num(self.errors),
            num(self.warnings),
        );
        if let Some(v) = &self.verdict {
            line.push_str(&format!(" verdict={v}"));
        }
        if !self.codes.is_empty() {
            line.push_str(&format!(" codes={}", self.codes.join(",")));
        }
        crate::report::text::terminal_line(&line)
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for seconds since the Unix epoch.
pub fn timestamp(secs: i64) -> String {
    let t = secs.rem_euclid(86_400);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        crate::audit_html::date(secs),
        t / 3_600,
        t % 3_600 / 60,
        t % 60
    )
}

/// The lines of the log of the repository holding `dir`, oldest first, and how many lines
/// were not records (not a JSON object). `None` when no hook has written the log.
pub fn read(dir: &Path) -> Result<Option<(Vec<Entry>, usize)>> {
    let path = path(dir)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut unread = 0;
    let entries = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let e = Entry::parse(l);
            unread += usize::from(e.is_none());
            e
        })
        .collect();
    Ok(Some((entries, unread)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_is_read_only_from_the_value_a_hook_writes() {
        let ok = HookContext::env_value("claude-code", "edit", Mode::Enforce);
        assert_eq!(ok, "agent=claude-code event=edit mode=enforce");
        for (value, want) in [
            (ok.as_str(), true),
            ("1", false),
            ("agent=claude-code event=edit", false),
            ("agent=claude-code event=edit mode=loud", false),
            ("agent=Claude event=edit mode=observe", false),
            ("agent=claude-code event=edit mode=observe extra=1", false),
        ] {
            assert_eq!(
                HookContext::from_value(value, true).is_some(),
                want,
                "{value}"
            );
        }
        let c = HookContext::from_value(&ok, false).unwrap();
        assert_eq!((c.mode, c.decision), (Mode::Enforce, Decision::Blocked));
        assert_eq!(c.binary_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn a_decision_follows_the_mode_only_when_the_check_did_not_pass() {
        assert_eq!(Decision::of_check(Mode::Enforce, true), Decision::Allowed);
        assert_eq!(Decision::of_check(Mode::Observe, true), Decision::Allowed);
        assert_eq!(Decision::of_check(Mode::Enforce, false), Decision::Blocked);
        assert_eq!(Decision::of_check(Mode::Observe, false), Decision::Observed);
    }

    #[test]
    fn a_line_an_earlier_release_wrote_reads_as_an_observe_mode_record() {
        let old = Entry::parse(
            r#"{"time":0,"agent":"codex","event":"edit","verdict":"findings","reason":null,"codes":["a/b"]}"#,
        )
        .unwrap();
        assert_eq!(old.mode.as_deref(), Some("observe"));
        assert_eq!(old.decision.as_deref(), Some("observed"));
        assert_eq!(old.version, None);
        let new = Entry::parse(
            r#"{"record":1,"time":0,"agent":"codex","event":"edit","mode":"enforce","version":"9.9.9","decision":"allowed","exit_code":0,"errors":0,"warnings":2,"codes":[]}"#,
        )
        .unwrap();
        assert_eq!(new.mode.as_deref(), Some("enforce"));
        assert_eq!(new.decision.as_deref(), Some("allowed"));
        assert_eq!(new.warnings, Some(2));
        assert_eq!(Entry::parse("not json"), None);
        assert_eq!(Entry::parse("[1]"), None);
    }

    #[test]
    fn a_timestamp_is_utc_to_the_second() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_793_318_399), "2026-10-29T23:59:59Z");
    }
}
