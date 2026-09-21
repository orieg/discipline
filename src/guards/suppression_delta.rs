//! Suppression delta sentinel (`suppression-delta`).
//!
//! Prevents quality degradation from agents introducing new compiler, type checker,
//! or linter suppression attributes (e.g. `#[allow]`, `@ts-ignore`, `# noqa`).

use crate::ast::{language_for, Language};
use crate::guards::{line_allows, Context, GateOutcome};
use crate::tokens::ALLOW_SUPPRESSION;
use anyhow::Result;
use globset::{Glob, GlobSetBuilder};
use std::fs;

pub const GATE: &str = "suppression-delta";

fn matches_language(lang: Language, pattern_lang: &str) -> bool {
    matches!(
        (lang, pattern_lang),
        (Language::Rust, "rust")
            | (Language::Python, "python")
            | (
                Language::JavaScript | Language::TypeScript,
                "javascript" | "typescript"
            )
            | (Language::C | Language::Cpp, "c/c++")
            | (Language::Go, "go")
            | (Language::CSharp, "c#")
    )
}

fn line_matches_suppression(lang: Language, trimmed: &str, pattern: &str) -> bool {
    match lang {
        Language::Rust => {
            (trimmed.starts_with("#[allow(")
                || trimmed.starts_with("#![allow(")
                || trimmed.starts_with("#[expect(")
                || trimmed.starts_with("#![expect("))
                && trimmed.contains(pattern)
        }
        _ => trimmed.contains(pattern),
    }
}

struct SuppressionPattern {
    pattern: &'static str,
    language: &'static str,
}

static SUPPRESSION_PATTERNS: &[SuppressionPattern] = &[
    SuppressionPattern {
        pattern: "#[allow(",
        language: "rust",
    },
    SuppressionPattern {
        pattern: "#[expect(",
        language: "rust",
    },
    SuppressionPattern {
        pattern: "# noqa",
        language: "python",
    },
    SuppressionPattern {
        pattern: "# type: ignore",
        language: "python",
    },
    SuppressionPattern {
        pattern: "# pylint: disable",
        language: "python",
    },
    SuppressionPattern {
        pattern: "// @ts-ignore",
        language: "typescript",
    },
    SuppressionPattern {
        pattern: "// @ts-nocheck",
        language: "typescript",
    },
    SuppressionPattern {
        pattern: "/* eslint-disable",
        language: "javascript",
    },
    SuppressionPattern {
        pattern: "// eslint-disable-next-line",
        language: "javascript",
    },
    SuppressionPattern {
        pattern: "// NOLINT",
        language: "c/c++",
    },
    SuppressionPattern {
        pattern: "// NOLINTNEXTLINE",
        language: "c/c++",
    },
    SuppressionPattern {
        pattern: "//nolint",
        language: "go",
    },
    SuppressionPattern {
        pattern: "//lint:ignore",
        language: "go",
    },
    SuppressionPattern {
        pattern: "#pragma warning disable",
        language: "c#",
    },
];

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

    let root = ctx.git.root();
    let mut detected_suppressions: Vec<(String, usize, &'static str, String, String)> = Vec::new();
    let mut total_added_lines = 0;

    for file in &changed {
        if file.is_deleted() || exempt_set.is_match(&file.path) {
            continue;
        }

        let Some(lang) = language_for(&file.path) else {
            continue;
        };

        let full_path = root.join(&file.path);
        if !full_path.is_file() {
            continue;
        }

        let content = match fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue, // binary or unreadable file
        };

        for (lineno, line) in content.lines().enumerate() {
            let line_idx = lineno + 1;
            // Only inspect newly added lines in the diff
            if !file.added_lines.contains(&line_idx) {
                continue;
            }
            total_added_lines += 1;

            if line_allows(line, GATE) {
                out.overrides.push(crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    subject: format!("{}:{line_idx}", file.path),
                    directive: format!("discipline:allow({GATE})"),
                    reason: "inline exemption marker".to_string(),
                    source: crate::tokens::OverrideSource::Inline {
                        file: file.path.clone(),
                        line: line_idx,
                    },
                    hidden: true,
                });
                continue;
            }

            let trimmed = line.trim();
            for sp in SUPPRESSION_PATTERNS {
                if !matches_language(lang, sp.language) {
                    continue;
                }
                if line_matches_suppression(lang, trimmed, sp.pattern) {
                    // Check if specifically allowed by configuration
                    let is_allowed = settings
                        .allowed_suppressions
                        .iter()
                        .any(|a| trimmed.contains(a));
                    if !is_allowed {
                        detected_suppressions.push((
                            file.path.clone(),
                            line_idx,
                            sp.language,
                            sp.pattern.to_string(),
                            line.trim().to_string(),
                        ));
                    }
                }
            }
        }
    }

    out.examined = total_added_lines;

    let mut unwaived = Vec::new();
    for (path, lno, lang, pat, snippet) in &detected_suppressions {
        let mut candidate_subjects = extract_suppression_rules(snippet, pat);
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
                ov.directive, ov.reason, matched_subj, path, lno, ov.source
            ));
        } else {
            unwaived.push((path, lno, lang, pat, snippet));
        }
    }

    if unwaived.len() > settings.max_increase {
        for (path, lno, lang, pat, snippet) in &unwaived {
            out.add_violation(
                ctx.overridable(settings.severity),
                path,
                **lno,
                format!("new {lang} suppression `{pat}` introduced without override"),
                format!(
                    "line contains suppression annotation `{}`: `{}`; use `discipline:allow(suppression-delta): <rule-or-path> <reason>` to waive",
                    pat, snippet
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
