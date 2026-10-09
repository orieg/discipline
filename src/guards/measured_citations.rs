//! What a `(measured: <host>, <commit>)` tag cites (`provenance-tags`, opt-in keys).
//!
//! The gate's other rules check that a published figure carries a tag. These check what
//! the tag says, in three independent steps:
//!
//! - `verify_measured_commit`: a `measured` tag on an added line names a host and a commit,
//!   neither a placeholder, and the commit resolves to a commit object in the local object
//!   database.
//! - `record_paths`: the commit key of every added or changed JSON / JSONL result record is
//!   a full object id that resolves to a commit.
//! - `verify_cited_figures`: each figure of a paragraph or table carrying a `measured` tag
//!   equals a numeric value of the tracked data artifact the paragraph cites.
//!
//! Nothing here reads the network. A commit is looked up in the local object database and
//! an artifact is read from the head side of the change, both through [`Evidence`], so
//! every rule is testable without a repository.
//!
//! What cannot be decided is never a pass: an id absent from a shallow clone, an
//! abbreviation more than one object starts with, and an artifact or record file that does
//! not parse stop the check.

use super::provenance_tags::{paragraphs, strip_fences, HygieneFinding, UNIT_TOKEN};
use crate::findings::{
    FIGURE_DISAGREES_WITH_ARTIFACT, PLACEHOLDER_PROVENANCE_TAG, UNRESOLVABLE_MEASURED_COMMIT,
};
use crate::gitctx::CommitLookup;
use crate::tokens;
use anyhow::{anyhow, bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

/// Fewest hexadecimal digits a tag's commit field may abbreviate an object id to.
pub const MIN_ABBREVIATION: usize = 7;
/// Hexadecimal digits of a full object id.
pub const FULL_ID: usize = 40;

/// Extensions of the data artifacts a tagged paragraph can cite.
const ARTIFACT_EXTENSIONS: &[&str] = &[".jsonl", ".json", ".csv"];

/// The repository facts the rules read.
pub trait Evidence {
    /// What the hexadecimal id `hex` names in the local object database.
    fn lookup_commit(&self, hex: &str) -> Result<CommitLookup>;
    /// Whether the clone's history is truncated, so an absent object may exist upstream.
    fn is_shallow(&self) -> bool;
    /// Bytes of the tracked file `path` on the head side; `None` when it is not tracked
    /// or not present there.
    fn artifact(&self, path: &str) -> Result<Option<Vec<u8>>>;
}

impl Evidence for crate::gitctx::GitCtx {
    fn lookup_commit(&self, hex: &str) -> Result<CommitLookup> {
        crate::gitctx::GitCtx::lookup_commit(self, hex)
    }

    fn is_shallow(&self) -> bool {
        crate::gitctx::GitCtx::is_shallow(self)
    }

    fn artifact(&self, path: &str) -> Result<Option<Vec<u8>>> {
        if !self.is_tracked(path)? {
            return Ok(None);
        }
        self.head_bytes(path)
    }
}

/// A `(measured ...)` tag found in a paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredTag {
    /// Byte range of the tag in the paragraph's text, parentheses included.
    pub span: Range<usize>,
    /// Host and commit, when the tag has the form `(measured: <host>, <commit>[, ...])`.
    pub fields: Option<(String, String)>,
}

/// Every `(measured ...)` tag of `text`, in order. The word is matched without regard to
/// case and must end at `(measured`: `(measurements ...)` is not a tag.
pub fn measured_tags(text: &str) -> Vec<MeasuredTag> {
    const OPEN: &str = "(measured";
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find(OPEN) {
        let start = at + found;
        let body_start = start + OPEN.len();
        at = body_start;
        if text[body_start..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        // The tag ends at the parenthesis that closes it; one opened inside it is part
        // of a field.
        let mut depth = 0usize;
        let mut close = None;
        for (i, c) in text[body_start..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' if depth == 0 => {
                    close = Some(body_start + i);
                    break;
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        let (end, fields) = match close {
            Some(close) => (close + 1, tag_fields(&text[body_start..close])),
            None => (body_start, None),
        };
        out.push(MeasuredTag {
            span: start..end,
            fields,
        });
        at = end;
    }
    out
}

/// Host and commit of a tag body (what follows `(measured`): a colon, then fields
/// separated by `,` or `;`. Fewer than two fields is not the form.
fn tag_fields(body: &str) -> Option<(String, String)> {
    let rest = body.trim_start().strip_prefix(':')?;
    let mut fields = rest.split([',', ';']).map(|f| {
        f.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_matches(['`', '*'])
            .trim()
            .to_string()
    });
    let host = fields.next()?;
    let commit = fields.next()?;
    Some((host, commit))
}

pub(crate) fn is_hex_id(s: &str, digits: std::ops::RangeInclusive<usize>) -> bool {
    digits.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What is wrong with one tag or record commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The tag names no host or no commit.
    Placeholder(String),
    /// The commit named resolves to no commit object.
    Unresolvable(String),
    /// The repository cannot say: the check stops.
    CannotCheck(String),
}

/// Looks `id` up as a commit. `None` when it is one.
fn commit_problem(id: &str, ev: &dyn Evidence) -> Result<Option<Problem>> {
    let id = id.to_ascii_lowercase();
    Ok(match ev.lookup_commit(&id)? {
        CommitLookup::Commit(_) => None,
        CommitLookup::NotACommit => Some(Problem::Unresolvable(format!(
            "`{id}` names an object that is not a commit"
        ))),
        CommitLookup::Ambiguous => Some(Problem::CannotCheck(format!(
            "abbreviated id `{id}` matches more than one object (write more digits)"
        ))),
        CommitLookup::Missing if ev.is_shallow() => Some(Problem::CannotCheck(format!(
            "`{id}` is not in this shallow clone, which cannot say whether it exists (fetch the full history)"
        ))),
        CommitLookup::Missing => Some(Problem::Unresolvable(format!(
            "`{id}` names no object in this repository"
        ))),
    })
}

/// Everything wrong with a tag's fields. The host is never quoted: a host name can be
/// an internal one.
pub fn judge_tag(fields: Option<&(String, String)>, ev: &dyn Evidence) -> Result<Vec<Problem>> {
    let Some((host, commit)) = fields else {
        return Ok(vec![Problem::Placeholder(
            "does not have the form `(measured: <host>, <commit>)`, so it names no host and no commit"
                .to_string(),
        )]);
    };
    let mut problems = Vec::new();
    if tokens::is_placeholder_field(host) {
        problems.push(Problem::Placeholder(
            "has a placeholder where the host is named".to_string(),
        ));
    }
    // A field that is an object id is looked up, never read as a word.
    if is_hex_id(commit, MIN_ABBREVIATION..=FULL_ID) {
        problems.extend(commit_problem(commit, ev)?);
    } else if tokens::is_placeholder_field(commit) {
        problems.push(Problem::Placeholder(
            "has a placeholder where the commit is named".to_string(),
        ));
    } else {
        problems.push(Problem::Unresolvable(format!(
            "has a commit field that is not an object id ({MIN_ABBREVIATION} to {FULL_ID} hexadecimal digits; a branch or tag name is not accepted)"
        )));
    }
    Ok(problems)
}

/// One reading of a figure's number: its value and the decimals the document shows.
pub type Reading = (f64, usize);

/// A figure of a document line.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    /// The figure as written, unit included.
    pub text: String,
    /// The values the number can be read as. `1,200` is read as both 1200 and 1.200.
    pub readings: Vec<Reading>,
}

/// The readings of a number as written. A comma followed by groups of three digits
/// separates thousands; a single comma can also be a decimal mark.
fn number_readings(literal: &str) -> Vec<Reading> {
    let (int, frac) = match literal.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (literal, None),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if frac.is_some_and(|f| !digits(f)) {
        return Vec::new();
    }
    let decimals = frac.map_or(0, str::len);
    let with_frac = |int: &str| match frac {
        Some(f) => format!("{int}.{f}"),
        None => int.to_string(),
    };
    let mut out = Vec::new();
    let groups: Vec<&str> = int.split(',').collect();
    if groups.iter().all(|g| digits(g)) {
        if groups.len() == 1 {
            out.extend(with_frac(int).parse::<f64>().ok().map(|v| (v, decimals)));
        } else {
            if groups[0].len() <= 3 && groups[1..].iter().all(|g| g.len() == 3) {
                let joined = with_frac(&groups.concat());
                out.extend(joined.parse::<f64>().ok().map(|v| (v, decimals)));
            }
            if groups.len() == 2 && frac.is_none() {
                let decimal = format!("{}.{}", groups[0], groups[1]);
                out.extend(decimal.parse::<f64>().ok().map(|v| (v, groups[1].len())));
            }
        }
    }
    out.retain(|(v, _)| v.is_finite());
    out
}

/// The figures of a line, as the gate's other rules define a figure (`UNIT_TOKEN`: a
/// number carrying a unit, or a multiplier). The number is read in full: digits and
/// separators the pattern leaves to its left belong to it, and so does a leading minus.
pub fn figures(line: &str) -> Vec<Figure> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    for m in UNIT_TOKEN.find_iter(line) {
        let matched = m.as_str();
        let number_len = matched
            .bytes()
            .take_while(|b| b.is_ascii_digit() || matches!(b, b'.' | b','))
            .count();
        let number_end = m.start() + number_len;
        let mut start = m.start();
        // A digit to the left belongs to the number, and so does a separator between
        // two digits.
        while start >= 1 {
            let digit = bytes[start - 1].is_ascii_digit();
            let separator = start >= 2
                && matches!(bytes[start - 1], b'.' | b',')
                && bytes[start - 2].is_ascii_digit();
            if !(digit || separator) {
                break;
            }
            start -= 1;
        }
        let mut readings = number_readings(&line[start..number_end]);
        if readings.is_empty() {
            // Not a number once extended (`1.2.3 ms`): the pattern's own match stands.
            start = m.start();
            readings = number_readings(&line[start..number_end]);
        }
        let before = &line[..start];
        let minus = ['-', '\u{2212}']
            .into_iter()
            .find(|sign| before.ends_with(*sign))
            .filter(|sign| {
                before[..before.len() - sign.len_utf8()]
                    .chars()
                    .next_back()
                    .is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '|' | '*' | '`'))
            });
        let text = match minus {
            Some(_) => {
                readings.iter_mut().for_each(|(v, _)| *v = -*v);
                format!("-{}", &line[start..m.end()])
            }
            None => line[start..m.end()].to_string(),
        };
        out.push(Figure { text, readings });
    }
    out
}

/// Whether a document's reading equals an artifact's value: the value rounds to the
/// reading at the decimals the document shows, or lies within `tolerance_pct` percent of
/// the value.
pub fn agrees((shown, decimals): Reading, value: f64, tolerance_pct: f64) -> bool {
    let diff = (value - shown).abs();
    let half_unit = 0.5 * 10f64.powi(-(decimals.min(300) as i32));
    // The bound is widened by a part in a thousand million so that a value exactly half
    // a unit away (0.125 shown as 0.13) is not lost to binary representation.
    diff <= half_unit * (1.0 + 1e-9) || diff <= tolerance_pct / 100.0 * value.abs()
}

fn json_numbers(value: &serde_json::Value, out: &mut Vec<f64>) {
    match value {
        serde_json::Value::Number(n) => out.extend(n.as_f64()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| json_numbers(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| json_numbers(v, out)),
        _ => {}
    }
}

/// The cells of comma-separated text. A quoted cell may hold commas, line breaks and
/// doubled quotes; a quote left open is an error.
fn csv_cells(text: &str) -> std::result::Result<Vec<String>, String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut line = 1usize;
    let mut opened_at = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            line += 1;
        }
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                other => cell.push(other),
            }
        } else {
            match c {
                '"' if cell.trim().is_empty() => {
                    cell.clear();
                    quoted = true;
                    opened_at = line;
                }
                ',' | '\n' => cells.push(std::mem::take(&mut cell)),
                '\r' => {}
                other => cell.push(other),
            }
        }
    }
    if quoted {
        return Err(format!(
            "a quoted cell opened on line {opened_at} is never closed"
        ));
    }
    cells.push(cell);
    Ok(cells)
}

/// Every numeric value of a data artifact: each numeric leaf of JSON (each line of
/// JSONL), each numeric cell of CSV. Content that does not parse is an error.
pub fn artifact_numbers(path: &str, bytes: &[u8]) -> Result<Vec<f64>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| anyhow!("cited artifact `{path}` is not UTF-8 text"))?;
    let lower = path.to_ascii_lowercase();
    let mut out = Vec::new();
    if lower.ends_with(".jsonl") {
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
                anyhow!(
                    "cited artifact `{path}` does not parse: line {} is not JSON ({:?})",
                    i + 1,
                    e.classify()
                )
            })?;
            json_numbers(&value, &mut out);
        }
    } else if lower.ends_with(".json") {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| anyhow!("cited artifact `{path}` does not parse as JSON: {e}"))?;
        json_numbers(&value, &mut out);
    } else {
        let cells = csv_cells(text)
            .map_err(|why| anyhow!("cited artifact `{path}` does not parse as CSV: {why}"))?;
        for cell in cells {
            let cell = cell.trim();
            let numeric_start = cell
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.'));
            if numeric_start {
                out.extend(cell.parse::<f64>().ok().filter(|v| v.is_finite()));
            }
        }
    }
    Ok(out)
}

/// Path references to data artifacts in `text`, each with its byte range. A reference is
/// a word ending in `.json`, `.jsonl` or `.csv`; a URL is not a path.
pub fn artifact_citations(text: &str) -> Vec<(Range<usize>, String)> {
    let is_break = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | '*' | ',' | ';' | '|'
            )
    };
    let mut out = Vec::new();
    let mut start = None;
    let mut push = |range: Range<usize>| {
        let raw = &text[range.clone()];
        let word = raw.trim_end_matches(['.', ':', '!', '?']);
        let lower = word.to_ascii_lowercase();
        let Some(ext) = ARTIFACT_EXTENSIONS.iter().find(|e| lower.ends_with(**e)) else {
            return;
        };
        let stem = &word[..word.len() - ext.len()];
        if word.contains("://") || stem.is_empty() || stem.ends_with('/') {
            return;
        }
        let path = word.strip_prefix("./").unwrap_or(word);
        out.push((range.start..range.start + word.len(), path.to_string()));
    };
    for (i, c) in text.char_indices() {
        match (is_break(c), start) {
            (true, Some(s)) => {
                push(s..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        push(s..text.len());
    }
    out
}

/// The numeric values of each artifact read so far; `None` for a path that is not a
/// tracked file on the head side.
#[derive(Default)]
pub struct Artifacts(BTreeMap<String, Option<Vec<f64>>>);

impl Artifacts {
    fn values(&mut self, path: &str, ev: &dyn Evidence) -> Result<Option<&Vec<f64>>> {
        if !self.0.contains_key(path) {
            let values = match ev.artifact(path)? {
                Some(bytes) => Some(artifact_numbers(path, &bytes)?),
                None => None,
            };
            self.0.insert(path.to_string(), values);
        }
        Ok(self.0[path].as_ref())
    }
}

/// Which of the document rules run.
#[derive(Debug, Clone, Copy, Default)]
pub struct DocPolicy {
    pub verify_measured_commit: bool,
    pub verify_cited_figures: bool,
    pub figure_tolerance_pct: f64,
    /// Compare figures only in paragraphs holding an added line (the gate's `diff_only`).
    pub figures_on_added_lines_only: bool,
}

/// What one document's scan found.
#[derive(Default)]
pub struct DocScan {
    pub findings: Vec<HygieneFinding>,
    /// What the repository could not decide, each naming its place.
    pub cannot_check: Vec<String>,
    /// Tags judged by `verify_measured_commit`.
    pub tags_judged: usize,
    /// Tagged paragraphs whose figures were compared with an artifact.
    pub paragraphs_compared: usize,
    /// Tagged paragraphs holding figures and citing no data artifact.
    pub uncited: usize,
}

const TAG_REMEDIATION: &str =
    "Write the tag as (measured: <host>, <commit>) with the host the figure was measured on and the id of the commit it was measured at.";
const COMMIT_REMEDIATION: &str =
    "Name a commit this repository holds, by its full id or an abbreviation of at least 7 hexadecimal digits.";
const FIGURE_REMEDIATION: &str =
    "Quote the value the cited artifact records, store the derived figure in the artifact, or cite the artifact that records it.";

/// One paragraph's text with its line numbers kept: byte offset of each line's start.
struct Block {
    text: String,
    starts: Vec<(usize, usize)>,
}

impl Block {
    fn new(lines: &[(usize, String)]) -> Self {
        let mut text = String::new();
        let mut starts = Vec::new();
        for (number, line) in lines {
            if !text.is_empty() {
                text.push('\n');
            }
            starts.push((text.len(), *number));
            text.push_str(line);
        }
        Self { text, starts }
    }

    fn line_at(&self, offset: usize) -> usize {
        self.starts
            .iter()
            .rev()
            .find(|(start, _)| *start <= offset)
            .map_or(1, |(_, number)| *number)
    }
}

fn is_table(lines: &[(usize, String)]) -> bool {
    lines.iter().all(|(_, l)| l.trim_start().starts_with('|'))
}

fn has_table_row(lines: &[(usize, String)]) -> bool {
    lines.iter().any(|(_, l)| l.trim_start().starts_with('|'))
}

/// Applies the document rules to markdown `text`. `added` holds the lines the change
/// added; `None` judges every line (a pull-request description).
pub fn scan_document(
    text: &str,
    label: &str,
    added: Option<&BTreeSet<usize>>,
    policy: &DocPolicy,
    ev: &dyn Evidence,
    artifacts: &mut Artifacts,
) -> Result<DocScan> {
    let mut scan = DocScan::default();
    let stripped = strip_fences(&text.lines().collect::<Vec<_>>());
    let paras = paragraphs(stripped);
    let touched =
        |from: usize, to: usize| added.is_none_or(|a| a.range(from..=to).next().is_some());

    for (idx, para) in paras.iter().enumerate() {
        let block = Block::new(para);
        let tags = measured_tags(&block.text);
        if tags.is_empty() {
            continue;
        }

        if policy.verify_measured_commit {
            for tag in &tags {
                let first = block.line_at(tag.span.start);
                let last = block.line_at(tag.span.end.saturating_sub(1));
                // A tag the change did not write is not judged: its commit may have
                // been dropped by a squash merge since.
                if !touched(first, last) {
                    continue;
                }
                scan.tags_judged += 1;
                for problem in judge_tag(tag.fields.as_ref(), ev)? {
                    match problem {
                        Problem::Placeholder(what) => scan.findings.push(HygieneFinding {
                            line: first,
                            kind: &PLACEHOLDER_PROVENANCE_TAG,
                            message: format!("`(measured ...)` tag {what}."),
                            remediation: TAG_REMEDIATION,
                            is_warning: false,
                        }),
                        Problem::Unresolvable(what) => scan.findings.push(HygieneFinding {
                            line: first,
                            kind: &UNRESOLVABLE_MEASURED_COMMIT,
                            message: if what.starts_with('`') {
                                format!("`(measured ...)` tag names a commit that does not resolve: {what}.")
                            } else {
                                format!("`(measured ...)` tag {what}.")
                            },
                            remediation: COMMIT_REMEDIATION,
                            is_warning: false,
                        }),
                        Problem::CannotCheck(what) => {
                            scan.cannot_check.push(format!("{label}:{first}: {what}"))
                        }
                    }
                }
            }
        }

        if !policy.verify_cited_figures {
            continue;
        }
        // The tagged paragraph, and the table it captions when a blank line parts them.
        let mut scope: Vec<(usize, String)> = para.clone();
        if !has_table_row(para) {
            if let Some(next) = paras.get(idx + 1) {
                if is_table(next) && measured_tags(&Block::new(next).text).is_empty() {
                    scope.extend(next.iter().cloned());
                }
            }
        }
        let (Some((from, _)), Some((to, _))) = (scope.first(), scope.last()) else {
            continue;
        };
        if policy.figures_on_added_lines_only && !touched(*from, *to) {
            continue;
        }
        let scope = Block::new(&scope);
        let citations = artifact_citations(&scope.text);
        // A number inside the tag (a host named `M1 16 GB`) or inside a path is not a
        // figure the paragraph publishes.
        let mut masked = scope.text.clone().into_bytes();
        let hidden = measured_tags(&scope.text)
            .into_iter()
            .map(|t| t.span)
            .chain(citations.iter().map(|(range, _)| range.clone()));
        for range in hidden {
            for b in &mut masked[range] {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
        let masked = String::from_utf8_lossy(&masked).into_owned();
        let mut found: Vec<(usize, Figure)> = Vec::new();
        for ((_, number), line) in scope.starts.iter().zip(masked.split('\n')) {
            found.extend(figures(line).into_iter().map(|f| (*number, f)));
        }
        if found.is_empty() {
            continue;
        }
        if citations.is_empty() {
            scan.uncited += 1;
            continue;
        }

        let mut cited: Vec<String> = Vec::new();
        let mut values: Vec<f64> = Vec::new();
        let mut seen = BTreeSet::new();
        for (range, path) in &citations {
            if !seen.insert(path.clone()) {
                continue;
            }
            match artifacts.values(path, ev)? {
                Some(v) => {
                    cited.push(format!("`{path}`"));
                    values.extend(v);
                }
                None => scan.findings.push(HygieneFinding {
                    line: scope.line_at(range.start),
                    kind: &FIGURE_DISAGREES_WITH_ARTIFACT,
                    message: format!(
                        "Cited artifact `{path}` is not a tracked file on the head side, so the figures beside it agree with nothing."
                    ),
                    remediation: "Cite a data artifact (.json, .jsonl, .csv) that is committed to the repository.",
                    is_warning: false,
                }),
            }
        }
        if cited.is_empty() {
            continue;
        }
        scan.paragraphs_compared += 1;
        for (line, figure) in found {
            let matched = figure.readings.iter().any(|reading| {
                values
                    .iter()
                    .any(|v| agrees(*reading, *v, policy.figure_tolerance_pct))
            });
            if !matched {
                scan.findings.push(HygieneFinding {
                    line,
                    kind: &FIGURE_DISAGREES_WITH_ARTIFACT,
                    message: format!(
                        "Figure `{}` beside a `(measured ...)` tag matches no value in {}.",
                        figure.text,
                        cited.join(", ")
                    ),
                    remediation: FIGURE_REMEDIATION,
                    is_warning: false,
                });
            }
        }
    }
    Ok(scan)
}

/// A result record whose commit does not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFinding {
    /// Line of the record (JSONL); a JSON record has none.
    pub line: Option<usize>,
    /// Where the record sits in a JSON file (`[2].commit`).
    pub anchor: Option<String>,
    pub message: String,
}

/// What one record file's scan found.
#[derive(Debug, Default)]
pub struct RecordScan {
    pub findings: Vec<RecordFinding>,
    pub cannot_check: Vec<String>,
    /// Records judged (added or changed).
    pub judged: usize,
}

/// What is wrong with a record's commit. A value that is not an object id is never
/// quoted: it is whatever the harness wrote there.
fn record_problem(
    record: &serde_json::Value,
    key: &str,
    ev: &dyn Evidence,
) -> Result<(String, Option<Problem>)> {
    let run_file = record.get("schema").and_then(|s| s.as_str())
        == Some(super::perf::paired_ratio::RUN_SCHEMA);
    let (name, value) = if run_file {
        (
            "provenance.commit".to_string(),
            record.get("provenance").and_then(|p| p.get("commit")),
        )
    } else {
        (key.to_string(), record.get(key))
    };
    let unresolved = |what: &str| Some(Problem::Unresolvable(format!("`{name}` {what}")));
    let problem = match value {
        None if record.is_object() => unresolved("is missing"),
        None => unresolved("is missing: the record is not an object"),
        Some(serde_json::Value::String(id)) if is_hex_id(id, FULL_ID..=FULL_ID) => {
            match commit_problem(id, ev)? {
                Some(Problem::Unresolvable(what)) => {
                    unresolved(&format!("does not resolve: {what}"))
                }
                other => other,
            }
        }
        Some(serde_json::Value::String(_)) => {
            unresolved("is not a full object id (40 hexadecimal digits)")
        }
        Some(_) => unresolved("is not a string"),
    };
    Ok((name, problem))
}

/// Checks the commit of every added or changed record of a JSON / JSONL file. A JSONL
/// record is a line, judged when the change added the line. A JSON file is one record, or
/// an array of records; a record is judged when the base side holds no equal one.
pub fn scan_records(
    path: &str,
    head: &str,
    base: Option<&str>,
    added: &BTreeSet<usize>,
    key: &str,
    ev: &dyn Evidence,
) -> Result<RecordScan> {
    let mut scan = RecordScan::default();
    let judge = |record: &serde_json::Value,
                 line: Option<usize>,
                 index: Option<usize>,
                 scan: &mut RecordScan|
     -> Result<()> {
        scan.judged += 1;
        let (name, problem) = record_problem(record, key, ev)?;
        let anchor = match (line, index) {
            (Some(_), _) => None,
            (None, Some(i)) => Some(format!("[{i}].{name}")),
            (None, None) => Some(name),
        };
        let place = match (line, index) {
            (Some(l), _) => format!("{path}:{l}"),
            (None, Some(i)) => format!("{path}: record {i}"),
            (None, None) => path.to_string(),
        };
        match problem {
            None => {}
            Some(Problem::CannotCheck(what)) => scan.cannot_check.push(format!("{place}: {what}")),
            Some(Problem::Unresolvable(what)) | Some(Problem::Placeholder(what)) => {
                scan.findings.push(RecordFinding {
                    line,
                    anchor,
                    message: match index {
                        Some(i) => format!("Result record {i}: {what}."),
                        None => format!("Result record: {what}."),
                    },
                })
            }
        }
        Ok(())
    };

    if path.to_ascii_lowercase().ends_with(".jsonl") {
        for (i, line) in head.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: serde_json::Value = serde_json::from_str(line).map_err(|e| {
                anyhow!(
                    "result record file `{path}` does not parse: line {} is not JSON ({:?})",
                    i + 1,
                    e.classify()
                )
            })?;
            if added.contains(&(i + 1)) {
                judge(&record, Some(i + 1), None, &mut scan)?;
            }
        }
        return Ok(scan);
    }

    let value: serde_json::Value = serde_json::from_str(head)
        .map_err(|e| anyhow!("result record file `{path}` does not parse as JSON: {e}"))?;
    // A base side that does not parse holds no record equal to any of the head's.
    let before: Option<serde_json::Value> = base.and_then(|b| serde_json::from_str(b).ok());
    match &value {
        serde_json::Value::Array(records) => {
            let known: &[serde_json::Value] = match &before {
                Some(serde_json::Value::Array(b)) => b,
                _ => &[],
            };
            for (i, record) in records.iter().enumerate() {
                if !known.contains(record) {
                    judge(record, None, Some(i), &mut scan)?;
                }
            }
        }
        record => {
            if before.as_ref() != Some(record) {
                judge(record, None, None, &mut scan)?;
            }
        }
    }
    Ok(scan)
}

/// A `figure_tolerance_pct` the gate can use: a finite number, zero or above.
pub fn check_tolerance(pct: f64) -> Result<()> {
    if !pct.is_finite() || pct < 0.0 {
        bail!("provenance-tags: `figure_tolerance_pct` must be a number of percent, zero or above");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A repository told in a table: the commits and other objects it holds.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub commits: Vec<&'static str>,
        pub other_objects: Vec<&'static str>,
        pub shallow: bool,
        pub files: Vec<(&'static str, &'static str)>,
    }

    impl Evidence for Fake {
        fn lookup_commit(&self, hex: &str) -> Result<CommitLookup> {
            let all: Vec<(&str, bool)> = self
                .commits
                .iter()
                .map(|c| (*c, true))
                .chain(self.other_objects.iter().map(|o| (*o, false)))
                .filter(|(id, _)| id.starts_with(hex))
                .collect();
            Ok(match all.as_slice() {
                [] => CommitLookup::Missing,
                [(id, true)] => CommitLookup::Commit(id.to_string()),
                [(_, false)] => CommitLookup::NotACommit,
                _ => CommitLookup::Ambiguous,
            })
        }

        fn is_shallow(&self) -> bool {
            self.shallow
        }

        fn artifact(&self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self
                .files
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, c)| c.as_bytes().to_vec()))
        }
    }

    const C1: &str = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const C2: &str = "1111111bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TREE: &str = "2222222ccccccccccccccccccccccccccccccccc";

    fn repo() -> Fake {
        Fake {
            commits: vec![C1, C2],
            other_objects: vec![TREE],
            ..Fake::default()
        }
    }

    fn commit_only(text: &str, ev: &Fake) -> DocScan {
        scan_document(
            text,
            "d.md",
            None,
            &DocPolicy {
                verify_measured_commit: true,
                ..DocPolicy::default()
            },
            ev,
            &mut Artifacts::default(),
        )
        .unwrap()
    }

    fn codes(scan: &DocScan) -> Vec<&'static str> {
        scan.findings.iter().map(|f| f.kind.code).collect()
    }

    #[test]
    fn a_tag_is_parsed_into_host_and_commit_and_other_shapes_have_no_fields() {
        let fields = |t: &str| {
            measured_tags(t)
                .into_iter()
                .map(|t| t.fields)
                .collect::<Vec<_>>()
        };
        let pair = |h: &str, c: &str| Some((h.to_string(), c.to_string()));
        assert_eq!(
            fields("(measured: box-1, abc1234)"),
            [pair("box-1", "abc1234")]
        );
        assert_eq!(
            fields("*(Measured: Apple M1 (16 GB), `abc1234`; workload: w)*"),
            [pair("Apple M1 (16 GB)", "abc1234")]
        );
        // A tag wrapped over two lines is one tag.
        assert_eq!(
            fields("(measured: box-1,\n  abc1234, quiet)"),
            [pair("box-1", "abc1234")]
        );
        // Not the form: no colon, one field, never closed.
        assert_eq!(fields("(measured on a laptop, abc1234)"), [None]);
        assert_eq!(fields("(measured: box-1)"), [None]);
        assert_eq!(fields("(measured: box-1, abc1234"), [None]);
        assert_eq!(fields("(measured)"), [None]);
        // Not a tag at all.
        assert!(fields("(measurements: box-1, abc1234) and (target)").is_empty());
        assert!(fields("(measuredness: a, b)").is_empty());
        // Two tags in one paragraph.
        assert_eq!(
            fields("(measured: a1, abc1234) then (measured: b2, def5678)").len(),
            2
        );
    }

    #[test]
    fn a_placeholder_host_or_commit_is_reported_and_a_named_one_is_not() {
        let ev = repo();
        // Control: a named host and a commit the repository holds.
        let good = commit_only(&format!("(measured: box-1, {C1})"), &ev);
        assert!(
            good.findings.is_empty() && good.tags_judged == 1,
            "{:?}",
            codes(&good)
        );

        for tag in [
            "(measured: host, commit)",
            "(measured: <host>, <commit>)",
            "(measured: HOST, sha)",
            "(measured: tbd, unknown)",
            "(measured: n/a, n/a)",
            "(measured: , )",
        ] {
            let scan = commit_only(tag, &ev);
            assert_eq!(
                codes(&scan),
                ["placeholder-provenance-tag", "placeholder-provenance-tag"],
                "{tag}"
            );
        }
        // One field a placeholder, the other good: one finding, about that field.
        let host = commit_only(&format!("(measured: hostname, {C1})"), &ev);
        assert_eq!(codes(&host), ["placeholder-provenance-tag"]);
        assert!(
            host.findings[0].message.contains("host"),
            "{}",
            host.findings[0].message
        );
        let commit = commit_only("(measured: box-1, commit-sha)", &ev);
        assert_eq!(codes(&commit), ["placeholder-provenance-tag"]);
        assert!(commit.findings[0].message.contains("commit"));
        // A tag without the form names neither.
        for tag in ["(measured)", "(measured on a laptop)", "(measured: box-1)"] {
            assert_eq!(
                codes(&commit_only(tag, &ev)),
                ["placeholder-provenance-tag"],
                "{tag}"
            );
        }
        // The host is never quoted.
        let quoted = commit_only("(measured: build-07.corp.example, commit)", &ev);
        assert!(!quoted.findings[0].message.contains("build-07"));
    }

    #[test]
    fn an_object_id_made_of_placeholder_letters_is_looked_up_not_read_as_a_word() {
        // `70d07bd` folds to `todotbd`; as an id it resolves.
        let ev = Fake {
            commits: vec!["70d07bd000000000000000000000000000000000"],
            ..Fake::default()
        };
        assert!(commit_only("(measured: box-1, 70d07bd)", &ev)
            .findings
            .is_empty());
    }

    #[test]
    fn a_commit_field_must_resolve_to_exactly_one_commit_object() {
        let ev = repo();
        let judged = |tag: &str| commit_only(tag, &ev);
        // Controls: the full id, an abbreviation only one object starts with, upper case.
        assert!(judged(&format!("(measured: box-1, {C1})"))
            .findings
            .is_empty());
        assert!(judged("(measured: box-1, 1111111a)").findings.is_empty());
        assert!(judged("(measured: box-1, 1111111A)").findings.is_empty());

        let absent = judged("(measured: box-1, 9999999)");
        assert_eq!(codes(&absent), ["unresolvable-measured-commit"]);
        assert!(absent.findings[0].message.contains("`9999999`"));
        // A tree is an object, not a commit.
        assert_eq!(
            codes(&judged("(measured: box-1, 2222222)")),
            ["unresolvable-measured-commit"]
        );
        // A name, and an abbreviation under seven digits, are not ids.
        for field in ["main", "v1.2.0", "111111", "HEAD~1"] {
            let scan = judged(&format!("(measured: box-1, {field})"));
            assert_eq!(codes(&scan), ["unresolvable-measured-commit"], "{field}");
            assert!(
                !scan.findings[0].message.contains(field),
                "{field} is quoted"
            );
        }
        // More than one object starts with it: the repository cannot say which is meant.
        let ambiguous = judged("(measured: box-1, 1111111)");
        assert!(ambiguous.findings.is_empty());
        assert_eq!(ambiguous.cannot_check.len(), 1);
        assert!(
            ambiguous.cannot_check[0].starts_with("d.md:1: "),
            "{:?}",
            ambiguous.cannot_check
        );
    }

    #[test]
    fn an_id_absent_from_a_shallow_clone_cannot_be_checked() {
        let shallow = Fake {
            shallow: true,
            ..repo()
        };
        let scan = commit_only("(measured: box-1, 9999999)", &shallow);
        assert!(scan.findings.is_empty());
        assert_eq!(scan.cannot_check.len(), 1);
        assert!(scan.cannot_check[0].contains("shallow"));
        // Control: an id the shallow clone holds is checked as anywhere else.
        let held = commit_only(&format!("(measured: box-1, {C1})"), &shallow);
        assert!(held.findings.is_empty() && held.cannot_check.is_empty());
        // Control: in a full clone the same id is a finding (see the test above).
        assert_eq!(
            codes(&commit_only("(measured: box-1, 9999999)", &repo())),
            ["unresolvable-measured-commit"]
        );
    }

    #[test]
    fn only_a_tag_on_an_added_line_is_judged() {
        let ev = repo();
        let text = "Old (measured: host, commit).\n\nNew (measured: host, commit).\n\nWrapped (measured: box-1,\n9999999).\n";
        let run = |added: &[usize]| {
            scan_document(
                text,
                "d.md",
                Some(&added.iter().copied().collect()),
                &DocPolicy {
                    verify_measured_commit: true,
                    ..DocPolicy::default()
                },
                &ev,
                &mut Artifacts::default(),
            )
            .unwrap()
        };
        let lines = |s: &DocScan| s.findings.iter().map(|f| f.line).collect::<Vec<_>>();
        assert!(run(&[]).findings.is_empty());
        assert_eq!(lines(&run(&[3])), [3, 3]);
        // A tag is judged when any line it spans is added, and reported where it opens.
        assert_eq!(lines(&run(&[6])), [5]);
        assert_eq!(run(&[1, 3, 5]).tags_judged, 3);
    }

    #[test]
    fn a_tag_inside_a_code_fence_is_not_a_tag() {
        let ev = repo();
        let fenced = "```\n(measured: host, commit)\n```\n";
        assert!(commit_only(fenced, &ev).findings.is_empty());
        assert_eq!(
            commit_only("(measured: host, commit)\n", &ev)
                .findings
                .len(),
            2
        );
    }

    #[test]
    fn a_figure_is_read_with_its_whole_number_sign_and_separators() {
        let read = |line: &str| {
            figures(line)
                .into_iter()
                .map(|f| (f.text, f.readings))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            read("took 12.4 ns"),
            [("12.4 ns".to_string(), vec![(12.4, 1)])]
        );
        assert_eq!(
            read("is 2.90x faster"),
            [("2.90x".to_string(), vec![(2.9, 2)])]
        );
        // Thousands separators: the pattern alone would read `345,678 ns`.
        assert_eq!(
            read("12,345,678 ns"),
            [("12,345,678 ns".to_string(), vec![(12_345_678.0, 0)])]
        );
        // One comma and three digits: thousands or a decimal mark.
        assert_eq!(
            read("1,200 ns"),
            [("1,200 ns".to_string(), vec![(1200.0, 0), (1.2, 3)])]
        );
        assert_eq!(read("1,5 ms"), [("1,5 ms".to_string(), vec![(1.5, 1)])]);
        assert_eq!(
            read("delta -3.2 ms"),
            [("-3.2 ms".to_string(), vec![(-3.2, 1)])]
        );
        // A range is two figures' worth of text, not a negative number.
        assert_eq!(read("10-12 ms")[0].1, [(12.0, 0)]);
        // A number with no unit is not a figure.
        assert!(read("in 2024 we ran 3 rounds").is_empty());
    }

    #[test]
    fn a_reading_agrees_by_rounding_or_within_the_tolerance() {
        // Rounding to the decimals shown.
        assert!(agrees((1.23, 2), 1.2349, 0.0));
        assert!(agrees((1.23, 2), 1.2251, 0.0));
        assert!(!agrees((1.23, 2), 1.2351, 0.0));
        assert!(!agrees((1.23, 2), 1.2249, 0.0));
        assert!(agrees((12.0, 0), 12.4, 0.0) && !agrees((12.0, 0), 12.6, 0.0));
        // Half a unit away, either rounding convention.
        assert!(agrees((0.13, 2), 0.125, 0.0) && agrees((0.12, 2), 0.125, 0.0));
        // The tolerance widens it, relative to the artifact's value.
        assert!(!agrees((105.0, 0), 100.0, 0.0));
        assert!(!agrees((105.0, 0), 100.0, 4.0));
        assert!(agrees((105.0, 0), 100.0, 5.0));
        assert!(agrees((-3.2, 1), -3.24, 0.0) && !agrees((3.2, 1), -3.2, 0.0));
    }

    #[test]
    fn artifact_numbers_are_every_numeric_leaf_or_cell_and_bad_content_is_an_error() {
        let json = r#"{"a": 1.5, "b": [2, {"c": -3e2}], "s": "4.5", "t": true, "n": null}"#;
        assert_eq!(
            artifact_numbers("r.json", json.as_bytes()).unwrap(),
            [1.5, 2.0, -300.0]
        );
        let jsonl = "{\"v\": 1}\n\n{\"v\": 2.5}\n";
        assert_eq!(
            artifact_numbers("r.jsonl", jsonl.as_bytes()).unwrap(),
            [1.0, 2.5]
        );
        let csv = "arm,ns,note\nget,12.4,\"a, b\"\nput,\"1e3\",inf\nx,nan,-0.5\n";
        assert_eq!(
            artifact_numbers("r.csv", csv.as_bytes()).unwrap(),
            [12.4, 1000.0, -0.5]
        );
        assert!(artifact_numbers("r.json", b"{\"a\": ").is_err());
        assert!(artifact_numbers("r.jsonl", b"{\"a\": 1}\nnot json\n").is_err());
        assert!(artifact_numbers("r.csv", b"a,\"open\n1,2\n").is_err());
        assert!(artifact_numbers("r.json", &[0xff, 0xfe]).is_err());
    }

    #[test]
    fn a_citation_is_a_path_to_a_data_file_and_a_url_is_not() {
        let paths = |t: &str| {
            artifact_citations(t)
                .into_iter()
                .map(|(_, p)| p)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths("see `results/a.json`, [raw](./results/b.jsonl) and results/c.CSV."),
            ["results/a.json", "results/b.jsonl", "results/c.CSV"]
        );
        assert!(paths("https://example.com/x/a.json and *.json and notes.md").is_empty());
        let (range, _) = &artifact_citations("in results/a.json.")[0];
        assert_eq!(&"in results/a.json."[range.clone()], "results/a.json");
    }

    fn figures_scan(text: &str, ev: &Fake, tolerance: f64) -> DocScan {
        scan_document(
            text,
            "d.md",
            None,
            &DocPolicy {
                verify_cited_figures: true,
                figure_tolerance_pct: tolerance,
                ..DocPolicy::default()
            },
            ev,
            &mut Artifacts::default(),
        )
        .unwrap()
    }

    fn with_artifact() -> Fake {
        Fake {
            files: vec![
                (
                    "results/get.json",
                    r#"{"median_ns": 12.3849, "ratio": 2.9}"#,
                ),
                ("results/put.csv", "arm,ns\nput,40.5\n"),
                ("results/bad.json", "{"),
            ],
            ..repo()
        }
    }

    #[test]
    fn a_tagged_figure_must_be_a_value_of_the_artifact_the_paragraph_cites() {
        let ev = with_artifact();
        // Control: both figures are stored (one rounded as the document shows it).
        let good = "Lookup takes 12.38 ns, 2.9x the old one (measured: box-1, 1111111a; results/get.json).\n";
        let scan = figures_scan(good, &ev, 0.0);
        assert!(scan.findings.is_empty(), "{:?}", scan.findings);
        assert_eq!(scan.paragraphs_compared, 1);

        let stale = "Lookup takes 11.9 ns (measured: box-1, 1111111a; results/get.json).\n";
        let scan = figures_scan(stale, &ev, 0.0);
        assert_eq!(codes(&scan), ["figure-disagrees-with-artifact"]);
        assert!(scan.findings[0].message.contains("`11.9 ns`"));
        assert!(scan.findings[0].message.contains("`results/get.json`"));
        // Within the tolerance it agrees.
        assert!(figures_scan(stale, &ev, 5.0).findings.is_empty());

        // Two artifacts cited: a figure may be in either.
        let two = "Get 12.38 ns, put 40.5 ns (measured: box-1, 1111111a) from results/get.json and results/put.csv.\n";
        assert!(figures_scan(two, &ev, 0.0).findings.is_empty());
        // Control: with one of them the other's figure is reported.
        let one = "Get 12.38 ns, put 40.5 ns (measured: box-1, 1111111a) from results/get.json.\n";
        assert_eq!(figures_scan(one, &ev, 0.0).findings.len(), 1);
    }

    #[test]
    fn numbers_inside_the_tag_or_a_path_are_not_figures() {
        let ev = Fake {
            files: vec![("results/2x/a.json", r#"{"v": 5.0}"#)],
            ..repo()
        };
        let text = "Takes 5 ms (measured: M1 16 GB, 1111111a; results/2x/a.json).\n";
        let scan = figures_scan(text, &ev, 0.0);
        assert!(scan.findings.is_empty(), "{:?}", scan.findings);
        assert_eq!(scan.paragraphs_compared, 1);
    }

    #[test]
    fn an_untracked_citation_is_reported_and_no_citation_is_counted() {
        let ev = with_artifact();
        let untracked = "Lookup takes 12.38 ns (measured: box-1, 1111111a; results/gone.json).\n";
        let scan = figures_scan(untracked, &ev, 0.0);
        assert_eq!(codes(&scan), ["figure-disagrees-with-artifact"]);
        assert!(scan.findings[0].message.contains("not a tracked file"));
        assert_eq!(scan.paragraphs_compared, 0);

        let uncited = "Lookup takes 12.38 ns (measured: box-1, 1111111a).\n";
        let scan = figures_scan(uncited, &ev, 0.0);
        assert!(scan.findings.is_empty());
        assert_eq!((scan.uncited, scan.paragraphs_compared), (1, 0));
        // A paragraph with no measured tag is not compared, whatever it cites.
        let untagged = "Lookup takes 11.9 ns (target), see results/get.json.\n";
        let scan = figures_scan(untagged, &ev, 0.0);
        assert!(scan.findings.is_empty());
        assert_eq!((scan.uncited, scan.paragraphs_compared), (0, 0));
        // An artifact that does not parse stops the scan.
        let bad = "Lookup takes 12.38 ns (measured: box-1, 1111111a; results/bad.json).\n";
        let err = scan_document(
            bad,
            "d.md",
            None,
            &DocPolicy {
                verify_cited_figures: true,
                ..DocPolicy::default()
            },
            &ev,
            &mut Artifacts::default(),
        );
        assert!(err.is_err_and(|e| e.to_string().contains("results/bad.json")));
    }

    #[test]
    fn a_caption_tag_covers_the_table_under_it() {
        let ev = with_artifact();
        let table = |ns: &str| {
            format!("*(measured: box-1, 1111111a; results/get.json)*\n\n| arm | time |\n|---|---|\n| get | {ns} ns |\n")
        };
        assert!(figures_scan(&table("12.38"), &ev, 0.0).findings.is_empty());
        let wrong = figures_scan(&table("14.00"), &ev, 0.0);
        assert_eq!(codes(&wrong), ["figure-disagrees-with-artifact"]);
        assert_eq!(wrong.findings[0].line, 5);
        // Attached to the caption, the table is the same paragraph.
        let attached = "*(measured: box-1, 1111111a; results/get.json)*\n| arm | time |\n|---|---|\n| get | 14.00 ns |\n";
        assert_eq!(figures_scan(attached, &ev, 0.0).findings.len(), 1);
        // Prose after the tagged paragraph is not part of it.
        let prose = "*(measured: box-1, 1111111a; results/get.json)* 12.38 ns.\n\nElsewhere it takes 99 ms.\n";
        assert!(figures_scan(prose, &ev, 0.0).findings.is_empty());
    }

    #[test]
    fn figures_follow_diff_only_when_asked() {
        let ev = with_artifact();
        let text = "Lookup takes 11.9 ns (measured: box-1, 1111111a; results/get.json).\n";
        let run = |added: &[usize], only: bool| {
            scan_document(
                text,
                "d.md",
                Some(&added.iter().copied().collect()),
                &DocPolicy {
                    verify_cited_figures: true,
                    figures_on_added_lines_only: only,
                    ..DocPolicy::default()
                },
                &ev,
                &mut Artifacts::default(),
            )
            .unwrap()
            .findings
            .len()
        };
        assert_eq!(run(&[], false), 1);
        assert_eq!(run(&[], true), 0);
        assert_eq!(run(&[1], true), 1);
    }

    fn records(
        path: &str,
        head: &str,
        base: Option<&str>,
        added: &[usize],
        ev: &Fake,
    ) -> RecordScan {
        scan_records(
            path,
            head,
            base,
            &added.iter().copied().collect(),
            "commit",
            ev,
        )
        .unwrap()
    }

    #[test]
    fn a_jsonl_record_on_an_added_line_must_name_a_full_commit_id() {
        let ev = repo();
        let head = format!(
            "{{\"commit\": \"{C1}\", \"ns\": 1}}\n{{\"commit\": \"unknown\"}}\n{{\"ns\": 3}}\n{{\"commit\": \"1111111a\"}}\n{{\"commit\": \"{TREE}\"}}\n{{\"commit\": 7}}\n[1]\n{{\"commit\": \"{}\"}}\n",
            "9".repeat(40)
        );
        let all = records("r.jsonl", &head, None, &[1, 2, 3, 4, 5, 6, 7, 8], &ev);
        assert_eq!(all.judged, 8);
        let lines: Vec<_> = all.findings.iter().map(|f| f.line.unwrap()).collect();
        assert_eq!(lines, [2, 3, 4, 5, 6, 7, 8]);
        let message = |line: usize| {
            &all.findings
                .iter()
                .find(|f| f.line == Some(line))
                .unwrap()
                .message
        };
        assert!(message(2).contains("not a full object id") && !message(2).contains("unknown"));
        assert!(message(3).contains("is missing"));
        // An abbreviation is not accepted in a record.
        assert!(message(4).contains("not a full object id"));
        assert!(message(5).contains("not a commit"));
        assert!(message(6).contains("not a string"));
        assert!(message(7).contains("not an object"));
        assert!(message(8).contains("names no object") && message(8).contains(&"9".repeat(40)));
        // Only added lines are judged; every line must still parse.
        assert!(records("r.jsonl", &head, None, &[1], &ev)
            .findings
            .is_empty());
        assert_eq!(records("r.jsonl", &head, None, &[2], &ev).findings.len(), 1);
        assert!(scan_records(
            "r.jsonl",
            "{\"commit\": 1}\nnope\n",
            None,
            &BTreeSet::new(),
            "commit",
            &ev
        )
        .is_err());
    }

    #[test]
    fn a_json_file_is_one_record_or_an_array_and_only_new_records_are_judged() {
        let ev = repo();
        let bad = r#"{"commit": "unknown", "ns": 1}"#;
        let one = records("r.json", bad, None, &[], &ev);
        assert_eq!(one.findings.len(), 1);
        assert_eq!(one.findings[0].anchor.as_deref(), Some("commit"));
        // Unchanged since the base (reformatted only): not judged.
        assert_eq!(
            records(
                "r.json",
                bad,
                Some("{ \"ns\": 1, \"commit\": \"unknown\" }"),
                &[],
                &ev
            )
            .judged,
            0
        );
        assert!(records(
            "r.json",
            &format!(r#"{{"commit": "{C1}"}}"#),
            None,
            &[],
            &ev
        )
        .findings
        .is_empty());

        let base = r#"[{"commit": "old", "ns": 1}]"#;
        let head =
            format!(r#"[{{"commit": "old", "ns": 1}}, {{"commit": "{C1}"}}, {{"commit": ""}}]"#);
        let array = records("r.json", &head, Some(base), &[], &ev);
        assert_eq!(array.judged, 2);
        assert_eq!(array.findings.len(), 1);
        assert_eq!(array.findings[0].anchor.as_deref(), Some("[2].commit"));
        // With no base every record is new.
        assert_eq!(records("r.json", &head, None, &[], &ev).findings.len(), 2);
        assert!(scan_records("r.json", "[", None, &BTreeSet::new(), "commit", &ev).is_err());
        // Another key.
        let keyed = scan_records(
            "r.json",
            &format!(r#"{{"rev": "{C1}"}}"#),
            None,
            &BTreeSet::new(),
            "rev",
            &ev,
        )
        .unwrap();
        assert!(keyed.findings.is_empty() && keyed.judged == 1);
    }

    #[test]
    fn a_ratio_run_file_is_read_by_its_provenance_commit() {
        let ev = repo();
        let run = |commit: &str| {
            format!(
                r#"{{"schema": "discipline-bench-ratio/v1", "commit": "{C1}", "provenance": {{"commit": "{commit}"}}}}"#
            )
        };
        assert!(records("run.json", &run(C1), None, &[], &ev)
            .findings
            .is_empty());
        let bad = records("run.json", &run("abc123"), None, &[], &ev);
        assert_eq!(bad.findings.len(), 1);
        assert_eq!(bad.findings[0].anchor.as_deref(), Some("provenance.commit"));
        assert!(bad.findings[0].message.contains("`provenance.commit`"));
    }

    #[test]
    fn a_record_id_absent_from_a_shallow_clone_cannot_be_checked() {
        let shallow = Fake {
            shallow: true,
            ..repo()
        };
        let head = format!(r#"{{"commit": "{}"}}"#, "9".repeat(40));
        let scan = records("r.json", &head, None, &[], &shallow);
        assert!(scan.findings.is_empty());
        assert_eq!(scan.cannot_check.len(), 1);
        assert_eq!(
            records("r.json", &head, None, &[], &repo()).findings.len(),
            1
        );
    }

    #[test]
    fn the_tolerance_is_a_finite_number_zero_or_above() {
        assert!(check_tolerance(0.0).is_ok() && check_tolerance(2.5).is_ok());
        assert!(check_tolerance(-0.1).is_err());
        assert!(check_tolerance(f64::NAN).is_err() && check_tolerance(f64::INFINITY).is_err());
    }
}
