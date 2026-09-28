//! Citation record sentinel (`citation-metadata`).
//!
//! A repository's citation record is `CITATION.cff` (GitHub's "Cite this repository",
//! Citation File Format 1.2.0) and `.zenodo.json` (read by Zenodo's GitHub integration
//! when it archives a published release). Nothing else checks them: a malformed edit
//! merges green, and a rename can move one file without the other, so the archive and
//! the citation name the same software differently.
//!
//! The gate checks each file that exists at the repository root: it parses, carries the
//! keys its format requires with values of the right shape, every ORCID has a valid
//! check digit, and every DOI is syntactically a DOI. When both exist they must agree
//! on title, authors, ORCIDs, keywords and licence. A `CITATION.cff` whose `doi` is
//! described under `identifiers` as a version DOI, or which lists a concept DOI other
//! than its `doi`, pins every citation to one release. Whether a DOI resolves is not
//! checked: that needs the network, and no gate does.
//!
//! Problems are reported only when the change adds or edits one of the two files; a
//! problem the base already had in files this change leaves alone is a note.

use crate::guards::{exempt_filter, Context, GateOutcome};
use crate::tokens;
use anyhow::{Context as _, Result};
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

pub const GATE: &str = "citation-metadata";
/// The Citation File Format record, at the repository root.
pub const CFF: &str = "CITATION.cff";
/// Zenodo's deposit metadata, at the repository root.
pub const ZENODO: &str = ".zenodo.json";

/// Crossref's recommended DOI pattern (case-insensitive).
static DOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^10\.\d{4,9}/[-._;()/:a-z0-9]+$").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(https|http|ftp|sftp)://.+").unwrap());

/// Zenodo's `upload_type` vocabulary.
const UPLOAD_TYPES: &[&str] = &[
    "publication",
    "poster",
    "presentation",
    "dataset",
    "image",
    "video",
    "software",
    "lesson",
    "physicalobject",
    "other",
];
const ACCESS_RIGHTS: &[&str] = &["open", "embargoed", "restricted", "closed"];

/// One problem in the citation record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: &'static crate::findings::FindingKind,
    /// The file the problem is reported against.
    pub file: &'static str,
    /// What tells this problem apart from another of its code in the same file: the key
    /// it concerns (`authors[0].orcid`, `title`), the first backticked name in `message`.
    pub anchor: String,
    pub message: String,
}

pub fn citation_metadata(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.citation_metadata;
    let mut out = GateOutcome::new(GATE);
    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }
    let exempt = exempt_filter(settings)?;
    let root = ctx.git.root();
    let read = |name: &str| -> Result<Option<String>> {
        let path = root.join(name);
        if exempt.matches(name) || !path.is_file() {
            return Ok(None);
        }
        std::fs::read_to_string(&path)
            .map(Some)
            .with_context(|| format!("failed to read `{name}`"))
    };
    let cff = read(CFF)?;
    let zenodo = read(ZENODO)?;
    out.examined = usize::from(cff.is_some()) + usize::from(zenodo.is_some());
    if out.examined == 0 {
        out.notes.push(format!(
            "no `{CFF}` or `{ZENODO}` at the repository root: no citation record to check"
        ));
        return Ok(out);
    }

    let changed = ctx.git.changed_files()?;
    let touched = changed
        .iter()
        .any(|f| !f.is_deleted() && (f.path == CFF || f.path == ZENODO));
    let severity = ctx.overridable(settings.severity);
    for problem in check(cff.as_deref(), zenodo.as_deref()) {
        if !touched {
            out.notes.push(format!(
                "`{}`: {} (the base already has this and the change edits neither `{CFF}` nor `{ZENODO}`; the next change that edits one must resolve it)",
                problem.file, problem.message
            ));
            continue;
        }
        if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_CITATION_METADATA, problem.file) {
            out.overrides.push(ov);
            continue;
        }
        out.push(
            severity,
            problem.kind,
            Some(problem.file),
            None,
            problem.message.clone(),
            &format!(
                "correct `{}`, or justify it on its own line in the PR body or a commit message: `allow-citation-metadata: {} <reason>`",
                problem.file, problem.file
            ),
        );
        out.anchor_last(problem.anchor);
    }
    Ok(out)
}

/// Every problem in the citation record: `cff` and `zenodo` are the two files' text,
/// `None` for a file that does not exist.
pub fn check(cff: Option<&str>, zenodo: Option<&str>) -> Vec<Problem> {
    let mut problems = Vec::new();
    let cff = cff.and_then(|text| check_cff(text, &mut problems));
    let zenodo = zenodo.and_then(|text| check_zenodo(text, &mut problems));
    if let (Some(cff), Some(zenodo)) = (cff, zenodo) {
        compare(&cff, &zenodo, &mut problems);
    }
    problems
}

/// What the two files say about the work, for comparing them.
#[derive(Debug, Default)]
struct Record {
    title: Option<String>,
    /// `Family, Given` (or an entity's name) to its bare ORCID, if any.
    authors: Vec<(String, Option<String>)>,
    keywords: Option<BTreeSet<String>>,
    /// Lower-cased licence identifiers.
    licenses: Option<Vec<String>>,
}

fn problem(
    problems: &mut Vec<Problem>,
    kind: &'static crate::findings::FindingKind,
    file: &'static str,
    message: String,
) {
    let anchor = message
        .split('`')
        .nth(1)
        .unwrap_or(message.as_str())
        .to_string();
    problems.push(Problem {
        kind,
        file,
        anchor,
        message,
    });
}

fn check_cff(text: &str, problems: &mut Vec<Problem>) -> Option<Record> {
    use crate::findings::{CFF_INVALID, DOI_MALFORMED, DOI_NOT_CONCEPT};
    let mut bad = |m: String| problem(problems, &CFF_INVALID, CFF, m);
    let value: serde_yaml::Value = match serde_yaml::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            bad(format!("`{CFF}` is not YAML: {e}"));
            return None;
        }
    };
    let Some(map) = value.as_mapping() else {
        bad(format!("`{CFF}` is not a mapping of keys to values"));
        return None;
    };
    let get = |key: &str| map.get(serde_yaml::Value::from(key));
    let text_of = |key: &str| get(key).and_then(yaml_text);

    for key in ["cff-version", "message", "title", "authors"] {
        if get(key).is_none_or(serde_yaml::Value::is_null) {
            bad(format!("required key `{key}` is missing"));
        }
    }
    if let Some(v) = get("cff-version") {
        if yaml_text(v).as_deref() != Some("1.2.0") {
            bad(format!(
                "`cff-version` is {}, expected `1.2.0`",
                yaml_shown(v)
            ));
        }
    }
    for key in ["message", "title", "abstract"] {
        if let Some(v) = get(key) {
            if !v.is_string() {
                bad(format!("`{key}` must be a string"));
            }
        }
    }
    if let Some(t) = text_of("type") {
        if t != "software" && t != "dataset" {
            bad(format!("`type` is `{t}`, expected `software` or `dataset`"));
        }
    }
    if let Some(v) = get("date-released") {
        match yaml_text(v) {
            Some(d) if valid_date(&d) => {}
            _ => bad(format!(
                "`date-released` is {}, expected a date `YYYY-MM-DD`",
                yaml_shown(v)
            )),
        }
    }
    for key in [
        "repository-code",
        "repository",
        "repository-artifact",
        "url",
    ] {
        if let Some(u) = text_of(key) {
            if !URL.is_match(&u) {
                bad(format!("`{key}` is not a URL: `{u}`"));
            }
        }
    }

    let mut record = Record {
        title: text_of("title"),
        ..Default::default()
    };
    match get("authors") {
        Some(serde_yaml::Value::Sequence(list)) if !list.is_empty() => {
            for (i, author) in list.iter().enumerate() {
                let field = |k: &str| {
                    author
                        .as_mapping()
                        .and_then(|m| m.get(serde_yaml::Value::from(k)))
                        .and_then(yaml_text)
                };
                let name = match (field("family-names"), field("given-names"), field("name")) {
                    (Some(f), Some(g), _) => format!("{f}, {g}"),
                    (Some(f), None, _) => f,
                    (None, _, Some(n)) => n,
                    (None, Some(g), None) => g,
                    (None, None, None) => {
                        bad(format!(
                            "`authors[{i}]` has no `family-names`, `given-names` or `name`"
                        ));
                        continue;
                    }
                };
                let orcid = field("orcid").and_then(|o| match bare_orcid(&o, true) {
                    Ok(id) => Some(id),
                    Err(why) => {
                        bad(format!("`authors[{i}].orcid` {why}: `{o}`"));
                        None
                    }
                });
                record.authors.push((name, orcid));
            }
        }
        Some(serde_yaml::Value::Sequence(_)) => bad("`authors` is empty".into()),
        Some(v) if !v.is_null() => bad("`authors` must be a list".into()),
        _ => {}
    }
    record.keywords = match get("keywords") {
        None => None,
        Some(v) => match yaml_strings(v) {
            Some(k) => Some(k.into_iter().collect()),
            None => {
                bad("`keywords` must be a list of strings".into());
                None
            }
        },
    };
    record.licenses = match get("license") {
        None => None,
        Some(v) => match yaml_text(v).map(|l| vec![l]).or_else(|| yaml_strings(v)) {
            Some(l) => Some(l.iter().map(|s| s.to_lowercase()).collect()),
            None => {
                bad("`license` must be a licence identifier or a list of them".into());
                None
            }
        },
    };

    // DOIs: the top-level one and every `doi` identifier.
    let doi = text_of("doi");
    if let Some(d) = &doi {
        if !DOI.is_match(d) {
            problem(
                problems,
                &DOI_MALFORMED,
                CFF,
                format!("`doi` is not a DOI (`10.<registrant>/<suffix>`, no URL prefix): `{d}`"),
            );
        }
    }
    let mut identifiers: Vec<(String, String)> = Vec::new();
    match get("identifiers") {
        None => {}
        Some(serde_yaml::Value::Sequence(list)) => {
            for (i, ident) in list.iter().enumerate() {
                let field = |k: &str| {
                    ident
                        .as_mapping()
                        .and_then(|m| m.get(serde_yaml::Value::from(k)))
                        .and_then(yaml_text)
                };
                let (Some(kind), Some(value)) = (field("type"), field("value")) else {
                    problem(
                        problems,
                        &CFF_INVALID,
                        CFF,
                        format!("`identifiers[{i}]` needs a `type` and a `value`"),
                    );
                    continue;
                };
                if !matches!(kind.as_str(), "doi" | "url" | "swh" | "other") {
                    problem(
                        problems,
                        &CFF_INVALID,
                        CFF,
                        format!("`identifiers[{i}].type` is `{kind}`, expected `doi`, `url`, `swh` or `other`"),
                    );
                }
                if kind == "doi" {
                    if DOI.is_match(&value) {
                        identifiers.push((value, field("description").unwrap_or_default()));
                    } else {
                        problem(
                            problems,
                            &DOI_MALFORMED,
                            CFF,
                            format!("`identifiers[{i}].value` is not a DOI: `{value}`"),
                        );
                    }
                }
            }
        }
        Some(_) => problem(
            problems,
            &CFF_INVALID,
            CFF,
            "`identifiers` must be a list".into(),
        ),
    }
    if let Some(d) = doi.filter(|d| DOI.is_match(d)) {
        let described = |id: &str| {
            identifiers
                .iter()
                .find(|(v, _)| v.eq_ignore_ascii_case(id))
                .map(|(_, desc)| desc.to_lowercase())
        };
        let concept = identifiers
            .iter()
            .find(|(_, desc)| desc.to_lowercase().contains("concept"))
            .map(|(v, _)| v.clone());
        if let Some(c) = concept.filter(|c| !c.eq_ignore_ascii_case(&d)) {
            problem(
                problems,
                &DOI_NOT_CONCEPT,
                CFF,
                format!("`doi` is `{d}`, but `identifiers` describes `{c}` as the concept DOI; `doi` should name the work across all its releases"),
            );
        } else if described(&d)
            .is_some_and(|desc| desc.contains("version") && !desc.contains("concept"))
        {
            problem(
                problems,
                &DOI_NOT_CONCEPT,
                CFF,
                format!("`doi` is `{d}`, which `identifiers` describes as a version DOI; a version DOI as `doi` pins every citation to one release"),
            );
        }
    }
    Some(record)
}

fn check_zenodo(text: &str, problems: &mut Vec<Problem>) -> Option<Record> {
    use crate::findings::{DOI_MALFORMED, ZENODO_INVALID};
    let mut bad = |m: String| problem(problems, &ZENODO_INVALID, ZENODO, m);
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            bad(format!("`{ZENODO}` is not JSON: {e}"));
            return None;
        }
    };
    let Some(map) = value.as_object() else {
        bad(format!("`{ZENODO}` is not a JSON object"));
        return None;
    };
    let text_of = |key: &str| map.get(key).and_then(|v| v.as_str()).map(str::to_string);
    for key in ["title", "description", "license", "language", "version"] {
        if map.get(key).is_some_and(|v| !v.is_string()) {
            bad(format!("`{key}` must be a string"));
        }
    }
    if let Some(t) = text_of("upload_type") {
        if !UPLOAD_TYPES.contains(&t.as_str()) {
            bad(format!(
                "`upload_type` is `{t}`, not one of Zenodo's: {}",
                UPLOAD_TYPES.join(", ")
            ));
        }
    }
    if let Some(a) = text_of("access_right") {
        if !ACCESS_RIGHTS.contains(&a.as_str()) {
            bad(format!(
                "`access_right` is `{a}`, expected one of {}",
                ACCESS_RIGHTS.join(", ")
            ));
        }
    }

    let mut record = Record {
        title: text_of("title"),
        ..Default::default()
    };
    match map.get("creators") {
        None => bad("required key `creators` is missing".into()),
        Some(serde_json::Value::Array(list)) if !list.is_empty() => {
            for (i, creator) in list.iter().enumerate() {
                let field = |k: &str| creator.get(k).and_then(|v| v.as_str()).map(str::to_string);
                let Some(name) = field("name").filter(|n| !n.trim().is_empty()) else {
                    bad(format!("`creators[{i}]` has no `name`"));
                    continue;
                };
                let orcid = field("orcid").and_then(|o| match bare_orcid(&o, false) {
                    Ok(id) => Some(id),
                    Err(why) => {
                        bad(format!("`creators[{i}].orcid` {why}: `{o}`"));
                        None
                    }
                });
                record.authors.push((name, orcid));
            }
        }
        Some(serde_json::Value::Array(_)) => bad("`creators` is empty".into()),
        Some(_) => bad("`creators` must be a list".into()),
    }
    record.keywords = match map.get("keywords") {
        None => None,
        Some(v) => match v.as_array().and_then(|a| {
            a.iter()
                .map(|k| k.as_str().map(str::to_string))
                .collect::<Option<BTreeSet<_>>>()
        }) {
            Some(k) => Some(k),
            None => {
                bad("`keywords` must be a list of strings".into());
                None
            }
        },
    };
    record.licenses = text_of("license").map(|l| vec![l.to_lowercase()]);

    if let Some(d) = text_of("doi") {
        if !DOI.is_match(&d) {
            problem(
                problems,
                &DOI_MALFORMED,
                ZENODO,
                format!("`doi` is not a DOI: `{d}`"),
            );
        }
    }
    match map.get("related_identifiers") {
        None => {}
        Some(serde_json::Value::Array(list)) => {
            for (i, rel) in list.iter().enumerate() {
                let field = |k: &str| rel.get(k).and_then(|v| v.as_str()).map(str::to_string);
                let (Some(id), Some(_)) = (field("identifier"), field("relation")) else {
                    problem(
                        problems,
                        &ZENODO_INVALID,
                        ZENODO,
                        format!(
                            "`related_identifiers[{i}]` needs an `identifier` and a `relation`"
                        ),
                    );
                    continue;
                };
                let is_doi = field("scheme").is_some_and(|s| s.eq_ignore_ascii_case("doi"))
                    || id.starts_with("10.");
                if is_doi && !DOI.is_match(&id) {
                    problem(
                        problems,
                        &DOI_MALFORMED,
                        ZENODO,
                        format!("`related_identifiers[{i}].identifier` is not a DOI: `{id}`"),
                    );
                }
            }
        }
        Some(_) => problem(
            problems,
            &ZENODO_INVALID,
            ZENODO,
            "`related_identifiers` must be a list".into(),
        ),
    }
    Some(record)
}

/// The two records must describe the same work: what one leaves out is not compared.
fn compare(cff: &Record, zenodo: &Record, problems: &mut Vec<Problem>) {
    use crate::findings::RECORDS_DISAGREE;
    let mut disagree = |m: String| problem(problems, &RECORDS_DISAGREE, ZENODO, m);
    if let (Some(a), Some(b)) = (&cff.title, &zenodo.title) {
        if a != b {
            disagree(format!(
                "`title` differs: `{CFF}` has `{a}`, `{ZENODO}` has `{b}`"
            ));
        }
    }
    let names = |r: &Record| {
        r.authors
            .iter()
            .map(|(n, _)| n.clone())
            .collect::<BTreeSet<_>>()
    };
    let (a, b) = (names(cff), names(zenodo));
    if !a.is_empty() && !b.is_empty() && a != b {
        disagree(format!(
            "`authors` differ: `{CFF}` has {a:?}, `{ZENODO}` has {b:?} (`{ZENODO}` names a person `Family, Given`)"
        ));
    }
    for (name, orcid) in &cff.authors {
        let other = zenodo.authors.iter().find(|(n, _)| n == name);
        if let (Some(x), Some((_, Some(y)))) = (orcid, other) {
            if x != y {
                disagree(format!(
                    "`{name}`: ORCID differs, `{CFF}` has `{x}`, `{ZENODO}` has `{y}`"
                ));
            }
        }
    }
    if let (Some(a), Some(b)) = (&cff.keywords, &zenodo.keywords) {
        if a != b {
            disagree(format!(
                "`keywords` differ: only in `{CFF}`: {:?}; only in `{ZENODO}`: {:?}",
                a.difference(b).collect::<Vec<_>>(),
                b.difference(a).collect::<Vec<_>>()
            ));
        }
    }
    if let (Some(a), Some(b)) = (&cff.licenses, &zenodo.licenses) {
        if let Some(z) = b.first().filter(|z| !a.contains(z)) {
            disagree(format!(
                "`license` differs: `{z}` in `{ZENODO}` is not one of `{CFF}`'s ({})",
                a.join(", ")
            ));
        }
    }
}

/// An ORCID iD's bare form (`0000-0002-1825-0097`) after its shape and ISO 7064 11,2
/// check digit are verified. `CITATION.cff` writes it as `https://orcid.org/<id>`;
/// Zenodo takes the bare iD, and the URL form is accepted there too.
pub fn bare_orcid(value: &str, url_required: bool) -> Result<String, &'static str> {
    let id = match value.strip_prefix("https://orcid.org/") {
        Some(id) => id,
        None if url_required => return Err("must be `https://orcid.org/<iD>`"),
        None => value,
    };
    let digits: Vec<char> = id.chars().filter(|c| *c != '-').collect();
    let shaped = id.len() == 19
        && id.chars().enumerate().all(|(i, c)| {
            if matches!(i, 4 | 9 | 14) {
                c == '-'
            } else {
                c.is_ascii_digit() || (i == 18 && c == 'X')
            }
        });
    if !shaped {
        return Err("is not an ORCID iD (`0000-0000-0000-000X`)");
    }
    let total = digits[..15]
        .iter()
        .fold(0u32, |t, c| (t + c.to_digit(10).unwrap_or(0)) * 2);
    let check = (12 - total % 11) % 11;
    let expected = if check == 10 {
        'X'
    } else {
        char::from_digit(check, 10).unwrap_or('?')
    };
    if digits[15] != expected {
        return Err("has a wrong check digit");
    }
    Ok(id.to_string())
}

fn valid_date(d: &str) -> bool {
    DATE.captures(d).is_some_and(|c| {
        let n = |i: usize| c[i].parse::<u32>().unwrap_or(0);
        let (y, m, day) = (n(1), n(2), n(3));
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days = match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return false,
        };
        (1..=days).contains(&day)
    })
}

/// A scalar as text: YAML reads `1.2.0` and `2026-09-28` as strings, `1.2` as a number.
fn yaml_text(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn yaml_strings(v: &serde_yaml::Value) -> Option<Vec<String>> {
    v.as_sequence()?
        .iter()
        .map(|s| s.as_str().map(str::to_string))
        .collect()
}

fn yaml_shown(v: &serde_yaml::Value) -> String {
    yaml_text(v).map_or_else(|| "not a string".into(), |t| format!("`{t}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{
        CFF_INVALID, DOI_MALFORMED, DOI_NOT_CONCEPT, RECORDS_DISAGREE, ZENODO_INVALID,
    };

    const GOOD_CFF: &str = "cff-version: 1.2.0\nmessage: \"Cite it.\"\ntitle: \"Tool: a thing\"\ntype: software\nversion: 1.0.0\ndate-released: 2026-09-28\nauthors:\n  - family-names: \"Doe\"\n    given-names: \"Jane\"\n    orcid: \"https://orcid.org/0000-0002-1825-0097\"\nlicense:\n  - MIT\n  - Apache-2.0\nrepository-code: \"https://example.org/tool\"\nkeywords:\n  - ci\n  - testing\n";
    const GOOD_ZENODO: &str = r#"{"title": "Tool: a thing", "upload_type": "software", "description": "<p>A thing.</p>", "creators": [{"name": "Doe, Jane", "orcid": "0000-0002-1825-0097"}], "license": "apache-2.0", "access_right": "open", "keywords": ["testing", "ci"]}"#;

    fn kinds(cff: Option<&str>, zenodo: Option<&str>) -> Vec<(&'static str, String)> {
        check(cff, zenodo)
            .into_iter()
            .map(|p| (p.kind.code, p.message))
            .collect()
    }

    fn codes(cff: Option<&str>, zenodo: Option<&str>) -> Vec<&'static str> {
        kinds(cff, zenodo).into_iter().map(|(c, _)| c).collect()
    }

    #[test]
    fn a_good_record_has_no_problems_alone_or_together() {
        assert_eq!(kinds(Some(GOOD_CFF), Some(GOOD_ZENODO)), vec![]);
        assert_eq!(kinds(Some(GOOD_CFF), None), vec![]);
        assert_eq!(kinds(None, Some(GOOD_ZENODO)), vec![]);
        assert_eq!(kinds(None, None), vec![]);
    }

    #[test]
    fn cff_structure_is_checked() {
        let with = |from: &str, to: &str| {
            assert!(GOOD_CFF.contains(from), "{from}");
            codes(Some(&GOOD_CFF.replace(from, to)), None)
        };
        assert_eq!(codes(Some("title: [unclosed\n"), None), [CFF_INVALID.code]);
        assert_eq!(codes(Some("- a list\n"), None), [CFF_INVALID.code]);
        assert_eq!(
            with("cff-version: 1.2.0", "cff-version: 1.1.0"),
            [CFF_INVALID.code]
        );
        assert_eq!(with("message: \"Cite it.\"\n", ""), [CFF_INVALID.code]);
        assert_eq!(with("type: software", "type: library"), [CFF_INVALID.code]);
        assert_eq!(with("2026-09-28", "2026-02-30"), [CFF_INVALID.code]);
        assert_eq!(with("2026-09-28", "28/09/2026"), [CFF_INVALID.code]);
        assert_eq!(
            with("\"https://example.org/tool\"", "example.org/tool"),
            [CFF_INVALID.code]
        );
        assert_eq!(
            with(
                "family-names: \"Doe\"\n    given-names: \"Jane\"",
                "affiliation: \"X\""
            ),
            [CFF_INVALID.code]
        );
        assert_eq!(
            with("  - ci\n  - testing\n", "  - ci\n  - [nested]\n"),
            [CFF_INVALID.code]
        );
        // An ORCID with a wrong check digit, and one not written as a URL.
        assert_eq!(with("1825-0097", "1825-0098"), [CFF_INVALID.code]);
        assert_eq!(
            with(
                "\"https://orcid.org/0000-0002-1825-0097\"",
                "\"0000-0002-1825-0097\""
            ),
            [CFF_INVALID.code]
        );
    }

    #[test]
    fn zenodo_structure_is_checked() {
        let with = |from: &str, to: &str| {
            assert!(GOOD_ZENODO.contains(from), "{from}");
            codes(None, Some(&GOOD_ZENODO.replace(from, to)))
        };
        assert_eq!(codes(None, Some("{\"title\": ")), [ZENODO_INVALID.code]);
        assert_eq!(codes(None, Some("[]")), [ZENODO_INVALID.code]);
        assert_eq!(
            codes(None, Some(r#"{"title": "x"}"#)),
            [ZENODO_INVALID.code]
        );
        assert_eq!(with(r#""software""#, r#""library""#), [ZENODO_INVALID.code]);
        assert_eq!(with(r#""open""#, r#""public""#), [ZENODO_INVALID.code]);
        assert_eq!(with(r#""name": "Doe, Jane", "#, ""), [ZENODO_INVALID.code]);
        assert_eq!(with("1825-0097", "1825-0098"), [ZENODO_INVALID.code]);
        // The URL form of an ORCID is accepted in `.zenodo.json`.
        assert_eq!(
            with(r#""0000-0002"#, r#""https://orcid.org/0000-0002"#),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn dois_are_checked_for_shape_and_the_concept_rule() {
        let concept = "doi: \"10.5281/zenodo.100\"\nidentifiers:\n  - type: doi\n    value: \"10.5281/zenodo.100\"\n    description: \"Concept DOI (all versions)\"\n  - type: doi\n    value: \"10.5281/zenodo.101\"\n    description: \"Version DOI (v1.0.0)\"\n";
        let cff = |extra: &str| format!("{GOOD_CFF}{extra}");
        assert_eq!(codes(Some(&cff(concept)), None), Vec::<&str>::new());
        // The version DOI as `doi`.
        assert_eq!(
            codes(
                Some(&cff(&concept.replacen(
                    "zenodo.100\"\nidentifiers",
                    "zenodo.101\"\nidentifiers",
                    1
                ))),
                None
            ),
            [DOI_NOT_CONCEPT.code]
        );
        // `doi` described as a version DOI, with no concept DOI listed.
        let version_only = "doi: \"10.5281/zenodo.101\"\nidentifiers:\n  - type: doi\n    value: \"10.5281/zenodo.101\"\n    description: \"Version DOI (v1.0.0)\"\n";
        assert_eq!(
            codes(Some(&cff(version_only)), None),
            [DOI_NOT_CONCEPT.code]
        );
        // A DOI written as a URL, and a malformed identifier value.
        assert_eq!(
            codes(
                Some(&cff("doi: \"https://doi.org/10.5281/zenodo.100\"\n")),
                None
            ),
            [DOI_MALFORMED.code]
        );
        assert_eq!(
            codes(
                Some(&cff(&concept.replace(
                    "value: \"10.5281/zenodo.101\"",
                    "value: \"zenodo.101\""
                ))),
                None
            ),
            [DOI_MALFORMED.code]
        );
        // A `doi` not listed under `identifiers` cannot be judged concept or version.
        assert_eq!(
            codes(Some(&cff("doi: \"10.5281/zenodo.100\"\n")), None),
            Vec::<&str>::new()
        );
        // `.zenodo.json` related identifiers.
        let related = GOOD_ZENODO.replace(
            r#""keywords""#,
            r#""related_identifiers": [{"identifier": "zenodo.123", "relation": "isSupplementTo", "scheme": "doi"}], "keywords""#,
        );
        assert_eq!(codes(None, Some(&related)), [DOI_MALFORMED.code]);
    }

    #[test]
    fn the_two_files_must_describe_the_same_work() {
        let zen = |from: &str, to: &str| {
            assert!(GOOD_ZENODO.contains(from), "{from}");
            codes(Some(GOOD_CFF), Some(&GOOD_ZENODO.replace(from, to)))
        };
        assert_eq!(
            zen(r#""Tool: a thing""#, r#""Tool""#),
            [RECORDS_DISAGREE.code]
        );
        assert_eq!(zen("Doe, Jane", "Roe, Jane"), [RECORDS_DISAGREE.code]);
        assert_eq!(
            zen("0000-0002-1825-0097", "0000-0001-7649-1668"),
            [RECORDS_DISAGREE.code]
        );
        assert_eq!(
            zen(r#"["testing", "ci"]"#, r#"["ci"]"#),
            [RECORDS_DISAGREE.code]
        );
        assert_eq!(zen("apache-2.0", "gpl-3.0"), [RECORDS_DISAGREE.code]);
        // A key one file leaves out is not compared.
        assert_eq!(
            zen(r#", "keywords": ["testing", "ci"]"#, ""),
            Vec::<&str>::new()
        );
    }

    /// Findings of one code in one file have no line, so each needs its own anchor for
    /// baselines to tell them apart.
    #[test]
    fn every_problem_of_a_code_in_a_file_has_its_own_anchor() {
        let cff = "cff-version: 1.1.0\ntitle: 3\ntype: library\ndate-released: 2026-13-01\nurl: x\nrepository-code: y\nauthors:\n  - affiliation: A\n  - family-names: Doe\n    orcid: \"0000-0002-1825-0097\"\nkeywords: 5\nlicense: {a: b}\nidentifiers:\n  - type: isbn\n    value: \"1\"\n  - {}\n";
        let zenodo = r#"{"title": 1, "description": 2, "upload_type": "x", "access_right": "y", "creators": [{}, {"name": "A", "orcid": "1"}], "keywords": [1], "related_identifiers": [{}, 3]}"#;
        for (c, z) in [(Some(cff), None), (None, Some(zenodo))] {
            let problems = check(c, z);
            assert!(problems.len() >= 8, "{problems:#?}");
            let mut seen = BTreeSet::new();
            for p in &problems {
                assert!(
                    seen.insert((p.kind.code, p.file, p.anchor.clone())),
                    "anchor `{}` repeats for `{}`: {problems:#?}",
                    p.anchor,
                    p.kind.code
                );
            }
        }
        // The records-disagree findings name their field.
        let anchors: Vec<String> = check(
            Some(GOOD_CFF),
            Some(
                &GOOD_ZENODO
                    .replace("Tool: a thing", "Tool")
                    .replace("0000-0002-1825-0097", "0000-0001-7649-1668")
                    .replace(r#"["testing", "ci"]"#, r#"["ci"]"#)
                    .replace("apache-2.0", "gpl-3.0"),
            ),
        )
        .into_iter()
        .map(|p| p.anchor)
        .collect();
        assert_eq!(anchors, ["title", "Doe, Jane", "keywords", "license"]);
    }

    #[test]
    fn orcid_check_digits() {
        // Published ORCID examples, one with an `X` check digit.
        assert_eq!(
            bare_orcid("0000-0002-1825-0097", false),
            Ok("0000-0002-1825-0097".into())
        );
        assert_eq!(
            bare_orcid("0000-0001-5109-3700", false),
            Ok("0000-0001-5109-3700".into())
        );
        assert_eq!(
            bare_orcid("0000-0002-1694-233X", false),
            Ok("0000-0002-1694-233X".into())
        );
        assert!(bare_orcid("0000-0002-1694-2339", false).is_err());
        assert!(bare_orcid("0000-0002-1825-009", false).is_err());
        assert!(bare_orcid("0000-0002-1825-0097", true).is_err());
        assert!(bare_orcid("https://orcid.org/0000-0002-1825-0097", true).is_ok());
    }
}
