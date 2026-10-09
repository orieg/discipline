//! Mechanism sections sentinel (`mechanism-sections`).
//!
//! Requires `(inferred)` or `(verified: <target>)` evidence tags in root cause and mechanism sections.

use crate::guards::{Context, GateOutcome, PathFilter};
use anyhow::Result;

pub const GATE: &str = "mechanism-sections";

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

/// Parses designated sections from a markdown document.
pub fn parse_designated_sections(content: &str, headings: &[String]) -> Vec<DesignatedSection> {
    let mut sections = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    let mut current_section: Option<DesignatedSection> = None;
    let mut current_level = 0;
    let mut current_paragraph: Option<(usize, Vec<String>)> = None;
    let mut in_code_fence = false;

    let flush_paragraph = |para: &mut Option<(usize, Vec<String>)>,
                           claims: &mut Vec<SectionClaim>| {
        if let Some((line_num, lines)) = para.take() {
            let text = lines.join(" ").trim().to_string();
            if !text.is_empty() {
                claims.push(SectionClaim { line_num, text });
            }
        }
    };

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }

        // Check if line is a markdown heading: #, ##, ###, etc.
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let title = trimmed[level..].trim();

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
            let is_match = headings.iter().any(|h| h.eq_ignore_ascii_case(title));

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

fn target_exists(target: &str, ctx: &Context) -> Result<bool> {
    let clean = target
        .trim()
        .trim_matches(|c| matches!(c, '`' | '"' | '\'' | '(' | ')'));
    let file_part = clean.split("::").next().unwrap_or(clean);

    if ctx.git.head_bytes(file_part)?.is_some() || ctx.git.root().join(file_part).exists() {
        return Ok(true);
    }
    if ctx.git.head_bytes(clean)?.is_some() || ctx.git.root().join(clean).exists() {
        return Ok(true);
    }
    Ok(false)
}

fn truncate_claim(text: &str) -> String {
    if text.len() > 60 {
        format!("{}...", &text[..57])
    } else {
        text.to_string()
    }
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

    let inferred_re = regex::Regex::new(r"(?i)\(inferred\)").unwrap();
    let verified_re = regex::Regex::new(r"(?i)\(verified:\s*([^)]+)\)").unwrap();

    for path in files_to_check {
        if exempt.matches(&path) {
            continue;
        }

        let Some(content) = ctx.git.head_content(&path)? else {
            continue;
        };

        let sections = parse_designated_sections(&content, &settings.section_headings);
        designated_sections_found += sections.len();

        for section in sections {
            if section.claims.is_empty() {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    &crate::findings::EMPTY_DESIGNATED_SECTION,
                    &path,
                    section.heading_line,
                    format!("designated section `{}` is empty", section.heading_title),
                    "provide substantive mechanism claims with evidence tags",
                );
                continue;
            }

            for claim in section.claims {
                out.examined += 1;

                let has_inferred = inferred_re.is_match(&claim.text);
                let verified_caps: Vec<_> = verified_re.captures_iter(&claim.text).collect();

                if !has_inferred && verified_caps.is_empty() {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        &crate::findings::UNTAGGED_MECHANISM_CLAIM,
                        &path,
                        claim.line_num,
                        format!(
                            "claim in `{}` section lacks evidence tag: \"{}\"",
                            section.heading_title,
                            truncate_claim(&claim.text)
                        ),
                        "append `(inferred)` or `(verified: <test or artifact>)` to the claim",
                    );
                    continue;
                }

                for cap in verified_caps {
                    if let Some(target_match) = cap.get(1) {
                        let target = target_match.as_str().trim();
                        if !target_exists(target, ctx)? {
                            out.add_violation(
                                ctx.overridable(settings.severity),
                                &crate::findings::EVIDENCE_TARGET_NOT_FOUND,
                                &path,
                                claim.line_num,
                                format!(
                                    "verification target `{target}` cited in `{}` was not found in repository",
                                    section.heading_title
                                ),
                                "cite an existing test file, test identifier, or tracked artifact",
                            );
                        }
                    }
                }
            }
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
}
