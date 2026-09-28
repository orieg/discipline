//! Owner ratification of edits to protected paths (`ratified-paths`), read from the forge.
//!
//! A pull request that edits a protected path passes only when an issue it **closes**
//! carries a comment, written by a listed human login, that names the path exactly:
//!
//! ```text
//! Owner-ratified-paths:
//! - scripts/check_x.py
//! - docs/OPS.md
//! ```
//!
//! The authority is the comment's author as the forge reports it, which is why the
//! ratification lives in a comment and not in the pull request's body (a body is written
//! by whoever opened the pull request). Everything that could make a comment say what its
//! author did not write is refused: an edited comment (on Gitea and Forgejo anyone who can
//! write issues can edit another user's comment, and it keeps its author), a comment
//! imported by a migration, a GitHub comment created by an email reply, a bot, a GitLab
//! system note.
//!
//! Every lookup ends in a verdict or a classified [`ForgeError`]: an issue number that does
//! not exist is a verdict and is never asked for its comments (Gitea answers 500, not 404,
//! for the comments of a missing issue); an unreachable forge is exit 2, never a pass.

use crate::config::{AcceptEdited, ClosingSource, RatificationWindow, RatifiedPathsGate};
use crate::could_not_check::{tag, Reason};
use crate::forge::{
    gitlab_project_id, read_all, Forge, ForgeApi, ForgeError, ForgeErrorKind, ForgeKind, MAX_PAGES,
};
use crate::references::{self, Reference, Verdict};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// Seconds since the Unix epoch of an RFC 3339 timestamp (`2026-09-27T10:00:00Z`,
/// `...+02:00`, fractional seconds allowed). `None` when it is not one.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    if d.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (c, off) = rest.split_at(at);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let (oh, om) = off[1..].split_once(':')?;
        (
            c,
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60),
        )
    };
    let mut c = clock.split(':');
    let (hh, mm): (i64, i64) = (c.next()?.parse().ok()?, c.next()?.parse().ok()?);
    let ss: i64 = c.next()?.split('.').next()?.parse().ok()?;
    if c.next().is_some() || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss - offset)
}

/// Why an entry of a ratification block is refused, or `None` for an exact path.
pub fn refuse_entry(entry: &str) -> Option<&'static str> {
    if entry.is_empty() {
        return Some("it is empty");
    }
    if entry.chars().any(|c| c.is_control()) {
        return Some("it holds a control character");
    }
    if entry.chars().any(|c| "*?[]{}!".contains(c)) {
        return Some("it is a glob; name each path exactly");
    }
    if entry.contains('\\') {
        return Some("it holds a backslash");
    }
    if entry.starts_with('/') {
        return Some("it starts with `/`; paths are relative to the repository root");
    }
    if entry.ends_with('/') {
        return Some("it ends with `/`; a directory is not a path, name each file");
    }
    if entry
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Some("it has an empty, `.` or `..` segment");
    }
    None
}

/// One ratification block of a comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Block {
    /// Every entry, as written (a surrounding pair of backticks removed).
    pub entries: Vec<String>,
    /// Entries refused, with the reason. One refused entry voids the whole block.
    pub refused: Vec<(String, &'static str)>,
}

/// The ratification blocks of a comment body: a line that is exactly `marker`, then its
/// `- <path>` lines. Fenced code, HTML comments and quoted lines (`> `) are not read, so
/// an example or a quotation of someone else's block ratifies nothing.
pub fn parse_blocks(body: &str, marker: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current: Option<Block> = None;
    let mut fence: Option<&str> = None;
    let mut in_comment = false;
    for line in body.lines() {
        let t = line.trim();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
            continue;
        }
        if in_comment {
            if t.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            if let Some(b) = current.take() {
                blocks.push(b);
            }
            fence = Some(if t.starts_with("```") { "```" } else { "~~~" });
            continue;
        }
        if t.starts_with("<!--") {
            if let Some(b) = current.take() {
                blocks.push(b);
            }
            in_comment = !t.contains("-->");
            continue;
        }
        if t == marker {
            if let Some(b) = current.take() {
                blocks.push(b);
            }
            current = Some(Block::default());
            continue;
        }
        if let Some(b) = current.as_mut() {
            if let Some(item) = t.strip_prefix("- ").or_else(|| (t == "-").then_some("")) {
                let item = item.trim();
                let item = item
                    .strip_prefix('`')
                    .and_then(|i| i.strip_suffix('`'))
                    .unwrap_or(item);
                match refuse_entry(item) {
                    Some(why) => b.refused.push((item.to_string(), why)),
                    None => b.entries.push(item.to_string()),
                }
                continue;
            }
            blocks.push(current.take().unwrap_or_default());
        }
    }
    if let Some(b) = current {
        blocks.push(b);
    }
    blocks
        .into_iter()
        .filter(|b| !b.entries.is_empty() || !b.refused.is_empty())
        .collect()
}

/// Whether and by whom a comment was edited after it was posted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edited {
    No,
    /// The forge names the editor (GitHub).
    By(String),
    /// Edited, by someone the forge does not name (GitLab, Gitea, Forgejo: `updated_at`
    /// differs from `created_at`).
    Unknown,
}

/// A comment as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    /// A bot or app account (GitHub `__typename` `Bot`).
    pub author_is_bot: bool,
    /// A note the forge wrote itself (GitLab `system`).
    pub system: bool,
    /// Imported by a migration (Gitea / Forgejo `original_author`).
    pub imported: bool,
    /// Created by an email reply (GitHub `createdViaEmail`).
    pub via_email: bool,
    pub created_at: i64,
    pub edited: Edited,
    pub body: String,
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for k in path {
        match cur.get(k) {
            Some(n) => cur = n,
            None => return "",
        }
    }
    cur.as_str().unwrap_or("")
}

fn time_at(v: &Value, key: &str) -> Result<i64, ForgeError> {
    parse_time(str_at(v, &[key])).ok_or_else(|| {
        ForgeError::new(
            ForgeErrorKind::Malformed,
            format!("a comment carries no readable `{key}`"),
        )
    })
}

fn owner_name(repo: &str) -> Result<(&str, &str), ForgeError> {
    repo.split_once('/').ok_or_else(|| {
        ForgeError::new(
            ForgeErrorKind::Malformed,
            format!("`{repo}` is not owner/name"),
        )
    })
}

/// The GitHub GraphQL query for an issue's comments, with what makes them trustworthy.
pub const ISSUE_COMMENTS_QUERY: &str = "query IssueComments($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { issue(number: $number) { comments(first: 100, after: $after) { totalCount pageInfo { hasNextPage endCursor } nodes { databaseId body createdAt lastEditedAt createdViaEmail author { login __typename } editor { login } } } } } }";

/// The variables of [`ISSUE_COMMENTS_QUERY`].
pub fn issue_comments_vars(repo: &str, number: u64, after: Option<&str>) -> Value {
    let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
    json!({"owner": owner, "name": name, "number": number, "after": after})
}

/// Every comment of issue `number` of `repo`, or an error: never part of them.
pub fn issue_comments(
    api: &dyn ForgeApi,
    forge: &Forge,
    repo: &str,
    number: u64,
) -> Result<Vec<Comment>, ForgeError> {
    match forge.kind {
        ForgeKind::GitHub => {
            owner_name(repo)?;
            let mut out = Vec::new();
            let mut after: Option<String> = None;
            for _ in 0..MAX_PAGES {
                let data = api.graphql(
                    forge,
                    ISSUE_COMMENTS_QUERY,
                    &issue_comments_vars(repo, number, after.as_deref()),
                )?;
                let comments = data
                    .pointer("/repository/issue/comments")
                    .filter(|c| !c.is_null())
                    .ok_or_else(|| {
                        ForgeError::new(
                            ForgeErrorKind::NotFound,
                            format!("issue #{number} of {repo} is not visible"),
                        )
                    })?;
                for n in comments["nodes"].as_array().into_iter().flatten() {
                    let created_at = time_at(n, "createdAt")?;
                    let edited = match n.get("lastEditedAt") {
                        None | Some(Value::Null) => Edited::No,
                        Some(_) => match str_at(n, &["editor", "login"]) {
                            "" => Edited::Unknown,
                            login => Edited::By(login.to_string()),
                        },
                    };
                    out.push(Comment {
                        id: n["databaseId"].to_string(),
                        author: str_at(n, &["author", "login"]).to_string(),
                        author_is_bot: str_at(n, &["author", "__typename"]) != "User",
                        system: false,
                        imported: false,
                        via_email: n["createdViaEmail"].as_bool().unwrap_or(false),
                        created_at,
                        edited,
                        body: str_at(n, &["body"]).to_string(),
                    });
                }
                if comments["pageInfo"]["hasNextPage"].as_bool() == Some(true) {
                    after = comments["pageInfo"]["endCursor"]
                        .as_str()
                        .map(str::to_string);
                    continue;
                }
                let total = comments["totalCount"].as_u64().unwrap_or(0);
                if out.len() as u64 != total {
                    return Err(ForgeError::new(
                        ForgeErrorKind::Partial,
                        format!(
                            "issue #{number} of {repo}: {} of {total} comments read; refusing to judge a partial list",
                            out.len()
                        ),
                    ));
                }
                return Ok(out);
            }
            Err(ForgeError::new(
                ForgeErrorKind::Partial,
                format!("issue #{number} of {repo} has more than {MAX_PAGES} pages of comments"),
            ))
        }
        ForgeKind::GitLab => {
            let path = format!("projects/{}/issues/{number}/notes", gitlab_project_id(repo));
            read_all(api, forge, &path)?
                .iter()
                .map(|n| {
                    let created_at = time_at(n, "created_at")?;
                    let updated_at = time_at(n, "updated_at").unwrap_or(created_at);
                    Ok(Comment {
                        id: n["id"].to_string(),
                        author: str_at(n, &["author", "username"]).to_string(),
                        author_is_bot: n["author"]["bot"].as_bool().unwrap_or(false),
                        system: n["system"].as_bool().unwrap_or(false),
                        imported: n["imported"].as_bool().unwrap_or(false),
                        via_email: false,
                        created_at,
                        edited: if updated_at == created_at {
                            Edited::No
                        } else {
                            Edited::Unknown
                        },
                        body: str_at(n, &["body"]).to_string(),
                    })
                })
                .collect()
        }
        ForgeKind::Gitea | ForgeKind::Forgejo => {
            let path = format!("repos/{repo}/issues/{number}/comments");
            read_all(api, forge, &path)?
                .iter()
                .map(|c| {
                    let created_at = time_at(c, "created_at")?;
                    let updated_at = time_at(c, "updated_at").unwrap_or(created_at);
                    Ok(Comment {
                        id: c["id"].to_string(),
                        author: str_at(c, &["user", "login"]).to_string(),
                        author_is_bot: false,
                        system: false,
                        imported: !str_at(c, &["original_author"]).is_empty(),
                        via_email: false,
                        created_at,
                        edited: if updated_at == created_at {
                            Edited::No
                        } else {
                            Edited::Unknown
                        },
                        body: str_at(c, &["body"]).to_string(),
                    })
                })
                .collect()
        }
    }
}

/// The GitHub GraphQL query for the issues a pull request closes.
pub const CLOSING_ISSUES_QUERY: &str = "query ClosingIssues($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { closingIssuesReferences(first: 50) { totalCount nodes { number repository { nameWithOwner } } } } } }";

/// The variables of [`CLOSING_ISSUES_QUERY`].
pub fn closing_issues_vars(repo: &str, number: u64) -> Value {
    let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
    json!({"owner": owner, "name": name, "number": number})
}

fn closed_reference(repo: &str, number: u64) -> Reference {
    Reference {
        text: format!("{repo}#{number}"),
        repo: Some(repo.to_string()),
        host: None,
        number,
        pull: false,
        closing: true,
    }
}

/// The issues pull request `number` closes: the forge's own list where it has one and
/// `source` is [`ClosingSource::Server`], otherwise the closing references of `body`.
pub fn closing_references(
    api: &dyn ForgeApi,
    forge: &Forge,
    number: u64,
    body: &str,
    source: ClosingSource,
    keywords: &[String],
) -> Result<Vec<Reference>, ForgeError> {
    match (source, forge.kind) {
        (ClosingSource::Server, ForgeKind::GitHub) => {
            let data = api.graphql(
                forge,
                CLOSING_ISSUES_QUERY,
                &closing_issues_vars(&forge.repo, number),
            )?;
            let refs = data
                .pointer("/repository/pullRequest/closingIssuesReferences")
                .filter(|r| !r.is_null())
                .ok_or_else(|| {
                    ForgeError::new(
                        ForgeErrorKind::NotFound,
                        format!("pull request #{number} is not visible"),
                    )
                })?;
            let nodes = refs["nodes"].as_array().cloned().unwrap_or_default();
            if refs["totalCount"].as_u64().unwrap_or(0) > nodes.len() as u64 {
                return Err(ForgeError::new(
                    ForgeErrorKind::Partial,
                    format!("pull request #{number} closes more issues than one page lists"),
                ));
            }
            Ok(nodes
                .iter()
                .filter_map(|n| {
                    Some(closed_reference(
                        str_at(n, &["repository", "nameWithOwner"]),
                        n["number"].as_u64()?,
                    ))
                })
                .collect())
        }
        (ClosingSource::Server, ForgeKind::GitLab) => {
            let path = format!(
                "projects/{}/merge_requests/{number}/closes_issues",
                gitlab_project_id(&forge.repo)
            );
            Ok(read_all(api, forge, &path)?
                .iter()
                .filter_map(|i| {
                    // `https://host/<project path>/-/issues/<iid>`
                    let url = str_at(i, &["web_url"]);
                    let path = url.split_once("://")?.1.split_once('/')?.1;
                    let repo = path.split_once("/-/")?.0;
                    Some(closed_reference(repo, i["iid"].as_u64()?))
                })
                .collect())
        }
        _ => Ok(references::parse(body, forge.kind, keywords)
            .into_iter()
            .filter(|r| r.closing)
            .collect()),
    }
}

/// When pull request `number` was opened.
pub fn pull_created_at(api: &dyn ForgeApi, forge: &Forge, number: u64) -> Result<i64, ForgeError> {
    let path = match forge.kind {
        ForgeKind::GitLab => format!(
            "projects/{}/merge_requests/{number}",
            gitlab_project_id(&forge.repo)
        ),
        _ => format!("repos/{}/pulls/{number}", forge.repo),
    };
    time_at(&api.fetch(forge, &path)?, "created_at")
}

/// A finding of the judgement, before it becomes a gate violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A protected path no usable ratification names.
    Unratified { path: String, why: String },
    /// A refused entry, which voids its block.
    EntryMalformed {
        anchor: String,
        entry: String,
        why: &'static str,
    },
    /// A block names a path no ratification can cover.
    NamesNeverRatifiable { anchor: String, entry: String },
    /// A block whose author is not accepted.
    AuthorNotAccepted { anchor: String, author: String },
    /// A block in a comment that was edited, imported or created by email.
    CommentRefused { anchor: String, why: &'static str },
    /// A block that names a path but is older than that path's window.
    OutsideWindow {
        anchor: String,
        path: String,
        why: String,
    },
}

/// What the ratification check concluded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Judgement {
    pub findings: Vec<Finding>,
    pub notes: Vec<String>,
}

/// Why `comment` cannot ratify, or `None` when it can.
fn refuse_comment(cfg: &RatifiedPathsGate, c: &Comment) -> Option<Finding> {
    let anchor = format!("comment:{}", c.id);
    let listed = |list: &[String]| list.iter().any(|l| l.eq_ignore_ascii_case(&c.author));
    if c.author.is_empty()
        || c.author_is_bot
        || c.system
        || !listed(&cfg.ratifiers)
        || listed(&cfg.agent_logins)
    {
        return Some(Finding::AuthorNotAccepted {
            anchor,
            author: if c.author.is_empty() {
                "(unknown)".into()
            } else {
                c.author.clone()
            },
        });
    }
    if c.imported {
        return Some(Finding::CommentRefused {
            anchor,
            why: "it was imported by a migration, so the author shown did not write it",
        });
    }
    if c.via_email && !cfg.accept_email_replies {
        return Some(Finding::CommentRefused {
            anchor,
            why: "it was created by an email reply",
        });
    }
    let edit_ok = match (&c.edited, cfg.accept_edited) {
        (Edited::No, _) => true,
        (Edited::By(e), AcceptEdited::ByAuthor) => e.eq_ignore_ascii_case(&c.author),
        _ => false,
    };
    if !edit_ok {
        return Some(Finding::CommentRefused {
            anchor,
            why: "it was edited after it was posted, and an edit can be made by someone other than its author",
        });
    }
    None
}

/// The inputs of [`judge`] that are not the forge.
pub struct Input<'a> {
    pub pull_number: u64,
    pub pull_body: &'a str,
    /// Changed protected paths (never-ratifiable ones already removed).
    pub protected: &'a [String],
    /// Globs no ratification can cover.
    pub never_ratifiable: &'a crate::guards::PathFilter,
    /// Committer time of the last base-branch change to a path; `None` if never changed.
    pub last_change: &'a dyn Fn(&str) -> Result<Option<i64>>,
    /// Now, in Unix seconds.
    pub now: i64,
}

/// Decide which protected paths are ratified. Errors are tagged for exit 2: `forge` for a
/// forge that cannot answer, `configuration` for a policy that cannot be applied.
pub fn judge(
    api: &dyn ForgeApi,
    forge: &Forge,
    cfg: &RatifiedPathsGate,
    input: &Input,
) -> Result<Judgement> {
    let forge_err = |what: &str, e: ForgeError| {
        tag(
            Reason::Forge,
            anyhow!("ratified-paths could not read {what}: {e}"),
        )
    };
    if cfg.accept_edited == AcceptEdited::ByAuthor && forge.kind != ForgeKind::GitHub {
        return Err(tag(
            Reason::Configuration,
            anyhow!(
                "`gates.ratified-paths.accept_edited = \"by-author\"` needs a forge that names a comment's editor; {} does not",
                forge.kind.label()
            ),
        ));
    }
    let mut out = Judgement::default();

    let closing = closing_references(
        api,
        forge,
        input.pull_number,
        input.pull_body,
        cfg.closing_source,
        &cfg.closing_keywords,
    )
    .map_err(|e| forge_err("the issues the pull request closes", e))?;
    let resolved = references::resolve_all(api, forge, &closing, &cfg.ratification_repos)
        .map_err(|e| forge_err("the issues the pull request closes", e))?;
    if resolved.is_empty() {
        out.notes
            .push("the pull request closes no issue, so nothing can ratify its edits".into());
    } else {
        out.notes.push(format!(
            "closing references: {}",
            resolved
                .iter()
                .map(|r| r.describe())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    let pull_created = if cfg.ratification_valid_from == RatificationWindow::PullCreated {
        Some(
            pull_created_at(api, forge, input.pull_number)
                .map_err(|e| forge_err("when the pull request was opened", e))?,
        )
    } else {
        None
    };

    // path -> (comment anchor, author) of a ratification in its window.
    let mut ratified: std::collections::BTreeMap<String, String> = Default::default();
    let mut lapsed: std::collections::BTreeMap<String, String> = Default::default();
    for r in &resolved {
        let usable = match r.verdict {
            Verdict::Issue => true,
            Verdict::Closed => !cfg.require_open_issue,
            _ => false,
        };
        if !usable {
            continue;
        }
        let comments = issue_comments(api, forge, &r.repo, r.reference.number)
            .map_err(|e| forge_err(&format!("the comments of {}", r.reference.text), e))?;
        for c in &comments {
            let blocks = parse_blocks(&c.body, &cfg.marker);
            if blocks.is_empty() {
                continue;
            }
            let anchor = format!("issue:{}#{}/comment:{}", r.repo, r.reference.number, c.id);
            if let Some(refusal) = refuse_comment(cfg, c) {
                out.findings.push(match refusal {
                    Finding::AuthorNotAccepted { author, .. } => {
                        Finding::AuthorNotAccepted { anchor, author }
                    }
                    Finding::CommentRefused { why, .. } => Finding::CommentRefused { anchor, why },
                    other => other,
                });
                continue;
            }
            for b in blocks {
                if !b.refused.is_empty() {
                    for (entry, why) in b.refused {
                        out.findings.push(Finding::EntryMalformed {
                            anchor: anchor.clone(),
                            entry,
                            why,
                        });
                    }
                    continue;
                }
                for entry in b.entries {
                    if input.never_ratifiable.matches(&entry) {
                        out.findings.push(Finding::NamesNeverRatifiable {
                            anchor: anchor.clone(),
                            entry,
                        });
                        continue;
                    }
                    if !input.protected.contains(&entry) || ratified.contains_key(&entry) {
                        continue;
                    }
                    // The window, for this path.
                    let too_old = match cfg.ratification_valid_from {
                        RatificationWindow::Any => None,
                        RatificationWindow::PullCreated => pull_created
                            .filter(|p| c.created_at <= *p)
                            .map(|_| "it predates the pull request".to_string()),
                        RatificationWindow::PathLastChanged => {
                            match (input.last_change)(&entry)? {
                                Some(t) if t > input.now + 300 => {
                                    return Err(tag(
                                        Reason::Repository,
                                        anyhow!(
                                            "the base branch's last change to `{entry}` is dated in the future; its time cannot bound a ratification"
                                        ),
                                    ))
                                }
                                Some(t) if c.created_at <= t => Some(
                                    "it predates the base branch's last change to this path"
                                        .to_string(),
                                ),
                                _ => None,
                            }
                        }
                    }
                    .or_else(|| {
                        cfg.ratification_max_age_days
                            .filter(|d| c.created_at < input.now - (*d as i64) * 86_400)
                            .map(|d| format!("it is older than {d} day(s)"))
                    });
                    match too_old {
                        None => {
                            lapsed.remove(&entry);
                            ratified.insert(entry, format!("{anchor} by {}", c.author));
                        }
                        Some(why) => {
                            lapsed.insert(entry, format!("{anchor}: {why}"));
                        }
                    }
                }
            }
        }
    }

    for path in input.protected {
        match ratified.get(path) {
            Some(by) => out.notes.push(format!("`{path}` ratified in {by}")),
            None => {
                if let Some(l) = lapsed.get(path) {
                    let (anchor, why) = l.split_once(": ").unwrap_or((l, ""));
                    out.findings.push(Finding::OutsideWindow {
                        anchor: anchor.to_string(),
                        path: path.clone(),
                        why: why.to_string(),
                    });
                }
                out.findings.push(Finding::Unratified {
                    path: path.clone(),
                    why: if resolved.is_empty() {
                        "the pull request closes no issue".into()
                    } else {
                        "no usable ratification on the issues it closes names it".into()
                    },
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CannedApi;

    #[test]
    fn rfc3339_times_parse_to_unix_seconds() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-09-27T10:00:00Z"), Some(1_790_503_200));
        assert_eq!(parse_time("2026-09-27T12:00:00+02:00"), Some(1_790_503_200));
        assert_eq!(
            parse_time("2026-09-27T10:00:00.123456789Z"),
            Some(1_790_503_200)
        );
        assert_eq!(parse_time("2000-02-29T00:00:00Z"), Some(951_782_400));
        for bad in [
            "",
            "2026-13-01T00:00:00Z",
            "yesterday",
            "2026-09-27",
            "2026-09-27T25:00:00Z",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn entries_must_be_exact_paths() {
        assert_eq!(refuse_entry("scripts/check_x.py"), None);
        assert_eq!(refuse_entry("docs/OPS.md"), None);
        assert_eq!(refuse_entry("a file with spaces.md"), None);
        for bad in [
            "scripts/*.py",
            "scripts/check_[xy].py",
            "docs/**",
            "{a,b}.md",
            "../scripts/check_x.py",
            "scripts/../x.py",
            "/scripts/check_x.py",
            "docs/",
            "./docs/OPS.md",
            "docs//OPS.md",
            "docs\\OPS.md",
            "",
        ] {
            assert!(refuse_entry(bad).is_some(), "accepted `{bad}`");
        }
    }

    #[test]
    fn blocks_are_read_outside_code_quotes_and_comments_only() {
        let body =
            "Reviewed.\n\nOwner-ratified-paths:\n- scripts/check_x.py\n- `docs/OPS.md`\n\nThanks\n\
                    ```\nOwner-ratified-paths:\n- fenced.py\n```\n\
                    > Owner-ratified-paths:\n> - quoted.py\n\
                    <!--\nOwner-ratified-paths:\n- hidden.py\n-->\n\
                    Owner-ratified-paths:\n- scripts/*.py\n- ok.py\n";
        let blocks = parse_blocks(body, "Owner-ratified-paths:");
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(blocks[0].entries, vec!["scripts/check_x.py", "docs/OPS.md"]);
        assert!(blocks[0].refused.is_empty());
        assert_eq!(blocks[1].entries, vec!["ok.py"]);
        assert_eq!(blocks[1].refused[0].0, "scripts/*.py");
        // A marker that is part of a sentence is not a block.
        assert!(parse_blocks(
            "see Owner-ratified-paths: above\n- x.py",
            "Owner-ratified-paths:"
        )
        .is_empty());
    }

    const MARKER: &str = "Owner-ratified-paths:";

    fn gitea() -> Forge {
        Forge {
            kind: ForgeKind::Gitea,
            url: "https://git.example.com".into(),
            repo: "o/r".into(),
        }
    }

    fn cfg() -> RatifiedPathsGate {
        RatifiedPathsGate {
            enabled: true,
            protected_paths: vec!["scripts/check_*.py".into(), "AGENTS.md".into()],
            ratifiers: vec!["owner".into()],
            agent_logins: vec!["agent".into()],
            ratification_valid_from: RatificationWindow::Any,
            ..Default::default()
        }
    }

    fn comment(id: u64, login: &str, body: &str, created: &str, updated: &str) -> Value {
        json!({"id": id, "user": {"login": login}, "body": body,
               "created_at": created, "updated_at": updated, "original_author": ""})
    }

    const T0: &str = "2026-09-27T10:00:00Z";

    /// A Gitea that has issue 12 (open) with `comments`, answers 404 for issue 1162 and
    /// 500 for 1162's comments, like Gitea 1.24.
    fn forge_with(comments: Vec<Value>) -> CannedApi {
        let mut api = CannedApi::default();
        api.responses
            .insert("gitea:repos/o/r".into(), json!({"full_name": "o/r"}));
        api.responses
            .insert("gitea:repos/o/r/issues/1162".into(), Value::Null);
        api.responses.insert(
            "gitea:repos/o/r/issues/1162/comments?limit=50&page=1".into(),
            json!({"__status": 500}),
        );
        api.responses.insert(
            "gitea:repos/o/r/issues/12".into(),
            json!({"number": 12, "state": "open"}),
        );
        let n = comments.len().to_string();
        api.responses.insert(
            "gitea:repos/o/r/issues/12/comments?limit=50&page=1".into(),
            json!({"__status": 200, "__headers": {"X-Total-Count": n}, "__body": comments}),
        );
        api
    }

    fn run(
        api: &CannedApi,
        cfg: &RatifiedPathsGate,
        body: &str,
        protected: &[&str],
    ) -> Result<Judgement> {
        let protected: Vec<String> = protected.iter().map(|s| s.to_string()).collect();
        let never = crate::guards::PathFilter::new(&cfg.never_ratifiable).unwrap();
        let last_change = |_: &str| Ok(None);
        judge(
            api,
            &gitea(),
            cfg,
            &Input {
                pull_number: 7,
                pull_body: body,
                protected: &protected,
                never_ratifiable: &never,
                last_change: &last_change,
                now: 1_790_600_000,
            },
        )
    }

    fn unratified(j: &Judgement) -> Vec<String> {
        j.findings
            .iter()
            .filter_map(|f| match f {
                Finding::Unratified { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_motivating_body_is_a_verdict_and_the_real_reference_ratifies() {
        let api = forge_with(vec![comment(
            1,
            "owner",
            &format!("{MARKER}\n- scripts/check_x.py\n"),
            T0,
            T0,
        )]);
        let j = run(
            &api,
            &cfg(),
            "Closes #12\n\nThis is the change which fixes #1162 upstream.",
            &["scripts/check_x.py"],
        )
        .unwrap();
        assert!(j.findings.is_empty(), "{j:?}");
        assert!(
            j.notes.iter().any(|n| n.contains("#1162 → ref-not-found")),
            "{j:?}"
        );
        assert!(!api.log().iter().any(|k| k.contains("1162/comments")));
    }

    #[test]
    fn a_protected_edit_with_no_ratification_fails() {
        let api = forge_with(vec![comment(1, "owner", "LGTM", T0, T0)]);
        let j = run(&api, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap();
        assert_eq!(unratified(&j), vec!["scripts/check_x.py"]);
        // No closing reference at all.
        let j = run(&api, &cfg(), "Refs #12", &["scripts/check_x.py"]).unwrap();
        assert_eq!(unratified(&j), vec!["scripts/check_x.py"]);
    }

    #[test]
    fn a_ratification_by_an_agent_login_does_not_count() {
        let block = format!("{MARKER}\n- scripts/check_x.py\n");
        for author in ["Agent", "stranger", ""] {
            let api = forge_with(vec![comment(1, author, &block, T0, T0)]);
            let j = run(&api, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap();
            assert_eq!(unratified(&j), vec!["scripts/check_x.py"], "{author}");
            assert!(
                j.findings
                    .iter()
                    .any(|f| matches!(f, Finding::AuthorNotAccepted { .. })),
                "{author}: {j:?}"
            );
        }
        // Listed as both ratifier and agent: the agent list wins.
        let mut both = cfg();
        both.ratifiers.push("agent".into());
        let api = forge_with(vec![comment(1, "agent", &block, T0, T0)]);
        let j = run(&api, &both, "Closes #12", &["scripts/check_x.py"]).unwrap();
        assert_eq!(unratified(&j), vec!["scripts/check_x.py"]);
    }

    #[test]
    fn a_glob_in_a_block_is_refused_and_voids_the_block() {
        for bad in ["scripts/*.py", "docs/", "/AGENTS.md", "../AGENTS.md"] {
            let body = format!("{MARKER}\n- {bad}\n- scripts/check_x.py\n");
            let api = forge_with(vec![comment(1, "owner", &body, T0, T0)]);
            let j = run(&api, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap();
            assert_eq!(unratified(&j), vec!["scripts/check_x.py"], "{bad}");
            assert!(
                j.findings
                    .iter()
                    .any(|f| matches!(f, Finding::EntryMalformed { entry, .. } if entry == bad)),
                "{bad}: {j:?}"
            );
        }
    }

    #[test]
    fn an_edited_or_imported_comment_does_not_ratify() {
        let block = format!("{MARKER}\n- scripts/check_x.py\n");
        let edited = forge_with(vec![comment(
            1,
            "owner",
            &block,
            T0,
            "2026-09-27T10:00:01Z",
        )]);
        let j = run(&edited, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap();
        assert_eq!(unratified(&j), vec!["scripts/check_x.py"]);
        assert!(j
            .findings
            .iter()
            .any(|f| matches!(f, Finding::CommentRefused { .. })));

        let mut imported = comment(1, "owner", &block, T0, T0);
        imported["original_author"] = json!("someone-elsewhere");
        let j = run(
            &forge_with(vec![imported]),
            &cfg(),
            "Closes #12",
            &["scripts/check_x.py"],
        )
        .unwrap();
        assert_eq!(unratified(&j), vec!["scripts/check_x.py"]);

        // `by-author` needs a forge that names the editor.
        let mut by_author = cfg();
        by_author.accept_edited = AcceptEdited::ByAuthor;
        let err = run(&edited, &by_author, "Closes #12", &["scripts/check_x.py"]).unwrap_err();
        assert_eq!(
            crate::could_not_check::classify(&err).0,
            Reason::Configuration
        );
    }

    #[test]
    fn a_closed_issue_carries_no_ratification_by_default() {
        let mut api = forge_with(vec![comment(
            1,
            "owner",
            &format!("{MARKER}\n- scripts/check_x.py\n"),
            T0,
            T0,
        )]);
        api.responses.insert(
            "gitea:repos/o/r/issues/12".into(),
            json!({"number": 12, "state": "closed"}),
        );
        let j = run(&api, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap();
        assert_eq!(unratified(&j), vec!["scripts/check_x.py"]);
        let mut open_not_needed = cfg();
        open_not_needed.require_open_issue = false;
        let j = run(
            &api,
            &open_not_needed,
            "Closes #12",
            &["scripts/check_x.py"],
        )
        .unwrap();
        assert!(unratified(&j).is_empty());
    }

    #[test]
    fn a_ratification_lapses_when_its_path_changed_on_the_base_after_it() {
        let api = forge_with(vec![comment(
            1,
            "owner",
            &format!("{MARKER}\n- scripts/check_x.py\n"),
            T0,
            T0,
        )]);
        let mut c = cfg();
        c.ratification_valid_from = RatificationWindow::PathLastChanged;
        let protected = vec!["scripts/check_x.py".to_string()];
        let never = crate::guards::PathFilter::new(&c.never_ratifiable).unwrap();
        let t0 = parse_time(T0).unwrap();
        let with = |changed: Option<i64>| {
            let last = move |_: &str| Ok(changed);
            judge(
                &api,
                &gitea(),
                &c,
                &Input {
                    pull_number: 7,
                    pull_body: "Closes #12",
                    protected: &protected,
                    never_ratifiable: &never,
                    last_change: &last,
                    now: t0 + 86_400,
                },
            )
        };
        assert!(unratified(&with(None).unwrap()).is_empty(), "never changed");
        assert!(
            unratified(&with(Some(t0 - 60)).unwrap()).is_empty(),
            "changed before"
        );
        let lapsed = with(Some(t0 + 60)).unwrap();
        assert_eq!(unratified(&lapsed), vec!["scripts/check_x.py"]);
        assert!(lapsed
            .findings
            .iter()
            .any(|f| matches!(f, Finding::OutsideWindow { .. })));
        let future = with(Some(t0 + 10 * 86_400)).unwrap_err();
        assert_eq!(
            crate::could_not_check::classify(&future).0,
            Reason::Repository
        );
    }

    #[test]
    fn forge_failures_are_exit_2_never_a_pass() {
        let mut api = forge_with(vec![]);
        api.responses.insert(
            "gitea:repos/o/r/issues/12/comments?limit=50&page=1".into(),
            json!({"__status": 502}),
        );
        let err = run(&api, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap_err();
        assert_eq!(crate::could_not_check::classify(&err).0, Reason::Forge);
        assert!(err.to_string().contains("forge-unavailable"), "{err}");
    }

    #[test]
    fn a_block_naming_a_workflow_path_is_reported() {
        let api = forge_with(vec![comment(
            1,
            "owner",
            &format!("{MARKER}\n- .gitea/workflows/ci.yml\n- scripts/check_x.py\n"),
            T0,
            T0,
        )]);
        let j = run(&api, &cfg(), "Closes #12", &["scripts/check_x.py"]).unwrap();
        assert!(
            unratified(&j).is_empty(),
            "the rest of the block stands: {j:?}"
        );
        assert!(j
            .findings
            .iter()
            .any(|f| matches!(f, Finding::NamesNeverRatifiable { .. })));
    }

    fn github() -> Forge {
        Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        }
    }

    fn graphql_key(query: &str, vars: &Value) -> String {
        format!(
            "github:graphql:{} {vars}",
            crate::forge::graphql_operation(query)
        )
    }

    #[test]
    fn github_reads_closing_issues_and_editors_from_graphql() {
        let mut api = CannedApi::default();
        api.responses.insert(
            graphql_key(CLOSING_ISSUES_QUERY, &closing_issues_vars("o/r", 7)),
            json!({"data": {"repository": {"pullRequest": {"closingIssuesReferences": {
                "totalCount": 2,
                "nodes": [
                    {"number": 12, "repository": {"nameWithOwner": "o/r"}},
                    {"number": 9, "repository": {"nameWithOwner": "up/stream"}}]}}}}}),
        );
        api.responses
            .insert("github:repos/o/r".into(), json!({"full_name": "o/r"}));
        api.responses.insert(
            "github:repos/o/r/issues/12".into(),
            json!({"number": 12, "state": "open"}),
        );
        let node = |login: &str, typename: &str, edited_by: Option<&str>| {
            json!({"databaseId": 1, "body": format!("{MARKER}\n- scripts/check_x.py\n"),
                   "createdAt": T0, "createdViaEmail": false,
                   "lastEditedAt": edited_by.map(|_| "2026-09-27T11:00:00Z"),
                   "editor": edited_by.map(|e| json!({"login": e})),
                   "author": {"login": login, "__typename": typename}})
        };
        let comments = |nodes: Vec<Value>| {
            json!({"data": {"repository": {"issue": {"comments": {
                "totalCount": nodes.len(), "pageInfo": {"hasNextPage": false, "endCursor": null},
                "nodes": nodes}}}}})
        };
        let key = graphql_key(ISSUE_COMMENTS_QUERY, &issue_comments_vars("o/r", 12, None));
        let protected = vec!["scripts/check_x.py".to_string()];
        let never = crate::guards::PathFilter::new(&cfg().never_ratifiable).unwrap();
        let last = |_: &str| Ok(None);
        let check = |api: &CannedApi, c: &RatifiedPathsGate| {
            judge(
                api,
                &github(),
                c,
                &Input {
                    pull_number: 7,
                    pull_body: "",
                    protected: &protected,
                    never_ratifiable: &never,
                    last_change: &last,
                    now: 1_790_600_000,
                },
            )
            .unwrap()
        };

        let mut ok = api.clone();
        ok.responses
            .insert(key.clone(), comments(vec![node("owner", "User", None)]));
        let j = check(&ok, &cfg());
        assert!(unratified(&j).is_empty(), "{j:?}");
        assert!(j
            .notes
            .iter()
            .any(|n| n.contains("up/stream#9 → ref-cross-repo")));

        // An app posting as the owner's login is still a bot.
        let mut bot = api.clone();
        bot.responses
            .insert(key.clone(), comments(vec![node("owner", "Bot", None)]));
        assert_eq!(unratified(&check(&bot, &cfg())), protected);

        // Edited by an agent: refused under every setting; by the owner: only `by-author`.
        let mut by_agent = api.clone();
        by_agent.responses.insert(
            key.clone(),
            comments(vec![node("owner", "User", Some("agent"))]),
        );
        let mut by_author = cfg();
        by_author.accept_edited = AcceptEdited::ByAuthor;
        assert_eq!(unratified(&check(&by_agent, &by_author)), protected);
        let mut by_owner = api.clone();
        by_owner.responses.insert(
            key.clone(),
            comments(vec![node("owner", "User", Some("owner"))]),
        );
        assert_eq!(unratified(&check(&by_owner, &cfg())), protected);
        assert!(unratified(&check(&by_owner, &by_author)).is_empty());
    }

    #[test]
    fn gitlab_reads_closes_issues_and_skips_system_notes() {
        let forge = Forge {
            kind: ForgeKind::GitLab,
            url: "https://gitlab.com".into(),
            repo: "g/sub/p".into(),
        };
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitlab:projects/g%2Fsub%2Fp/merge_requests/7/closes_issues?per_page=100&page=1".into(),
            json!([{"iid": 3, "web_url": "https://gitlab.com/g/sub/p/-/issues/3"}]),
        );
        api.responses
            .insert("gitlab:projects/g%2Fsub%2Fp".into(), json!({"id": 1}));
        api.responses.insert(
            "gitlab:projects/g%2Fsub%2Fp/issues/3".into(),
            json!({"iid": 3, "state": "opened"}),
        );
        let note = |system: bool| {
            json!({"id": 5, "author": {"username": "owner"}, "system": system,
                   "body": format!("{MARKER}\n- AGENTS.md\n"),
                   "created_at": T0, "updated_at": T0})
        };
        let protected = vec!["AGENTS.md".to_string()];
        let never = crate::guards::PathFilter::new(&cfg().never_ratifiable).unwrap();
        let last = |_: &str| Ok(None);
        let check = |api: &CannedApi| {
            judge(
                api,
                &forge,
                &cfg(),
                &Input {
                    pull_number: 7,
                    pull_body: "",
                    protected: &protected,
                    never_ratifiable: &never,
                    last_change: &last,
                    now: 1_790_600_000,
                },
            )
            .unwrap()
        };
        let notes_key = "gitlab:projects/g%2Fsub%2Fp/issues/3/notes?per_page=100&page=1";
        let mut ok = api.clone();
        ok.responses.insert(notes_key.into(), json!([note(false)]));
        assert!(unratified(&check(&ok)).is_empty());
        let mut system = api.clone();
        system
            .responses
            .insert(notes_key.into(), json!([note(true)]));
        assert_eq!(unratified(&check(&system)), protected);
    }
}
