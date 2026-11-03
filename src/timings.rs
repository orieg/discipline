//! Where a check spends its time: wall time per gate and per shared phase, and counters
//! of the work behind it (#995). Recorded only on request (`check --timings`,
//! `DISCIPLINE_TIMINGS=1`) and written as the JSON report's `timings` object.
//!
//! Wall time is not deterministic, so it never reaches a verdict: the record is kept
//! apart from [`crate::guards::CheckSummary`], which baselines, fingerprints and `replay`
//! read, and is added to the JSON report only as it is written ([`crate::report::json_report`]).
//! The counters are deterministic for a given tree and configuration, which is what a test
//! asserts; the times are for people.
//!
//! A run is recorded on the thread that runs it: the gates run in sequence on one thread.
//! Scopes nest. A phase opened inside a gate (the shared diff extraction, the runner rules
//! built on first use) is charged to the phase, and the gate's entry keeps its own time
//! and counts only, so no work is counted twice and the entries add up to the run.

use serde::Serialize;
use std::cell::RefCell;
use std::time::{Duration, Instant};

/// The environment variable that asks for timings, as `--timings` does.
pub const TIMINGS_ENV: &str = "DISCIPLINE_TIMINGS";

/// Whether `DISCIPLINE_TIMINGS` asks for timings: `1` or `true`.
pub fn requested_by_env() -> bool {
    requested(std::env::var("DISCIPLINE_TIMINGS").ok().as_deref())
}

fn requested(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"))
}

/// Counts of the work a scope did. Deterministic for one tree and configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counters {
    /// Files a language pack extracted facts from (a full parse).
    pub extract_calls: u64,
    /// Bytes of source those extractions read.
    pub extract_bytes: u64,
    /// Calls listing the change's files (`changed_files`).
    pub changed_files_calls: u64,
    /// Calls listing the repository's tracked files (`tracked_files`).
    pub tracked_files_calls: u64,
}

impl Counters {
    fn add(&mut self, other: &Counters) {
        self.extract_calls += other.extract_calls;
        self.extract_bytes += other.extract_bytes;
        self.changed_files_calls += other.changed_files_calls;
        self.tracked_files_calls += other.tracked_files_calls;
    }
}

/// One unit of counted work.
#[derive(Debug, Clone, Copy)]
pub enum Counter {
    /// A full extraction of `bytes` bytes of source.
    Extract {
        bytes: usize,
    },
    ChangedFiles,
    TrackedFiles,
}

/// One gate's or one phase's share of the run. A name met more than once (a phase entered
/// by several gates) is one entry, its calls summed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub name: String,
    /// How many times the scope was entered.
    pub calls: u64,
    /// Wall time in the scope, less the time of the scopes nested in it, in milliseconds.
    pub duration_ms: f64,
    /// The work done in the scope, less the work of the scopes nested in it.
    pub counters: Counters,
}

/// The report's `timings` object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Timings {
    /// Says the times are not deterministic and never part of a verdict.
    pub stability: &'static str,
    /// Wall time from the start of the run to the report, in milliseconds.
    pub total_ms: f64,
    /// Shared phases, in the order first entered.
    pub phases: Vec<Entry>,
    /// Gates, in the order they ran.
    pub gates: Vec<Entry>,
    /// Every count of the run, inside a scope or not.
    pub counters: Counters,
}

/// The `stability` statement of every `timings` object.
pub const STABILITY: &str = "UNSTABLE until 1.0. Wall time is not deterministic: it is never compared by replay, kept in a baseline or fingerprinted. The counters are deterministic for one tree and configuration.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Phase,
    Gate,
}

struct Frame {
    kind: Kind,
    name: &'static str,
    start: Instant,
    nested: Duration,
    counters: Counters,
}

struct Recorder {
    started: Instant,
    stack: Vec<Frame>,
    phases: Vec<(Entry, Duration)>,
    gates: Vec<(Entry, Duration)>,
    counters: Counters,
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
    static FINISHED: RefCell<Option<Timings>> = const { RefCell::new(None) };
}

/// Starts recording a run on this thread, dropping any earlier record.
pub fn start() {
    FINISHED.with(|f| *f.borrow_mut() = None);
    RECORDER.with(|r| {
        *r.borrow_mut() = Some(Recorder {
            started: Instant::now(),
            stack: Vec::new(),
            phases: Vec::new(),
            gates: Vec::new(),
            counters: Counters::default(),
        })
    });
}

/// Ends the run's record and keeps it for [`finished`]. `None` when nothing was recording.
pub fn finish() -> Option<Timings> {
    let rec = RECORDER.with(|r| r.borrow_mut().take())?;
    let entries = |list: Vec<(Entry, Duration)>| {
        list.into_iter()
            .map(|(mut e, d)| {
                e.duration_ms = millis(d);
                e
            })
            .collect()
    };
    let timings = Timings {
        stability: STABILITY,
        total_ms: millis(rec.started.elapsed()),
        phases: entries(rec.phases),
        gates: entries(rec.gates),
        counters: rec.counters,
    };
    FINISHED.with(|f| *f.borrow_mut() = Some(timings.clone()));
    Some(timings)
}

/// The record [`finish`] kept on this thread, for the report written after it.
pub fn finished() -> Option<Timings> {
    FINISHED.with(|f| f.borrow().clone())
}

/// Drops any record on this thread, running or kept.
pub fn clear() {
    RECORDER.with(|r| *r.borrow_mut() = None);
    FINISHED.with(|f| *f.borrow_mut() = None);
}

fn millis(d: Duration) -> f64 {
    d.as_micros() as f64 / 1000.0
}

/// Runs `f` as the shared phase `name`.
pub fn phase<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    scoped(Kind::Phase, name, f)
}

/// Runs `f` as the gate `name`.
pub fn gate<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    scoped(Kind::Gate, name, f)
}

fn scoped<T>(kind: Kind, name: &'static str, f: impl FnOnce() -> T) -> T {
    let recording = RECORDER.with(|r| {
        let mut r = r.borrow_mut();
        let Some(rec) = r.as_mut() else {
            return false;
        };
        rec.stack.push(Frame {
            kind,
            name,
            start: Instant::now(),
            nested: Duration::ZERO,
            counters: Counters::default(),
        });
        true
    });
    let out = f();
    if recording {
        RECORDER.with(|r| {
            let mut r = r.borrow_mut();
            let Some(rec) = r.as_mut() else {
                return;
            };
            let Some(frame) = rec.stack.pop() else {
                return;
            };
            let elapsed = frame.start.elapsed();
            if let Some(parent) = rec.stack.last_mut() {
                parent.nested += elapsed;
            }
            let own = elapsed.saturating_sub(frame.nested);
            let list = match frame.kind {
                Kind::Phase => &mut rec.phases,
                Kind::Gate => &mut rec.gates,
            };
            match list.iter_mut().find(|(e, _)| e.name == frame.name) {
                Some((e, d)) => {
                    e.calls += 1;
                    e.counters.add(&frame.counters);
                    *d += own;
                }
                None => list.push((
                    Entry {
                        name: frame.name.to_string(),
                        calls: 1,
                        duration_ms: 0.0,
                        counters: frame.counters,
                    },
                    own,
                )),
            }
        });
    }
    out
}

/// Counts `what` against the innermost open scope and the run. Nothing when no run is
/// recording.
pub fn count(what: Counter) {
    RECORDER.with(|r| {
        let mut r = r.borrow_mut();
        let Some(rec) = r.as_mut() else {
            return;
        };
        let one = match what {
            Counter::Extract { bytes } => Counters {
                extract_calls: 1,
                extract_bytes: bytes as u64,
                ..Counters::default()
            },
            Counter::ChangedFiles => Counters {
                changed_files_calls: 1,
                ..Counters::default()
            },
            Counter::TrackedFiles => Counters {
                tracked_files_calls: 1,
                ..Counters::default()
            },
        };
        rec.counters.add(&one);
        if let Some(top) = rec.stack.last_mut() {
            top.counters.add(&one);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(list: &'a [Entry], name: &str) -> &'a Entry {
        list.iter().find(|e| e.name == name).expect(name)
    }

    #[test]
    fn nothing_is_recorded_unless_a_run_was_started() {
        clear();
        let got = gate("g", || {
            count(Counter::ChangedFiles);
            7
        });
        assert_eq!(got, 7);
        assert_eq!(finish(), None);
        assert_eq!(finished(), None);
    }

    #[test]
    fn work_is_charged_to_the_innermost_scope_once() {
        start();
        count(Counter::TrackedFiles);
        gate("first", || {
            count(Counter::ChangedFiles);
            phase("shared", || {
                count(Counter::Extract { bytes: 10 });
                count(Counter::Extract { bytes: 5 });
            });
        });
        gate("second", || {
            phase("shared", || count(Counter::Extract { bytes: 1 }));
        });
        let t = finish().expect("recording");
        assert_eq!(finished().as_ref(), Some(&t));

        let first = entry(&t.gates, "first");
        assert_eq!(first.calls, 1);
        assert_eq!(
            first.counters,
            Counters {
                changed_files_calls: 1,
                ..Counters::default()
            }
        );
        assert_eq!(entry(&t.gates, "second").counters, Counters::default());
        let shared = entry(&t.phases, "shared");
        assert_eq!(shared.calls, 2);
        assert_eq!(
            shared.counters,
            Counters {
                extract_calls: 3,
                extract_bytes: 16,
                ..Counters::default()
            }
        );
        // The run counts everything, the work outside any scope too.
        assert_eq!(
            t.counters,
            Counters {
                extract_calls: 3,
                extract_bytes: 16,
                changed_files_calls: 1,
                tracked_files_calls: 1,
            }
        );
        assert_eq!(
            t.gates.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        clear();
    }

    #[test]
    fn a_gate_keeps_its_own_time_and_the_nested_phase_takes_its_share() {
        start();
        gate("outer", || {
            phase("inner", || std::thread::sleep(Duration::from_millis(30)));
        });
        let t = finish().unwrap();
        let (outer, inner) = (entry(&t.gates, "outer"), entry(&t.phases, "inner"));
        assert!(inner.duration_ms >= 30.0, "{inner:?}");
        assert!(outer.duration_ms < inner.duration_ms, "{outer:?} {inner:?}");
        assert!(t.total_ms >= outer.duration_ms + inner.duration_ms);
        clear();
    }

    #[test]
    fn the_variable_asks_only_with_one_or_true() {
        for (v, want) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("0", false),
            ("", false),
            ("yes", false),
        ] {
            assert_eq!(requested(Some(v)), want, "{v:?}");
        }
        assert!(!requested(None));
    }
}
