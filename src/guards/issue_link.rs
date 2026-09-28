//! PR tracking issue link sentinel (`issue-link`).
//!
//! Enforces:
//! - Pull requests must link a tracking issue in the PR title or body:
//!   - `#123`, `Fixes #123`, `Closes #123`, `Resolves #123`, `Refs #123`, `Part of #123`
//! - Or provide an explicit waiver directive:
//!   - `no-issue: <non-empty reason>` (e.g. `no-issue: typo fix in README`)
//!   - `discipline:allow(issue-link) <reason>`
//! - When running locally without PR metadata, records truthful `examined: 0` and note
//!   to avoid false-positive failures during pre-commit.
//! - With `verify_references`, looks every reference up on the forge
//!   ([`crate::references`]): at least one must be an issue of this repository. A forge
//!   that cannot be read is exit 2, never a pass.

use crate::config::{GateSettings, IssueLinkGate, IssueWaiver};
use crate::could_not_check::{tag, Reason};
use crate::guards::{exempt_filter, Context, GateOutcome};
use crate::references::{self, Verdict};
use crate::tokens::{self, OverrideRecord};
use anyhow::{anyhow, Result};
use regex::Regex;

pub const GATE: &str = "issue-link";

pub const DEFAULT_ISSUE_PATTERN: &str = r"(?i)(?:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)?#\d+\b|https?://[^\s/]+/[^\s/]+/[^\s/]+/(?:issues|pull)/\d+\b";

pub fn has_issue_reference(text: &str, re: &Regex) -> bool {
    re.is_match(text)
}

pub fn evaluate_issue_link(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.issue_link;
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);

    if !settings.exempt_paths.is_empty() {
        if let Ok(changed) = ctx.git.changed_files() {
            if !changed.is_empty() && changed.iter().all(|f| exempt.matches(&f.path)) {
                out.examined = 0;
                out.notes
                    .push("all changed files are exempt from issue-link".to_string());
                return Ok(out);
            }
        }
    }

    // Check for directive leakage into commit subjects on the branch (Q6).
    // Directives belong in the commit body, never in the subject line.
    let branch_commits = ctx.git.commits().unwrap_or_default();
    let mut directive_in_subject_found = false;
    for (sha, msg) in &branch_commits {
        let subject = msg.lines().next().unwrap_or("").trim();
        if let Some(dir) = tokens::find_directive_in_subject(subject) {
            directive_in_subject_found = true;
            out.push(
                settings.severity(),
                &crate::findings::DIRECTIVE_IN_SUBJECT_LINE,
                None,
                None,
                format!(
                    "Commit {sha} subject '{subject}' contains directive '{dir}'. Directives belong in the commit body, never in the subject line."
                ),
                "Move the directive into the commit body.",
            );
            out.anchor_last(format!("commit:{sha}"));
        }
    }

    if let Some(pr_title) = ctx.pr_title.as_deref() {
        let title = pr_title.trim();
        if !title.is_empty() {
            if let Some(dir) = tokens::find_directive_in_subject(title) {
                directive_in_subject_found = true;
                out.push(
                    settings.severity(),
                    &crate::findings::DIRECTIVE_IN_SUBJECT_LINE,
                    None,
                    None,
                    format!(
                        "PR title '{title}' contains directive '{dir}'. Directives belong in the PR body or commit body, never in the subject line."
                    ),
                    "Move the directive into the PR body.",
                );
                out.anchor_last("pr-title");
            }
        }
    }

    if ctx.staged {
        let msg = ctx.pr_body.as_deref().unwrap_or("").trim();
        let subject = msg.lines().next().unwrap_or("").trim();
        if !subject.is_empty() {
            if let Some(dir) = tokens::find_directive_in_subject(subject) {
                out.push(
                    settings.severity(),
                    &crate::findings::DIRECTIVE_IN_SUBJECT_LINE,
                    None,
                    None,
                    format!(
                        "Staged commit subject '{subject}' contains directive '{dir}'. Directives belong in the commit body, never in the subject line."
                    ),
                    "Move the directive into the commit body.",
                );
                out.anchor_last("staged-subject");
                out.examined = 1;
                return Ok(out);
            }
        }
        if settings.require_in_commit_if_no_pr {
            if msg.is_empty() {
                out.examined = 0;
                out.notes.push(
                    "staged change without commit message; issue-link check skipped".to_string(),
                );
                return Ok(out);
            }
            out.examined = 1;
            let pattern_str = settings.pattern.as_deref().unwrap_or(DEFAULT_ISSUE_PATTERN);
            let re_issue = Regex::new(pattern_str)?;
            if has_issue_reference(msg, &re_issue) {
                return Ok(out);
            }
            if let Some(waiver) = waiver(settings, &ctx.directives) {
                out.overrides.push(waiver);
                return Ok(out);
            }
            out.push(
                settings.severity(),
                &crate::findings::ISSUE_LINK_MISSING_IN_COMMIT_MESSAGE,
                None,
                None,
                "Commit message does not reference a tracking issue (#123) and lacks a no-issue waiver.".to_string(),
                "Reference a tracking issue in the commit message, or add 'no-issue: <reason>'.",
            );
            return Ok(out);
        } else {
            out.examined = 0;
            out.notes
                .push("staged change (pre-commit); issue-link check skipped".to_string());
            return Ok(out);
        }
    }

    let pattern_str = settings.pattern.as_deref().unwrap_or(DEFAULT_ISSUE_PATTERN);
    let re_issue = Regex::new(pattern_str)?;

    let pr_title = ctx.pr_title.as_deref().unwrap_or("").trim();
    let pr_body = ctx.pr_body.as_deref().unwrap_or("").trim();

    let has_pr_context = !pr_title.is_empty() || !pr_body.is_empty();

    if !has_pr_context {
        if settings.require_in_commit_if_no_pr {
            if branch_commits.is_empty() {
                out.examined = 0;
                out.notes.push("no commits examined on branch".to_string());
                return Ok(out);
            }
            out.examined = branch_commits.len();
            let mut found = false;
            for (_sha, msg) in &branch_commits {
                if has_issue_reference(msg, &re_issue) {
                    found = true;
                    break;
                }
            }
            if !found {
                // Check if any directive waived it
                if let Some(waiver) = waiver(settings, &ctx.directives) {
                    out.overrides.push(waiver);
                } else {
                    out.push(
                        settings.severity(),
                        &crate::findings::ISSUE_LINK_MISSING_IN_COMMITS,
                        None,
                        None,
                        "No commit message on the branch references a tracking issue (#123) and lack a no-issue waiver."
                            .to_string(),
                        "Reference a tracking issue in a commit message, or add 'no-issue: <reason>' to the commit message.",
                    );
                }
            }
            return Ok(out);
        } else {
            if directive_in_subject_found {
                out.examined = branch_commits.len();
                return Ok(out);
            }
            out.examined = 0;
            out.notes
                .push("no PR title or body supplied; issue-link check skipped".to_string());
            return Ok(out);
        }
    }

    out.examined = 1;

    if settings.verify_references {
        return verify_references(ctx, settings, pr_title, pr_body, out);
    }

    // 1. Check PR title or PR body for issue reference
    if has_issue_reference(pr_title, &re_issue) || has_issue_reference(pr_body, &re_issue) {
        return Ok(out);
    }

    // 2. Check for waiver directive `no-issue: <reason>`
    if let Some(waiver) = waiver(settings, &ctx.directives) {
        out.overrides.push(waiver);
        return Ok(out);
    }

    // 3. Report violation
    out.push(
        settings.severity(),
        &crate::findings::ISSUE_LINK_MISSING,
        None,
        None,
        "Pull request title and body do not reference any tracking issue (#123, Fixes #123) and lack a no-issue waiver."
            .to_string(),
        "Reference a tracking issue (#123, Fixes #123) in the PR title or body, or add 'no-issue: <reason>' to the PR body.",
    );

    Ok(out)
}

/// The accepted `no-issue:` waiver, unless `waiver = "none"`.
fn waiver(
    settings: &IssueLinkGate,
    directives: &[tokens::ParsedDirective],
) -> Option<OverrideRecord> {
    match settings.waiver {
        IssueWaiver::Directive => find_no_issue_directive(directives),
        IssueWaiver::None => None,
    }
}

/// Whether a resolved reference tracks this change under `settings`.
pub fn qualifies(settings: &IssueLinkGate, verdict: Verdict) -> bool {
    match verdict {
        Verdict::Issue => true,
        Verdict::Closed => !settings.require_open_issue,
        Verdict::IsPull => settings.accept_pull_references,
        Verdict::NotFound | Verdict::CrossRepo => false,
    }
}

/// `verify_references`: every reference in the title and body is looked up, and at least
/// one must be an issue of this repository (or of `reference_repos`). A number that
/// resolves nowhere is a note while another reference qualifies; a forge that cannot be
/// read is exit 2, never a pass.
fn verify_references(
    ctx: &Context,
    settings: &IssueLinkGate,
    pr_title: &str,
    pr_body: &str,
    mut out: GateOutcome,
) -> Result<GateOutcome> {
    if settings.pattern.is_some() {
        return Err(tag(
            Reason::Configuration,
            anyhow!("`gates.issue-link.verify_references` looks up forge issue numbers and cannot verify a custom `pattern`; set one or the other"),
        ));
    }
    let access = ctx.forge.as_ref().ok_or_else(|| {
        tag(
            Reason::Forge,
            anyhow!("`gates.issue-link.verify_references` needs the forge, and this run has no forge access"),
        )
    })?;
    let forge = (access.identify)().map_err(|e| {
        tag(
            Reason::Forge,
            anyhow!("issue-link cannot identify the forge: {e}"),
        )
    })?;
    let refs = references::parse(&format!("{pr_title}\n{pr_body}"), forge.kind, &[]);
    if !refs.is_empty() {
        let resolved =
            references::resolve_all(access.api, &forge, &refs, &settings.reference_repos).map_err(
                |e| {
                    tag(
                        Reason::Forge,
                        anyhow!("issue-link could not verify the pull request's references: {e}"),
                    )
                },
            )?;
        out.notes.push(format!(
            "references: {}",
            resolved
                .iter()
                .map(|r| r.describe())
                .collect::<Vec<_>>()
                .join("; ")
        ));
        if resolved.iter().any(|r| qualifies(settings, r.verdict)) {
            return Ok(out);
        }
        if let Some(w) = waiver(settings, &ctx.directives) {
            out.overrides.push(w);
            return Ok(out);
        }
        let closed = resolved.iter().any(|r| r.verdict == Verdict::Closed);
        let listed = resolved
            .iter()
            .map(|r| r.describe())
            .collect::<Vec<_>>()
            .join(", ");
        if closed && settings.require_open_issue {
            out.push(
                settings.severity(),
                &crate::findings::ISSUE_REFERENCE_CLOSED,
                None,
                None,
                format!("Every issue this pull request references is closed ({listed})."),
                "Reference the open issue this change tracks, or add 'no-issue: <reason>' to the PR body.",
            );
        } else {
            out.push(
                settings.severity(),
                &crate::findings::ISSUE_REFERENCE_NOT_FOUND,
                None,
                None,
                format!("No reference in the pull request is an issue of this repository ({listed})."),
                "Reference an existing issue of this repository (#123), or add 'no-issue: <reason>' to the PR body.",
            );
        }
        return Ok(out);
    }
    if let Some(w) = waiver(settings, &ctx.directives) {
        out.overrides.push(w);
        return Ok(out);
    }
    out.push(
        settings.severity(),
        &crate::findings::ISSUE_LINK_MISSING,
        None,
        None,
        "Pull request title and body do not reference any tracking issue (#123, Fixes #123) and lack a no-issue waiver."
            .to_string(),
        "Reference a tracking issue (#123, Fixes #123) in the PR title or body, or add 'no-issue: <reason>' to the PR body.",
    );
    Ok(out)
}

fn find_no_issue_directive(directives: &[tokens::ParsedDirective]) -> Option<OverrideRecord> {
    for d in directives {
        if tokens::NO_ISSUE
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&d.directive))
        {
            let trimmed = d.reason.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('<') {
                return Some(OverrideRecord {
                    gate: GATE.to_string(),
                    subject: "pull-request".to_string(),
                    directive: d.directive.clone(),
                    reason: d.reason.clone(),
                    source: d.source.clone(),
                    hidden: d.hidden,
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_patterns_discriminate() {
        let re = Regex::new(DEFAULT_ISSUE_PATTERN).unwrap();

        // Positive controls
        assert!(has_issue_reference("#123", &re));
        assert!(has_issue_reference("Fixes #123", &re));
        assert!(has_issue_reference("Closes #456", &re));
        assert!(has_issue_reference("Resolves #789", &re));
        assert!(has_issue_reference("Refs #1011", &re));
        assert!(has_issue_reference("Part of #2022", &re));
        assert!(has_issue_reference("feat(core): update parser (#42)", &re));
        assert!(has_issue_reference("orieg/discipline#99", &re));
        assert!(has_issue_reference(
            "https://github.com/orieg/discipline/issues/100",
            &re
        ));

        // Negative controls
        assert!(!has_issue_reference("Just a regular PR title", &re));
        assert!(!has_issue_reference(
            "Fixing bug in parser without issue",
            &re
        ));
        assert!(!has_issue_reference("Ticket 123", &re));
        assert!(!has_issue_reference("#abc", &re));
    }

    #[test]
    fn no_issue_directive_extraction() {
        let dirs = vec![tokens::ParsedDirective {
            directive: "no-issue".to_string(),
            reason: "trivial typo in README".to_string(),
            source: tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let record = find_no_issue_directive(&dirs);
        assert!(record.is_some());
        assert_eq!(record.unwrap().reason, "trivial typo in README");

        // Placeholder is rejected
        let bad_dirs = vec![tokens::ParsedDirective {
            directive: "no-issue".to_string(),
            reason: "<reason>".to_string(),
            source: tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        assert!(find_no_issue_directive(&bad_dirs).is_none());
    }
}
