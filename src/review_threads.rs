//! Review threads of a pull request and whether each is resolved (`review-threads`).
//!
//! Each forge keeps this differently:
//!
//! - **GitHub**: GraphQL `pullRequest.reviewThreads { isResolved }`; the REST API does
//!   not expose a thread's resolved state.
//! - **GitLab**: merge-request discussions; a discussion is a thread when its notes are
//!   `resolvable`, and resolved when every resolvable note is.
//! - **Gitea, Forgejo**: code comments of the pull request's reviews. A conversation is
//!   the comments on one path and one line (new-side `position`, or old-side
//!   `original_position`), across reviews; resolving it sets `resolver` on its **first**
//!   comment only, and replies keep `resolver: null`. Observed on Gitea 1.24.7 and
//!   Forgejo 12.0.4 (a reply in a second review joined the conversation; resolve and
//!   unresolve set and cleared the first comment's `resolver`). Counting comments one by
//!   one would report every resolved conversation with a reply as unresolved.
//!
//! A list that cannot be read to its end is an error, never a count of what was read.

use crate::forge::{
    gitlab_project_id, read_all, Forge, ForgeApi, ForgeError, ForgeErrorKind, ForgeKind, MAX_PAGES,
};
use serde_json::{json, Value};

/// One review thread (a GitLab discussion, a Gitea conversation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    /// The file it is on; empty for a thread on the whole change.
    pub path: String,
    /// The line it is on, as the forge reports it (`12`, or `-12` for an old-side line on
    /// Gitea and Forgejo); empty when not on a line.
    pub line: String,
    pub resolved: bool,
    /// On a line the latest commit no longer has (GitHub `isOutdated`).
    pub outdated: bool,
}

/// The GitHub GraphQL query for a pull request's review threads.
pub const REVIEW_THREADS_QUERY: &str = "query ReviewThreads($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100, after: $after) { totalCount pageInfo { hasNextPage endCursor } nodes { isResolved isOutdated path line originalLine } } } } }";

/// The variables of [`REVIEW_THREADS_QUERY`].
pub fn review_threads_vars(repo: &str, number: u64, after: Option<&str>) -> Value {
    let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
    json!({"owner": owner, "name": name, "number": number, "after": after})
}

fn line_of(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Every review thread of pull request `number`, or an error: never part of them.
pub fn review_threads(
    api: &dyn ForgeApi,
    forge: &Forge,
    number: u64,
) -> Result<Vec<Thread>, ForgeError> {
    match forge.kind {
        ForgeKind::GitHub => {
            let mut out = Vec::new();
            let mut after: Option<String> = None;
            for _ in 0..MAX_PAGES {
                let data = api.graphql(
                    forge,
                    REVIEW_THREADS_QUERY,
                    &review_threads_vars(&forge.repo, number, after.as_deref()),
                )?;
                let threads = data
                    .pointer("/repository/pullRequest/reviewThreads")
                    .filter(|t| !t.is_null())
                    .ok_or_else(|| {
                        ForgeError::new(
                            ForgeErrorKind::NotFound,
                            format!("pull request #{number} is not visible"),
                        )
                    })?;
                for n in threads["nodes"].as_array().into_iter().flatten() {
                    let line = match &n["line"] {
                        Value::Null => line_of(&n["originalLine"]),
                        l => line_of(l),
                    };
                    out.push(Thread {
                        path: n["path"].as_str().unwrap_or_default().to_string(),
                        line,
                        resolved: n["isResolved"].as_bool().unwrap_or(false),
                        outdated: n["isOutdated"].as_bool().unwrap_or(false),
                    });
                }
                if threads["pageInfo"]["hasNextPage"].as_bool() == Some(true) {
                    after = threads["pageInfo"]["endCursor"]
                        .as_str()
                        .map(str::to_string);
                    continue;
                }
                let total = threads["totalCount"].as_u64().unwrap_or(0);
                if out.len() as u64 != total {
                    return Err(ForgeError::new(
                        ForgeErrorKind::Partial,
                        format!(
                            "pull request #{number}: {} of {total} review threads read; refusing to judge a partial list",
                            out.len()
                        ),
                    ));
                }
                return Ok(out);
            }
            Err(ForgeError::new(
                ForgeErrorKind::Partial,
                format!("pull request #{number} has more than {MAX_PAGES} pages of review threads"),
            ))
        }
        ForgeKind::GitLab => {
            let path = format!(
                "projects/{}/merge_requests/{number}/discussions",
                gitlab_project_id(&forge.repo)
            );
            Ok(read_all(api, forge, &path)?
                .iter()
                .filter_map(|d| {
                    let notes: Vec<&Value> = d["notes"]
                        .as_array()?
                        .iter()
                        .filter(|n| n["resolvable"].as_bool() == Some(true))
                        .collect();
                    let first = notes.first()?;
                    let pos = &first["position"];
                    let line = match &pos["new_line"] {
                        Value::Null => line_of(&pos["old_line"]),
                        l => line_of(l),
                    };
                    Some(Thread {
                        path: pos["new_path"]
                            .as_str()
                            .or_else(|| pos["old_path"].as_str())
                            .unwrap_or_default()
                            .to_string(),
                        line,
                        resolved: notes.iter().all(|n| n["resolved"].as_bool() == Some(true)),
                        outdated: false,
                    })
                })
                .collect())
        }
        ForgeKind::Gitea | ForgeKind::Forgejo => {
            let reviews = read_all(
                api,
                forge,
                &format!("repos/{}/pulls/{number}/reviews", forge.repo),
            )?;
            // (path, signed line) -> comments, as the forge groups a conversation.
            let mut conversations: std::collections::BTreeMap<(String, i64), Vec<Value>> =
                Default::default();
            for r in &reviews {
                // A pending review is its author's draft: not yet a thread anyone sees.
                if r["state"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("PENDING"))
                {
                    continue;
                }
                let Some(id) = r["id"].as_u64() else {
                    return Err(ForgeError::new(
                        ForgeErrorKind::Malformed,
                        format!("a review of pull request #{number} carries no id"),
                    ));
                };
                // The review-comments endpoint is not paged: one read is the whole list.
                let comments = api.fetch(
                    forge,
                    &format!("repos/{}/pulls/{number}/reviews/{id}/comments", forge.repo),
                )?;
                let comments = comments.as_array().ok_or_else(|| {
                    ForgeError::new(
                        ForgeErrorKind::Malformed,
                        format!("comments of review {id} are not a list"),
                    )
                })?;
                for c in comments {
                    let pos = c["position"].as_i64().unwrap_or(0);
                    let orig = c["original_position"].as_i64().unwrap_or(0);
                    let line = if pos > 0 { pos } else { -orig };
                    let path = c["path"].as_str().unwrap_or_default().to_string();
                    conversations
                        .entry((path, line))
                        .or_default()
                        .push(c.clone());
                }
            }
            Ok(conversations
                .into_iter()
                .map(|((path, line), mut comments)| {
                    comments.sort_by_key(|c| c["id"].as_u64().unwrap_or(u64::MAX));
                    let resolved = comments.first().is_some_and(|c| c["resolver"].is_object());
                    Thread {
                        path,
                        line: if line == 0 {
                            String::new()
                        } else {
                            line.to_string()
                        },
                        resolved,
                        outdated: false,
                    }
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CannedApi;

    fn forge(kind: ForgeKind) -> Forge {
        Forge {
            kind,
            url: "https://git.example.com".into(),
            repo: "o/r".into(),
        }
    }

    fn unresolved(threads: &[Thread]) -> Vec<String> {
        threads
            .iter()
            .filter(|t| !t.resolved)
            .map(|t| format!("{}:{}", t.path, t.line))
            .collect()
    }

    /// The shape Gitea 1.24.7 and Forgejo 12.0.4 answered with (RUN): thread A (line 1)
    /// resolved, with a reply in a second review; thread B (line 3) open.
    fn gitea_like(kind: ForgeKind, a_resolved: bool) -> CannedApi {
        let k = kind.label();
        let mut api = CannedApi::default();
        api.responses.insert(
            format!("{k}:repos/o/r/pulls/2/reviews?limit=50&page=1"),
            json!({"__status": 200, "__headers": {"X-Total-Count": "3"}, "__body": [
                {"id": 1, "state": "COMMENT"}, {"id": 2, "state": "COMMENT"},
                {"id": 3, "state": "PENDING"}]}),
        );
        let resolver = if a_resolved {
            json!({"login": "owner"})
        } else {
            Value::Null
        };
        api.responses.insert(
            format!("{k}:repos/o/r/pulls/2/reviews/1/comments"),
            json!([
                {"id": 58, "path": "scripts/check_x.py", "position": 1, "original_position": 0, "body": "thread A", "resolver": resolver},
                {"id": 59, "path": "scripts/check_x.py", "position": 3, "original_position": 0, "body": "thread B", "resolver": null}]),
        );
        api.responses.insert(
            format!("{k}:repos/o/r/pulls/2/reviews/2/comments"),
            json!([{"id": 61, "path": "scripts/check_x.py", "position": 1, "original_position": 0, "body": "reply to A", "resolver": null}]),
        );
        api
    }

    #[test]
    fn a_gitea_conversation_is_resolved_by_its_first_comment_not_its_replies() {
        for kind in [ForgeKind::Gitea, ForgeKind::Forgejo] {
            let t = review_threads(&gitea_like(kind, true), &forge(kind), 2).unwrap();
            assert_eq!(t.len(), 2, "the reply joins thread A: {t:?}");
            assert_eq!(unresolved(&t), vec!["scripts/check_x.py:3"], "{kind:?}");
            let t = review_threads(&gitea_like(kind, false), &forge(kind), 2).unwrap();
            assert_eq!(
                unresolved(&t),
                vec!["scripts/check_x.py:1", "scripts/check_x.py:3"]
            );
        }
        // The pending review's comments are never asked for.
        let api = gitea_like(ForgeKind::Gitea, true);
        review_threads(&api, &forge(ForgeKind::Gitea), 2).unwrap();
        assert!(!api.was_called("gitea:repos/o/r/pulls/2/reviews/3/comments"));
    }

    #[test]
    fn an_old_side_comment_is_its_own_conversation() {
        let mut api = gitea_like(ForgeKind::Gitea, true);
        api.responses.insert(
            "gitea:repos/o/r/pulls/2/reviews/2/comments".into(),
            json!([{"id": 61, "path": "scripts/check_x.py", "position": 0, "original_position": 1, "body": "on the old line 1", "resolver": null}]),
        );
        let t = review_threads(&api, &forge(ForgeKind::Gitea), 2).unwrap();
        assert_eq!(
            unresolved(&t),
            vec!["scripts/check_x.py:-1", "scripts/check_x.py:3"]
        );
    }

    #[test]
    fn a_review_comment_list_that_cannot_be_read_is_an_error() {
        let mut api = gitea_like(ForgeKind::Gitea, true);
        api.responses.insert(
            "gitea:repos/o/r/pulls/2/reviews/2/comments".into(),
            json!({"__status": 502}),
        );
        let e = review_threads(&api, &forge(ForgeKind::Gitea), 2).unwrap_err();
        assert_eq!(e.kind, ForgeErrorKind::Unavailable);
    }

    #[test]
    fn github_threads_come_from_graphql() {
        let mut api = CannedApi::default();
        let key = format!(
            "github:graphql:ReviewThreads {}",
            review_threads_vars("o/r", 7, None)
        );
        api.responses.insert(
            key.clone(),
            json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
                "totalCount": 2, "pageInfo": {"hasNextPage": false, "endCursor": null},
                "nodes": [
                    {"isResolved": true, "isOutdated": false, "path": "a.rs", "line": 3, "originalLine": 3},
                    {"isResolved": false, "isOutdated": true, "path": "b.rs", "line": null, "originalLine": 9}]}}}}}),
        );
        let t = review_threads(&api, &forge(ForgeKind::GitHub), 7).unwrap();
        assert_eq!(unresolved(&t), vec!["b.rs:9"]);
        assert!(t[1].outdated);
        // A total the pages do not reach is refused.
        api.responses.insert(
            key,
            json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
                "totalCount": 3, "pageInfo": {"hasNextPage": false, "endCursor": null},
                "nodes": []}}}}}),
        );
        assert_eq!(
            review_threads(&api, &forge(ForgeKind::GitHub), 7)
                .unwrap_err()
                .kind,
            ForgeErrorKind::Partial
        );
    }

    #[test]
    fn gitlab_threads_are_resolvable_discussions() {
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitlab:projects/o%2Fr/merge_requests/7/discussions?per_page=100&page=1".into(),
            json!([
                {"notes": [{"resolvable": false, "body": "a plain comment"}]},
                {"notes": [
                    {"resolvable": true, "resolved": true, "position": {"new_path": "a.rs", "new_line": 3}},
                    {"resolvable": true, "resolved": false}]},
                {"notes": [
                    {"resolvable": true, "resolved": true, "position": {"new_path": "b.rs", "new_line": null, "old_line": 4}}]}]),
        );
        let t = review_threads(&api, &forge(ForgeKind::GitLab), 7).unwrap();
        assert_eq!(t.len(), 2, "a non-resolvable discussion is not a thread");
        assert_eq!(unresolved(&t), vec!["a.rs:3"]);
    }
}
