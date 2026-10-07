//! `error-swallowing`: a change must not add a handler that drops the error, or a
//! statement that throws a `Result` away, outside tests.
//!
//! `except: pass`, `catch (e) {}`, `let _ = fallible();` make a failure invisible; the
//! tests pass because nothing surfaces. The sites come from the language packs
//! (`Fact::Handlers`) and are a base-versus-head delta per file, as `suppression-delta`
//! does: a handler that moved is not new.
//!
//! In the files `constant_fallback_paths` names (a benchmark or evaluation harness), a
//! handler that puts a numeric literal in place of the result is a site too
//! (`constant-fallback`, `except FileNotFoundError: ops_per_sec = 150000.0`): the number
//! flows on as if the guarded code had produced it. It is reported at `warning` at most.

use super::{Context, GateOutcome, PathFilter};
use crate::ast::handlers::SwallowSite;
use crate::ast::{default_registry, Fact, ParsedFileFacts};
use crate::config::GateSettings;
use crate::tokens;
use anyhow::Result;
use std::collections::HashMap;

pub const GATE: &str = "error-swallowing";

fn signature(s: &SwallowSite) -> (&'static str, String) {
    (
        s.kind,
        s.snippet.split_whitespace().collect::<Vec<_>>().join(" "),
    )
}

/// Head-side sites the base side does not have, as a multiset.
pub fn new_sites(base: &[SwallowSite], head: &[SwallowSite]) -> Vec<SwallowSite> {
    let mut budget: HashMap<_, usize> = HashMap::new();
    for s in base {
        *budget.entry(signature(s)).or_default() += 1;
    }
    head.iter()
        .filter(|s| match budget.get_mut(&signature(s)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// The sites of one side of a file, in source order: the handlers that swallow, and, when
/// `constant_fallbacks` (the file is one `constant_fallback_paths` names), the handlers
/// that replace the failure with a numeric literal. A handler is in one list at most.
fn sites_of(facts: ParsedFileFacts, constant_fallbacks: bool) -> Vec<SwallowSite> {
    let mut sites = facts.swallowed;
    if constant_fallbacks {
        sites.extend(facts.constant_fallbacks);
        sites.sort_by_key(|s| s.line);
    }
    sites
}

pub fn error_swallowing(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.error_swallowing;
    let mut out = GateOutcome::new(GATE);
    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let harness = PathFilter::new(&settings.constant_fallback_paths)?;
    let registry = default_registry();
    let vocab = super::agent_diff::assert_vocabulary(ctx.config);
    let mut unsupported: Vec<String> = Vec::new();

    let changed = ctx.git.changed_files()?;
    for (file, pack) in super::ast_changes(
        &changed,
        &exempt,
        &registry,
        Fact::Handlers,
        &mut unsupported,
    ) {
        // Base-anchored classification: a renamed file that was production code on the
        // base side stays it, so a move into test scope cannot silence its findings.
        let anchored = super::base_anchored_classification(&file, &registry, &vocab.test_paths);
        if file.old_path != file.path {
            // The rename check is an item looked at, whatever the file holds.
            out.examined += 1;
        }
        // A rename out of test scope into it is reported once, here, before any
        // head-side early exit; `stub-bodies` judges the same file's bodies and
        // skips it. Paths only, no contents.
        let mut lifted_reclassification = false;
        if anchored.reclassified {
            // The finding has no line: a directive written as `path:line` names a
            // handler, not the move.
            let lift = |subject: &str| {
                ctx.find_whole_file_override(
                    GATE,
                    &crate::findings::TEST_PATH_RECLASSIFIED,
                    tokens::ALLOW_SWALLOW,
                    subject,
                )
            };
            if let Some(ov) =
                lift(&file.path).or_else(|| file.path.rsplit('/').next().and_then(lift))
            {
                out.overrides.push(ov);
                lifted_reclassification = true;
            } else {
                out.push(
                    ctx.overridable(settings.severity()),
                    &crate::findings::TEST_PATH_RECLASSIFIED,
                    Some(&file.path),
                    None,
                    format!(
                        "`{}` was renamed from `{}` into test scope and is still judged as production code.",
                        file.path, file.old_path
                    ),
                    &format!(
                        "Rename it back out of test scope, or justify the move on its own line in the PR body or a commit message: `allow-swallow: {} <reason>`.",
                        file.path
                    ),
                );
            }
        }
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            out.notes.push(super::unread_note(&file.path));
            continue;
        };
        out.notes.extend(
            anchored
                .language_changed_note
                .into_iter()
                .chain(anchored.declared_scope_note),
        );
        // Judged by the head path on both sides, so a handler the base already had is not
        // new on the day the repository names the path.
        let constant_fallbacks = harness.matches(&file.path);
        let head = match pack.extract(&anchored.classify_path, &head_src, &vocab) {
            Ok(f) => {
                // A file added in test scope by its name only, with no test that checks.
                let by_name_only =
                    super::name_only_scope(&file, &changed, pack, &registry, &vocab.test_paths, &f);
                let as_production = by_name_only.and_then(|by_name| {
                    let Some(classify_path) = &by_name.classify_path else {
                        out.notes.push(by_name.note);
                        return None;
                    };
                    let facts = pack.extract(classify_path, &head_src, &vocab).ok()?;
                    let sites = sites_of(facts, constant_fallbacks);
                    (!sites.is_empty()).then_some((by_name.note, sites))
                });
                match as_production {
                    Some((note, production)) => {
                        out.notes.push(note);
                        production
                    }
                    None => sites_of(f, constant_fallbacks),
                }
            }
            Err(e) => {
                out.notes.push(format!(
                    "`{}`: not analysed, the head side does not parse ({e})",
                    file.path
                ));
                continue;
            }
        };
        let base = match ctx.git.base_content(&file.old_path)? {
            Some(src) => pack
                .extract(&file.old_path, &src, &vocab)
                .map(|f| sites_of(f, constant_fallbacks))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        out.examined += head.len();
        let new = new_sites(&base, &head);
        if lifted_reclassification {
            // The override line of a report shows the directive and the path, and one
            // `allow-swallow` on a path lifts the move and every handler in the file.
            let handlers = new
                .iter()
                .filter(|site| {
                    !super::line_allows(
                        head_src
                            .lines()
                            .nth(site.line.saturating_sub(1))
                            .unwrap_or(""),
                        GATE,
                    )
                })
                .count();
            out.notes.push(format!(
                "`allow-swallow` lifted `{}` for `{}` (renamed from `{}` into test scope); a lift by path also covers the handlers of the file, each with its own override record: {handlers} handler finding(s) here; `allow-swallow: {}:<line>` lifts one handler and not the move",
                out.code_of(&crate::findings::TEST_PATH_RECLASSIFIED),
                file.path,
                file.old_path,
                file.path
            ));
        }
        for site in new {
            let line_text = head_src
                .lines()
                .nth(site.line.saturating_sub(1))
                .unwrap_or("");
            if super::line_allows(line_text, GATE) {
                out.inline_exemptions += 1;
                continue;
            }
            let (title, what) = match site.kind {
                "discarded-result" => (&crate::findings::RESULT_DISCARDED, "throws a fallible call's result away"),
                "discarded-value" => (
                    &crate::findings::VALUE_DISCARDED,
                    "throws a call's value away; the callee is not on the known-fallible list, so this is reported at `warning` at most",
                ),
                "logging-handler" => (
                    &crate::findings::ERROR_LOGGED_AND_DROPPED,
                    "catches an error, logs it, and does nothing else with it: the failure is recorded and dropped",
                ),
                "skipped-input" => (
                    &crate::findings::UNPARSEABLE_INPUT_SKIPPED,
                    "skips an input item that does not parse and goes on with the next; the item is dropped without a count, so this is reported at `warning` at most",
                ),
                "silenced-error" => (
                    &crate::findings::ERROR_SILENCED,
                    "replaces every error it raises with nothing",
                ),
                "constant-fallback" => (
                    &crate::findings::ERROR_REPLACED_BY_CONSTANT,
                    "replaces the failure with a numeric literal: what reads the value next takes the number as a result the guarded code produced; `constant_fallback_paths` names this file, and this is reported at `warning` at most",
                ),
                _ => (
                    &crate::findings::EMPTY_ERROR_HANDLER_ADDED,
                    "catches an error and does nothing with it",
                ),
            };
            // The finding's own `path:line` first, written in full. Then the file, by
            // its path or its name, from a directive that names no line: one that does
            // (`pkg/io.py:9`) lifts the finding on that line and no other.
            let lift = |subject: &str| {
                ctx.find_whole_file_override(GATE, title, tokens::ALLOW_SWALLOW, subject)
            };
            let own = [
                format!("{}:{}", file.path, site.line),
                format!(
                    "{}:{}",
                    file.path.rsplit('/').next().unwrap_or(&file.path),
                    site.line
                ),
            ];
            if let Some(ov) = own
                .iter()
                .find_map(|s| ctx.find_override(GATE, title, tokens::ALLOW_SWALLOW, s))
                .or_else(|| lift(&file.path))
                .or_else(|| file.path.rsplit('/').next().and_then(lift))
            {
                out.overrides.push(ov);
                continue;
            }
            // The syntax tree carries no types: a callee off a pack's known-fallible list
            // may return a plain value, so it never blocks on its own.
            let severity = if matches!(site.kind, "discarded-value" | "skipped-input")
                && settings.severity() == crate::config::Severity::Error
            {
                crate::config::Severity::Warning
            } else if site.kind == "constant-fallback" {
                // A number in a handler is a defect only where it is recorded as an
                // observation, which the path says and the tree does not.
                settings.severity().capped_at_warning()
            } else {
                settings.severity()
            };
            let fix = if site.kind == "constant-fallback" {
                "Let the failure show: re-raise it, or record a value that marks the result as absent (`None`, `null`, NaN) instead of a number"
            } else {
                "Handle or propagate the error"
            };
            out.push(
                ctx.overridable(severity),
                title,
                Some(&file.path),
                Some(site.line),
                format!("`{}` {what} in `{}`.", site.snippet, file.path),
                &format!(
                    "{fix}, or justify it on its own line in the PR body or a commit message: `allow-swallow: {} <reason>` (or `discipline:allow(error-swallowing)` on the line).",
                    file.path
                ),
            );
        }
    }
    out.notes
        .extend(super::unsupported_fact_note(&unsupported, "handler"));
    if out.examined == 0 && unsupported.is_empty() {
        out.notes
            .push("no error handlers in changed files of an analysed language".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(line: usize, kind: &'static str, snippet: &str) -> SwallowSite {
        SwallowSite {
            line,
            kind,
            snippet: snippet.into(),
        }
    }

    #[test]
    fn a_moved_handler_is_not_new_and_a_second_one_is() {
        let base = vec![site(3, "empty-handler", "except ValueError:")];
        let moved = vec![site(30, "empty-handler", "except  ValueError:")];
        assert!(new_sites(&base, &moved).is_empty());
        let two = vec![
            site(3, "empty-handler", "except ValueError:"),
            site(9, "empty-handler", "except ValueError:"),
        ];
        assert_eq!(
            new_sites(&base, &two),
            vec![site(9, "empty-handler", "except ValueError:")]
        );
        assert_eq!(new_sites(&[], &base).len(), 1);
    }
}
