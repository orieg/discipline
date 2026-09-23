//! Agent-guard gates that reason about the *change*: base vs head facts from
//! the tree-sitter extractor, plus deleted files.

use super::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::ast::{
    default_registry, is_unsupported_source_in, AssertVocabulary, ParsedFileFacts, TestFn,
};
use crate::config::GateSettings;
use crate::gitctx::{ChangeKind, ChangedFile};
use crate::tokens;
use anyhow::Result;

pub struct FileFacts {
    pub file: ChangedFile,
    pub base: Option<ParsedFileFacts>,
    pub head: Option<ParsedFileFacts>,
    pub newly_added_nul: bool,
}

pub struct TestPair<'a> {
    pub path: &'a str,
    pub base: &'a TestFn,
    pub head: &'a TestFn,
    pub forced: bool,
}

pub struct Located<'a> {
    pub path: &'a str,
    /// The file still exists on the head side.
    pub file_survives: bool,
    pub test: &'a TestFn,
}

/// The assertion vocabulary the agent-guard gates extract facts with.
pub(crate) fn assert_vocabulary(config: &crate::config::DisciplineConfig) -> AssertVocabulary {
    let gates = &config.gates;
    AssertVocabulary {
        extra_macros: [
            &gates.assertion_reduction.extra_assert_macros[..],
            &gates.vacuous_tests.extra_assert_macros[..],
        ]
        .concat(),
        helper_fns: [
            &gates.assertion_reduction.assert_helper_fns[..],
            &gates.vacuous_tests.assert_helper_fns[..],
        ]
        .concat(),
        safety_placeholders: gates.unsafe_safety_comment.placeholders.clone(),
        mock_setup_fns: [
            &gates.assertion_reduction.mock_setup_fns[..],
            &gates.vacuous_tests.mock_setup_fns[..],
        ]
        .concat(),
        mock_assert_fns: [
            &gates.assertion_reduction.mock_assert_fns[..],
            &gates.vacuous_tests.mock_assert_fns[..],
        ]
        .concat(),
        test_functions: config.tests.functions.clone(),
        test_paths: config.tests.paths.clone(),
    }
}

/// Runs every diff-based agent-guard gate and returns one outcome per gate.
/// Disabled gates are filtered by the caller; computing them is cheap.
pub fn run(ctx: &Context) -> Result<Vec<GateOutcome>> {
    let gates = &ctx.config.gates;
    let vocab = assert_vocabulary(ctx.config);

    let registry = default_registry();
    let changed = ctx.git.changed_files()?;
    let mut analyzed_files = Vec::new();
    for file in changed
        .iter()
        .filter(|f| registry.is_supported(&f.path) || registry.is_supported(&f.old_path))
    {
        let base_bytes = ctx.git.base_bytes(&file.old_path)?;
        let base_had_nul = base_bytes.as_ref().is_some_and(|b| b.contains(&0));
        let base = match base_bytes {
            Some(bytes) => {
                if let Some(pack) = registry.find_pack(&file.old_path) {
                    let src = String::from_utf8_lossy(&bytes);
                    Some(pack.extract(&file.old_path, &src, &vocab)?)
                } else {
                    None
                }
            }
            None => None,
        };
        let (head, newly_added_nul) = match file.kind {
            ChangeKind::Deleted => (None, false),
            _ => match ctx.git.head_bytes(&file.path)? {
                Some(bytes) => {
                    let has_nul = bytes.contains(&0);
                    let newly_added = has_nul && !base_had_nul;
                    if let Some(pack) = registry.find_pack(&file.path) {
                        let src = String::from_utf8_lossy(&bytes);
                        (Some(pack.extract(&file.path, &src, &vocab)?), newly_added)
                    } else {
                        (None, newly_added)
                    }
                }
                None => (None, false),
            },
        };
        analyzed_files.push(FileFacts {
            file: file.clone(),
            base,
            head,
            newly_added_nul,
        });
    }

    let (pairs, removed, added) = match_tests(&analyzed_files);

    let is_staged = ctx.staged && ctx.pr_body.is_none();
    let mut ast_gates = vec![
        evaluate_assertion_reduction(
            &pairs,
            &added,
            &gates.assertion_reduction,
            &ctx.directives,
            is_staged,
        )?,
        evaluate_vacuous_tests(&added, &gates.vacuous_tests)?,
        evaluate_ignored_tests(
            &pairs,
            &added,
            &gates.ignored_tests,
            &ctx.directives,
            is_staged,
        )?,
        evaluate_unsafe_safety_comment(&analyzed_files, &gates.unsafe_safety_comment)?,
    ];

    let first_enabled = [
        ("assertion-reduction", gates.assertion_reduction.enabled),
        ("vacuous-tests", gates.vacuous_tests.enabled),
        ("ignored-tests", gates.ignored_tests.enabled),
        ("unsafe-safety-comment", gates.unsafe_safety_comment.enabled),
    ]
    .into_iter()
    .find(|(_, on)| *on)
    .map(|(id, _)| id);

    if let Some(target) = first_enabled {
        if let Some(outcome) = ast_gates.iter_mut().find(|o| o.gate == target) {
            let target_settings = ctx.config.gates.settings(target);
            let sev = target_settings.map_or(crate::config::Severity::Error, |s| s.severity());
            let exempt = target_settings
                .map(exempt_filter)
                .transpose()?
                .unwrap_or_else(|| PathFilter::new(&[]).unwrap());
            report_parse_errors(&analyzed_files, sev, outcome, &exempt);
            report_newly_added_nul_bytes(
                &analyzed_files,
                sev,
                outcome,
                &ctx.directives,
                is_staged,
                &exempt,
            );
        }
    }

    fn format_unsupported_breakdown(paths: &[&str]) -> String {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for p in paths {
            let ext = p.rsplit('.').next().unwrap_or("");
            *counts.entry(ext).or_default() += 1;
        }
        let c_count = counts.remove("c").unwrap_or(0);
        let h_count = counts.remove("h").unwrap_or(0);
        let ch_count = c_count + h_count;

        let mut cpp_count = 0;
        for k in ["cpp", "cc", "cxx", "hpp", "hh"] {
            cpp_count += counts.remove(k).unwrap_or(0);
        }

        let mut groups: Vec<(String, usize)> = Vec::new();
        if ch_count > 0 {
            groups.push((".c/.h".to_string(), ch_count));
        }
        if cpp_count > 0 {
            groups.push((".cpp/.hpp".to_string(), cpp_count));
        }
        for (ext, count) in counts {
            groups.push((format!(".{ext}"), count));
        }
        groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        groups
            .into_iter()
            .map(|(ext, count)| format!("{count} {ext}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    // Named degradation: source files in a language with no extractor were
    // not analysed. Saying so keeps "0 violations" from reading as coverage.
    let unseen: Vec<&str> = changed
        .iter()
        .filter(|f| f.kind != ChangeKind::Deleted && is_unsupported_source_in(&f.path, &registry))
        .map(|f| f.path.as_str())
        .collect();
    if !unseen.is_empty() {
        let breakdown = format_unsupported_breakdown(&unseen);
        let sample = unseen
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let note = format!(
            "{} changed source file(s) are in a language with no extractor yet ({breakdown}) and were NOT \
             analysed by this gate (e.g. {sample})",
            unseen.len()
        );
        for gate in &mut ast_gates {
            gate.notes.push(note.clone());
        }
    }

    ast_gates.push(evaluate_deletion_rationale(
        &changed,
        &removed,
        &gates.deletion_rationale,
        &ctx.directives,
        is_staged,
    )?);
    Ok(ast_gates)
}

pub const RENAME_NAME_SIMILARITY_THRESHOLD: f64 = 0.5;

pub fn name_similarity(a: &str, b: &str) -> f64 {
    let a_leaf = a.rsplit("::").next().unwrap_or(a);
    let b_leaf = b.rsplit("::").next().unwrap_or(b);
    if a_leaf == b_leaf {
        return 1.0;
    }
    if a_leaf.is_empty() || b_leaf.is_empty() {
        return 0.0;
    }
    // Character bigram Sorensen-Dice similarity
    let bigrams = |s: &str| -> Vec<(char, char)> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 2 {
            return Vec::new();
        }
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let bg_a = bigrams(a_leaf);
    let bg_b = bigrams(b_leaf);
    let bigram_sim = if bg_a.is_empty() || bg_b.is_empty() {
        if a_leaf == b_leaf {
            1.0
        } else {
            0.0
        }
    } else {
        let mut matches = 0;
        let mut b_used = vec![false; bg_b.len()];
        for ga in &bg_a {
            for (i, gb) in bg_b.iter().enumerate() {
                if !b_used[i] && ga == gb {
                    b_used[i] = true;
                    matches += 1;
                    break;
                }
            }
        }
        (2.0 * matches as f64) / ((bg_a.len() + bg_b.len()) as f64)
    };

    // Word-level token Dice similarity
    let clean_a = a_leaf.strip_prefix("test_").unwrap_or(a_leaf);
    let clean_b = b_leaf.strip_prefix("test_").unwrap_or(b_leaf);
    let w_a: Vec<&str> = clean_a
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let w_b: Vec<&str> = clean_b
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let word_sim = if !w_a.is_empty() && !w_b.is_empty() {
        let mut w_matches = 0;
        let mut wb_used = vec![false; w_b.len()];
        for wa in &w_a {
            for (i, wb) in w_b.iter().enumerate() {
                if !wb_used[i] && wa == wb {
                    wb_used[i] = true;
                    w_matches += 1;
                    break;
                }
            }
        }
        (2.0 * w_matches as f64) / ((w_a.len() + w_b.len()) as f64)
    } else {
        0.0
    };

    bigram_sim.max(word_sim)
}

/// Pair tests by name within a file, then pair the leftovers across files so a
/// test moved to another file is compared instead of reported as removed+new.
pub fn match_tests(files: &[FileFacts]) -> (Vec<TestPair<'_>>, Vec<Located<'_>>, Vec<Located<'_>>) {
    let mut pairs = Vec::new();
    let mut removed: Vec<Located> = Vec::new();
    let mut added: Vec<Located> = Vec::new();

    struct FileUnmatched<'a> {
        path: &'a str,
        file_survives: bool,
        base: Vec<&'a TestFn>,
        head: Vec<(usize, &'a TestFn)>,
    }

    let mut file_unmatched: Vec<FileUnmatched> = Vec::new();

    for ff in files {
        let base_tests: &[TestFn] = ff.base.as_ref().map(|f| &f.tests[..]).unwrap_or(&[]);
        let head_tests: &[TestFn] = ff.head.as_ref().map(|f| &f.tests[..]).unwrap_or(&[]);
        let mut taken = vec![false; head_tests.len()];
        let mut unmatched_base = Vec::new();

        // 1. Exact name match within file
        for b in base_tests {
            let hit = head_tests
                .iter()
                .enumerate()
                .find(|(i, h)| !taken[*i] && h.name == b.name);
            match hit {
                Some((i, h)) => {
                    taken[i] = true;
                    pairs.push(TestPair {
                        path: &ff.file.path,
                        base: b,
                        head: h,
                        forced: false,
                    });
                }
                None => unmatched_base.push(b),
            }
        }

        // 2. Name similarity pairing within file for renames
        let mut unmatched_head: Vec<(usize, &TestFn)> = head_tests
            .iter()
            .enumerate()
            .filter(|(i, _)| !taken[*i])
            .collect();

        let mut still_unmatched_base = Vec::new();
        for b in unmatched_base {
            let best = unmatched_head
                .iter()
                .enumerate()
                .map(|(idx, &(orig_i, h))| (idx, orig_i, h, name_similarity(&b.name, &h.name)))
                .filter(|&(_, _, _, sim)| sim >= RENAME_NAME_SIMILARITY_THRESHOLD)
                .max_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));

            if let Some((idx, orig_i, h, _sim)) = best {
                taken[orig_i] = true;
                unmatched_head.remove(idx);
                pairs.push(TestPair {
                    path: &ff.file.path,
                    base: b,
                    head: h,
                    forced: false,
                });
            } else {
                still_unmatched_base.push(b);
            }
        }

        file_unmatched.push(FileUnmatched {
            path: &ff.file.path,
            file_survives: ff.head.is_some(),
            base: still_unmatched_base,
            head: unmatched_head,
        });
    }

    // 3. Exact leaf name match across files (e.g. test moved to another file)
    fn str_leaf(s: &str) -> &str {
        s.rsplit("::").next().unwrap_or(s)
    }
    for i in 0..file_unmatched.len() {
        let mut b_idx = 0;
        while b_idx < file_unmatched[i].base.len() {
            let b_leaf = str_leaf(&file_unmatched[i].base[b_idx].name);
            let mut matched = false;
            for j in 0..file_unmatched.len() {
                if i == j {
                    continue;
                }
                if let Some(pos) = file_unmatched[j]
                    .head
                    .iter()
                    .position(|(_, h)| str_leaf(&h.name) == b_leaf)
                {
                    let b = file_unmatched[i].base.remove(b_idx);
                    let (_, h) = file_unmatched[j].head.remove(pos);
                    pairs.push(TestPair {
                        path: file_unmatched[j].path,
                        base: b,
                        head: h,
                        forced: false,
                    });
                    matched = true;
                    break;
                }
            }
            if !matched {
                b_idx += 1;
            }
        }
    }

    // 4. Forced 1-to-1 pairing within file for remaining tests
    for fu in &mut file_unmatched {
        while !fu.base.is_empty() && !fu.head.is_empty() {
            let mut best_pair: Option<(usize, usize, f64)> = None;
            for (b_idx, b) in fu.base.iter().enumerate() {
                for (h_idx, &(_, h)) in fu.head.iter().enumerate() {
                    let sim = name_similarity(&b.name, &h.name);
                    match best_pair {
                        None => best_pair = Some((b_idx, h_idx, sim)),
                        Some((_, _, best_sim)) => {
                            if sim > best_sim {
                                best_pair = Some((b_idx, h_idx, sim));
                            }
                        }
                    }
                }
            }
            if let Some((b_idx, h_idx, _)) = best_pair {
                let b = fu.base.remove(b_idx);
                let (_, h) = fu.head.remove(h_idx);
                pairs.push(TestPair {
                    path: fu.path,
                    base: b,
                    head: h,
                    forced: true,
                });
            } else {
                break;
            }
        }

        // Any remaining base tests are genuine surplus removals
        for b in fu.base.drain(..) {
            removed.push(Located {
                path: fu.path,
                file_survives: fu.file_survives,
                test: b,
            });
        }

        // Any remaining head tests are genuine surplus additions
        for (_, h) in fu.head.drain(..) {
            added.push(Located {
                path: fu.path,
                file_survives: true,
                test: h,
            });
        }
    }

    // 5. Compile-time assertion pairing
    for ff in files {
        if let (Some(b_facts), Some(h_facts)) = (&ff.base, &ff.head) {
            if let (Some(b), Some(h)) = (&b_facts.compile_time_test, &h_facts.compile_time_test) {
                if b.total_asserts > 0 || h.total_asserts > 0 {
                    pairs.push(TestPair {
                        path: &ff.file.path,
                        base: b,
                        head: h,
                        forced: false,
                    });
                }
            }
        }
    }

    (pairs, removed, added)
}

pub(crate) fn leaf_name(test: &TestFn) -> &str {
    let s = test.name.rsplit("::").next().unwrap_or(&test.name);
    let s = s.rsplit('#').next().unwrap_or(s);
    s.rsplit(" > ").next().unwrap_or(s)
}

pub(crate) fn report_parse_errors(
    files: &[FileFacts],
    severity: crate::config::Severity,
    out: &mut GateOutcome,
    exempt: &PathFilter,
) {
    for ff in files {
        if exempt.matches(&ff.file.path) {
            continue;
        }
        if let Some(h) = ff.head.as_ref() {
            if h.has_parse_errors {
                let is_c_like = matches!(
                    crate::ast::language_for(&ff.file.path),
                    Some(
                        crate::ast::Language::C
                            | crate::ast::Language::Cpp
                            | crate::ast::Language::CSharp
                    )
                );
                let sev = if is_c_like {
                    crate::config::Severity::Warning
                } else {
                    severity
                };
                let err_line = if is_c_like {
                    h.first_parse_error_line.or(Some(1))
                } else {
                    h.first_parse_error_line
                };
                let (title, msg) = if is_c_like {
                    let line_display = err_line.unwrap_or(1);
                    let lang_name = match crate::ast::language_for(&ff.file.path) {
                        Some(crate::ast::Language::CSharp) => "C#",
                        _ => "C/C++",
                    };
                    out.notes.push(format!(
                        "file `{}` had {} skipped {} parse error region(s) (first error near line {})",
                        ff.file.path, h.skipped_error_nodes_count, lang_name, line_display
                    ));
                    (
                        "Preprocessor or Syntax Parse Warning",
                        format!(
                            "{lang_name} grammar encountered preprocessor or syntax errors near line {line_display} ({} skipped AST error region(s)). Surrounding well-formed code was inspected, but some facts may be incomplete.",
                            h.skipped_error_nodes_count
                        ),
                    )
                } else {
                    (
                        "Source File Could Not Be Fully Parsed",
                        "The grammar reported syntax errors, so assertion and unsafe facts for \
                         this file may be incomplete. A gate that cannot read its input does not pass."
                            .to_string(),
                    )
                };

                out.push(
                    sev,
                    title,
                    Some(&ff.file.path),
                    err_line,
                    msg,
                    "Fix the syntax error, or list the path under `exempt_paths` for the AST gates \
                     if it uses syntax the bundled grammar does not know yet.",
                );
            }
        }
    }
}

/// Newly added NUL bytes in source files flag a violation, liftable by directive.
pub(crate) fn report_newly_added_nul_bytes(
    files: &[FileFacts],
    severity: crate::config::Severity,
    out: &mut GateOutcome,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
    exempt: &PathFilter,
) {
    for ff in files {
        if exempt.matches(&ff.file.path) {
            continue;
        }
        if ff.newly_added_nul {
            if let Some(record) =
                tokens::find_override(directives, out.gate, tokens::ALLOW_NUL, &ff.file.path)
            {
                out.overrides.push(record);
                continue;
            }
            let sev = if is_staged {
                crate::config::Severity::Warning
            } else {
                severity
            };
            out.push(
                sev,
                "Source File Contains Newly Added NUL Byte",
                Some(&ff.file.path),
                None,
                format!(
                    "Source file `{}` contains a newly added NUL byte; refusing corrupted or binary source without directive.",
                    ff.file.path
                ),
                &format!(
                    "Remove the NUL byte, or justify it on its own line in the PR body or a commit message: `allow-nul: {} <reason>` (or `discipline:allow({}): {} <reason>`).",
                    ff.file.path, out.gate, ff.file.path
                ),
            );
        }
    }
}

pub fn evaluate_assertion_reduction(
    pairs: &[TestPair],
    _added: &[Located],
    settings: &crate::config::AssertionGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
) -> Result<GateOutcome> {
    const GATE: &str = "assertion-reduction";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = pairs.len();

    for p in pairs.iter().filter(|p| !exempt.matches(p.path)) {
        let (b, h) = (p.base, p.head);
        let b_eff = b.effective_asserts();
        let h_eff = h.effective_asserts();
        let mut total_drop = h_eff < b_eff;
        let mut strong_drop = h.strong_asserts < b.strong_asserts;
        // Checks moved into same-file helpers that fail (assert, raise, throw, panic): one
        // `raise` in a helper's loop stands for many inline assertions, so the count drops
        // while the test calls more failing helpers than before. Deleting a helper call
        // lowers `helper_checks` and is still a drop.
        if (total_drop || strong_drop) && h.helper_checks > b.helper_checks {
            out.notes.push(format!(
                "`{}` in `{}`: assertions {} -> {} read as moved into same-file helpers that fail ({} -> {} calls)",
                h.name, p.path, b_eff, h_eff, b.helper_checks, h.helper_checks
            ));
            total_drop = false;
            strong_drop = false;
        }
        let fatal_drop = h.fatal_asserts < b.fatal_asserts;
        // More doubles in the test, and no stronger assertion on what the code produced:
        // the shape of an integration failure sidestepped by mocking it away.
        let mock_growth = h.mock_setups > b.mock_setups
            && h.strong_asserts <= b.strong_asserts
            && h_eff.saturating_sub(h.mock_asserts) <= b_eff.saturating_sub(b.mock_asserts);
        if !(total_drop || strong_drop || fatal_drop || mock_growth) {
            continue;
        }

        // For forced pairs (unrelated names forced together), the override directive MUST name
        // the old test that was replaced/gutted. For non-forced pairs (exact name or similarity rename),
        // naming either the old test or the new test is accepted.
        let allowed = if p.forced {
            tokens::find_override(directives, GATE, tokens::ALLOW_ASSERTION_DROP, leaf_name(b))
                .or_else(|| {
                    tokens::find_override(directives, GATE, tokens::ALLOW_ASSERTION_DROP, p.path)
                })
        } else {
            tokens::find_override(directives, GATE, tokens::ALLOW_ASSERTION_DROP, leaf_name(h))
                .or_else(|| {
                    tokens::find_override(
                        directives,
                        GATE,
                        tokens::ALLOW_ASSERTION_DROP,
                        leaf_name(b),
                    )
                })
                .or_else(|| {
                    tokens::find_override(directives, GATE, tokens::ALLOW_ASSERTION_DROP, p.path)
                })
        };
        if let Some(record) = allowed {
            out.overrides.push(record);
            continue;
        }

        let test_label = if p.forced {
            format!("Test `{}` -> `{}`", b.name, h.name)
        } else {
            format!("Test `{}`", h.name)
        };
        let directive_name = if p.forced { leaf_name(b) } else { leaf_name(h) };

        if !total_drop && !strong_drop && !fatal_drop && mock_growth {
            out.push(
                crate::config::Severity::Warning,
                "Mocking Grew Without Stronger Assertions",
                Some(p.path),
                Some(h.line),
                format!(
                    "{test_label}: test doubles rose from {} to {} while assertions on real output did not grow (equality / pattern assertions: {} -> {}).",
                    b.mock_setups, h.mock_setups, b.strong_asserts, h.strong_asserts
                ),
                &format!(
                    "Assert on what the code produces alongside the new doubles, or justify the change in the PR body: `allow-assertion-drop: {} <reason>`.",
                    directive_name
                ),
            );
            continue;
        }

        if !total_drop && !strong_drop && fatal_drop {
            out.push(
                crate::config::Severity::Warning,
                "Fatal Assertions Weakened to Non-Fatal",
                Some(p.path),
                Some(h.line),
                format!(
                    "{test_label}: fatal assertions dropped from {} to {} (weakened from abort-on-failure to non-fatal).",
                    b.fatal_asserts, h.fatal_asserts
                ),
                &format!(
                    "Restore fatal assertions (e.g. ASSERT_* or require.*), or justify the change in the PR body: `allow-assertion-drop: {} <reason>`.",
                    directive_name
                ),
            );
            continue;
        }

        let what = if total_drop {
            format!(
                "effective assertions dropped from {} to {}",
                b.effective_asserts(),
                h.effective_asserts()
            )
        } else {
            format!(
                "equality / pattern assertions dropped from {} to {} (weakened to a looser form)",
                b.strong_asserts, h.strong_asserts
            )
        };
        let test_label = if p.forced {
            format!("Test `{}` -> `{}`", b.name, h.name)
        } else {
            format!("Test `{}`", h.name)
        };
        let directive_name = if p.forced { leaf_name(b) } else { leaf_name(h) };

        let severity = if is_staged {
            crate::config::Severity::Warning
        } else {
            settings.severity()
        };

        let violation_line = if h.total_asserts > 0 { h.line } else { b.line };

        out.push(
            severity,
            "Assertion Reduction In Existing Test",
            Some(p.path),
            Some(violation_line),
            format!("{test_label}: {what}."),
            &format!(
                "Restore the assertions, or justify the drop on its own line in the PR body or \
                 a commit message: `allow-assertion-drop: {} <reason>`.",
                directive_name
            ),
        );
    }
    Ok(out)
}

pub fn evaluate_vacuous_tests(
    added: &[Located],
    settings: &crate::config::AssertionGate,
) -> Result<GateOutcome> {
    const GATE: &str = "vacuous-tests";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = added.len();

    for a in added.iter().filter(|a| !exempt.matches(a.path)) {
        // Every assertion is on a double's interactions: the test checks that the mock
        // was called, and nothing about what the code produced.
        if a.test.mock_asserts > 0
            && a.test.mock_asserts >= a.test.effective_asserts()
            && a.test.strong_asserts == 0
            && !a.test.should_panic
        {
            out.push(
                crate::config::Severity::Warning,
                "Test Asserts Only On Mocks",
                Some(a.path),
                Some(a.test.line),
                format!(
                    "New test `{}` makes {} assertion(s), all on test-double interactions; it does not check what the code produces.",
                    a.test.name, a.test.mock_asserts
                ),
                "Assert on the result or the observable effect as well; interaction checks alone pass whatever the code returns.",
            );
            continue;
        }
        // Every assertion holds for nearly any value: `is not None`, `toBeDefined`,
        // `is_ok()`. The test runs the code and checks that something came back.
        // (`is not None` is a comparison, so the pack may count it as strong; the
        // trivial count decides.)
        if a.test.trivial_asserts > 0
            && a.test.trivial_asserts >= a.test.effective_asserts()
            && !a.test.should_panic
        {
            out.push(
                crate::config::Severity::Warning,
                "Test Asserts Only Trivial Properties",
                Some(a.path),
                Some(a.test.line),
                format!(
                    "New test `{}` makes {} assertion(s) that hold for nearly any value (not-null, defined, ok, truthy); it does not check what the code produced.",
                    a.test.name, a.test.trivial_asserts
                ),
                "Assert on the value or the effect; a not-null check passes any wrong answer.",
            );
            continue;
        }
        if a.test.is_vacuous() {
            let why = if a.test.total_asserts == 0 {
                "contains no assertion".to_string()
            } else {
                format!(
                    "contains only tautological assertions ({} of {})",
                    a.test.tautologies, a.test.total_asserts
                )
            };
            out.push(
                settings.severity(),
                "Vacuous Test Added",
                Some(a.path),
                Some(a.test.line),
                format!("New test `{}` {why}; it cannot fail.", a.test.name),
                "Assert the behavior under test. If the suite asserts through helpers or custom \
                 macros, declare them in `assert_helper_fns` / `extra_assert_macros`.",
            );
        } else if let Some(min) = settings.min_assertions_per_test {
            if a.test.effective_asserts() < min {
                out.push(
                    settings.severity(),
                    "Insufficient Assertion Density",
                    Some(a.path),
                    Some(a.test.line),
                    format!(
                        "New test `{}` contains {} effective assertion(s), failing minimum assertion density floor of {min}.",
                        a.test.name,
                        a.test.effective_asserts()
                    ),
                    "Add additional discriminating assertions to meet the configured assertion density floor.",
                );
            }
        }
    }
    Ok(out)
}

pub fn evaluate_ignored_tests(
    pairs: &[TestPair],
    added: &[Located],
    settings: &crate::config::IgnoredTestsGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
) -> Result<GateOutcome> {
    const GATE: &str = "ignored-tests";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = pairs.len() + added.len();

    let newly_ignored_existing = pairs
        .iter()
        .filter(|p| p.head.ignored && !p.base.ignored)
        .map(|p| (p.path, p.head, false));
    let newly_ignored_added = added
        .iter()
        .filter(|a| a.test.ignored)
        .map(|a| (a.path, a.test, true));

    for (path, test, arrives_ignored) in newly_ignored_existing.chain(newly_ignored_added) {
        if exempt.matches(path) {
            continue;
        }
        if let Some(record) =
            tokens::find_override(directives, GATE, tokens::ALLOW_IGNORE, leaf_name(test))
        {
            let subject = leaf_name(test);
            let cleaned = record.reason.trim().trim_matches(['"', '\'', '`']);
            let explanation = cleaned
                .strip_prefix(subject)
                .map(|s| s.trim_start_matches(|c: char| c == ':' || c == '-' || c.is_whitespace()))
                .unwrap_or(cleaned)
                .trim();
            if explanation.is_empty()
                || explanation.eq_ignore_ascii_case("todo")
                || explanation.eq_ignore_ascii_case("tbd")
                || explanation.eq_ignore_ascii_case("fix later")
                || explanation.eq_ignore_ascii_case("temporary")
                || explanation.eq_ignore_ascii_case("wip")
            {
                out.push(
                    settings.severity(),
                    "Unannotated Skip Justification",
                    Some(path),
                    Some(test.line),
                    format!(
                        "Directive for skipped test `{}` lacks a substantive rationale or issue tracker reference (got `{}`).",
                        test.name, record.reason
                    ),
                    "Provide a substantive explanation or linked issue reference (e.g. `allow-ignore: <test> #123 fix broken upstream API`).",
                );
                continue;
            }
            out.overrides.push(record);
            continue;
        }
        let severity = if is_staged {
            crate::config::Severity::Warning
        } else {
            settings.severity()
        };
        let (title, message) = if arrives_ignored {
            (
                "Test Arrives Ignored",
                format!("Test `{}` arrives ignored.", test.name),
            )
        } else {
            (
                "Test Newly Skipped",
                format!("Test `{}` no longer runs.", test.name),
            )
        };
        out.push(
            severity,
            title,
            Some(path),
            Some(test.line),
            message,
            &format!(
                "Fix the test, or justify it on its own line in the PR body or a commit \
                 message: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            ),
        );
    }

    // A test made green by running it again. A retry marker does not skip the test, but
    // it lets a failure through as often as the marker allows.
    // A delay added to a test: the shape of a race fixed by waiting for it.
    let newly_slept = pairs
        .iter()
        .filter(|p| p.head.sleeps > p.base.sleeps)
        .map(|p| (p.path, p.head, p.base.sleeps))
        .chain(
            added
                .iter()
                .filter(|a| a.test.sleeps > 0)
                .map(|a| (a.path, a.test, 0)),
        );
    for (path, test, before) in newly_slept {
        if exempt.matches(path) {
            continue;
        }
        if let Some(record) =
            tokens::find_override(directives, GATE, tokens::ALLOW_IGNORE, leaf_name(test))
        {
            out.overrides.push(record);
            continue;
        }
        out.push(
            crate::config::Severity::Warning,
            "Test Sleeps",
            Some(path),
            Some(test.line),
            format!(
                "Test `{}` carries {} hard-coded delay(s) (was {before}); a timing-dependent pass slows the suite and hides the race.",
                test.name, test.sleeps
            ),
            &format!(
                "Synchronise on the event the test waits for, or justify the delay: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            ),
        );
    }

    let newly_retried = pairs
        .iter()
        .filter(|p| p.head.retries.is_some() && p.base.retries.is_none())
        .map(|p| (p.path, p.head))
        .chain(
            added
                .iter()
                .filter(|a| a.test.retries.is_some())
                .map(|a| (a.path, a.test)),
        );
    for (path, test) in newly_retried {
        if exempt.matches(path) {
            continue;
        }
        if let Some(record) =
            tokens::find_override(directives, GATE, tokens::ALLOW_IGNORE, leaf_name(test))
        {
            out.overrides.push(record);
            continue;
        }
        let marker = test.retries.as_deref().unwrap_or("");
        out.push(
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            },
            "Test Retries On Failure",
            Some(path),
            Some(test.line),
            format!(
                "Test `{}` carries a retry marker (`{marker}`); a failure passes on a later attempt.",
                test.name
            ),
            &format!(
                "Fix the cause of the flakiness, or justify the retry on its own line in the PR body or a commit message: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            ),
        );
    }

    let newly_cond_ignored = pairs
        .iter()
        .filter(|p| {
            p.head.conditional_ignore.is_some()
                && p.base.conditional_ignore.is_none()
                && !p.head.ignored
        })
        .map(|p| (p.path, p.head))
        .chain(
            added
                .iter()
                .filter(|a| a.test.conditional_ignore.is_some() && !a.test.ignored)
                .map(|a| (a.path, a.test)),
        );
    for (path, test) in newly_cond_ignored {
        if exempt.matches(path) {
            continue;
        }
        let cond = test.conditional_ignore.as_deref().unwrap_or("condition");
        if settings
            .approved_predicates
            .iter()
            .any(|p| cond == p || cond.contains(p) || p.contains(cond))
        {
            continue;
        }
        if let Some(record) =
            tokens::find_override(directives, GATE, tokens::ALLOW_IGNORE, leaf_name(test))
        {
            out.overrides.push(record);
            continue;
        }
        out.push(
            crate::config::Severity::Note,
            "Test Conditionally Skipped",
            Some(path),
            Some(test.line),
            format!(
                "Test `{}` is conditionally skipped under predicate `{}`.",
                test.name, cond
            ),
            "Conditional skips are monitored. If this was unintended, remove the conditional ignore attribute.",
        );
    }

    Ok(out)
}

pub fn evaluate_unsafe_safety_comment(
    files: &[FileFacts],
    settings: &crate::config::UnsafeSafetyCommentGate,
) -> Result<GateOutcome> {
    const GATE: &str = "unsafe-safety-comment";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    // Only a pack that reads unsafe sites examines a file for this gate. A file in a
    // language with its own explicit unsafe construct (Go's `unsafe` package, C#
    // `unsafe` blocks, Swift's `Unsafe*Pointer`) that no pack reads is named, never
    // counted as examined; a language without one has nothing here to miss.
    let registry = default_registry();
    let reads_unsafe = |path: &str| {
        registry
            .find_pack(path)
            .is_some_and(|p| p.supplies(crate::ast::Fact::UnsafeSites))
    };
    let mut unread: Vec<&str> = Vec::new();
    for ff in files {
        if ff.head.is_none() || exempt.matches(&ff.file.path) {
            continue;
        }
        if reads_unsafe(&ff.file.path) {
            out.examined += 1;
        } else if matches!(
            crate::ast::extension(&ff.file.path),
            Some("go" | "cs" | "swift")
        ) {
            unread.push(&ff.file.path);
        }
    }
    if !unread.is_empty() {
        let sample: Vec<&str> = unread.iter().take(3).copied().collect();
        out.notes.push(format!(
            "{} changed file(s) are in a language with unsafe code this gate does not read (Go, C#, Swift) and were NOT analysed for it (e.g. {})",
            unread.len(),
            sample.join(", ")
        ));
    }

    for ff in files {
        let Some(head) = &ff.head else { continue };
        if exempt.matches(&ff.file.path) || !reads_unsafe(&ff.file.path) {
            continue;
        }
        let undocumented =
            |f: &ParsedFileFacts| f.unsafe_sites.iter().filter(|s| !s.documented).count();
        let base_undocumented = ff.base.as_ref().map(undocumented).unwrap_or(0);
        // A site is in scope when its line was added, or — to catch a SAFETY
        // comment deleted from above an untouched block — when the file now
        // has more undocumented sites than it had on the base side.
        let regressed = undocumented(head) > base_undocumented;
        for site in head.unsafe_sites.iter().filter(|s| !s.documented) {
            if !(ff.file.added_lines.contains(&site.line) || regressed) {
                continue;
            }
            out.push(
                settings.severity(),
                "Unsafe Without SAFETY Comment",
                Some(&ff.file.path),
                Some(site.line),
                format!("Undocumented {}: `{}`", site.kind, site.snippet),
                "Add a `// SAFETY: <why the invariants hold>` comment directly above the block \
                 or the statement that contains it.",
            );
        }
    }
    Ok(out)
}

pub fn evaluate_deletion_rationale(
    changed: &[ChangedFile],
    removed: &[Located],
    settings: &crate::config::DeletionGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
) -> Result<GateOutcome> {
    const GATE: &str = "deletion-rationale";
    let exempt = exempt_filter(settings)?;
    let watched = PathFilter::new(&settings.paths)?;
    let severity = if is_staged {
        crate::config::Severity::Warning
    } else {
        settings.severity()
    };
    let mut out = GateOutcome::new(GATE);

    let check_override = |subject: &str,
                          alt_subject: Option<&str>|
     -> Option<crate::tokens::OverrideRecord> {
        let rec = if settings.require_scope {
            tokens::find_override(directives, GATE, tokens::REMOVES, subject).or_else(|| {
                alt_subject
                    .and_then(|alt| tokens::find_override(directives, GATE, tokens::REMOVES, alt))
            })
        } else {
            directives
                .iter()
                .find(|d| {
                    tokens::REMOVES
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(&d.directive))
                })
                .map(|d| crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    subject: subject.to_string(),
                    directive: d.directive.clone(),
                    reason: d.reason.clone(),
                    source: d.source.clone(),
                    hidden: d.hidden,
                })
        };
        if let Some(r) = rec {
            if settings.allow_hidden == Some(false) && r.hidden {
                return None;
            }
            return Some(r);
        }
        None
    };

    for file in changed.iter().filter(|f| f.kind == ChangeKind::Deleted) {
        if !watched.matches(&file.path) || exempt.matches(&file.path) {
            continue;
        }
        out.examined += 1;
        if let Some(record) = check_override(&file.path, None) {
            out.overrides.push(record);
            continue;
        }
        out.push(
            severity,
            "File Deleted Without Rationale",
            Some(&file.path),
            None,
            format!(
                "`{}` was deleted and no `removes:` directive names it.",
                file.path
            ),
            &format!(
                "State why on its own line in the PR body or a commit message: \
                 `removes: {} <reason>` (a directory prefix covers everything under it).",
                file.path
            ),
        );
    }

    // Tests removed from a file that still exists. Because match_tests force-pairs
    // tests 1-to-1 within each file, any tests remaining in `removed` are genuine
    // surplus deletions (removed tests outnumber added tests in that file).
    // Tests inside a deleted file are covered by that file's own rationale.
    for r in removed.iter().filter(|r| r.file_survives) {
        if !watched.matches(r.path) || exempt.matches(r.path) {
            continue;
        }
        out.examined += 1;
        if let Some(record) = check_override(leaf_name(r.test), Some(r.path)) {
            out.overrides.push(record);
            continue;
        }
        out.push(
            severity,
            "Test Removed Without Rationale",
            Some(r.path),
            None,
            format!("Test `{}` was removed from `{}`.", r.test.name, r.path),
            &format!(
                "State why on its own line in the PR body or a commit message: \
                 `removes: {} <reason>`.",
                leaf_name(r.test)
            ),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::RustFacts;

    #[test]
    fn name_similarity_discriminates_renames_from_unrelated_tests() {
        assert_eq!(name_similarity("adds", "adds"), 1.0);
        assert!(name_similarity("adds", "adds_integers") >= RENAME_NAME_SIMILARITY_THRESHOLD);
        assert!(
            name_similarity(
                "ablation_sharded_alloc_counts_per_stripe",
                "alloc_counts_per_stripe"
            ) >= RENAME_NAME_SIMILARITY_THRESHOLD
        );
        assert!(
            name_similarity(
                "str_node_is_just_the_map_core",
                "map_core_is_just_the_str_node"
            ) >= RENAME_NAME_SIMILARITY_THRESHOLD
        );
        assert!(name_similarity("test_alpha", "test_beta") < RENAME_NAME_SIMILARITY_THRESHOLD);
        assert!(name_similarity("adds", "orders") < RENAME_NAME_SIMILARITY_THRESHOLD);
    }

    #[test]
    fn match_tests_force_pairs_unrelated_tests_in_same_file() {
        let t1 = TestFn {
            name: "test_alpha".to_string(),
            line: 10,
            total_asserts: 3,
            strong_asserts: 2,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let t2 = TestFn {
            name: "test_omega".to_string(),
            line: 20,
            total_asserts: 1,
            strong_asserts: 0,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let facts = vec![FileFacts {
            file: ChangedFile {
                path: "tests/foo.rs".into(),
                old_path: "tests/foo.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: std::collections::BTreeSet::new(),
            },
            base: Some(RustFacts {
                tests: vec![t1],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            head: Some(RustFacts {
                tests: vec![t2],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];

        let (pairs, removed, added) = match_tests(&facts);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].forced);
        assert_eq!(pairs[0].base.name, "test_alpha");
        assert_eq!(pairs[0].head.name, "test_omega");
        assert!(removed.is_empty());
        assert!(added.is_empty());
    }

    #[test]
    fn match_tests_surplus_base_goes_to_removed_and_surplus_head_to_added() {
        let b1 = TestFn {
            name: "b1".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let b2 = TestFn {
            name: "b2".to_string(),
            line: 5,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let h1 = TestFn {
            name: "h1".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let facts = vec![FileFacts {
            file: ChangedFile {
                path: "tests/foo.rs".into(),
                old_path: "tests/foo.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: std::collections::BTreeSet::new(),
            },
            base: Some(RustFacts {
                tests: vec![b1, b2],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            head: Some(RustFacts {
                tests: vec![h1],
                unsafe_sites: vec![],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];

        let (pairs, removed, added) = match_tests(&facts);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].forced);
        assert_eq!(pairs[0].base.name, "b1");
        assert_eq!(pairs[0].head.name, "h1");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].test.name, "b2");
        assert!(added.is_empty());
    }

    #[test]
    fn test_assertion_reduction_pure() {
        let b = TestFn {
            name: "test_something".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 2,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let h_weak = TestFn {
            name: "test_something".to_string(),
            line: 1,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let h_drop = TestFn {
            name: "test_something".to_string(),
            line: 1,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let settings = crate::config::AssertionGate::default();

        // Weakened strong assert
        let pair_weak = [TestPair {
            path: "tests/a.rs",
            base: &b,
            head: &h_weak,
            forced: false,
        }];
        let out_weak =
            evaluate_assertion_reduction(&pair_weak, &[], &settings, &[], false).unwrap();
        assert_eq!(out_weak.violations.len(), 1);
        assert_eq!(out_weak.examined, 1);

        // Dropped total assert
        let pair_drop = [TestPair {
            path: "tests/a.rs",
            base: &b,
            head: &h_drop,
            forced: false,
        }];
        let out_drop =
            evaluate_assertion_reduction(&pair_drop, &[], &settings, &[], false).unwrap();
        assert_eq!(out_drop.violations.len(), 1);

        // Override justifies the drop
        let directives = [crate::tokens::ParsedDirective {
            directive: "allow-assertion-drop".to_string(),
            reason: "test_something refactored".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_override =
            evaluate_assertion_reduction(&pair_drop, &[], &settings, &directives, false).unwrap();
        assert_eq!(out_override.violations.len(), 0);
        assert_eq!(out_override.overrides.len(), 1);

        // Added test does NOT absorb the drop: reductions are strictly per-paired test
        let added_split = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &b, // has 2 asserts
        }];
        let out_unabsorbed =
            evaluate_assertion_reduction(&pair_drop, &added_split, &settings, &[], false).unwrap();
        assert_eq!(out_unabsorbed.violations.len(), 1);
    }

    #[test]
    fn test_vacuous_tests_pure() {
        let settings = crate::config::AssertionGate::default();
        let t_empty = TestFn {
            name: "empty".to_string(),
            line: 10,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let t_tautology = TestFn {
            name: "tauto".to_string(),
            line: 20,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 1,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let t_real = TestFn {
            name: "real".to_string(),
            line: 30,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };

        let loc_empty = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &t_empty,
        }];
        let out_empty = evaluate_vacuous_tests(&loc_empty, &settings).unwrap();
        assert_eq!(out_empty.violations.len(), 1);
        assert!(out_empty.violations[0]
            .message
            .contains("contains no assertion"));

        let loc_tauto = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &t_tautology,
        }];
        let out_tauto = evaluate_vacuous_tests(&loc_tauto, &settings).unwrap();
        assert_eq!(out_tauto.violations.len(), 1);
        assert!(out_tauto.violations[0]
            .message
            .contains("contains only tautological assertions"));

        let loc_real = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &t_real,
        }];
        let out_real = evaluate_vacuous_tests(&loc_real, &settings).unwrap();
        assert_eq!(out_real.violations.len(), 0);
    }

    #[test]
    fn test_ignored_tests_pure() {
        let settings = crate::config::IgnoredTestsGate::default();
        let b = TestFn {
            name: "active".to_string(),
            line: 1,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let h_ignored = TestFn {
            name: "active".to_string(),
            line: 1,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: true,
            should_panic: false,
            ..Default::default()
        };

        let pairs = [TestPair {
            path: "tests/a.rs",
            base: &b,
            head: &h_ignored,
            forced: false,
        }];
        let out = evaluate_ignored_tests(&pairs, &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].title, "Test Newly Skipped");
        assert!(out.violations[0].message.contains("no longer runs"));

        // Test arriving ignored
        let added_ignored = [Located {
            path: "tests/new.rs",
            file_survives: true,
            test: &h_ignored,
        }];
        let out_added = evaluate_ignored_tests(&[], &added_ignored, &settings, &[], false).unwrap();
        assert_eq!(out_added.violations.len(), 1);
        assert_eq!(out_added.violations[0].title, "Test Arrives Ignored");
        assert!(out_added.violations[0].message.contains("arrives ignored"));

        // Test conditional ignore with and without approved_predicates
        let h_miri = TestFn {
            name: "miri_test".to_string(),
            line: 10,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            conditional_ignore: Some("miri".to_string()),
            should_panic: false,
            ..Default::default()
        };
        let added_miri = [Located {
            path: "tests/miri.rs",
            file_survives: true,
            test: &h_miri,
        }];
        // Without approved predicate -> warning violation
        let out_unapproved =
            evaluate_ignored_tests(&[], &added_miri, &settings, &[], false).unwrap();
        assert_eq!(out_unapproved.violations.len(), 1);
        assert_eq!(
            out_unapproved.violations[0].title,
            "Test Conditionally Skipped"
        );

        // With approved predicate -> 0 violations
        let mut approved_settings = settings.clone();
        approved_settings.approved_predicates = vec!["miri".to_string()];
        let out_approved =
            evaluate_ignored_tests(&[], &added_miri, &approved_settings, &[], false).unwrap();
        assert_eq!(out_approved.violations.len(), 0);

        let directives = [crate::tokens::ParsedDirective {
            directive: "allow-ignore".to_string(),
            reason: "active flaky upstream".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_override =
            evaluate_ignored_tests(&pairs, &[], &settings, &directives, false).unwrap();
        assert_eq!(out_override.violations.len(), 0);
        assert_eq!(out_override.overrides.len(), 1);
    }

    #[test]
    fn test_unsafe_safety_comment_pure() {
        use crate::ast::UnsafeSite;
        use std::collections::BTreeSet;

        let settings = crate::config::UnsafeSafetyCommentGate::default();
        let mut added_lines = BTreeSet::new();
        added_lines.insert(15);

        let facts_undocumented = [FileFacts {
            file: ChangedFile {
                path: "src/lib.rs".into(),
                old_path: "src/lib.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: added_lines.clone(),
            },
            base: None,
            head: Some(RustFacts {
                tests: vec![],
                unsafe_sites: vec![UnsafeSite {
                    kind: "block",
                    line: 15,
                    documented: false,
                    snippet: "unsafe { *p }".into(),
                }],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];
        let out_bad = evaluate_unsafe_safety_comment(&facts_undocumented, &settings).unwrap();
        assert_eq!(out_bad.violations.len(), 1);

        let facts_documented = [FileFacts {
            file: ChangedFile {
                path: "src/lib.rs".into(),
                old_path: "src/lib.rs".into(),
                kind: ChangeKind::Modified,
                added_lines,
            },
            base: None,
            head: Some(RustFacts {
                tests: vec![],
                unsafe_sites: vec![UnsafeSite {
                    kind: "block",
                    line: 15,
                    documented: true,
                    snippet: "unsafe { *p }".into(),
                }],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];
        let out_good = evaluate_unsafe_safety_comment(&facts_documented, &settings).unwrap();
        assert_eq!(out_good.violations.len(), 0);
    }

    #[test]
    fn test_deletion_rationale_pure() {
        let settings = crate::config::DeletionGate::default();
        let deleted_files = [ChangedFile {
            path: "src/legacy.rs".into(),
            old_path: "src/legacy.rs".into(),
            kind: ChangeKind::Deleted,
            added_lines: std::collections::BTreeSet::new(),
        }];

        // File deletion without rationale
        let out_unexcused =
            evaluate_deletion_rationale(&deleted_files, &[], &settings, &[], false).unwrap();
        assert_eq!(out_unexcused.violations.len(), 1);

        // File deletion with rationale
        let directives = [crate::tokens::ParsedDirective {
            directive: "removes".to_string(),
            reason: "src/legacy.rs removed in v2".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_excused =
            evaluate_deletion_rationale(&deleted_files, &[], &settings, &directives, false)
                .unwrap();
        assert_eq!(out_excused.violations.len(), 0);
        assert_eq!(out_excused.overrides.len(), 1);

        // Test removal from surviving file without rationale
        let t = TestFn {
            name: "test_old".to_string(),
            line: 5,
            total_asserts: 2,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: false,
            ..Default::default()
        };
        let removed_tests = [Located {
            path: "tests/suite.rs",
            file_survives: true,
            test: &t,
        }];
        let out_test_unexcused =
            evaluate_deletion_rationale(&[], &removed_tests, &settings, &[], false).unwrap();
        assert_eq!(out_test_unexcused.violations.len(), 1);

        // Test removal with rationale
        let test_directive = [crate::tokens::ParsedDirective {
            directive: "removes".to_string(),
            reason: "test_old superseded".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_test_excused =
            evaluate_deletion_rationale(&[], &removed_tests, &settings, &test_directive, false)
                .unwrap();
        assert_eq!(out_test_excused.violations.len(), 0);
        assert_eq!(out_test_excused.overrides.len(), 1);
    }

    #[test]
    fn test_compile_time_assertions_drop_detected_and_overridden() {
        let mut base_facts = ParsedFileFacts {
            compile_time_asserts: 3,
            compile_time_assert_line: Some(10),
            ..Default::default()
        };
        base_facts.build_compile_time_test();

        let mut head_facts = ParsedFileFacts {
            compile_time_asserts: 1,
            compile_time_assert_line: Some(10),
            ..Default::default()
        };
        head_facts.build_compile_time_test();

        let files = vec![FileFacts {
            file: ChangedFile {
                path: "src/types.rs".into(),
                old_path: "src/types.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: std::collections::BTreeSet::new(),
            },
            base: Some(base_facts),
            head: Some(head_facts),
            newly_added_nul: false,
        }];

        let (pairs, _removed, _added) = match_tests(&files);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].base.name, "compile-time-assertions");

        let settings = crate::config::AssertionGate {
            enabled: true,
            severity: crate::config::Severity::Error,
            exempt_paths: vec![],
            ..Default::default()
        };

        // Without override -> violation
        let out = evaluate_assertion_reduction(&pairs, &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        assert!(out.violations[0]
            .message
            .contains("effective assertions dropped from 3 to 1"));
        assert_eq!(out.violations[0].line, Some(10));

        // With override targeting compile-time-assertions -> pass
        let directive = [crate::tokens::ParsedDirective {
            directive: "allow-assertion-drop".to_string(),
            reason: "compile-time-assertions size refactor".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_override =
            evaluate_assertion_reduction(&pairs, &[], &settings, &directive, false).unwrap();
        assert_eq!(out_override.violations.len(), 0);
        assert_eq!(out_override.overrides.len(), 1);

        // With override targeting file path -> pass
        let file_directive = [crate::tokens::ParsedDirective {
            directive: "allow-assertion-drop".to_string(),
            reason: "src/types.rs size refactor".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_file_override =
            evaluate_assertion_reduction(&pairs, &[], &settings, &file_directive, false).unwrap();
        assert_eq!(out_file_override.violations.len(), 0);
        assert_eq!(out_file_override.overrides.len(), 1);
    }
}
