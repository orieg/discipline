//! Citation anchors sentinel (`citation-anchors`).
//!
//! Verifies `path:line@sha "<text>"` citations against quoted line content and git history.

use crate::config::UnanchoredCitationsMode;
use crate::gitctx::CommitLookup;
use crate::guards::{Context, GateOutcome, PathFilter};
use anyhow::Result;

pub const GATE: &str = "citation-anchors";

#[derive(Debug, Clone)]
pub struct AnchoredCitation {
    pub line_num: usize,
    pub path: String,
    pub line: usize,
    pub commit: String,
    pub quoted_text: String,
}

#[derive(Debug, Clone)]
pub struct UnanchoredCitation {
    pub line_num: usize,
    pub path: String,
    pub line: usize,
}

fn is_plausible_citation_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.contains(' ')
        || path.contains('\t')
    {
        return false;
    }
    let has_slash = path.contains('/');
    let has_ext = path.rsplit('.').next().is_some_and(|ext| {
        !ext.is_empty()
            && ext.len() <= 6
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && ext != path
    });
    has_slash || has_ext
}

pub fn extract_anchored_citations(content: &str) -> Vec<AnchoredCitation> {
    let mut citations = Vec::new();
    let re =
        regex::Regex::new(r#"([a-zA-Z0-9_./\-]+):(\d+)@([0-9a-fA-F]{4,40})\s+["']([^"']+)["']"#)
            .unwrap();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        for caps in re.captures_iter(line) {
            let path = caps.get(1).unwrap().as_str();
            if !is_plausible_citation_path(path) {
                continue;
            }
            let line_val: usize = match caps.get(2).unwrap().as_str().parse() {
                Ok(v) if v > 0 => v,
                _ => continue,
            };
            let commit = caps.get(3).unwrap().as_str().to_string();
            let quoted_text = caps.get(4).unwrap().as_str().to_string();

            citations.push(AnchoredCitation {
                line_num,
                path: path.to_string(),
                line: line_val,
                commit,
                quoted_text,
            });
        }
    }

    citations
}

pub fn extract_unanchored_citations(content: &str) -> Vec<UnanchoredCitation> {
    let mut citations = Vec::new();
    let re = regex::Regex::new(r#"(?:^|[\s`"'(\[])([a-zA-Z0-9_./\-]+):(\d+)"#).unwrap();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        for caps in re.captures_iter(line) {
            let path = caps.get(1).unwrap().as_str();
            if !is_plausible_citation_path(path) {
                continue;
            }
            let line_match = caps.get(2).unwrap();
            let after_match = &line[line_match.end()..];
            if after_match.starts_with('@') {
                continue;
            }
            let line_val: usize = match line_match.as_str().parse() {
                Ok(v) if v > 0 => v,
                _ => continue,
            };

            citations.push(UnanchoredCitation {
                line_num,
                path: path.to_string(),
                line: line_val,
            });
        }
    }

    citations
}

/// Evaluates citation anchors across inspected documents.
pub fn evaluate_citation_anchors(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.citation_anchors;
    let mut out = GateOutcome::new(GATE);

    let files_to_check = if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        changed
            .into_iter()
            .filter(|f| !f.is_deleted())
            .map(|f| f.path)
            .collect::<Vec<_>>()
    } else {
        ctx.git.tracked_files()?
    };

    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let tracker_re = if settings.verify_tracker_titles {
        Some(regex::Regex::new(r#"#(\d+)\s+\("([^"]+)"\)"#).unwrap())
    } else {
        None
    };

    for path in files_to_check {
        if exempt.matches(&path) {
            continue;
        }

        let Some(content) = ctx.git.head_content(&path)? else {
            continue;
        };

        // 1. Anchored citations
        let anchored = extract_anchored_citations(&content);
        for cit in anchored {
            out.examined += 1;

            // Commit lookup
            match ctx.git.lookup_commit(&cit.commit) {
                Ok(CommitLookup::Commit(full_oid)) => {
                    match ctx.git.commit_content(&full_oid, &cit.path)? {
                        None => {
                            out.add_violation(
                                ctx.overridable(settings.severity),
                                &crate::findings::FILE_NOT_FOUND,
                                &path,
                                cit.line_num,
                                format!(
                                    "file `{}` cited at commit `{}` was not found",
                                    cit.path, cit.commit
                                ),
                                "verify path exists in the cited commit tree",
                            );
                        }
                        Some(target_content) => {
                            let target_lines: Vec<&str> = target_content.lines().collect();
                            if cit.line < 1 || cit.line > target_lines.len() {
                                out.add_violation(
                                    ctx.overridable(settings.severity),
                                    &crate::findings::LINE_OUT_OF_BOUNDS,
                                    &path,
                                    cit.line_num,
                                    format!(
                                        "cited line {} is out of bounds in `{}` (file has {} lines)",
                                        cit.line,
                                        cit.path,
                                        target_lines.len()
                                    ),
                                    "update citation line number to match cited commit source",
                                );
                            } else {
                                let actual = target_lines[cit.line - 1].trim_end();
                                let expected = cit.quoted_text.trim();
                                if !actual.contains(expected) {
                                    out.add_violation(
                                        ctx.overridable(settings.severity),
                                        &crate::findings::TEXT_MISMATCH,
                                        &path,
                                        cit.line_num,
                                        format!(
                                            "citation `{}:{}` text mismatch at commit `{}`: expected \"{}\", found \"{}\"",
                                            cit.path,
                                            cit.line,
                                            cit.commit,
                                            expected,
                                            actual.trim()
                                        ),
                                        "update citation anchor line or commit to match repository source",
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        &crate::findings::COMMIT_UNRESOLVABLE,
                        &path,
                        cit.line_num,
                        format!(
                            "commit `{}` in citation `{}:{}` could not be resolved in repository history",
                            cit.commit, cit.path, cit.line
                        ),
                        "ensure cited commit is fetched or update anchor to a reachable commit",
                    );
                }
            }
        }

        // 2. Unanchored citations
        if settings.unanchored_citations != UnanchoredCitationsMode::Ignore {
            let unanchored = extract_unanchored_citations(&content);
            for cit in unanchored {
                out.examined += 1;
                let sev = match settings.unanchored_citations {
                    UnanchoredCitationsMode::Warn => crate::config::Severity::Warning,
                    UnanchoredCitationsMode::Reject => ctx.overridable(settings.severity),
                    UnanchoredCitationsMode::Ignore => unreachable!(),
                };
                out.add_violation(
                    sev,
                    &crate::findings::UNANCHORED_CITATION,
                    &path,
                    cit.line_num,
                    format!("unanchored citation `{}:{}`", cit.path, cit.line),
                    "anchor citation with commit sha and quoted text: `path:line@commit \"<quoted line>\"`",
                );
            }
        }

        // 3. Optional tracker titles
        if let Some(ref re) = tracker_re {
            for caps in re.captures_iter(&content) {
                let num = caps.get(1).unwrap().as_str();
                let no_network = std::env::var("DISCIPLINE_NO_NETWORK").is_ok();
                if no_network || ctx.forge.is_none() {
                    if settings.require_online {
                        return Err(anyhow::anyhow!(
                            "forge connection required to verify tracker titles but network is disabled or forge unavailable"
                        ));
                    } else {
                        out.notes.push(format!(
                            "tracker reference #{num} title check skipped (offline/no network)"
                        ));
                        out.examined += 1;
                    }
                }
            }
        }
    }

    if out.examined == 0 {
        out.notes
            .push("no citation anchors found to verify".to_string());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CitationAnchorsGate, DisciplineConfig, Severity, UnanchoredCitationsMode};

    #[test]
    fn test_citation_anchors_defaults() {
        let gate = CitationAnchorsGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(gate.diff_only);
        assert_eq!(gate.unanchored_citations, UnanchoredCitationsMode::Warn);
        assert!(!gate.verify_tracker_titles);
        assert!(!gate.require_online);
    }

    #[test]
    fn test_anchored_citation_lifecycle() {
        // Base commit has src/service.rs with:
        // line 1: pub fn init() {}
        // line 2: pub fn invalidate_cache() {
        // line 3: }
        let base_service = "pub fn init() {}\npub fn invalidate_cache() {\n}\n";
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(
            &[("src/service.rs", base_service), ("docs/spec.md", "")],
            &[],
        );
        let commit_hex = git.head_oid().unwrap();
        let short_sha = &commit_hex[..7];

        let config_toml = r#"
[meta]
version = 1
name = "t"
[gates.citation-anchors]
enabled = true
unanchored_citations = "ignore"
"#;
        let config = DisciplineConfig::from_toml_str(config_toml).unwrap();

        // 1. Valid citation matching line 2
        let valid_doc = format!("As defined in src/service.rs:2@{short_sha} \"pub fn invalidate_cache() {{\", it works.\n");
        let doc_path = git.root().join("docs/spec.md");
        std::fs::create_dir_all(doc_path.parent().unwrap()).unwrap();
        std::fs::write(&doc_path, &valid_doc).unwrap();

        let out = evaluate_citation_anchors(&crate::guards::test_support::context(&config, &git))
            .unwrap();
        assert!(out.violations.is_empty(), "{:?}", out.violations);

        // 2. Text mismatch
        let mismatch_doc = format!(
            "As defined in src/service.rs:2@{short_sha} \"non_existent_symbol\", it fails.\n"
        );
        std::fs::write(&doc_path, &mismatch_doc).unwrap();
        let out_mismatch =
            evaluate_citation_anchors(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert_eq!(out_mismatch.violations.len(), 1);
        assert_eq!(
            out_mismatch.violations[0].code,
            "citation-anchors/text-mismatch"
        );

        // 3. Unresolvable commit
        let unresolvable_doc =
            "src/service.rs:2@deadbeefdeadbeef \"pub fn invalidate_cache() {\"\n";
        std::fs::write(&doc_path, unresolvable_doc).unwrap();
        let out_unresolvable =
            evaluate_citation_anchors(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert_eq!(out_unresolvable.violations.len(), 1);
        assert_eq!(
            out_unresolvable.violations[0].code,
            "citation-anchors/commit-unresolvable"
        );
    }

    #[test]
    fn test_unanchored_citation_warn_and_reject() {
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "docs/spec.md",
            "",
            "As defined in src/service.rs:42, cache invalidation occurs immediately.\n",
        );

        // Warn by default
        let warn_toml = r#"
[meta]
version = 1
name = "t"
[gates.citation-anchors]
enabled = true
unanchored_citations = "warn"
"#;
        let warn_config = DisciplineConfig::from_toml_str(warn_toml).unwrap();
        let out_warn =
            evaluate_citation_anchors(&crate::guards::test_support::context(&warn_config, &git))
                .unwrap();
        assert_eq!(out_warn.violations.len(), 1);
        assert_eq!(
            out_warn.violations[0].code,
            "citation-anchors/unanchored-citation"
        );
        assert_eq!(out_warn.violations[0].severity, Severity::Warning);

        // Reject
        let reject_toml = r#"
[meta]
version = 1
name = "t"
[gates.citation-anchors]
enabled = true
unanchored_citations = "reject"
"#;
        let reject_config = DisciplineConfig::from_toml_str(reject_toml).unwrap();
        let out_reject =
            evaluate_citation_anchors(&crate::guards::test_support::context(&reject_config, &git))
                .unwrap();
        assert_eq!(out_reject.violations.len(), 1);
        assert_eq!(
            out_reject.violations[0].code,
            "citation-anchors/unanchored-citation"
        );
        assert_eq!(out_reject.violations[0].severity, Severity::Error);
    }
}
