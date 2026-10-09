//! `commit-provenance`: every commit in the change says where it came from, and a commit
//! an agent produced carries a review by someone else.
//!
//! Trailers are self-asserted text, so this is hygiene: it makes a missing statement
//! visible, it does not prove one. The signals a change cannot forge are the forge's
//! review state and commit signatures; `discipline doctor` reads those from branch
//! protection. `directives.require_approval` reads the reviews only for a change that
//! carries an override directive.
//!
//! `required_trailers` is judged for each entry of a squash when the forge's message lets
//! the entries be told apart ([`squash_entries`]), and for the whole message otherwise,
//! with a note. The agent rule reads the squash as one commit. Only a `Co-authored-by:`
//! line tells that a squashed commit had a machine author: the layout has no author per
//! entry, and the squash commit's own author is whoever merged.
//!
//! Default off: which trailers a repository requires is its own policy.

use super::{Context, GateOutcome};
use crate::config::GateSettings;
use crate::gitctx::CommitDetail;
use crate::tokens;
use anyhow::Result;

pub const GATE: &str = "commit-provenance";

/// Trailer lines of a commit message, in the order written. Case of the key is kept.
///
/// The baseline is git's (`git interpret-trailers`): the last paragraph, when every line
/// of it is `Key: value`. Two forge layouts widen it, because a squash merge moves the
/// trailers an author wrote away from the end of the message:
///
/// - A squash of one commit can split one block into paragraphs: GitHub writes each
///   trailer it does not recognise as its own paragraph and appends its own
///   `Signed-off-by:` / `Co-authored-by:` block last. So the trailing run of paragraphs
///   in which every line is a trailer is read, back to the first paragraph that holds any
///   other line.
/// - A squash of several commits lists them: a paragraph that starts with `* ` at the
///   start of a line opens an entry (`* subject`, then that commit's body and trailers),
///   and a paragraph that is one line of dashes separates the last entry from the block
///   the forge gathers. The run of trailer paragraphs that ends each entry, directly
///   before the next entry or the dashed line, is read the same way as the run that ends
///   the message.
///
/// The subject paragraph is never read. A line that starts with whitespace continues the
/// trailer above it, as in git, and a paragraph that opens with one is prose. The line
/// `git cherry-pick -x` appends to a trailer block is skipped. CRLF line endings are read
/// as LF, and a line of whitespace separates paragraphs like an empty one.
pub fn trailers(message: &str) -> Vec<(String, String)> {
    let paragraphs = paragraphs(message);
    let mut out = Vec::new();
    // Trailer paragraphs seen since the last prose paragraph: kept when an entry or the
    // message ends right after them, dropped when prose follows.
    let mut run: Vec<(String, String)> = Vec::new();
    // The subject paragraph is never a trailer block, whatever it looks like.
    for paragraph in paragraphs.iter().skip(1) {
        if ends_a_squash_entry(paragraph) {
            out.append(&mut run);
            continue;
        }
        match trailer_block(paragraph) {
            Some(block) => run.extend(block),
            None => run.clear(),
        }
    }
    out.append(&mut run);
    out
}

/// The paragraphs of a message, each as its lines. `str::lines` ends a line at LF or CRLF,
/// and a line of whitespace ends a paragraph.
fn paragraphs(message: &str) -> Vec<Vec<&str>> {
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in message.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
}

/// One commit of a squash, as the forge's message lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The entry's `* subject` line without the marker, as written.
    pub subject: String,
    /// The trailer paragraphs that end the entry, read as the end of a message is.
    pub trailers: Vec<(String, String)>,
}

/// How the `* ` paragraphs of a message were read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entries {
    /// Fewer than two one-line `* ` paragraphs and no entry directly under the subject:
    /// an ordinary message.
    None,
    /// The message lists the commits of a squash, and each one's text can be told apart.
    Delimited(Vec<Entry>),
    /// The message has several one-line `* ` paragraphs that the layout does not place:
    /// they may be a list in an ordinary message. The reason, for the note.
    Undelimited(&'static str),
}

/// A paragraph that opens an entry of a squash: one line that starts with `* `. A
/// paragraph of several lines that starts with `* ` is a list in a body.
fn opens_an_entry(paragraph: &[&str]) -> bool {
    paragraph.len() == 1 && paragraph[0].starts_with("* ")
}

fn is_dashed_rule(paragraph: &[&str]) -> bool {
    let first = paragraph[0];
    paragraph.len() == 1 && first.len() >= 3 && first.chars().all(|c| c == '-')
}

/// The pull request number a subject ends with, `(#12)`: what a forge appends to the
/// title of the commit it writes for a squash. A `(#12)` anywhere else in the subject is
/// text of the subject, and `(#0)` is no pull request. The one reading of a number from a
/// subject: `replay` and `audit` use it too.
pub fn trailing_pull_number(subject: &str) -> Option<u64> {
    let (_, digits) = subject.trim_end().strip_suffix(')')?.rsplit_once("(#")?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0)
}

/// The entries of a forge's message for a squash of several commits, when each can be
/// delimited.
///
/// The layout read is the one a forge writes by default: the subject ends with the pull
/// request number, the paragraph directly under it is `* subject` on one line, and every
/// later one-line `* ` paragraph opens the next entry. An entry runs to the next one, to
/// the dashed rule that is followed by nothing but trailer paragraphs (the block the
/// forge gathers, which belongs to no entry), or to the end of the message. A dashed
/// line anywhere else is body text of its entry.
///
/// A one-line `* ` paragraph in the body of a squashed commit cannot be told from the
/// start of the next entry: it is read as one.
pub fn squash_entries(message: &str) -> Entries {
    let paragraphs = paragraphs(message);
    let openers: Vec<usize> = (1..paragraphs.len())
        .filter(|&i| opens_an_entry(&paragraphs[i]))
        .collect();
    let Some(&first) = openers.first() else {
        return Entries::None;
    };
    let numbered = trailing_pull_number(paragraphs[0][0]).is_some();
    if !numbered || first != 1 {
        // One such paragraph is a list item in a body far more often than a squash.
        if openers.len() < 2 {
            return Entries::None;
        }
        return Entries::Undelimited(if numbered {
            "the first `* ` paragraph does not follow the subject directly"
        } else {
            "the subject does not end with a pull request number such as `(#12)`"
        });
    }
    // The block the forge gathers: everything after the last dashed rule, when all of it
    // is trailers.
    let last = *openers.last().unwrap_or(&first);
    let end = (last + 1..paragraphs.len())
        .rev()
        .find(|&i| {
            is_dashed_rule(&paragraphs[i])
                && paragraphs[i + 1..]
                    .iter()
                    .all(|p| trailer_block(p).is_some())
        })
        .unwrap_or(paragraphs.len());
    let bounds = openers.iter().copied().chain([end]).collect::<Vec<_>>();
    let entries = bounds
        .windows(2)
        .map(|w| {
            let mut run: Vec<(String, String)> = Vec::new();
            for paragraph in &paragraphs[w[0] + 1..w[1]] {
                match trailer_block(paragraph) {
                    Some(block) => run.extend(block),
                    None => run.clear(),
                }
            }
            Entry {
                subject: paragraphs[w[0]][0][2..].trim().to_string(),
                trailers: run,
            }
        })
        .collect();
    Entries::Delimited(entries)
}

/// An entry's subject as a finding quotes it: one line, without the characters that would
/// end the code span around it, cut to a length a report line can hold.
fn quoted_subject(subject: &str) -> String {
    const MAX: usize = 60;
    let clean: String = subject
        .chars()
        .map(|c| if c == '`' || c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() > MAX {
        let cut: String = clean.chars().take(MAX).collect();
        format!("{}...", cut.trim_end())
    } else {
        clean.to_string()
    }
}

/// Whether a paragraph is a boundary in a forge's message for a squash of several
/// commits: the `* subject` paragraph that opens the next entry, or the line of dashes
/// before the trailers the forge gathers.
fn ends_a_squash_entry(paragraph: &[&str]) -> bool {
    let first = paragraph[0];
    first.starts_with("* ") || is_dashed_rule(paragraph)
}

/// `(cherry picked from commit <sha>)`, which `git cherry-pick -x` appends to the last
/// paragraph of a message, trailer block or not.
fn is_cherry_pick_line(line: &str) -> bool {
    line.strip_prefix("(cherry picked from commit ")
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The `Key: value` lines of one paragraph, or `None` when any line is not a trailer:
/// that paragraph is prose.
fn trailer_block(paragraph: &[&str]) -> Option<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in paragraph {
        let line = raw.trim();
        if is_cherry_pick_line(line) {
            continue;
        }
        if raw.starts_with([' ', '\t']) {
            // A continuation of the trailer above; with none above, an indented block.
            let (_, value) = out.last_mut()?;
            value.push(' ');
            value.push_str(line);
            continue;
        }
        let (k, v) = line.split_once(':')?;
        let k = k.trim();
        if k.is_empty()
            || k.contains(' ')
            || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return None;
        }
        out.push((k.to_string(), v.trim().to_string()));
    }
    Some(out)
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
    /// What tells this finding apart from another of its code in the run, for the
    /// baseline fingerprint ([`crate::guards::Violation::anchor`]): the commit, and for a
    /// missing trailer the key too, since one commit can lack several, and the entry's
    /// position when the commit is a squash judged entry by entry.
    pub anchor: String,
    /// For a trailer missing from one entry of a squash, the entry's position from 1.
    pub entry: Option<usize>,
}

pub fn judge(
    commits: &[CommitDetail],
    required: &[String],
    markers: &[String],
    review_key: &str,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for c in commits {
        // Merge commits (e.g. GitHub Actions pull_request test merge refs) do not
        // carry developer commit trailers.
        if c.parent_count > 1 {
            continue;
        }
        let t = trailers(&c.message);
        let short: String = c.sha.chars().take(10).collect();
        match (required.is_empty(), squash_entries(&c.message)) {
            (true, _) => {}
            // Each entry is one commit of the pull request: a trailer in another entry,
            // or in the block the forge gathers from all of them, is not this one's.
            (false, Entries::Delimited(entries)) => {
                for (i, entry) in entries.iter().enumerate() {
                    let n = i + 1;
                    for key in required {
                        if !has_trailer(&entry.trailers, key) {
                            out.push(Finding {
                                kind: &crate::findings::COMMIT_TRAILER_MISSING,
                                sha: c.sha.clone(),
                                what: format!(
                                    "squash commit {short}: entry {n} of {}, `{}`, has no `{key}:` trailer",
                                    entries.len(),
                                    quoted_subject(&entry.subject)
                                ),
                                // The position, not the subject: two entries can share one.
                                anchor: format!(
                                    "commit:{}:{}:entry:{n}",
                                    c.sha,
                                    key.to_ascii_lowercase()
                                ),
                                entry: Some(n),
                            });
                        }
                    }
                }
            }
            (false, Entries::None | Entries::Undelimited(_)) => {
                for key in required {
                    if !has_trailer(&t, key) {
                        out.push(Finding {
                            kind: &crate::findings::COMMIT_TRAILER_MISSING,
                            sha: c.sha.clone(),
                            what: format!("commit {short} has no `{key}:` trailer"),
                            // Keys match without regard to case, so the anchor does too.
                            anchor: format!("commit:{}:{}", c.sha, key.to_ascii_lowercase()),
                            entry: None,
                        });
                    }
                }
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
                    anchor: format!("commit:{}", c.sha),
                    entry: None,
                });
            } else if !reviewed_by_someone_else(c, &t, review_key) {
                out.push(Finding {
                    kind: &crate::findings::AGENT_COMMIT_REVIEWED_BY_AUTHOR,
                    sha: c.sha.clone(),
                    what: format!(
                        "commit {short} is agent-produced and its `{review_key}:` trailer names its own author"
                    ),
                    anchor: format!("commit:{}", c.sha),
                    entry: None,
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
    let non_merges: Vec<&CommitDetail> = commits.iter().filter(|c| c.parent_count <= 1).collect();
    if non_merges.is_empty() {
        out.notes
            .push("not evaluated: no non-merge commits between the base and HEAD".to_string());
        return Ok(out);
    }
    // An empty review key switches the agent rule off in `judge`.
    let review_key = if settings.require_agent_review {
        settings.review_trailer.as_str()
    } else {
        ""
    };
    if settings.required_trailers.is_empty() {
        let review_off = if review_key.is_empty() {
            Some("`require_agent_review` is false")
        } else if settings.agent_markers.is_empty() {
            Some("`agent_markers` is empty, so no commit can be read as agent-produced")
        } else {
            None
        };
        if let Some(why) = review_off {
            out.notes.push(format!(
                "not evaluated: no rule is on (`required_trailers` is empty and {why}), so none of the {} commit(s) between the base and HEAD was checked",
                non_merges.len()
            ));
            return Ok(out);
        }
    }
    out.examined = non_merges.len();
    // One rule on and the other off: say which was not evaluated, so a clean outcome is
    // not read as both.
    if settings.required_trailers.is_empty() {
        out.notes.push(
            "the required-trailer rule was not evaluated: `required_trailers` is empty, so only the agent-review rule was checked"
                .to_string(),
        );
    } else if review_key.is_empty() {
        out.notes.push(
            "the agent-review rule was not evaluated: `require_agent_review` is false, so only `required_trailers` was checked"
                .to_string(),
        );
    } else if settings.agent_markers.is_empty() {
        out.notes.push(
            "the agent-review rule was not evaluated: `agent_markers` is empty, so no commit can be read as agent-produced and only `required_trailers` was checked"
                .to_string(),
        );
    }
    if !settings.required_trailers.is_empty() {
        for c in &non_merges {
            if let Entries::Undelimited(why) = squash_entries(&c.message) {
                let short: String = c.sha.chars().take(7).collect();
                out.notes.push(format!(
                    "commit {short}: its `* ` paragraphs were not read as the entries of a squash ({why}), so `required_trailers` was judged on the whole message: a trailer in any paragraph that ends an entry counts for all of them"
                ));
            }
        }
    }
    for f in judge(
        &commits,
        &settings.required_trailers,
        &settings.agent_markers,
        review_key,
    ) {
        let short: String = f.sha.chars().take(7).collect();
        let ov = ctx
            .find_override(GATE, f.kind, tokens::ALLOW_COMMIT_PROVENANCE, &short)
            .or_else(|| ctx.find_override(GATE, f.kind, tokens::ALLOW_COMMIT_PROVENANCE, &f.sha));
        let anchored = ov.is_none();
        out.lift_or_push(
            ov,
            ctx.overridable(settings.severity()),
            f.kind,
            (None, None),
            format!("{}.", f.what),
            &if f.entry.is_some() {
                format!(
                    "Each commit of a pull request carries the trailer before it is squashed; for this one, justify it on its own line in the PR body: `allow-commit-provenance: {short} <reason>`."
                )
            } else {
                format!(
                    "Amend the commit with the trailer, or justify it on its own line in the PR body: `allow-commit-provenance: {short} <reason>`."
                )
            },
        );
        // No file and no line: without the anchor every finding of one code would share a
        // fingerprint, and one baseline entry would hide a finding on any other commit.
        if anchored {
            out.anchor_last(f.anchor);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Severity;
    use crate::guards::Violation;

    fn commit(author: &str, email: &str, message: &str) -> CommitDetail {
        CommitDetail {
            sha: "0123456789abcdef".into(),
            author_name: author.into(),
            author_email: email.into(),
            message: message.into(),
            parent_count: 1,
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
    fn trailers_split_into_paragraphs_by_a_squash_merge_are_all_read() {
        // The layout GitHub writes when it squash-merges a commit whose one trailer block
        // held trailers it does not recognise.
        let squashed = "chore: bump\n\nBody prose.\n\nReviewed-by: Owner <owner@x>\n\n\
                        Session: https://example.test/s\n\n\
                        Signed-off-by: Bot <noreply@anthropic.com>\n\
                        Co-authored-by: Bot <noreply@anthropic.com>\n";
        let keys: Vec<String> = trailers(squashed).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            ["Reviewed-by", "Session", "Signed-off-by", "Co-authored-by"]
        );
        let markers = v(&["noreply@anthropic.com"]);
        let c = commit("Bot", "noreply@anthropic.com", squashed);
        assert!(judge(&[c], &[], &markers, "Reviewed-by").is_empty());

        // A prose paragraph ends the run: a `Key: value` paragraph before it is body text.
        let interrupted = "chore: bump\n\nReviewed-by: Owner <owner@x>\n\nProse after it.\n\n\
                           Co-authored-by: Bot <noreply@anthropic.com>\n";
        let keys: Vec<String> = trailers(interrupted).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["Co-authored-by"]);
        let c = commit("Bot", "noreply@anthropic.com", interrupted);
        assert_eq!(
            judge(&[c], &[], &markers, "Reviewed-by")[0].kind.title,
            "Agent Commit Without Review"
        );
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

    /// GitHub's message for a squash of two commits, as it writes it: the pull request
    /// title, one `* subject` entry per commit followed by that commit's body and trailers,
    /// then a dashed rule and the `Signed-off-by:` / `Co-authored-by:` lines it gathers.
    fn squash(first_trailers: &str, second_trailers: &str, gathered: &str) -> String {
        let mut m = format!(
            "feat: parser (#7)\n\n* feat: parser\n\nFirst body, wrapped\nover two lines.\n\n\
             {first_trailers}\n\n* test: cover the parser\n\nSecond body.\n\n{second_trailers}\n"
        );
        if !gathered.is_empty() {
            m.push_str(&format!("\n---------\n\n{gathered}\n"));
        }
        m
    }

    fn keys(message: &str) -> Vec<String> {
        trailers(message).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn trailers_of_every_entry_of_a_multi_commit_squash_are_read() {
        let m = squash(
            "Reviewed-by: Rev Iewer <rev@example.com>\nAgent-Tool: coder 1.2",
            "Ticket: 12",
            "Signed-off-by: Dev Eloper <dev@example.com>\nCo-authored-by: Dev Eloper <dev@example.com>",
        );
        assert_eq!(
            keys(&m),
            [
                "Reviewed-by",
                "Agent-Tool",
                "Ticket",
                "Signed-off-by",
                "Co-authored-by"
            ]
        );
        // Without the gathered block the last entry's trailers end the message.
        let m = squash("Agent-Tool: coder 1.2", "Ticket: 12", "");
        assert_eq!(keys(&m), ["Agent-Tool", "Ticket"]);
        // Entries that are a subject and nothing else.
        assert_eq!(
            keys("feat: x (#7)\n\n* feat: x\n\n* test: x\n\n---------\n\nCo-authored-by: D <d@example.com>\n"),
            ["Co-authored-by"]
        );
    }

    #[test]
    fn a_squash_entry_reads_only_the_paragraphs_that_end_it() {
        // Control: inside an entry the rule is the one for a whole message. A `Key: value`
        // paragraph followed by prose is body text, in the first entry as in the last.
        let m = "feat: x (#7)\n\n* feat: x\n\nAgent-Tool: coder 1.2\n\nProse after it.\n\n\
                 * test: x\n\nReviewed-by: R <r@example.com>\n\nProse after it.\n\nTicket: 12\n";
        assert_eq!(keys(m), ["Ticket"]);
        // Control: a bullet inside a paragraph, an indented bullet and a dashed line inside
        // a paragraph start nothing.
        let m = "feat: x\n\nAgent-Tool: coder 1.2\n\nNotes:\n* one\n  * two\n-----\n\nTicket: 12\n";
        assert_eq!(keys(m), ["Ticket"]);
    }

    #[test]
    fn the_agent_rule_reads_a_squash_in_both_directions() {
        let markers = v(&["Agent-Tool:", "Co-authored-by: Claude"]);
        // The review sits in the first entry, the marker in the gathered block.
        let reviewed = squash(
            "Reviewed-by: Rev Iewer <rev@example.com>",
            "Ticket: 12",
            "Co-authored-by: Claude <agent@example.com>",
        );
        let c = commit("Dev Eloper", "dev@example.com", &reviewed);
        assert!(judge(&[c], &[], &markers, "Reviewed-by").is_empty());
        // The marker sits in the first entry and nowhere else.
        let marked = squash(
            "Agent-Tool: coder 1.2",
            "Ticket: 12",
            "Signed-off-by: Dev Eloper <dev@example.com>",
        );
        let c = commit("Dev Eloper", "dev@example.com", &marked);
        let f = judge(&[c], &[], &markers, "Reviewed-by");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind.title, "Agent Commit Without Review");
        // A person reviewing their own agent-assisted commit, squashed.
        let own = squash(
            "Reviewed-by: Dev Eloper <dev@example.com>\nAgent-Tool: coder 1.2",
            "Ticket: 12",
            "Signed-off-by: Dev Eloper <dev@example.com>",
        );
        let c = commit("Dev Eloper", "dev@example.com", &own);
        let f = judge(&[c], &[], &markers, "Reviewed-by");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind.title, "Agent Commit Reviewed By Its Author");
    }

    #[test]
    fn an_indented_line_is_a_continuation_and_never_a_trailer() {
        // A quoted block of another commit's trailers, indented.
        let quoted = "fix: x\n\nThe reverted commit ended with:\n\n    Signed-off-by: Dev Eloper <dev@example.com>\n    Agent-Tool: coder 1.2\n";
        assert!(trailers(quoted).is_empty());
        // git folds an indented line into the trailer above it.
        let folded = "fix: x\n\nReviewed-by: Rev Iewer\n  <rev@example.com>\nTicket: 12\n";
        assert_eq!(
            trailers(folded),
            [
                (
                    "Reviewed-by".to_string(),
                    "Rev Iewer <rev@example.com>".to_string()
                ),
                ("Ticket".to_string(), "12".to_string())
            ]
        );
        // Control: the same lines unindented are trailers.
        assert_eq!(
            keys("fix: x\n\nSigned-off-by: Dev Eloper <dev@example.com>\nAgent-Tool: coder 1.2\n"),
            ["Signed-off-by", "Agent-Tool"]
        );
    }

    #[test]
    fn a_cherry_pick_line_and_crlf_line_endings_do_not_void_the_block() {
        let picked = "fix: x\n\nBody.\n\nSigned-off-by: Dev Eloper <dev@example.com>\n\
                      (cherry picked from commit 0123456789abcdef0123456789abcdef01234567)\n";
        assert_eq!(keys(picked), ["Signed-off-by"]);
        // Control: any other non-trailer line still makes the paragraph prose.
        let prose = "fix: x\n\nSigned-off-by: Dev Eloper <dev@example.com>\n(see the tracker)\n";
        assert!(trailers(prose).is_empty());
        let crlf = "fix: x\r\n\r\nBody.\r\n\r\nSigned-off-by: Dev Eloper <dev@example.com>\r\n";
        assert_eq!(
            trailers(crlf),
            [(
                "Signed-off-by".to_string(),
                "Dev Eloper <dev@example.com>".to_string()
            )]
        );
        // Control: the subject of a CRLF message is still never a trailer.
        assert!(trailers("fix: x\r\n").is_empty());
    }

    /// The findings of `commits` as the gate reports them: no file, no line, anchored.
    fn reported(commits: &[CommitDetail], required: &[&str], anchored: bool) -> Vec<Violation> {
        let mut out = GateOutcome::new(GATE);
        for f in judge(commits, &v(required), &v(&["Agent-Tool:"]), "Reviewed-by") {
            out.push(
                Severity::Error,
                f.kind,
                None,
                None,
                format!("{}.", f.what),
                "",
            );
            if anchored {
                out.anchor_last(f.anchor);
            }
        }
        out.violations
    }

    fn fingerprints(mut found: Vec<Violation>) -> Vec<String> {
        let mut refs: Vec<&mut Violation> = found.iter_mut().collect();
        crate::baseline::fill_fingerprints(&mut refs, |_| None);
        found.into_iter().map(|f| f.fingerprint).collect()
    }

    #[test]
    fn each_finding_is_anchored_by_its_commit_and_a_missing_trailer_by_its_key_too() {
        let mut one = commit("Ada", "ada@x", "feat: one\n");
        one.sha = "1111111111111111111111111111111111111111".into();
        let mut two = commit("Ada", "ada@x", "feat: two\n\nAgent-Tool: coder 1.2\n");
        two.sha = "2222222222222222222222222222222222222222".into();
        let f = judge(
            &[one.clone(), two.clone()],
            &v(&["Signed-off-by", "Ticket"]),
            &v(&["Agent-Tool:"]),
            "Reviewed-by",
        );
        let anchors: Vec<&str> = f.iter().map(|f| f.anchor.as_str()).collect();
        assert_eq!(
            anchors,
            [
                "commit:1111111111111111111111111111111111111111:signed-off-by",
                "commit:1111111111111111111111111111111111111111:ticket",
                "commit:2222222222222222222222222222222222222222:signed-off-by",
                "commit:2222222222222222222222222222222222222222:ticket",
                "commit:2222222222222222222222222222222222222222",
            ]
        );
        // The key is matched without regard to case, and anchored the same way.
        let upper = judge(std::slice::from_ref(&one), &v(&["SIGNED-OFF-BY"]), &[], "");
        assert_eq!(upper[0].anchor, anchors[0]);

        // Five findings, three of one code on two commits: every fingerprint is its own,
        // and a debug build does not stop on a repeat (#599).
        let all = fingerprints(reported(
            &[one.clone(), two.clone()],
            &["Signed-off-by", "Ticket"],
            true,
        ));
        let distinct: std::collections::HashSet<&String> = all.iter().collect();
        assert_eq!(distinct.len(), 5, "{all:?}");
        // A finding keeps its fingerprint whatever else the run reports, in any order.
        let alone = fingerprints(reported(
            std::slice::from_ref(&one),
            &["Signed-off-by"],
            true,
        ));
        assert_eq!(alone[0], all[0]);
        let reversed = fingerprints(reported(&[two, one], &["Ticket", "Signed-off-by"], true));
        assert_eq!(reversed[4], all[0]);
    }

    /// What the anchor changes in a version-2 baseline, and what it leaves alone.
    #[test]
    fn the_anchor_changes_the_version_two_fingerprint_and_not_the_version_one() {
        let mut one = commit("Ada", "ada@x", "feat: one\n");
        one.sha = "1111111111111111111111111111111111111111".into();
        let mut two = commit("Ada", "ada@x", "feat: two\n");
        two.sha = "2222222222222222222222222222222222222222".into();
        let none = |_: &str| None;
        let fp = |c: &CommitDetail, anchored: bool, version: u32| {
            let found = reported(std::slice::from_ref(c), &["Signed-off-by"], anchored);
            crate::baseline::fingerprint_for_version(&found[0], none, version)
        };
        // Control, the fingerprint before #599: a lone finding hashed its code and nothing
        // else, so it was one value for every commit. This is the value a release build of
        // the parent commit printed for two unrelated commits.
        const UNANCHORED: &str = "91d0d430d82518c8483b6ff7a74d00df81d460213fdca7f5460da11a3d9cd93d";
        assert_eq!(fp(&one, false, 2), UNANCHORED);
        assert_eq!(fp(&two, false, 2), UNANCHORED);
        // Anchored, it is the commit's own, so an entry written before no longer matches.
        assert_ne!(fp(&one, true, 2), UNANCHORED);
        assert_ne!(fp(&one, true, 2), fp(&two, true, 2));
        // Version 1 hashes the message and never the anchor.
        assert_eq!(fp(&one, true, 1), fp(&one, false, 1));
        assert_ne!(fp(&one, true, 1), fp(&two, true, 1));
    }

    fn subjects_and_keys(message: &str) -> Vec<(String, Vec<String>)> {
        match squash_entries(message) {
            Entries::Delimited(entries) => entries
                .into_iter()
                .map(|e| (e.subject, e.trailers.into_iter().map(|(k, _)| k).collect()))
                .collect(),
            other => panic!("not delimited: {other:?}"),
        }
    }

    #[test]
    fn the_entries_of_a_squash_are_delimited_with_their_own_trailers() {
        let m = squash(
            "Signed-off-by: Dev Eloper <dev@example.com>\nAgent-Tool: coder 1.2",
            "Ticket: 12",
            "Signed-off-by: Dev Eloper <dev@example.com>\nCo-authored-by: Dev Eloper <dev@example.com>",
        );
        let own = |keys: &[&str]| keys.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(
            subjects_and_keys(&m),
            [
                (
                    "feat: parser".to_string(),
                    own(&["Signed-off-by", "Agent-Tool"])
                ),
                ("test: cover the parser".to_string(), own(&["Ticket"])),
            ]
        );
        // Without the gathered block the last entry runs to the end of the message.
        let m = squash(
            "Ticket: 1",
            "Ticket: 2\nSigned-off-by: D <d@example.com>",
            "",
        );
        assert_eq!(
            subjects_and_keys(&m)[1].1,
            own(&["Ticket", "Signed-off-by"])
        );
        // A dashed line that is not followed by trailers only is body text of its entry,
        // as is a list of several lines and an indented bullet.
        let m = "feat: x (#7)\n\n* feat: x\n\n---\n\nProse.\n\n* one\n* two\n\n  * three\n\nTicket: 1\n\n\
                 * test: x\n\nTicket: 2\n\n---------\n\nSigned-off-by: D <d@example.com>\n";
        assert_eq!(
            subjects_and_keys(m),
            [
                ("feat: x".to_string(), own(&["Ticket"])),
                ("test: x".to_string(), own(&["Ticket"])),
            ]
        );
        // Inside an entry the rule is the one for a whole message: prose ends the run.
        let m = "feat: x (#7)\n\n* feat: x\n\nTicket: 1\n\nProse after it.\n\n* test: x\n";
        assert_eq!(
            subjects_and_keys(m),
            [
                ("feat: x".to_string(), own(&[])),
                ("test: x".to_string(), own(&[])),
            ]
        );
    }

    #[test]
    fn a_message_is_not_split_unless_the_layout_places_every_entry() {
        // Control: the same paragraphs under a subject with a pull request number.
        let body = "\n\n* feat: x\n\nTicket: 1\n\n* test: x\n\nTicket: 2\n";
        assert!(matches!(
            squash_entries(&format!("feat: x (#7){body}")),
            Entries::Delimited(e) if e.len() == 2
        ));
        // No pull request number at the end of the subject.
        for subject in [
            "feat: x",
            "feat: x (#7) again",
            "feat: x (#)",
            "feat: x (#7a)",
        ] {
            assert!(
                matches!(
                    squash_entries(&format!("{subject}{body}")),
                    Entries::Undelimited(why) if why.contains("pull request number")
                ),
                "{subject}"
            );
        }
        // Text between the subject and the first entry.
        assert!(matches!(
            squash_entries(&format!("feat: x (#7)\n\nA description.{body}")),
            Entries::Undelimited(why) if why.contains("does not follow the subject")
        ));
        // One `* ` paragraph that is not directly under a numbered subject, a list of
        // several lines, and no `* ` paragraph at all: an ordinary message.
        for m in [
            "feat: x\n\n* one\n\nTicket: 1\n",
            "feat: x (#7)\n\nBody.\n\n* one\n\nTicket: 1\n",
            "feat: x (#7)\n\n* one\n* two\n\nTicket: 1\n",
            "feat: x (#7)\n\nBody.\n\nTicket: 1\n",
            "feat: x (#7)",
        ] {
            assert_eq!(squash_entries(m), Entries::None, "{m}");
        }
    }

    #[test]
    fn a_required_trailer_is_judged_for_each_entry_of_a_delimited_squash() {
        let signed = "Signed-off-by: Dev Eloper <dev@example.com>";
        let required = v(&["Signed-off-by"]);
        let judge_one = |message: &str| {
            let mut c = commit("Dev Eloper", "dev@example.com", message);
            c.sha = "1111111111111111111111111111111111111111".into();
            judge(&[c], &required, &[], "")
        };
        // Control: every entry signed off.
        assert!(judge_one(&squash(signed, signed, signed)).is_empty());
        // The sign-off of the first entry, and the one the forge gathers, do not cover
        // the second.
        let f = judge_one(&squash(signed, "Ticket: 12", signed));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].entry, Some(2));
        assert_eq!(
            f[0].what,
            "squash commit 1111111111: entry 2 of 2, `test: cover the parser`, has no `Signed-off-by:` trailer"
        );
        assert_eq!(
            f[0].anchor,
            "commit:1111111111111111111111111111111111111111:signed-off-by:entry:2"
        );
        // Two unsigned entries: two findings, anchored apart.
        let f = judge_one(&squash("Ticket: 1", "Ticket: 2", signed));
        let anchors: Vec<&str> = f.iter().map(|f| f.anchor.as_str()).collect();
        assert_eq!(
            anchors,
            [
                "commit:1111111111111111111111111111111111111111:signed-off-by:entry:1",
                "commit:1111111111111111111111111111111111111111:signed-off-by:entry:2",
            ]
        );
        // A message that is not delimited keeps the whole-message judgement and anchor.
        let whole = squash(signed, "Ticket: 12", "").replace(" (#7)", "");
        assert!(judge_one(&whole).is_empty());
        let whole = squash("Ticket: 1", "Ticket: 2", "").replace(" (#7)", "");
        let f = judge_one(&whole);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].entry, None);
        assert_eq!(
            f[0].anchor,
            "commit:1111111111111111111111111111111111111111:signed-off-by"
        );
        // The agent rule still reads the squash as one commit.
        let mut c = commit(
            "Dev Eloper",
            "dev@example.com",
            &squash(
                "Reviewed-by: Rev Iewer <rev@example.com>",
                "Agent-Tool: coder 1.2",
                "",
            ),
        );
        c.sha = "1111111111111111111111111111111111111111".into();
        assert!(judge(&[c], &[], &v(&["Agent-Tool:"]), "Reviewed-by").is_empty());
    }

    #[test]
    fn an_entry_subject_is_quoted_on_one_line_and_cut() {
        assert_eq!(quoted_subject("feat: `x`\u{1b}[2J"), "feat:  x  [2J");
        let long = "a".repeat(80);
        let q = quoted_subject(&long);
        assert_eq!(q.chars().count(), 63);
        assert!(q.ends_with("..."));
        assert_eq!(quoted_subject(&"a".repeat(60)), "a".repeat(60));
    }

    #[test]
    fn merge_commits_are_skipped() {
        let markers = v(&["Agent-Tool:"]);
        let mut merge = commit("GitHub", "noreply@github.com", "Merge abc into def\n");
        merge.parent_count = 2;
        assert!(judge(&[merge], &v(&["Signed-off-by"]), &markers, "Reviewed-by").is_empty());
    }
}
