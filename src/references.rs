//! Issue references in a pull request's text, and what each one resolves to on the forge.
//!
//! [`parse`] reads references the way the forges do: `#12`, `owner/repo#12` (GitLab:
//! `group/sub/project#12`), and issue or pull-request URLs, each marked closing when a
//! closing keyword (`fixes`, `closes`, ...) comes right before it. Code spans, fenced code
//! blocks and HTML comments are skipped, as the forges skip them.
//!
//! [`resolve`] asks the forge what a reference is. The issue is read first
//! (`issues/{n}`), which answers 404 for a number that does not exist; comments are only
//! ever read for an issue that exists. (Gitea answers **500**, not 404, for the comments
//! of a missing issue, so reading comments first would turn a stray number into an
//! outage.) A 404 is [`Verdict::NotFound`], a verdict and not an error; every other
//! failure is a [`ForgeError`] the caller turns into exit 2.

use crate::forge::{gitlab_project_id, Forge, ForgeApi, ForgeError, ForgeErrorKind, ForgeKind};
use regex::Regex;
use std::sync::LazyLock;

/// Closing keywords of GitHub, Gitea and Forgejo (all forms of close, fix, resolve).
pub const CLOSING_KEYWORDS: &[&str] = &[
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// GitLab's default closing keywords: those of [`CLOSING_KEYWORDS`] plus the `-ing` forms
/// and `implement`.
pub const GITLAB_CLOSING_KEYWORDS: &[&str] = &[
    "close",
    "closes",
    "closed",
    "closing",
    "fix",
    "fixes",
    "fixed",
    "fixing",
    "resolve",
    "resolves",
    "resolved",
    "resolving",
    "implement",
    "implements",
    "implemented",
    "implementing",
];

/// The forge's default closing keywords.
pub fn default_closing_keywords(kind: ForgeKind) -> &'static [&'static str] {
    match kind {
        ForgeKind::GitLab => GITLAB_CLOSING_KEYWORDS,
        _ => CLOSING_KEYWORDS,
    }
}

/// Most references [`parse`] returns; a text naming more is refused by the callers.
pub const MAX_REFERENCES: usize = 10;

/// One reference as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The reference as written (`#12`, `o/r#12`, a URL).
    pub text: String,
    /// The repository it names (`owner/repo`, a GitLab project path); `None` for a bare
    /// number, which means the repository under review.
    pub repo: Option<String>,
    /// The host of a URL reference, lower-cased.
    pub host: Option<String>,
    pub number: u64,
    /// Written as a pull or merge request (`/pull/12`, `/-/merge_requests/12`, GitLab `!12`).
    pub pull: bool,
    /// A closing keyword comes right before it.
    pub closing: bool,
}

static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)",
        // A URL: host, repository path, then the issue or pull-request segment.
        r"https?://(?P<host>[a-z0-9.\-]+(?::\d+)?)/(?P<upath>[a-z0-9_.\-]+(?:/[a-z0-9_.\-]+)+?)",
        r"/(?:-/)?(?P<ukind>issues|pull|pulls|merge_requests)/(?P<un>\d+)\b",
        // A qualified reference: owner/repo#12, group/sub/project#12, and GitLab's !12.
        r"|(?P<qrepo>\b[a-z0-9_.\-]+(?:/[a-z0-9_.\-]+)+)(?P<qsig>[#!])(?P<qn>\d+)\b",
        // A bare reference: #12, or GitLab's !12.
        r"|(?:^|[\s(\[,;:'\x22])(?P<lsig>[#!])(?P<ln>\d+)\b",
    ))
    .expect("reference pattern compiles")
});

static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?ms)^\s*(```|~~~).*?(^\s*(```|~~~)\s*$|\z)").unwrap());
static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`\n]*`").unwrap());
static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());

/// `text` with code blocks, code spans and HTML comments replaced by spaces (newlines
/// kept), so offsets and lines do not move.
fn blank_unread(text: &str) -> String {
    let mut out = text.to_string();
    for re in [&*FENCE, &*CODE_SPAN, &*HTML_COMMENT] {
        let spans: Vec<(usize, usize)> = re.find_iter(&out).map(|m| (m.start(), m.end())).collect();
        let mut bytes = out.into_bytes();
        for (s, e) in spans {
            for b in &mut bytes[s..e] {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
        out = String::from_utf8(bytes).expect("only ASCII bytes were replaced");
    }
    out
}

/// Whether `before` ends with one of `keywords` (and an optional colon) and whitespace.
fn ends_with_keyword(before: &str, keywords: &[String]) -> bool {
    let trimmed = before.trim_end();
    if trimmed.len() == before.len() {
        return false; // no whitespace between the keyword and the reference
    }
    let trimmed = trimmed.strip_suffix(':').unwrap_or(trimmed);
    let word_start = trimmed
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let word = &trimmed[word_start..];
    keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Whether the text between two references continues a GitLab closing list
/// (`fixes #1, #2 and #3`).
fn continues_list(between: &str) -> bool {
    let t = between.trim();
    t.is_empty() || t == "," || t.eq_ignore_ascii_case("and") || {
        let t = t.trim_start_matches(',').trim();
        t.eq_ignore_ascii_case("and")
    }
}

/// Every reference in `text`, in order, as `kind`'s forge reads it. `keywords` empty means
/// the forge's defaults.
pub fn parse(text: &str, kind: ForgeKind, keywords: &[String]) -> Vec<Reference> {
    let keywords: Vec<String> = if keywords.is_empty() {
        default_closing_keywords(kind)
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        keywords.to_vec()
    };
    let text = blank_unread(text);
    let mut out: Vec<Reference> = Vec::new();
    let mut last_end = 0usize;
    for caps in REFERENCE.captures_iter(&text) {
        let (start, end, reference) = if let Some(n) = caps.name("un") {
            let whole = caps.get(0).unwrap();
            let kind_seg = caps["ukind"].to_ascii_lowercase();
            (
                whole.start(),
                whole.end(),
                Reference {
                    text: whole.as_str().to_string(),
                    repo: Some(caps["upath"].to_string()),
                    host: Some(caps["host"].to_ascii_lowercase()),
                    number: n.as_str().parse().unwrap_or(0),
                    pull: kind_seg != "issues",
                    closing: false,
                },
            )
        } else if let Some(n) = caps.name("qn") {
            let repo = caps.name("qrepo").unwrap();
            (
                repo.start(),
                n.end(),
                Reference {
                    text: text[repo.start()..n.end()].to_string(),
                    repo: Some(repo.as_str().to_string()),
                    host: None,
                    number: n.as_str().parse().unwrap_or(0),
                    pull: &caps["qsig"] == "!",
                    closing: false,
                },
            )
        } else {
            let sig = caps.name("lsig").unwrap();
            let n = caps.name("ln").unwrap();
            (
                sig.start(),
                n.end(),
                Reference {
                    text: text[sig.start()..n.end()].to_string(),
                    repo: None,
                    host: None,
                    number: n.as_str().parse().unwrap_or(0),
                    pull: sig.as_str() == "!",
                    closing: false,
                },
            )
        };
        // `!12` is a merge request on GitLab only; elsewhere it is not a reference.
        if reference.pull && reference.host.is_none() && kind != ForgeKind::GitLab {
            continue;
        }
        let mut reference = reference;
        reference.closing = ends_with_keyword(&text[..start], &keywords)
            || (kind == ForgeKind::GitLab
                && out.last().is_some_and(|prev| prev.closing)
                && continues_list(&text[last_end..start]));
        last_end = end;
        if reference.number > 0 {
            out.push(reference);
        }
    }
    out
}

/// What a reference is on the forge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// An open issue.
    Issue,
    /// An issue that is closed.
    Closed,
    /// No such issue (HTTP 404).
    NotFound,
    /// A pull or merge request, not an issue.
    IsPull,
    /// In another repository, and not one the configuration lets it resolve in.
    CrossRepo,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Issue => "ref-issue",
            Verdict::Closed => "ref-closed",
            Verdict::NotFound => "ref-not-found",
            Verdict::IsPull => "ref-is-pull",
            Verdict::CrossRepo => "ref-cross-repo",
        }
    }
}

/// A reference and what it resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub reference: Reference,
    /// The repository it was looked up in; empty for [`Verdict::CrossRepo`].
    pub repo: String,
    pub verdict: Verdict,
}

impl Resolved {
    /// `#12 → ref-issue`, for notes.
    pub fn describe(&self) -> String {
        format!("{} → {}", self.reference.text, self.verdict.label())
    }
}

/// The repository `reference` is looked up in: the one under review, or one of `others`.
/// `None` means another repository, which is not looked up.
pub fn locate(reference: &Reference, forge: &Forge, others: &[String]) -> Option<String> {
    if let Some(host) = &reference.host {
        let forge_host = forge.host().to_ascii_lowercase();
        if *host != forge_host {
            return None;
        }
    }
    match &reference.repo {
        None => Some(forge.repo.clone()),
        Some(r) if r.eq_ignore_ascii_case(&forge.repo) => Some(forge.repo.clone()),
        Some(r) => others
            .iter()
            .find(|o| o.eq_ignore_ascii_case(r))
            .map(|o| o.to_string()),
    }
}

/// Check once that the token can see the repository under review. A repository the
/// token cannot see answers 404 on GitHub, which would otherwise make every reference in
/// it look missing; it is [`ForgeErrorKind::Denied`] instead.
pub fn probe_repository(api: &dyn ForgeApi, forge: &Forge) -> Result<(), ForgeError> {
    let path = match forge.kind {
        ForgeKind::GitLab => format!("projects/{}", gitlab_project_id(&forge.repo)),
        _ => format!("repos/{}", forge.repo),
    };
    match api.fetch(forge, &path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind == ForgeErrorKind::NotFound => Err(ForgeError {
            kind: ForgeErrorKind::Denied,
            message: format!(
                "repository {} is not visible with this token (HTTP 404); references in it cannot be checked",
                forge.repo
            ),
            attempts: e.attempts,
        }),
        Err(e) => Err(e),
    }
}

/// What `number` is in `repo`. `as_pull` is a reference written as a pull or merge request.
pub fn resolve_number(
    api: &dyn ForgeApi,
    forge: &Forge,
    repo: &str,
    number: u64,
    as_pull: bool,
) -> Result<Verdict, ForgeError> {
    if as_pull && forge.kind == ForgeKind::GitLab {
        // GitLab numbers merge requests apart from issues: `!12` is never issue 12.
        return Ok(Verdict::IsPull);
    }
    let path = match forge.kind {
        ForgeKind::GitLab => format!("projects/{}/issues/{number}", gitlab_project_id(repo)),
        _ => format!("repos/{repo}/issues/{number}"),
    };
    let issue = match api.fetch(forge, &path) {
        Ok(v) => v,
        Err(e) if e.kind == ForgeErrorKind::NotFound => return Ok(Verdict::NotFound),
        Err(e) => return Err(e),
    };
    // GitHub, Gitea and Forgejo number pull requests and issues together and answer a
    // pull request here too, with a `pull_request` member.
    if issue.get("pull_request").is_some_and(|p| !p.is_null()) {
        return Ok(Verdict::IsPull);
    }
    match issue.get("state").and_then(|s| s.as_str()) {
        Some(s) if s.eq_ignore_ascii_case("open") || s.eq_ignore_ascii_case("opened") => {
            Ok(Verdict::Issue)
        }
        Some(s) if s.eq_ignore_ascii_case("closed") => Ok(Verdict::Closed),
        other => Err(ForgeError::new(
            ForgeErrorKind::Malformed,
            format!("issue #{number} of {repo} has no readable state ({other:?})"),
        )),
    }
}

/// Resolve every reference: cross-repository ones without a request, the rest with one
/// read each. Stops at the first forge failure.
pub fn resolve_all(
    api: &dyn ForgeApi,
    forge: &Forge,
    references: &[Reference],
    others: &[String],
) -> Result<Vec<Resolved>, ForgeError> {
    if references.len() > MAX_REFERENCES {
        return Err(ForgeError::new(
            ForgeErrorKind::Malformed,
            format!(
                "{} issue references; at most {MAX_REFERENCES} are looked up",
                references.len()
            ),
        ));
    }
    let mut out: Vec<Resolved> = Vec::new();
    let mut probed = false;
    for r in references {
        let Some(repo) = locate(r, forge, others) else {
            out.push(Resolved {
                reference: r.clone(),
                repo: String::new(),
                verdict: Verdict::CrossRepo,
            });
            continue;
        };
        // The same issue written twice is looked up once.
        if let Some(prev) = out.iter().find(|p| {
            p.repo.eq_ignore_ascii_case(&repo)
                && p.reference.number == r.number
                && p.reference.pull == r.pull
        }) {
            let verdict = prev.verdict;
            out.push(Resolved {
                reference: r.clone(),
                repo,
                verdict,
            });
            continue;
        }
        if !probed {
            probe_repository(api, forge)?;
            probed = true;
        }
        let verdict = resolve_number(api, forge, &repo, r.number, r.pull)?;
        out.push(Resolved {
            reference: r.clone(),
            repo,
            verdict,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::CannedApi;
    use serde_json::json;

    fn forge(kind: ForgeKind) -> Forge {
        Forge {
            kind,
            url: "https://git.example.com".into(),
            repo: "o/r".into(),
        }
    }

    fn texts(refs: &[Reference]) -> Vec<(String, bool)> {
        refs.iter().map(|r| (r.text.clone(), r.closing)).collect()
    }

    #[test]
    fn closing_references_are_read_like_the_forge_reads_them() {
        let refs = parse(
            "Closes #12. Refs #13, which fixes #1162 upstream.\nFixes: o/r#14 and Resolves https://git.example.com/o/r/issues/15",
            ForgeKind::Gitea,
            &[],
        );
        assert_eq!(
            texts(&refs),
            vec![
                ("#12".into(), true),
                ("#13".into(), false),
                ("#1162".into(), true),
                ("o/r#14".into(), true),
                ("https://git.example.com/o/r/issues/15".into(), true),
            ]
        );
        // Not references: a heading anchor, an HTML entity, a URL fragment, code.
        let none = parse(
            "see a#1 and &#123; and https://x.com/o/r/issues/3#issuecomment-9 `fixes #4`\n```\nfixes #5\n```\n<!-- fixes #6 -->",
            ForgeKind::GitHub,
            &[],
        );
        assert_eq!(
            texts(&none),
            vec![("https://x.com/o/r/issues/3".into(), false)]
        );
        // A keyword glued to the number is not a closing reference; "prefixes" is not "fixes".
        assert!(parse("prefixes #7", ForgeKind::GitHub, &[])
            .iter()
            .all(|r| !r.closing));
    }

    #[test]
    fn gitlab_reads_its_own_keywords_lists_and_nested_paths() {
        let refs = parse(
            "Implements #1, #2 and group/sub/proj#3. Related to #4. Merges !5",
            ForgeKind::GitLab,
            &[],
        );
        assert_eq!(
            texts(&refs),
            vec![
                ("#1".into(), true),
                ("#2".into(), true),
                ("group/sub/proj#3".into(), true),
                ("#4".into(), false),
                ("!5".into(), false),
            ]
        );
        assert!(refs[4].pull);
        // GitHub closes only the reference right after the keyword, and has no `!N`.
        let gh = parse("Fixes #1, #2 !3", ForgeKind::GitHub, &[]);
        assert_eq!(texts(&gh), vec![("#1".into(), true), ("#2".into(), false)]);
        // Configured keywords replace the defaults.
        let custom = parse("Closes #1 Done #2", ForgeKind::GitHub, &["done".into()]);
        assert_eq!(
            texts(&custom),
            vec![("#1".into(), false), ("#2".into(), true)]
        );
    }

    #[test]
    fn references_are_located_in_this_repository_or_a_listed_one() {
        let f = forge(ForgeKind::GitHub);
        let r = |s: &str| parse(s, ForgeKind::GitHub, &[]).remove(0);
        assert_eq!(locate(&r("#1"), &f, &[]), Some("o/r".into()));
        assert_eq!(locate(&r("O/R#1"), &f, &[]), Some("o/r".into()));
        assert_eq!(locate(&r("up/stream#1"), &f, &[]), None);
        assert_eq!(
            locate(&r("up/stream#1"), &f, &["Up/Stream".into()]),
            Some("Up/Stream".into())
        );
        assert_eq!(
            locate(&r("https://other.host/o/r/issues/1"), &f, &[]),
            None,
            "same path on another host is another repository"
        );
    }

    #[test]
    fn a_missing_issue_is_a_verdict_and_its_comments_are_never_read() {
        let f = forge(ForgeKind::Gitea);
        let mut api = CannedApi::default();
        api.responses
            .insert("gitea:repos/o/r".into(), json!({"full_name": "o/r"}));
        api.responses
            .insert("gitea:repos/o/r/issues/1162".into(), json!(null));
        api.responses.insert(
            "gitea:repos/o/r/issues/1162/comments".into(),
            json!({"__status": 500}),
        );
        api.responses.insert(
            "gitea:repos/o/r/issues/12".into(),
            json!({"number": 12, "state": "open"}),
        );
        api.responses.insert(
            "gitea:repos/o/r/issues/7".into(),
            json!({"number": 7, "state": "open", "pull_request": {"merged": false}}),
        );
        api.responses.insert(
            "gitea:repos/o/r/issues/8".into(),
            json!({"number": 8, "state": "closed"}),
        );
        let refs = parse(
            "which fixes #1162. Closes #12, #7 and #8; see up/stream#9",
            ForgeKind::Gitea,
            &[],
        );
        let got: Vec<&str> = resolve_all(&api, &f, &refs, &[])
            .unwrap()
            .iter()
            .map(|r| r.verdict.label())
            .collect();
        assert_eq!(
            got,
            vec![
                "ref-not-found",
                "ref-issue",
                "ref-is-pull",
                "ref-closed",
                "ref-cross-repo"
            ]
        );
        assert!(!api.was_called("gitea:repos/o/r/issues/1162/comments"));
        assert!(!api.log().iter().any(|k| k.contains("up/stream")));
    }

    #[test]
    fn forge_failures_are_errors_never_verdicts() {
        let f = forge(ForgeKind::Gitea);
        let refs = parse("Closes #12", ForgeKind::Gitea, &[]);
        // The repository itself is invisible: every 404 would lie.
        let mut api = CannedApi::default();
        api.responses.insert("gitea:repos/o/r".into(), json!(null));
        let err = resolve_all(&api, &f, &refs, &[]).unwrap_err();
        assert_eq!(err.kind, ForgeErrorKind::Denied, "{err}");
        // A 5xx on the issue is an outage, not a missing issue.
        let mut api = CannedApi::default();
        api.responses
            .insert("gitea:repos/o/r".into(), json!({"full_name": "o/r"}));
        api.responses
            .insert("gitea:repos/o/r/issues/12".into(), json!({"__status": 500}));
        let err = resolve_all(&api, &f, &refs, &[]).unwrap_err();
        assert_eq!(err.kind, ForgeErrorKind::Unavailable, "{err}");
        // Too many references to look up.
        let many = parse(
            &(1..=11)
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(" "),
            ForgeKind::Gitea,
            &[],
        );
        assert!(resolve_all(&api, &f, &many, &[]).is_err());
    }

    #[test]
    fn gitlab_issues_and_merge_requests_resolve_apart() {
        let f = forge(ForgeKind::GitLab);
        let mut api = CannedApi::default();
        api.responses
            .insert("gitlab:projects/o%2Fr".into(), json!({"id": 1}));
        api.responses.insert(
            "gitlab:projects/o%2Fr/issues/3".into(),
            json!({"iid": 3, "state": "opened"}),
        );
        let refs = parse("Closes #3 and !3", ForgeKind::GitLab, &[]);
        let got: Vec<&str> = resolve_all(&api, &f, &refs, &[])
            .unwrap()
            .iter()
            .map(|r| r.verdict.label())
            .collect();
        assert_eq!(got, vec!["ref-issue", "ref-is-pull"]);
    }
}
