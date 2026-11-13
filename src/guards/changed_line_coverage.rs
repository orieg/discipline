//! Changed-line coverage (`changed-line-coverage`): a line the change adds that the
//! project's coverage report lists as executable and never run (#1068).
//!
//! The report is the project's own (LCOV or Cobertura), written by its test run before
//! discipline runs; discipline runs no test. The rule is the report's own fact, with no
//! threshold: a changed line the report lists with zero hits is reported; a changed line
//! a listed file does not list is not executable to the tool, and is not.
//!
//! What is judged:
//! - The changed source files: added or modified, with at least one added line, in a
//!   language discipline reads, outside the test paths and `exempt_paths`.
//! - Each one the report has an entry for. One it has none for is named in a note; when it
//!   has none for any of them, the run stops (exit 2), since nothing was checked.
//!
//! The report is not trusted when the change could have written it: a report that is
//! tracked (on the base side, in the index, or in the change), a symlink, or outside the
//! repository stops the run (exit 2), as does one that is missing, unreadable or
//! malformed.

use crate::config::{CoverageFormat, GateSettings};
use crate::could_not_check::{tag, Reason};
use crate::escape::unescape_xml;
use crate::guards::{exempt_filter, Context, GateOutcome};
use crate::tokens;
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

pub const GATE: &str = "changed-line-coverage";

/// The largest report read: a report past it is refused, not cut.
pub const MAX_REPORT_BYTES: u64 = 256 * 1024 * 1024;

/// Hits by line, by the path the report names.
pub type Hits = BTreeMap<String, BTreeMap<usize, u64>>;

/// Records `hits` for `line` of `path`; a line listed twice is covered when either says so.
fn add(out: &mut Hits, path: &str, line: usize, hits: u64) {
    let at = out
        .entry(path.to_string())
        .or_default()
        .entry(line)
        .or_insert(0);
    *at = (*at).max(hits);
}

/// An LCOV tracefile: `SF:<path>` opens a record, `DA:<line>,<hits>[,<checksum>]` lists
/// a line, `end_of_record` closes it. Other records (functions, branches) are not read.
/// A `DA` outside a record, a malformed `DA`, or a file with no record is refused.
pub fn parse_lcov(text: &str) -> Result<Hits> {
    let mut out = Hits::new();
    let mut file: Option<String> = None;
    let mut records = 0usize;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if let Some(path) = line.strip_prefix("SF:") {
            file = Some(path.trim().to_string());
            out.entry(path.trim().to_string()).or_default();
            records += 1;
        } else if let Some(da) = line.strip_prefix("DA:") {
            let Some(path) = file.as_deref() else {
                bail!("LCOV line {}: `DA` outside an `SF` record", n + 1);
            };
            let mut parts = da.split(',');
            let (Some(at), Some(hits)) = (
                parts.next().and_then(|s| s.trim().parse::<usize>().ok()),
                parts.next().and_then(|s| s.trim().parse::<u64>().ok()),
            ) else {
                bail!("LCOV line {}: malformed `DA:{da}`", n + 1);
            };
            add(&mut out, path, at, hits);
        } else if line == "end_of_record" {
            file = None;
        }
    }
    if records == 0 {
        bail!("no `SF:` record: not an LCOV tracefile");
    }
    Ok(out)
}

/// A Cobertura report: each `<class filename="..">` with its `<line number=".."
/// hits="..">` children. A filename is relative to one of the `<source>` roots; each
/// root it can be joined to is recorded, the bare filename too. A report with no
/// `<coverage` element or no class is refused.
pub fn parse_cobertura(xml: &str) -> Result<(Hits, Vec<String>)> {
    if !xml.contains("<coverage") {
        bail!("no `<coverage>` element: not a Cobertura report");
    }
    let comment = Regex::new(r"(?s)<!--.*?-->")?;
    let xml = comment.replace_all(xml, "");
    let source = Regex::new(r"(?s)<source>(.*?)</source>")?;
    let class = Regex::new(r"(?s)<class\b([^>]*?)(?:/>|>(.*?)</class>)")?;
    let line = Regex::new(r"<line\b([^>]*?)/?>")?;
    let attr = Regex::new(r#"([a-zA-Z_:-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)?;
    let attrs = |s: &str| -> BTreeMap<String, String> {
        attr.captures_iter(s)
            .map(|c| {
                let v = c.get(2).or_else(|| c.get(3)).map_or("", |m| m.as_str());
                (c[1].to_string(), unescape_xml(v))
            })
            .collect()
    };
    let roots: Vec<String> = source
        .captures_iter(&xml)
        .map(|c| unescape_xml(c[1].trim()))
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = Hits::new();
    let mut classes = 0usize;
    for c in class.captures_iter(&xml) {
        let a = attrs(&c[1]);
        let Some(filename) = a.get("filename") else {
            bail!("a `<class>` without `filename`");
        };
        classes += 1;
        out.entry(filename.clone()).or_default();
        for l in line.captures_iter(c.get(2).map_or("", |m| m.as_str())) {
            let la = attrs(&l[1]);
            let (Some(at), Some(hits)) = (
                la.get("number").and_then(|s| s.parse::<usize>().ok()),
                la.get("hits").and_then(|s| s.parse::<u64>().ok()),
            ) else {
                bail!("`{filename}`: a `<line>` without a numeric `number` and `hits`");
            };
            add(&mut out, filename, at, hits);
        }
    }
    if classes == 0 {
        bail!("no `<class>` element: the report lists no file");
    }
    Ok((out, roots))
}

/// The report's entries under repository-relative paths: an absolute path under the
/// repository root loses the root, a leading `./` is dropped, and a Cobertura filename is
/// also read under each `<source>` root, itself made repository-relative.
pub fn by_repo_path(hits: Hits, roots: &[String], root: &str) -> Hits {
    let root = root.trim_end_matches('/');
    let rel = |p: &str| -> Option<String> {
        let p = p.replace('\\', "/");
        let p = if p.starts_with('/') {
            p.strip_prefix(root)?.strip_prefix('/')?.to_string()
        } else {
            p.trim_start_matches("./").to_string()
        };
        Some(p)
    };
    let mut out = Hits::new();
    for (path, lines) in hits {
        let mut names: BTreeSet<String> = rel(&path).into_iter().collect();
        for r in roots {
            if let Some(r) = rel(r) {
                let joined = if r.is_empty() {
                    path.clone()
                } else {
                    format!("{r}/{path}")
                };
                names.extend(rel(&joined));
            }
        }
        for name in names {
            let entry = out.entry(name).or_default();
            for (line, h) in &lines {
                let at = entry.entry(*line).or_insert(0);
                *at = (*at).max(*h);
            }
        }
    }
    out
}

/// The changed lines `lines` the report lists with zero hits, as `start-end` runs.
pub fn uncovered(added: &BTreeSet<usize>, lines: &BTreeMap<usize, u64>) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for line in added.iter().copied().filter(|l| lines.get(l) == Some(&0)) {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == line => *end = line,
            _ => runs.push((line, line)),
        }
    }
    runs
}

fn config_err(msg: String) -> anyhow::Error {
    tag(Reason::Configuration, anyhow!(msg))
}

/// The report's text, once the change could not have written it.
fn read_report(ctx: &Context, report: &str) -> Result<String> {
    let key = format!("gates.{GATE}.report");
    let path = std::path::Path::new(report);
    if report.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(config_err(format!(
            "`{key}` = `{report}`: a repository-relative path without `..` is required"
        )));
    }
    let tracked = ctx.git.is_tracked(report)?
        || ctx.git.base_bytes(report)?.is_some()
        || ctx.git.changed_files()?.iter().any(|f| f.path == report);
    if tracked {
        bail!(
            "the coverage report `{report}` is tracked by git, so the change could have written it; the report must be one the test run writes"
        );
    }
    let full = ctx.git.root().join(path);
    let meta = std::fs::symlink_metadata(&full)
        .map_err(|e| anyhow!("cannot read the coverage report `{report}`: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        bail!("the coverage report `{report}` is not a regular file");
    }
    if meta.len() > MAX_REPORT_BYTES {
        bail!(
            "the coverage report `{report}` is {} bytes, past the {MAX_REPORT_BYTES}-byte limit; it is not cut",
            meta.len()
        );
    }
    std::fs::read_to_string(&full)
        .map_err(|e| anyhow!("cannot read the coverage report `{report}`: {e}"))
}

/// Whether `path` is a test file: by its language's convention or the declared test paths.
fn is_test(ctx: &Context, path: &str) -> bool {
    crate::ast::default_registry()
        .find_pack(path)
        .is_some_and(|p| p.is_test_path(path))
        || crate::ast::functions::declared_test_path(path, &ctx.config.tests.paths)
}

pub fn evaluate_changed_line_coverage(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.changed_line_coverage;
    let mut out = GateOutcome::new(GATE);
    let Some(report) = settings.report.as_deref() else {
        return Err(config_err(format!(
            "`gates.{GATE}` is enabled but `report` is not set"
        )));
    };
    let text = read_report(ctx, report)?;
    let root = ctx.git.root().to_string_lossy().to_string();
    let hits = match settings.format {
        CoverageFormat::Lcov => by_repo_path(parse_lcov(&text)?, &[], &root),
        CoverageFormat::Cobertura => {
            let (hits, roots) = parse_cobertura(&text)?;
            by_repo_path(hits, &roots, &root)
        }
    }
    .into_iter()
    .collect::<Hits>();

    let exempt = exempt_filter(settings)?;
    let mut candidates: Vec<crate::gitctx::ChangedFile> = ctx
        .git
        .changed_files()?
        .into_iter()
        .filter(|f| {
            !f.is_deleted()
                && !f.added_lines.is_empty()
                && crate::ast::language_for(&f.path).is_some()
                && !is_test(ctx, &f.path)
                && !exempt.matches(&f.path)
        })
        .collect();
    candidates.sort_by(|a, b| a.path.cmp(&b.path));
    if candidates.is_empty() {
        out.notes.push(
            "the change adds no line to a source file outside the test paths and `exempt_paths`; nothing to check against the report"
                .into(),
        );
        return Ok(out);
    }
    let (listed, missing): (Vec<_>, Vec<_>) =
        candidates.iter().partition(|f| hits.contains_key(&f.path));
    if listed.is_empty() {
        bail!(
            "the coverage report `{report}` has no entry for any changed source file ({}); its paths may be relative to another directory",
            missing
                .iter()
                .map(|f| format!("`{}`", f.path))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !missing.is_empty() {
        out.notes.push(format!(
            "not in the coverage report, so not checked: {}",
            missing
                .iter()
                .map(|f| format!("`{}`", f.path))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for f in listed {
        out.examined += 1;
        let runs = uncovered(&f.added_lines, &hits[&f.path]);
        let Some(&(first, _)) = runs.first() else {
            continue;
        };
        let ranges = runs
            .iter()
            .map(|(s, e)| format!("{s}-{e}"))
            .collect::<Vec<_>>()
            .join(",");
        let lifted = ctx.find_override(
            GATE,
            &crate::findings::UNCOVERED_LINES,
            tokens::ALLOW_UNCOVERED,
            &f.path,
        );
        out.lift_or_push(
            lifted,
            ctx.overridable(settings.severity()),
            &crate::findings::UNCOVERED_LINES,
            (Some(&f.path), Some(first)),
            format!(
                "Lines this change adds to `{}` that the coverage report `{report}` lists as executable and never run: {ranges}.",
                f.path
            ),
            &format!(
                "Add a test that runs them, or justify it on its own line in the PR body or a commit message: `allow-uncovered: {} <reason>`.",
                f.path
            ),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcov_records_merge_and_a_malformed_file_is_refused() {
        let hits = parse_lcov(
            "TN:\nSF:/r/src/a.rs\nDA:1,3\nDA:2,0\nDA:3,0,abc\nend_of_record\nSF:/r/src/a.rs\nDA:2,1\nend_of_record\n",
        )
        .unwrap();
        assert_eq!(
            hits["/r/src/a.rs"],
            BTreeMap::from([(1, 3), (2, 1), (3, 0)])
        );
        assert!(parse_lcov("TN:\n").is_err());
        assert!(parse_lcov("DA:1,1\n").is_err());
        assert!(parse_lcov("SF:a\nDA:x,1\n").is_err());
    }

    #[test]
    fn cobertura_classes_are_read_under_their_sources() {
        let xml = r#"<?xml version="1.0"?>
<coverage line-rate="0.5">
  <sources><source>/r/src</source></sources>
  <packages><package name="p"><classes>
    <class name="a" filename="a.py"><lines>
      <line number="1" hits="2"/><line number="2" hits="0"/>
    </lines></class>
    <!-- <class filename="hidden.py"><lines><line number="1" hits="0"/></lines></class> -->
  </classes></package></packages>
</coverage>"#;
        let (hits, roots) = parse_cobertura(xml).unwrap();
        assert_eq!(roots, ["/r/src"]);
        assert_eq!(hits["a.py"], BTreeMap::from([(1, 2), (2, 0)]));
        assert!(!hits.contains_key("hidden.py"));
        let rel = by_repo_path(hits, &roots, "/r");
        assert_eq!(rel["src/a.py"], BTreeMap::from([(1, 2), (2, 0)]));
        assert!(parse_cobertura("<report/>").is_err());
        assert!(parse_cobertura("<coverage></coverage>").is_err());
        assert!(parse_cobertura(
            r#"<coverage><class filename="a.py"><lines><line number="1"/></lines></class></coverage>"#
        )
        .is_err());
    }

    #[test]
    fn absolute_report_paths_are_made_repository_relative() {
        let hits = Hits::from([
            ("/r/src/a.rs".to_string(), BTreeMap::from([(1, 0)])),
            ("./src/b.rs".to_string(), BTreeMap::from([(1, 1)])),
            ("/elsewhere/c.rs".to_string(), BTreeMap::from([(1, 0)])),
        ]);
        let rel = by_repo_path(hits, &[], "/r/");
        assert_eq!(rel.keys().collect::<Vec<_>>(), ["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn only_listed_changed_lines_with_zero_hits_are_uncovered() {
        let added = BTreeSet::from([2, 3, 4, 7, 9]);
        // 2-3 never run, 4 run, 7 not listed (not executable), 9 never run.
        let lines = BTreeMap::from([(1, 1), (2, 0), (3, 0), (4, 5), (9, 0)]);
        assert_eq!(uncovered(&added, &lines), [(2, 3), (9, 9)]);
        // Control: every listed changed line run.
        assert_eq!(
            uncovered(&added, &BTreeMap::from([(2, 1), (3, 1), (9, 1)])),
            []
        );
    }
}
