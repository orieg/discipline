//! Owner ratification of protected-path edits (`ratified-paths`).
//!
//! A change that edits a path matching `protected_paths` passes only when an issue the
//! pull request closes carries a ratification naming that path, written by a listed
//! human login ([`crate::ratification`]). A path matching `never_ratifiable` fails
//! whatever any comment says. No directive lifts this gate: its escape hatch is the
//! ratification, a forge fact that the change's author does not control.
//!
//! The gate reads its policy from the base ref (`--policy-from base`), since otherwise the
//! change could drop a path from `protected_paths` in the same edit. It asks the forge
//! nothing when no protected path changed.

use crate::config::GateSettings;
use crate::could_not_check::{tag, Reason};
use crate::guards::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::ratification::{self, Finding, Input};
use anyhow::{anyhow, Result};

pub const GATE: &str = "ratified-paths";

pub fn evaluate_ratified_paths(ctx: &Context) -> Result<GateOutcome> {
    let cfg = &ctx.config.gates.ratified_paths;
    let mut out = GateOutcome::new(GATE);
    let config_err = |msg: String| tag(Reason::Configuration, anyhow!(msg));
    if cfg.protected_paths.is_empty() {
        return Err(config_err(
            "`gates.ratified-paths` is enabled but `protected_paths` is empty".into(),
        ));
    }
    if cfg.ratifiers.is_empty() {
        return Err(config_err(
            "`gates.ratified-paths` is enabled but `ratifiers` names no one, so nothing could ever be ratified".into(),
        ));
    }
    if let Some(both) = cfg
        .ratifiers
        .iter()
        .find(|r| cfg.agent_logins.iter().any(|a| a.eq_ignore_ascii_case(r)))
    {
        return Err(config_err(format!(
            "`{both}` is in both `ratifiers` and `agent_logins` of `gates.ratified-paths`"
        )));
    }
    let exempt = exempt_filter(cfg)?;
    let protected = PathFilter::new(&cfg.protected_paths)?;
    let never = PathFilter::new(&cfg.never_ratifiable)?;

    // Both sides of a rename or deletion count: moving a protected file away edits it.
    let mut touched = std::collections::BTreeSet::new();
    for f in ctx.git.changed_files()? {
        touched.insert(f.path.clone());
        if !f.old_path.is_empty() {
            touched.insert(f.old_path.clone());
        }
    }
    let mut changed_protected = Vec::new();
    for path in touched.iter().filter(|p| !exempt.matches(p)) {
        if never.matches(path) {
            out.examined += 1;
            out.push(
                cfg.severity(),
                &crate::findings::NEVER_RATIFIABLE_PATH_CHANGED,
                Some(path),
                None,
                format!("`{path}` is never ratifiable, and this change edits it."),
                "Land this edit through the repository owner's own path (an administrator merge), not a ratified pull request.",
            );
        } else if protected.matches(path) {
            out.examined += 1;
            changed_protected.push(path.clone());
        }
    }
    if changed_protected.is_empty() {
        out.notes
            .push("no protected path changed; nothing needs a ratification".into());
        return Ok(out);
    }

    if ctx.head_config.is_none() {
        return Err(config_err(
            "`gates.ratified-paths` must be judged by the base ref's policy (`--policy-from base`, action input `policy_from: base`): the change edits a protected path and could otherwise loosen the policy that judges it".into(),
        ));
    }
    let Some(access) = ctx.pull_request_for(GATE)? else {
        // A local run (hook, pre-commit, a developer's `check`) or a push has no pull
        // request to read; a direct push is for branch protection to refuse.
        out.examined -= changed_protected.len();
        out.notes.push(format!(
            "{} protected path(s) changed ({}); their ratification is read from the pull request's closing issues, and this run has no pull request, so it was not checked",
            changed_protected.len(),
            changed_protected.join(", ")
        ));
        return Ok(out);
    };
    let pull = access
        .pull
        .as_ref()
        .expect("pull_request_for returns a pull request");
    let forge = (access.identify)().map_err(|e| {
        tag(
            Reason::Forge,
            anyhow!("ratified-paths cannot identify the forge: {e}"),
        )
    })?;
    let last_change = |p: &str| ctx.git.last_change_on_base(p);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let judgement = ratification::judge(
        access.api,
        &forge,
        cfg,
        &Input {
            pull_number: pull.number,
            pull_body: ctx.pr_body.as_deref().unwrap_or(""),
            protected: &changed_protected,
            never_ratifiable: &never,
            last_change: &last_change,
            now,
        },
    )?;
    out.notes.extend(judgement.notes);
    let marker = &cfg.marker;
    for f in judgement.findings {
        match f {
            Finding::Unratified { path, why } => out.push(
                cfg.severity(),
                &crate::findings::PROTECTED_PATH_UNRATIFIED,
                Some(&path),
                None,
                format!("`{path}` is protected and has no owner ratification: {why}."),
                &format!(
                    "Close an issue on which a listed ratifier comments `{marker}` with `- {path}` on its own line, then re-run the check."
                ),
            ),
            Finding::EntryMalformed { anchor, entry, why } => {
                out.push(
                    cfg.severity(),
                    &crate::findings::RATIFICATION_ENTRY_MALFORMED,
                    None,
                    None,
                    format!("Ratification entry `{entry}` in {anchor} is refused: {why}. The whole block is void."),
                    "Post a new ratification comment that names each path exactly (no globs, no leading or trailing `/`, no `..`).",
                );
                out.anchor_last(format!("{anchor}:{entry}"));
            }
            Finding::NamesNeverRatifiable { anchor, entry } => {
                out.push(
                    cfg.severity(),
                    &crate::findings::RATIFICATION_NAMES_NEVER_RATIFIABLE_PATH,
                    None,
                    None,
                    format!("{anchor} ratifies `{entry}`, which is never ratifiable; that entry is ignored."),
                    "Remove the entry: a never-ratifiable path is changed through the owner's own path.",
                );
                out.anchor_last(format!("{anchor}:{entry}"));
            }
            Finding::AuthorNotAccepted { anchor, author } => {
                out.push(
                    crate::config::Severity::Note,
                    &crate::findings::RATIFICATION_AUTHOR_NOT_ACCEPTED,
                    None,
                    None,
                    format!("The ratification block in {anchor} is by `{author}`, who is not a listed ratifier (or is an agent or bot login); it does not count."),
                    "A login in `ratifiers` (and not in `agent_logins`) must post the ratification.",
                );
                out.anchor_last(anchor);
            }
            Finding::CommentRefused { anchor, why } => {
                out.push(
                    crate::config::Severity::Note,
                    &crate::findings::RATIFICATION_COMMENT_EDITED,
                    None,
                    None,
                    format!("The ratification block in {anchor} does not count: {why}."),
                    "Post the ratification as a new comment rather than editing an old one.",
                );
                out.anchor_last(anchor);
            }
            Finding::OutsideWindow { anchor, path, why } => {
                out.push(
                    crate::config::Severity::Note,
                    &crate::findings::RATIFICATION_OUTSIDE_WINDOW,
                    None,
                    None,
                    format!("The ratification of `{path}` in {anchor} no longer counts: {why}."),
                    "Post a new ratification for this path.",
                );
                out.anchor_last(format!("{anchor}:{path}"));
            }
        }
    }
    Ok(out)
}
