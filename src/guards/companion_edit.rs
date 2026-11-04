//! Companion edits (`companion-edit`): "a change to path set P must also change path set Q".
//!
//! Each `[[gates.companion-edit.rules]]` entry is one covenant, judged on its own: a
//! changed path matching any `trigger_paths` glob fires the rule, and a fired rule needs a
//! changed path matching one `require_edits_to` glob, or every one with `require_all`.
//!
//! What counts as a change:
//! - A rule fires on either side of a change: an added, modified or deleted path, and both
//!   the old and the new path of a rename. Deleting a mechanism file changes it.
//! - A requirement is met only by a path the head still has: added, modified, or the new
//!   side of a rename. Deleting the companion is not editing it.
//! - Without `require_all`, a path is never its own companion: a rule fired by one path
//!   alone is not met by that same path, so a mirror set written without `require_all`
//!   still needs a second member to move. With `require_all`, the firing path counts
//!   toward "every listed path changed".
//! - `exempt_paths` drops a path from the change before any rule reads it: it neither
//!   fires a rule nor meets one.

use crate::config::{CompanionEditRule, GateSettings};
use crate::could_not_check::{tag, Reason};
use crate::guards::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::{anyhow, Result};
use std::collections::BTreeSet;

pub const GATE: &str = "companion-edit";

/// One rule's globs, compiled.
pub struct CompiledRule {
    trigger: PathFilter,
    /// One filter per `require_edits_to` entry, in order, so `require_all` can tell which
    /// entry no changed path matched.
    required: Vec<(String, PathFilter)>,
    require_all: bool,
}

impl CompiledRule {
    /// Compiles rule `index`, refusing what could never fire or never be met: an empty
    /// list, a blank entry, a glob that does not compile.
    pub fn new(rule: &CompanionEditRule, index: usize) -> Result<Self> {
        let key = |field: &str| format!("gates.{GATE}.rules[{index}].{field}");
        let config_err = |msg: String| tag(Reason::Configuration, anyhow!(msg));
        for (field, globs) in [
            ("trigger_paths", &rule.trigger_paths),
            ("require_edits_to", &rule.require_edits_to),
        ] {
            if globs.is_empty() {
                return Err(config_err(format!(
                    "`{}` is empty: the rule could never {}",
                    key(field),
                    if field == "trigger_paths" {
                        "fire"
                    } else {
                        "be met"
                    }
                )));
            }
            if globs.iter().any(|g| g.trim().is_empty()) {
                return Err(config_err(format!(
                    "`{}` holds a blank entry, which matches no path",
                    key(field)
                )));
            }
        }
        let compile = |field: &str, globs: &[String]| {
            PathFilter::new(globs).map_err(|e| config_err(format!("`{}`: {e:#}", key(field))))
        };
        let trigger = compile("trigger_paths", &rule.trigger_paths)?;
        let mut required = Vec::new();
        for glob in &rule.require_edits_to {
            required.push((
                glob.clone(),
                compile("require_edits_to", std::slice::from_ref(glob))?,
            ));
        }
        Ok(Self {
            trigger,
            required,
            require_all: rule.require_all,
        })
    }

    /// Judges one rule against the change: `touched` is every path either side of the
    /// change names, `edited` every path the head side still has.
    pub fn judge(&self, touched: &BTreeSet<String>, edited: &BTreeSet<String>) -> Verdict {
        let fired: Vec<String> = touched
            .iter()
            .filter(|p| self.trigger.matches(p))
            .cloned()
            .collect();
        if fired.is_empty() {
            return Verdict::NotFired;
        }
        if self.require_all {
            let missing: Vec<String> = self
                .required
                .iter()
                .filter(|(_, f)| !edited.iter().any(|p| f.matches(p)))
                .map(|(g, _)| g.clone())
                .collect();
            return if missing.is_empty() {
                Verdict::Met
            } else {
                Verdict::Missing { fired, missing }
            };
        }
        let companions: Vec<&String> = edited
            .iter()
            .filter(|p| self.required.iter().any(|(_, f)| f.matches(p)))
            .collect();
        // A companion must differ from at least one firing path: a path is not its own
        // companion.
        if companions.iter().any(|c| fired.iter().any(|t| t != *c)) {
            Verdict::Met
        } else if companions.is_empty() {
            Verdict::NoneEdited { fired }
        } else {
            Verdict::OnlyItself { fired }
        }
    }
}

/// What one rule makes of the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No changed path matches `trigger_paths`.
    NotFired,
    /// The rule fired and its requirement is met.
    Met,
    /// Fired; no changed path the head still has matches `require_edits_to`.
    NoneEdited { fired: Vec<String> },
    /// Fired by one path that is also the only changed required path.
    OnlyItself { fired: Vec<String> },
    /// `require_all`: fired, and these `require_edits_to` entries matched no changed path.
    Missing {
        fired: Vec<String>,
        missing: Vec<String>,
    },
}

pub fn evaluate_companion_edit(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.companion_edit;
    let mut out = GateOutcome::new(GATE);
    if settings.rules.is_empty() {
        return Err(tag(
            Reason::Configuration,
            anyhow!("`gates.{GATE}` is enabled but no rules are configured"),
        ));
    }
    let rules = settings
        .rules
        .iter()
        .enumerate()
        .map(|(i, r)| CompiledRule::new(r, i))
        .collect::<Result<Vec<_>>>()?;
    let exempt = exempt_filter(settings)?;

    let mut touched = BTreeSet::new();
    let mut edited = BTreeSet::new();
    for f in ctx.git.changed_files()? {
        if !f.is_deleted() && !exempt.matches(&f.path) {
            edited.insert(f.path.clone());
        }
        for side in [&f.path, &f.old_path] {
            if !side.is_empty() && !exempt.matches(side) {
                touched.insert(side.clone());
            }
        }
    }
    if touched.is_empty() {
        out.notes
            .push("the change touches no path outside `exempt_paths`; no rule can fire".into());
        return Ok(out);
    }

    for (position, (rule, compiled)) in settings.rules.iter().zip(&rules).enumerate() {
        let verdict = compiled.judge(&touched, &edited);
        let (fired, what) = match verdict {
            Verdict::NotFired => continue,
            Verdict::Met => {
                out.examined += 1;
                continue;
            }
            Verdict::NoneEdited { fired } => (
                fired,
                format!(
                    "edits no path matching `require_edits_to` ({})",
                    list(&rule.require_edits_to)
                ),
            ),
            Verdict::OnlyItself { fired } => (
                fired,
                format!(
                    "edits no other path matching `require_edits_to` ({}): a path is not its own companion",
                    list(&rule.require_edits_to)
                ),
            ),
            Verdict::Missing { fired, missing } => (
                fired,
                format!(
                    "does not edit {} (`require_all` asks for every one of {})",
                    list(&missing),
                    list(&rule.require_edits_to)
                ),
            ),
        };
        out.examined += 1;
        let subject = &rule.require_edits_to[0];
        let reason = rule
            .reason
            .as_deref()
            .map(|r| format!(" Reason: {r}."))
            .unwrap_or_default();
        let lifted = ctx.find_override(
            GATE,
            &crate::findings::COMPANION_NOT_EDITED,
            tokens::ALLOW_COMPANION_EDIT,
            subject,
        );
        out.lift_or_push(
            lifted,
            ctx.overridable(settings.severity()),
            &crate::findings::COMPANION_NOT_EDITED,
            (Some(subject), None),
            format!(
                "This change touches {} (rule {position}, `trigger_paths` {}) and {what}.{reason}",
                list(&fired),
                list(&rule.trigger_paths),
            ),
            &format!(
                "Edit the companion path(s) in this change, or justify with `allow-companion-edit: {subject} <reason>`."
            ),
        );
        if let Some(anchor) = later_rule_anchor(&settings.rules, position) {
            out.anchor_last(anchor);
        }
    }
    if out.examined == 0 {
        out.notes
            .push("no changed path matches any rule's `trigger_paths`".into());
    }
    Ok(out)
}

fn list(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What tells the finding of the rule at `position` from that of an earlier rule whose
/// first `require_edits_to` entry is the same (the finding's path): a hash of the rule's
/// globs. `None` for the first such rule. The hash, not the position, so a rule added or
/// removed elsewhere in the list changes nothing.
pub(crate) fn later_rule_anchor(rules: &[CompanionEditRule], position: usize) -> Option<String> {
    let rule = &rules[position];
    if !rules[..position]
        .iter()
        .any(|r| r.require_edits_to.first() == rule.require_edits_to.first())
    {
        return None;
    }
    let mut parts = vec!["trigger"];
    parts.extend(rule.trigger_paths.iter().map(String::as_str));
    parts.push("require");
    parts.extend(rule.require_edits_to.iter().map(String::as_str));
    parts.push(if rule.require_all { "all" } else { "any" });
    Some(format!(
        "rule:{}",
        crate::report::gitlab::sha256_hex(parts.join("\u{1f}").as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(trigger: &[&str], required: &[&str], require_all: bool) -> CompanionEditRule {
        CompanionEditRule {
            trigger_paths: trigger.iter().map(|s| s.to_string()).collect(),
            require_edits_to: required.iter().map(|s| s.to_string()).collect(),
            require_all,
            reason: None,
        }
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn judge(r: &CompanionEditRule, touched: &[&str], edited: &[&str]) -> Verdict {
        CompiledRule::new(r, 0)
            .unwrap()
            .judge(&set(touched), &set(edited))
    }

    #[test]
    fn a_trigger_without_its_companion_is_reported_and_with_it_is_met() {
        let r = rule(&["argo/**"], &["docs/threat-model.md"], false);
        let fired = vec!["argo/wf.yaml".to_string()];
        assert_eq!(
            judge(&r, &["argo/wf.yaml"], &["argo/wf.yaml"]),
            Verdict::NoneEdited {
                fired: fired.clone()
            }
        );
        assert_eq!(
            judge(
                &r,
                &["argo/wf.yaml", "docs/threat-model.md"],
                &["argo/wf.yaml", "docs/threat-model.md"]
            ),
            Verdict::Met
        );
        assert_eq!(
            judge(&r, &["src/lib.rs"], &["src/lib.rs"]),
            Verdict::NotFired
        );
    }

    #[test]
    fn any_one_required_path_meets_the_rule_and_triggers_are_ored() {
        let r = rule(&["test/**", "Makefile"], &["docs/a.md", "docs/b.md"], false);
        assert_eq!(
            judge(&r, &["Makefile", "docs/b.md"], &["Makefile", "docs/b.md"]),
            Verdict::Met
        );
        assert_eq!(
            judge(&r, &["Makefile"], &["Makefile"]),
            Verdict::NoneEdited {
                fired: vec!["Makefile".into()]
            }
        );
    }

    #[test]
    fn a_path_is_not_its_own_companion_without_require_all() {
        let mirror = ["CLAUDE.md", "AGENTS.md", "GEMINI.md"];
        let r = rule(&mirror, &mirror, false);
        assert_eq!(
            judge(&r, &["CLAUDE.md"], &["CLAUDE.md"]),
            Verdict::OnlyItself {
                fired: vec!["CLAUDE.md".into()]
            }
        );
        assert_eq!(
            judge(&r, &["AGENTS.md", "CLAUDE.md"], &["AGENTS.md", "CLAUDE.md"]),
            Verdict::Met
        );
        // A specification inside its own trigger set, edited alone, is not met either.
        let r = rule(&["argo/**"], &["argo/README.md"], false);
        assert!(matches!(
            judge(&r, &["argo/README.md"], &["argo/README.md"]),
            Verdict::OnlyItself { .. }
        ));
        assert_eq!(
            judge(
                &r,
                &["argo/README.md", "argo/wf.yaml"],
                &["argo/README.md", "argo/wf.yaml"]
            ),
            Verdict::Met
        );
    }

    #[test]
    fn require_all_names_each_missing_path_and_counts_the_firing_one() {
        let mirror = ["CLAUDE.md", "AGENTS.md", "GEMINI.md"];
        let r = rule(&mirror, &mirror, true);
        assert_eq!(
            judge(&r, &["AGENTS.md", "CLAUDE.md"], &["AGENTS.md", "CLAUDE.md"]),
            Verdict::Missing {
                fired: vec!["AGENTS.md".into(), "CLAUDE.md".into()],
                missing: vec!["GEMINI.md".into()],
            }
        );
        let all = ["AGENTS.md", "CLAUDE.md", "GEMINI.md"];
        assert_eq!(judge(&r, &all, &all), Verdict::Met);
    }

    #[test]
    fn a_deleted_companion_does_not_meet_the_rule_but_a_deleted_trigger_fires_it() {
        let r = rule(&["argo/**"], &["docs/threat-model.md"], false);
        // `docs/threat-model.md` deleted: touched, not edited.
        assert!(matches!(
            judge(
                &r,
                &["argo/wf.yaml", "docs/threat-model.md"],
                &["argo/wf.yaml"]
            ),
            Verdict::NoneEdited { .. }
        ));
        // `argo/wf.yaml` deleted: the rule fires.
        assert!(matches!(
            judge(&r, &["argo/wf.yaml"], &[]),
            Verdict::NoneEdited { .. }
        ));
    }

    #[test]
    fn a_rule_that_could_never_fire_or_be_met_is_refused() {
        for (r, key) in [
            (rule(&[], &["docs/a.md"], false), "rules[3].trigger_paths"),
            (rule(&["a/**"], &[], false), "rules[3].require_edits_to"),
            (rule(&["a/**"], &[" "], false), "rules[3].require_edits_to"),
            (
                rule(&["src/[a-z"], &["docs/a.md"], false),
                "rules[3].trigger_paths",
            ),
            (
                rule(&["a/**"], &["docs/[x"], true),
                "rules[3].require_edits_to",
            ),
        ] {
            let err = CompiledRule::new(&r, 3).err().expect("refused");
            assert!(format!("{err:#}").contains(key), "{err:#}");
        }
        assert!(CompiledRule::new(&rule(&["a/**"], &["docs/a.md"], false), 0).is_ok());
    }

    #[test]
    fn a_later_rule_with_the_same_first_requirement_is_anchored_on_its_globs() {
        let rules = vec![
            rule(&["a/**"], &["docs/x.md"], false),
            rule(&["b/**"], &["docs/y.md"], false),
            rule(&["c/**"], &["docs/x.md"], false),
        ];
        assert_eq!(later_rule_anchor(&rules, 0), None);
        assert_eq!(later_rule_anchor(&rules, 1), None);
        assert!(later_rule_anchor(&rules, 2).is_some_and(|a| a.starts_with("rule:")));
    }
}
