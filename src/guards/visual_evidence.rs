//! Visual evidence (`visual-evidence`): a change to UI paths needs before and after
//! images in the pull request body.
//!
//! Each `[[gates.visual-evidence.rules]]` entry is a set of `trigger_paths` globs. A
//! changed path matching one fires it: an added, modified or deleted path, and both sides
//! of a rename. When any rule fires, the body must have an `After` heading with an image
//! under it, and a `Before` heading with one too unless every matched path is new:
//! - Every matched path added (a new page): only `After` is required, and a note says so.
//! - Any matched path modified, deleted or renamed: `Before` is required as well, and the
//!   finding names that path. This is decided from the diff, never from the body.
//!
//! The body is the one the declared-surface contract reads: the pull request body, the
//! merged pull request's body on a push, else the head commit message, as
//! `directives.sources` allows. A heading of any level whose text is `Before` or `After`
//! opens a section that runs to the next heading of the same or a higher level. An image
//! is a Markdown `![alt](url)` or an HTML `<img src=...>` with a non-empty target. Lines in
//! fenced code, HTML comments and `>` quotes are not read. Images are never fetched.

use crate::config::{GateSettings, VisualEvidenceRule};
use crate::could_not_check::{tag, Reason};
use crate::gitctx::ChangeKind;
use crate::guards::pr_checklist::{readable_lines, HEADING_RE};
use crate::guards::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::{anyhow, Result};
use regex::Regex;
use std::sync::LazyLock;

pub const GATE: &str = "visual-evidence";

static MARKDOWN_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\(\s*<?[^\s)>]+").expect("static regex"));
static HTML_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<img\b[^>]*\bsrc\s*=\s*(?:"[^"\s]+"|'[^'\s]+'|[^\s"'>]+)"#)
        .expect("static regex")
});

/// Which sections of a body hold an image.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Evidence {
    /// The line of the first `Before` heading, and whether its section holds an image.
    pub before: Option<(usize, bool)>,
    pub after: Option<(usize, bool)>,
}

impl Evidence {
    pub fn before_shown(&self) -> bool {
        self.before.is_some_and(|(_, img)| img)
    }
    pub fn after_shown(&self) -> bool {
        self.after.is_some_and(|(_, img)| img)
    }
}

/// Whether a line holds an image.
pub fn has_image(line: &str) -> bool {
    MARKDOWN_IMAGE.is_match(line) || HTML_IMAGE.is_match(line)
}

/// Reads a body for its `Before` and `After` sections. A second heading of the same name
/// is read too: either section holding an image counts.
pub fn read_evidence(body: &str) -> Evidence {
    #[derive(Clone, Copy, PartialEq)]
    enum Side {
        Before,
        After,
    }
    let mut evidence = Evidence::default();
    let mut open: Option<(Side, usize)> = None;
    for (line_no, line) in readable_lines(body) {
        if let Some(h) = HEADING_RE.captures(&line) {
            let level = h[1].len();
            let text = h
                .get(2)
                .map_or("", |m| m.as_str())
                .replace(['*', '_', '`'], "");
            let text = text.trim().trim_end_matches(':').trim();
            let side = if text.eq_ignore_ascii_case("before") {
                Some(Side::Before)
            } else if text.eq_ignore_ascii_case("after") {
                Some(Side::After)
            } else {
                None
            };
            match side {
                Some(side) => {
                    let slot = match side {
                        Side::Before => &mut evidence.before,
                        Side::After => &mut evidence.after,
                    };
                    slot.get_or_insert((line_no, false));
                    open = Some((side, level));
                }
                None => {
                    if open.is_some_and(|(_, l)| level <= l) {
                        open = None;
                    }
                }
            }
            continue;
        }
        if let Some((side, _)) = open {
            if has_image(&line) {
                let slot = match side {
                    Side::Before => &mut evidence.before,
                    Side::After => &mut evidence.after,
                };
                if let Some((_, img)) = slot {
                    *img = true;
                }
            }
        }
    }
    evidence
}

/// One rule's globs, compiled; refuses an empty list, a blank entry, a glob that does
/// not compile.
fn compile_rule(rule: &VisualEvidenceRule, index: usize) -> Result<PathFilter> {
    let key = format!("gates.{GATE}.rules[{index}].trigger_paths");
    let config_err = |msg: String| tag(Reason::Configuration, anyhow!(msg));
    if rule.trigger_paths.is_empty() {
        return Err(config_err(format!(
            "`{key}` is empty: the rule could never fire"
        )));
    }
    if rule.trigger_paths.iter().any(|g| g.trim().is_empty()) {
        return Err(config_err(format!(
            "`{key}` holds a blank entry, which matches no path"
        )));
    }
    PathFilter::new(&rule.trigger_paths).map_err(|e| config_err(format!("`{key}`: {e:#}")))
}

/// What the matched paths of a change require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Required {
    /// No rule fired.
    Nothing,
    /// Every matched path is new: an `After` image only.
    After { matched: Vec<String> },
    /// `before_because` existed on the base side: both images.
    BeforeAndAfter {
        matched: Vec<String>,
        before_because: String,
    },
}

/// Judges a change: each `(kind, path, old_path)` against the compiled rules.
pub fn required(
    changes: &[(ChangeKind, String, String)],
    rules: &[PathFilter],
    exempt: &PathFilter,
) -> Required {
    let mut matched = Vec::new();
    let mut before_because = None;
    for (kind, path, old) in changes {
        for side in [path, old] {
            if side.is_empty() || exempt.matches(side) || !rules.iter().any(|r| r.matches(side)) {
                continue;
            }
            if !matched.contains(side) {
                matched.push(side.clone());
            }
            if *kind != ChangeKind::Added && before_because.is_none() {
                before_because = Some(side.clone());
            }
        }
    }
    if matched.is_empty() {
        return Required::Nothing;
    }
    matched.sort();
    match before_because {
        Some(before_because) => Required::BeforeAndAfter {
            matched,
            before_because,
        },
        None => Required::After { matched },
    }
}

pub fn evaluate_visual_evidence(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.visual_evidence;
    let mut out = GateOutcome::new(GATE);
    if settings.rules.is_empty() {
        return Err(tag(
            Reason::Configuration,
            anyhow!("`gates.{GATE}` is enabled but no rules are configured"),
        ));
    }
    if let Some(bad) = settings
        .exempt_authors
        .iter()
        .find(|a| a.trim().is_empty() || a.chars().any(char::is_whitespace))
    {
        return Err(tag(
            Reason::Configuration,
            anyhow!("`gates.{GATE}.exempt_authors` entry {bad:?} is not a login"),
        ));
    }
    let rules = settings
        .rules
        .iter()
        .enumerate()
        .map(|(i, r)| compile_rule(r, i))
        .collect::<Result<Vec<_>>>()?;
    let exempt = exempt_filter(settings)?;

    let changes: Vec<(ChangeKind, String, String)> = ctx
        .git
        .changed_files()?
        .into_iter()
        .map(|f| (f.kind, f.path, f.old_path))
        .collect();
    let (matched, before_because) = match required(&changes, &rules, &exempt) {
        Required::Nothing => {
            out.notes
                .push("no changed path matches any rule's `trigger_paths`".into());
            return Ok(out);
        }
        Required::After { matched } => {
            out.notes
                .push("every matched path is new to this change: `Before` is not required".into());
            (matched, None)
        }
        Required::BeforeAndAfter {
            matched,
            before_because,
        } => (matched, Some(before_because)),
    };
    out.examined = 1;

    let author = crate::guards::issue_link::exemption_author(ctx, &settings.exempt_authors, GATE)?;
    if let Some(author) =
        crate::guards::issue_link::exempt_author(author.as_deref(), &settings.exempt_authors)
    {
        out.notes.push(format!(
            "pull request opened by `{author}`, listed in `exempt_authors`: no visual evidence required"
        ));
        return Ok(out);
    }

    let body = &ctx.declaration_body;
    let evidence = body.text.as_deref().map(read_evidence).unwrap_or_default();
    let shown: Vec<String> = matched.iter().take(3).map(|p| format!("`{p}`")).collect();
    let more = if matched.len() > 3 {
        format!(" and {} more", matched.len() - 3)
    } else {
        String::new()
    };
    let what = format!("This change touches {}{more}", shown.join(", "));
    let reasons: Vec<&str> = settings
        .rules
        .iter()
        .zip(&rules)
        .filter(|(_, f)| matched.iter().any(|p| f.matches(p)))
        .filter_map(|(r, _)| r.reason.as_deref())
        .collect();
    let reason = if reasons.is_empty() {
        String::new()
    } else {
        format!(" Reason: {}.", reasons.join("; "))
    };
    let source = if body.text.is_some() {
        body.source.clone()
    } else {
        format!("no body to read ({})", body.source)
    };
    let severity = ctx.overridable(settings.severity());

    if !evidence.after_shown() {
        let lifted = ctx.find_override(
            GATE,
            &crate::findings::VISUAL_EVIDENCE_MISSING,
            tokens::ALLOW_VISUAL_EVIDENCE,
            "after",
        );
        let section = match evidence.after {
            None => "has no `After` heading",
            Some(_) => "has an `After` heading with no image under it",
        };
        out.lift_or_push(
            lifted,
            severity,
            &crate::findings::VISUAL_EVIDENCE_MISSING,
            (Some("PR body"), evidence.after.map(|(l, _)| l)),
            format!("{what}, which needs visual evidence, and {source} {section}.{reason}"),
            "Add an `After` heading with a screenshot under it (`![after](url)` or `<img src=...>`), or justify with `allow-visual-evidence: after <reason>`.",
        );
    }
    if let Some(because) = before_because {
        if !evidence.before_shown() {
            let lifted = ctx.find_override(
                GATE,
                &crate::findings::VISUAL_EVIDENCE_INCOMPLETE,
                tokens::ALLOW_VISUAL_EVIDENCE,
                "before",
            );
            let section = match evidence.before {
                None => "has no `Before` heading",
                Some(_) => "has a `Before` heading with no image under it",
            };
            out.lift_or_push(
                lifted,
                severity,
                &crate::findings::VISUAL_EVIDENCE_INCOMPLETE,
                (Some("PR body"), evidence.before.map(|(l, _)| l)),
                format!(
                    "{what}; `{because}` existed before this change, so a `Before` image is required, and {source} {section}.{reason}"
                ),
                "Add a `Before` heading with a screenshot of the screen as it was, or justify with `allow-visual-evidence: before <reason>`.",
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filters(globs: &[&str]) -> Vec<PathFilter> {
        vec![PathFilter::new(&globs.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()]
    }

    fn change(kind: ChangeKind, path: &str, old: &str) -> (ChangeKind, String, String) {
        (kind, path.into(), old.into())
    }

    #[test]
    fn images_under_before_and_after_headings_are_read_and_examples_are_not() {
        let body = "\
## Summary
![logo](x.png)

## Before
![before](https://example.com/a.png)

### Details
text

## After
<img width=\"300\" src=\"https://example.com/b.png\">
";
        let e = read_evidence(body);
        assert_eq!(e.before, Some((4, true)));
        assert_eq!(e.after, Some((10, true)));

        // The Summary image is outside both sections; a sub-heading does not close a section.
        let e = read_evidence("## Before\n### shot\n![b](b.png)\n## After\ntext only\n");
        assert!(e.before_shown());
        assert_eq!(e.after, Some((4, false)));

        // Fenced, quoted and commented examples are not read.
        let examples = "```md\n## After\n![a](a.png)\n```\n> ## After\n> ![a](a.png)\n<!-- ## After\n![a](a.png) -->\n";
        assert_eq!(read_evidence(examples), Evidence::default());

        // A heading ending in `:` and in bold counts; an empty target is not an image.
        let e = read_evidence("**not a heading**\n# **After:**\n![a]()\n<img src=\"\">\n");
        assert_eq!(e.after, Some((2, false)));
        assert!(read_evidence("# After\n![a](<b.png>)\n").after_shown());
        assert!(read_evidence("# after\n<IMG SRC=b.png>\n").after_shown());
        // A section closed by a heading of the same level loses later images.
        assert!(!read_evidence("## After\n## Notes\n![a](a.png)\n").after_shown());
    }

    #[test]
    fn before_is_required_only_when_a_matched_path_existed_on_base() {
        let rules = filters(&["web/**"]);
        let none = PathFilter::new(&[]).unwrap();
        // All added: After only.
        assert_eq!(
            required(
                &[change(ChangeKind::Added, "web/new.tsx", "")],
                &rules,
                &none
            ),
            Required::After {
                matched: vec!["web/new.tsx".into()]
            }
        );
        // A modified, deleted or renamed matched path needs Before, naming it.
        for (kind, path, old) in [
            (ChangeKind::Modified, "web/nav.tsx", ""),
            (ChangeKind::Deleted, "web/old.tsx", ""),
            (ChangeKind::Renamed, "lib/moved.tsx", "web/moved.tsx"),
        ] {
            let got = required(
                &[
                    change(ChangeKind::Added, "web/new.tsx", ""),
                    change(kind, path, old),
                ],
                &rules,
                &none,
            );
            let want_because = if old.is_empty() { path } else { old };
            assert!(
                matches!(&got, Required::BeforeAndAfter { before_because, .. } if before_because == want_because),
                "{got:?}"
            );
        }
        // Unmatched or exempt paths fire nothing.
        let exempt = PathFilter::new(&["web/gen/**".to_string()]).unwrap();
        assert_eq!(
            required(
                &[
                    change(ChangeKind::Modified, "src/lib.rs", ""),
                    change(ChangeKind::Modified, "web/gen/x.tsx", "")
                ],
                &rules,
                &exempt
            ),
            Required::Nothing
        );
    }

    #[test]
    fn a_rule_that_could_never_fire_is_a_configuration_error() {
        let rule = |globs: &[&str]| VisualEvidenceRule {
            trigger_paths: globs.iter().map(|s| s.to_string()).collect(),
            reason: None,
        };
        assert!(compile_rule(&rule(&["web/**"]), 0).is_ok());
        assert!(compile_rule(&rule(&[]), 0).is_err());
        assert!(compile_rule(&rule(&[" "]), 0).is_err());
        assert!(compile_rule(&rule(&["web/["]), 0).is_err());
    }
}
