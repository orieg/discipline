//! Universal property-test and fuzz effort ratchet sentinel (`test-budget`).
//!
//! Enforces:
//! - Property-testing budgets are not reduced without justification:
//!   - Rust `proptest`: `ProptestConfig { cases, max_shrink_iters, .. }`, `ProptestConfig::with_cases(..)`
//!   - Rust `quickcheck`: `QuickCheck::new().tests(..)`, `.gen_size(..)`, `quickcheck(tests = ..)`
//!   - Python `hypothesis`: `@settings(max_examples=.., deadline=..)`, `settings(...)`
//!   - JS/TS `fast-check`: `fc.assert(..., { numRuns: .. })`
//! - Fuzzing duration, runs, and flags in workflows/scripts are not lowered:
//!   - `PROPTEST_CASES`
//!   - `-max_total_time`
//!   - `-runs`
//!   - Go fuzz `-fuzztime`
//! - Fuzz targets are not removed from harness manifests (`fuzz/Cargo.toml`, `fuzz/fuzz_targets/`)
//! - Seed corpus directories do not shrink without a directive (`fuzz/corpus/**`, `corpus/**`)

use crate::config::GateSettings;
use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome, Violation};
use crate::tokens;
use anyhow::Result;
use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const GATE: &str = "test-budget";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetMetric {
    pub subject: String,
    pub value: u64,
    pub raw: String,
    pub path: String,
    pub line: usize,
}

/// Parses a duration string (e.g. "10m", "300s", "1h", "1000x", "500") into a comparable integer.
pub fn parse_duration_or_count(s: &str) -> Option<u64> {
    let trimmed = s.trim().trim_matches(|c| c == '\'' || c == '"');
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_suffix("ms") {
        rest.trim().parse::<u64>().ok()
    } else if let Some(rest) = trimmed
        .strip_suffix('h')
        .or_else(|| trimmed.strip_suffix("hr"))
    {
        rest.trim().parse::<u64>().ok().map(|h| h * 3600)
    } else if let Some(rest) = trimmed
        .strip_suffix('m')
        .or_else(|| trimmed.strip_suffix("min"))
    {
        rest.trim().parse::<u64>().ok().map(|m| m * 60)
    } else if let Some(rest) = trimmed
        .strip_suffix('s')
        .or_else(|| trimmed.strip_suffix("sec"))
    {
        rest.trim().parse::<u64>().ok()
    } else if let Some(rest) = trimmed.strip_suffix('x') {
        rest.trim().parse::<u64>().ok()
    } else {
        trimmed.parse::<u64>().ok()
    }
}

/// Budgets a language pack reads from the syntax tree (`Fact::Budgets`): a named
/// integer in a configuration position. The same word in a string or a comment, or an
/// unrelated assignment (`min_tests = 40`), is not one.
pub fn extract_ast_budgets(content: &str, path: &str) -> Vec<BudgetMetric> {
    let reg = crate::ast::default_registry();
    let Some(pack) = reg.find_pack(path) else {
        return Vec::new();
    };
    if !pack.supplies(crate::ast::Fact::Budgets) {
        return Vec::new();
    }
    let Ok(facts) = pack.extract(path, content, &crate::ast::AssertVocabulary::default()) else {
        return Vec::new();
    };
    facts
        .budgets
        .into_iter()
        .map(|b| BudgetMetric {
            subject: b.subject.to_string(),
            value: b.value,
            raw: b.value.to_string(),
            path: path.to_string(),
            line: b.line,
        })
        .collect()
}

/// Extracts workflow and script flags (`PROPTEST_CASES`, `-max_total_time`, `-runs`, `-fuzztime`).
pub fn extract_script_and_workflow_budgets(content: &str, path: &str) -> Vec<BudgetMetric> {
    let mut metrics = Vec::new();

    let re_proptest_cases = Regex::new(r"PROPTEST_CASES\s*[:=]\s*(\d+)").unwrap();
    let re_max_time = Regex::new(r"-max_total_time[=\s]+(\d+)").unwrap();
    let re_runs = Regex::new(r"-runs[=\s]+(\d+)").unwrap();
    let re_fuzztime = Regex::new(r"-fuzztime[=\s]+([0-9a-zA-Z]+)").unwrap();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let line_str = line.trim();
        if line_str.starts_with('#') {
            continue;
        }

        if let Some(caps) = re_proptest_cases.captures(line) {
            if let Some(m) = caps.get(1) {
                if let Ok(val) = m.as_str().parse::<u64>() {
                    metrics.push(BudgetMetric {
                        subject: "PROPTEST_CASES".to_string(),
                        value: val,
                        raw: m.as_str().to_string(),
                        path: path.to_string(),
                        line: line_num,
                    });
                }
            }
        }

        if let Some(caps) = re_max_time.captures(line) {
            if let Some(m) = caps.get(1) {
                if let Ok(val) = m.as_str().parse::<u64>() {
                    metrics.push(BudgetMetric {
                        subject: "fuzz -max_total_time".to_string(),
                        value: val,
                        raw: m.as_str().to_string(),
                        path: path.to_string(),
                        line: line_num,
                    });
                }
            }
        }

        if let Some(caps) = re_runs.captures(line) {
            if let Some(m) = caps.get(1) {
                if let Ok(val) = m.as_str().parse::<u64>() {
                    metrics.push(BudgetMetric {
                        subject: "fuzz -runs".to_string(),
                        value: val,
                        raw: m.as_str().to_string(),
                        path: path.to_string(),
                        line: line_num,
                    });
                }
            }
        }

        if let Some(caps) = re_fuzztime.captures(line) {
            if let Some(m) = caps.get(1) {
                if let Some(val) = parse_duration_or_count(m.as_str()) {
                    metrics.push(BudgetMetric {
                        subject: "go fuzz -fuzztime".to_string(),
                        value: val,
                        raw: m.as_str().to_string(),
                        path: path.to_string(),
                        line: line_num,
                    });
                }
            }
        }
    }

    metrics
}

/// Extracts fuzz target names from `fuzz/Cargo.toml` or similar manifests.
pub fn extract_fuzz_manifest_targets(content: &str) -> HashSet<String> {
    let mut targets = HashSet::new();
    let re_bin = Regex::new(r#"\[\[bin\]\]\s*name\s*=\s*"([^"]+)""#).unwrap();
    for caps in re_bin.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            targets.insert(m.as_str().to_string());
        }
    }
    targets
}

/// Extracts budgets for any supported file type based on extension / path.
pub fn extract_budgets_for_file(content: &str, path: &str) -> Vec<BudgetMetric> {
    let p = Path::new(path);
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    let file_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");

    match ext {
        "rs" | "py" | "pyi" | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            extract_ast_budgets(content, path)
        }
        "sh" | "bash" | "yml" | "yaml" => extract_script_and_workflow_budgets(content, path),
        _ if file_name == ".gitlab-ci.yml" || path.contains(".github/workflows/") => {
            extract_script_and_workflow_budgets(content, path)
        }
        _ => Vec::new(),
    }
}

/// Evaluates the `test-budget` gate.
pub fn evaluate_test_budget(ctx: &Context) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::new(GATE);
    let gate = &ctx.config.gates.test_budget;

    if !gate.enabled() {
        outcome.enabled = false;
        return Ok(outcome);
    }

    // Build exemption matcher
    let mut ex_builder = GlobSetBuilder::new();
    for pattern in &gate.exempt_paths {
        ex_builder.add(Glob::new(pattern)?);
    }
    let ex_matcher = ex_builder.build()?;

    // Build corpus directory matcher
    let mut corpus_builder = GlobSetBuilder::new();
    for pattern in &gate.corpus_dirs {
        corpus_builder.add(Glob::new(pattern)?);
        if let Some(stripped) = pattern.strip_prefix("**/") {
            corpus_builder.add(Glob::new(stripped)?);
        }
    }
    let corpus_matcher = corpus_builder.build()?;

    let changed = ctx.git.changed_files()?;
    let mut examined_count = 0usize;

    // Track corpus files by directory to detect corpus shrink
    let mut base_corpus_counts: HashMap<String, usize> = HashMap::new();
    let mut head_corpus_counts: HashMap<String, usize> = HashMap::new();

    for f in &changed {
        if ex_matcher.is_match(&f.path) {
            continue;
        }

        examined_count += 1;

        // Check if this file is in a seed corpus directory
        if corpus_matcher.is_match(&f.path) || corpus_matcher.is_match(&f.old_path) {
            let parent = Path::new(&f.path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            let old_parent = Path::new(&f.old_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();

            if f.kind == ChangeKind::Deleted {
                *base_corpus_counts.entry(old_parent.clone()).or_insert(0) += 1;
            } else if f.kind == ChangeKind::Added {
                *head_corpus_counts.entry(parent.clone()).or_insert(0) += 1;
            }
        }

        // Check fuzz target manifest deletion (e.g. fuzz/Cargo.toml)
        let is_fuzz_manifest = f.path == "fuzz/Cargo.toml" || f.old_path == "fuzz/Cargo.toml";
        if is_fuzz_manifest {
            let base_content = ctx.git.base_content(&f.old_path)?.unwrap_or_default();
            let head_content = ctx.git.head_content(&f.path)?.unwrap_or_default();
            let base_targets = extract_fuzz_manifest_targets(&base_content);
            let head_targets = extract_fuzz_manifest_targets(&head_content);

            for target in &base_targets {
                if !head_targets.contains(target) {
                    // Fuzz target removed from harness list
                    let subject = format!("fuzz target {target}");
                    if let Some(rec) = ctx
                        .find_override(GATE, tokens::ALLOW_TEST_SHRINK, &subject)
                        .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, target))
                    {
                        outcome.overrides.push(rec);
                    } else {
                        outcome.violations.push(Violation {
                            gate: GATE,
                            code: crate::findings::full_code(GATE, &crate::findings::FUZZ_TARGET_REMOVED),
                            fingerprint: String::new(),
                            title: crate::findings::FUZZ_TARGET_REMOVED.title.to_string(),
                            legacy_title: crate::findings::FUZZ_TARGET_REMOVED.was_title(),
                            message: format!(
                                "Fuzz target `{target}` was removed from fuzz harness `{}` without an explicit override.",
                                f.path
                            ),
                            file: Some(f.path.clone()),
                            line: None,
                            remediation: Some(format!(
                                "Restore fuzz target `{target}` or justify removal with `allow-test-shrink: {target} <reason>`."
                            )),
                            severity: gate.severity,
                        });
                    }
                }
            }
        }

        // Check if individual fuzz target file was deleted (e.g. fuzz/fuzz_targets/*.rs)
        if (f.old_path.starts_with("fuzz/fuzz_targets/") || f.old_path.starts_with("fuzz/"))
            && f.kind == ChangeKind::Deleted
            && f.old_path.ends_with(".rs")
        {
            let target_name = Path::new(&f.old_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&f.old_path);
            let subject = format!("fuzz target {target_name}");
            if let Some(rec) = ctx
                .find_override(GATE, tokens::ALLOW_TEST_SHRINK, &subject)
                .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, target_name))
                .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, &f.old_path))
            {
                outcome.overrides.push(rec);
            } else {
                outcome.violations.push(Violation {
                    gate: GATE,
                    code: crate::findings::full_code(GATE, &crate::findings::FUZZ_TARGET_DELETED),
                    fingerprint: String::new(),
                    title: crate::findings::FUZZ_TARGET_DELETED.title.to_string(),
                    legacy_title: crate::findings::FUZZ_TARGET_DELETED.was_title(),
                    message: format!(
                        "Fuzz target file `{}` was deleted without an explicit override.",
                        f.old_path
                    ),
                    file: Some(f.old_path.clone()),
                    line: None,
                    remediation: Some(format!(
                        "Restore fuzz target `{}` or justify deletion with `allow-test-shrink: {target_name} <reason>`.",
                        f.old_path
                    )),
                    severity: gate.severity,
                });
            }
        }

        // Compare property test and fuzz flags between base and head
        let base_raw = ctx.git.base_content(&f.old_path)?;
        let head_raw = ctx.git.head_content(&f.path)?;

        let (Some(base_content), Some(head_content)) = (base_raw, head_raw) else {
            continue;
        };

        let base_metrics = extract_budgets_for_file(&base_content, &f.old_path);
        let head_metrics = extract_budgets_for_file(&head_content, &f.path);

        if base_metrics.is_empty() {
            continue;
        }

        // Compare matching metrics by subject
        // For files with multiple metrics of same subject, group or match in sequence
        for base_m in &base_metrics {
            // Find head metrics with same subject
            let matching_head = head_metrics.iter().find(|h| h.subject == base_m.subject);

            let is_drop = match matching_head {
                Some(head_m) => head_m.value < base_m.value,
                None => true, // Metric removed completely (fallback to lower default)
            };

            if is_drop {
                let head_val_str = match matching_head {
                    Some(h) => format!("{}", h.value),
                    None => "removed (default)".to_string(),
                };

                let subject_key = &base_m.subject;
                let path_key = &f.path;
                let file_stem = Path::new(&f.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&f.path);

                if let Some(rec) = ctx
                    .find_override(GATE, tokens::ALLOW_TEST_SHRINK, subject_key)
                    .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, path_key))
                    .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, file_stem))
                {
                    outcome.overrides.push(rec);
                } else {
                    outcome.violations.push(Violation {
                        gate: GATE,
                        code: crate::findings::full_code(GATE, &crate::findings::TEST_BUDGET_DECREASED),
                        fingerprint: String::new(),
                        title: crate::findings::TEST_BUDGET_DECREASED.title.to_string(),
                        legacy_title: crate::findings::TEST_BUDGET_DECREASED.was_title(),
                        message: format!(
                            "Testing effort `{}` in `{}` reduced from {} to {}.",
                            base_m.subject, f.path, base_m.value, head_val_str
                        ),
                        file: Some(f.path.clone()),
                        line: matching_head.map(|h| h.line).or(Some(base_m.line)),
                        remediation: Some(format!(
                            "Restore testing effort to at least {} or justify with `allow-test-shrink: {} <reason>`.",
                            base_m.value, base_m.subject
                        )),
                        severity: gate.severity,
                    });
                }
            }
        }
    }

    // Check seed corpus deletions
    for (dir, deleted_count) in base_corpus_counts {
        if deleted_count > 0 {
            let dir_subject = format!("corpus {dir}");
            let dir_stem = Path::new(&dir)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&dir);

            if let Some(rec) = ctx
                .find_override(GATE, tokens::ALLOW_TEST_SHRINK, &dir_subject)
                .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, &dir))
                .or_else(|| ctx.find_override(GATE, tokens::ALLOW_TEST_SHRINK, dir_stem))
            {
                outcome.overrides.push(rec);
            } else {
                outcome.violations.push(Violation {
                    gate: GATE,
                    code: crate::findings::full_code(GATE, &crate::findings::SEED_CORPUS_DECREASED),
                    fingerprint: String::new(),
                    title: crate::findings::SEED_CORPUS_DECREASED.title.to_string(),
                    legacy_title: crate::findings::SEED_CORPUS_DECREASED.was_title(),
                    message: format!(
                        "Seed corpus directory `{dir}` lost {deleted_count} seed file(s) without an explicit override."
                    ),
                    file: Some(dir.clone()),
                    line: None,
                    remediation: Some(format!(
                        "Restore corpus seeds or justify reduction with `allow-test-shrink: {dir_stem} <reason>`."
                    )),
                    severity: gate.severity,
                });
            }
        }
    }

    outcome.examined = examined_count;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_or_count() {
        assert_eq!(parse_duration_or_count("10m"), Some(600));
        assert_eq!(parse_duration_or_count("2h"), Some(7200));
        assert_eq!(parse_duration_or_count("45s"), Some(45));
        assert_eq!(parse_duration_or_count("1000x"), Some(1000));
        assert_eq!(parse_duration_or_count("500"), Some(500));
        assert_eq!(parse_duration_or_count("500ms"), Some(500));
        assert_eq!(parse_duration_or_count(""), None);
    }

    #[test]
    fn rust_budgets_come_from_the_tree_not_from_lines() {
        let sample = "fn setup() {\n    let config = ProptestConfig {\n        cases: 5000,\n        max_shrink_iters: 2000,\n        ..Default::default()\n    };\n    let config2 = ProptestConfig::with_cases(1000);\n    QuickCheck::new().tests(250).gen_size(50).quickcheck(test_fn as fn(u32) -> bool);\n}\nproptest! {\n    #![proptest_config(ProptestConfig { cases: 400, .. ProptestConfig::default() })]\n    fn p(x in any::<u32>()) { prop_assert!(x >= 0); }\n}\n";
        let metrics = extract_budgets_for_file(sample, "tests/property.rs");
        let rows: Vec<(&str, u64)> = metrics
            .iter()
            .map(|m| (m.subject.as_str(), m.value))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("proptest cases", 5000),
                ("proptest max_shrink_iters", 2000),
                ("proptest cases", 1000),
                ("quickcheck tests", 250),
                ("quickcheck gen_size", 50),
                ("proptest cases", 400),
            ]
        );

        // A test-floor setting quoted in a fixture, a comment, and a bare assignment are
        // not budgets: the line patterns this replaced read all three.
        let none = extract_budgets_for_file(
            "fn f() {\n    let cfg = \"min_tests = 40\";\n    // cases: 9 once\n    let tests = 300;\n}\n",
            "tests/e2e.rs",
        );
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn python_and_javascript_budgets_come_from_the_tree() {
        let py = extract_budgets_for_file(
            "@settings(max_examples=2000, deadline=500)\ndef test_property():\n    note = 'max_examples=1'\n",
            "tests/test_hypo.py",
        );
        let rows: Vec<(&str, u64)> = py.iter().map(|m| (m.subject.as_str(), m.value)).collect();
        assert_eq!(
            rows,
            vec![
                ("hypothesis max_examples", 2000),
                ("hypothesis deadline", 500)
            ]
        );
        let js = extract_budgets_for_file(
            "fc.assert(\n  fc.property(fc.integer(), (n) => n === n),\n  { numRuns: 1000 }\n);\n// numRuns: 7\n",
            "tests/fc.test.ts",
        );
        assert_eq!(js.len(), 1, "{js:?}");
        assert_eq!(js[0].subject, "fast-check numRuns");
        assert_eq!(js[0].value, 1000);
        assert_eq!(js[0].line, 3);
    }

    #[test]
    fn test_extract_script_and_workflow_budgets() {
        let sample = r#"
            env:
              PROPTEST_CASES: 10000
            run: |
              cargo fuzz run target -max_total_time 3600 -runs 500000
              go test -fuzz=FuzzSearch -fuzztime=10m
        "#;
        let metrics = extract_script_and_workflow_budgets(sample, ".github/workflows/fuzz.yml");
        assert_eq!(metrics.len(), 4);

        let prop = metrics
            .iter()
            .find(|m| m.subject == "PROPTEST_CASES")
            .unwrap();
        assert_eq!(prop.value, 10000);

        let max_time = metrics
            .iter()
            .find(|m| m.subject == "fuzz -max_total_time")
            .unwrap();
        assert_eq!(max_time.value, 3600);

        let runs = metrics.iter().find(|m| m.subject == "fuzz -runs").unwrap();
        assert_eq!(runs.value, 500000);

        let fuzztime = metrics
            .iter()
            .find(|m| m.subject == "go fuzz -fuzztime")
            .unwrap();
        assert_eq!(fuzztime.value, 600); // 10m -> 600s
    }

    #[test]
    fn test_extract_fuzz_manifest_targets() {
        let sample = r#"
            [package]
            name = "project-fuzz"
            version = "0.0.0"

            [[bin]]
            name = "parse_target"
            path = "fuzz_targets/parse_target.rs"

            [[bin]]
            name = "encode_target"
            path = "fuzz_targets/encode_target.rs"
        "#;
        let targets = extract_fuzz_manifest_targets(sample);
        assert_eq!(targets.len(), 2);
        assert!(targets.contains("parse_target"));
        assert!(targets.contains("encode_target"));
    }
}
