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
    /// Read each change's merged pull request from the forge: its body's directives and
    /// whether a login other than its author approved it.
    pub forge: bool,
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
    /// Where a directive was read: `commit-message` or `pull-request-body`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    /// A protected edit's ratification, read with `--forge`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratification: Option<RatificationFact>,
    /// Issue references in a directive's reason (`#12`, `owner/repo#12`, an issue URL),
    /// as written; with `--forge` each is read into `issues`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cites: Vec<String>,
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
            source: None,
            ratification: None,
            cites: Vec::new(),
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
    /// Indexes into the list `list` names.
    pub records: Vec<usize>,
    /// `records`, or `protected_edits` for the ratification signals.
    pub list: &'static str,
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
        "pull request bodies are read with `--forge`",
    ),
    (
        "owner-ratification",
        "ratification comments live on issues: run with `--forge`",
    ),
    (
        "cited-issues",
        "the issues waivers cite are read with `--forge`",
    ),
    (
        "independent-review",
        "reviews live on the forge: run with `--forge`",
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
            detail: format!(
                "{} in {}",
                hits.len(),
                crate::audit_html::plural(changes.len(), "change", "changes")
            ),
        });
        found.push(Signal {
            id: def.id,
            rank: def.rank,
            count: hits.len(),
            changes,
            records: hits,
            list: "records",
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
    /// A comment on an issue, with `{repo}` (`owner/name`), `{n}` and `{id}`.
    pub issue_comment: String,
    /// An issue's page, with `{repo}` and `{n}`.
    pub issue: String,
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
        let issue_comment = match f.kind {
            GitHub | Gitea | Forgejo => "issues/{n}#issuecomment-{id}",
            GitLab => "-/issues/{n}#note_{id}",
        };
        Links {
            issue_comment: format!("{}/{{repo}}/{issue_comment}", f.url),
            issue: format!(
                "{}/{{repo}}/{}",
                f.url,
                if f.kind == GitLab {
                    "-/issues/{n}"
                } else {
                    "issues/{n}"
                }
            ),
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
    /// Each change's merged pull request, read with `--forge`; empty otherwise.
    pub pulls: Vec<Pull>,
    /// The issues waivers cite, read with `--forge`; empty otherwise.
    pub issues: Vec<IssueFact>,
    /// What `--forge` read; `None` without it.
    pub forge: Option<ForgeRead>,
}

/// A change's merged pull request, as the forge reported it. Logins are not carried.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Pull {
    pub sha: String,
    pub pr: u64,
    /// A login other than the pull request's author approved its head.
    pub approved_by_other: bool,
    /// The author's login and the body, for the ratification judgement; never reported.
    #[serde(skip)]
    pub author: String,
    #[serde(skip)]
    pub body: String,
}

/// Whether a protected edit was ratified, as `ratified-paths` judges it on the forge.
/// The ratifier's login is not carried; `self-ratified` compares it with the pull
/// request's author.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RatificationFact {
    /// `ratified`, `self-ratified` (by the pull request's own author login),
    /// `unratified`, `not-required` (the gate was off in the parent configuration),
    /// `never-ratifiable`, or `not-checked` (no pull request read, or the forge failed).
    pub state: &'static str,
    /// The issue carrying the ratifying comment, `owner/name`, and its number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// When the ratifying comment was posted, seconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    /// Why it is unratified or not checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// How the forge reads of `--forge` went.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ForgeRead {
    /// Changes whose merged pull request was looked up.
    pub changes: usize,
    /// Of those, the ones that arrived through a merged pull request.
    pub pulls: usize,
    /// Changes the forge could not answer for.
    pub failed: usize,
    /// The first error, when one failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The changes that failed, for the ratification judgement.
    #[serde(skip)]
    pub failed_shas: Vec<String>,
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
                "\nProtected paths: {} in {}; ratification not checked (needs the forge)\n",
                crate::audit_html::plural(self.protected_edits.len(), "edit", "edits"),
                crate::audit_html::plural(changes.len(), "change", "changes")
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
                "baseline" => format!(
                    "{} grandfathered",
                    crate::audit_html::plural(r.count.unwrap_or(0), "finding", "findings")
                ),
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
    directives_from(
        message,
        crate::tokens::OverrideSource::Commit(info.sha.clone()),
        "commit-message",
        info,
        reasons,
    )
}

/// The directive records of a change's merged pull-request body, less those its commit
/// message already carries (a squash merge often copies the body into the message).
pub fn pull_body_records(
    body: &str,
    info: &ChangeInfoRef,
    reasons: bool,
    from_commit: &[Record],
) -> Vec<Record> {
    directives_from(
        body,
        crate::tokens::OverrideSource::PrBody,
        "pull-request-body",
        info,
        reasons,
    )
    .into_iter()
    .filter(|r| {
        !from_commit.iter().any(|c| {
            c.kind == "directive"
                && c.directive.as_deref().map(str::to_ascii_lowercase)
                    == r.directive.as_deref().map(str::to_ascii_lowercase)
                && c.reason_sha256 == r.reason_sha256
        })
    })
    .collect()
}

fn directives_from(
    text: &str,
    origin: crate::tokens::OverrideSource,
    source: &'static str,
    info: &ChangeInfoRef,
    reasons: bool,
) -> Vec<Record> {
    let change = info.to_owned();
    crate::tokens::parse_directives(text, origin)
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
            r.source = Some(source);
            r.cites = crate::references::parse(&d.reason, crate::forge::ForgeKind::GitHub, &[])
                .into_iter()
                .filter(|x| !x.pull)
                .map(|x| x.text)
                .collect();
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
    let forge = detect_forge(&repo);
    let (pulls, read) = if opts.forge {
        let f = forge.as_ref().map_err(|e| {
            crate::could_not_check::tag(
                crate::could_not_check::Reason::Forge,
                anyhow!("--forge: the forge cannot be identified: {e}"),
            )
        })?;
        // Each change costs a pull-request read and a review read, plus the ratification
        // and cited-issue reads of the few that need them.
        crate::forge::raise_request_limit(crate::forge::MAX_REQUESTS + commits.len() * 4);
        let api = crate::forge::HttpApi::from_env();
        let changes: Vec<ChangeInfoRef> = commits
            .iter()
            .enumerate()
            .map(|(ord, c)| ChangeInfoRef {
                sha: c.id().to_string(),
                pr: None,
                time: c.time().seconds(),
                ord,
                subject: c.summary().ok().flatten().unwrap_or("").to_string(),
            })
            .collect();
        let (pulls, read, body) = read_pulls(&api, f, &changes, &records, opts.reasons);
        // The pull request a change arrived through names changes whose subject does not.
        for r in records
            .iter_mut()
            .chain(&mut tightenings)
            .chain(&mut protected)
        {
            if r.pr.is_none() {
                r.pr = pulls.iter().find(|p| p.sha == r.sha).map(|p| p.pr);
            }
        }
        records.extend(body);
        records.sort_by_key(|r| r.ord);
        (pulls, Some(read))
    } else {
        (Vec::new(), None)
    };
    if let (true, Ok(f), Some(read)) = (opts.forge, &forge, &read) {
        let api = crate::forge::HttpApi::from_env();
        judge_protected(&repo, &api, f, &mut protected, &pulls, &read.failed_shas)?;
    }
    let mut s = Summary::from_records(reference, commits.len(), records, tightenings);
    s.tip = tip.to_string();
    let issues = match (opts.forge, &forge) {
        (true, Ok(f)) => read_issues(&crate::forge::HttpApi::from_env(), f, &s.records),
        _ => Vec::new(),
    };
    if read.is_some() {
        s.signals.extend(protected_signals(&protected));
        s.signals.extend(citation_signals(&s.records, &issues));
        issues_check(&mut s.checks, &s.records, &issues);
        s.signals.sort_by_key(|g| rank_order(g.rank));
        ratification_check(&mut s.checks, &protected);
    }
    s.protected_edits = protected;
    s.links = forge.ok().map(|f| Links::for_forge(&f));
    if let Some(read) = read {
        forge_checks(&mut s.checks, &read, &pulls, &s.records);
        s.forge = Some(read);
    }
    s.pulls = pulls;
    s.issues = issues;
    Ok(s)
}

/// Why a ratifying issue does not count: it was closed before the merge, so the gate
/// that required it open could not have accepted it. `None` when it stands; a reason
/// starting `could not` when the forge could not say.
fn closed_before_merge(
    api: &dyn crate::forge::ForgeApi,
    forge: &crate::forge::Forge,
    repo: &str,
    issue: u64,
    merged: i64,
) -> Option<String> {
    match crate::references::issue_facts(api, forge, repo, issue, false) {
        Err(e) => Some(format!("could not read issue #{issue}: {e}")),
        // The merge closes it at once; the margin allows for clock skew.
        Ok(f) => f
            .closed_at
            .filter(|t| *t < merged - 120)
            .map(|_| format!("issue #{issue} was closed before the merge")),
    }
}

/// One issue a waiver cites, as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IssueFact {
    /// The reference as written in a reason (`#12`, `o/r#12`, a URL).
    pub reference: String,
    /// The repository it was looked up in (`owner/name`); empty for another repository.
    pub repo: String,
    pub number: u64,
    /// `open`, `closed`, `not-found`, `pull-request`, `cross-repo` (not looked up), or
    /// `not-checked` (the forge could not answer, reason in `error`).
    pub state: &'static str,
    /// GitHub's close reason (`completed`, `not_planned`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    /// When it was closed, seconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Read each issue the waivers cite once. References to other repositories are not read.
pub fn read_issues(
    api: &dyn crate::forge::ForgeApi,
    forge: &crate::forge::Forge,
    records: &[Record],
) -> Vec<IssueFact> {
    use crate::references::Verdict;
    let mut out: Vec<IssueFact> = Vec::new();
    for text in records.iter().flat_map(|r| r.cites.iter()) {
        if out.iter().any(|f| &f.reference == text) {
            continue;
        }
        let Some(reference) = crate::references::parse(text, forge.kind, &[])
            .into_iter()
            .next()
        else {
            continue;
        };
        let Some(repo) = crate::references::locate(&reference, forge, &[]) else {
            out.push(IssueFact {
                reference: text.clone(),
                repo: String::new(),
                number: reference.number,
                state: "cross-repo",
                state_reason: None,
                closed_at: None,
                error: None,
            });
            continue;
        };
        // The same issue written another way is read once.
        if let Some(prev) = out
            .iter()
            .find(|f| f.repo.eq_ignore_ascii_case(&repo) && f.number == reference.number)
            .cloned()
        {
            out.push(IssueFact {
                reference: text.clone(),
                ..prev
            });
            continue;
        }
        let fact = match crate::references::issue_facts(api, forge, &repo, reference.number, false)
        {
            Ok(f) => IssueFact {
                reference: text.clone(),
                repo,
                number: reference.number,
                state: match f.verdict {
                    Verdict::Issue => "open",
                    Verdict::Closed => "closed",
                    Verdict::NotFound => "not-found",
                    Verdict::IsPull => "pull-request",
                    Verdict::CrossRepo => "cross-repo",
                },
                state_reason: f.state_reason,
                closed_at: f.closed_at,
                error: None,
            },
            Err(e) => IssueFact {
                reference: text.clone(),
                repo,
                number: reference.number,
                state: "not-checked",
                state_reason: None,
                closed_at: None,
                error: Some(e.to_string()),
            },
        };
        out.push(fact);
    }
    out
}

/// Whether a waiver and the issue it cites make a citation signal.
type CiteTest = dyn Fn(&Record, &IssueFact) -> bool;

/// The citation signals: a waiver whose cited issue does not exist, was closed as not
/// planned, or was already closed when the waiver was written.
pub fn citation_signals(records: &[Record], issues: &[IssueFact]) -> Vec<Signal> {
    let fact = |t: &str| issues.iter().find(|f| f.reference == t);
    let defs: [(&str, &str, &str, &CiteTest); 3] = [
        (
            "waiver-cites-missing-issue",
            "look-soon",
            "Open the waiver: the issue its reason cites does not exist, so nothing tracks the follow-up.",
            &|_, f| f.state == "not-found",
        ),
        (
            "waiver-cites-issue-closed-before",
            "look-soon",
            "Open the waiver: the issue its reason cites was already closed when it was written, so it tracks nothing.",
            // A finding waiver's cited issue is its promise to follow up; a skipped issue
            // link citing a closed issue is context ("follows #12").
            &|r, f| {
                r.class == "detector"
                    && f.state == "closed"
                    && f.closed_at.is_some_and(|t| t < r.time - 60)
            },
        ),
        (
            "waiver-cites-issue-not-planned",
            "review",
            "Decide what replaces the follow-up: the issue the waiver cites was closed as not planned.",
            &|r, f| {
                r.class == "detector"
                    && f.state == "closed"
                    && f.state_reason.as_deref() == Some("not_planned")
                    && !f.closed_at.is_some_and(|t| t < r.time - 60)
            },
        ),
    ];
    let mut out = Vec::new();
    for (id, rank, next, test) in defs {
        let hits: Vec<usize> = (0..records.len())
            .filter(|&i| {
                records[i]
                    .cites
                    .iter()
                    .filter_map(|t| fact(t))
                    .any(|f| test(&records[i], f))
            })
            .collect();
        if hits.is_empty() {
            continue;
        }
        let mut changes: Vec<String> = Vec::new();
        for &i in &hits {
            let l = records[i].label();
            if !changes.contains(&l) {
                changes.push(l);
            }
        }
        out.push(Signal {
            id,
            rank,
            count: hits.len(),
            changes,
            records: hits,
            list: "records",
            next,
        });
    }
    out
}

/// The `cited-issues` check once `--forge` read them.
pub fn issues_check(checks: &mut [Check], records: &[Record], issues: &[IssueFact]) {
    let Some(c) = checks.iter_mut().find(|c| c.id == "cited-issues") else {
        return;
    };
    let citing = records.iter().filter(|r| !r.cites.is_empty()).count();
    if let Some(f) = issues.iter().find(|f| f.state == "not-checked") {
        c.state = "not-checked";
        c.detail = format!(
            "could not read {}: {}",
            f.reference,
            f.error.as_deref().unwrap_or("")
        );
        return;
    }
    let bad = citation_signals(records, issues)
        .iter()
        .map(|s| s.count)
        .sum::<usize>();
    let waivers = crate::audit_html::plural(citing, "waiver", "waivers");
    if bad > 0 {
        c.state = "found";
        c.detail = format!("{bad} of {waivers} citing an issue point at one missing, closed before, or not planned");
    } else {
        c.state = "clean";
        c.detail = if citing == 0 {
            "no waiver cites an issue".to_string()
        } else {
            format!("every one of {waivers} citing an issue points at one that tracks it")
        };
    }
}

/// The merged pull request of each change: its body's directives, and whether another
/// login approved it. A change the forge cannot answer for is counted, never guessed.
pub fn read_pulls(
    api: &dyn crate::forge::ForgeApi,
    forge: &crate::forge::Forge,
    changes: &[ChangeInfoRef],
    records: &[Record],
    reasons: bool,
) -> (Vec<Pull>, ForgeRead, Vec<Record>) {
    let mut pulls = Vec::new();
    let mut body_records = Vec::new();
    let mut read = ForgeRead::default();
    for c in changes {
        read.changes += 1;
        let found =
            crate::forge::merged_pull_for_commit(api, forge, &c.sha).and_then(|m| match m {
                None => Ok(None),
                Some(m) => crate::forge::pull_approvers(api, forge, m.number, &m.head_sha)
                    .map(|approvers| Some((m, approvers))),
            });
        match found {
            Ok(None) => {}
            Ok(Some((m, approvers))) => {
                read.pulls += 1;
                let info = ChangeInfoRef {
                    pr: Some(m.number),
                    ..c.clone()
                };
                let from_commit: Vec<Record> =
                    records.iter().filter(|r| r.sha == c.sha).cloned().collect();
                body_records.extend(pull_body_records(&m.body, &info, reasons, &from_commit));
                pulls.push(Pull {
                    sha: c.sha.clone(),
                    pr: m.number,
                    approved_by_other: approvers.iter().any(|a| !a.eq_ignore_ascii_case(&m.author)),
                    author: m.author.clone(),
                    body: m.body.clone(),
                });
            }
            Err(e) => {
                read.failed += 1;
                read.failed_shas.push(c.sha.clone());
                read.error.get_or_insert(e);
            }
        }
    }
    (pulls, read, body_records)
}

/// When `path` last changed on the first-parent line ending at `from`: the committer
/// time of the newest commit whose tree gives the path another blob than its parent's.
/// The start of the `path-last-changed` ratification window.
pub fn last_change_before(repo: &Repository, from: Oid, path: &str) -> Result<Option<i64>> {
    let p = std::path::Path::new(path);
    let blob =
        |c: &Commit| -> Result<Option<Oid>> { Ok(c.tree()?.get_path(p).ok().map(|e| e.id())) };
    let mut cur = repo.find_commit(from)?;
    loop {
        let here = blob(&cur)?;
        let Ok(parent) = cur.parent(0) else {
            return Ok(here.map(|_| cur.time().seconds()));
        };
        if here != blob(&parent)? {
            return Ok(Some(cur.time().seconds()));
        }
        cur = parent;
    }
}

/// Judge each protected edit as `ratified-paths` does, against the change's parent
/// configuration and its merged pull request. The forge answers per change; a change
/// the forge could not answer for is `not-checked`.
pub fn judge_protected(
    repo: &Repository,
    api: &dyn crate::forge::ForgeApi,
    forge: &crate::forge::Forge,
    protected: &mut [Record],
    pulls: &[Pull],
    failed: &[String],
) -> Result<()> {
    let shas: std::collections::BTreeSet<String> =
        protected.iter().map(|r| r.sha.clone()).collect();
    for sha in shas {
        let commit = repo.find_commit(Oid::from_str(&sha)?)?;
        let parent = commit.parent(0)?;
        let cfg = blob_text(repo, &parent.tree()?, CONFIG_NAME)?
            .and_then(|t| DisciplineConfig::from_toml_str(&t).ok())
            .map(|c| c.gates.ratified_paths)
            .unwrap_or_default();
        // The audit reads after the merge, which closed the issues the pull request
        // closes: an issue that was open when the gate ran is closed now. An issue closed
        // before the merge is not told apart until the audit reads issue timelines.
        let cfg = crate::config::RatifiedPathsGate {
            require_open_issue: false,
            ..cfg
        };
        let never = crate::guards::PathFilter::new(&cfg.never_ratifiable)?;
        let fact = |state: &'static str, why: Option<String>| RatificationFact {
            state,
            issue_repo: None,
            issue: None,
            comment_id: None,
            created: None,
            why,
        };
        let mine: Vec<&mut Record> = protected.iter_mut().filter(|r| r.sha == sha).collect();
        let paths: Vec<String> = mine
            .iter()
            .filter_map(|r| r.file.clone())
            .filter(|p| !never.matches(p))
            .collect();
        let verdicts: std::collections::BTreeMap<String, RatificationFact> = if !cfg.enabled {
            Default::default()
        } else if let Some(pull) = pulls.iter().find(|p| p.sha == sha) {
            let last_change = |p: &str| last_change_before(repo, parent.id(), p);
            let judged = crate::ratification::judge(
                api,
                forge,
                &cfg,
                &crate::ratification::Input {
                    pull_number: pull.pr,
                    pull_body: &pull.body,
                    protected: &paths,
                    never_ratifiable: &never,
                    last_change: &last_change,
                    now: commit.time().seconds(),
                },
            );
            match judged {
                Err(e) => paths
                    .iter()
                    .map(|p| (p.clone(), fact("not-checked", Some(e.to_string()))))
                    .collect(),
                Ok(j) => paths
                    .iter()
                    .map(|p| {
                        let v = match j.ratified.iter().find(|r| &r.path == p) {
                            Some(r) => {
                                let early = closed_before_merge(
                                    api,
                                    forge,
                                    &r.repo,
                                    r.issue,
                                    commit.time().seconds(),
                                );
                                let state = match &early {
                                    Some(why) if why.starts_with("could not") => "not-checked",
                                    Some(_) => "unratified",
                                    None if r.author.eq_ignore_ascii_case(&pull.author) => {
                                        "self-ratified"
                                    }
                                    None => "ratified",
                                };
                                RatificationFact {
                                    state,
                                    issue_repo: Some(r.repo.clone()),
                                    issue: Some(r.issue),
                                    comment_id: Some(r.comment_id.clone()),
                                    created: Some(r.created_at),
                                    why: early,
                                }
                            }
                            None => fact(
                                "unratified",
                                j.findings.iter().find_map(|f| match f {
                                    crate::ratification::Finding::Unratified { path, why }
                                        if path == p =>
                                    {
                                        Some(why.clone())
                                    }
                                    _ => None,
                                }),
                            ),
                        };
                        (p.clone(), v)
                    })
                    .collect(),
            }
        } else if failed.contains(&sha) {
            paths
                .iter()
                .map(|p| {
                    (
                        p.clone(),
                        fact(
                            "not-checked",
                            Some("the forge could not answer for this change".into()),
                        ),
                    )
                })
                .collect()
        } else {
            paths
                .iter()
                .map(|p| (p.clone(), fact("unratified", Some("the change arrived with no pull request, so nothing could ratify it".into()))))
                .collect()
        };
        for r in mine {
            let path = r.file.clone().unwrap_or_default();
            r.ratification = Some(if !cfg.enabled {
                fact("not-required", None)
            } else if never.matches(&path) {
                fact("never-ratifiable", None)
            } else {
                verdicts
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| fact("not-checked", None))
            });
        }
    }
    Ok(())
}

fn rank_order(rank: &str) -> u8 {
    match rank {
        "look-first" => 0,
        "look-soon" => 1,
        _ => 2,
    }
}

/// The ratification signals, over the protected edits `--forge` judged.
pub fn protected_signals(protected: &[Record]) -> Vec<Signal> {
    let mut out = Vec::new();
    for (id, rank, state, next) in [
        (
            "protected-edit-unratified",
            "look-first",
            "unratified",
            "Open the change: a protected path was edited with no ratification the gate accepts.",
        ),
        (
            "protected-edit-self-ratified",
            "look-soon",
            "self-ratified",
            "Read the change: its protected edit was ratified by the pull request's own author login, which is not a second party.",
        ),
    ] {
        let hits: Vec<usize> = (0..protected.len())
            .filter(|&i| protected[i].ratification.as_ref().is_some_and(|f| f.state == state))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let mut changes: Vec<String> = Vec::new();
        for &i in &hits {
            let l = protected[i].label();
            if !changes.contains(&l) {
                changes.push(l);
            }
        }
        out.push(Signal {
            id,
            rank,
            count: hits.len(),
            changes,
            records: hits,
            list: "protected_edits",
            next,
        });
    }
    out
}

/// The owner-ratification check, once `--forge` judged the protected edits.
pub fn ratification_check(checks: &mut [Check], protected: &[Record]) {
    let Some(c) = checks.iter_mut().find(|c| c.id == "owner-ratification") else {
        return;
    };
    let states: Vec<&str> = protected
        .iter()
        .filter_map(|r| r.ratification.as_ref().map(|f| f.state))
        .collect();
    let count = |s: &str| states.iter().filter(|x| **x == s).count();
    let edits = crate::audit_html::plural(states.len(), "protected edit", "protected edits");
    if let Some(why) = protected
        .iter()
        .filter_map(|r| r.ratification.as_ref())
        .find(|f| f.state == "not-checked")
        .map(|f| f.why.clone().unwrap_or_default())
    {
        c.state = "not-checked";
        c.detail = format!(
            "{} of {edits} could not be judged: {why}",
            count("not-checked")
        );
    } else if count("unratified") + count("self-ratified") > 0 {
        c.state = "found";
        c.detail = format!(
            "{} unratified and {} ratified by the pull request's own author login, of {edits}",
            count("unratified"),
            count("self-ratified")
        );
    } else {
        c.state = "clean";
        c.detail = if states.is_empty() {
            "no protected-path edit".to_string()
        } else {
            format!("every one of {edits} ratified by another login, or not required")
        };
    }
}

/// The checks `--forge` answers: pull-request-body directives and independent review.
/// A read that failed for any change leaves them not checked, with the reason.
pub fn forge_checks(checks: &mut [Check], read: &ForgeRead, pulls: &[Pull], records: &[Record]) {
    let set = |checks: &mut [Check], id: &str, state: &'static str, detail: String| {
        if let Some(c) = checks.iter_mut().find(|c| c.id == id) {
            c.state = state;
            c.detail = detail;
        }
    };
    if read.failed > 0 {
        let why = format!(
            "the forge could not answer for {} of {} changes: {}",
            read.failed,
            read.changes,
            read.error.as_deref().unwrap_or("")
        );
        set(
            checks,
            "pull-request-body-directives",
            "not-checked",
            why.clone(),
        );
        set(checks, "independent-review", "not-checked", why);
        return;
    }
    let n = pulls.len();
    let prs = crate::audit_html::plural(n, "pull request", "pull requests");
    let body = records
        .iter()
        .filter(|r| r.source == Some("pull-request-body"))
        .count();
    if body > 0 {
        set(
            checks,
            "pull-request-body-directives",
            "found",
            format!("{body} in {prs}, beyond the commit messages"),
        );
    } else {
        set(
            checks,
            "pull-request-body-directives",
            "clean",
            format!("none beyond the commit messages, in {prs}"),
        );
    }
    let unreviewed = pulls.iter().filter(|p| !p.approved_by_other).count();
    if n == 0 {
        set(
            checks,
            "independent-review",
            "clean",
            "no change arrived through a pull request".to_string(),
        );
    } else if unreviewed > 0 {
        set(
            checks,
            "independent-review",
            "found",
            format!("{unreviewed} of {prs} merged with no approval from another login"),
        );
    } else {
        set(
            checks,
            "independent-review",
            "clean",
            format!("every one of {prs} approved by another login"),
        );
    }
}

/// The forge the `origin` remote names, the same detection `replay` uses; nothing is read
/// from the network.
fn detect_forge(repo: &Repository) -> Result<crate::forge::Forge, String> {
    let origin = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().ok().map(str::to_string))
        .map(|o| crate::forge::resolve_ssh_alias(&o, &|a| crate::forge::ssh_hostname_from_home(a)));
    crate::forge::detect(&|k| std::env::var(k).ok(), origin.as_deref())
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

    fn canned(entries: &[(&str, serde_json::Value)]) -> crate::forge::CannedApi {
        let mut api = crate::forge::CannedApi::default();
        for (k, v) in entries {
            api.responses.insert((*k).to_string(), v.clone());
        }
        api
    }

    fn github() -> crate::forge::Forge {
        crate::forge::Forge {
            kind: crate::forge::ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }
    }

    #[test]
    fn pull_requests_add_body_directives_and_say_who_approved() {
        let a = at(0, None);
        let b = at(1, None);
        let pr = |n: u64, body: &str| serde_json::json!([{"number": n, "merged_at": "2026-09-20T00:00:00Z", "user": {"login": "dev"}, "body": body, "head": {"sha": format!("h{n}")}}]);
        let api = canned(&[
            (
                &format!("github:repos/o/r/commits/{}/pulls", a.sha),
                pr(
                    8,
                    "Refs #1\n\nallow-dependency: serde parser\nno-issue: bookkeeping\n",
                ),
            ),
            (
                "github:repos/o/r/pulls/8/reviews?per_page=100",
                serde_json::json!([
                    {"user": {"login": "dev"}, "state": "APPROVED", "commit_id": "h8"},
                    {"user": {"login": "lead"}, "state": "APPROVED", "commit_id": "h8"}
                ]),
            ),
            // #9's only approval is its author's own, on its head.
            (
                &format!("github:repos/o/r/commits/{}/pulls", b.sha),
                pr(9, "no directive here"),
            ),
            (
                "github:repos/o/r/pulls/9/reviews?per_page=100",
                serde_json::json!([
                    {"user": {"login": "DEV"}, "state": "APPROVED", "commit_id": "h9"}
                ]),
            ),
        ]);
        // The commit message already carries the `no-issue` line: only the other is new.
        let from_commit = directive_records("s\n\nno-issue: bookkeeping", &a, false);
        let (pulls, read, body) = read_pulls(&api, &github(), &[a.clone(), b], &from_commit, false);
        assert_eq!((read.changes, read.pulls, read.failed), (2, 2, 0));
        assert_eq!(
            pulls
                .iter()
                .map(|p| (p.pr, p.approved_by_other))
                .collect::<Vec<_>>(),
            vec![(8, true), (9, false)]
        );
        assert_eq!(body.len(), 1, "{body:?}");
        assert_eq!(
            (
                body[0].directive.as_deref(),
                body[0].source,
                body[0].pr,
                body[0].tier
            ),
            (
                Some("allow-dependency"),
                Some("pull-request-body"),
                Some(8),
                "C"
            )
        );
        let mut checks = signals(&body, &[]).1;
        forge_checks(&mut checks, &read, &pulls, &body);
        let state = |id: &str| {
            checks
                .iter()
                .find(|c| c.id == id)
                .map(|c| (c.state, c.detail.clone()))
                .unwrap()
        };
        assert_eq!(state("pull-request-body-directives").0, "found");
        assert_eq!(
            state("independent-review"),
            (
                "found",
                "1 of 2 pull requests merged with no approval from another login".to_string()
            )
        );
    }

    #[test]
    fn a_forge_that_cannot_answer_leaves_the_questions_not_checked() {
        let a = at(0, None);
        let api = canned(&[(
            &format!("github:repos/o/r/commits/{}/pulls", a.sha),
            serde_json::json!({"__status": 500}),
        )]);
        let (pulls, read, body) = read_pulls(&api, &github(), &[a], &[], false);
        assert!(pulls.is_empty() && body.is_empty());
        assert_eq!((read.changes, read.failed), (1, 1));
        let mut checks = signals(&[], &[]).1;
        forge_checks(&mut checks, &read, &pulls, &[]);
        let review = checks
            .iter()
            .find(|c| c.id == "independent-review")
            .unwrap();
        assert_eq!(review.state, "not-checked");
        assert!(
            review
                .detail
                .starts_with("the forge could not answer for 1 of 1 changes"),
            "{}",
            review.detail
        );
        // A direct push (no merged pull request) is read, and clean.
        let d = at(1, None);
        let api = canned(&[(
            &format!("github:repos/o/r/commits/{}/pulls", d.sha),
            serde_json::json!([]),
        )]);
        let (pulls, read, _) = read_pulls(&api, &github(), &[d], &[], false);
        let mut checks = signals(&[], &[]).1;
        forge_checks(&mut checks, &read, &pulls, &[]);
        assert_eq!(
            checks
                .iter()
                .find(|c| c.id == "independent-review")
                .unwrap()
                .state,
            "clean"
        );
    }

    /// A repository with `commits`, each `(time, [(path, content)])`, on one line.
    fn repo_with(commits: &[(i64, &[(&str, &str)])]) -> (crate::replay::TempDir, Vec<Oid>) {
        let dir = crate::replay::TempDir::named("audit-test").unwrap();
        let repo = Repository::init(&dir.0).unwrap();
        let mut ids = Vec::new();
        for (time, files) in commits {
            let mut index = repo.index().unwrap();
            for (path, content) in *files {
                let full = dir.0.join(path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, content).unwrap();
                index.add_path(std::path::Path::new(path)).unwrap();
            }
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig =
                git2::Signature::new("t", "t@example.invalid", &git2::Time::new(*time, 0)).unwrap();
            let parents: Vec<Commit> = ids
                .last()
                .map(|id| repo.find_commit(*id).unwrap())
                .into_iter()
                .collect();
            let parents: Vec<&Commit> = parents.iter().collect();
            ids.push(
                repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &parents)
                    .unwrap(),
            );
        }
        (dir, ids)
    }

    #[test]
    fn a_path_last_changed_where_its_blob_last_differed_from_the_parent() {
        let (dir, ids) = repo_with(&[
            (1_000, &[("a.py", "1")]),
            (2_000, &[("b.py", "1")]),
            (3_000, &[("a.py", "2")]),
        ]);
        let repo = Repository::open(&dir.0).unwrap();
        assert_eq!(
            last_change_before(&repo, ids[2], "a.py").unwrap(),
            Some(3_000)
        );
        assert_eq!(
            last_change_before(&repo, ids[1], "a.py").unwrap(),
            Some(1_000)
        );
        assert_eq!(
            last_change_before(&repo, ids[2], "b.py").unwrap(),
            Some(2_000)
        );
        assert_eq!(last_change_before(&repo, ids[2], "never.py").unwrap(), None);
    }

    #[test]
    fn protected_edits_are_judged_as_the_gate_judges_them() {
        let config = "[meta]\nversion = 1\nname = \"t\"\n[gates.ratified-paths]\nenabled = true\nprotected_paths = [\"scripts/*.py\"]\nratifiers = [\"owner\"]\nratification_valid_from = \"any\"\n";
        let (dir, ids) = repo_with(&[
            (1_000, &[("discipline.toml", config), ("scripts/a.py", "1")]),
            (2_000, &[("scripts/a.py", "2")]),
        ]);
        let repo = Repository::open(&dir.0).unwrap();
        let sha = ids[1].to_string();
        let change = ChangeInfoRef {
            sha: sha.clone(),
            pr: Some(7),
            time: 2_000,
            ord: 0,
            subject: "s".into(),
        };
        let c = repo.find_commit(ids[1]).unwrap();
        let (pt, ct) = (c.parent(0).unwrap().tree().unwrap(), c.tree().unwrap());
        let edits = protected_records(&repo, &pt, &ct, Some(config), &change).unwrap();
        assert_eq!(edits.len(), 1);
        let gitea = crate::forge::Forge {
            kind: crate::forge::ForgeKind::Gitea,
            url: "https://git.example.org".into(),
            repo: "o/r".into(),
        };
        let api = |comments: serde_json::Value| {
            let n = comments.as_array().unwrap().len().to_string();
            canned(&[
                ("gitea:repos/o/r", serde_json::json!({"full_name": "o/r"})),
                // Closed by the merge itself (the change is dated 2000 s).
                (
                    "gitea:repos/o/r/issues/12",
                    serde_json::json!({"number": 12, "state": "closed", "closed_at": "1970-01-01T00:33:25Z"}),
                ),
                (
                    "gitea:repos/o/r/issues/12/comments?limit=50&page=1",
                    serde_json::json!({"__status": 200, "__headers": {"X-Total-Count": n}, "__body": comments}),
                ),
            ])
        };
        let ratifying = serde_json::json!([{"id": 5, "user": {"login": "owner"}, "body": "Owner-ratified-paths:\n- scripts/a.py\n", "created_at": "2026-09-27T10:00:00Z", "updated_at": "2026-09-27T10:00:00Z", "original_author": ""}]);
        let pull = |author: &str| Pull {
            sha: sha.clone(),
            pr: 7,
            approved_by_other: false,
            author: author.into(),
            body: "Closes #12".into(),
        };
        let judge = |api: &crate::forge::CannedApi, pulls: &[Pull], failed: &[String]| {
            let mut e = edits.clone();
            judge_protected(&repo, api, &gitea, &mut e, pulls, failed).unwrap();
            e[0].ratification.clone().unwrap()
        };
        // Ratified by another login, on the issue the pull request closed at merge.
        let r = judge(&api(ratifying.clone()), &[pull("dev")], &[]);
        assert_eq!(
            (r.state, r.issue, r.comment_id.as_deref()),
            ("ratified", Some(12), Some("5"))
        );
        // The same comment, when the pull request's author is the ratifier.
        assert_eq!(
            judge(&api(ratifying.clone()), &[pull("OWNER")], &[]).state,
            "self-ratified"
        );
        // No ratifying comment.
        assert_eq!(
            judge(&api(serde_json::json!([])), &[pull("dev")], &[]).state,
            "unratified"
        );
        // The ratifying issue was closed long before the merge: the gate, which needs it
        // open, could not have accepted it.
        let closed_early = canned(&[
            ("gitea:repos/o/r", serde_json::json!({"full_name": "o/r"})),
            (
                "gitea:repos/o/r/issues/12",
                serde_json::json!({"number": 12, "state": "closed", "closed_at": "1970-01-01T00:00:00Z"}),
            ),
            (
                "gitea:repos/o/r/issues/12/comments?limit=50&page=1",
                serde_json::json!({"__status": 200, "__headers": {"X-Total-Count": "1"}, "__body": ratifying.clone()}),
            ),
        ]);
        let early = judge(&closed_early, &[pull("dev")], &[]);
        assert_eq!(
            (early.state, early.why.as_deref()),
            ("unratified", Some("issue #12 was closed before the merge"))
        );
        // A direct push, and a change the forge could not answer for.
        assert_eq!(judge(&api(ratifying.clone()), &[], &[]).state, "unratified");
        assert_eq!(
            judge(&api(ratifying), &[], std::slice::from_ref(&sha)).state,
            "not-checked"
        );

        let mut judged = edits.clone();
        judged[0].ratification = Some(RatificationFact {
            state: "self-ratified",
            issue_repo: None,
            issue: None,
            comment_id: None,
            created: None,
            why: None,
        });
        let sigs = protected_signals(&judged);
        assert_eq!(
            sigs.iter().map(|g| (g.id, g.list)).collect::<Vec<_>>(),
            vec![("protected-edit-self-ratified", "protected_edits")]
        );
        let mut checks = signals(&[], &[]).1;
        ratification_check(&mut checks, &judged);
        let c = checks
            .iter()
            .find(|c| c.id == "owner-ratification")
            .unwrap();
        assert_eq!(c.state, "found", "{c:?}");
    }

    #[test]
    fn a_waiver_cites_the_issues_its_reason_names_but_not_pull_requests() {
        let r = directive_records(
            "s\n\nallow-stub: fn_a tracked in #12 and o/r#5, see https://github.com/o/r/pull/9\n",
            &info(),
            false,
        );
        assert_eq!(r[0].cites, vec!["#12".to_string(), "o/r#5".to_string()]);
        assert!(
            directive_records("s\n\nallow-stub: fn_a no issue", &info(), false)[0]
                .cites
                .is_empty()
        );
    }

    #[test]
    fn cited_issues_are_read_once_and_judged_against_the_waiver() {
        // The waiver's change is dated 2026-09-20T00:00:00Z.
        let at_time = |cites: &str| {
            let mut r =
                directive_records(&format!("s\n\nallow-stub: fn_a {cites}"), &info(), false);
            r[0].time = 1_789_862_400;
            r
        };
        let mut records = at_time("tracked in #12");
        records.extend(at_time("tracked in #13"));
        records.extend(at_time("tracked in #14 and other/x#1"));
        records.extend(at_time("tracked in #15 and o/r#15"));
        let api = canned(&[
            ("github:repos/o/r", serde_json::json!({"full_name": "o/r"})),
            // Closed after the waiver, as not planned.
            (
                "github:repos/o/r/issues/12",
                serde_json::json!({"number": 12, "state": "closed", "state_reason": "not_planned", "closed_at": "2026-09-25T00:00:00Z"}),
            ),
            ("github:repos/o/r/issues/13", serde_json::Value::Null),
            // Already closed when the waiver was written.
            (
                "github:repos/o/r/issues/14",
                serde_json::json!({"number": 14, "state": "closed", "state_reason": "completed", "closed_at": "2026-09-01T00:00:00Z"}),
            ),
            (
                "github:repos/o/r/issues/15",
                serde_json::json!({"number": 15, "state": "open"}),
            ),
        ]);
        let issues = read_issues(&api, &github(), &records);
        let states: Vec<(&str, &str)> = issues
            .iter()
            .map(|f| (f.reference.as_str(), f.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("#12", "closed"),
                ("#13", "not-found"),
                ("#14", "closed"),
                ("other/x#1", "cross-repo"),
                ("#15", "open"),
                ("o/r#15", "open")
            ]
        );
        // `o/r#15` is `#15`: read once. Another repository is never read.
        assert_eq!(
            api.log().iter().filter(|k| k.contains("issues/15")).count(),
            1
        );
        assert!(!api.log().iter().any(|k| k.contains("other/x")));
        let sigs = citation_signals(&records, &issues);
        let hit = |id: &str| {
            sigs.iter()
                .find(|g| g.id == id)
                .map(|g| g.records.clone())
                .unwrap_or_default()
        };
        assert_eq!(hit("waiver-cites-missing-issue"), vec![1]);
        assert_eq!(hit("waiver-cites-issue-closed-before"), vec![2]);
        assert_eq!(hit("waiver-cites-issue-not-planned"), vec![0]);
        // A skipped issue link citing a closed issue is context, not a promise.
        let mut context = directive_records("s\n\nno-issue: follows #14", &info(), false);
        context[0].time = 1_789_862_400;
        assert!(citation_signals(&context, &issues).is_empty());
        let mut checks = signals(&records, &[]).1;
        issues_check(&mut checks, &records, &issues);
        let c = checks.iter().find(|c| c.id == "cited-issues").unwrap();
        assert_eq!(c.state, "found", "{c:?}");

        // A forge that cannot answer leaves the question not checked.
        let down = canned(&[
            ("github:repos/o/r", serde_json::json!({"full_name": "o/r"})),
            (
                "github:repos/o/r/issues/12",
                serde_json::json!({"__status": 500}),
            ),
        ]);
        let issues = read_issues(&down, &github(), &records[..1]);
        assert_eq!(issues[0].state, "not-checked");
        let mut checks = signals(&records, &[]).1;
        issues_check(&mut checks, &records[..1], &issues);
        assert_eq!(
            checks
                .iter()
                .find(|c| c.id == "cited-issues")
                .unwrap()
                .state,
            "not-checked"
        );
    }
}
