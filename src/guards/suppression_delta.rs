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

    if detected_suppressions.len() > settings.max_increase {
        let net = detected_suppressions.len();
        if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_SUPPRESSION, GATE) {
            out.notes.push(format!(
                "override applied: `{}: {}` (net increase of {} suppressions permitted) ({})",
                ov.directive, ov.reason, net, ov.source
            ));
        } else {
            for (path, lno, lang, pat, snippet) in &detected_suppressions {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    path,
                    *lno,
                    format!("new {lang} suppression `{pat}` introduced without override"),
                    format!(
                        "line contains suppression annotation `{}`: `{}`; use `discipline:allow(suppression-delta): <reason>` to waive",
                        pat, snippet
                    ),
                );
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::config::{Severity, SuppressionDeltaGate};

    #[test]
    fn test_suppression_delta_defaults() {
        let gate = SuppressionDeltaGate::default();
        assert!(gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(gate.max_increase, 0);
    }
}
