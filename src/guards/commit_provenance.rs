//! `commit-provenance`: every commit in the change says where it came from, and a commit
//! an agent produced carries a review by someone else.
//!
//! Trailers are self-asserted text, so this is hygiene: it makes a missing statement
//! visible, it does not prove one. The signals a change cannot forge are the forge's
//! review state and commit signatures; `discipline doctor` reads those from branch
//! protection, and `directives.require_approval` reads the reviews.
//!
//! Default off: which trailers a repository requires is its own policy.

use super::{Context, GateOutcome};
use crate::config::GateSettings;
use crate::gitctx::CommitDetail;
use crate::tokens;
use anyhow::Result;

pub const GATE: &str = "commit-provenance";

/// Trailer lines at the end of a commit message: `Key: value` runs after the last blank
/// line, in the order written. Case of the key is kept.
pub fn trailers(message: &str) -> Vec<(String, String)> {
    let body = message.trim_end();
    // The subject paragraph is never a trailer block, whatever it looks like.
    let Some((_, last_block)) = body.rsplit_once("\n\n") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in last_block.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once(':') else {
            // A non-trailer line means this block is prose, not a trailer block.
            return Vec::new();
        };
        let k = k.trim();
        if k.is_empty()
            || k.contains(' ')
            || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Vec::new();
        }
        out.push((k.to_string(), v.trim().to_string()));
    }
    out
}

fn has_trailer(trailers: &[(String, String)], key: &str) -> bool {
    trailers.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
}

/// Whether a commit identifies itself as produced by an agent: an `agent_markers` entry
/// matches a trailer key, a trailer line, the author name or the author email
/// (case-insensitive substring).
pub fn is_agent_commit(
    c: &CommitDetail,
    trailers: &[(String, String)],
    markers: &[String],
) -> bool {
    let hay: Vec<String> = trailers
        .iter()
        .map(|(k, v)| format!("{k}: {v}").to_lowercase())
        .chain([c.author_name.to_lowercase(), c.author_email.to_lowercase()])
        .collect();
    markers.iter().any(|m| {
        let m = m.to_lowercase();
        hay.iter().any(|h| h.contains(&m))
    })
}

/// Whether the review trailer names someone other than the author.
fn reviewed_by_someone_else(
    c: &CommitDetail,
    trailers: &[(String, String)],
    review_key: &str,
) -> bool {
    trailers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(review_key))
        .any(|(_, v)| {
            let v = v.to_lowercase();
            let name = c.author_name.to_lowercase();
            let email = c.author_email.to_lowercase();
            !v.is_empty()
                && !(name.len() > 2 && v.contains(&name))
                && !(email.len() > 2 && v.contains(&email))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: &'static crate::findings::FindingKind,
    pub sha: String,
    pub what: String,
}

pub fn judge(
    commits: &[CommitDetail],
    required: &[String],
    markers: &[String],
    review_key: &str,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for c in commits {
        let t = trailers(&c.message);
        let short: String = c.sha.chars().take(10).collect();
        for key in required {
            if !has_trailer(&t, key) {
                out.push(Finding {
                    kind: &crate::findings::COMMIT_TRAILER_MISSING,
                    sha: c.sha.clone(),
                    what: format!("commit {short} has no `{key}:` trailer"),
                });
            }
        }
        if !review_key.is_empty() && is_agent_commit(c, &t, markers) {
            if !has_trailer(&t, review_key) {
                out.push(Finding {
                    kind: &crate::findings::AGENT_COMMIT_WITHOUT_REVIEW,
                    sha: c.sha.clone(),
                    what: format!(
                        "commit {short} identifies itself as agent-produced and carries no `{review_key}:` trailer"
                    ),
                });
            } else if !reviewed_by_someone_else(c, &t, review_key) {
                out.push(Finding {
                    kind: &crate::findings::AGENT_COMMIT_REVIEWED_BY_AUTHOR,
                    sha: c.sha.clone(),
                    what: format!(
                        "commit {short} is agent-produced and its `{review_key}:` trailer names its own author"
                    ),
                });
            }
        }
    }
    out
}

pub fn commit_provenance(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.commit_provenance;
    let mut out = GateOutcome::new(GATE);
    let commits = ctx.git.commit_details()?;
    if commits.is_empty() {
        out.notes.push(
            "not evaluated: no commits between the base and HEAD (a staged check has none)"
                .to_string(),
        );
        return Ok(out);
    }
    out.examined = commits.len();
    for f in judge(
        &commits,
        &settings.required_trailers,
        &settings.agent_markers,
        &settings.review_trailer,
    ) {
        let short: String = f.sha.chars().take(7).collect();
        if let Some(ov) = ctx
            .find_override(GATE, tokens::ALLOW_COMMIT_PROVENANCE, &short)
            .or_else(|| ctx.find_override(GATE, tokens::ALLOW_COMMIT_PROVENANCE, &f.sha))
        {
            out.overrides.push(ov);
            continue;
        }
        out.push(
            ctx.overridable(settings.severity()),
            f.kind,
            None,
            None,
            format!("{}.", f.what),
            &format!(
                "Amend the commit with the trailer, or justify it on its own line in the PR body: `allow-commit-provenance: {short} <reason>`."
            ),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(author: &str, email: &str, message: &str) -> CommitDetail {
        CommitDetail {
            sha: "0123456789abcdef".into(),
            author_name: author.into(),
            author_email: email.into(),
            committer_email: email.into(),
            message: message.into(),
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn trailers_are_the_last_block_of_key_value_lines() {
        let t = trailers(
            "feat: x\n\nBody text: not a trailer\n\nSigned-off-by: A <a@x>\nReviewed-by: B <b@x>\n",
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].0, "Signed-off-by");
        assert!(trailers("feat: x\n\nJust prose here.\n").is_empty());
        assert!(trailers("feat: x").is_empty());
    }

    #[test]
    fn missing_required_trailer_and_agent_commit_rules() {
        let markers = v(&["Agent-Tool:", "[bot]", "noreply@anthropic.com"]);
        let plain = commit("Ada", "ada@x", "feat: x\n\nSigned-off-by: Ada <ada@x>\n");
        assert!(judge(
            std::slice::from_ref(&plain),
            &v(&["Signed-off-by"]),
            &markers,
            "Reviewed-by"
        )
        .is_empty());
        let unsigned = commit("Ada", "ada@x", "feat: x\n");
        let f = judge(&[unsigned], &v(&["Signed-off-by"]), &markers, "Reviewed-by");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind.title, "Commit Trailer Missing");

        let agent = commit("Ada", "ada@x", "feat: x\n\nAgent-Tool: coder 1.2\n");
        let f = judge(&[agent], &[], &markers, "Reviewed-by");
        assert_eq!(f[0].kind.title, "Agent Commit Without Review");
        let self_reviewed = commit(
            "Ada",
            "ada@x",
            "feat: x\n\nAgent-Tool: coder 1.2\nReviewed-by: Ada <ada@x>\n",
        );
        let f = judge(&[self_reviewed], &[], &markers, "Reviewed-by");
        assert_eq!(f[0].kind.title, "Agent Commit Reviewed By Its Author");
        let reviewed = commit(
            "Ada",
            "ada@x",
            "feat: x\n\nAgent-Tool: coder 1.2\nReviewed-by: Bob <bob@x>\n",
        );
        assert!(judge(&[reviewed], &[], &markers, "Reviewed-by").is_empty());
        // The author identity itself marks a bot; no trailer needed to notice it.
        let bot = commit(
            "coder[bot]",
            "1234+coder[bot]@users.noreply.github.com",
            "feat: x\n",
        );
        assert_eq!(
            judge(&[bot], &[], &markers, "Reviewed-by")[0].kind.title,
            "Agent Commit Without Review"
        );
        // Without a review key the agent rule is off.
        let agent = commit("Ada", "ada@x", "feat: x\n\nAgent-Tool: coder 1.2\n");
        assert!(judge(&[agent], &[], &markers, "").is_empty());
    }
}
