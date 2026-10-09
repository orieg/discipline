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
//! - Fuzz targets are not removed from harness manifests (`fuzz/Cargo.toml` and Rust files
//!   under `fuzz/`, plus the manifests and harness files `fuzz_targets` globs name elsewhere)
//! - Seed corpus directories do not shrink without a directive (`fuzz/corpus/**`, `corpus/**`)

use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashSet};
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
///
/// An error is why the file's budgets could not be read: the pack could not parse it.
fn read_ast_budgets(content: &str, path: &str) -> Result<Vec<BudgetMetric>> {
    let reg = crate::ast::default_registry();
    let Some(pack) = reg.find_pack(path) else {
        return Ok(Vec::new());
    };
    if !pack.supplies(crate::ast::Fact::Budgets) {
        return Ok(Vec::new());
    }
    let facts = pack.extract(path, content, &crate::ast::AssertVocabulary::default())?;
    Ok(facts
        .budgets
        .into_iter()
        .map(|b| BudgetMetric {
            subject: b.subject.to_string(),
            value: b.value,
            raw: b.value.to_string(),
            path: path.to_string(),
            line: b.line,
        })
        .collect())
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

/// Extracts fuzz target names from `fuzz/Cargo.toml` or similar manifests. A manifest
/// that does not parse gives none: a caller that must tell the two apart uses
/// [`read_fuzz_manifest_targets`].
pub fn extract_fuzz_manifest_targets(content: &str) -> HashSet<String> {
    read_fuzz_manifest_targets(content, "")
        .map(|targets| targets.into_iter().collect())
        .unwrap_or_default()
}

/// The fuzz targets a fuzz manifest lists, in name order: the `name` of every entry of
/// its `bin` array, read from the parsed TOML. `[[bin]]` tables and an inline
/// `bin = [{ name = ".." }]` array are the same value, the key may stand anywhere in its
/// entry, and text that spells an entry in a comment or inside a string is not one.
///
/// An error is why the targets could not be read: the manifest is not TOML. It names the
/// path and the line, never the file's text.
pub fn read_fuzz_manifest_targets(content: &str, path: &str) -> Result<BTreeSet<String>> {
    let manifest: toml::Value = toml::from_str(content).map_err(|e| {
        let at = e
            .span()
            .map(|s| {
                let before = &content.as_bytes()[..s.start.min(content.len())];
                let line = before.iter().filter(|b| **b == b'\n').count() + 1;
                format!(" (line {line})")
            })
            .unwrap_or_default();
        anyhow::anyhow!("`{path}` does not parse as TOML{at}")
    })?;
    Ok(manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bin| bin.get("name").and_then(toml::Value::as_str))
        .map(str::to_string)
        .collect())
}

/// Whether `path` is a fuzz crate's manifest: the root crate's `fuzz/Cargo.toml`, or a
/// `Cargo.toml` outside `fuzz/` that a `fuzz_targets` glob names. Inside `fuzz/` the
/// built-in rule decides alone, so the default list reports what the gate reported before
/// the list was read.
fn is_fuzz_manifest(fuzz: &PathFilter, path: &str) -> bool {
    if path.starts_with("fuzz/") {
        return path == "fuzz/Cargo.toml";
    }
    Path::new(path).file_name().and_then(|n| n.to_str()) == Some("Cargo.toml") && fuzz.matches(path)
}

/// Whether `path` is a fuzz harness source: a Rust file under the root `fuzz/`, or one
/// that a `fuzz_targets` glob names.
fn is_fuzz_harness(fuzz: &PathFilter, path: &str) -> bool {
    path.ends_with(".rs") && (path.starts_with("fuzz/") || fuzz.matches(path))
}

/// Extracts budgets for any supported file type based on extension / path.
pub fn extract_budgets_for_file(content: &str, path: &str) -> Vec<BudgetMetric> {
    read_budgets_for_file(content, path).unwrap_or_default()
}

/// [`extract_budgets_for_file`], or why the file's budgets could not be read.
fn read_budgets_for_file(content: &str, path: &str) -> Result<Vec<BudgetMetric>> {
    let p = Path::new(path);
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    let file_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");

    match ext {
        "rs" | "py" | "pyi" | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            read_ast_budgets(content, path)
        }
        "sh" | "bash" | "yml" | "yaml" => Ok(extract_script_and_workflow_budgets(content, path)),
        _ if file_name == ".gitlab-ci.yml" || path.contains(".github/workflows/") => {
            Ok(extract_script_and_workflow_budgets(content, path))
        }
        _ => Ok(Vec::new()),
    }
}

/// Evaluates the `test-budget` gate.
pub fn evaluate_test_budget(ctx: &Context) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::new(GATE);
    let gate = &ctx.config.gates.test_budget;

    // Build exemption matcher
    let exempt = PathFilter::new(&gate.exempt_paths)?;

    // Build corpus directory matcher (`**/x` already matches a bare `x`, so no
    // stripped duplicate is added).
    let corpus = PathFilter::new(&gate.corpus_dirs)?;

    // `fuzz_targets` names fuzz manifests and harness files beside the built-in `fuzz/`
    // crate, which is watched whatever the list says.
    let fuzz = PathFilter::new(&gate.fuzz_targets)?;

    let changed = ctx.git.changed_files()?;
    let mut examined_count = 0usize;

    // Deleted seed files by corpus directory. Every deleted seed counts: seeds the change
    // adds to the same directory do not offset it.
    let mut deleted_seeds: BTreeMap<String, usize> = BTreeMap::new();

    for f in &changed {
        if exempt.matches(&f.path) {
            continue;
        }

        examined_count += 1;

        // Check if this file is in a seed corpus directory
        if f.kind == ChangeKind::Deleted && (corpus.matches(&f.path) || corpus.matches(&f.old_path))
        {
            let old_parent = Path::new(&f.old_path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            *deleted_seeds.entry(old_parent).or_insert(0) += 1;
        }

        // Check fuzz target manifest deletion (e.g. fuzz/Cargo.toml)
        let is_fuzz_manifest =
            is_fuzz_manifest(&fuzz, &f.path) || is_fuzz_manifest(&fuzz, &f.old_path);
        if is_fuzz_manifest {
            let base_content = ctx.git.base_content(&f.old_path)?.unwrap_or_default();
            let head_content = ctx.git.head_content(&f.path)?.unwrap_or_default();
            // A head manifest that does not parse is not one that lists no target, or
            // every target: what the change leaves cannot be read, so the gate cannot
            // answer.
            let head_targets = read_fuzz_manifest_targets(&head_content, &f.path)
                .map_err(|e| anyhow::anyhow!("{e}; its fuzz targets could not be read"))?;
            // A base manifest that does not parse gives nothing to compare with. The
            // change may be the one that repairs it: said, not stopped.
            let base_targets = match read_fuzz_manifest_targets(&base_content, &f.old_path) {
                Ok(targets) => targets,
                Err(e) => {
                    outcome.notes.push(format!(
                        "{e} on the base side, so removed fuzz targets were not looked for"
                    ));
                    BTreeSet::new()
                }
            };

            // In name order, so the findings come in the same order on every run.
            let base_in_order = &base_targets;
            for target in base_in_order {
                if !head_targets.contains(target) {
                    // Fuzz target removed from harness list
                    let subject = format!("fuzz target {target}");
                    let rec = ctx
                        .find_override(
                            GATE,
                            &crate::findings::FUZZ_TARGET_REMOVED,
                            tokens::ALLOW_TEST_SHRINK,
                            &subject,
                        )
                        .or_else(|| {
                            ctx.find_override(
                                GATE,
                                &crate::findings::FUZZ_TARGET_REMOVED,
                                tokens::ALLOW_TEST_SHRINK,
                                target,
                            )
                        });
                    let anchored = rec.is_none();
                    outcome.lift_or_push(
                        rec,
                        gate.severity,
                        &crate::findings::FUZZ_TARGET_REMOVED,
                        (Some(&f.path), None),
                        format!(
                            "Fuzz target `{target}` was removed from fuzz harness `{}` without an explicit override.",
                            f.path
                        ),
                        &format!(
                            "Restore fuzz target `{target}` or justify removal with `allow-test-shrink: {target} <reason>`."
                        ),
                    );
                    if anchored {
                        // One harness lists many targets: the target tells them apart.
                        outcome.anchor_last(format!("fuzz-target:{target}"));
                    }
                }
            }
        }

        // Check if individual fuzz target file was deleted (e.g. fuzz/fuzz_targets/*.rs)
        if f.kind == ChangeKind::Deleted && is_fuzz_harness(&fuzz, &f.old_path) {
            let target_name = Path::new(&f.old_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&f.old_path);
            let subject = format!("fuzz target {target_name}");
            let rec = ctx
                .find_override(
                    GATE,
                    &crate::findings::FUZZ_TARGET_DELETED,
                    tokens::ALLOW_TEST_SHRINK,
                    &subject,
                )
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::FUZZ_TARGET_DELETED,
                        tokens::ALLOW_TEST_SHRINK,
                        target_name,
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::FUZZ_TARGET_DELETED,
                        tokens::ALLOW_TEST_SHRINK,
                        &f.old_path,
                    )
                });
            outcome.lift_or_push(
                rec,
                gate.severity,
                &crate::findings::FUZZ_TARGET_DELETED,
                (Some(&f.old_path), None),
                format!(
                    "Fuzz target file `{}` was deleted without an explicit override.",
                    f.old_path
                ),
                &format!(
                    "Restore fuzz target `{}` or justify deletion with `allow-test-shrink: {target_name} <reason>`.",
                    f.old_path
                ),
            );
        }

        // Compare property test and fuzz flags between base and head
        let base_raw = ctx.git.base_content(&f.old_path)?;
        let head_raw = ctx.git.head_content(&f.path)?;

        let (Some(base_content), Some(head_content)) = (base_raw, head_raw) else {
            continue;
        };

        // A side the pack could not parse has no budgets to compare: said, not passed.
        let (base_metrics, head_metrics) = match (
            read_budgets_for_file(&base_content, &f.old_path),
            read_budgets_for_file(&head_content, &f.path),
        ) {
            (Ok(base), Ok(head)) => (base, head),
            (Err(e), _) | (_, Err(e)) => {
                outcome
                    .notes
                    .push(format!("{e}, so its test budgets were not compared"));
                continue;
            }
        };

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

                let rec = ctx
                    .find_override(
                        GATE,
                        &crate::findings::TEST_BUDGET_DECREASED,
                        tokens::ALLOW_TEST_SHRINK,
                        subject_key,
                    )
                    .or_else(|| {
                        ctx.find_override(
                            GATE,
                            &crate::findings::TEST_BUDGET_DECREASED,
                            tokens::ALLOW_TEST_SHRINK,
                            path_key,
                        )
                    })
                    .or_else(|| {
                        ctx.find_override(
                            GATE,
                            &crate::findings::TEST_BUDGET_DECREASED,
                            tokens::ALLOW_TEST_SHRINK,
                            file_stem,
                        )
                    });
                let line = matching_head.map(|h| h.line).or(Some(base_m.line));
                outcome.lift_or_push(
                    rec,
                    gate.severity,
                    &crate::findings::TEST_BUDGET_DECREASED,
                    (Some(&f.path), line),
                    format!(
                        "Testing effort `{}` in `{}` reduced from {} to {}.",
                        base_m.subject, f.path, base_m.value, head_val_str
                    ),
                    &format!(
                        "Restore testing effort to at least {} or justify with `allow-test-shrink: {} <reason>`.",
                        base_m.value, base_m.subject
                    ),
                );
            }
        }
    }

    // Check seed corpus deletions
    // In path order, so the findings come in the same order on every run.
    for (dir, deleted_count) in deleted_seeds {
        if deleted_count > 0 {
            let dir_subject = format!("corpus {dir}");
            let dir_stem = Path::new(&dir)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&dir);

            let rec = ctx
                .find_override(
                    GATE,
                    &crate::findings::SEED_CORPUS_DECREASED,
                    tokens::ALLOW_TEST_SHRINK,
                    &dir_subject,
                )
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::SEED_CORPUS_DECREASED,
                        tokens::ALLOW_TEST_SHRINK,
                        &dir,
                    )
                })
                .or_else(|| {
                    ctx.find_override(
                        GATE,
                        &crate::findings::SEED_CORPUS_DECREASED,
                        tokens::ALLOW_TEST_SHRINK,
                        dir_stem,
                    )
                });
            outcome.lift_or_push(
                rec,
                gate.severity,
                &crate::findings::SEED_CORPUS_DECREASED,
                (Some(&dir), None),
                format!(
                    "Seed corpus directory `{dir}` lost {deleted_count} seed file(s) without an explicit override."
                ),
                &format!(
                    "Restore corpus seeds or justify reduction with `allow-test-shrink: {dir_stem} <reason>`."
                ),
            );
        }
    }

    outcome.examined = examined_count;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use globset::{Glob, GlobSetBuilder};

    #[test]
    fn leading_double_star_already_matches_a_bare_name() {
        // Pins why no stripped `strip_prefix("**/")` copy is added: globset's
        // `**/` matches zero or more directories, so the stripped copy is redundant.
        let mut only_star = GlobSetBuilder::new();
        only_star.add(Glob::new("**/tests/corpus/**").unwrap());
        let only_star = only_star.build().unwrap();
        assert!(only_star.is_match("tests/corpus/seed.bin"));
        assert!(only_star.is_match("a/tests/corpus/seed.bin"));

        let mut with_stripped = GlobSetBuilder::new();
        with_stripped.add(Glob::new("**/tests/corpus/**").unwrap());
        with_stripped.add(Glob::new("tests/corpus/**").unwrap());
        let with_stripped = with_stripped.build().unwrap();
        for path in [
            "tests/corpus/seed.bin",
            "a/tests/corpus/seed.bin",
            "fuzz/corpus/seed.bin",
            "src/lib.rs",
        ] {
            assert_eq!(
                only_star.is_match(path),
                with_stripped.is_match(path),
                "mismatch on {path}"
            );
        }
    }

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
    #[test]
    fn fuzz_targets_come_from_the_parsed_manifest() {
        let read = |text: &str| -> Vec<String> {
            read_fuzz_manifest_targets(text, "fuzz/Cargo.toml")
                .unwrap()
                .into_iter()
                .collect()
        };
        // The key anywhere in its entry, a header with a comment, a literal string.
        assert_eq!(
            read("[[bin]] # first\npath = \"a.rs\"\nname = \"a\"\n\n[[bin]]\nname = 'b'\n"),
            ["a", "b"]
        );
        // An inline array is the same value.
        assert_eq!(
            read("bin = [{ name = \"a\" }, { path = \"b.rs\", name = \"b\" }]\n"),
            ["a", "b"]
        );
        // Text that spells an entry is not one: a comment, a string, another table's key.
        assert_eq!(
            read("# [[bin]] name = \"c\"\n[package]\nname = \"p\"\ndescription = \"\"\"\n[[bin]]\nname = \"d\"\n\"\"\"\n\n[[example]]\nname = \"e\"\n"),
            [] as [&str; 0]
        );
        // An entry with no name, or a name that is not a string, names no target.
        assert_eq!(
            read("[[bin]]\npath = \"a.rs\"\n\n[[bin]]\nname = 3\n"),
            [] as [&str; 0]
        );
        assert_eq!(read(""), [] as [&str; 0]);
    }

    #[test]
    fn a_manifest_that_does_not_parse_is_an_error_that_names_the_line_only() {
        let text = "[[bin]]\nname = \"a\"\n\n[features]\nsecret-word = [\n";
        let shown = format!(
            "{:#}",
            read_fuzz_manifest_targets(text, "fuzz/Cargo.toml").unwrap_err()
        );
        assert!(
            shown.starts_with("`fuzz/Cargo.toml` does not parse as TOML (line "),
            "{shown}"
        );
        assert!(!shown.contains("secret-word"), "{shown}");
        // The form that returns a set gives none for such a file.
        assert!(extract_fuzz_manifest_targets(text).is_empty());
    }

    /// What the gate matched before `fuzz_targets` was read: the root manifest, and a
    /// Rust file anywhere under `fuzz/`.
    fn literal_manifest(path: &str) -> bool {
        path == "fuzz/Cargo.toml"
    }

    fn literal_harness(path: &str) -> bool {
        (path.starts_with("fuzz/fuzz_targets/") || path.starts_with("fuzz/"))
            && path.ends_with(".rs")
    }

    const FUZZ_PATHS: &[&str] = &[
        "fuzz/Cargo.toml",
        "fuzz/fuzz_targets/parse.rs",
        "fuzz/fuzz_targets/nested/encode.rs",
        "fuzz/fuzz_targets/Cargo.toml",
        "fuzz/fuzz_targets/README.md",
        "fuzz/src/support.rs",
        "fuzz/build.rs",
        "fuzz/afl/Cargo.toml",
        "fuzz/corpus/parse/seed.bin",
        "fuzz.rs",
        "Cargo.toml",
        "src/fuzz/Cargo.toml",
        "src/fuzz/fuzz_targets/parse.rs",
        "crates/core/fuzz/Cargo.toml",
        "crates/core/fuzz/fuzz_targets/parse.rs",
        "crates/core/fuzz/fuzz_targets/notes.md",
        "crates/core/src/lib.rs",
        "afuzz/Cargo.toml",
        "afuzz/fuzz_targets/parse.rs",
    ];

    /// With the default `fuzz_targets` the gate matches exactly the paths it matched
    /// when the list was not read.
    #[test]
    fn the_default_fuzz_targets_match_what_the_literal_prefixes_matched() {
        let defaults = crate::config::TestBudgetGate::default().fuzz_targets;
        assert_eq!(defaults, ["fuzz/Cargo.toml", "fuzz/fuzz_targets/**"]);
        let fuzz = PathFilter::new(&defaults).unwrap();
        for path in FUZZ_PATHS {
            assert_eq!(
                is_fuzz_manifest(&fuzz, path),
                literal_manifest(path),
                "manifest: {path}"
            );
            assert_eq!(
                is_fuzz_harness(&fuzz, path),
                literal_harness(path),
                "harness: {path}"
            );
        }
        // The sample has paths on both sides of each rule.
        assert!(FUZZ_PATHS.iter().any(|p| literal_harness(p)));
        assert!(FUZZ_PATHS.iter().any(|p| !literal_harness(p)));
    }

    /// A configured glob adds the fuzz crate it names; the root `fuzz/` crate stays.
    #[test]
    fn a_fuzz_targets_glob_names_a_crate_outside_the_root_fuzz_directory() {
        let fuzz = PathFilter::new(&[
            "crates/*/fuzz/Cargo.toml".to_string(),
            "crates/*/fuzz/fuzz_targets/**".to_string(),
        ])
        .unwrap();
        assert!(is_fuzz_manifest(&fuzz, "crates/core/fuzz/Cargo.toml"));
        assert!(is_fuzz_harness(
            &fuzz,
            "crates/core/fuzz/fuzz_targets/parse.rs"
        ));
        // Named by the glob, but neither a manifest nor a Rust harness.
        assert!(!is_fuzz_manifest(
            &fuzz,
            "crates/core/fuzz/fuzz_targets/parse.rs"
        ));
        assert!(!is_fuzz_harness(
            &fuzz,
            "crates/core/fuzz/fuzz_targets/notes.md"
        ));
        assert!(!is_fuzz_harness(&fuzz, "crates/core/src/lib.rs"));
        assert!(is_fuzz_manifest(&fuzz, "fuzz/Cargo.toml"));
        assert!(is_fuzz_harness(&fuzz, "fuzz/src/support.rs"));
        let none = PathFilter::new(&[]).unwrap();
        assert!(!is_fuzz_manifest(&none, "crates/core/fuzz/Cargo.toml"));
        assert!(is_fuzz_harness(&none, "fuzz/fuzz_targets/parse.rs"));
    }
}
