//! `discipline audit --last N`: every escape hatch across a branch's merged changes.
//!
//! Each of the last N first-parent commits is one merged change. The audit reads git
//! objects only, runs no check and starts no child process, and records one entry per
//! exception the change carried:
//!
//! * `directive`: a directive line in the commit message, parsed by [`crate::tokens`];
//! * `config`: a loosening of `discipline.toml` between the change's parent and itself,
//!   as `config-integrity` judges it ([`crate::guards::integrity::diff_configs`]);
//! * `config-unreadable`: a side of that comparison that does not parse (an old key this
//!   binary no longer reads), so the comparison could not be made;
//! * `baseline`: findings added to `discipline-baseline.toml`, per gate;
//! * `inline-marker`: an added line carrying a `discipline:allow(<gate>)` marker.
//!
//! Tightenings of `discipline.toml` are listed separately (`tightenings`), so a loosening
//! can be matched with a later change that restored it.
//!
//! Signals are queries over those records, each with a rank and the next action a
//! reviewer takes. `checks` gives every signal a state (`found` or `clean`) and names what
//! git alone cannot tell (`not-checked`, with the reason), so an absent answer never reads
//! as a clean one.
//!
//! What a record claims is not what a check applied: a directive in a commit message may
//! have lifted nothing. `evidence` says which it is, and `tier` who controls the input:
//! `A` for the tree on the audited branch, `C` for text the change's author wrote. A
//! directive's reason is free text and can echo secret material or a name, so it is
//! reported as a SHA-256 and a length unless `--reasons` asks for the text.

use crate::baseline::DisciplineBaseline;
use crate::config::DisciplineConfig;
use crate::guards::integrity::{self, Change};
use anyhow::{anyhow, bail, Context, Result};
use git2::{Commit, Oid, Repository};
use std::collections::BTreeMap;

const CONFIG_NAME: &str = "discipline.toml";
const BASELINE_NAME: &str = crate::baseline::DEFAULT_BASELINE_FILE;
const MARKER: &str = "discipline:allow(";

pub struct Options {
    pub last: usize,
    /// The branch whose history is audited (default: the default branch).
    pub reference: Option<String>,
    /// Report each directive's reason text, not only its hash.
    pub reasons: bool,
}

/// One exception one change carried.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Record {
    /// Full commit id of the change.
    pub sha: String,
    /// The pull request number, from the subject's `(#N)`.
    pub pr: Option<u64>,
    /// Commit time, seconds since the Unix epoch.
    pub time: i64,
    /// `directive`, `config`, `config-unreadable`, `baseline` or `inline-marker`.
    pub kind: &'static str,
    /// `process` (a waiver of a process rule, such as `no-issue`), `detector` (a waiver
    /// of a finding), `config`, `baseline` or `inline`.
    pub class: &'static str,
    /// `claimed` (text that asks for an exception) or `applied` (a tree change that is one).
    pub evidence: &'static str,
    /// `A`: git objects on the audited branch. `C`: text the change's author wrote.
    pub tier: &'static str,
    /// The gate id, or the configuration table a loosening is under.
    pub gate: Option<String>,
    /// The directive's name, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directive: Option<String>,
    /// The configuration option a loosening changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// List entries gained or lost, or baseline findings added.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// Where the exception is: the file of an inline marker or a baseline, `discipline.toml`
    /// for a loosening.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The directive was inside an HTML comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// SHA-256 of the directive's reason, to group reuse without the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_len: Option<usize>,
    /// The reason text: only under `--reasons`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why a configuration could not be compared: which side, and the parse error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The change's position, 0 for the newest audited change.
    #[serde(rename = "change_index")]
    pub ord: usize,
    /// The change's subject line, as its author wrote it.
    pub subject: String,
}

impl Record {
    fn new(change: &ChangeInfo, kind: &'static str, class: &'static str) -> Self {
        let (evidence, tier) = match kind {
            "directive" => ("claimed", "C"),
            "inline-marker" => ("claimed", "A"),
            _ => ("applied", "A"),
        };
        Record {
            sha: change.sha.clone(),
            pr: change.pr,
            time: change.time,
            kind,
            class,
            evidence,
            tier,
            gate: None,
            directive: None,
            key: None,
            change: None,
            before: None,
            after: None,
            count: None,
            file: None,
            line: None,
            hidden: None,
            reason_sha256: None,
            reason_len: None,
            reason: None,
            detail: None,
            ord: change.ord,
            subject: change.subject.clone(),
        }
    }

    fn label(&self) -> String {
        match self.pr {
            Some(n) => format!("#{n}"),
            None => self.sha.chars().take(10).collect(),
        }
    }
}

/// Gates that guard the other gates: loosening one weakens the checks on every change.
pub const GUARD_GATES: &[&str] = &[
    "ratified-paths",
    "config-integrity",
    "ci-integrity",
    "instruction-smuggling",
];

/// A query over the records that found something: what, how urgent, and what to do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Signal {
    pub id: &'static str,
    /// `look-first`, `look-soon` or `review`.
    pub rank: &'static str,
    /// Records the signal is about.
    pub count: usize,
    /// The changes they are in, newest first (`#N`, else a 10-character commit id).
    pub changes: Vec<String>,
    /// Indexes into `records` (or `tightenings`, for none today).
    pub records: Vec<usize>,
    /// The next action a reviewer takes.
    pub next: &'static str,
}

/// One question the audit asks, and whether it could answer it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    pub id: &'static str,
    /// `found`, `clean` or `not-checked`.
    pub state: &'static str,
    /// What was found, or why it was not checked.
    pub detail: String,
}

struct SignalDef {
    id: &'static str,
    rank: &'static str,
    next: &'static str,
    /// Whether record `i` belongs to the signal, given every record and the tightenings.
    test: fn(usize, &[Record], &[Record]) -> bool,
}

fn is_config(r: &Record) -> bool {
    r.kind == "config"
}

/// The change also carries a directive that waives `config-integrity`.
fn waived_in_change(r: &Record, all: &[Record]) -> bool {
    all.iter().any(|x| {
        x.kind == "directive" && x.sha == r.sha && x.gate.as_deref() == Some("config-integrity")
    })
}

const SIGNALS: &[SignalDef] = &[
    SignalDef {
        id: "guard-gate-loosened",
        rank: "look-first",
        next: "Open the change and confirm each loosened option of a gate that guards the other gates was intended.",
        test: |i, all, _| {
            let r = &all[i];
            is_config(r) && r.gate.as_deref().is_some_and(|g| GUARD_GATES.contains(&g))
        },
    },
    SignalDef {
        id: "hidden-directive",
        rank: "look-first",
        next: "Read the commit message as text: a directive inside an HTML comment does not show where the message is rendered.",
        test: |i, all, _| all[i].hidden == Some(true),
    },
    SignalDef {
        id: "config-unreadable",
        rank: "look-soon",
        next: "Compare that change's discipline.toml by hand: this binary could not read one side, so its loosenings are unknown.",
        test: |i, all, _| all[i].kind == "config-unreadable",
    },
    SignalDef {
        id: "loosened-without-pull-request",
        rank: "look-soon",
        next: "Confirm the loosening with whoever pushed it: there was no pull request to review it in.",
        test: |i, all, _| is_config(&all[i]) && all[i].pr.is_none(),
    },
    SignalDef {
        id: "loosening-without-waiver",
        rank: "review",
        next: "Read the pull request body for the allow-gate-weakening this audit did not find in the commit message; if it is not there, find out how config-integrity passed.",
        test: |i, all, _| {
            let r = &all[i];
            is_config(r) && r.pr.is_some() && !waived_in_change(r, all)
        },
    },
    SignalDef {
        id: "waived-then-loosened",
        rank: "review",
        next: "Decide whether the loosening fixed the rule or only stopped it asking.",
        test: |i, all, _| {
            let r = &all[i];
            is_config(r)
                && all.iter().any(|x| {
                    x.kind == "directive" && x.ord > r.ord && x.gate.is_some() && x.gate == r.gate
                })
        },
    },
    SignalDef {
        id: "loosened-not-restored",
        rank: "review",
        next: "Tighten the option back, or record why the looser value stays.",
        test: |i, all, tightenings| {
            let r = &all[i];
            is_config(r)
                && !tightenings
                    .iter()
                    .any(|t| t.ord < r.ord && t.gate == r.gate && t.key == r.key)
        },
    },
    SignalDef {
        id: "baseline-grew",
        rank: "review",
        next: "Fix the grandfathered findings, or record why each stays in the baseline.",
        test: |i, all, _| all[i].kind == "baseline" && all[i].count.is_some_and(|n| n > 0),
    },
];

/// What git alone cannot tell, and why.
const NOT_CHECKED: &[(&str, &str)] = &[
    (
        "directive-lifted-a-finding",
        "needs `discipline replay`: a directive in a commit message may have lifted nothing",
    ),
    (
        "pull-request-body-directives",
        "pull request bodies are not read",
    ),
    (
        "owner-ratification",
        "needs the forge: ratification comments live on issues",
    ),
    (
        "independent-review",
        "needs the forge: reviews live on pull requests",
    ),
    (
        "agent-identity",
        "no record of which agent made a change is kept in history",
    ),
];

/// The signals that found something, and a state for every question.
pub fn signals(records: &[Record], tightenings: &[Record]) -> (Vec<Signal>, Vec<Check>) {
    let mut found = Vec::new();
    let mut checks = Vec::new();
    for def in SIGNALS {
        let hits: Vec<usize> = (0..records.len())
            .filter(|&i| (def.test)(i, records, tightenings))
            .collect();
        if hits.is_empty() {
            checks.push(Check {
                id: def.id,
                state: "clean",
                detail: "none".to_string(),
            });
            continue;
        }
        let mut changes: Vec<String> = Vec::new();
        for &i in &hits {
            let l = records[i].label();
            if !changes.contains(&l) {
                changes.push(l);
            }
        }
        checks.push(Check {
            id: def.id,
            state: "found",
            detail: format!("{} in {} change(s)", hits.len(), changes.len()),
        });
        found.push(Signal {
            id: def.id,
            rank: def.rank,
            count: hits.len(),
            changes,
            records: hits,
            next: def.next,
        });
    }
    for (id, why) in NOT_CHECKED {
        checks.push(Check {
            id,
            state: "not-checked",
            detail: why.to_string(),
        });
    }
    (found, checks)
}

/// Web links for a change, from the `origin` remote (no request is made).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Links {
    /// The repository's web page.
    pub repository: String,
    /// A pull request's page, with `{n}` for its number.
    pub pull: String,
    /// A commit's page, with `{sha}` for its full id.
    pub commit: String,
    /// A file at a commit, with `{sha}`, `{path}` and `{line}` (drop `#L{line}` when the
    /// record has no line).
    pub file: String,
    /// One file's diff within a commit, with `{sha}` and `{path_sha256}`; GitHub only.
    pub file_diff: Option<String>,
}

impl Links {
    /// Links for a forge's web UI, or `None` when the remote names no known forge.
    pub fn for_forge(f: &crate::forge::Forge) -> Self {
        use crate::forge::ForgeKind::*;
        let base = format!("{}/{}", f.url, f.repo);
        let (pull, commit, file) = match f.kind {
            GitHub => ("pull/{n}", "commit/{sha}", "blob/{sha}/{path}#L{line}"),
            GitLab => (
                "-/merge_requests/{n}",
                "-/commit/{sha}",
                "-/blob/{sha}/{path}#L{line}",
            ),
            Gitea | Forgejo => (
                "pulls/{n}",
                "commit/{sha}",
                "src/commit/{sha}/{path}#L{line}",
            ),
        };
        Links {
            pull: format!("{base}/{pull}"),
            commit: format!("{base}/{commit}"),
            file: format!("{base}/{file}"),
            file_diff: (f.kind == GitHub)
                .then(|| format!("{base}/commit/{{sha}}#diff-{{path_sha256}}")),
            repository: base,
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Summary {
    /// [`crate::output_schema::AUDIT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The discipline version that wrote the report.
    pub version: String,
    /// The ref audited, as given or defaulted.
    pub reference: String,
    /// The commit the ref resolved to.
    pub tip: String,
    /// Web links, when the `origin` remote names a known forge.
    pub links: Option<Links>,
    /// Changes audited.
    pub changes: usize,
    /// Changes that carried at least one record.
    pub changes_with_records: usize,
    /// Record count per kind.
    pub by_kind: BTreeMap<String, usize>,
    /// Record count per class.
    pub by_class: BTreeMap<String, usize>,
    /// Record count per gate (or configuration table).
    pub by_gate: BTreeMap<String, usize>,
    /// Signals that found something, most urgent first.
    pub signals: Vec<Signal>,
    /// Every question the audit asks, with `found`, `clean` or `not-checked`.
    pub checks: Vec<Check>,
    /// Newest change first; within a change, in the order the kinds are listed above.
    pub records: Vec<Record>,
    /// Tightenings of `discipline.toml`, newest first (`kind` `config-tightening`): the
    /// value moved in the stricter direction, so `before` is the looser one.
    pub tightenings: Vec<Record>,
    /// Edits to paths the change's parent configuration protects under
    /// `ratified-paths` (`kind` `protected-edit`), newest first. Whether each was ratified
    /// needs the forge and is `not-checked`.
    pub protected_edits: Vec<Record>,
}

impl Summary {
    pub fn from_records(
        reference: String,
        changes: usize,
        records: Vec<Record>,
        tightenings: Vec<Record>,
    ) -> Self {
        let mut s = Summary {
            schema_version: crate::output_schema::AUDIT_SCHEMA_VERSION,
            version: env!("CARGO_PKG_VERSION").to_string(),
            reference,
            changes,
            ..Default::default()
        };
        let mut shas = std::collections::BTreeSet::new();
        for r in &records {
            shas.insert(r.sha.as_str());
            *s.by_kind.entry(r.kind.to_string()).or_default() += 1;
            *s.by_class.entry(r.class.to_string()).or_default() += 1;
            if let Some(g) = &r.gate {
                *s.by_gate.entry(g.clone()).or_default() += 1;
            }
        }
        s.changes_with_records = shas.len();
        let (found, checks) = signals(&records, &tightenings);
        s.signals = found;
        s.checks = checks;
        s.records = records;
        s.tightenings = tightenings;
        s
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.signals.is_empty() {
            out.push_str("Needs a decision: nothing the git history shows.\n");
        } else {
            out.push_str("Needs a decision:\n");
            for sg in &self.signals {
                let mut changes = sg
                    .changes
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                if sg.changes.len() > 8 {
                    changes.push_str(&format!(" and {} more", sg.changes.len() - 8));
                }
                out.push_str(&format!(
                    "  {:<10} {:<30} {:>3}  {changes}\n             next: {}\n",
                    sg.rank, sg.id, sg.count, sg.next
                ));
            }
        }
        if !self.protected_edits.is_empty() {
            let changes: std::collections::BTreeSet<&str> = self
                .protected_edits
                .iter()
                .map(|r| r.sha.as_str())
                .collect();
            out.push_str(&format!(
                "\nProtected paths: {} edit(s) in {} change(s); ratification not checked (needs the forge)\n",
                self.protected_edits.len(),
                changes.len()
            ));
        }
        out.push_str("\nChecks:\n");
        for c in &self.checks {
            out.push_str(&format!("  {:<11} {:<30} {}\n", c.state, c.id, c.detail));
        }
        out.push('\n');
        for r in self.records.iter().chain(&self.tightenings) {
            let label = match r.pr {
                Some(n) => format!("#{n}"),
                None => r.sha.chars().take(10).collect(),
            };
            let what = match r.kind {
                "directive" => format!(
                    "{}{}",
                    r.directive.as_deref().unwrap_or(""),
                    if r.hidden == Some(true) {
                        " (hidden)"
                    } else {
                        ""
                    }
                ),
                "config" | "config-tightening" => format!(
                    "{} {}",
                    r.key.as_deref().unwrap_or(""),
                    r.change
                        .map(|c| serde_json::to_value(c)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default())
                        .unwrap_or_default()
                ),
                "config-unreadable" => r.detail.clone().unwrap_or_default(),
                "baseline" => format!("{} finding(s) grandfathered", r.count.unwrap_or(0)),
                _ => format!(
                    "{}:{}",
                    r.file.as_deref().unwrap_or(""),
                    r.line.unwrap_or(0)
                ),
            };
            out.push_str(&format!(
                "{label:<9} {:<17} {:<24} {what}\n",
                r.kind,
                r.gate.as_deref().unwrap_or("-")
            ));
        }
        out.push_str(&format!(
            "\n{} changes audited, {} carried an exception ({} records)\n",
            self.changes,
            self.changes_with_records,
            self.records.len()
        ));
        for (k, n) in &self.by_class {
            out.push_str(&format!("  class {k:<10} {n}\n"));
        }
        for (g, n) in &self.by_gate {
            out.push_str(&format!("  gate  {g:<24} {n}\n"));
        }
        out.push_str(
            "Directives are read from commit messages only; whether one lifted a finding is `discipline replay`'s to say.\n",
        );
        out
    }
}

struct ChangeInfo {
    sha: String,
    pr: Option<u64>,
    time: i64,
    ord: usize,
    subject: String,
}

/// The gate a directive name waives, from the directive registry.
pub fn gate_for_directive(name: &str) -> Option<&'static str> {
    crate::tokens::DIRECTIVE_SPECS
        .iter()
        .find(|s| {
            crate::tokens::names_for_directive(s.canonical)
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name))
        })
        .map(|s| s.gate)
}

/// `process` for a waiver of a process rule (`no-issue`), `detector` otherwise.
fn directive_class(name: &str) -> &'static str {
    if crate::tokens::NO_ISSUE
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
    {
        "process"
    } else {
        "detector"
    }
}

/// The directive records of one commit message.
pub fn directive_records(message: &str, info: &ChangeInfoRef, reasons: bool) -> Vec<Record> {
    let change = info.to_owned();
    crate::tokens::parse_directives(
        message,
        crate::tokens::OverrideSource::Commit(change.sha.clone()),
    )
    .into_iter()
    .map(|d| {
        let mut r = Record::new(&change, "directive", directive_class(&d.directive));
        r.gate = gate_for_directive(&d.directive).map(str::to_string);
        r.hidden = Some(d.hidden);
        r.reason_sha256 = Some(crate::report::gitlab::sha256_hex(d.reason.as_bytes()));
        r.reason_len = Some(d.reason.chars().count());
        if reasons {
            r.reason = Some(d.reason.clone());
        }
        r.directive = Some(d.directive);
        r
    })
    .collect()
}

/// A change's identity, as the record constructors take it.
#[derive(Debug, Clone)]
pub struct ChangeInfoRef {
    pub sha: String,
    pub pr: Option<u64>,
    pub time: i64,
    /// The change's position, 0 for the newest audited change.
    pub ord: usize,
    pub subject: String,
}

impl ChangeInfoRef {
    fn to_owned(&self) -> ChangeInfo {
        ChangeInfo {
            sha: self.sha.clone(),
            pr: self.pr,
            time: self.time,
            ord: self.ord,
            subject: self.subject.clone(),
        }
    }
}

/// The configuration records of one change, from the two sides' `discipline.toml` text
/// (`None`: the side has no such file).
pub fn config_records(base: Option<&str>, head: Option<&str>, info: &ChangeInfoRef) -> Vec<Record> {
    config_changes(base, head, info).0
}

/// The loosenings and the tightenings of one change's `discipline.toml`. A tightening is
/// a loosening read backwards (head to parent); its `before` is the looser value.
pub fn config_changes(
    base: Option<&str>,
    head: Option<&str>,
    info: &ChangeInfoRef,
) -> (Vec<Record>, Vec<Record>) {
    let change = info.to_owned();
    if base == head {
        return (Vec::new(), Vec::new());
    }
    // Adopting a configuration is neither: there was none before.
    let Some(base) = base else {
        return (Vec::new(), Vec::new());
    };
    let unreadable = |detail: String| {
        let mut r = Record::new(&change, "config-unreadable", "config");
        r.file = Some(CONFIG_NAME.to_string());
        r.detail = Some(detail);
        (vec![r], Vec::new())
    };
    let parse = |side: &str, text: &str| {
        DisciplineConfig::from_toml_str(text)
            .map_err(|e| format!("{side}: {}", first_line(&e.to_string())))
    };
    let base_cfg = match parse("parent", base) {
        Ok(c) => c,
        Err(detail) => return unreadable(detail),
    };
    // A removed configuration leaves the built-in defaults in force.
    let head_cfg = match head {
        Some(h) => match parse("change", h) {
            Ok(c) => c,
            Err(detail) => return unreadable(detail),
        },
        None => DisciplineConfig::default_for_repo(&base_cfg.meta.name),
    };
    let (loosened, tightened) = match (
        integrity::diff_configs(&base_cfg, &head_cfg),
        integrity::diff_configs(&head_cfg, &base_cfg),
    ) {
        (Ok(l), Ok(t)) => (l, t),
        (Err(e), _) | (_, Err(e)) => {
            return unreadable(format!("compare: {}", first_line(&e.to_string())))
        }
    };
    let record = |kind: &'static str, w: integrity::Weakening, backwards: bool| {
        let mut r = Record::new(&change, kind, "config");
        r.file = Some(CONFIG_NAME.to_string());
        r.gate = Some(w.gate);
        r.key = Some(w.key);
        r.count = w.count;
        if backwards {
            // Read head to parent: swap the values back to the change's own direction.
            r.before = w.after;
            r.after = w.before;
        } else {
            r.change = Some(w.change);
            r.before = w.before;
            r.after = w.after;
        }
        r
    };
    // The option's line in the change's own file, to link to; a removed option has none.
    let at_line = |mut r: Record| {
        if let (Some(h), Some(g), Some(k)) = (head, r.gate.as_deref(), r.key.as_deref()) {
            r.line = key_line(h, g, k);
        }
        r
    };
    (
        loosened
            .into_iter()
            .map(|w| at_line(record("config", w, false)))
            .collect(),
        tightened
            .into_iter()
            .map(|w| at_line(record("config-tightening", w, true)))
            .collect(),
    )
}

/// The 1-based line of `key` under its table in a `discipline.toml` text: `[gates.<gate>]`,
/// or `[directives]`, `[tests]`, `[meta]`, `[languages.c]` for those tables.
pub fn key_line(text: &str, gate: &str, key: &str) -> Option<u32> {
    let (table, key) = match gate {
        "directives" | "tests" | "meta" => (format!("[{gate}]"), key),
        "languages" => match key.split_once('.') {
            Some((lang, k)) => (format!("[languages.{lang}]"), k),
            None => ("[languages]".to_string(), key),
        },
        g => (format!("[gates.{g}]"), key),
    };
    let mut inside = false;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == table;
            continue;
        }
        if inside {
            if let Some(rest) = t.strip_prefix(key) {
                if rest.trim_start().starts_with('=') {
                    return Some(i as u32 + 1);
                }
            }
        }
    }
    None
}

/// Findings added to the baseline, one record per gate.
pub fn baseline_records(
    base: Option<&str>,
    head: Option<&str>,
    info: &ChangeInfoRef,
) -> Vec<Record> {
    let change = info.to_owned();
    if base == head {
        return Vec::new();
    }
    let load = |text: Option<&str>| -> Option<DisciplineBaseline> {
        match text {
            None => Some(DisciplineBaseline::default()),
            Some(t) => toml::from_str(t).ok(),
        }
    };
    let (Some(b), Some(h)) = (load(base), load(head)) else {
        let mut r = Record::new(&change, "baseline", "baseline");
        r.file = Some(BASELINE_NAME.to_string());
        r.detail = Some("a side of the baseline does not parse".to_string());
        return vec![r];
    };
    let mut added: BTreeMap<&str, usize> = BTreeMap::new();
    for e in h.findings.iter().filter(|e| !b.findings.contains(e)) {
        *added.entry(e.gate.as_str()).or_default() += 1;
    }
    added
        .into_iter()
        .map(|(gate, n)| {
            let mut r = Record::new(&change, "baseline", "baseline");
            r.file = Some(BASELINE_NAME.to_string());
            r.gate = Some(gate.to_string());
            r.change = Some(Change::Gained);
            r.count = Some(n);
            r
        })
        .collect()
}

/// Comment openers a marker may follow, with only whitespace between.
const COMMENT_OPENERS: &[&str] = &["//", "#", "<!--", "/*", "*", "--", ";", "%"];

/// The gate of the `discipline:allow(<gate>)` marker an added line carries, if any. The
/// marker must open a comment (`// discipline:allow(pii): ...`, `<!-- discipline:allow(x) -->`)
/// and name a gate this binary has: the marker's name in a string literal, a list of
/// directive names or a code span is not a marker.
pub fn marker_gate(line: &str) -> Option<&'static str> {
    let mut from = 0;
    while let Some(i) = line[from..].find(MARKER) {
        let at = from + i;
        let before = line[..at].trim_end();
        let rest = &line[at + MARKER.len()..];
        if COMMENT_OPENERS.iter().any(|o| before.ends_with(o)) {
            if let Some(end) = rest.find(')') {
                let name = &rest[..end];
                if let Some(g) = crate::config::GATES.iter().find(|g| g.id == name) {
                    return Some(g.id);
                }
            }
        }
        from = at + MARKER.len();
    }
    None
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn blob_text(repo: &Repository, tree: &git2::Tree, path: &str) -> Result<Option<String>> {
    let Ok(entry) = tree.get_path(std::path::Path::new(path)) else {
        return Ok(None);
    };
    let blob = repo
        .find_blob(entry.id())
        .with_context(|| format!("cannot read `{path}` at {}", tree.id()))?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

/// Inline markers on lines the change added.
fn inline_records(
    repo: &Repository,
    parent: &git2::Tree,
    tree: &git2::Tree,
    info: &ChangeInfoRef,
) -> Result<Vec<Record>> {
    let change = info.to_owned();
    let diff = repo.diff_tree_to_tree(Some(parent), Some(tree), None)?;
    let mut out = Vec::new();
    diff.foreach(
        &mut |_, _| true,
        None,
        None,
        Some(&mut |delta, _hunk, line| {
            if line.origin() != '+' {
                return true;
            }
            let text = String::from_utf8_lossy(line.content());
            if let Some(gate) = marker_gate(&text) {
                let mut r = Record::new(&change, "inline-marker", "inline");
                r.gate = Some(gate.to_string());
                r.file = delta
                    .new_file()
                    .path()
                    .map(|p| p.to_string_lossy().replace('\\', "/"));
                r.line = line.new_lineno();
                out.push(r);
            }
            true
        }),
    )?;
    Ok(out)
}

/// Paths this change touched (either side of a rename) that its parent configuration
/// protects under `ratified-paths`. `detail` says whether that gate was on.
fn protected_records(
    repo: &Repository,
    parent: &git2::Tree,
    tree: &git2::Tree,
    parent_config: Option<&str>,
    info: &ChangeInfoRef,
) -> Result<Vec<Record>> {
    let Some(cfg) = parent_config.and_then(|t| DisciplineConfig::from_toml_str(t).ok()) else {
        return Ok(Vec::new());
    };
    let gate = &cfg.gates.ratified_paths;
    if gate.protected_paths.is_empty() {
        return Ok(Vec::new());
    }
    let filter = crate::guards::PathFilter::new(&gate.protected_paths)?;
    let change = info.to_owned();
    let diff = repo.diff_tree_to_tree(Some(parent), Some(tree), None)?;
    let mut paths = std::collections::BTreeSet::new();
    for d in diff.deltas() {
        for f in [d.old_file(), d.new_file()] {
            if let Some(p) = f.path() {
                let p = p.to_string_lossy().replace('\\', "/");
                if filter.matches(&p) {
                    paths.insert(p);
                }
            }
        }
    }
    Ok(paths
        .into_iter()
        .map(|p| {
            let mut r = Record::new(&change, "protected-edit", "protected");
            r.gate = Some("ratified-paths".to_string());
            r.file = Some(p);
            r.detail = Some(if gate.enabled { "gate on" } else { "gate off" }.to_string());
            r
        })
        .collect())
}

/// The records and tightenings of one first-parent change.
type ChangeParts = (Vec<Record>, Vec<Record>, Vec<Record>);

fn change_records(repo: &Repository, c: &Commit, ord: usize, reasons: bool) -> Result<ChangeParts> {
    let parent = c.parent(0)?;
    let (pt, ct) = (parent.tree()?, c.tree()?);
    let subject = c.summary().ok().flatten().unwrap_or("").to_string();
    let info = ChangeInfoRef {
        sha: c.id().to_string(),
        pr: crate::replay::pr_from_subject(&subject),
        time: c.time().seconds(),
        ord,
        subject,
    };
    let mut out = directive_records(c.message().unwrap_or(""), &info, reasons);
    let parent_config = blob_text(repo, &pt, CONFIG_NAME)?;
    let (loosened, tightened) = config_changes(
        parent_config.as_deref(),
        blob_text(repo, &ct, CONFIG_NAME)?.as_deref(),
        &info,
    );
    out.extend(loosened);
    out.extend(baseline_records(
        blob_text(repo, &pt, BASELINE_NAME)?.as_deref(),
        blob_text(repo, &ct, BASELINE_NAME)?.as_deref(),
        &info,
    ));
    out.extend(inline_records(repo, &pt, &ct, &info)?);
    let protected = protected_records(repo, &pt, &ct, parent_config.as_deref(), &info)?;
    Ok((out, tightened, protected))
}

pub fn run(opts: &Options) -> Result<Summary> {
    if opts.last == 0 {
        bail!("--last must be at least 1");
    }
    let repo = crate::gitctx::discover_repository(".")?;
    let reference = match &opts.reference {
        Some(r) => r.clone(),
        None => crate::hook::default_base(&repo).unwrap_or_else(|| "HEAD".to_string()),
    };
    let tip: Oid = repo
        .revparse_single(&reference)
        .with_context(|| format!("`{reference}` does not resolve"))?
        .peel_to_commit()
        .map_err(|e| anyhow!("`{reference}` is not a commit: {e}"))?
        .id();
    let commits = crate::replay::commits_to_replay(&repo, tip, opts.last)?;
    let mut records = Vec::new();
    let mut tightenings = Vec::new();
    let mut protected = Vec::new();
    for (ord, c) in commits.iter().enumerate() {
        let (r, t, p) = change_records(&repo, c, ord, opts.reasons)
            .with_context(|| format!("cannot read change {}", c.id()))?;
        records.extend(r);
        tightenings.extend(t);
        protected.extend(p);
    }
    let mut s = Summary::from_records(reference, commits.len(), records, tightenings);
    s.tip = tip.to_string();
    s.protected_edits = protected;
    s.links = forge_links(&repo);
    Ok(s)
}

/// Web links from the `origin` remote, the same detection `replay` uses; nothing is read
/// from the network.
fn forge_links(repo: &Repository) -> Option<Links> {
    let origin = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().ok().map(str::to_string))
        .map(|o| crate::forge::resolve_ssh_alias(&o, &|a| crate::forge::ssh_hostname_from_home(a)));
    crate::forge::detect(&|k| std::env::var(k).ok(), origin.as_deref())
        .ok()
        .map(|f| Links::for_forge(&f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> ChangeInfoRef {
        ChangeInfoRef {
            sha: "0123456789abcdef".into(),
            pr: Some(7),
            time: 1,
            ord: 0,
            subject: "s".into(),
        }
    }

    #[test]
    fn a_directive_is_recorded_with_its_gate_and_class_and_no_reason_text() {
        let msg = "fix: x\n\nno-issue: release bookkeeping\nallow-dependency: serde AKIA-SECRET\n<!-- allow-stub: fn_a later -->\nsee allow-ignore: in prose\n";
        let r = directive_records(msg, &info(), false);
        type Row<'a> = (Option<&'a str>, Option<&'a str>, &'a str, Option<bool>);
        let got: Vec<Row> = r
            .iter()
            .map(|r| (r.directive.as_deref(), r.gate.as_deref(), r.class, r.hidden))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some("no-issue"), Some("issue-link"), "process", Some(false)),
                (
                    Some("allow-dependency"),
                    Some("dependency-delta"),
                    "detector",
                    Some(false)
                ),
                (
                    Some("allow-stub"),
                    Some("stub-bodies"),
                    "detector",
                    Some(true)
                ),
            ]
        );
        assert!(r.iter().all(|r| r.evidence == "claimed" && r.tier == "C"));
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("AKIA"), "{json}");
        assert_eq!(r[1].reason_len, Some("serde AKIA-SECRET".len()));
        // The same reason hashes the same, so reuse can be counted without the text.
        let again = directive_records("s\n\nno-issue: release bookkeeping", &info(), false);
        assert_eq!(again[0].reason_sha256, r[0].reason_sha256);
        let with_text = directive_records(msg, &info(), true);
        assert_eq!(with_text[0].reason.as_deref(), Some("release bookkeeping"));
    }

    #[test]
    fn a_namespaced_marker_names_its_gate() {
        assert_eq!(gate_for_directive("discipline:allow(pii)"), None);
        assert_eq!(
            gate_for_directive("discipline:allow(assertion-reduction)"),
            Some("assertion-reduction")
        );
        assert_eq!(
            gate_for_directive("ALLOW-DEPENDENCY"),
            Some("dependency-delta")
        );
        assert_eq!(
            marker_gate("    // discipline:allow(pii): fixture host"),
            Some("pii")
        );
        assert_eq!(
            marker_gate("let h = \"x\"; # discipline:allow(pii): fixture"),
            Some("pii")
        );
        assert_eq!(
            marker_gate("<!-- discipline:allow(time-estimates) -->"),
            Some("time-estimates")
        );
        // A mention is not a marker: string literals, name lists, code spans, prose.
        for mention in [
            r#"    "discipline:allow(pii)","#,
            r#"{"directive":"discipline:allow(pii)"}"#,
            "use `discipline:allow(pii): <reason>` on the line",
            "discipline:allow(pii): at the start of prose",
            "// discipline:allow(no-such-gate): x",
            "no marker here",
        ] {
            assert_eq!(marker_gate(mention), None, "{mention}");
        }
    }

    #[test]
    fn a_configuration_loosening_is_recorded_as_data() {
        let base = "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nexempt_paths = [\"a/**\"]\n";
        let head =
            "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nexempt_paths = [\"a/**\", \"b/**\"]\n";
        let r = config_records(Some(base), Some(head), &info());
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(
            (
                r[0].kind,
                r[0].gate.as_deref(),
                r[0].key.as_deref(),
                r[0].change,
                r[0].count,
                r[0].evidence,
                r[0].tier
            ),
            (
                "config",
                Some("pii"),
                Some("exempt_paths"),
                Some(Change::Gained),
                Some(1),
                "applied",
                "A"
            )
        );
        // Tightening, adopting and leaving it alone record nothing.
        assert!(config_records(Some(head), Some(base), &info()).is_empty());
        assert!(config_records(None, Some(head), &info()).is_empty());
        assert!(config_records(Some(base), Some(base), &info()).is_empty());
        // Removing it leaves the defaults, which is compared too.
        let strict = "[meta]\nversion = 1\nname = \"t\"\n[gates.test-floor]\nenabled = true\nmin_tests = 40\n";
        assert!(!config_records(Some(strict), None, &info()).is_empty());
    }

    #[test]
    fn a_configuration_that_does_not_parse_is_a_record_not_an_error() {
        let base = "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nretired_key = true\n";
        let head = "[meta]\nversion = 1\nname = \"t\"\n";
        let r = config_records(Some(base), Some(head), &info());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, "config-unreadable");
        assert!(
            r[0].detail.as_deref().unwrap().starts_with("parent: "),
            "{r:?}"
        );
    }

    #[test]
    fn baseline_growth_is_counted_per_gate() {
        let entry = |gate: &str, fp: &str| {
            format!("[[findings]]\ngate = \"{gate}\"\nrule = \"r\"\npath = \"a.rs\"\nfingerprint = \"{fp}\"\n")
        };
        let base = format!("version = 2\n{}", entry("pii", "1"));
        let head = format!(
            "version = 2\n{}{}{}",
            entry("pii", "1"),
            entry("pii", "2"),
            entry("stub-bodies", "3")
        );
        let r = baseline_records(Some(&base), Some(&head), &info());
        let got: Vec<(Option<&str>, Option<usize>)> =
            r.iter().map(|r| (r.gate.as_deref(), r.count)).collect();
        assert_eq!(
            got,
            vec![(Some("pii"), Some(1)), (Some("stub-bodies"), Some(1))]
        );
        assert!(baseline_records(Some(&head), Some(&base), &info()).is_empty());
        assert_eq!(baseline_records(None, Some(&base), &info()).len(), 1);
    }

    #[test]
    fn the_summary_counts_by_kind_class_and_gate() {
        let mut records = directive_records(
            "s\n\nno-issue: a\nallow-dependency: serde b",
            &info(),
            false,
        );
        records.extend(directive_records(
            "s\n\nallow-dependency: serde c",
            &ChangeInfoRef {
                sha: "fedcba9876543210".into(),
                pr: None,
                time: 2,
                ord: 1,
                subject: "s".into(),
            },
            false,
        ));
        let s = Summary::from_records("main".into(), 5, records, Vec::new());
        assert_eq!((s.changes, s.changes_with_records), (5, 2));
        assert_eq!(s.by_kind["directive"], 3);
        assert_eq!(s.by_class["process"], 1);
        assert_eq!(s.by_gate["dependency-delta"], 2);
        let text = s.render();
        assert!(
            text.contains("5 changes audited, 2 carried an exception (3 records)"),
            "{text}"
        );
        assert!(text.contains("#7 "), "{text}");
        assert!(text.contains("fedcba9876 "), "{text}");
    }

    /// A change at position `ord` (0 = newest) with an optional pull request.
    fn at(ord: usize, pr: Option<u64>) -> ChangeInfoRef {
        ChangeInfoRef {
            sha: format!("{ord:0>16}"),
            pr,
            time: 100 - ord as i64,
            ord,
            subject: format!("change {ord}"),
        }
    }

    const CFG: &str = "[meta]\nversion = 1\nname = \"t\"\n";

    fn loosen(
        ord: usize,
        pr: Option<u64>,
        table: &str,
        before: &str,
        after: &str,
    ) -> (Vec<Record>, Vec<Record>) {
        config_changes(
            Some(&format!("{CFG}{table}{before}")),
            Some(&format!("{CFG}{table}{after}")),
            &at(ord, pr),
        )
    }

    fn found(records: &[Record], tightenings: &[Record]) -> Vec<(&'static str, Vec<String>)> {
        signals(records, tightenings)
            .0
            .into_iter()
            .map(|s| (s.id, s.changes))
            .collect()
    }

    #[test]
    fn links_follow_the_forge_not_github() {
        use crate::forge::{Forge, ForgeKind};
        let f = |kind, url: &str, repo: &str| {
            Links::for_forge(&Forge {
                kind,
                url: url.into(),
                repo: repo.into(),
            })
        };
        let gh = f(ForgeKind::GitHub, "https://github.com", "o/r");
        assert_eq!(gh.pull, "https://github.com/o/r/pull/{n}");
        assert_eq!(gh.file, "https://github.com/o/r/blob/{sha}/{path}#L{line}");
        assert!(gh.file_diff.is_some());
        let gl = f(ForgeKind::GitLab, "https://gitlab.example.com", "g/sub/r");
        assert_eq!(
            gl.pull,
            "https://gitlab.example.com/g/sub/r/-/merge_requests/{n}"
        );
        assert_eq!(
            gl.commit,
            "https://gitlab.example.com/g/sub/r/-/commit/{sha}"
        );
        assert_eq!(
            gl.file,
            "https://gitlab.example.com/g/sub/r/-/blob/{sha}/{path}#L{line}"
        );
        assert_eq!(gl.file_diff, None);
        for kind in [ForgeKind::Gitea, ForgeKind::Forgejo] {
            let g = f(kind, "https://code.example.org", "o/r");
            assert_eq!(g.pull, "https://code.example.org/o/r/pulls/{n}");
            assert_eq!(
                g.file,
                "https://code.example.org/o/r/src/commit/{sha}/{path}#L{line}"
            );
            assert_eq!(g.file_diff, None);
        }
    }

    #[test]
    fn a_loosened_option_links_to_its_line() {
        let text = "[meta]\nversion = 1\n[gates.pii]\nenabled = true\nexempt_paths = [\"a\"]\n[gates.stub-bodies]\nexempt_paths = []\n[languages.c]\nmacros = [\"X\"]\n";
        assert_eq!(key_line(text, "pii", "exempt_paths"), Some(5));
        assert_eq!(key_line(text, "stub-bodies", "exempt_paths"), Some(7));
        assert_eq!(key_line(text, "languages", "c.macros"), Some(9));
        assert_eq!(key_line(text, "pii", "severity"), None);
        // `exempt_paths_extra` is not `exempt_paths`.
        assert_eq!(
            key_line(
                "[gates.pii]\nexempt_paths_extra = 1\n",
                "pii",
                "exempt_paths"
            ),
            None
        );
        let (l, _) = config_changes(
            Some("[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nexempt_paths = []\n"),
            Some("[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nexempt_paths = [\"a\"]\n"),
            &info(),
        );
        assert_eq!(l[0].line, Some(5));
    }

    #[test]
    fn a_tightening_is_a_loosening_read_backwards() {
        let (l, t) = loosen(
            0,
            Some(9),
            "[gates.pii]\n",
            "exempt_paths = [\"a\", \"b\"]\n",
            "exempt_paths = [\"a\"]\n",
        );
        assert!(l.is_empty(), "{l:?}");
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(
            (t[0].kind, t[0].key.as_deref(), t[0].count, t[0].change),
            ("config-tightening", Some("exempt_paths"), Some(1), None)
        );
        let (l, t) = loosen(
            0,
            Some(9),
            "[gates.test-floor]\nenabled = true\n",
            "min_tests = 3\n",
            "min_tests = 40\n",
        );
        assert!(l.is_empty());
        // Values in the change's own direction: it raised the floor from 3 to 40.
        assert_eq!(
            (t[0].before.as_deref(), t[0].after.as_deref()),
            (Some("3"), Some("40"))
        );
    }

    #[test]
    fn each_signal_fires_on_its_case_and_not_on_its_control() {
        let waiver = |ord, pr| {
            directive_records(
                "s\n\nallow-gate-weakening: ratified-paths reviewed",
                &at(ord, pr),
                false,
            )
        };
        // Newest first: #5 loosens a guard gate with a waiver; 0000000000000003 (no pull
        // request) loosens pii; #2 tightens pii back; #1 waives dependency-delta, which
        // #4 then loosens with no waiver.
        let mut records = Vec::new();
        let mut tightenings = Vec::new();
        records.extend(waiver(0, Some(5)));
        records.extend(
            loosen(
                0,
                Some(5),
                "[gates.ratified-paths]\n",
                "ratifiers = [\"a\"]\n",
                "ratifiers = [\"a\", \"b\"]\n",
            )
            .0,
        );
        records.extend(
            loosen(
                1,
                Some(4),
                "[gates.dependency-delta]\n",
                "allow_dependencies = [\"y\"]\n",
                "allow_dependencies = [\"y\", \"x\"]\n",
            )
            .0,
        );
        records.extend(
            loosen(
                2,
                None,
                "[gates.pii]\n",
                "exempt_paths = []\n",
                "exempt_paths = [\"a\"]\n",
            )
            .0,
        );
        let (_, t) = loosen(
            1,
            Some(2),
            "[gates.pii]\n",
            "exempt_paths = [\"a\"]\n",
            "exempt_paths = []\n",
        );
        tightenings.extend(t);
        records.extend(directive_records(
            "s\n\nallow-dependency: x needed",
            &at(3, Some(1)),
            false,
        ));
        let got = found(&records, &tightenings);
        let ids: Vec<&str> = got.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec![
                "guard-gate-loosened",
                "loosened-without-pull-request",
                "loosening-without-waiver",
                "waived-then-loosened",
                "loosened-not-restored",
            ],
            "{got:?}"
        );
        let changes = |id: &str| got.iter().find(|(i, _)| *i == id).unwrap().1.clone();
        assert_eq!(changes("guard-gate-loosened"), vec!["#5"]);
        assert_eq!(changes("loosened-without-pull-request"), vec!["0000000000"]);
        // #5 carries its waiver; #4 does not.
        assert_eq!(changes("loosening-without-waiver"), vec!["#4"]);
        assert_eq!(changes("waived-then-loosened"), vec!["#4"]);
        // The pii loosening was tightened back by a newer change; the others were not.
        assert_eq!(changes("loosened-not-restored"), vec!["#5", "#4"]);

        // A tightening older than the loosening does not restore it.
        let mut old = tightenings.clone();
        old[0].ord = 9;
        assert!(changes_of(&found(&records, &old), "loosened-not-restored")
            .contains(&"0000000000".to_string()));
        // A waiver newer than the loosening is not "waived, then loosened".
        let mut later = records.clone();
        for r in later
            .iter_mut()
            .filter(|r| r.kind == "directive" && r.pr == Some(1))
        {
            r.ord = 0;
        }
        assert!(!found(&later, &tightenings)
            .iter()
            .any(|(id, _)| *id == "waived-then-loosened"));
    }

    fn changes_of(got: &[(&'static str, Vec<String>)], id: &str) -> Vec<String> {
        got.iter()
            .find(|(i, _)| *i == id)
            .map(|(_, c)| c.clone())
            .unwrap_or_default()
    }

    #[test]
    fn hidden_unreadable_and_baseline_signals_and_their_controls() {
        let hidden = directive_records(
            "s\n\n<!-- allow-stub: fn_a later -->",
            &at(0, Some(3)),
            false,
        );
        let shown = directive_records("s\n\nallow-stub: fn_a later", &at(0, Some(3)), false);
        assert_eq!(
            changes_of(&found(&hidden, &[]), "hidden-directive"),
            vec!["#3"]
        );
        assert!(changes_of(&found(&shown, &[]), "hidden-directive").is_empty());
        let unreadable = config_records(
            Some(&format!("{CFG}[gates.pii]\nretired = 1\n")),
            Some(CFG),
            &at(0, Some(3)),
        );
        assert_eq!(
            changes_of(&found(&unreadable, &[]), "config-unreadable"),
            vec!["#3"]
        );
        let entry =
            "[[findings]]\ngate = \"pii\"\nrule = \"r\"\npath = \"a\"\nfingerprint = \"\"\n";
        let grew = baseline_records(
            None,
            Some(&format!("version = 2\n{entry}")),
            &at(0, Some(3)),
        );
        let shrank = baseline_records(
            Some(&format!("version = 2\n{entry}")),
            None,
            &at(0, Some(3)),
        );
        assert_eq!(changes_of(&found(&grew, &[]), "baseline-grew"), vec!["#3"]);
        assert!(shrank.is_empty());
    }

    #[test]
    fn every_question_has_a_state_and_nothing_found_reads_as_clean_not_as_absent() {
        let (sigs, checks) = signals(&[], &[]);
        assert!(sigs.is_empty());
        assert_eq!(checks.len(), SIGNALS.len() + NOT_CHECKED.len());
        assert!(checks[..SIGNALS.len()].iter().all(|c| c.state == "clean"));
        assert!(checks[SIGNALS.len()..]
            .iter()
            .all(|c| c.state == "not-checked" && !c.detail.is_empty()));
        let text = Summary::from_records("main".into(), 3, Vec::new(), Vec::new()).render();
        assert!(
            text.contains("Needs a decision: nothing the git history shows."),
            "{text}"
        );
        assert!(text.contains("not-checked owner-ratification"), "{text}");
    }
}
