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
//! - `require_added_line_match` reads the content of a met rule's companion change: every
//!   listed pattern must match some line the change adds to a companion path (a path the
//!   head has that matches `require_edits_to`). Context lines, removed lines and other
//!   files do not count, so a whitespace-only touch, or a matching line moved or deleted,
//!   does not meet it.

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
    /// `require_added_line_match`, compiled: every pattern must match an added companion
    /// line.
    line_patterns: Vec<(String, regex::Regex)>,
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
        let mut line_patterns = Vec::new();
        if let Some(patterns) = &rule.require_added_line_match {
            let field = key("require_added_line_match");
            if patterns.is_empty() {
                return Err(config_err(format!(
                    "`{field}` is an empty list, which asks nothing: remove the key or list a pattern"
                )));
            }
            for pattern in patterns {
                if pattern.trim().is_empty() {
                    return Err(config_err(format!(
                        "`{field}` holds a blank pattern, which every line matches"
                    )));
                }
                let re = regex::Regex::new(pattern).map_err(|e| {
                    config_err(format!(
                        "`{field}`: `{pattern}` is not a regular expression: {e}"
                    ))
                })?;
                // A pattern a blank line matches is met by a whitespace-only edit: the
                // very edit the key exists to refuse.
                if ["", " ", "\t"].iter().any(|blank| re.is_match(blank)) {
                    return Err(config_err(format!(
                        "`{field}`: `{pattern}` matches an empty or whitespace-only line, so a whitespace-only edit would meet it"
                    )));
                }
                line_patterns.push((pattern.clone(), re));
            }
        }
        Ok(Self {
            trigger,
            required,
            require_all: rule.require_all,
            line_patterns,
        })
    }

    /// Whether `path` is a companion of this rule: it matches a `require_edits_to` glob.
    fn is_companion(&self, path: &str) -> bool {
        self.required.iter().any(|(_, f)| f.matches(path))
    }

    /// `require_added_line_match`: the patterns no line in `added` matches, in the order
    /// configured. Empty when the rule sets none, or every one matched some line.
    pub fn missing_line_patterns<'a>(
        &self,
        added: impl IntoIterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut unmet: Vec<&(String, regex::Regex)> = self.line_patterns.iter().collect();
        for line in added {
            if unmet.is_empty() {
                break;
            }
            unmet.retain(|(_, re)| !re.is_match(line));
        }
        unmet.into_iter().map(|(p, _)| p.clone()).collect()
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
    let changed = ctx.git.changed_files()?;
    for f in changed.iter().cloned() {
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
                if !compiled.line_patterns.is_empty() {
                    judge_added_lines(ctx, rule, compiled, position, &changed, &exempt, &mut out)?;
                }
                if rule.require_mention {
                    judge_mention(ctx, rule, compiled, position, &changed, &exempt, &mut out)?;
                }
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

/// The forms in which a changed public name counts as named: the qualified name, and its
/// last segment (`get` for `Store::get`, `Cache#get`, `-[Store get:]`).
pub fn mention_forms(name: &str) -> Vec<String> {
    let mut forms = vec![name.to_string()];
    let last = name
        .trim_start_matches(['-', '+'])
        .trim_matches(['[', ']'])
        .rsplit(['.', ':', '#', ' '])
        .find(|seg| !seg.is_empty())
        .unwrap_or(name)
        .to_string();
    if !last.is_empty() && last != name {
        forms.push(last);
    }
    forms
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `line` holds `form` as a whole word: neither neighbour continues an identifier.
pub fn names(line: &str, form: &str) -> bool {
    line.match_indices(form).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + form.len()..].chars().next();
        !before.is_some_and(word_char) && !after.is_some_and(word_char)
    })
}

/// Files a mention never counts in: a changelog lists changes without describing them.
const CHANGELOG: &[&str] = &["**/CHANGELOG*", "**/CHANGES*", "**/HISTORY*", "**/NEWS*"];

/// `require_mention`: the rule fired and its companion path changed; the companion's added
/// prose must name a public name the triggering paths added, removed or modified.
fn judge_mention(
    ctx: &Context,
    rule: &CompanionEditRule,
    compiled: &CompiledRule,
    position: usize,
    changed: &[crate::gitctx::ChangedFile],
    exempt: &PathFilter,
    out: &mut GateOutcome,
) -> Result<()> {
    use crate::ast::public_names::{changed_public_names, FileChange};
    let changelog = PathFilter::new(&CHANGELOG.iter().map(|s| s.to_string()).collect::<Vec<_>>())?;
    let fires = |p: &str| !p.is_empty() && !exempt.matches(p) && compiled.trigger.matches(p);
    let companion =
        |p: &str| !exempt.matches(p) && compiled.required.iter().any(|(_, f)| f.matches(p));
    let mut texts = Vec::new();
    for f in changed {
        if !(fires(&f.path) || fires(&f.old_path)) || companion(&f.path) {
            continue;
        }
        let base = if f.kind == crate::gitctx::ChangeKind::Added {
            None
        } else {
            ctx.git.base_content(&f.old_path)?
        };
        let head = if f.is_deleted() {
            None
        } else {
            ctx.git.head_content(&f.path)?
        };
        texts.push((f, base, head));
    }
    let files: Vec<FileChange> = texts
        .iter()
        .map(|(f, base, head)| FileChange {
            base_path: if f.old_path.is_empty() {
                &f.path
            } else {
                &f.old_path
            },
            head_path: &f.path,
            base: base.as_deref(),
            head: head.as_deref(),
        })
        .collect();
    let vocab = super::agent_diff::assert_vocabulary(ctx.config);
    let changes = changed_public_names(&crate::ast::default_registry(), &vocab, &files);
    let names: Vec<&crate::ast::public_names::ChangedName> = changes
        .added
        .iter()
        .chain(&changes.removed)
        .chain(&changes.modified)
        .collect();
    let unread: Vec<String> = changes
        .not_checked
        .iter()
        .map(|(p, _)| format!("`{p}`"))
        .collect();
    if names.is_empty() {
        out.notes.push(if unread.is_empty() {
            format!("rule {position}: the triggering change adds, removes or modifies no public name; `require_mention` asks nothing of the companion")
        } else {
            format!(
                "rule {position}: could not check `require_mention`: no public name read, and the public names of {} could not be read",
                unread.join(", ")
            )
        });
        return Ok(());
    }
    if !unread.is_empty() {
        out.notes.push(format!(
            "rule {position}: `require_mention` judged on the public names read; those of {} could not be read",
            unread.join(", ")
        ));
    }

    // The companion's added lines, as prose: fenced code and HTML comments are not read.
    let mut companions = Vec::new();
    let mut found = false;
    for f in changed {
        if f.is_deleted() || !companion(&f.path) || changelog.matches(&f.path) {
            continue;
        }
        companions.push(f.path.clone());
        let Some(text) = ctx.git.head_content(&f.path)? else {
            continue;
        };
        found |= crate::guards::pr_checklist::readable_lines(&text)
            .iter()
            .filter(|(n, _)| f.added_lines.contains(n))
            .any(|(_, line)| {
                names.iter().any(|c| {
                    mention_forms(&c.name)
                        .iter()
                        .any(|form| self::names(line, form))
                })
            });
        if found {
            break;
        }
    }
    if found {
        return Ok(());
    }
    let subject = &rule.require_edits_to[0];
    let at = companions.first().unwrap_or(subject);
    let mut shown: Vec<String> = names
        .iter()
        .take(5)
        .map(|c| format!("`{}`", c.name))
        .collect();
    if names.len() > 5 {
        shown.push(format!("and {} more", names.len() - 5));
    }
    let lifted = ctx.find_override(
        GATE,
        &crate::findings::COMPANION_NOT_SPECIFIC,
        tokens::ALLOW_COMPANION_EDIT,
        subject,
    );
    out.lift_or_push(
        lifted,
        ctx.overridable(rule_severity(ctx)),
        &crate::findings::COMPANION_NOT_SPECIFIC,
        (Some(at), None),
        format!(
            "Rule {position} (`require_mention`): the companion edit names none of the public names the change touches in `trigger_paths` ({}). Lines in fenced code, HTML comments and changelog files do not count.",
            shown.join(", ")
        ),
        &format!(
            "Describe the change in `{at}`, naming what changed, or justify with `allow-companion-edit: {subject} <reason>`."
        ),
    );
    if let Some(anchor) = later_rule_anchor(&ctx.config.gates.companion_edit.rules, position) {
        out.anchor_last(anchor);
    }
    Ok(())
}

/// `require_added_line_match`: the rule fired and its companion path changed; every
/// pattern must match a line the change adds to a companion path.
fn judge_added_lines(
    ctx: &Context,
    rule: &CompanionEditRule,
    compiled: &CompiledRule,
    position: usize,
    changed: &[crate::gitctx::ChangedFile],
    exempt: &PathFilter,
    out: &mut GateOutcome,
) -> Result<()> {
    let mut companions = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for f in changed {
        if f.is_deleted() || exempt.matches(&f.path) || !compiled.is_companion(&f.path) {
            continue;
        }
        companions.push(f.path.clone());
        if f.added_lines.is_empty() {
            continue;
        }
        let Some(text) = ctx.git.head_content(&f.path)? else {
            out.notes.push(format!(
                "rule {position}: `require_added_line_match` could not read `{}` as text; its lines do not count",
                f.path
            ));
            continue;
        };
        added.extend(
            text.lines()
                .enumerate()
                .filter(|(i, _)| f.added_lines.contains(&(i + 1)))
                .map(|(_, l)| l.to_string()),
        );
    }
    let missing = compiled.missing_line_patterns(added.iter().map(String::as_str));
    if missing.is_empty() {
        return Ok(());
    }
    let subject = &rule.require_edits_to[0];
    let at = companions.first().unwrap_or(subject);
    let lifted = ctx.find_override(
        GATE,
        &crate::findings::COMPANION_LINE_MISSING,
        tokens::ALLOW_COMPANION_EDIT,
        subject,
    );
    out.lift_or_push(
        lifted,
        ctx.overridable(rule_severity(ctx)),
        &crate::findings::COMPANION_LINE_MISSING,
        (Some(at), None),
        format!(
            "Rule {position} (`require_added_line_match`): the companion edit adds no line matching {} ({} added line(s) read in {}). Every listed pattern must match a line the change adds; context and removed lines do not count.",
            list(&missing),
            added.len(),
            list(&companions),
        ),
        &format!(
            "Add the required entry to `{at}`, or justify with `allow-companion-edit: {subject} <reason>`."
        ),
    );
    if let Some(anchor) = later_rule_anchor(&ctx.config.gates.companion_edit.rules, position) {
        out.anchor_last(anchor);
    }
    Ok(())
}

fn rule_severity(ctx: &Context) -> crate::config::Severity {
    ctx.config.gates.companion_edit.severity()
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
            require_mention: false,
            require_added_line_match: None,
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
    fn a_name_is_mentioned_as_a_whole_word_in_its_qualified_or_short_form() {
        assert_eq!(mention_forms("Store::get"), ["Store::get", "get"]);
        assert_eq!(mention_forms("Cache#get"), ["Cache#get", "get"]);
        assert_eq!(mention_forms("-[Store get:]"), ["-[Store get:]", "get"]);
        assert_eq!(mention_forms("open"), ["open"]);
        assert!(names("Adds `Store::get` for lookups.", "Store::get"));
        assert!(names("Call get() to read.", "get"));
        assert!(names("| `limit` | the cap |", "limit"));
        // Not inside a longer identifier.
        assert!(!names("Call getter() to read.", "get"));
        assert!(!names("my_limit is new", "limit"));
        assert!(!names("nothing here", "get"));
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

    fn with_lines(patterns: &[&str]) -> CompanionEditRule {
        let mut r = rule(&["src/**"], &["docs/REVIEW-LOG.md"], false);
        r.require_added_line_match = Some(patterns.iter().map(|s| s.to_string()).collect());
        r
    }

    #[test]
    fn require_added_line_match_needs_an_added_line_matching_every_pattern() {
        let dated = r"^## \d{4}-\d{2}-\d{2} ";
        let c = CompiledRule::new(&with_lines(&[dated]), 0).unwrap();
        // A whitespace-only edit adds blank lines: unmet.
        assert_eq!(c.missing_line_patterns(["", "   ", "\t"]), [dated]);
        // An added line that does not match: unmet.
        assert_eq!(c.missing_line_patterns(["## Notes on 2026-11-24"]), [dated]);
        // No added line at all (a removed matching line contributes nothing): unmet.
        assert_eq!(c.missing_line_patterns([]), [dated]);
        // An added dated entry: met.
        assert!(c
            .missing_line_patterns(["", "## 2026-11-24 quarterly review"])
            .is_empty());

        // A list asks for every pattern, each on some added line, not one line for all.
        let c = CompiledRule::new(&with_lines(&[dated, "^Reviewer: "]), 0).unwrap();
        assert_eq!(
            c.missing_line_patterns(["## 2026-11-24 review"]),
            ["^Reviewer: "]
        );
        assert!(c
            .missing_line_patterns(["## 2026-11-24 review", "Reviewer: a"])
            .is_empty());
        // A rule without the key asks nothing.
        let c = CompiledRule::new(&rule(&["a/**"], &["b.md"], false), 0).unwrap();
        assert!(c.missing_line_patterns([]).is_empty());
    }

    #[test]
    fn a_line_pattern_that_cannot_discriminate_is_refused() {
        for (patterns, why) in [
            (vec![], "empty list"),
            (vec![" "], "blank pattern"),
            (vec!["^## ("], "not a regular expression"),
            (vec![".*"], "whitespace-only"),
            (vec![r"^\s*$"], "whitespace-only"),
            (vec!["^## ", "x?"], "whitespace-only"),
        ] {
            let err = CompiledRule::new(&with_lines(&patterns), 2)
                .err()
                .expect("refused");
            let text = format!("{err:#}");
            assert!(
                text.contains("rules[2].require_added_line_match") && text.contains(why),
                "{text}"
            );
        }
        assert!(CompiledRule::new(&with_lines(&["^## "]), 0).is_ok());
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
