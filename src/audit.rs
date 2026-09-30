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
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Summary {
    /// [`crate::output_schema::AUDIT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The ref audited, as given or defaulted.
    pub reference: String,
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
    /// Newest change first; within a change, in the order the kinds are listed above.
    pub records: Vec<Record>,
}

impl Summary {
    pub fn from_records(reference: String, changes: usize, records: Vec<Record>) -> Self {
        let mut s = Summary {
            schema_version: crate::output_schema::AUDIT_SCHEMA_VERSION,
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
        s.records = records;
        s
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for r in &self.records {
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
                "config" => format!(
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
}

impl ChangeInfoRef {
    fn to_owned(&self) -> ChangeInfo {
        ChangeInfo {
            sha: self.sha.clone(),
            pr: self.pr,
            time: self.time,
        }
    }
}

/// The configuration records of one change, from the two sides' `discipline.toml` text
/// (`None`: the side has no such file).
pub fn config_records(base: Option<&str>, head: Option<&str>, info: &ChangeInfoRef) -> Vec<Record> {
    let change = info.to_owned();
    if base == head {
        return Vec::new();
    }
    // Adopting a configuration is not a loosening: there was none to loosen.
    let Some(base) = base else {
        return Vec::new();
    };
    let unreadable = |detail: String| {
        let mut r = Record::new(&change, "config-unreadable", "config");
        r.file = Some(CONFIG_NAME.to_string());
        r.detail = Some(detail);
        vec![r]
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
    match integrity::diff_configs(&base_cfg, &head_cfg) {
        Ok(found) => found
            .into_iter()
            .map(|w| {
                let mut r = Record::new(&change, "config", "config");
                r.file = Some(CONFIG_NAME.to_string());
                r.gate = Some(w.gate);
                r.key = Some(w.key);
                r.change = Some(w.change);
                r.before = w.before;
                r.after = w.after;
                r.count = w.count;
                r
            })
            .collect(),
        Err(e) => unreadable(format!("compare: {}", first_line(&e.to_string()))),
    }
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

/// The records of one first-parent change.
fn change_records(repo: &Repository, c: &Commit, reasons: bool) -> Result<Vec<Record>> {
    let parent = c.parent(0)?;
    let (pt, ct) = (parent.tree()?, c.tree()?);
    let subject = c.summary().ok().flatten().unwrap_or("").to_string();
    let info = ChangeInfoRef {
        sha: c.id().to_string(),
        pr: crate::replay::pr_from_subject(&subject),
        time: c.time().seconds(),
    };
    let mut out = directive_records(c.message().unwrap_or(""), &info, reasons);
    out.extend(config_records(
        blob_text(repo, &pt, CONFIG_NAME)?.as_deref(),
        blob_text(repo, &ct, CONFIG_NAME)?.as_deref(),
        &info,
    ));
    out.extend(baseline_records(
        blob_text(repo, &pt, BASELINE_NAME)?.as_deref(),
        blob_text(repo, &ct, BASELINE_NAME)?.as_deref(),
        &info,
    ));
    out.extend(inline_records(repo, &pt, &ct, &info)?);
    Ok(out)
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
    for c in &commits {
        records.extend(
            change_records(&repo, c, opts.reasons)
                .with_context(|| format!("cannot read change {}", c.id()))?,
        );
    }
    Ok(Summary::from_records(reference, commits.len(), records))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> ChangeInfoRef {
        ChangeInfoRef {
            sha: "0123456789abcdef".into(),
            pr: Some(7),
            time: 1,
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
            },
            false,
        ));
        let s = Summary::from_records("main".into(), 5, records);
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
}
