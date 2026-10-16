//! Mechanism sections sentinel (`mechanism-sections`).
//!
//! Requires `(inferred)` or `(verified: <target>)` evidence tags in root cause and mechanism sections.

use crate::findings::FindingKind;
use crate::guards::{Context, GateOutcome, PathFilter};
use anyhow::Result;
use std::sync::LazyLock;

pub const GATE: &str = "mechanism-sections";

static INFERRED_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\(inferred\)").expect("valid regex"));
static VERIFIED_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\(verified:\s*([^)]+)\)").expect("valid regex"));

#[derive(Debug, Clone)]
pub struct SectionClaim {
    pub line_num: usize,
    pub text: String,
}

pub struct DesignatedSection {
    pub heading_line: usize,
    pub heading_title: String,
    pub claims: Vec<SectionClaim>,
}

/// The level and title of an ATX heading (`#` to `######` followed by a blank or the end
/// of the line), or `None` for any other line. `#hashtag` is not a heading.
fn atx_heading(trimmed: &str) -> Option<(usize, &str)> {
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    // An optional closing sequence (`## Title ##`) is not part of the title.
    let title = rest.trim().trim_end_matches('#').trim_end();
    Some((level, title))
}

/// Whether `title` names the designated heading `designated`: the same words, case
/// ignored, optionally followed by more words or punctuation (`Root cause (summary)`,
/// `Root cause analysis`, `Mechanism: lock ordering`), after leading numbering
/// (`2.`, `2)`) and emphasis markers are set aside. `Causes` does not name `Cause`.
pub fn heading_matches(title: &str, designated: &str) -> bool {
    let designated = designated.trim().to_lowercase();
    if designated.is_empty() {
        return false;
    }
    let unnumbered = title
        .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ')'))
        .trim();
    let plain =
        unnumbered.trim_matches(|c: char| matches!(c, '*' | '_' | '`') || c.is_whitespace());
    let lower = plain.to_lowercase();
    lower
        .strip_prefix(&designated)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
}

/// Parses designated sections from a markdown document.
pub fn parse_designated_sections(content: &str, headings: &[String]) -> Vec<DesignatedSection> {
    let mut sections = Vec::new();

    let mut current_section: Option<DesignatedSection> = None;
    let mut current_level = 0;
    let mut current_paragraph: Option<(usize, Vec<String>)> = None;
    // The open fence's character (`` ` `` or `~`) and length: CommonMark closes a fence
    // only with the same character, at least as many times, and nothing after it.
    let mut fence: Option<(char, usize)> = None;

    let flush_paragraph = |para: &mut Option<(usize, Vec<String>)>,
                           claims: &mut Vec<SectionClaim>| {
        if let Some((line_num, lines)) = para.take() {
            let text = lines.join(" ").trim().to_string();
            if !text.is_empty() {
                claims.push(SectionClaim { line_num, text });
            }
        }
    };

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if let Some(ch) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) {
            let count = trimmed.chars().take_while(|c| *c == ch).count();
            if count >= 3 {
                match fence {
                    None => {
                        // A fence ends the paragraph before it.
                        if let Some(ref mut sec) = current_section {
                            flush_paragraph(&mut current_paragraph, &mut sec.claims);
                        }
                        fence = Some((ch, count));
                    }
                    Some((open, open_count))
                        if open == ch
                            && count >= open_count
                            && trimmed[count..].trim().is_empty() =>
                    {
                        fence = None;
                    }
                    Some(_) => {}
                }
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }

        if let Some((level, title)) = atx_heading(trimmed) {
            if let Some(mut sec) = current_section.take() {
                flush_paragraph(&mut current_paragraph, &mut sec.claims);
                if level <= current_level {
                    sections.push(sec);
                } else {
                    // Subheading within current designated section
                    current_section = Some(sec);
                }
            }

            // Check if this new heading matches one of the designated headings
            let is_match = headings.iter().any(|h| heading_matches(title, h));

            if is_match {
                if let Some(sec) = current_section.take() {
                    sections.push(sec);
                }
                current_level = level;
                current_section = Some(DesignatedSection {
                    heading_line: line_num,
                    heading_title: title.to_string(),
                    claims: Vec::new(),
                });
            }
            continue;
        }

        if let Some(ref mut sec) = current_section {
            if trimmed.is_empty() {
                flush_paragraph(&mut current_paragraph, &mut sec.claims);
                continue;
            }

            // Check for list items: -, *, +, 1., etc.
            let is_list_item = trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("+ ")
                || (trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && trimmed.contains(". "));

            if is_list_item {
                flush_paragraph(&mut current_paragraph, &mut sec.claims);
                let item_text = if let Some(stripped) = trimmed
                    .strip_prefix("- ")
                    .or_else(|| trimmed.strip_prefix("* "))
                    .or_else(|| trimmed.strip_prefix("+ "))
                {
                    stripped.to_string()
                } else if let Some((_, rest)) = trimmed.split_once(". ") {
                    rest.to_string()
                } else {
                    trimmed.to_string()
                };
                sec.claims.push(SectionClaim {
                    line_num,
                    text: item_text,
                });
            } else {
                // Continuation or start of a paragraph
                if let Some((_, ref mut lines)) = current_paragraph {
                    lines.push(trimmed.to_string());
                } else {
                    current_paragraph = Some((line_num, vec![trimmed.to_string()]));
                }
            }
        }
    }

    if let Some(mut sec) = current_section.take() {
        flush_paragraph(&mut current_paragraph, &mut sec.claims);
        sections.push(sec);
    }

    sections
}

/// Whether a `(verified: <target>)` target resolves: a path tracked at head (in the
/// index, so a file only on disk does not count) whose content, when the target names a
/// symbol after `::`, contains that symbol.
fn target_exists(target: &str, ctx: &Context) -> Result<bool> {
    let clean = target
        .trim()
        .trim_matches(|c| matches!(c, '`' | '"' | '\'' | '(' | ')'));

    // Reject absolute paths, parent directory traversal, and empty targets:
    if clean.is_empty() || clean.starts_with('/') || clean.contains("..") || clean == "." {
        return Ok(false);
    }

    let (file_part, symbol_part) = match clean.split_once("::") {
        Some((f, s)) => (f.trim(), Some(s.trim())),
        None => (clean, None),
    };

    // An untracked file on disk is not evidence a reviewer can check out.
    if !ctx.git.is_tracked(file_part)? {
        return Ok(false);
    }
    // Tracked, but a directory, a deleted file or a binary is not readable evidence.
    let Some(content) = ctx.git.head_content(file_part)? else {
        return Ok(false);
    };

    // If a symbol/test was named after '::', it must exist in the file content:
    if let Some(symbol) = symbol_part {
        if !symbol.is_empty() && !content.contains(symbol) {
            return Ok(false);
        }
    }

    Ok(true)
}

/// At most 60 characters of `text`, cut on a character boundary.
fn truncate_claim(text: &str) -> String {
    if text.chars().count() > 60 {
        let head: String = text.chars().take(57).collect();
        format!("{head}...")
    } else {
        text.to_string()
    }
}

/// One finding of [`judge_document`].
#[derive(Debug, Clone)]
pub struct MechanismFinding {
    pub kind: &'static FindingKind,
    pub line: usize,
    pub message: String,
    pub remediation: &'static str,
}

/// What [`judge_document`] found in one document.
#[derive(Debug, Default)]
pub struct DocumentVerdict {
    /// Designated sections found.
    pub sections: usize,
    /// Claims examined.
    pub claims: usize,
    pub findings: Vec<MechanismFinding>,
}

/// Judges one Markdown document: every claim in a designated section must carry
/// `(inferred)` or a `(verified: <target>)` whose target `target_exists` resolves.
pub fn judge_document(
    content: &str,
    headings: &[String],
    mut target_exists: impl FnMut(&str) -> Result<bool>,
) -> Result<DocumentVerdict> {
    let mut verdict = DocumentVerdict::default();
    let sections = parse_designated_sections(content, headings);
    verdict.sections = sections.len();

    for section in sections {
        if section.claims.is_empty() {
            verdict.findings.push(MechanismFinding {
                kind: &crate::findings::EMPTY_DESIGNATED_SECTION,
                line: section.heading_line,
                message: format!("designated section `{}` is empty", section.heading_title),
                remediation: "provide substantive mechanism claims with evidence tags",
            });
            continue;
        }

        for claim in section.claims {
            verdict.claims += 1;

            let has_inferred = INFERRED_RE.is_match(&claim.text);
            let verified_caps: Vec<_> = VERIFIED_RE.captures_iter(&claim.text).collect();

            if !has_inferred && verified_caps.is_empty() {
                verdict.findings.push(MechanismFinding {
                    kind: &crate::findings::UNTAGGED_MECHANISM_CLAIM,
                    line: claim.line_num,
                    message: format!(
                        "claim in `{}` section lacks evidence tag: \"{}\"",
                        section.heading_title,
                        truncate_claim(&claim.text)
                    ),
                    remediation:
                        "append `(inferred)` or `(verified: <test or artifact>)` to the claim",
                });
                continue;
            }

            for cap in verified_caps {
                if let Some(target_match) = cap.get(1) {
                    let target = target_match.as_str().trim();
                    if !target_exists(target)? {
                        verdict.findings.push(MechanismFinding {
                            kind: &crate::findings::EVIDENCE_TARGET_NOT_FOUND,
                            line: claim.line_num,
                            message: format!(
                                "verification target `{target}` cited in `{}` is not a file tracked in the repository",
                                section.heading_title
                            ),
                            remediation: "cite a tracked test file, test identifier, or artifact (`git add` it first)",
                        });
                    }
                }
            }
        }
    }
    Ok(verdict)
}

/// Evaluates markdown documents for mechanism section evidence tags.
pub fn evaluate_mechanism_sections(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.mechanism_sections;
    let mut out = GateOutcome::new(GATE);

    let files_to_check = if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        changed
            .into_iter()
            .filter(|f| !f.is_deleted() && f.path.ends_with(".md"))
            .map(|f| f.path)
            .collect::<Vec<_>>()
    } else {
        let tracked = ctx.git.tracked_files()?;
        tracked
            .into_iter()
            .filter(|p| p.ends_with(".md"))
            .collect::<Vec<_>>()
    };

    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let mut designated_sections_found = 0;

    for path in files_to_check {
        if exempt.matches(&path) {
            continue;
        }

        let Some(content) = ctx.git.head_content(&path)? else {
            continue;
        };

        let verdict = judge_document(&content, &settings.section_headings, |target| {
            target_exists(target, ctx)
        })?;
        designated_sections_found += verdict.sections;
        out.examined += verdict.claims;
        for f in verdict.findings {
            out.add_violation(
                ctx.overridable(settings.severity),
                f.kind,
                &path,
                f.line,
                f.message,
                f.remediation,
            );
        }
    }

    if designated_sections_found == 0 {
        out.notes
            .push("no designated mechanism/cause sections found in modified documents".to_string());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DisciplineConfig, MechanismSectionsGate, Severity};

    #[test]
    fn test_mechanism_sections_defaults() {
        let gate = MechanismSectionsGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(gate.diff_only);
        assert_eq!(
            gate.section_headings,
            vec![
                "Mechanism".to_string(),
                "Root cause".to_string(),
                "Cause".to_string(),
            ]
        );
    }

    fn eval(content: &str, extra_files: &[(&str, &str)]) -> GateOutcome {
        let toml = r#"
[meta]
version = 1
name = "t"
[gates.mechanism-sections]
enabled = true
diff_only = true
"#;
        let config = DisciplineConfig::from_toml_str(toml).unwrap();
        let mut head_files = vec![("docs/postmortem.md", content)];
        head_files.extend_from_slice(extra_files);
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(&[], &head_files);
        evaluate_mechanism_sections(&crate::guards::test_support::context(&config, &git)).unwrap()
    }

    #[test]
    fn test_motivating_example_verbatim() {
        let untagged = r#"
## Root cause

The latency spike occurs because hash table collisions cause lock contention across worker threads.
"#;
        let out = eval(untagged, &[]);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "mechanism-sections/untagged-mechanism-claim"
        );

        let tagged = r#"
## Root cause

The latency spike occurs because hash table collisions cause lock contention across worker threads. (inferred)
"#;
        let out_tagged = eval(tagged, &[]);
        assert!(
            out_tagged.violations.is_empty(),
            "{:?}",
            out_tagged.violations
        );
    }

    #[test]
    fn test_verified_tag_existing_and_missing() {
        let existing = r#"
## Mechanism
- Fast path bypasses serialization. (verified: tests/test_contention.rs)
"#;
        let out = eval(existing, &[("tests/test_contention.rs", "// test")]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);

        let missing = r#"
## Mechanism
- Fast path bypasses serialization. (verified: tests/missing_test.rs)
"#;
        let out_missing = eval(missing, &[]);
        assert_eq!(out_missing.violations.len(), 1);
        assert_eq!(
            out_missing.violations[0].code,
            "mechanism-sections/evidence-target-not-found"
        );
    }

    #[test]
    fn test_text_outside_designated_section_ignored() {
        let content = r#"
## Overview
The architecture is designed for scale without any tags.

## Architecture
Another paragraph with no evidence tags.
"#;
        let out = eval(content, &[]);
        assert!(out.violations.is_empty());
        assert_eq!(out.examined, 0);
        assert!(out
            .notes
            .iter()
            .any(|n| n.contains("no designated mechanism/cause sections found")));
    }

    /// 56 ASCII bytes then `été`: byte 57 falls inside the first `é`.
    fn claim_with_multibyte_at_byte_57() -> String {
        format!("{}été and the rest of the sentence", "a".repeat(56))
    }

    #[test]
    fn truncate_claim_never_splits_a_multibyte_character() {
        let text = claim_with_multibyte_at_byte_57();
        assert!(
            !text.is_char_boundary(57),
            "the control must straddle byte 57"
        );
        let shown = truncate_claim(&text);
        assert!(shown.ends_with("..."), "{shown}");
        assert_eq!(shown.chars().count(), 60, "{shown}");
        // Short text is returned whole.
        assert_eq!(truncate_claim("été"), "été");
    }

    #[test]
    fn an_untagged_non_ascii_claim_is_reported_not_a_crash() {
        let doc = format!("## Root cause\n\n{}\n", claim_with_multibyte_at_byte_57());
        let out = eval(&doc, &[]);
        assert_eq!(out.violations.len(), 1, "{:?}", out.violations);
        assert_eq!(
            out.violations[0].code,
            "mechanism-sections/untagged-mechanism-claim"
        );
    }

    #[test]
    fn a_tilde_fence_is_a_code_fence() {
        let doc = "## Root cause\n\n~~~text\nraw log output with no tag\n~~~\n\n- Lock contention. (inferred)\n";
        let out = eval(doc, &[]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert_eq!(out.examined, 1);
        // Control: the same lines outside a fence are an untagged claim.
        let bare = "## Root cause\n\nraw log output with no tag\n\n- Lock contention. (inferred)\n";
        assert_eq!(eval(bare, &[]).violations.len(), 1);
    }

    #[test]
    fn a_heading_that_starts_with_a_designated_heading_stays_in_scope() {
        for heading in [
            "## Root cause (summary)",
            "## Root cause analysis",
            "## 2. Root cause",
            "## **Root cause**",
            "## Root cause ##",
            "### Mechanism: lock ordering",
        ] {
            let doc = format!("{heading}\n\nThe pool starves under load.\n");
            let out = eval(&doc, &[]);
            assert_eq!(out.violations.len(), 1, "`{heading}`: {:?}", out.violations);
        }
        // Controls: a word that only starts with the same letters is out of scope, and
        // `#Root cause` (no blank after the `#`) is not a heading.
        for heading in [
            "## Causes of delay",
            "## Mechanisms overview",
            "#Root cause",
        ] {
            let doc = format!("{heading}\n\nThe pool starves under load.\n");
            let out = eval(&doc, &[]);
            assert!(
                out.violations.is_empty(),
                "`{heading}`: {:?}",
                out.violations
            );
        }
    }

    #[test]
    fn a_verified_target_must_be_tracked_not_only_on_disk() {
        let doc =
            "## Mechanism\n- Fast path bypasses serialization. (verified: tests/untracked.rs)\n";
        let config = DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\n[gates.mechanism-sections]\nenabled = true\n",
        )
        .unwrap();
        let (_dir, git) =
            crate::gitctx::test_support::repo_with_files(&[], &[("docs/postmortem.md", doc)]);
        let untracked = git.root().join("tests/untracked.rs");
        std::fs::create_dir_all(untracked.parent().unwrap()).unwrap();
        std::fs::write(&untracked, "// on disk, never added\n").unwrap();
        let out = evaluate_mechanism_sections(&crate::guards::test_support::context(&config, &git))
            .unwrap();
        assert_eq!(out.violations.len(), 1, "{:?}", out.violations);
        assert_eq!(
            out.violations[0].code,
            "mechanism-sections/evidence-target-not-found"
        );
        // Control: the same file once tracked resolves.
        let tracked = eval(doc, &[("tests/untracked.rs", "// tracked\n")]);
        assert!(tracked.violations.is_empty(), "{:?}", tracked.violations);
    }

    #[test]
    fn test_target_exists_traversal_and_symbol_checks() {
        // 1. Directory traversal rejected
        let traversal = "## Mechanism\n- Bug was in worker loop. (verified: ../secret.rs)\n";
        let out_trav = eval(traversal, &[]);
        assert_eq!(out_trav.violations.len(), 1);
        assert_eq!(
            out_trav.violations[0].code,
            "mechanism-sections/evidence-target-not-found"
        );

        // 2. Directory '.' rejected
        let dot_target = "## Mechanism\n- Bug was in worker loop. (verified: .)\n";
        let out_dot = eval(dot_target, &[]);
        assert_eq!(out_dot.violations.len(), 1);
        assert_eq!(
            out_dot.violations[0].code,
            "mechanism-sections/evidence-target-not-found"
        );

        // 3. Symbol in file exists vs missing
        let valid_symbol =
            "## Mechanism\n- Bug was in worker loop. (verified: src/worker.rs::process_item)\n";
        let out_valid_sym = eval(valid_symbol, &[("src/worker.rs", "fn process_item() {}")]);
        assert!(
            out_valid_sym.violations.is_empty(),
            "{:?}",
            out_valid_sym.violations
        );

        let missing_symbol =
            "## Mechanism\n- Bug was in worker loop. (verified: src/worker.rs::non_existent)\n";
        let out_missing_sym = eval(missing_symbol, &[("src/worker.rs", "fn process_item() {}")]);
        assert_eq!(out_missing_sym.violations.len(), 1);
        assert_eq!(
            out_missing_sym.violations[0].code,
            "mechanism-sections/evidence-target-not-found"
        );
    }
}
