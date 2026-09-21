//! Retracted-figure registry and pending-measurement citations (`provenance-tags`).
//!
//! Two claim checks that need data outside the line being read:
//!
//! - **Superseded figures.** A repository keeps a registry of figures it has withdrawn
//!   (`superseded_registry`, a JSON file at `HEAD`). A registered figure may be published
//!   again only next to a retraction marker (`retracted`, `superseded`, `corrected`, ...)
//!   within three lines of it. A registry pattern matches only when at least two of the
//!   figure's context words (one, when it declares one) appear in the same sentence or
//!   table cell, or in the surrounding window, so an unrelated `12.0x` does not fire.
//!   Tracked JSON datasets named by `superseded_json_paths` are swept value by value.
//! - **Pending measurements.** A statement that a measurement is pending (`pending re-run`,
//!   `pending re-measurement`, ...) must cite a tracking issue within the next 150
//!   characters. With `require_open_pending_issues`, at least one cited issue must be
//!   open: an issue closed while the text still says "pending" is a stale claim.
//!
//! Registry format (unknown fields are ignored, so a registry can carry its own notes):
//!
//! ```json
//! { "figures": [ { "id": "...", "patterns": ["regex", ...], "context": ["word", ...],
//!                  "array_sequence": ["1.5", "2.0"], "replacement": "..." } ] }
//! ```
//!
//! Patterns are compiled case-insensitively with look-around support. A registry that is
//! missing at `HEAD`, is not JSON, or holds a pattern that does not compile is a
//! could-not-check (exit 2), never an empty registry.
//!
//! Issue state comes from the forge's REST API over HTTPS (`crate::forge`, AGENTS.md
//! §3.3). An issue whose state the
//! instruments cannot decide is reported by name as could-not-check.

use anyhow::{bail, Context as _, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Lines on each side of a registered figure searched for a retraction marker.
pub const RETRACTION_WINDOW: usize = 3;

/// Backtracking steps one registry pattern may take on one sentence before the search is
/// abandoned as could-not-check. Registry patterns are repository data, so they are bounded.
pub const BACKTRACK_LIMIT: usize = 100_000;

/// Characters after a pending statement searched for its issue citation.
pub const PENDING_CITATION_SPAN: usize = 150;

/// JSON object keys whose values record a retraction rather than publish a figure.
const JSON_SKIP_KEYS: &[&str] = &[
    "provenance",
    "removed_for_lack_of_provenance",
    "retraction",
    "meta",
    "description",
    "_comment",
];

static RETRACTION_MARKERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:retract(?:ed|ion|ing)?|withdraw(?:n)?|supersed(?:ed|es|ing)?|correct(?:ed|ion)?|previously|refut(?:ed|es|ing)?|stale|anti-example|strawman|earlier|was measured with|both were|unmeasured|unverified|definitional|indicative)\b",
    )
    .unwrap()
});

static PENDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:pending\s+(?:(?:a\s+)?(?:tagged\s+)?(?:reference-host|quiet-host|fair-baseline|clean-host)\s+)?(?:re-run|re-measurement|run)|unverified\s+until\s+the\s+next\s+nightly\s+baseline\s+run)\b",
    )
    .unwrap()
});

static ISSUE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)https?://([A-Za-z0-9.:-]+)/([A-Za-z0-9_./-]+?)/(?:-/)?(issues|pull|pulls|merge_requests|work_items)/(\d+)|(?:#|issues/)(\d+)",
    )
    .unwrap()
});

/// One withdrawn figure.
#[derive(Debug)]
pub struct SupersededFigure {
    pub id: String,
    pub patterns: Vec<fancy_regex::Regex>,
    /// Lower-cased context words.
    pub context: Vec<String>,
    /// A retracted run of array values, compared as strings.
    pub array_sequence: Option<Vec<String>>,
    pub replacement: String,
}

impl SupersededFigure {
    /// Context words that must co-occur with a pattern match.
    fn required_context(&self) -> usize {
        self.context.len().min(2)
    }

    fn context_hits(&self, texts: &[&str]) -> usize {
        self.context
            .iter()
            .filter(|c| texts.iter().any(|t| t.contains(c.as_str())))
            .count()
    }
}

/// Parse a superseded-figure registry. `path` names it in errors.
pub fn load_registry(json: &str, path: &str) -> Result<Vec<SupersededFigure>> {
    let root: serde_json::Value = serde_json::from_str(json)
        .with_context(|| format!("superseded registry `{path}` is not valid JSON"))?;
    let Some(figures) = root.get("figures").and_then(|f| f.as_array()) else {
        bail!("superseded registry `{path}` has no `figures` array");
    };
    let mut out = Vec::with_capacity(figures.len());
    for (i, fig) in figures.iter().enumerate() {
        let Some(id) = fig.get("id").and_then(|v| v.as_str()) else {
            bail!("superseded registry `{path}`: figure {i} has no string `id`");
        };
        let strings = |key: &str| -> Result<Vec<String>> {
            match fig.get(key) {
                None => Ok(Vec::new()),
                Some(serde_json::Value::Array(items)) => items
                    .iter()
                    .map(|v| match v {
                        serde_json::Value::String(s) => Ok(s.clone()),
                        serde_json::Value::Number(n) => Ok(n.to_string()),
                        _ => bail!(
                            "superseded registry `{path}`: `{id}.{key}` holds a non-string entry"
                        ),
                    })
                    .collect(),
                Some(_) => bail!("superseded registry `{path}`: `{id}.{key}` is not an array"),
            }
        };
        let mut patterns = Vec::new();
        for (j, p) in strings("patterns")?.iter().enumerate() {
            let re = fancy_regex::RegexBuilder::new(&format!("(?i){p}"))
                .backtrack_limit(BACKTRACK_LIMIT)
                .build()
                .with_context(|| {
                    format!("superseded registry `{path}`: `{id}.patterns[{j}]` does not compile")
                })?;
            patterns.push(re);
        }
        let array_sequence = fig
            .get("array_sequence")
            .map(|_| strings("array_sequence"))
            .transpose()?
            .filter(|s| !s.is_empty());
        if patterns.is_empty() && array_sequence.is_none() {
            bail!(
                "superseded registry `{path}`: `{id}` has neither `patterns` nor `array_sequence`"
            );
        }
        out.push(SupersededFigure {
            id: id.to_string(),
            patterns,
            context: strings("context")?
                .into_iter()
                .map(|c| c.to_lowercase())
                .collect(),
            array_sequence,
            replacement: fig
                .get("replacement")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        });
    }
    Ok(out)
}

/// The registry a change is checked against: every figure of the base registry plus every
/// figure of the head registry. A change cannot withdraw the entry for a figure it
/// republishes; an entry removed on purpose stops applying once the change is merged.
pub fn merge_registries(
    base: Vec<SupersededFigure>,
    head: Vec<SupersededFigure>,
) -> Vec<SupersededFigure> {
    let key = |f: &SupersededFigure| {
        (
            f.id.clone(),
            f.patterns
                .iter()
                .map(|p| p.as_str().to_string())
                .collect::<Vec<_>>(),
            f.context.clone(),
            f.array_sequence.clone(),
        )
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for f in head.into_iter().chain(base) {
        if seen.insert(key(&f)) {
            out.push(f);
        }
    }
    out
}

/// A registered figure published without a retraction marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersededHit {
    pub line: usize,
    pub figure_id: String,
    pub matched: String,
    pub replacement: String,
}

/// Split a line into sentences and table cells: the unit a context word must share.
fn chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for cell in text.split('|') {
        let mut start = 0;
        let bytes = cell.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if matches!(bytes[i], b'.' | b'!' | b'?')
                && bytes.get(i + 1).is_some_and(|b| b.is_ascii_whitespace())
            {
                out.push(&cell[start..=i]);
                start = i + 1;
            }
            i += 1;
        }
        out.push(&cell[start..]);
    }
    out.into_iter()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect()
}

/// Scan fence-stripped markdown lines `(line number, text)` for registered figures.
pub fn scan_superseded(
    lines: &[(usize, String)],
    registry: &[SupersededFigure],
) -> Result<Vec<SupersededHit>> {
    let mut hits = Vec::new();
    if registry.is_empty() {
        return Ok(hits);
    }
    for (idx, (line, text)) in lines.iter().enumerate() {
        let lo = idx.saturating_sub(RETRACTION_WINDOW);
        let hi = (idx + RETRACTION_WINDOW + 1).min(lines.len());
        let window: String = lines[lo..hi]
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let retracted = RETRACTION_MARKERS.is_match(&window);
        let window_lower = window.to_lowercase();
        for chunk in chunks(text) {
            let chunk_lower = chunk.to_lowercase();
            for fig in registry {
                // Context is a cheap substring test and a hit needs it, so it gates the
                // (backtracking) pattern search.
                if fig.context_hits(&[&chunk_lower, &window_lower]) < fig.required_context() {
                    continue;
                }
                for pat in &fig.patterns {
                    let Some(m) = pat
                        .find(chunk)
                        .with_context(|| format!("superseded pattern for `{}` failed", fig.id))?
                    else {
                        continue;
                    };
                    let dup = hits
                        .iter()
                        .any(|h: &SupersededHit| h.line == *line && h.figure_id == fig.id);
                    if !retracted && !dup {
                        hits.push(SupersededHit {
                            line: *line,
                            figure_id: fig.id.clone(),
                            matched: m.as_str().trim().to_string(),
                            replacement: fig.replacement.clone(),
                        });
                    }
                    break;
                }
            }
        }
    }
    Ok(hits)
}

/// Sweep a JSON dataset for registered figures. Returns `(key path, message)` pairs.
pub fn scan_superseded_json(
    value: &serde_json::Value,
    registry: &[SupersededFigure],
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    walk_json(value, "root", registry, &mut out)?;
    Ok(out)
}

fn walk_json(
    value: &serde_json::Value,
    key_path: &str,
    registry: &[SupersededFigure],
    out: &mut Vec<(String, String)>,
) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if JSON_SKIP_KEYS.contains(&k.as_str()) || k.starts_with("retraction_") {
                    continue;
                }
                walk_json(v, &format!("{key_path}.{k}"), registry, out)?;
            }
        }
        serde_json::Value::Array(items) => {
            let as_strings: Vec<String> = items
                .iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            for fig in registry {
                let Some(seq) = &fig.array_sequence else {
                    continue;
                };
                if as_strings.windows(seq.len()).any(|w| w == seq.as_slice()) {
                    out.push((
                        key_path.to_string(),
                        format!("retracted sequence of `{}` ({})", fig.id, seq.join(", ")),
                    ));
                }
            }
            for (i, v) in items.iter().enumerate() {
                walk_json(v, &format!("{key_path}[{i}]"), registry, out)?;
            }
        }
        serde_json::Value::String(s) => {
            let lower = s.to_lowercase();
            let key_lower = key_path.to_lowercase();
            for fig in registry {
                if fig.context_hits(&[&lower, &key_lower]) < fig.required_context() {
                    continue;
                }
                for pat in &fig.patterns {
                    let Some(m) = pat
                        .find(s)
                        .with_context(|| format!("superseded pattern for `{}` failed", fig.id))?
                    else {
                        continue;
                    };
                    out.push((
                        key_path.to_string(),
                        format!("`{}` is superseded figure `{}`", m.as_str().trim(), fig.id),
                    ));
                    break;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Why a pending statement fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingProblem {
    /// No issue is cited within [`PENDING_CITATION_SPAN`] characters.
    NoCitation,
    /// Every cited issue is closed.
    Closed(Vec<String>),
    /// The only citations name another repository not in `pending_issue_repos`.
    OutsideRepository(Vec<String>),
}

/// A pending statement whose cited issues' state could not be decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUndecidable {
    pub line: usize,
    pub reasons: Vec<String>,
}

/// A cited issue: an explicit `host`/`repo` from a URL, or a bare `#n` in this repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub host: Option<String>,
    pub repo: Option<String>,
    pub number: String,
    /// A GitLab merge request (`/-/merge_requests/n`) rather than an issue.
    pub merge_request: bool,
}

fn issue_refs(text: &str) -> Vec<IssueRef> {
    ISSUE_REF
        .captures_iter(text)
        .map(|c| match (c.get(1), c.get(2), c.get(4)) {
            (Some(host), Some(repo), Some(n)) => IssueRef {
                host: Some(host.as_str().to_ascii_lowercase()),
                repo: Some(repo.as_str().to_string()),
                number: n.as_str().to_string(),
                merge_request: c.get(3).is_some_and(|k| k.as_str() == "merge_requests"),
            },
            _ => IssueRef {
                host: None,
                repo: None,
                number: c.get(5).map(|n| n.as_str()).unwrap_or_default().to_string(),
                merge_request: false,
            },
        })
        .collect()
}

/// Issue state lookups against the repository's forge, cached per reference.
pub struct IssueStates<'a> {
    api: &'a dyn crate::forge::ForgeApi,
    forge: std::result::Result<crate::forge::Forge, String>,
    /// Other repositories (`owner/name`) whose issues may be cited.
    allowed: Vec<String>,
    cache: BTreeMap<String, std::result::Result<bool, String>>,
}

impl<'a> IssueStates<'a> {
    /// `forge` is where bare `#123` references resolve; `Err` says why it is unknown.
    pub fn new(
        api: &'a dyn crate::forge::ForgeApi,
        forge: std::result::Result<crate::forge::Forge, String>,
    ) -> Self {
        Self {
            api,
            forge,
            allowed: Vec::new(),
            cache: BTreeMap::new(),
        }
    }

    /// Also accept issues of these repositories (`owner/name`). Anything else is refused:
    /// an open issue anywhere on the host must not satisfy a claim about this one.
    pub fn allow_repositories(mut self, repos: &[String]) -> Self {
        self.allowed = repos
            .iter()
            .map(|r| r.trim_matches('/').to_string())
            .collect();
        self
    }

    /// Whether `r` names a repository other than this one that is not allowed.
    fn outside(&self, r: &IssueRef) -> bool {
        match (&self.forge, &r.repo) {
            (Ok(f), Some(repo)) => {
                !repo.eq_ignore_ascii_case(&f.repo)
                    && !self.allowed.iter().any(|a| a.eq_ignore_ascii_case(repo))
            }
            _ => false,
        }
    }

    /// `Ok(true)` when open, `Ok(false)` when closed, `Err` when undecidable.
    fn is_open(&mut self, r: &IssueRef) -> std::result::Result<bool, String> {
        let forge = match &self.forge {
            Ok(f) => f.clone(),
            Err(e) => return Err(format!("#{}: {e}", r.number)),
        };
        if let Some(host) = &r.host {
            if *host != forge.host() {
                return Err(format!(
                    "#{} is on {host}, not on this repository's forge ({})",
                    r.number,
                    forge.host()
                ));
            }
        }
        let repo = r.repo.clone().unwrap_or_else(|| forge.repo.clone());
        let key = format!(
            "{}/{repo}#{}{}",
            forge.host(),
            r.number,
            if r.merge_request { "!" } else { "" }
        );
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let answer = if r.merge_request {
            let path = format!(
                "projects/{}/merge_requests/{}",
                crate::forge::gitlab_project_id(&repo),
                r.number
            );
            self.api.get(&forge, &path).and_then(|v| {
                match v
                    .as_ref()
                    .and_then(|v| v.get("state"))
                    .and_then(|s| s.as_str())
                {
                    Some("opened") => Ok(true),
                    Some(_) => Ok(false),
                    None => Err("merge request has no readable state".to_string()),
                }
            })
        } else {
            crate::forge::issue_is_open(self.api, &forge, &repo, &r.number)
        }
        .map_err(|e| format!("{key}: {e}"));
        self.cache.insert(key, answer.clone());
        answer
    }
}

/// Scan fence-stripped lines for pending statements. With `states`, cited issues must
/// include an open one; without, a citation is enough.
pub fn scan_pending(
    lines: &[(usize, String)],
    mut states: Option<&mut IssueStates<'_>>,
) -> (Vec<(usize, PendingProblem)>, Vec<PendingUndecidable>) {
    let mut problems = Vec::new();
    let mut undecidable = Vec::new();
    for (line, text) in lines {
        for m in PENDING.find_iter(text) {
            let after: String = text[m.end()..]
                .chars()
                .take(PENDING_CITATION_SPAN)
                .collect();
            let refs = issue_refs(&after);
            if refs.is_empty() {
                problems.push((*line, PendingProblem::NoCitation));
                break;
            }
            let Some(states) = states.as_deref_mut() else {
                continue;
            };
            let mut closed = Vec::new();
            let mut unknown = Vec::new();
            let mut outside = Vec::new();
            let mut open = false;
            for r in &refs {
                if states.outside(r) {
                    outside.push(format!(
                        "{}#{}",
                        r.repo.as_deref().unwrap_or_default(),
                        r.number
                    ));
                    continue;
                }
                match states.is_open(r) {
                    Ok(true) => {
                        open = true;
                        break;
                    }
                    Ok(false) => closed.push(format!("#{}", r.number)),
                    Err(e) => unknown.push(e),
                }
            }
            if open {
                continue;
            }
            if !unknown.is_empty() {
                undecidable.push(PendingUndecidable {
                    line: *line,
                    reasons: unknown,
                });
            } else if closed.is_empty() {
                problems.push((*line, PendingProblem::OutsideRepository(outside)));
            } else {
                problems.push((*line, PendingProblem::Closed(closed)));
            }
            break;
        }
    }
    (problems, undecidable)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &str = r#"{
      "_comment": ["notes are ignored"],
      "figures": [
        { "id": "old_deficit",
          "patterns": ["(?<![\\w.])1\\.11\\s*[x×](?!\\w)", "(?<![\\w.])11%\\s*slower"],
          "context": ["lookup", "stock", "deficit"],
          "replacement": "1.031x [1.024, 1.038]",
          "rationale": "ignored field" },
        { "id": "old_curve", "array_sequence": [1.0, 1.9, 12.0], "context": [] }
      ]
    }"#;

    fn lines(text: &str) -> Vec<(usize, String)> {
        text.lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_string()))
            .collect()
    }

    #[test]
    fn registry_loads_lookbehind_patterns_and_ignores_unknown_fields() {
        let reg = load_registry(REGISTRY, "reg.json").unwrap();
        assert_eq!(reg.len(), 2);
        assert_eq!(reg[0].patterns.len(), 2);
        assert_eq!(reg[0].context, vec!["lookup", "stock", "deficit"]);
        assert_eq!(
            reg[1].array_sequence.as_deref(),
            Some(&["1.0".to_string(), "1.9".to_string(), "12.0".to_string()][..])
        );
    }

    #[test]
    fn malformed_registries_are_errors_not_empty() {
        assert!(load_registry("not json", "r").is_err());
        assert!(load_registry(r#"{"items": []}"#, "r").is_err());
        assert!(load_registry(r#"{"figures": [{"patterns": ["x"]}]}"#, "r").is_err());
        assert!(load_registry(
            r#"{"figures": [{"id": "a", "patterns": ["(unclosed"]}]}"#,
            "r"
        )
        .is_err());
        assert!(load_registry(r#"{"figures": [{"id": "a"}]}"#, "r").is_err());
    }

    #[test]
    fn unretracted_figure_with_context_fires_and_retraction_marker_clears_it() {
        let reg = load_registry(REGISTRY, "r").unwrap();
        let bad = lines("Random lookup is 1.11x slower than stock.");
        let hits = scan_superseded(&bad, &reg).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].figure_id, "old_deficit");
        assert_eq!(hits[0].matched, "1.11x");
        assert_eq!(hits[0].line, 1);

        let retracted =
            lines("Random lookup was 1.11x slower than stock (retracted: loaded host).");
        assert!(scan_superseded(&retracted, &reg).unwrap().is_empty());

        // The marker may sit up to three lines away, not four.
        let near = lines("Retracted figures:\n\n\nRandom lookup 1.11x vs stock.");
        assert!(scan_superseded(&near, &reg).unwrap().is_empty());
        let far = lines("Retracted figures:\n\n\n\nRandom lookup 1.11x vs stock.");
        assert_eq!(scan_superseded(&far, &reg).unwrap().len(), 1);
    }

    #[test]
    fn figure_without_its_context_or_inside_a_longer_number_does_not_fire() {
        let reg = load_registry(REGISTRY, "r").unwrap();
        // One context word is not enough when the figure declares three.
        assert!(
            scan_superseded(&lines("Compression is 1.11x better on lookup."), &reg)
                .unwrap()
                .is_empty()
        );
        // Lookbehind: 21.11x is a different number.
        assert!(
            scan_superseded(&lines("Stock lookup is 21.11x slower."), &reg)
                .unwrap()
                .is_empty()
        );
        // Unicode multiplication sign and a table cell are both matched.
        assert_eq!(
            scan_superseded(&lines("| stock lookup | 1.11× |"), &reg)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn json_sweep_matches_strings_and_sequences_and_skips_retraction_keys() {
        let reg = load_registry(REGISTRY, "r").unwrap();
        let data: serde_json::Value = serde_json::json!({
            "lookup_vs_stock": { "label": "11% slower" },
            "series": [0.5, 1.0, 1.9, 12.0],
            "retraction": { "label": "stock lookup 11% slower" },
            "unrelated": "11% slower"
        });
        let hits = scan_superseded_json(&data, &reg).unwrap();
        let paths: Vec<&str> = hits.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"root.lookup_vs_stock.label"), "{hits:?}");
        assert!(paths.contains(&"root.series"), "{hits:?}");
        assert!(!paths.iter().any(|p| p.contains("retraction")), "{hits:?}");
        assert!(!paths.contains(&"root.unrelated"), "{hits:?}");
    }

    #[test]
    fn pending_statement_needs_a_citation() {
        let (p, u) = scan_pending(
            &lines("Arm B is pending re-run on the reference host."),
            None,
        );
        assert_eq!(p, vec![(1, PendingProblem::NoCitation)]);
        assert!(u.is_empty());
        let (p, _) = scan_pending(&lines("Arm B is pending re-run (#812)."), None);
        assert!(p.is_empty());
        let (p, _) = scan_pending(&lines("Nothing is pending here."), None);
        assert!(p.is_empty());
    }

    fn github_forge() -> crate::forge::Forge {
        crate::forge::Forge {
            kind: crate::forge::ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }
    }

    #[test]
    fn pending_citation_must_include_an_open_issue_when_state_is_required() {
        let mut api = crate::forge::CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/issues/1".into(),
            serde_json::json!({"state": "closed"}),
        );
        api.responses.insert(
            "github:repos/o/r/issues/2".into(),
            serde_json::json!({"state": "open"}),
        );
        api.responses.insert(
            "github:repos/x/y/issues/9".into(),
            serde_json::json!({"state": "OPEN"}),
        );

        let mut states = IssueStates::new(&api, Ok(github_forge()));
        let (p, u) = scan_pending(&lines("B is pending re-run (#1)."), Some(&mut states));
        assert_eq!(p, vec![(1, PendingProblem::Closed(vec!["#1".into()]))]);
        assert!(u.is_empty());

        let (p, u) = scan_pending(&lines("B is pending re-run (#1, #2)."), Some(&mut states));
        assert!(p.is_empty() && u.is_empty());

        // Another repository's open issue does not satisfy a claim about this one...
        let other = lines("B is pending re-run, see https://github.com/x/y/issues/9.");
        let (p, u) = scan_pending(&other, Some(&mut states));
        assert_eq!(
            p,
            vec![(1, PendingProblem::OutsideRepository(vec!["x/y#9".into()]))]
        );
        assert!(u.is_empty());
        // ...unless that repository is listed.
        let mut allowed =
            IssueStates::new(&api, Ok(github_forge())).allow_repositories(&["x/y".to_string()]);
        let (p, u) = scan_pending(&other, Some(&mut allowed));
        assert!(p.is_empty() && u.is_empty(), "{p:?} {u:?}");
    }

    #[test]
    fn base_registry_entries_still_bind_a_change_that_removes_them() {
        let base = load_registry(REGISTRY, "r").unwrap();
        let head = load_registry(
            r#"{"figures": [{"id": "old_curve", "array_sequence": [1.0, 1.9, 12.0]}]}"#,
            "r",
        )
        .unwrap();
        let merged = merge_registries(base, head);
        assert_eq!(
            merged.len(),
            2,
            "head's copy of old_curve and base's old_deficit"
        );
        let hits =
            scan_superseded(&lines("Random lookup is 1.11x slower than stock."), &merged).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].figure_id, "old_deficit");
        // A loosened pattern in head does not replace base's: both apply, one hit per line.
        let loosened = load_registry(
            r#"{"figures": [{"id": "old_deficit", "patterns": ["9\\.99x"], "context": ["lookup"]}]}"#,
            "r",
        )
        .unwrap();
        let merged = merge_registries(load_registry(REGISTRY, "r").unwrap(), loosened);
        let hits =
            scan_superseded(&lines("Random lookup is 1.11x slower than stock."), &merged).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn a_pattern_past_the_backtrack_limit_is_an_error_not_a_pass() {
        let reg = load_registry(
            r#"{"figures": [{"id": "slow", "patterns": ["(a*)*\\1b"], "context": []}]}"#,
            "r",
        )
        .unwrap();
        // Between our limit and fancy-regex's default (1,000,000): the default finishes
        // the search, ours stops it. The input length is calibrated for that window.
        let n = (10..=24)
            .find(|&n| {
                let input = format!("{}c", "a".repeat(n));
                let default = fancy_regex::Regex::new(r"(?i)(a*)*\1b").unwrap();
                default.find(&input).is_ok() && scan_superseded(&lines(&input), &reg).is_err()
            })
            .expect("an input whose search needs more than BACKTRACK_LIMIT steps");
        assert!(n > 10, "the limit must not trip on trivial input");
    }

    #[test]
    fn pending_issues_resolve_on_gitea_forgejo_and_gitlab() {
        use crate::forge::{CannedApi, Forge, ForgeKind};
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r/issues/4".into(),
            serde_json::json!({"state": "open"}),
        );
        api.responses.insert(
            "forgejo:repos/o/r/issues/5".into(),
            serde_json::json!({"state": "closed"}),
        );
        api.responses.insert(
            "gitlab:projects/g%2Fs%2Fp/issues/6".into(),
            serde_json::json!({"state": "opened"}),
        );
        api.responses.insert(
            "gitlab:projects/g%2Fs%2Fp/merge_requests/7".into(),
            serde_json::json!({"state": "merged"}),
        );
        let on = |kind, url: &str, repo: &str| Forge {
            kind,
            url: url.into(),
            repo: repo.into(),
        };

        let mut gitea = IssueStates::new(
            &api,
            Ok(on(ForgeKind::Gitea, "https://git.example.com", "o/r")),
        );
        let (p, u) = scan_pending(&lines("pending re-run (#4)."), Some(&mut gitea));
        assert!(p.is_empty() && u.is_empty(), "{p:?} {u:?}");
        let (p, _) = scan_pending(
            &lines("pending re-run (https://git.example.com/o/r/issues/4)."),
            Some(&mut gitea),
        );
        assert!(p.is_empty());

        let mut forgejo = IssueStates::new(
            &api,
            Ok(on(ForgeKind::Forgejo, "https://codeberg.org", "o/r")),
        );
        let (p, _) = scan_pending(&lines("pending re-run (#5)."), Some(&mut forgejo));
        assert_eq!(p, vec![(1, PendingProblem::Closed(vec!["#5".into()]))]);

        let mut gitlab = IssueStates::new(
            &api,
            Ok(on(ForgeKind::GitLab, "https://gitlab.com", "g/s/p")),
        );
        let (p, u) = scan_pending(
            &lines("pending re-run (https://gitlab.com/g/s/p/-/issues/6)."),
            Some(&mut gitlab),
        );
        assert!(p.is_empty() && u.is_empty(), "{p:?} {u:?}");
        let (p, _) = scan_pending(
            &lines("pending re-run (https://gitlab.com/g/s/p/-/merge_requests/7)."),
            Some(&mut gitlab),
        );
        assert_eq!(p, vec![(1, PendingProblem::Closed(vec!["#7".into()]))]);

        // An issue on another host cannot be checked against this forge.
        let (_, u) = scan_pending(
            &lines("pending re-run (https://github.com/o/r/issues/4)."),
            Some(&mut gitea),
        );
        assert!(
            u[0].reasons[0].contains("not on this repository's forge"),
            "{u:?}"
        );
    }

    #[test]
    fn undecidable_issue_state_is_reported_not_passed() {
        let mut states = IssueStates::new(&crate::forge::NoApi, Ok(github_forge()));
        let (p, u) = scan_pending(&lines("B is pending re-run (#5)."), Some(&mut states));
        assert!(p.is_empty());
        assert_eq!(u.len(), 1);
        assert!(u[0].reasons[0].contains("o/r#5"), "{u:?}");

        let mut no_forge =
            IssueStates::new(&crate::forge::NoApi, Err("set DISCIPLINE_FORGE".into()));
        let (_, u) = scan_pending(&lines("B is pending re-run (#5)."), Some(&mut no_forge));
        assert!(u[0].reasons[0].contains("DISCIPLINE_FORGE"), "{u:?}");
    }
}
