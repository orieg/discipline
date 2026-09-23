//! `error-swallowing`: a change must not add a handler that drops the error, or a
//! statement that throws a `Result` away, outside tests.
//!
//! `except: pass`, `catch (e) {}`, `let _ = fallible();` make a failure invisible; the
//! tests pass because nothing surfaces. The sites come from the language packs
//! (`Fact::Handlers`) and are a base-versus-head delta per file, as `suppression-delta`
//! does: a handler that moved is not new.

use super::{Context, GateOutcome, PathFilter};
use crate::ast::handlers::SwallowSite;
use crate::ast::{default_registry, Fact};
use crate::config::GateSettings;
use crate::gitctx::ChangeKind;
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

pub fn error_swallowing(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.error_swallowing;
    let mut out = GateOutcome::new(GATE);
    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let registry = default_registry();
    let vocab = super::agent_diff::assert_vocabulary(ctx.config);
    let mut unsupported: Vec<String> = Vec::new();

    for file in ctx.git.changed_files()? {
        if file.kind == ChangeKind::Deleted || exempt.matches(&file.path) {
            continue;
        }
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        if !pack.supplies(Fact::Handlers) {
            unsupported.push(file.path.clone());
            continue;
        }
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(f) => f.swallowed,
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
                .map(|f| f.swallowed)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        out.examined += head.len();
        for site in new_sites(&base, &head) {
            let line_text = head_src
                .lines()
                .nth(site.line.saturating_sub(1))
                .unwrap_or("");
            if super::line_allows(line_text, GATE) {
                out.inline_exemptions += 1;
                continue;
            }
            let lift = |subject: &str| ctx.find_override(GATE, tokens::ALLOW_SWALLOW, subject);
            if let Some(ov) = lift(&file.path)
                .or_else(|| file.path.rsplit('/').next().and_then(lift))
                .or_else(|| lift(&format!("{}:{}", file.path, site.line)))
            {
                out.overrides.push(ov);
                continue;
            }
            let (title, what) = match site.kind {
                "discarded-result" => ("Result Discarded", "throws a fallible call's result away"),
                "discarded-value" => (
                    "Value Discarded",
                    "throws a call's value away; the callee is not on the known-fallible list, so this is reported at `warning` at most",
                ),
                "logging-handler" => (
                    "Empty Error Handler Added",
                    "catches an error, logs it, and does nothing else with it: the failure is recorded and dropped",
                ),
                "silenced-error" => (
                    "Error Silenced",
                    "replaces every error it raises with nothing",
                ),
                _ => (
                    "Empty Error Handler Added",
                    "catches an error and does nothing with it",
                ),
            };
            // The syntax tree carries no types: a callee off a pack's known-fallible list
            // may return a plain value, so it never blocks on its own.
            let severity = if site.kind == "discarded-value"
                && settings.severity() == crate::config::Severity::Error
            {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            };
            out.push(
                ctx.overridable(severity),
                title,
                Some(&file.path),
                Some(site.line),
                format!("`{}` {what} in `{}`.", site.snippet, file.path),
                &format!(
                    "Handle or propagate the error, or justify it on its own line in the PR body or a commit message: `allow-swallow: {} <reason>` (or `discipline:allow(error-swallowing)` on the line).",
                    file.path
                ),
            );
        }
    }
    if !unsupported.is_empty() {
        let sample: Vec<&str> = unsupported.iter().take(3).map(String::as_str).collect();
        out.notes.push(format!(
            "{} changed file(s) are in a language whose pack supplies no handler facts and were NOT analysed (e.g. {})",
            unsupported.len(),
            sample.join(", ")
        ));
    }
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
