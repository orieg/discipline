//! `agents-md`: `AGENTS.md` exists and its aliases do not fork it.

use crate::config::GateSettings;
use crate::guards::{exempt_filter, Context, GateOutcome};
use anyhow::Result;

pub fn evaluate_agents_guide(
    has_agents_md: bool,
    canonical_content: Option<&str>,
    aliases: &[(&str, bool, bool, Option<&str>)],
    settings: &crate::config::AgentsMdGate,
) -> Result<GateOutcome> {
    const GATE: &str = "agents-md";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = 1;

    if !has_agents_md {
        out.push(
            settings.severity(),
            &crate::findings::AGENTS_MD_MISSING,
            Some("AGENTS.md"),
            None,
            "The repository tracks no canonical AGENTS.md to govern AI agent behavior.".to_string(),
            "Create AGENTS.md and symlink CLAUDE.md / GEMINI.md to it.",
        );
        return Ok(out);
    }

    for (alias, is_tracked, is_symlink, head_content) in aliases {
        if exempt.matches(alias) || !*is_tracked || *is_symlink {
            continue;
        }
        out.examined += 1;
        if *head_content != canonical_content {
            out.push(
                settings.severity(),
                &crate::findings::AGENT_GUIDE_FORKED,
                Some(alias),
                None,
                format!("`{alias}` is a regular file whose content differs from AGENTS.md."),
                "Replace it with a symlink to AGENTS.md so agents read one set of rules.",
            );
        }
    }
    Ok(out)
}

pub fn agents_md(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.agents_md;
    let has_agents_md = ctx.git.is_tracked("AGENTS.md")?;
    let canonical = if has_agents_md {
        ctx.git.head_content("AGENTS.md")?
    } else {
        None
    };
    let mut aliases = Vec::new();
    for alias in ["CLAUDE.md", "GEMINI.md"] {
        let tracked = ctx.git.is_tracked(alias)?;
        let symlink = if tracked {
            ctx.git.is_symlink(alias)?
        } else {
            false
        };
        let content = if tracked && !symlink {
            ctx.git.head_content(alias)?
        } else {
            None
        };
        aliases.push((alias, tracked, symlink, content));
    }
    let alias_refs: Vec<(&str, bool, bool, Option<&str>)> = aliases
        .iter()
        .map(|(a, t, s, c)| (*a, *t, *s, c.as_deref()))
        .collect();
    evaluate_agents_guide(has_agents_md, canonical.as_deref(), &alias_refs, settings)
}
