//! Document staleness (`doc-staleness`): a registered document must have a qualifying
//! edit within `max_age_days`, judged on every run whatever the change touches.
//!
//! Each `[[gates.doc-staleness.rules]]` entry names one `path` and its `max_age_days`.
//! - **Clock.** The age is the head commit's committer time minus the committer time of
//!   the newest qualifying commit. The wall clock is never read, so `discipline replay`
//!   reaches the same verdict on any day.
//! - **Qualifying commit.** A commit on the head commit's first-parent history whose tree
//!   changes `path` from its first parent's and still holds it (a root commit that adds it
//!   counts). With `require_added_line_match`, its diff to `path` must also add, for every
//!   pattern, a line matching it, as in `companion-edit`. The head commit is judged first, so a change that
//!   refreshes the document clears its own finding.
//! - **Lift.** `review-scheduled: <path> <YYYY-MM-DD>` in the pull request or commit body
//!   lifts that path's finding while the date is after the head commit's date (UTC) and no
//!   more than `max_age_days` after it. A date in the past, one past that cap, one that is
//!   not a calendar date, and a directive that names no registered path grant nothing and
//!   are reported (`doc-staleness/review-schedule-void`).
//! - **Fails closed (exit 2).** `max_age_days = 0`, a blank, absolute or duplicate path, a
//!   path that is not a regular file at the head commit, a qualifying commit dated after
//!   the head commit, and a shallow clone whose history ends before a qualifying commit is
//!   found. The whole history with no qualifying commit is a stale document of unknown age.

use crate::config::{DocStalenessRule, GateSettings};
use crate::could_not_check::{tag, Reason};
use crate::gitctx::{AddedLinesJudge, HeadEntry, PathHistory};
use crate::guards::companion_edit::{compile_line_patterns, missing_line_patterns, LinePattern};
use crate::guards::{exempt_filter, Context, GateOutcome, PREDATES_CONFIG_NOTE};
use crate::tokens::{self, ParsedDirective};
use anyhow::{anyhow, Result};

pub const GATE: &str = "doc-staleness";

const DAY: i64 = 86_400;

/// One rule, validated.
#[derive(Debug)]
pub struct CompiledRule {
    pub path: String,
    pub max_age_days: u32,
    /// `require_added_line_match`, compiled; empty when the rule sets none.
    pub patterns: Vec<LinePattern>,
}

impl CompiledRule {
    /// Whether a commit whose diff to the path adds `added` qualifies: every pattern
    /// matches some added line (as `companion-edit` reads the same key), or any change at
    /// all when the rule sets none.
    pub fn qualifies(&self, added: &[String]) -> bool {
        missing_line_patterns(&self.patterns, added.iter().map(String::as_str)).is_empty()
    }
}

/// Validates and compiles every rule, refusing what could never be judged: a blank,
/// absolute or non-normal path, the same path twice, `max_age_days = 0`, and a
/// `require_added_line_match` that cannot discriminate.
pub fn compile_rules(rules: &[DocStalenessRule]) -> Result<Vec<CompiledRule>> {
    let config_err = |msg: String| tag(Reason::Configuration, anyhow!(msg));
    let mut compiled: Vec<CompiledRule> = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        let key = |field: &str| format!("gates.{GATE}.rules[{i}].{field}");
        let path = rule.path.as_str();
        if path.trim().is_empty() {
            return Err(config_err(format!("`{}` is blank", key("path"))));
        }
        let normal = path == path.trim()
            && !path.starts_with('/')
            && !path.ends_with('/')
            && !path.contains('\\')
            && path
                .split('/')
                .all(|seg| !seg.is_empty() && seg != "." && seg != "..");
        if !normal {
            return Err(config_err(format!(
                "`{}` = {path:?} is not a repository-relative file path (no leading `/`, no `.` or `..` segment, no trailing `/`)",
                key("path")
            )));
        }
        if let Some(earlier) = compiled.iter().position(|c| c.path == path) {
            return Err(config_err(format!(
                "`{}` = `{path}` is already registered by rule {earlier}",
                key("path")
            )));
        }
        if rule.max_age_days == 0 {
            return Err(config_err(format!(
                "`{}` is 0: every document would be stale the moment it is committed",
                key("max_age_days")
            )));
        }
        let patterns = match &rule.require_added_line_match {
            Some(p) => compile_line_patterns(&key("require_added_line_match"), p)?,
            None => Vec::new(),
        };
        compiled.push(CompiledRule {
            path: path.to_string(),
            max_age_days: rule.max_age_days,
            patterns,
        });
    }
    Ok(compiled)
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` calendar date; `None` for anything else.
pub fn parse_day(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = &text[r];
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())
            .flatten()
    };
    let (y, m, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=month_days).contains(&d) {
        return None;
    }
    // H. Hinnant, "chrono-Compatible Low-Level Date Algorithms", `days_from_civil`
    // (http://howardhinnant.github.io/date_algorithms.html).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The UTC calendar day of a time in seconds since the epoch.
fn day_of(secs: i64) -> i64 {
    secs.div_euclid(DAY)
}

fn date_of_day(day: i64) -> String {
    crate::audit_html::date(day * DAY)
}

/// Judges the date a `review-scheduled` directive gives: the day it names when it is after
/// `head_day` and no more than `max_age_days` after it, else why it grants nothing. Text
/// that is not a date is never echoed: it is free text the change's author wrote.
pub fn judge_schedule(
    date: Option<&str>,
    head_day: i64,
    max_age_days: u32,
) -> std::result::Result<i64, String> {
    let Some(day) = date.and_then(parse_day) else {
        return Err("its date is missing or is not a `YYYY-MM-DD` calendar date".to_string());
    };
    let cap = head_day + i64::from(max_age_days);
    if day <= head_day {
        return Err(format!(
            "its date {} is not after the head commit's date {}",
            date_of_day(day),
            date_of_day(head_day)
        ));
    }
    if day > cap {
        return Err(format!(
            "its date {} is more than `max_age_days` ({max_age_days}) after the head commit's date {} (latest {})",
            date_of_day(day),
            date_of_day(head_day),
            date_of_day(cap)
        ));
    }
    Ok(day)
}

/// How old a document is at the head commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    /// Refreshed `days` (whole days, rounded down) before the head commit, within the limit.
    Fresh { days: i64 },
    /// Refreshed `days` before the head commit, past the limit.
    Stale { days: i64 },
    /// No qualifying commit in the whole history: the age is unknown.
    NeverRefreshed,
}

/// Judges the age of a document refreshed at `newest` (committer time) against the head
/// commit's time. A refresh dated after the head commit cannot bound an age: an error.
pub fn judge_age(
    head_time: i64,
    newest: Option<i64>,
    max_age_days: u32,
) -> std::result::Result<Age, String> {
    let Some(newest) = newest else {
        return Ok(Age::NeverRefreshed);
    };
    if newest > head_time {
        return Err(format!(
            "it is dated {} second(s) after the head commit",
            newest - head_time
        ));
    }
    let age = head_time - newest;
    let days = age / DAY;
    if age > i64::from(max_age_days) * DAY {
        Ok(Age::Stale { days })
    } else {
        Ok(Age::Fresh { days })
    }
}

/// The date a `review-scheduled` directive gives for `path`, the first word after the
/// subject (`Some(None)`: it names `path` and gives no date). `None` when the directive
/// does not name `path`.
fn scheduled_date<'a>(d: &'a ParsedDirective, path: &str) -> Option<Option<&'a str>> {
    tokens::text_after_subject(&d.reason, path).map(|rest| rest.split_whitespace().next())
}

fn is_schedule(d: &ParsedDirective) -> bool {
    tokens::REVIEW_SCHEDULED
        .iter()
        .any(|n| n.eq_ignore_ascii_case(&d.directive))
}

fn patterns_clause(rule: &CompiledRule) -> String {
    if rule.patterns.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = rule
        .patterns
        .iter()
        .map(|(p, _)| format!("`{p}`"))
        .collect();
    if shown.len() == 1 {
        format!(" and adds a line matching {}", shown[0])
    } else {
        format!(
            " and adds, for each of {}, a line matching it",
            shown.join(", ")
        )
    }
}

fn short(id: &str) -> String {
    id.chars().take(10).collect()
}

pub fn evaluate_doc_staleness(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.doc_staleness;
    let mut out = GateOutcome::new(GATE);
    let rules = compile_rules(&settings.rules)?;
    let exempt = exempt_filter(settings)?;
    let severity = ctx.overridable(settings.severity());
    let schedules: Vec<(usize, &ParsedDirective)> = ctx
        .directives
        .iter()
        .filter(|d| is_schedule(d))
        .enumerate()
        .collect();

    // A directive that names no registered path schedules nothing.
    for (n, d) in &schedules {
        if rules.iter().all(|r| scheduled_date(d, &r.path).is_none()) {
            out.push(
                severity,
                &crate::findings::REVIEW_SCHEDULE_VOID,
                None,
                None,
                format!(
                    "A `review-scheduled` directive ({}) names no path registered in `[[gates.{GATE}.rules]]`, so it schedules nothing.",
                    d.source
                ),
                "Write `review-scheduled: <path> <YYYY-MM-DD>` with a registered rule's `path` first.",
            );
            out.anchor_last(format!("{} #{n}", d.source));
        }
    }
    if rules.is_empty() {
        out.notes.push(format!(
            "no `[[gates.{GATE}.rules]]` entry: nothing to judge"
        ));
        return Ok(out);
    }

    let (head, head_time) = ctx.git.head_commit_time()?.ok_or_else(|| {
        tag(
            Reason::Repository,
            anyhow!(
                "`gates.{GATE}` reads the head commit's date and the repository has no commit yet"
            ),
        )
    })?;
    let head_day = day_of(head_time);
    let head_short = short(&head);
    if ctx.staged {
        out.notes.push(format!(
            "the staged change is not a commit yet: ages are read at `HEAD` ({head_short})"
        ));
    }
    let replay_parent = ctx.replay_parent();

    for (i, rule) in rules.iter().enumerate() {
        let path = rule.path.as_str();
        if exempt.matches(path) {
            out.notes.push(format!(
                "`{path}` (rule {i}) is in `exempt_paths`: not judged"
            ));
            continue;
        }
        match ctx.git.head_commit_entry(path)? {
            HeadEntry::File => {}
            HeadEntry::Missing if ctx.predates_config(path)? => {
                out.notes.push(format!(
                    "rule {i} skipped: `{path}` is on neither side of this change {PREDATES_CONFIG_NOTE}"
                ));
                continue;
            }
            HeadEntry::Missing => {
                return Err(tag(
                    Reason::Configuration,
                    anyhow!(
                        "`gates.{GATE}.rules[{i}].path` = `{path}` does not exist at the head commit ({head_short})"
                    ),
                ))
            }
            HeadEntry::Other(what) => {
                return Err(tag(
                    Reason::Configuration,
                    anyhow!(
                        "`gates.{GATE}.rules[{i}].path` = `{path}` is {what} at the head commit, not a regular file"
                    ),
                ))
            }
        }
        out.examined += 1;

        // Every schedule naming this path is judged, stale or not: a void one is reported.
        let mut valid: Vec<ParsedDirective> = Vec::new();
        for (n, d) in &schedules {
            let Some(date) = scheduled_date(d, path) else {
                continue;
            };
            match judge_schedule(date, head_day, rule.max_age_days) {
                Ok(_) => valid.push((*d).clone()),
                Err(why) => {
                    out.push(
                        severity,
                        &crate::findings::REVIEW_SCHEDULE_VOID,
                        Some(path),
                        None,
                        format!(
                            "The `review-scheduled` directive for `{path}` ({}) grants nothing: {why}.",
                            d.source
                        ),
                        &format!(
                            "Give a date after {} and no later than {}.",
                            date_of_day(head_day),
                            date_of_day(head_day + i64::from(rule.max_age_days))
                        ),
                    );
                    out.anchor_last(format!("{} #{n}", d.source));
                }
            }
        }

        let judge = |added: &[String]| rule.qualifies(added);
        let judge: Option<&AddedLinesJudge<'_>> = if rule.patterns.is_empty() {
            None
        } else {
            Some(&judge)
        };
        let newest = match ctx
            .git
            .newest_first_parent_change(path, replay_parent.as_deref(), judge)?
        {
            PathHistory::Found { commit, time } => Some((commit, time)),
            PathHistory::NeverChanged => None,
            PathHistory::Truncated { at } => {
                return Err(tag(
                    Reason::Repository,
                    anyhow!(
                        "the history of `{path}` ends at commit {} before a qualifying commit was found: the clone is probably shallow — fetch full history (`fetch-depth: 0`)",
                        short(&at)
                    ),
                ))
            }
        };
        let age = judge_age(head_time, newest.as_ref().map(|(_, t)| *t), rule.max_age_days)
            .map_err(|why| {
                let commit = newest.as_ref().map(|(c, _)| short(c)).unwrap_or_default();
                tag(
                    Reason::Repository,
                    anyhow!(
                        "the newest qualifying commit for `{path}` ({commit}) cannot bound its age: {why}"
                    ),
                )
            })?;
        let refreshed = |days: i64| match &newest {
            Some((commit, time)) => format!(
                "last refreshed by commit {} ({}), {days} day(s) before the head commit ({})",
                short(commit),
                crate::audit_html::date(*time),
                date_of_day(head_day)
            ),
            None => String::new(),
        };
        let what = match age {
            Age::Fresh { days } => {
                out.notes.push(format!(
                    "`{path}`: {}; limit {} day(s)",
                    refreshed(days),
                    rule.max_age_days
                ));
                continue;
            }
            Age::Stale { days } => format!(
                "`{path}` was {}; rule {i} allows {} day(s).",
                refreshed(days),
                rule.max_age_days
            ),
            Age::NeverRefreshed => format!(
                "No commit on the head commit's first-parent history changes `{path}`{}: its age is unknown, and rule {i} allows {} day(s).",
                patterns_clause(rule),
                rule.max_age_days
            ),
        };
        let qualifying = if rule.patterns.is_empty() {
            String::new()
        } else {
            format!(" (a commit that changes it{})", patterns_clause(rule))
        };
        let lifted = tokens::find_override(
            &valid,
            GATE,
            &crate::findings::DOC_STALE,
            tokens::REVIEW_SCHEDULED,
            path,
        );
        out.lift_or_push(
            lifted,
            severity,
            &crate::findings::DOC_STALE,
            (Some(path), None),
            what,
            &format!(
                "Refresh `{path}` in this change{qualifying}, or schedule its review with `review-scheduled: {path} <YYYY-MM-DD>` (a date after {} and no later than {}).",
                date_of_day(head_day),
                date_of_day(head_day + i64::from(rule.max_age_days))
            ),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(path: &str, days: u32, patterns: Option<&[&str]>) -> DocStalenessRule {
        DocStalenessRule {
            path: path.to_string(),
            max_age_days: days,
            require_added_line_match: patterns.map(|p| p.iter().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn a_day_number_is_the_civil_calendar_day() {
        for (text, day) in [
            ("1970-01-01", 0),
            ("1970-01-02", 1),
            ("1969-12-31", -1),
            ("2000-02-29", 11_016),
            ("2026-11-27", 20_784),
        ] {
            assert_eq!(parse_day(text), Some(day), "{text}");
            assert_eq!(date_of_day(day), text);
        }
        // Round trip over leap years and century boundaries.
        for day in (-700_000..700_000).step_by(997) {
            assert_eq!(parse_day(&date_of_day(day)), Some(day), "{day}");
        }
        for bad in [
            "2026-02-29",
            "2100-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-04-31",
            "2026-1-01",
            "26-01-01",
            "2026/01/01",
            "2026-01-01x",
            "+026-01-01",
            "<YYYY-MM-DD>",
            "",
        ] {
            assert_eq!(parse_day(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_schedule_grants_only_a_date_after_the_head_and_within_the_cap() {
        let head = parse_day("2026-11-27").unwrap();
        // Positive controls: the day after, and the cap itself.
        assert_eq!(judge_schedule(Some("2026-11-28"), head, 92), Ok(head + 1));
        assert_eq!(judge_schedule(Some("2027-02-27"), head, 92), Ok(head + 92));
        // Negative controls: the head's own day, the past, one day past the cap, not a
        // date, nothing.
        for (date, why) in [
            (Some("2026-11-27"), "not after"),
            (Some("2025-01-01"), "not after"),
            (Some("2027-02-28"), "more than `max_age_days`"),
            (Some("soon"), "not a `YYYY-MM-DD`"),
            (None, "missing"),
        ] {
            let err = judge_schedule(date, head, 92).unwrap_err();
            assert!(err.contains(why), "{date:?}: {err}");
        }
    }

    #[test]
    fn age_is_the_head_time_minus_the_newest_refresh() {
        let head = 100 * DAY;
        assert_eq!(judge_age(head, Some(head), 1), Ok(Age::Fresh { days: 0 }));
        // Exactly the limit is still fresh; one second more is stale.
        assert_eq!(
            judge_age(head, Some(head - 92 * DAY), 92),
            Ok(Age::Fresh { days: 92 })
        );
        assert_eq!(
            judge_age(head, Some(head - 92 * DAY - 1), 92),
            Ok(Age::Stale { days: 92 })
        );
        assert_eq!(judge_age(head, None, 92), Ok(Age::NeverRefreshed));
        assert!(judge_age(head, Some(head + 1), 92)
            .unwrap_err()
            .contains("after the head commit"));
    }

    #[test]
    fn rules_that_could_never_be_judged_are_refused() {
        for (rules, why) in [
            (vec![rule("docs/a.md", 0, None)], "is 0"),
            (vec![rule(" ", 5, None)], "is blank"),
            (
                vec![rule("/docs/a.md", 5, None)],
                "not a repository-relative",
            ),
            (
                vec![rule("docs/../a.md", 5, None)],
                "not a repository-relative",
            ),
            (vec![rule("docs/", 5, None)], "not a repository-relative"),
            (
                vec![rule("docs/a.md", 5, None), rule("docs/a.md", 9, None)],
                "already registered by rule 0",
            ),
            (vec![rule("docs/a.md", 5, Some(&[]))], "empty list"),
            (
                vec![rule("docs/a.md", 5, Some(&[r"^\s*$"]))],
                "whitespace-only",
            ),
        ] {
            let err = compile_rules(&rules).expect_err(why);
            let text = format!("{err:#}");
            assert!(text.contains(why), "{text}");
            assert!(text.contains("gates.doc-staleness.rules["), "{text}");
        }
        assert!(compile_rules(&[rule("docs/REVIEW-LOG.md", 92, Some(&["^## "]))]).is_ok());
    }

    #[test]
    fn a_commit_qualifies_when_every_pattern_matches_an_added_line() {
        let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let c = compile_rules(&[rule(
            "docs/REVIEW-LOG.md",
            92,
            Some(&[r"^## \d{4}-\d{2}-\d{2} ", "^Reviewed-by: "]),
        )])
        .unwrap()
        .remove(0);
        assert!(c.qualifies(&lines(&["## 2026-11-27 quarterly", "Reviewed-by: a"])));
        // Every pattern is needed, each on some added line, as in `companion-edit`.
        assert!(!c.qualifies(&lines(&["## 2026-11-27 quarterly"])));
        assert!(!c.qualifies(&lines(&["", "Reviewed-by: a"])));
        // A whitespace-only edit, or an undated heading, does not.
        assert!(!c.qualifies(&lines(&["", "   "])));
        assert!(!c.qualifies(&lines(&["## notes"])));
        assert!(!c.qualifies(&[]));
        // Without patterns any change qualifies.
        let any = compile_rules(&[rule("docs/a.md", 5, None)])
            .unwrap()
            .remove(0);
        assert!(any.qualifies(&[]));
    }
}
