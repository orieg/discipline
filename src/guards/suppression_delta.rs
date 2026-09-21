//! Suppression delta sentinel (`suppression-delta`).
//!
//! A change must not add compiler, type-checker or linter suppressions (`#[allow]`,
//! `@ts-ignore`, `# noqa`, `NOLINT`, `@SuppressWarnings`, ...) without saying why.
//!
//! The sites come from the language packs (`ParsedFileFacts::escape_hatches`), so a
//! marker inside a string or an ordinary comment is not one, and the count is a delta:
//! the head side of each changed file is compared with its base side, and a site that
//! merely moved is not new. `unsafe` sites are left to `unsafe-safety-comment` and
//! `unsafe-budget`.

use crate::ast::{default_registry, AssertVocabulary, EscapeHatchSite};
use crate::guards::{line_allows, Context, GateOutcome};
use crate::tokens::ALLOW_SUPPRESSION;
use anyhow::Result;
use globset::{Glob, GlobSetBuilder};
use std::collections::HashMap;

pub const GATE: &str = "suppression-delta";

/// A suppression site as this gate sees it: what kind, which rule, and the text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Site {
    pub line: usize,
    /// `type-ignore` or `linter-disable`.
    pub kind: &'static str,
    pub rule: String,
    pub snippet: String,
}

impl Site {
    /// Identity across a move: kind, rule and whitespace-normalised text, not the line.
    fn signature(&self) -> (&'static str, String, String) {
        (
            self.kind,
            self.rule.trim().to_string(),
            self.snippet
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

pub fn sites_of(hatches: &[EscapeHatchSite]) -> Vec<Site> {
    hatches
        .iter()
        .filter_map(|h| match h {
            EscapeHatchSite::TypeIgnore {
                line,
                tool,
                snippet,
            } => Some(Site {
                line: *line,
                kind: "type-ignore",
                rule: tool.clone(),
                snippet: snippet.clone(),
            }),
            EscapeHatchSite::LinterDisable {
                line,
                rule,
                snippet,
            } => Some(Site {
                line: *line,
                kind: "linter-disable",
                rule: rule.clone(),
                snippet: snippet.clone(),
            }),
            EscapeHatchSite::UnsafeBlock { .. } => None,
        })
        .collect()
}

/// Sites on the head side that the base side does not have (as a multiset). A site the
/// base had at another line, or the same site repeated as often as before, is not new.
pub fn new_sites(base: &[Site], head: &[Site]) -> Vec<Site> {
    let mut budget: HashMap<_, usize> = HashMap::new();
    for s in base {
        *budget.entry(s.signature()).or_default() += 1;
    }
    let mut new = Vec::new();
    for s in head {
        match budget.get_mut(&s.signature()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => new.push(s.clone()),
        }
    }
    new
}

/// The pattern name `extract_suppression_rules` keys on, from the site's text.
fn pattern_of(site: &Site) -> String {
    let t = site.snippet.trim();
    let starts = |p: &str| t.starts_with(p);
    if starts("#[allow(") || starts("#![allow(") {
        "#[allow(".into()
    } else if starts("#[expect(") || starts("#![expect(") {
        "#[expect(".into()
    } else if t.contains("# noqa") {
        "# noqa".into()
    } else if t.contains("type: ignore") {
        "# type: ignore".into()
    } else if t.contains("pylint: disable") {
        "# pylint: disable".into()
    } else if t.contains("@ts-nocheck") {
        "// @ts-nocheck".into()
    } else if t.contains("@ts-expect-error") {
        "// @ts-expect-error".into()
    } else if t.contains("@ts-ignore") {
        "// @ts-ignore".into()
    } else if t.contains("eslint-disable") {
        "eslint-disable".into()
    } else if t.contains("NOLINT") {
        "// NOLINT".into()
    } else if t.contains("nolint") || t.contains("lint:ignore") {
        "//nolint".into()
    } else if t.contains("pragma warning disable") {
        "#pragma warning disable".into()
    } else {
        site.rule.clone()
    }
}

pub fn evaluate_suppression_delta(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.suppression_delta;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let changed = ctx.git.changed_files()?;
    if changed.is_empty() {
        return Ok(out);
    }

    let mut exempt_builder = GlobSetBuilder::new();
    for pat in &settings.exempt_paths {
        if let Ok(g) = Glob::new(pat) {
            exempt_builder.add(g);
        }
    }
    let exempt_set = exempt_builder
        .build()
        .unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap());

    let registry = default_registry();
    let vocab = AssertVocabulary::default();
    // (path, site, pattern name)
    let mut detected: Vec<(String, Site, String)> = Vec::new();

    for file in &changed {
        if file.is_deleted() || exempt_set.is_match(&file.path) {
            continue;
        }
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head_facts = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(f) => f,
            Err(e) => {
                out.notes.push(format!(
                    "`{}`: not analysed, the head side does not parse ({e})",
                    file.path
                ));
                continue;
            }
        };
        let base_sites = match ctx.git.base_content(&file.old_path)? {
            Some(base_src) => match pack.extract(&file.old_path, &base_src, &vocab) {
                Ok(f) => sites_of(&f.escape_hatches),
                Err(_) => {
                    out.notes.push(format!(
                        "`{}`: the base side does not parse; every head-side suppression is judged as new",
                        file.path
                    ));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let head_sites = sites_of(&head_facts.escape_hatches);
        out.examined += head_sites.len();

        for site in new_sites(&base_sites, &head_sites) {
            let line_text = head_src
                .lines()
                .nth(site.line.saturating_sub(1))
                .unwrap_or("");
            if line_allows(line_text, GATE) {
                out.overrides.push(crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    subject: format!("{}:{}", file.path, site.line),
                    directive: format!("discipline:allow({GATE})"),
                    reason: "inline exemption marker".to_string(),
                    source: crate::tokens::OverrideSource::Inline {
                        file: file.path.clone(),
                        line: site.line,
                    },
                    hidden: true,
                });
                continue;
            }
            if settings
                .allowed_suppressions
                .iter()
                .any(|a| site.snippet.contains(a.as_str()))
            {
                continue;
            }
            let pat = pattern_of(&site);
            detected.push((file.path.clone(), site, pat));
        }
    }

    let mut unwaived = Vec::new();
    for (path, site, pat) in &detected {
        let mut candidate_subjects = extract_suppression_rules(&site.snippet, pat);
        if !site.rule.is_empty() {
            candidate_subjects.push(site.rule.clone());
        }
        candidate_subjects.push(path.clone());
        if let Some(file_name) = path.rsplit('/').next() {
            if file_name != path {
                candidate_subjects.push(file_name.to_string());
            }
        }

        let mut applied: Option<(crate::tokens::OverrideRecord, String)> = None;
        for subj in &candidate_subjects {
            if let Some(ov) = ctx.find_override(GATE, ALLOW_SUPPRESSION, subj) {
                applied = Some((ov, subj.clone()));
                break;
            }
        }

        if let Some((ov, matched_subj)) = applied {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for suppression `{}` in `{}:{}` ({})",
                ov.directive, ov.reason, matched_subj, path, site.line, ov.source
            ));
        } else {
            unwaived.push((path, site, pat));
        }
    }

    if unwaived.len() > settings.max_increase {
        for (path, site, pat) in &unwaived {
            out.add_violation(
                ctx.overridable(settings.severity),
                path,
                site.line,
                format!("new {} suppression `{pat}` introduced without override", site.kind),
                format!(
                    "line contains suppression annotation `{}`: `{}`; use `discipline:allow(suppression-delta): <rule-or-path> <reason>` to waive",
                    pat, site.snippet
                ),
            );
        }
    }

    Ok(out)
}

pub fn extract_suppression_rules(snippet: &str, pat: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let trimmed = snippet.trim();

    if pat.contains("allow(") || pat.contains("expect(") {
        if let Some(start) = trimmed.find('(') {
            if let Some(end) = trimmed[start + 1..].find(')') {
                let inner = &trimmed[start + 1..start + 1 + end];
                for part in inner.split(',') {
                    let r = part.trim().trim_matches(['"', '\'']);
                    if !r.is_empty() {
                        rules.push(r.to_string());
                        if let Some(leaf) = r.rsplit("::").next() {
                            if leaf != r && !leaf.is_empty() {
                                rules.push(leaf.to_string());
                            }
                        }
                    }
                }
            }
        }
    } else if pat == "# noqa" {
        rules.push("noqa".to_string());
        if let Some(pos) = trimmed.find("# noqa:") {
            let after = &trimmed[pos + 8..];
            for part in after.split(|c: char| c == ',' || c.is_whitespace()) {
                let r = part.trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    } else if pat == "# type: ignore" {
        rules.push("type: ignore".to_string());
        rules.push("type-ignore".to_string());
        rules.push("type_ignore".to_string());
        rules.push("ignore".to_string());
        if let Some(pos) = trimmed.find("# type: ignore[") {
            if let Some(end) = trimmed[pos + 15..].find(']') {
                let inner = &trimmed[pos + 15..pos + 15 + end];
                for part in inner.split(',') {
                    let r = part.trim();
                    if !r.is_empty() {
                        rules.push(r.to_string());
                    }
                }
            }
        }
    } else if pat == "# pylint: disable" {
        rules.push("pylint".to_string());
        if let Some(pos) = trimmed.find("disable=") {
            let after = &trimmed[pos + 8..];
            for part in after.split(',') {
                let r = part.trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    } else if pat.starts_with("// @ts-") || pat.starts_with("@ts-") {
        let tag = pat.trim_start_matches("//").trim().trim_start_matches('@');
        rules.push(format!("@{tag}"));
        rules.push(tag.to_string());
        rules.push(tag.replace('-', "_"));
    } else if pat.contains("eslint-disable") {
        rules.push("eslint-disable".to_string());
        rules.push("eslint".to_string());
        let after = if let Some(p) = trimmed.find("eslint-disable-next-line") {
            &trimmed[p + 24..]
        } else if let Some(p) = trimmed.find("eslint-disable") {
            &trimmed[p + 14..]
        } else {
            ""
        };
        let cleaned = after.trim_end_matches("*/").trim();
        for part in cleaned.split(|c: char| c == ',' || c.is_whitespace()) {
            let r = part.trim();
            if !r.is_empty() {
                rules.push(r.to_string());
            }
        }
    } else if pat.to_ascii_uppercase().contains("NOLINT") {
        rules.push("NOLINT".to_string());
        rules.push("nolint".to_string());
        if let Some(start) = trimmed.find('(') {
            if let Some(end) = trimmed[start + 1..].find(')') {
                let inner = trimmed[start + 1..start + 1 + end].trim();
                if !inner.is_empty() {
                    rules.push(inner.to_string());
                }
            }
        }
    } else if pat.contains("nolint") || pat.contains("lint:ignore") {
        rules.push("nolint".to_string());
        rules.push("lint:ignore".to_string());
        rules.push("ignore".to_string());
        if let Some(pos) = trimmed.find("//nolint:") {
            let after = &trimmed[pos + 9..];
            for part in after.split(',') {
                let r = part.trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    } else if pat.contains("pragma warning disable") {
        rules.push("pragma".to_string());
        if let Some(pos) = trimmed.find("disable") {
            let after = &trimmed[pos + 7..];
            for part in after.split_whitespace() {
                let r = part.trim_end_matches(';').trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    }

    rules
}

#[cfg(test)]
mod tests {
    use crate::config::{Severity, SuppressionDeltaGate};

    #[test]
    fn test_suppression_delta_defaults() {
        let gate = SuppressionDeltaGate::default();
        assert!(gate.enabled);
        // Non-blocking by default: `#[allow]` / `# noqa` are routine reviewed
        // escape hatches, so the population in an unknown repository is high.
        assert_eq!(gate.severity, Severity::Warning);
        assert_eq!(gate.max_increase, 0);
    }

    #[test]
    fn a_moved_or_repeated_site_is_not_new_and_an_added_one_is() {
        use super::{new_sites, Site};
        let site = |line: usize, rule: &str| Site {
            line,
            kind: "linter-disable",
            rule: rule.into(),
            snippet: format!("#[allow({rule})]"),
        };
        let base = vec![site(3, "dead_code"), site(9, "unused")];
        let moved = vec![site(30, "unused"), site(12, "dead_code")];
        assert!(new_sites(&base, &moved).is_empty());
        let grown = vec![site(3, "dead_code"), site(9, "unused"), site(20, "unused")];
        assert_eq!(new_sites(&base, &grown), vec![site(20, "unused")]);
        let swapped = vec![site(3, "dead_code"), site(9, "clippy::all")];
        assert_eq!(new_sites(&base, &swapped), vec![site(9, "clippy::all")]);
        assert_eq!(new_sites(&[], &base).len(), 2);
    }

    #[test]
    fn test_extract_suppression_rules() {
        use super::extract_suppression_rules;

        let rust_rules = extract_suppression_rules("#[allow(dead_code, clippy::all)]", "#[allow(");
        assert!(rust_rules.contains(&"dead_code".to_string()));
        assert!(rust_rules.contains(&"clippy::all".to_string()));
        assert!(rust_rules.contains(&"all".to_string()));

        let py_noqa = extract_suppression_rules("x = 1 # noqa: E501", "# noqa");
        assert!(py_noqa.contains(&"noqa".to_string()));
        assert!(py_noqa.contains(&"E501".to_string()));

        let py_type =
            extract_suppression_rules("y = 2 # type: ignore[attr-defined]", "# type: ignore");
        assert!(py_type.contains(&"type: ignore".to_string()));
        assert!(py_type.contains(&"attr-defined".to_string()));

        let ts_rules = extract_suppression_rules("// @ts-ignore", "// @ts-ignore");
        assert!(ts_rules.contains(&"ts-ignore".to_string()));
        assert!(ts_rules.contains(&"@ts-ignore".to_string()));

        let cpp_rules = extract_suppression_rules("// NOLINT(readability-something)", "// NOLINT");
        assert!(cpp_rules.contains(&"NOLINT".to_string()));
        assert!(cpp_rules.contains(&"readability-something".to_string()));
    }
}
