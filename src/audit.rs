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
    /// A `discipline replay --json` report, read for which waivers lifted a finding.
    pub replay: Option<std::path::PathBuf>,
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
    /// A loosening whose own change also tightened the same option: an edited list
    /// entry, which `config-integrity` counts as lost unless it can prove it tighter.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
    /// With `--replay`, for a finding waiver: whether the replayed check applied it to
    /// lift a finding (then `evidence` is `applied`). Absent when the replay did not
    /// judge the change, or for a `process` waiver (`no-issue`), which lifts no finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifted: Option<bool>,
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
            edited: false,
            lifted: None,
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
    "sandbox-config",
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
        next: "Open the change and check that each loosened setting was intended.",
        test: |i, all, _| {
            let r = &all[i];
            is_config(r) && r.gate.as_deref().is_some_and(|g| GUARD_GATES.contains(&g))
        },
    },
    SignalDef {
        id: "hidden-directive",
        rank: "look-first",
        next: "Read the commit message or pull request as plain text and check that the hidden waiver was meant.",
        test: |i, all, _| all[i].hidden == Some(true),
    },
    SignalDef {
        id: "config-unreadable",
        rank: "look-soon",
        next: "Compare that change's discipline.toml by hand: this version of discipline could not read it.",
        test: |i, all, _| all[i].kind == "config-unreadable",
    },
    SignalDef {
        id: "loosened-without-pull-request",
        rank: "look-soon",
        next: "Ask whoever pushed it why: nobody reviewed the change.",
        test: |i, all, _| is_config(&all[i]) && all[i].pr.is_none(),
    },
    SignalDef {
        id: "loosening-without-waiver",
        rank: "review",
        next: "Look for the reason in the pull request description; if there is none, find out why the configuration check let it through.",
        test: |i, all, _| {
            let r = &all[i];
            is_config(r) && r.pr.is_some() && !waived_in_change(r, all)
        },
    },
    SignalDef {
        id: "waived-then-loosened",
        rank: "review",
        next: "Check whether the loosening fixed a real problem with the check, or only stopped it from reporting.",
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
        next: "Tighten each setting back, or write down why it stays loose.",
        test: |i, all, tightenings| {
            let r = &all[i];
            // An edited entry is not a loosening waiting to be paid back.
            is_config(r)
                && !r.edited
                && !tightenings
                    .iter()
                    .any(|t| t.ord < r.ord && t.gate == r.gate && t.key == r.key)
        },
    },
    SignalDef {
        id: "baseline-grew",
        rank: "review",
        next: "Fix the findings, or write down why each one stays in the baseline.",
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
        "pull-request-body-edited",
        "a pull request body's edit time is read with `--forge`, on GitHub",
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
    /// For each change with an exception (a record or a protected edit), newest first:
    /// whether anything records that an agent made it.
    pub identities: Vec<Identity>,
    /// What `--replay` read; `None` without it.
    pub replay: Option<ReplayRead>,
}

/// The replay report `--replay` read.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ReplayRead {
    /// Changes in the report.
    pub cases: usize,
    /// Of those, the ones whose check ran (`passed` or `blocked`).
    pub checked: usize,
}

/// Whether a change carries any record that an agent made it. A missing record is
/// `no-record`, never a person: agents can commit under a person's identity.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Identity {
    pub sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    /// `agent-login` (with `--forge`, its pull request was opened by an `agent_logins`
    /// or `[bot]` login), `claimed` (a commit carries a `commit-provenance` agent marker,
    /// which the commit asserts and nothing verifies), or `no-record`.
    pub state: &'static str,
    /// A `commit-provenance` agent marker matched the commit's trailers or author.
    pub marker: bool,
    /// Its pull request's author is an agent login; absent without `--forge` or a pull
    /// request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_login: Option<bool>,
}

/// A change's merged pull request, as the forge reported it. Logins are not carried.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Pull {
    pub sha: String,
    pub pr: u64,
    /// A login other than the pull request's author approved its head.
    pub approved_by_other: bool,
    /// Its body was edited after the merge, so the directives read from it now may not be
    /// the ones the gates read. Absent when not known (not GitHub, or no merge time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_edited_after_merge: Option<bool>,
    /// When it merged, for the edit comparison; not reported.
    #[serde(skip)]
    pub merged_at: Option<i64>,
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
    /// Why the pull request bodies' edit times were not read, when they were not.
    #[serde(skip)]
    pub edits_error: Option<String>,
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
        // A detail can quote the forge (why a read failed) and a record names a file, a
        // key or a directive from a commit: each stays on its own line.
        let line = crate::report::text::terminal_line;
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
                    "  {:<10} {:<30} {:>3}  {}\n             next: {}\n",
                    sg.rank,
                    sg.id,
                    sg.count,
                    line(&changes),
                    sg.next
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
            out.push_str(&format!(
                "  {:<11} {:<30} {}\n",
                c.state,
                c.id,
                line(&c.detail)
            ));
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
                "{label:<9} {:<17} {:<24} {}\n",
                r.kind,
                line(r.gate.as_deref().unwrap_or("-")),
                line(&what)
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
            out.push_str(&format!("  gate  {:<24} {n}\n", line(g)));
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
#[cfg(test)]
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
            .map_err(|e| format!("{side}: {}", parse_failure(&e.to_string())))
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
    let both = |g: &str, k: &str| tightened.iter().any(|t| t.gate == g && t.key == k);
    let loosened: Vec<Record> = loosened
        .iter()
        .map(|w| {
            let edited = both(&w.gate, &w.key);
            let mut r = at_line(record("config", w.clone(), false));
            r.edited = edited;
            r
        })
        .collect();
    (
        loosened,
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

/// A configuration parse error in one line: its first line, and the key when the parser
/// stopped on one it does not know (a key a later release removed). Only the key's name is
/// taken; the line that quotes the source, and any message that quotes a value, is not.
fn parse_failure(error: &str) -> String {
    let unknown_key = error
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| l.trim().strip_prefix("unknown field `"))
        .and_then(|rest| rest.split_once('`'))
        .map(|(key, _)| key);
    match unknown_key {
        Some(key) => format!("{}: unknown key `{key}`", first_line(error)),
        None => first_line(error).to_string(),
    }
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
    let tip_config = blob_text(&repo, &repo.find_commit(tip)?.tree()?, CONFIG_NAME)?
        .and_then(|t| DisciplineConfig::from_toml_str(&t).ok())
        .unwrap_or_else(|| DisciplineConfig::default_for_repo("audit"));
    let mut marked = std::collections::BTreeSet::new();
    for (ord, c) in commits.iter().enumerate() {
        let detail = crate::gitctx::CommitDetail {
            sha: c.id().to_string(),
            author_name: c.author().name().unwrap_or("").to_string(),
            author_email: c.author().email().unwrap_or("").to_string(),
            committer_email: c.committer().email().unwrap_or("").to_string(),
            message: c.message().unwrap_or("").to_string(),
            parent_count: c.parent_count(),
        };
        if crate::guards::commit_provenance::is_agent_commit(
            &detail,
            &crate::guards::commit_provenance::trailers(&detail.message),
            &tip_config.gates.commit_provenance.agent_markers,
        ) {
            marked.insert(detail.sha);
        }
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
    if let Some(path) = &opts.replay {
        let cases = read_replay(path)?;
        judge_lifted(&mut s.records, &cases);
        s.signals.extend(replay_signals(&s.records));
        s.signals.sort_by_key(|g| rank_order(g.rank));
        lifted_check(&mut s.checks, &s.records, &cases);
        s.replay = Some(ReplayRead {
            cases: cases.len(),
            checked: cases.iter().filter(|c| c.judged).count(),
        });
    }
    s.identities = identities(
        &s.records,
        &s.protected_edits,
        &marked,
        s.forge.as_ref().map(|_| pulls.as_slice()),
        &tip_config.gates.ratified_paths.agent_logins,
    );
    identity_check(&mut s.checks, &s.identities, s.forge.is_some());
    s.pulls = pulls;
    s.issues = issues;
    Ok(s)
}

/// One change as a `discipline replay --json` report records it: whether its check ran,
/// and the overrides it applied, as `(gate, directive, source)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCase {
    pub sha: String,
    /// The check ran (`passed` or `blocked`); a `could_not_check` case judges nothing.
    pub judged: bool,
    pub overrides: Vec<(String, String, String)>,
}

/// The cases of a `discipline replay --json` report.
pub fn read_replay(path: &std::path::Path) -> Result<Vec<ReplayCase>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the replay report {}", path.display()))?;
    parse_replay(&text).with_context(|| format!("{} is not a replay report", path.display()))
}

pub fn parse_replay(text: &str) -> Result<Vec<ReplayCase>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("schema_version").and_then(|n| n.as_u64()).is_none() {
        bail!("no `schema_version`");
    }
    let cases = v
        .get("cases_detail")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("no `cases_detail` list"))?;
    let s = |c: &serde_json::Value, k: &str| {
        c.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    cases
        .iter()
        .map(|c| {
            let sha = s(c, "sha");
            if sha.is_empty() {
                bail!("a case has no `sha`");
            }
            let overrides = c
                .get("overrides")
                .and_then(|o| o.as_array())
                .ok_or_else(|| anyhow!("case {sha} has no `overrides` list"))?
                .iter()
                .map(|o| (s(o, "gate"), s(o, "directive"), s(o, "source")))
                .collect();
            Ok(ReplayCase {
                judged: matches!(s(c, "verdict").as_str(), "passed" | "blocked"),
                sha,
                overrides,
            })
        })
        .collect()
}

/// Whether the replayed check of each waiver's change applied it. A directive matches an
/// override of the same gate under any of its names; an inline marker, an override of the
/// same gate read from its own file and line (`inline <file>:<line>`). A marker the gates
/// read by line text is often quoted text (a fixture, a doc example) on a line with no
/// finding: it lifts nothing, and the replay says so. Process waivers (`no-issue`) and
/// changes the replay did not judge stay `None`.
pub fn judge_lifted(records: &mut [Record], cases: &[ReplayCase]) {
    for r in records.iter_mut() {
        let waiver = (r.kind == "directive" && r.class == "detector") || r.kind == "inline-marker";
        let Some(case) = cases.iter().find(|c| c.sha == r.sha && c.judged) else {
            continue;
        };
        if !waiver {
            continue;
        }
        let same_gate = |g: &str| {
            r.gate
                .as_deref()
                .is_some_and(|rg| rg.eq_ignore_ascii_case(g))
        };
        let at = match (r.file.as_deref(), r.line) {
            (Some(f), Some(l)) => format!("inline {f}:{l}"),
            _ => String::new(),
        };
        let lifted = case.overrides.iter().any(|(gate, directive, source)| {
            same_gate(gate)
                && if r.kind == "inline-marker" {
                    !at.is_empty() && *source == at
                } else {
                    r.directive.as_deref().is_some_and(|d| {
                        crate::tokens::names_for_directive(d)
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(directive))
                            || d.eq_ignore_ascii_case(directive)
                    })
                }
        });
        r.lifted = Some(lifted);
        if lifted {
            r.evidence = "applied";
        }
    }
}

/// `waiver-lifted-nothing`: directive waivers the replayed check did not apply. An inline
/// marker that lifted nothing is not a signal: it is usually quoted text, and it is
/// reported by `lifted` and in the check's detail.
pub fn replay_signals(records: &[Record]) -> Vec<Signal> {
    let hits: Vec<usize> = (0..records.len())
        .filter(|&i| records[i].kind == "directive" && records[i].lifted == Some(false))
        .collect();
    if hits.is_empty() {
        return Vec::new();
    }
    let mut changes: Vec<String> = Vec::new();
    for &i in &hits {
        let l = records[i].label();
        if !changes.contains(&l) {
            changes.push(l);
        }
    }
    vec![Signal {
        id: "waiver-lifted-nothing",
        rank: "review",
        count: hits.len(),
        changes,
        records: hits,
        list: "records",
        next: "Check whether the waiver was needed when it was written. The replay re-checks with the discipline version that ran it, so a gate refined since, or a directive format that version no longer reads, also leaves nothing to lift; a waiver that was never needed was written in advance, which is a habit to stop.",
    }]
}

/// `directive-lifted-a-finding` from a replay: `found` when a waiver lifted nothing,
/// `clean` when the replay judged every change with a finding waiver and each one lifted
/// something, else `not-checked` with what the replay did not cover.
pub fn lifted_check(checks: &mut [Check], records: &[Record], cases: &[ReplayCase]) {
    let Some(c) = checks
        .iter_mut()
        .find(|c| c.id == "directive-lifted-a-finding")
    else {
        return;
    };
    let waivers: Vec<&Record> = records
        .iter()
        .filter(|r| r.kind == "directive" && r.class == "detector")
        .collect();
    let judged = waivers.iter().filter(|r| r.lifted.is_some()).count();
    let nothing = waivers.iter().filter(|r| r.lifted == Some(false)).count();
    let unjudged: std::collections::BTreeSet<&str> = waivers
        .iter()
        .filter(|r| r.lifted.is_none())
        .map(|r| r.sha.as_str())
        .collect();
    let waivers_n = crate::audit_html::plural(waivers.len(), "finding waiver", "finding waivers");
    let replayed = cases.iter().filter(|c| c.judged).count();
    if nothing > 0 {
        c.state = "found";
        c.detail = format!(
            "{nothing} of the {} the replay judged lifted nothing{}",
            crate::audit_html::plural(judged, "finding waiver", "finding waivers"),
            if unjudged.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} changes with a finding waiver were not judged",
                    unjudged.len()
                )
            }
        );
    } else if waivers.is_empty() {
        c.state = "clean";
        c.detail = "no finding waiver to judge".to_string();
    } else if unjudged.is_empty() {
        c.state = "clean";
        c.detail = format!("each of {judged} {waivers_n} lifted a finding in the replay");
    } else {
        c.state = "not-checked";
        let missing = unjudged
            .iter()
            .filter(|sha| !cases.iter().any(|c| c.sha == **sha))
            .count();
        c.detail = format!(
            "{} of the changes with a finding waiver were not judged: {} the replay could not check, {missing} it did not include (the replay checked {})",
            unjudged.len(),
            unjudged.len() - missing,
            crate::audit_html::plural(replayed, "change", "changes")
        );
    }
    let markers: Vec<&Record> = records
        .iter()
        .filter(|r| r.kind == "inline-marker" && r.lifted.is_some())
        .collect();
    if !markers.is_empty() {
        let applied = markers.iter().filter(|r| r.lifted == Some(true)).count();
        let n = crate::audit_html::plural(
            markers.len(),
            "replayed inline marker",
            "replayed inline markers",
        );
        c.detail.push_str(&if applied == markers.len() {
            format!("; each of {n} lifted a finding")
        } else if applied == 0 {
            format!("; none of the {n} lifted a finding (they sit on lines with nothing to lift, such as markers quoted in tests and docs)")
        } else {
            format!("; {applied} of {n} lifted a finding (the rest sit on lines with nothing to lift, such as markers quoted in tests and docs)")
        });
    }
}

/// One [`Identity`] per change with a record or a protected edit, newest first. `pulls`
/// is `Some` when the forge was read.
pub fn identities(
    records: &[Record],
    protected: &[Record],
    marked: &std::collections::BTreeSet<String>,
    pulls: Option<&[Pull]>,
    agent_logins: &[String],
) -> Vec<Identity> {
    let mut changes: Vec<(&str, Option<u64>, usize)> = Vec::new();
    for r in records.iter().chain(protected) {
        if !changes.iter().any(|(s, _, _)| *s == r.sha) {
            changes.push((&r.sha, r.pr, r.ord));
        }
    }
    changes.sort_by_key(|(_, _, ord)| *ord);
    changes
        .into_iter()
        .map(|(sha, pr, _)| {
            let marker = marked.contains(sha);
            let agent_login = pulls.and_then(|ps| {
                ps.iter().find(|p| p.sha == sha).map(|p| {
                    p.author.ends_with("[bot]")
                        || agent_logins
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(&p.author))
                })
            });
            Identity {
                sha: sha.to_string(),
                pr,
                state: match (agent_login, marker) {
                    (Some(true), _) => "agent-login",
                    (_, true) => "claimed",
                    _ => "no-record",
                },
                marker,
                agent_login,
            }
        })
        .collect()
}

/// `agent-identity`: `found` when a change with an exception carries an agent record,
/// else `not-checked`. Never `clean`, since a missing record does not show a person.
pub fn identity_check(checks: &mut [Check], ids: &[Identity], forge: bool) {
    let Some(c) = checks.iter_mut().find(|c| c.id == "agent-identity") else {
        return;
    };
    let n = ids.len();
    let changes = crate::audit_html::plural(n, "change", "changes");
    let login = ids.iter().filter(|i| i.state == "agent-login").count();
    let claimed = ids.iter().filter(|i| i.state == "claimed").count();
    let sources = if forge {
        "a commit's agent marker or an agent login"
    } else {
        "a commit's agent marker; agent logins are read with `--forge`"
    };
    if n == 0 {
        c.state = "not-checked";
        c.detail = "no change with an exception to attribute".to_string();
    } else if login + claimed == 0 {
        c.state = "not-checked";
        c.detail = format!(
            "none of the {changes} with an exception carries a record of an agent ({sources}); a missing record does not mean a person made the change"
        );
    } else {
        c.state = "found";
        c.detail = format!(
            "{} of the {changes} with an exception carry an agent record: {login} from an agent login, {claimed} claimed by the commit (a marker it asserts, not verified); the other {} have none, which does not mean a person made them",
            login + claimed,
            n - login - claimed
        );
    }
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
            "Open the waiver and find, or create, the issue it meant to point to.",
            &|_, f| f.state == "not-found",
        ),
        (
            "waiver-cites-issue-closed-before",
            "look-soon",
            "Open the waiver and the issue: an approval should say so there, and a follow-up needs an open issue.",
            // A finding waiver cites an issue as a follow-up or as the approval; either
            // way, a closed one deserves a look. A skipped issue link citing a closed
            // issue is context ("follows #12").
            &|r, f| {
                r.class == "detector"
                    && f.state == "closed"
                    && f.closed_at.is_some_and(|t| t < r.time - 60)
            },
        ),
        (
            "waiver-cites-issue-not-planned",
            "review",
            "Decide what replaces the follow-up the waiver promised.",
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
        let found = crate::forge::merged_pull_on_forge(api, forge, &c.sha).and_then(|m| match m {
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
                    body_edited_after_merge: None,
                    merged_at: m.merged_at,
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
    let numbers: Vec<u64> = pulls.iter().map(|p| p.pr).collect();
    if !numbers.is_empty() {
        match crate::forge::pull_body_edits(api, forge, &numbers) {
            Ok(edits) => {
                for p in &mut pulls {
                    let edited = edits.get(&p.pr).copied().flatten();
                    p.body_edited_after_merge = match (edited, p.merged_at) {
                        (None, _) => Some(false),
                        (Some(at), Some(merged)) => Some(at > merged),
                        (Some(_), None) => None,
                    };
                }
            }
            Err(e) => read.edits_error = Some(e),
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
        // before the merge is told apart by its close time (`closed_before_merge`).
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
                    pull_author: &pull.author,
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
            "Open the change: a protected file was edited with no owner approval.",
        ),
        (
            "protected-edit-self-ratified",
            "look-soon",
            "self-ratified",
            "Review the change yourself: the only approval came from the account that opened it.",
        ),
    ] {
        let hits: Vec<usize> = (0..protected.len())
            .filter(|&i| {
                protected[i]
                    .ratification
                    .as_ref()
                    .is_some_and(|f| f.state == state)
            })
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
        set(checks, "independent-review", "not-checked", why.clone());
        set(checks, "pull-request-body-edited", "not-checked", why);
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
    let unknown = pulls
        .iter()
        .filter(|p| p.body_edited_after_merge.is_none())
        .count();
    let edited: Vec<String> = pulls
        .iter()
        .filter(|p| p.body_edited_after_merge == Some(true))
        .map(|p| format!("#{}", p.pr))
        .collect();
    if let Some(e) = &read.edits_error {
        set(checks, "pull-request-body-edited", "not-checked", e.clone());
    } else if unknown > 0 {
        set(
            checks,
            "pull-request-body-edited",
            "not-checked",
            format!("the forge gave no merge time for {unknown} of {prs}"),
        );
    } else if !edited.is_empty() {
        set(
            checks,
            "pull-request-body-edited",
            "found",
            format!(
                "{} of {prs} edited after the merge: {}",
                edited.len(),
                edited.join(", ")
            ),
        );
    } else if n == 0 {
        set(
            checks,
            "pull-request-body-edited",
            "clean",
            "no change arrived through a pull request".to_string(),
        );
    } else {
        set(
            checks,
            "pull-request-body-edited",
            "clean",
            format!("none of {prs} edited after the merge"),
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

    /// #599: `allow_author_review` was added and removed between two releases, so the
    /// history of a repository that tracked `main` holds it. Both changes are reported as
    /// unreadable, naming the side and the key; the loosening the key was is not a
    /// `config` record. Reading a removed key again would be a design change.
    #[test]
    fn a_key_removed_from_commit_provenance_is_unreadable_on_the_side_that_holds_it() {
        let head = "[meta]\nversion = 1\nname = \"t\"\n[gates.commit-provenance]\nenabled = true\n";
        let with_key = format!("{head}allow_author_review = true\n");
        for (base, change, side) in [
            (head, with_key.as_str(), "change: "),
            (with_key.as_str(), head, "parent: "),
        ] {
            let r = config_records(Some(base), Some(change), &info());
            assert_eq!(r.len(), 1, "{r:?}");
            assert_eq!(r[0].kind, "config-unreadable");
            let detail = r[0].detail.as_deref().unwrap();
            assert!(detail.starts_with(side), "{detail}");
            assert!(
                detail.ends_with(": unknown key `allow_author_review`"),
                "{detail}"
            );
            assert_eq!(detail.lines().count(), 1, "{detail}");
        }
        // A failure that is not an unknown key keeps its first line and quotes no value.
        let wrong_type = format!("{head}require_agent_review = \"s3cr3t\"\n");
        let r = config_records(Some(head), Some(&wrong_type), &info());
        let detail = r[0].detail.as_deref().unwrap();
        assert!(detail.starts_with("change: "), "{detail}");
        assert!(
            !detail.contains("s3cr3t") && !detail.contains("unknown key"),
            "{detail}"
        );
        // Control: the same two sides without the key record nothing.
        assert!(config_records(Some(head), Some(head), &info()).is_empty());
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
    fn an_edited_entry_is_marked_and_not_waiting_to_be_restored() {
        // One group's source list rewritten: `config-integrity` reads the old entry as
        // lost; read backwards, the new one is lost too.
        let (l, t) = loosen(
            0,
            Some(56),
            "[gates.version-lockstep]\nenabled = true\n",
            "groups = [{ name = \"v\", sources = [{ path = \"a.toml\", regex = 'v = \"(.*)\"' }] }]\n",
            "groups = [{ name = \"v\", sources = [{ path = \"a.toml\", regex = '^v = \"(.*)\"' }, { path = \"b.json\", regex = 'v' }] }]\n",
        );
        assert_eq!((l.len(), t.len()), (1, 1), "{l:?} {t:?}");
        assert!(l[0].edited);
        assert!(changes_of(&found(&l, &t), "loosened-not-restored").is_empty());
        // A plain loosening is not edited.
        let (plain, _) = loosen(
            0,
            Some(9),
            "[gates.pii]\n",
            "exempt_paths = []\n",
            "exempt_paths = [\"a\"]\n",
        );
        assert!(!plain[0].edited);
        assert_eq!(
            changes_of(&found(&plain, &[]), "loosened-not-restored"),
            vec!["#9"]
        );
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
    fn switching_sandbox_config_off_is_a_guard_gate_loosening() {
        // An agent that turns this gate off can widen its own sandbox, hooks included,
        // with no finding: the same stakes as loosening the gates that guard the others.
        let (off, _) = loosen(
            0,
            Some(7),
            "[gates.sandbox-config]\n",
            "enabled = true\n",
            "enabled = false\n",
        );
        let (pii, _) = loosen(
            1,
            Some(6),
            "[gates.pii]\n",
            "enabled = true\n",
            "enabled = false\n",
        );
        let guard = |r: &[Record]| {
            found(r, &[])
                .into_iter()
                .find(|(id, _)| *id == "guard-gate-loosened")
                .map(|(_, c)| c)
        };
        assert_eq!(guard(&off), Some(vec!["#7".to_string()]));
        assert_eq!(guard(&pii), None);
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
    fn a_replay_says_which_waivers_lifted_a_finding() {
        let (a, b, c) = (at(0, Some(10)), at(1, Some(11)), at(2, Some(12)));
        // #10: `deletes` (an alias of `removes`) was applied, `allow-assertion-drop` was
        // not, and `no-issue` lifts no finding. Inline markers match by file and line:
        // `src/x.rs:4` was applied; `src/y.rs:4`, `src/x:4` and a marker at another line
        // of `src/x.rs` were not. #11 could not be checked; #12 was not
        // replayed at all.
        let mut records = directive_records(
            "s\n\ndeletes: tests/old.rs obsolete\nallow-assertion-drop: tests/a.rs weaker\nno-issue: chore",
            &a,
            false,
        );
        let marker = |info: &ChangeInfoRef, file: &str| {
            let mut r = Record::new(&info.to_owned(), "inline-marker", "inline");
            r.gate = Some("pii".into());
            r.file = Some(file.into());
            r.line = Some(4);
            r
        };
        records.push(marker(&a, "src/x.rs"));
        records.push(marker(&a, "src/y.rs"));
        records.push(marker(&a, "src/x"));
        let mut moved = marker(&a, "src/x.rs");
        moved.line = Some(40);
        records.push(moved);
        records.extend(directive_records(
            "s\n\nallow-dependency: serde parser",
            &b,
            false,
        ));
        records.extend(directive_records(
            "s\n\nallow-dependency: serde parser",
            &c,
            false,
        ));
        let gate = |d: &str| gate_for_directive(d).unwrap().to_string();
        let report = serde_json::json!({"schema_version": 1, "cases_detail": [
            {"sha": a.sha, "verdict": "passed", "overrides": [
                {"gate": gate("removes"), "directive": "removes", "source": format!("commit {}", a.sha)},
                {"gate": "pii", "directive": "discipline:allow(pii)", "source": "inline src/x.rs:4"},
                {"gate": "pii", "directive": "discipline:allow(pii)", "source": "inline src/x.rs.bak:1"}]},
            {"sha": b.sha, "verdict": "could_not_check", "overrides": []}
        ]});
        let cases = parse_replay(&report.to_string()).unwrap();
        judge_lifted(&mut records, &cases);
        assert_eq!(
            records
                .iter()
                .map(|r| (
                    r.directive.as_deref().or(r.file.as_deref()).unwrap_or(""),
                    r.lifted,
                    r.evidence
                ))
                .collect::<Vec<_>>(),
            vec![
                ("deletes", Some(true), "applied"),
                ("allow-assertion-drop", Some(false), "claimed"),
                ("no-issue", None, "claimed"),
                ("src/x.rs", Some(true), "applied"),
                ("src/y.rs", Some(false), "claimed"),
                ("src/x", Some(false), "claimed"),
                ("src/x.rs", Some(false), "claimed"),
                ("allow-dependency", None, "claimed"),
                ("allow-dependency", None, "claimed"),
            ]
        );
        let sig = replay_signals(&records);
        assert_eq!(
            sig.iter()
                .map(|g| (g.id, g.count, g.records.clone()))
                .collect::<Vec<_>>(),
            vec![("waiver-lifted-nothing", 1, vec![1])]
        );
        let check = |records: &[Record], cases: &[ReplayCase]| {
            let mut checks = signals(&[], &[]).1;
            lifted_check(&mut checks, records, cases);
            checks
                .into_iter()
                .find(|c| c.id == "directive-lifted-a-finding")
                .map(|c| (c.state, c.detail))
                .unwrap()
        };
        assert_eq!(
            check(&records, &cases),
            (
                "found",
                "1 of the 2 finding waivers the replay judged lifted nothing; 2 changes with a finding waiver were not judged; 1 of 4 replayed inline markers lifted a finding (the rest sit on lines with nothing to lift, such as markers quoted in tests and docs)".to_string()
            )
        );
        // Every waiver judged and applied: clean. Some not judged: not checked.
        let applied: Vec<Record> = records
            .iter()
            .filter(|r| r.lifted == Some(true))
            .cloned()
            .collect();
        assert_eq!(check(&applied, &cases).0, "clean");
        let partial: Vec<Record> = records
            .iter()
            .filter(|r| r.lifted != Some(false))
            .cloned()
            .collect();
        assert_eq!(
            check(&partial, &cases),
            (
                "not-checked",
                "2 of the changes with a finding waiver were not judged: 1 the replay could not check, 1 it did not include (the replay checked 1 change); each of 1 replayed inline marker lifted a finding".to_string()
            )
        );
        // A report that is not a replay is refused, never read as "nothing lifted".
        assert!(parse_replay("{}").is_err());
        assert!(parse_replay(
            r#"{"schema_version":1,"cases_detail":[{"sha":"x","verdict":"passed"}]}"#
        )
        .is_err());
    }

    #[test]
    fn the_protected_row_points_to_the_ratification_the_forge_read() {
        let a = at(0, Some(10));
        let mut edit = Record::new(&a.to_owned(), "protected-edit", "protected");
        edit.file = Some("AGENTS.md".into());
        let mut s = Summary::from_records("main".into(), 1, vec![], vec![]);
        s.protected_edits = vec![edit];
        assert!(crate::audit_html::render(&s).contains("from git; ratification not checked"));
        s.forge = Some(ForgeRead::default());
        assert!(crate::audit_html::render(&s)
            .contains("from git; ratification under Owner ratification"));
    }

    #[test]
    fn forge_facts_on_the_page_are_escaped_and_link_only_into_the_repository() {
        // What `--forge` adds to the page: issue states, ratifications and their reasons. A
        // reference to another repository is never read, so it is never linked; a reason is
        // escaped; a close reason is compared, never printed.
        use crate::forge::{Forge, ForgeKind};
        let a = at(0, Some(10));
        let mut unratified = Record::new(&a.to_owned(), "protected-edit", "protected");
        unratified.file = Some("AGENTS.md".into());
        unratified.ratification = Some(RatificationFact {
            state: "unratified",
            issue_repo: None,
            issue: None,
            comment_id: None,
            created: None,
            why: Some("<img src=x onerror=alert(1)> no owner comment".into()),
        });
        let mut ratified = Record::new(&a.to_owned(), "protected-edit", "protected");
        ratified.file = Some("discipline.toml".into());
        ratified.ratification = Some(RatificationFact {
            state: "ratified",
            issue_repo: Some("o/r".into()),
            issue: Some(3),
            comment_id: Some("42".into()),
            created: Some(1),
            why: None,
        });
        let mut waiver =
            directive_records("s\n\nno-issue: release bookkeeping", &a, false).remove(0);
        waiver.cites = vec!["x/../../evil/r#1".into(), "#2".into()];
        let mut s = Summary::from_records("main".into(), 1, vec![waiver], vec![]);
        s.protected_edits = vec![unratified, ratified];
        s.forge = Some(ForgeRead::default());
        s.links = Some(Links::for_forge(&Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }));
        s.issues = vec![
            IssueFact {
                reference: "x/../../evil/r#1".into(),
                repo: String::new(),
                number: 1,
                state: "cross-repo",
                state_reason: None,
                closed_at: None,
                error: None,
            },
            IssueFact {
                reference: "#2".into(),
                repo: "o/r".into(),
                number: 2,
                state: "closed",
                state_reason: Some("<b>done</b>".into()),
                closed_at: Some(1),
                error: None,
            },
        ];
        let page = crate::audit_html::render(&s);
        assert!(
            page.contains("&lt;img src=x onerror=alert(1)&gt; no owner comment"),
            "the reason is shown, escaped"
        );
        assert!(!page.contains("<img") && !page.contains("<b>done"));
        assert!(page.contains("<code>x/../../evil/r#1</code>"), "not a link");
        assert!(page.contains("https://github.com/o/r/issues/3#issuecomment-42"));
        assert!(page.contains("https://github.com/o/r/issues/2"));
        for href in page
            .split("href=\"")
            .skip(1)
            .map(|r| &r[..r.find('"').unwrap()])
        {
            assert!(
                href.starts_with('#')
                    || href == "https://github.com/o/r"
                    || href.starts_with("https://github.com/o/r/"),
                "{href}"
            );
            assert!(!href.contains(".."), "{href}");
        }
    }

    #[test]
    fn a_marker_that_lifted_nothing_leaves_the_markers_table() {
        let a = at(0, Some(10));
        let marker = |file: &str, lifted: Option<bool>| {
            let mut r = Record::new(&a.to_owned(), "inline-marker", "inline");
            r.gate = Some("pii".into());
            r.file = Some(file.into());
            r.line = Some(4);
            r.lifted = lifted;
            r
        };
        let page = |records: Vec<Record>| {
            crate::audit_html::render(&Summary::from_records("main".into(), 1, records, vec![]))
        };
        let rows = |page: &str| {
            page.split("Inline markers outside tests and docs")
                .nth(1)
                .and_then(|t| t.split("</div>").next())
                .unwrap()
                .matches("<tr><td>")
                .count()
        };
        // Before a replay, both markers in live code are listed.
        let before = page(vec![
            marker("src/live.rs", None),
            marker("src/quoted.rs", None),
        ]);
        assert_eq!(rows(&before), 2);
        // With it, the one that lifted nothing is left out of the table and counted.
        let after = page(vec![
            marker("src/live.rs", Some(true)),
            marker("src/quoted.rs", Some(false)),
        ]);
        let table = after
            .split("Inline markers outside tests and docs")
            .nth(1)
            .and_then(|t| t.split("</div>").next())
            .unwrap();
        assert_eq!(rows(&after), 1, "{table}");
        assert!(
            table.contains(
                "in the replay, 1 lifted a finding and 1 lifted nothing (left out below)"
            ),
            "{table}"
        );
        assert!(after.contains(">lifted nothing</span>"));
    }

    #[test]
    fn a_change_without_an_agent_record_is_never_read_as_a_person() {
        // Four changes with a waiver each, newest first, plus a protected edit.
        let records: Vec<Record> = (0..4)
            .flat_map(|o| {
                directive_records("s\n\nno-issue: chore", &at(o, Some(10 + o as u64)), false)
            })
            .collect();
        let mut edit = records[0].clone();
        edit.sha = at(4, None).sha;
        edit.ord = 4;
        edit.pr = None;
        let marked: std::collections::BTreeSet<String> =
            [at(1, None).sha, at(2, None).sha].into_iter().collect();
        let pull = |o: usize, author: &str| Pull {
            sha: at(o, None).sha,
            pr: 10 + o as u64,
            approved_by_other: false,
            body_edited_after_merge: None,
            merged_at: None,
            author: author.into(),
            body: String::new(),
        };
        // #10 by an agent login, #11 marked and opened by the owner, #12 marked and
        // opened by a bot, #13 neither; the protected edit was pushed directly.
        let pulls = vec![
            pull(0, "Agent"),
            pull(1, "owner"),
            pull(2, "dependabot[bot]"),
            pull(3, "owner"),
        ];
        let ids = identities(
            &records,
            &[edit.clone()],
            &marked,
            Some(&pulls),
            &["agent".to_string()],
        );
        assert_eq!(
            ids.iter()
                .map(|i| (i.pr, i.state, i.marker, i.agent_login))
                .collect::<Vec<_>>(),
            vec![
                (Some(10), "agent-login", false, Some(true)),
                (Some(11), "claimed", true, Some(false)),
                (Some(12), "agent-login", true, Some(true)),
                (Some(13), "no-record", false, Some(false)),
                (None, "no-record", false, None),
            ]
        );
        let check = |ids: &[Identity], forge: bool| {
            let mut checks = signals(&[], &[]).1;
            identity_check(&mut checks, ids, forge);
            checks
                .into_iter()
                .find(|c| c.id == "agent-identity")
                .map(|c| (c.state, c.detail))
                .unwrap()
        };
        let (state, detail) = check(&ids, true);
        assert_eq!(state, "found");
        assert_eq!(
            detail,
            "3 of the 5 changes with an exception carry an agent record: 2 from an agent login, \
             1 claimed by the commit (a marker it asserts, not verified); the other 2 have none, \
             which does not mean a person made them"
        );
        // Without the forge, only markers count, and no record is never `clean`.
        let offline = identities(&records, &[], &Default::default(), None, &[]);
        assert!(offline
            .iter()
            .all(|i| i.state == "no-record" && i.agent_login.is_none()));
        let (state, detail) = check(&offline, false);
        assert_eq!(state, "not-checked");
        assert!(
            detail.contains("does not mean a person made the change")
                && detail.contains("agent logins are read with `--forge`"),
            "{detail}"
        );
        assert_eq!(check(&[], false).0, "not-checked");
    }

    #[test]
    fn a_body_edited_after_the_merge_is_found_on_github_only() {
        let a = at(0, None);
        let b = at(1, None);
        let c = at(2, None);
        let pr = |n: u64| serde_json::json!([{"number": n, "merged_at": "2026-09-20T00:00:00Z", "user": {"login": "dev"}, "body": "", "head": {"sha": format!("h{n}")}}]);
        let mut answers: Vec<(String, serde_json::Value)> = Vec::new();
        for (ch, n) in [(&a, 7u64), (&b, 8), (&c, 9)] {
            answers.push((format!("github:repos/o/r/commits/{}/pulls", ch.sha), pr(n)));
            answers.push((
                format!("github:repos/o/r/pulls/{n}/reviews?per_page=100"),
                serde_json::json!([]),
            ));
        }
        let vars = serde_json::json!({"owner": "o", "name": "r", "n0": 7, "n1": 8, "n2": 9});
        // #7 was edited before its merge, #8 after it, #9 never.
        answers.push((
            format!("github:graphql:BodyEdits {vars}"),
            serde_json::json!({"data": {"repository": {
                "p0": {"lastEditedAt": "2026-09-19T23:00:00Z"},
                "p1": {"lastEditedAt": "2026-09-20T00:00:01Z"},
                "p2": {"lastEditedAt": null}
            }}}),
        ));
        let refs: Vec<(&str, serde_json::Value)> = answers
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let api = canned(&refs);
        let changes = [a.clone(), b.clone(), c.clone()];
        let (pulls, read, _) = read_pulls(&api, &github(), &changes, &[], false);
        assert_eq!(
            pulls
                .iter()
                .map(|p| (p.pr, p.body_edited_after_merge))
                .collect::<Vec<_>>(),
            vec![(7, Some(false)), (8, Some(true)), (9, Some(false))]
        );
        let check = |pulls: &[Pull], read: &ForgeRead| {
            let mut checks = signals(&[], &[]).1;
            forge_checks(&mut checks, read, pulls, &[]);
            checks
                .into_iter()
                .find(|c| c.id == "pull-request-body-edited")
                .map(|c| (c.state, c.detail))
                .unwrap()
        };
        assert_eq!(
            check(&pulls, &read),
            (
                "found",
                "1 of 3 pull requests edited after the merge: #8".to_string()
            )
        );
        // The same history on Gitea: the edit time is not read, so not checked.
        let gitea = crate::forge::Forge {
            kind: crate::forge::ForgeKind::Gitea,
            url: "https://gitea.example".into(),
            repo: "o/r".into(),
        };
        let api = canned(&[
            (
                &format!("gitea:repos/o/r/commits/{}/pull", a.sha),
                serde_json::json!({"number": 7, "merged": true, "merged_at": "2026-09-20T00:00:00Z", "user": {"login": "dev"}, "body": "", "head": {"sha": "h7"}}),
            ),
            (
                "gitea:repos/o/r/pulls/7/reviews?limit=50&page=1",
                serde_json::json!([]),
            ),
        ]);
        let (pulls, read, _) = read_pulls(&api, &gitea, &[a], &[], false);
        assert_eq!(read.failed, 0, "{:?}", read.error);
        assert_eq!(pulls[0].body_edited_after_merge, None);
        let (state, detail) = check(&pulls, &read);
        assert_eq!(state, "not-checked");
        assert!(detail.contains("on GitHub only"), "{detail}");
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

    /// The three checks the forge answers, as `(id, state, detail)`, after `read_pulls`
    /// asked a GitHub that answers the commit's pulls endpoint with `answer`.
    fn forge_states(answer: serde_json::Value) -> (ForgeRead, Vec<(String, String, String)>) {
        let a = at(0, None);
        let api = canned(&[(&format!("github:repos/o/r/commits/{}/pulls", a.sha), answer)]);
        let (pulls, read, body) = read_pulls(&api, &github(), &[a], &[], false);
        assert!(pulls.is_empty() && body.is_empty());
        let mut checks = signals(&[], &[]).1;
        forge_checks(&mut checks, &read, &pulls, &[]);
        let states = [
            "pull-request-body-directives",
            "independent-review",
            "pull-request-body-edited",
        ]
        .iter()
        .map(|id| {
            let c = checks.iter().find(|c| c.id == *id).unwrap();
            (id.to_string(), c.state.to_string(), c.detail.clone())
        })
        .collect();
        (read, states)
    }

    #[test]
    fn a_commit_the_forge_does_not_have_leaves_the_questions_not_checked() {
        // GitHub answers 422 for a commit it does not have: an unpushed `--ref`, or a
        // `--repo` that is another repository. Nothing about its review was read.
        let (read, states) = forge_states(serde_json::json!({
            "__status": 422,
            "__body": {"message": "No commit found for SHA"}
        }));
        assert_eq!((read.changes, read.failed), (1, 1), "{:?}", read.error);
        assert_eq!(read.failed_shas, vec![at(0, None).sha]);
        for (id, state, detail) in &states {
            assert_eq!(state, "not-checked", "{id}: {detail}");
            assert!(
                detail.starts_with("the forge could not answer for 1 of 1 changes"),
                "{id}: {detail}"
            );
            assert!(
                detail.contains("the forge does not have commit"),
                "{id}: {detail}"
            );
        }
        // The message names the commit by its short id only.
        let error = read.error.unwrap();
        let sha = at(0, None).sha;
        assert!(error.contains(&sha[..10.min(sha.len())]), "{error}");
        assert!(
            !error.contains("http") && !error.contains("token"),
            "{error}"
        );
    }

    #[test]
    fn a_commit_the_forge_has_outside_any_pull_request_is_read_and_clean() {
        // The control: GitHub answers 200 with an empty list for a commit it has that
        // no pull request carries. That is an answer, so nothing failed and the review
        // check is clean.
        let (read, states) = forge_states(serde_json::json!([]));
        assert_eq!((read.changes, read.failed), (1, 0), "{:?}", read.error);
        let review = states.iter().find(|s| s.0 == "independent-review").unwrap();
        assert_eq!(review.1, "clean", "{}", review.2);
        assert_eq!(review.2, "no change arrived through a pull request");
        // A 404 is not that answer (#634): GitHub gives it for a repository that is not
        // there or that the token cannot see, so nothing about the review was read.
        let (read, states) = forge_states(serde_json::json!({"__status": 404}));
        assert_eq!((read.changes, read.failed), (1, 1), "{:?}", read.error);
        assert_eq!(read.failed_shas, vec![at(0, None).sha]);
        for (id, state, detail) in &states {
            assert_eq!(state, "not-checked", "{id}: {detail}");
        }
        let error = read.error.unwrap();
        assert!(error.contains("HTTP 404"), "{error}");
        assert!(!error.contains("does not have commit"), "{error}");
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
            body_edited_after_merge: None,
            merged_at: None,
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
