//! `discipline check --comment`: one pull-request comment per pull request, updated on
//! every run.
//!
//! The comment is found by a hidden marker and edited in place, so a pull request
//! carries one report however many times the check runs. It lists the findings with
//! their repair, the overrides that lifted findings, and any policy refusal. Like the
//! other surfaces an agent can read, it never carries directive syntax: a reviewer
//! who needs it runs `discipline explain <gate>`.
//!
//! Text from the change (file names, messages) is escaped: no `@` mention, no HTML,
//! no table break, and no way to forge the marker.

use crate::forge::{Forge, ForgeApi, ForgeKind, ForgeWrite, WriteError, WriteMethod};
use crate::guards::CheckSummary;

/// First line of the comment; how the next run finds it.
pub const MARKER: &str = "<!-- discipline:report -->";

/// Findings listed before the rest are summarised.
const MAX_ROWS: usize = 50;

/// Pages of comments read when looking for the marker.
const MAX_PAGES: usize = 20;

/// Text from the change, safe inside a markdown table cell.
pub fn cell(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let escaped = flat
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "\\|")
        .replace('@', "&#64;")
        .replace('`', "'");
    crate::report::scrub_override_directives(&escaped)
}

/// The comment body for `summary`.
pub fn render(summary: &CheckSummary, success: bool) -> String {
    let violations: Vec<_> = summary.violations().collect();
    let (passed, _, _, examined) = summary.gate_counts(false, false);
    let mut out = format!("{MARKER}\n");
    let blocking = summary.errors;
    out.push_str(&if success {
        "### discipline: passed\n\n".to_string()
    } else if blocking > 0 {
        format!("### discipline: {blocking} blocking finding(s)\n\n")
    } else {
        "### discipline: failed\n\n".to_string()
    });
    out.push_str(&format!(
        "Base `{}` · {passed} gate(s) passed · {examined} item(s) examined · {} warning(s) · {} override(s)\n\n",
        cell(&summary.base),
        summary.warnings,
        summary.total_overrides()
    ));
    for f in &summary.policy_failures {
        out.push_str(&format!("- **Refused:** {}\n", cell(f)));
    }
    if !summary.policy_failures.is_empty() {
        out.push('\n');
    }
    if !violations.is_empty() {
        out.push_str("| Severity | Gate | Location | Finding | Repair |\n|---|---|---|---|---|\n");
        for v in violations.iter().take(MAX_ROWS) {
            let loc = match (&v.file, v.line) {
                (Some(f), Some(l)) => format!("{f}:{l}"),
                (Some(f), None) => f.clone(),
                _ => "-".to_string(),
            };
            out.push_str(&format!(
                "| {} | `{}` | {} | {}: {} | {} |\n",
                v.severity,
                v.gate,
                cell(&loc),
                cell(&v.title),
                cell(&v.message),
                cell(&crate::report::repair_action_for_violation(v)),
            ));
        }
        if violations.len() > MAX_ROWS {
            out.push_str(&format!(
                "\n{} more finding(s) are in the job log.\n",
                violations.len() - MAX_ROWS
            ));
        }
        out.push('\n');
    }
    let overrides: Vec<_> = summary.overrides().collect();
    if !overrides.is_empty() {
        out.push_str("**Findings lifted by an override:**\n\n");
        for o in overrides {
            out.push_str(&format!(
                "- `{}` on `{}`: {}\n",
                o.gate,
                cell(&o.subject),
                cell(&o.reason)
            ));
        }
        out.push('\n');
    }
    out.push_str("<sub>`discipline explain <gate>` prints what a gate checks and how a finding is lifted. This comment is updated on every run; the check's status is the verdict.</sub>\n");
    out
}

/// What happened to the comment.
#[derive(Debug, PartialEq, Eq)]
pub enum Posted {
    Created,
    Updated,
    /// The token cannot write here (a pull request from a fork): named, not fatal.
    Denied(String),
}

fn comments_path(forge: &Forge, number: u64, page: usize) -> String {
    match forge.kind {
        ForgeKind::GitLab => format!(
            "projects/{}/merge_requests/{number}/notes?sort=asc&per_page=100&page={page}",
            crate::forge::gitlab_project_id(&forge.repo)
        ),
        ForgeKind::GitHub => format!(
            "repos/{}/issues/{number}/comments?per_page=100&page={page}",
            forge.repo
        ),
        ForgeKind::Gitea | ForgeKind::Forgejo => format!(
            "repos/{}/issues/{number}/comments?limit=50&page={page}",
            forge.repo
        ),
    }
}

/// The id of this tool's comment on the pull request, if one exists.
pub fn find_existing(
    api: &dyn ForgeApi,
    forge: &Forge,
    number: u64,
) -> Result<Option<u64>, String> {
    for page in 1..=MAX_PAGES {
        let list = api
            .get(forge, &comments_path(forge, number, page))?
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        let items = list.as_array().cloned().unwrap_or_default();
        for c in &items {
            let body = c.get("body").and_then(|b| b.as_str()).unwrap_or("");
            if body.trim_start().starts_with(MARKER) {
                if let Some(id) = c.get("id").and_then(|i| i.as_u64()) {
                    return Ok(Some(id));
                }
            }
        }
        if items.is_empty() || items.len() < 50 {
            break;
        }
    }
    Ok(None)
}

/// Edits this tool's comment on pull request `number`, or creates it.
pub fn upsert(
    read: &dyn ForgeApi,
    write: &dyn ForgeWrite,
    forge: &Forge,
    number: u64,
    body: &str,
) -> Result<Posted, String> {
    let payload = serde_json::json!({ "body": body });
    let (collection, edit_method, item_base) = match forge.kind {
        ForgeKind::GitLab => {
            let base = format!(
                "projects/{}/merge_requests/{number}/notes",
                crate::forge::gitlab_project_id(&forge.repo)
            );
            (base.clone(), WriteMethod::Put, base)
        }
        _ => (
            format!("repos/{}/issues/{number}/comments", forge.repo),
            WriteMethod::Patch,
            format!("repos/{}/issues/comments", forge.repo),
        ),
    };
    if let Some(id) = find_existing(read, forge, number)? {
        match write.send(forge, edit_method, &format!("{item_base}/{id}"), &payload) {
            Ok(_) => return Ok(Posted::Updated),
            // A marked comment this token may not edit (another author's): post our own.
            Err(WriteError::Denied(_)) => {}
            Err(WriteError::Failed(e)) => return Err(e),
        }
    }
    match write.send(forge, WriteMethod::Post, &collection, &payload) {
        Ok(_) => Ok(Posted::Created),
        Err(WriteError::Denied(e)) => Ok(Posted::Denied(e)),
        Err(WriteError::Failed(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::forge::CannedApi;
    use crate::guards::{GateOutcome, Violation};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<(WriteMethod, String)>>,
        deny_patch: bool,
        deny_post: bool,
    }
    impl ForgeWrite for Recorder {
        fn send(
            &self,
            _: &Forge,
            m: WriteMethod,
            p: &str,
            _: &serde_json::Value,
        ) -> Result<serde_json::Value, WriteError> {
            self.calls.borrow_mut().push((m, p.to_string()));
            if (m == WriteMethod::Patch && self.deny_patch)
                || (m == WriteMethod::Post && self.deny_post)
            {
                return Err(WriteError::Denied("HTTP 403".into()));
            }
            Ok(serde_json::json!({}))
        }
    }

    fn forge(kind: ForgeKind) -> Forge {
        Forge {
            kind,
            url: "https://f.example".into(),
            repo: "o/r".into(),
        }
    }

    fn canned(kind: ForgeKind, path: &str, v: serde_json::Value) -> CannedApi {
        let mut api = CannedApi::default();
        api.responses.insert(format!("{}:{path}", kind.label()), v);
        api
    }

    #[test]
    fn a_marked_comment_is_edited_and_otherwise_one_is_created() {
        let f = forge(ForgeKind::GitHub);
        let list = comments_path(&f, 7, 1);
        let with = canned(
            ForgeKind::GitHub,
            &list,
            serde_json::json!([
                {"id": 1, "body": "LGTM"},
                {"id": 42, "body": format!("{MARKER}\n### discipline: passed")}
            ]),
        );
        let rec = Recorder::default();
        assert_eq!(upsert(&with, &rec, &f, 7, "x"), Ok(Posted::Updated));
        assert_eq!(
            rec.calls.borrow()[0],
            (
                WriteMethod::Patch,
                "repos/o/r/issues/comments/42".to_string()
            )
        );

        let without = canned(
            ForgeKind::GitHub,
            &list,
            serde_json::json!([{"id": 1, "body": "LGTM"}]),
        );
        let rec = Recorder::default();
        assert_eq!(upsert(&without, &rec, &f, 7, "x"), Ok(Posted::Created));
        assert_eq!(
            rec.calls.borrow()[0],
            (WriteMethod::Post, "repos/o/r/issues/7/comments".to_string())
        );
    }

    #[test]
    fn gitlab_uses_merge_request_notes() {
        let f = forge(ForgeKind::GitLab);
        let api = canned(
            ForgeKind::GitLab,
            &comments_path(&f, 3, 1),
            serde_json::json!([{"id": 9, "body": MARKER}]),
        );
        let rec = Recorder::default();
        assert_eq!(upsert(&api, &rec, &f, 3, "x"), Ok(Posted::Updated));
        assert_eq!(
            rec.calls.borrow()[0],
            (
                WriteMethod::Put,
                "projects/o%2Fr/merge_requests/3/notes/9".to_string()
            )
        );
    }

    #[test]
    fn a_fork_token_that_cannot_write_is_named_not_fatal() {
        let f = forge(ForgeKind::Gitea);
        let api = canned(
            ForgeKind::Gitea,
            &comments_path(&f, 5, 1),
            serde_json::json!([]),
        );
        let rec = Recorder {
            deny_post: true,
            ..Default::default()
        };
        assert!(matches!(
            upsert(&api, &rec, &f, 5, "x"),
            Ok(Posted::Denied(_))
        ));
        // Someone else's marked comment that cannot be edited: our own is posted.
        let api = canned(
            ForgeKind::Gitea,
            &comments_path(&f, 5, 1),
            serde_json::json!([{"id": 3, "body": MARKER}]),
        );
        let rec = Recorder {
            deny_patch: true,
            ..Default::default()
        };
        assert_eq!(upsert(&api, &rec, &f, 5, "x"), Ok(Posted::Created));
    }

    #[test]
    fn the_body_escapes_change_text_and_carries_no_waiver() {
        let mut o = GateOutcome::new("assertion-reduction");
        o.violations.push(Violation {
            gate: "assertion-reduction",
            code: "assertion-reduction/fixture".to_string(),
            severity: Severity::Error,
            title: "Assertion Reduction".into(),
            file: Some("tests/<b>|x.rs".into()),
            line: Some(3),
            message: "@team dropped <!-- discipline:report --> from 2 to 0".into(),
            remediation: Some("allow-assertion-drop: adds <reason>".into()),
        });
        let summary = CheckSummary {
            base: "main".into(),
            errors: 1,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![o],
            planned_gates: vec![],
            policy_failures: vec![],
            deprecations: Vec::new(),
        };
        let body = render(&summary, false);
        assert!(body.starts_with(MARKER));
        assert_eq!(
            body.matches(MARKER).count(),
            1,
            "the marker is forged from change text: {body}"
        );
        assert!(body.contains("1 blocking finding(s)"));
        assert!(
            !body.contains("@team") && body.contains("&#64;team"),
            "{body}"
        );
        assert!(!body.contains("<b>") && body.contains("\\|x.rs"), "{body}");
        assert!(!body.contains("allow-assertion-drop"), "{body}");
    }
}
