//! `discipline explain <gate-id>`: what a gate checks, its state here, and what lifts it.
//!
//! Human output: unlike the agent-facing surfaces (`agent-prompt`, `hook run`,
//! `discipline mcp`), it names the directive that waives a finding, because a
//! person deciding whether to write one needs its exact form.

use crate::config::{GateInfo, Severity, GATES};
use crate::tokens::DIRECTIVE_SPECS;

/// The gate a query names: an exact id, a finding line containing `[gate-id]`, or
/// text containing an id (the longest match wins).
pub fn gate_for(query: &str) -> Option<&'static GateInfo> {
    let q = query.trim();
    if let Some(g) = GATES.iter().find(|g| g.id == q) {
        return Some(g);
    }
    let longest = |pred: &dyn Fn(&str) -> bool| {
        GATES
            .iter()
            .filter(|g| pred(g.id))
            .max_by_key(|g| g.id.len())
    };
    longest(&|id| q.contains(&format!("[{id}]"))).or_else(|| longest(&|id| q.contains(id)))
}

/// Gate ids that share a word with `query`, for an unknown query.
pub fn suggestions(query: &str) -> Vec<&'static str> {
    let words: Vec<String> = query
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect();
    GATES
        .iter()
        .filter(|g| words.iter().any(|w| g.id.contains(w.as_str())))
        .map(|g| g.id)
        .collect()
}

/// The explanation. `state` is the gate's `(enabled, severity)` under the
/// repository's configuration, when it has settings.
pub fn render(g: &GateInfo, state: Option<(bool, Severity)>) -> String {
    let mut out = format!("{} ({})\n", g.id, g.suite.label());
    out.push_str(&format!("  Checks:      {}\n", g.summary));
    out.push_str(&format!("  Languages:   {}\n", g.languages));
    let here = match (g.available, state) {
        (false, _) => "planned, not in this binary".to_string(),
        (true, Some((true, sev))) => format!("on, {sev}"),
        (true, Some((false, sev))) => format!("off ({sev} when enabled)"),
        (true, None) => "on".to_string(),
    };
    out.push_str(&format!("  Here:        {here}\n"));
    let specs: Vec<_> = DIRECTIVE_SPECS.iter().filter(|s| s.gate == g.id).collect();
    if specs.is_empty() {
        out.push_str("  Lifted by:   no directive; change the code, or change the gate in discipline.toml (a config-integrity weakening)\n");
    } else {
        for (i, s) in specs.iter().enumerate() {
            let label = if i == 0 {
                "  Lifted by:  "
            } else {
                "              "
            };
            out.push_str(&format!(
                "{label} `{}: <subject> <reason>` on its own line in the PR body or a commit message; <subject>: {}\n",
                s.canonical, s.subject_doc
            ));
        }
    }
    out.push_str(&format!(
        "  Reference:   https://orieg.github.io/discipline/gates/#{}\n",
        g.id
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gate_resolves_and_renders_its_reference() {
        for g in GATES {
            assert_eq!(gate_for(g.id).unwrap().id, g.id);
            let t = render(g, Some((true, Severity::Error)));
            assert!(t.starts_with(g.id) && t.contains(g.summary), "{t}");
            assert!(t.contains(&format!("gates/#{}", g.id)), "{t}");
        }
    }

    #[test]
    fn a_gate_with_a_directive_names_it_and_one_without_says_so() {
        let ar = gate_for("assertion-reduction").unwrap();
        let t = render(ar, Some((true, Severity::Error)));
        assert!(
            t.contains("`allow-assertion-drop: <subject> <reason>`"),
            "{t}"
        );
        assert!(t.contains("Here:        on, error"), "{t}");
        let off = render(ar, Some((false, Severity::Warning)));
        assert!(off.contains("off (warning when enabled)"), "{off}");
        let bare = GATES
            .iter()
            .find(|g| g.available && !DIRECTIVE_SPECS.iter().any(|s| s.gate == g.id))
            .expect("some gate has no directive");
        assert!(render(bare, None).contains("no directive"));
    }

    #[test]
    fn a_finding_line_resolves_and_an_unknown_query_suggests() {
        assert_eq!(
            gate_for("error [error-swallowing] Result Discarded")
                .unwrap()
                .id,
            "error-swallowing"
        );
        // The bracketed id wins over a longer id elsewhere in the line.
        assert_eq!(
            gate_for("[pii] a host in the ci-integrity notes")
                .unwrap()
                .id,
            "pii"
        );
        assert!(gate_for("zzz").is_none());
        assert!(suggestions("swallow").contains(&"error-swallowing"));
    }
}
