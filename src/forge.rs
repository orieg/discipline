//! Which forge hosts the repository, and a read-only way to ask it questions.
//!
//! Opt-in features need platform data that is not in the repository: the open-issue check
//! of `provenance-tags` (`require_open_pending_issues`), `issue-link` reference
//! verification, bench-regression citation freshness, `require_approval`, the
//! `merged-pr-body` directive source, and the branch-protection checks of `discipline
//! doctor`. They go through this module. No gate needs the network otherwise.
//!
//! Reads are retried a bounded number of times ([`MAX_ATTEMPTS`]) and a failure keeps its
//! class ([`ForgeErrorKind`]), so a caller can name it instead of guessing from a message.
//!
//! Requests are made in-process over HTTPS (rustls, no OpenSSL), so the static binary and
//! the container need no external tool. The client:
//!
//! - speaks HTTPS only; plain HTTP is accepted for a loopback address, or for any host with
//!   `DISCIPLINE_FORGE_ALLOW_HTTP=1` (the token then travels in clear);
//! - follows a redirect only to the same scheme and host, so a token never leaves it;
//! - refuses API paths with empty or dot segments;
//! - verifies certificates with the platform's trust store (system CA bundle, macOS
//!   keychain), and honours `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`;
//! - does nothing when `DISCIPLINE_NO_NETWORK=1`, except against a loopback address.
//!
//! | Forge   | API base                         | Token environment                                   |
//! |---------|----------------------------------|-----------------------------------------------------|
//! | GitHub  | `https://api.github.com`, GHES `<url>/api/v3`, GHE.com `https://api.<host>` | `DISCIPLINE_FORGE_TOKEN`, else `GH_TOKEN`, else `GITHUB_TOKEN` |
//! | GitLab  | `<url>/api/v4`                   | `DISCIPLINE_FORGE_TOKEN`, else `GITLAB_TOKEN`       |
//! | Gitea   | `<url>/api/v1`                   | `DISCIPLINE_FORGE_TOKEN`, else `GITEA_TOKEN`        |
//! | Forgejo | `<url>/api/v1`                   | `DISCIPLINE_FORGE_TOKEN`, else `FORGEJO_TOKEN`, else `GITEA_TOKEN` |
//!
//! `DISCIPLINE_FORGE_API_URL` replaces the computed API base (a proxy, or a local mock in
//! tests). Without a token, requests are anonymous: enough for public repositories.

use regex::Regex;
use std::sync::LazyLock;

/// Seconds a single API request may take.
pub const API_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeKind {
    GitHub,
    GitLab,
    Gitea,
    Forgejo,
}

impl ForgeKind {
    pub fn label(self) -> &'static str {
        match self {
            ForgeKind::GitHub => "github",
            ForgeKind::GitLab => "gitlab",
            ForgeKind::Gitea => "gitea",
            ForgeKind::Forgejo => "forgejo",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" => Some(ForgeKind::GitHub),
            "gitlab" => Some(ForgeKind::GitLab),
            "gitea" => Some(ForgeKind::Gitea),
            "forgejo" | "codeberg" => Some(ForgeKind::Forgejo),
            _ => None,
        }
    }
}

/// A repository on a forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forge {
    pub kind: ForgeKind,
    /// Web base URL without a trailing slash, e.g. `https://gitea.example.com`.
    pub url: String,
    /// Repository path: `owner/name`, or `group/subgroup/name` on GitLab.
    pub repo: String,
}

impl Forge {
    /// Host part of [`Forge::url`], lower-cased.
    pub fn host(&self) -> String {
        host_of(&self.url)
    }
}

/// The lower-cased host of a URL (`https://host:port/path` → `host`).
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// A remote URL split into web base and repository path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub url: String,
    pub repo: String,
}

/// Parse `https://host[:port]/path(.git)`, `git@host:path(.git)` and
/// `ssh://git@host[:port]/path(.git)`. SSH remotes map to `https://host`.
pub fn parse_remote(remote: &str) -> Option<Remote> {
    static HTTP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(https?)://(?:[^@/]+@)?([^/]+)/(.+?)(?:\.git)?/?$").unwrap()
    });
    static SSH_URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^ssh://(?:[^@/]+@)?([^/:]+)(?::\d+)?/(.+?)(?:\.git)?/?$").unwrap()
    });
    static SCP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:[^@/]+@)?([^/:]+):(.+?)(?:\.git)?/?$").unwrap());
    let remote = remote.trim();
    if let Some(c) = HTTP.captures(remote) {
        return Some(Remote {
            url: format!("{}://{}", &c[1], &c[2]),
            repo: c[3].to_string(),
        });
    }
    if let Some(c) = SSH_URL.captures(remote) {
        return Some(Remote {
            url: format!("https://{}", &c[1]),
            repo: c[2].to_string(),
        });
    }
    let c = SCP.captures(remote)?;
    Some(Remote {
        url: format!("https://{}", &c[1]),
        repo: c[2].to_string(),
    })
}

/// Guess a forge from its host name alone.
pub fn kind_from_host(host: &str) -> Option<ForgeKind> {
    let h = host.to_ascii_lowercase();
    if h == "github.com" || h.ends_with(".ghe.com") {
        Some(ForgeKind::GitHub)
    } else if h == "codeberg.org" || h.contains("forgejo") {
        Some(ForgeKind::Forgejo)
    } else if h.contains("gitlab") {
        Some(ForgeKind::GitLab)
    } else if h.contains("gitea") {
        Some(ForgeKind::Gitea)
    } else {
        None
    }
}

/// Resolve the forge from the environment and the `origin` remote.
///
/// Order: `DISCIPLINE_FORGE` (with `DISCIPLINE_FORGE_URL` and `DISCIPLINE_FORGE_REPO`
/// when the remote does not say), then the CI runner's own variables (GitLab CI, Forgejo
/// Actions, Gitea Actions, GitHub Actions), then the remote's host name.
pub fn detect(env: &dyn Fn(&str) -> Option<String>, origin: Option<&str>) -> Result<Forge, String> {
    let var = |k: &str| env(k).filter(|v| !v.trim().is_empty());
    let remote = origin.and_then(parse_remote);
    let explicit_url = var("DISCIPLINE_FORGE_URL").map(|u| u.trim_end_matches('/').to_string());
    let explicit_repo = var("DISCIPLINE_FORGE_REPO");

    let build = |kind: ForgeKind,
                 url: Option<String>,
                 repo: Option<String>|
     -> Result<Forge, String> {
        let url = explicit_url
            .clone()
            .or(url)
            .or_else(|| remote.as_ref().map(|r| r.url.clone()))
            .or_else(|| (kind == ForgeKind::GitHub).then(|| "https://github.com".to_string()))
            .ok_or_else(|| format!("no {} URL: set DISCIPLINE_FORGE_URL", kind.label()))?;
        let repo = explicit_repo
            .clone()
            .or(repo)
            .or_else(|| remote.as_ref().map(|r| r.repo.clone()))
            .ok_or_else(|| format!("no {} repository: set DISCIPLINE_FORGE_REPO", kind.label()))?;
        // A URL variable may carry `user:token@`; keep the host, never the credential.
        let url = clean_url(&url).map_err(|e| format!("{} URL: {e}", kind.label()))?;
        let repo = repo.trim_matches('/').to_string();
        if check_api_path(&repo).is_err() {
            return Err(format!(
                "{} repository path is not a plain path",
                kind.label()
            ));
        }
        Ok(Forge { kind, url, repo })
    };

    if let Some(k) = var("DISCIPLINE_FORGE") {
        let kind = ForgeKind::parse(&k).ok_or_else(|| {
            format!("DISCIPLINE_FORGE=`{k}` is not github, gitlab, gitea or forgejo")
        })?;
        return build(kind, None, None);
    }
    if var("GITLAB_CI").is_some() {
        return build(
            ForgeKind::GitLab,
            var("CI_SERVER_URL"),
            var("CI_PROJECT_PATH"),
        );
    }
    // Gitea and Forgejo runners also set the GITHUB_* compatibility variables, so they are
    // checked first.
    for (flag, kind) in [
        ("FORGEJO_ACTIONS", ForgeKind::Forgejo),
        ("GITEA_ACTIONS", ForgeKind::Gitea),
    ] {
        if var(flag).is_some() {
            return build(kind, var("GITHUB_SERVER_URL"), var("GITHUB_REPOSITORY"));
        }
    }
    if var("GITHUB_ACTIONS").is_some() {
        return build(
            ForgeKind::GitHub,
            var("GITHUB_SERVER_URL"),
            var("GITHUB_REPOSITORY"),
        );
    }
    if let Some(r) = &remote {
        if let Some(kind) = kind_from_host(&host_of(&r.url)) {
            return build(kind, None, None);
        }
        return Err(format!(
            "cannot tell which forge hosts `{}`: set DISCIPLINE_FORGE to github, gitlab, gitea or forgejo",
            host_of(&r.url)
        ));
    }
    // No remote: a bare GITHUB_REPOSITORY (as set by hand or by a wrapper) means GitHub.
    if let Some(repo) = var("GITHUB_REPOSITORY").filter(|r| r.contains('/')) {
        return build(ForgeKind::GitHub, var("GITHUB_SERVER_URL"), Some(repo));
    }
    Err("no `origin` remote and no forge in the environment: set DISCIPLINE_FORGE".to_string())
}

/// [`detect`] against the process environment and a repository's `origin`.
pub fn detect_for(git: &crate::gitctx::GitCtx) -> Result<Forge, String> {
    let origin = git
        .remote_url("origin")
        .map(|o| resolve_ssh_alias(&o, &|alias| ssh_hostname_from_home(alias)));
    detect(&|k| std::env::var(k).ok(), origin.as_deref())
}

static SSH_REMOTE: LazyLock<Regex> = LazyLock::new(|| {
    // `ssh://[user@]host[:port]/path` or scp-like `[user@]host:path` (not `scheme://`).
    Regex::new(r"^(ssh://(?:[^@/]+@)?)([^/:]+)(.*)$|^((?:[^@/:]+@)?)([^/:]+)(:[^/].*)$").unwrap()
});

/// Rewrite the host of an SSH remote through `resolve` (an OpenSSH `Host` alias to its
/// `HostName`). HTTP(S) remotes and hosts that resolve to nothing are returned unchanged.
pub fn resolve_ssh_alias(remote: &str, resolve: &dyn Fn(&str) -> Option<String>) -> String {
    let remote = remote.trim();
    if remote.contains("://") && !remote.starts_with("ssh://") {
        return remote.to_string();
    }
    let Some(c) = SSH_REMOTE.captures(remote) else {
        return remote.to_string();
    };
    let (prefix, host, rest) = match (c.get(1), c.get(2), c.get(3)) {
        (Some(p), Some(h), Some(r)) => (p.as_str(), h.as_str(), r.as_str()),
        _ => (
            c.get(4).map_or("", |m| m.as_str()),
            c.get(5).map_or("", |m| m.as_str()),
            c.get(6).map_or("", |m| m.as_str()),
        ),
    };
    match resolve(host) {
        Some(real) if !real.is_empty() && real != host => format!("{prefix}{real}{rest}"),
        _ => remote.to_string(),
    }
}

/// `HostName` for `alias` from `~/.ssh/config`.
pub fn ssh_hostname_from_home(alias: &str) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let dir = std::path::Path::new(&home).join(".ssh");
    let text = read_ssh_config(&dir.join("config"), &dir, 0);
    ssh_hostname(&text, alias)
}

/// A config file with its `Include` directives expanded in place (relative paths are
/// under `~/.ssh`; `*` and `?` match file names), up to a fixed depth.
fn read_ssh_config(path: &std::path::Path, ssh_dir: &std::path::Path, depth: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    if depth > 4 {
        return text;
    }
    let mut out = String::new();
    for line in text.lines() {
        let (key, args) = ssh_config_line(line);
        if !key.eq_ignore_ascii_case("include") {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        for arg in args {
            let expanded = if let Some(rest) = arg.strip_prefix("~/") {
                ssh_dir.parent().map(|h| h.join(rest)).unwrap_or_default()
            } else if std::path::Path::new(&arg).is_absolute() {
                std::path::PathBuf::from(&arg)
            } else {
                ssh_dir.join(&arg)
            };
            let name = expanded
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            match (expanded.parent(), name) {
                (Some(parent), Some(name)) if name.contains(['*', '?']) => {
                    let Ok(glob) = globset::Glob::new(&name) else {
                        continue;
                    };
                    let matcher = glob.compile_matcher();
                    let mut files: Vec<_> = std::fs::read_dir(parent)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.file_name().is_some_and(|n| matcher.is_match(n)))
                        .collect();
                    files.sort();
                    for f in files {
                        out.push_str(&read_ssh_config(&f, ssh_dir, depth + 1));
                    }
                }
                _ => out.push_str(&read_ssh_config(&expanded, ssh_dir, depth + 1)),
            }
        }
    }
    out
}

/// Keyword and arguments of one OpenSSH config line (`Key value`, `Key=value`, quotes).
fn ssh_config_line(line: &str) -> (String, Vec<String>) {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return (String::new(), Vec::new());
    }
    let (key, rest) = match line.find(|c: char| c.is_whitespace() || c == '=') {
        Some(i) => (
            &line[..i],
            line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '='),
        ),
        None => (line, ""),
    };
    let args = rest
        .split_whitespace()
        .map(|a| a.trim_matches('"').to_string())
        .collect();
    (key.to_string(), args)
}

/// `HostName` for `alias` from OpenSSH client config text, as `ssh` resolves it: the
/// first `HostName` in a section that applies wins (the lines before any `Host` apply to
/// every host); `Host` patterns take `*`, `?` and `!` negation; `%h` is the alias.
/// `Match` sections are not evaluated and never apply.
pub fn ssh_hostname(config: &str, alias: &str) -> Option<String> {
    let pattern_matches = |pat: &str| {
        globset::Glob::new(pat)
            .map(|g| g.compile_matcher().is_match(alias))
            .unwrap_or(false)
    };
    let mut applies = true;
    for line in config.lines() {
        let (key, args) = ssh_config_line(line);
        if key.eq_ignore_ascii_case("host") {
            let negated = args
                .iter()
                .filter_map(|a| a.strip_prefix('!'))
                .any(&pattern_matches);
            applies = !negated
                && args
                    .iter()
                    .filter(|a| !a.starts_with('!'))
                    .any(|p| pattern_matches(p));
        } else if key.eq_ignore_ascii_case("match") {
            applies = false;
        } else if applies && key.eq_ignore_ascii_case("hostname") {
            if let Some(h) = args.first() {
                return Some(h.replace("%h", alias).replace("%%", "%"));
            }
        }
    }
    None
}

/// The class of a failed forge read. The label is the verdict a report carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeErrorKind {
    /// HTTP 404, or a GraphQL `NOT_FOUND`.
    NotFound,
    /// HTTP 401 / 403 that is not a rate limit: the token cannot read this.
    Denied,
    /// HTTP 429, or a 403 carrying rate-limit headers, still limited after the bounded wait.
    RateLimited,
    /// No answer (connection, timeout) or a 5xx, after the bounded retries.
    Unavailable,
    /// An answer this client cannot use: not JSON, a missing field, an unexpected status.
    Malformed,
    /// A list whose pages could not be read to the end.
    Partial,
}

impl ForgeErrorKind {
    pub fn label(self) -> &'static str {
        match self {
            ForgeErrorKind::NotFound => "forge-not-found",
            ForgeErrorKind::Denied => "forge-denied",
            ForgeErrorKind::RateLimited => "forge-rate-limited",
            ForgeErrorKind::Unavailable => "forge-unavailable",
            ForgeErrorKind::Malformed => "forge-malformed",
            ForgeErrorKind::Partial => "forge-partial-list",
        }
    }
}

/// A failed forge read: its class, what happened, and how many attempts were made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeError {
    pub kind: ForgeErrorKind,
    pub message: String,
    pub attempts: u32,
}

impl ForgeError {
    pub fn new(kind: ForgeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            attempts: 1,
        }
    }

    /// The message, with the attempt count when the read was retried.
    pub fn detail(&self) -> String {
        if self.attempts > 1 {
            format!("{} (after {} attempts)", self.message, self.attempts)
        } else {
            self.message.clone()
        }
    }
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.label(), self.detail())
    }
}

impl std::error::Error for ForgeError {}

/// The class of an HTTP answer, `None` for a success. A 403 is a rate limit only when
/// the forge says so in a header: a body that says "rate limit" is not evidence.
pub fn classify_status(status: u16, headers: &[(String, String)]) -> Option<ForgeErrorKind> {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    };
    match status {
        200..=299 => None,
        404 => Some(ForgeErrorKind::NotFound),
        429 => Some(ForgeErrorKind::RateLimited),
        403 if header("x-ratelimit-remaining") == Some("0") || header("retry-after").is_some() => {
            Some(ForgeErrorKind::RateLimited)
        }
        401 | 403 => Some(ForgeErrorKind::Denied),
        500 | 502 | 503 | 504 => Some(ForgeErrorKind::Unavailable),
        _ => Some(ForgeErrorKind::Malformed),
    }
}

/// Attempts made for one read, the first included.
pub const MAX_ATTEMPTS: u32 = 3;

/// Wait before the second and third attempt of a read the forge could not answer.
pub const RETRY_BACKOFF_MS: [u64; 2] = [500, 1500];

/// Longest wait a rate limit may ask for before the read is given up instead.
pub const MAX_RATE_LIMIT_WAIT_SECS: u64 = 60;

/// Most HTTP requests one process makes, retries included. A body naming hundreds of
/// issues cannot turn a check into a crawl.
pub const MAX_REQUESTS: usize = 500;

/// How long to wait before attempt `attempt + 1` of a read that failed with `kind`;
/// `None` when it is not retried. `now` is Unix seconds, for reset-time headers.
pub fn retry_delay(
    kind: ForgeErrorKind,
    headers: &[(String, String)],
    attempt: u32,
    now: u64,
) -> Option<std::time::Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let backoff = std::time::Duration::from_millis(
        RETRY_BACKOFF_MS[(attempt as usize - 1).min(RETRY_BACKOFF_MS.len() - 1)],
    );
    match kind {
        ForgeErrorKind::Unavailable => Some(backoff),
        ForgeErrorKind::RateLimited => {
            let header = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .and_then(|(_, v)| v.trim().parse::<u64>().ok())
            };
            // Retry-After is seconds; GitHub's x-ratelimit-reset and GitLab's
            // RateLimit-Reset are a Unix time.
            let wait = header("retry-after").or_else(|| {
                header("x-ratelimit-reset")
                    .or_else(|| header("ratelimit-reset"))
                    .map(|reset| reset.saturating_sub(now))
            });
            match wait {
                Some(w) if w > MAX_RATE_LIMIT_WAIT_SECS => None,
                Some(w) => Some(std::time::Duration::from_secs(w).max(backoff)),
                None => Some(backoff),
            }
        }
        _ => None,
    }
}

/// One page of a list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub items: Vec<serde_json::Value>,
    /// The size of the whole list, when the forge states it (`X-Total-Count` on Gitea and
    /// Forgejo, `x-total` on GitLab).
    pub total: Option<u64>,
    /// The forge says a further page exists (`Link: rel="next"`, GitLab `x-next-page`).
    pub has_next: bool,
}

impl Page {
    fn from_answer(body: serde_json::Value, headers: &[(String, String)]) -> Result<Page, String> {
        let items = match body {
            serde_json::Value::Array(items) => items,
            other => return Err(format!("expected a list, got {}", kind_of(&other))),
        };
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_string())
        };
        Ok(Page {
            items,
            total: header("x-total-count")
                .or_else(|| header("x-total"))
                .and_then(|v| v.parse().ok()),
            has_next: header("link").is_some_and(|l| l.contains("rel=\"next\""))
                || header("x-next-page").is_some_and(|p| !p.is_empty()),
        })
    }
}

fn kind_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Read-only access to a forge's REST API.
pub trait ForgeApi {
    /// GET `path` (relative to the forge's API base) parsed as JSON. `Ok(None)` for 404.
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String>;

    /// GET `path` as JSON, a failure classified ([`ForgeErrorKind::NotFound`] for 404).
    fn fetch(&self, forge: &Forge, path: &str) -> Result<serde_json::Value, ForgeError> {
        match self.get(forge, path) {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err(ForgeError::new(
                ForgeErrorKind::NotFound,
                format!("`{path}` does not exist or is not visible"),
            )),
            Err(e) => Err(ForgeError::new(ForgeErrorKind::Unavailable, e)),
        }
    }

    /// GET one page of a list, with the paging headers the forge sent.
    fn get_page(&self, forge: &Forge, path: &str) -> Result<Page, ForgeError> {
        match self.get(forge, path) {
            Ok(Some(v)) => {
                Page::from_answer(v, &[]).map_err(|e| ForgeError::new(ForgeErrorKind::Malformed, e))
            }
            Ok(None) => Err(ForgeError::new(
                ForgeErrorKind::NotFound,
                format!("`{path}` does not exist or is not visible"),
            )),
            Err(e) => Err(ForgeError::new(ForgeErrorKind::Unavailable, e)),
        }
    }

    /// A read-only GitHub GraphQL query: its `data`, or the class of its failure.
    fn graphql(
        &self,
        forge: &Forge,
        query: &str,
        variables: &serde_json::Value,
    ) -> Result<serde_json::Value, ForgeError> {
        let _ = (query, variables);
        Err(ForgeError::new(
            ForgeErrorKind::Malformed,
            format!("GraphQL is not available for {}", forge.kind.label()),
        ))
    }
}

/// The paging query for `page` (1-based) of a list endpoint. Gitea and Forgejo read
/// `limit` and ignore `per_page`; their largest page is 50 unless the instance says
/// otherwise.
pub fn page_query(kind: ForgeKind, page: usize) -> String {
    match kind {
        ForgeKind::Gitea | ForgeKind::Forgejo => format!("limit=50&page={page}"),
        ForgeKind::GitHub | ForgeKind::GitLab => format!("per_page=100&page={page}"),
    }
}

/// Most pages [`read_all`] reads before it refuses the list as partial.
pub const MAX_PAGES: usize = 20;

/// Every item of a list, or an error: never part of one.
///
/// `path` is the endpoint without a query; the paging query of [`page_query`] is added.
/// With a stated total (`X-Total-Count`, GitLab `x-total`), pages are read until the
/// items reach it: a page that brings nothing new before then, more items than the
/// total, or a total that changes between pages is [`ForgeErrorKind::Partial`]. A short
/// page is not an end, since a server may clamp the page size. An endpoint that ignores
/// paging (Gitea's and Forgejo's issue comments) answers the whole list on page 1 and is
/// complete there. Without a total, GitHub and GitLab end where no further page is stated
/// (`Link: rel="next"`, `x-next-page`); Gitea and Forgejo end at an empty page, and a page
/// that only repeats is refused, since it cannot tell a complete list from a server that
/// ignores `page`.
pub fn read_all(
    api: &dyn ForgeApi,
    forge: &Forge,
    path: &str,
) -> Result<Vec<serde_json::Value>, ForgeError> {
    let partial = |why: String| {
        ForgeError::new(
            ForgeErrorKind::Partial,
            format!("`{path}`: {why}; refusing to judge a partial list"),
        )
    };
    let identity = |v: &serde_json::Value| match v.get("id") {
        Some(id) if !id.is_null() => id.to_string(),
        _ => v.to_string(),
    };
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    let mut total = None;
    for page in 1..=MAX_PAGES {
        let p = api.get_page(
            forge,
            &format!("{path}{sep}{}", page_query(forge.kind, page)),
        )?;
        if page == 1 {
            total = p.total;
        } else if p.total != total {
            return Err(partial("the list changed while it was read".into()));
        }
        let empty = p.items.is_empty();
        let before = items.len();
        for item in p.items {
            if seen.insert(identity(&item)) {
                items.push(item);
            }
        }
        let added = items.len() - before;
        let n = items.len() as u64;
        match total {
            Some(t) if n > t => {
                return Err(partial(format!(
                    "the pages hold {n} but the forge counts {t}"
                )))
            }
            Some(t) if n == t => return Ok(items),
            Some(t) if added == 0 => {
                return Err(partial(format!("the pages ended after {n} of {t}")))
            }
            Some(_) => {}
            None if empty => return Ok(items),
            None if added == 0 => {
                return Err(partial(
                    "a page repeated the items before it and the forge states no total".into(),
                ))
            }
            None if matches!(forge.kind, ForgeKind::GitHub | ForgeKind::GitLab) && !p.has_next => {
                return Ok(items)
            }
            None => {}
        }
    }
    Err(partial(format!("more than {MAX_PAGES} pages")))
}

/// Answers from canned responses keyed by `"<forge label>:<path>"`.
///
/// A value is the JSON body of a 200, `null` for a 404, `{"__error": "..."}` for a failure
/// of `get`, `{"__status": N, "__headers": {...}, "__body": ...}` for any status with
/// headers, and `{"__sequence": [v1, v2, ...]}` for answers given in turn (the last one
/// repeats). A GraphQL query is keyed `"<forge>:graphql:<operation name> <variables>"`.
/// Every key asked for is logged, in order.
#[derive(Debug, Clone, Default)]
pub struct CannedApi {
    pub responses: std::collections::BTreeMap<String, serde_json::Value>,
    log: std::cell::RefCell<Vec<String>>,
    served: std::cell::RefCell<std::collections::BTreeMap<String, usize>>,
}

/// A canned answer: status, headers, body.
type CannedAnswer = (u16, Vec<(String, String)>, serde_json::Value);

impl CannedApi {
    /// Keys asked for so far, in order.
    pub fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Whether `key` (`"<forge>:<path>"`) was asked for.
    pub fn was_called(&self, key: &str) -> bool {
        self.log.borrow().iter().any(|k| k == key)
    }

    fn answer(&self, key: &str) -> Result<CannedAnswer, String> {
        self.log.borrow_mut().push(key.to_string());
        let mut v = self
            .responses
            .get(key)
            .cloned()
            .ok_or_else(|| format!("no recorded response for `{key}`"))?;
        if let Some(seq) = v.get("__sequence").and_then(|s| s.as_array()).cloned() {
            let mut served = self.served.borrow_mut();
            let n = served.entry(key.to_string()).or_default();
            v = seq
                .get(*n)
                .or_else(|| seq.last())
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            *n += 1;
        }
        if v.is_null() {
            return Ok((404, Vec::new(), serde_json::Value::Null));
        }
        if let Some(e) = v.get("__error") {
            return Err(e.as_str().unwrap_or("error").to_string());
        }
        if let Some(status) = v.get("__status").and_then(|s| s.as_u64()) {
            let headers = v
                .get("__headers")
                .and_then(|h| h.as_object())
                .map(|h| {
                    h.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let body = v.get("__body").cloned().unwrap_or(serde_json::Value::Null);
            return Ok((status as u16, headers, body));
        }
        Ok((200, Vec::new(), v))
    }
}

impl ForgeApi for CannedApi {
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String> {
        let key = format!("{}:{path}", forge.kind.label());
        let (status, _, body) = self.answer(&key)?;
        match status {
            200..=299 => Ok(Some(body)),
            404 => Ok(None),
            s => Err(format!("canned forge answered HTTP {s}")),
        }
    }

    fn fetch(&self, forge: &Forge, path: &str) -> Result<serde_json::Value, ForgeError> {
        let key = format!("{}:{path}", forge.kind.label());
        let (status, headers, body) = self
            .answer(&key)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Unavailable, e))?;
        match classify_status(status, &headers) {
            None => Ok(body),
            Some(kind) => Err(ForgeError::new(
                kind,
                format!("canned forge answered HTTP {status} for `{path}`"),
            )),
        }
    }

    fn get_page(&self, forge: &Forge, path: &str) -> Result<Page, ForgeError> {
        let key = format!("{}:{path}", forge.kind.label());
        let (status, headers, body) = self
            .answer(&key)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Unavailable, e))?;
        if let Some(kind) = classify_status(status, &headers) {
            return Err(ForgeError::new(
                kind,
                format!("canned forge answered HTTP {status} for `{path}`"),
            ));
        }
        Page::from_answer(body, &headers).map_err(|e| ForgeError::new(ForgeErrorKind::Malformed, e))
    }

    fn graphql(
        &self,
        forge: &Forge,
        query: &str,
        variables: &serde_json::Value,
    ) -> Result<serde_json::Value, ForgeError> {
        let key = format!(
            "{}:graphql:{} {variables}",
            forge.kind.label(),
            graphql_operation(query)
        );
        let (status, headers, body) = self
            .answer(&key)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Unavailable, e))?;
        if let Some(kind) = classify_status(status, &headers) {
            return Err(ForgeError::new(
                kind,
                format!("canned forge answered HTTP {status} for GraphQL"),
            ));
        }
        graphql_data(body)
    }
}

/// The operation name of a GraphQL document (`query Name(...)`), or `anonymous`.
pub fn graphql_operation(query: &str) -> &str {
    query
        .trim_start()
        .strip_prefix("query")
        .map(|rest| rest.trim_start())
        .and_then(|rest| {
            rest.split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or("anonymous")
}

/// The `data` of a GraphQL answer; an `errors` entry is classified like an HTTP status.
pub fn graphql_data(body: serde_json::Value) -> Result<serde_json::Value, ForgeError> {
    if let Some(first) = body
        .get("errors")
        .and_then(|e| e.as_array())
        .and_then(|e| e.first())
    {
        let message = first
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("GraphQL error")
            .chars()
            .take(200)
            .collect::<String>();
        let kind = match first.get("type").and_then(|t| t.as_str()) {
            Some("NOT_FOUND") => ForgeErrorKind::NotFound,
            Some("RATE_LIMITED") => ForgeErrorKind::RateLimited,
            Some("FORBIDDEN") | Some("INSUFFICIENT_SCOPES") => ForgeErrorKind::Denied,
            _ => ForgeErrorKind::Malformed,
        };
        return Err(ForgeError::new(kind, message));
    }
    match body.get("data") {
        Some(d) if !d.is_null() => Ok(d.clone()),
        _ => Err(ForgeError::new(
            ForgeErrorKind::Malformed,
            "GraphQL answer carries no `data`",
        )),
    }
}

/// An API that can answer nothing.
pub struct NoApi;

impl ForgeApi for NoApi {
    fn get(&self, _forge: &Forge, _path: &str) -> Result<Option<serde_json::Value>, String> {
        Err("no forge API was supplied to this evaluation".to_string())
    }
}

/// Maximum response body read from a forge.
pub const MAX_BODY_BYTES: u64 = 25 * 1024 * 1024;

/// Redirects followed within the same host.
const MAX_REDIRECTS: usize = 3;

/// Live requests over HTTPS.
pub struct HttpApi<'a> {
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

impl HttpApi<'_> {
    /// An API over the process environment.
    pub fn from_env() -> HttpApi<'static> {
        HttpApi {
            env: &|k: &str| std::env::var(k).ok(),
        }
    }

    fn var(&self, k: &str) -> Option<String> {
        (self.env)(k).filter(|v| !v.trim().is_empty())
    }

    fn flag(&self, k: &str) -> bool {
        self.var(k)
            .is_some_and(|v| v.trim() != "0" && !v.eq_ignore_ascii_case("false"))
    }

    fn token(&self, kind: ForgeKind) -> Option<String> {
        let names: &[&str] = match kind {
            ForgeKind::GitHub => &["DISCIPLINE_FORGE_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
            ForgeKind::GitLab => &["DISCIPLINE_FORGE_TOKEN", "GITLAB_TOKEN"],
            ForgeKind::Gitea => &["DISCIPLINE_FORGE_TOKEN", "GITEA_TOKEN"],
            ForgeKind::Forgejo => &["DISCIPLINE_FORGE_TOKEN", "FORGEJO_TOKEN", "GITEA_TOKEN"],
        };
        names
            .iter()
            .find_map(|n| self.var(n))
            .map(|t| t.trim().to_string())
    }

    /// The API base URL for `forge`, without a trailing slash.
    pub fn api_base(&self, forge: &Forge) -> Result<String, String> {
        if let Some(u) = self.var("DISCIPLINE_FORGE_API_URL") {
            return clean_url(&u).map_err(|e| format!("DISCIPLINE_FORGE_API_URL: {e}"));
        }
        let host = forge.host();
        Ok(match forge.kind {
            ForgeKind::GitHub if host == "github.com" => "https://api.github.com".to_string(),
            ForgeKind::GitHub if host.ends_with(".ghe.com") => format!("https://api.{host}"),
            ForgeKind::GitHub => format!("{}/api/v3", forge.url),
            ForgeKind::GitLab => format!("{}/api/v4", forge.url),
            ForgeKind::Gitea | ForgeKind::Forgejo => format!("{}/api/v1", forge.url),
        })
    }

    fn headers(&self, kind: ForgeKind) -> Vec<(&'static str, String)> {
        let mut h = vec![("Accept", "application/json".to_string())];
        if kind == ForgeKind::GitHub {
            h[0].1 = "application/vnd.github+json".to_string();
            h.push(("X-GitHub-Api-Version", "2022-11-28".to_string()));
        }
        if let Some(t) = self.token(kind) {
            h.push((
                "Authorization",
                match kind {
                    ForgeKind::Gitea | ForgeKind::Forgejo => format!("token {t}"),
                    _ => format!("Bearer {t}"),
                },
            ));
        }
        h
    }
}

/// Validate a base URL: http(s) scheme, credentials removed, no trailing slash.
pub fn clean_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("https".to_string(), url),
    };
    if scheme != "https" && scheme != "http" {
        return Err(format!("unsupported scheme `{scheme}`"));
    }
    // Drop `user:secret@`: credentials belong in a token variable, not a URL.
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if authority.is_empty() {
        return Err("no host".to_string());
    }
    Ok(if path.is_empty() {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/{path}")
    })
}

/// `host[:port]` of a URL, lower-cased, without credentials.
fn authority_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or_default();
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, h)| h)
        .to_ascii_lowercase()
}

fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h == "localhost" || h == "::1" || h.starts_with("127.")
}

/// An API path must be plain segments: no empty, `.` or `..` segment, no query tricks.
pub fn check_api_path(path: &str) -> Result<(), String> {
    let (p, query) = path.split_once('?').map_or((path, ""), |(p, q)| (p, q));
    let ok_char = |c: char| c.is_ascii_alphanumeric() || "-_.~%".contains(c);
    for seg in p.split('/') {
        if seg.is_empty() || seg.starts_with('.') || !seg.chars().all(ok_char) {
            return Err(format!("refusing API path `{path}`"));
        }
    }
    if !query.chars().all(|c| ok_char(c) || "=&,".contains(c)) {
        return Err(format!("refusing API query in `{path}`"));
    }
    Ok(())
}

/// A write to a forge's API (`discipline check --comment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMethod {
    Post,
    Patch,
    Put,
}

/// Why a write did not land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The forge refused it (HTTP 401 / 403 / 404): the token cannot write here, as on
    /// a pull request from a fork.
    Denied(String),
    /// The forge could not be reached or answered with another failure.
    Failed(String),
}

/// Write access to a forge's REST API. Only `discipline check --comment` writes.
pub trait ForgeWrite {
    /// Send `body` as JSON to `path` (relative to the API base) and return the answer.
    fn send(
        &self,
        forge: &Forge,
        method: WriteMethod,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, WriteError>;
}

impl HttpApi<'_> {
    /// The API base, its host, and whether plain HTTP is allowed, after the checks
    /// every request passes: a plain path, https (or loopback / an explicit opt-in),
    /// and `DISCIPLINE_NO_NETWORK` honoured.
    fn guard(&self, forge: &Forge, path: &str) -> Result<(String, String, bool), String> {
        check_api_path(path)?;
        let base = self.api_base(forge)?;
        let base_host = host_of(&base);
        let insecure_ok = is_loopback(&base_host) || self.flag("DISCIPLINE_FORGE_ALLOW_HTTP");
        if !base.starts_with("https://") && !insecure_ok {
            return Err(format!(
                "refusing plain HTTP to {base_host}: use https, or set DISCIPLINE_FORGE_ALLOW_HTTP=1"
            ));
        }
        if self.flag("DISCIPLINE_NO_NETWORK") && !is_loopback(&base_host) {
            return Err(format!(
                "network access is disabled (DISCIPLINE_NO_NETWORK); cannot reach {base_host}"
            ));
        }
        Ok((base, base_host, insecure_ok))
    }

    fn agent(&self, insecure_ok: bool) -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(API_TIMEOUT_SECS)))
            .max_redirects(0)
            .http_status_as_error(false)
            .https_only(!insecure_ok)
            .proxy(ureq::Proxy::try_from_env())
            .user_agent(concat!("discipline/", env!("CARGO_PKG_VERSION")))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        ureq::Agent::new_with_config(config)
    }
}

impl ForgeWrite for HttpApi<'_> {
    fn send(
        &self,
        forge: &Forge,
        method: WriteMethod,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, WriteError> {
        let (base, base_host, insecure_ok) = self.guard(forge, path).map_err(WriteError::Failed)?;
        if self.token(forge.kind).is_none() {
            return Err(WriteError::Denied(format!(
                "no token to write to {base_host} (DISCIPLINE_FORGE_TOKEN or the forge's token variable)"
            )));
        }
        let agent = self.agent(insecure_ok);
        let url = format!("{base}/{path}");
        let mut req = match method {
            WriteMethod::Post => agent.post(&url),
            WriteMethod::Patch => agent.patch(&url),
            WriteMethod::Put => agent.put(&url),
        };
        for (k, v) in &self.headers(forge.kind) {
            req = req.header(*k, v);
        }
        let mut resp = req
            .header("Content-Type", "application/json")
            .send(body.to_string().as_str())
            .map_err(|e| WriteError::Failed(format!("request to {base_host} failed: {e}")))?;
        let status = resp.status().as_u16();
        // A write is never replayed against another location.
        if (300..400).contains(&status) {
            return Err(WriteError::Failed(format!(
                "{base_host} redirected a write (HTTP {status}); not following"
            )));
        }
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_string()
            .map_err(|e| {
                WriteError::Failed(format!("reading the response from {base_host} failed: {e}"))
            })?;
        match status {
            200..=299 => Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)),
            401 | 403 | 404 => Err(WriteError::Denied(format!(
                "{base_host} refused the write (HTTP {status})"
            ))),
            _ => Err(WriteError::Failed(format!(
                "{base_host} answered HTTP {status}"
            ))),
        }
    }
}

/// Requests made by this process, retries included (see [`MAX_REQUESTS`]).
static REQUESTS_MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One HTTP answer.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpApi<'_> {
    /// One request, redirects followed within the same scheme and host (a GET only),
    /// and the answer read. A transport failure is [`ForgeErrorKind::Unavailable`].
    fn exchange(
        &self,
        forge: &Forge,
        url: &str,
        base_host: &str,
        insecure_ok: bool,
        post: Option<&serde_json::Value>,
    ) -> Result<Answer, ForgeError> {
        let agent = self.agent(insecure_ok);
        let headers = self.headers(forge.kind);
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let sent = match post {
                Some(body) => {
                    let mut req = agent.post(&url);
                    for (k, v) in &headers {
                        req = req.header(*k, v);
                    }
                    req.header("Content-Type", "application/json")
                        .send(body.to_string().as_str())
                }
                None => {
                    let mut req = agent.get(&url);
                    for (k, v) in &headers {
                        req = req.header(*k, v);
                    }
                    req.call()
                }
            };
            let mut resp = sent.map_err(|e| {
                ForgeError::new(
                    ForgeErrorKind::Unavailable,
                    format!("request to {base_host} failed: {e}"),
                )
            })?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                if post.is_some() {
                    return Err(ForgeError::new(
                        ForgeErrorKind::Malformed,
                        format!("{base_host} redirected a query (HTTP {status}); not following"),
                    ));
                }
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        ForgeError::new(
                            ForgeErrorKind::Malformed,
                            format!("{base_host} redirected without a location"),
                        )
                    })?;
                let next = if location.starts_with('/') {
                    let origin_end = url.find("://").map(|i| i + 3).unwrap_or(0);
                    let origin_end = url[origin_end..]
                        .find('/')
                        .map(|i| i + origin_end)
                        .unwrap_or(url.len());
                    format!("{}{location}", &url[..origin_end])
                } else {
                    location.to_string()
                };
                let same_scheme = next.split("://").next() == url.split("://").next();
                if authority_of(&next) != authority_of(&url) || !same_scheme {
                    return Err(ForgeError::new(
                        ForgeErrorKind::Malformed,
                        format!(
                            "{base_host} redirected to another host or scheme; not following with credentials"
                        ),
                    ));
                }
                url = next;
                continue;
            }
            let answer_headers = resp
                .headers()
                .iter()
                .filter_map(|(k, v)| {
                    Some((
                        k.as_str().to_ascii_lowercase(),
                        v.to_str().ok()?.to_string(),
                    ))
                })
                .collect();
            let body = resp
                .body_mut()
                .with_config()
                .limit(MAX_BODY_BYTES)
                .read_to_string()
                .map_err(|e| {
                    ForgeError::new(
                        ForgeErrorKind::Unavailable,
                        format!("reading the response from {base_host} failed: {e}"),
                    )
                })?;
            return Ok(Answer {
                status,
                headers: answer_headers,
                body,
            });
        }
        Err(ForgeError::new(
            ForgeErrorKind::Malformed,
            format!("{base_host} redirected more than {MAX_REDIRECTS} times"),
        ))
    }

    /// A read with bounded retries: a transport failure or a 5xx is retried after
    /// [`RETRY_BACKOFF_MS`], a rate limit after the wait it asks for when that is at most
    /// [`MAX_RATE_LIMIT_WAIT_SECS`]. Anything else is answered at once.
    fn read(
        &self,
        forge: &Forge,
        url: &str,
        base_host: &str,
        insecure_ok: bool,
        post: Option<&serde_json::Value>,
    ) -> Result<Answer, ForgeError> {
        let mut attempt = 1;
        loop {
            if REQUESTS_MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= MAX_REQUESTS {
                return Err(ForgeError {
                    kind: ForgeErrorKind::Unavailable,
                    message: format!(
                        "more than {MAX_REQUESTS} forge requests in one run; stopping"
                    ),
                    attempts: attempt,
                });
            }
            let (kind, message, headers) =
                match self.exchange(forge, url, base_host, insecure_ok, post) {
                    Ok(a) => match classify_status(a.status, &a.headers) {
                        None => return Ok(a),
                        Some(kind) => {
                            let message = serde_json::from_str::<serde_json::Value>(&a.body)
                                .ok()
                                .and_then(|v| {
                                    v.get("message")
                                        .and_then(|m| m.as_str())
                                        .map(str::to_string)
                                })
                                .map(|m| m.chars().take(200).collect::<String>())
                                .unwrap_or_default();
                            let text = format!("{base_host} answered HTTP {} {message}", a.status)
                                .trim_end()
                                .to_string();
                            (kind, text, a.headers)
                        }
                    },
                    Err(e) if e.kind == ForgeErrorKind::Unavailable => {
                        (e.kind, e.message, Vec::new())
                    }
                    Err(mut e) => {
                        e.attempts = attempt;
                        return Err(e);
                    }
                };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            match retry_delay(kind, &headers, attempt, now) {
                Some(wait) => {
                    std::thread::sleep(wait);
                    attempt += 1;
                }
                None => {
                    return Err(ForgeError {
                        kind,
                        message,
                        attempts: attempt,
                    })
                }
            }
        }
    }

    /// The GraphQL endpoint of a GitHub API base: `<root>/api/graphql` on GitHub
    /// Enterprise Server (whose REST base is `<root>/api/v3`), `<base>/graphql` otherwise.
    pub fn graphql_url(&self, forge: &Forge) -> Result<String, String> {
        let base = self.api_base(forge)?;
        Ok(match base.strip_suffix("/api/v3") {
            Some(root) => format!("{root}/api/graphql"),
            None => format!("{base}/graphql"),
        })
    }
}

impl ForgeApi for HttpApi<'_> {
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String> {
        let (base, base_host, insecure_ok) = self.guard(forge, path)?;
        match self.read(
            forge,
            &format!("{base}/{path}"),
            &base_host,
            insecure_ok,
            None,
        ) {
            Ok(a) => serde_json::from_str(&a.body)
                .map(Some)
                .map_err(|e| format!("{base_host} returned non-JSON: {e}")),
            Err(e) if e.kind == ForgeErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.detail()),
        }
    }

    fn fetch(&self, forge: &Forge, path: &str) -> Result<serde_json::Value, ForgeError> {
        let (base, base_host, insecure_ok) = self
            .guard(forge, path)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Unavailable, e))?;
        let a = self.read(
            forge,
            &format!("{base}/{path}"),
            &base_host,
            insecure_ok,
            None,
        )?;
        serde_json::from_str(&a.body).map_err(|e| {
            ForgeError::new(
                ForgeErrorKind::Malformed,
                format!("{base_host} returned non-JSON: {e}"),
            )
        })
    }

    fn get_page(&self, forge: &Forge, path: &str) -> Result<Page, ForgeError> {
        let (base, base_host, insecure_ok) = self
            .guard(forge, path)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Unavailable, e))?;
        let a = self.read(
            forge,
            &format!("{base}/{path}"),
            &base_host,
            insecure_ok,
            None,
        )?;
        let body = serde_json::from_str(&a.body).map_err(|e| {
            ForgeError::new(
                ForgeErrorKind::Malformed,
                format!("{base_host} returned non-JSON: {e}"),
            )
        })?;
        Page::from_answer(body, &a.headers)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Malformed, format!("{base_host}: {e}")))
    }

    fn graphql(
        &self,
        forge: &Forge,
        query: &str,
        variables: &serde_json::Value,
    ) -> Result<serde_json::Value, ForgeError> {
        if forge.kind != ForgeKind::GitHub {
            return Err(ForgeError::new(
                ForgeErrorKind::Malformed,
                format!("GraphQL is not available for {}", forge.kind.label()),
            ));
        }
        let (_, base_host, insecure_ok) = self
            .guard(forge, "graphql")
            .map_err(|e| ForgeError::new(ForgeErrorKind::Unavailable, e))?;
        let url = self
            .graphql_url(forge)
            .map_err(|e| ForgeError::new(ForgeErrorKind::Malformed, e))?;
        let body = serde_json::json!({"query": query, "variables": variables});
        let a = self.read(forge, &url, &base_host, insecure_ok, Some(&body))?;
        let answer = serde_json::from_str(&a.body).map_err(|e| {
            ForgeError::new(
                ForgeErrorKind::Malformed,
                format!("{base_host} returned non-JSON: {e}"),
            )
        })?;
        graphql_data(answer)
    }
}

/// Percent-encode a GitLab project path for `projects/:id`.
pub fn gitlab_project_id(repo: &str) -> String {
    repo.replace('%', "%25").replace('/', "%2F")
}

/// Whether issue `number` of `repo` (on `forge`'s host) is open.
pub fn issue_is_open(
    api: &dyn ForgeApi,
    forge: &Forge,
    repo: &str,
    number: &str,
) -> Result<bool, String> {
    let path = match forge.kind {
        ForgeKind::GitLab => format!("projects/{}/issues/{number}", gitlab_project_id(repo)),
        _ => format!("repos/{repo}/issues/{number}"),
    };
    let issue = api
        .get(forge, &path)?
        .ok_or_else(|| format!("issue #{number} of {repo} does not exist or is not visible"))?;
    match issue.get("state").and_then(|s| s.as_str()) {
        Some(s) if s.eq_ignore_ascii_case("open") || s.eq_ignore_ascii_case("opened") => Ok(true),
        Some(s) if s.eq_ignore_ascii_case("closed") || s.eq_ignore_ascii_case("merged") => {
            Ok(false)
        }
        other => Err(format!(
            "issue #{number} of {repo} has no readable state ({other:?})"
        )),
    }
}

/// The merged pull request a commit arrived through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPull {
    pub number: u64,
    pub author: String,
    pub body: String,
    /// The pull request's head commit, for `require_approval`.
    pub head_sha: String,
}

/// The merged pull request (or merge request) that carried `sha`, if the forge knows one.
///
/// GitHub lists every pull request a commit belongs to (`commits/{sha}/pulls`); the
/// merged one is taken, and a commit in several merged pull requests is refused rather
/// than guessed. Gitea and Forgejo answer with the one merged pull request
/// (`commits/{sha}/pull`, 404 when none). GitLab lists the merge requests
/// (`repository/commits/:sha/merge_requests`), filtered to `state == "merged"`.
/// `Ok(None)` is a direct push: no merged pull request carried the commit.
pub fn merged_pull_for_commit(
    api: &dyn ForgeApi,
    forge: &Forge,
    sha: &str,
) -> Result<Option<MergedPull>, String> {
    let str_of = |v: &serde_json::Value, keys: &[&str]| -> String {
        let mut cur = v;
        for k in keys {
            match cur.get(k) {
                Some(n) => cur = n,
                None => return String::new(),
            }
        }
        cur.as_str().unwrap_or_default().to_string()
    };
    match forge.kind {
        ForgeKind::GitHub => {
            let path = format!("repos/{}/commits/{sha}/pulls", forge.repo);
            let Some(list) = api.get(forge, &path)? else {
                return Ok(None);
            };
            let list = list
                .as_array()
                .ok_or_else(|| format!("pull requests of commit {sha} are not a list"))?;
            let merged: Vec<&serde_json::Value> = list
                .iter()
                .filter(|pr| pr.get("merged_at").is_some_and(|m| !m.is_null()))
                .collect();
            match merged.as_slice() {
                [] => Ok(None),
                [pr] => Ok(Some(MergedPull {
                    number: pr.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
                    author: str_of(pr, &["user", "login"]),
                    body: str_of(pr, &["body"]),
                    head_sha: str_of(pr, &["head", "sha"]),
                })),
                many => Err(format!(
                    "commit {sha} belongs to {} merged pull requests; refusing to pick one",
                    many.len()
                )),
            }
        }
        ForgeKind::Gitea | ForgeKind::Forgejo => {
            let path = format!("repos/{}/commits/{sha}/pull", forge.repo);
            let Some(pr) = api.get(forge, &path)? else {
                return Ok(None);
            };
            let merged = pr.get("merged").and_then(|m| m.as_bool()).unwrap_or(false);
            if !merged {
                return Ok(None);
            }
            Ok(Some(MergedPull {
                number: pr.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
                author: str_of(&pr, &["user", "login"]),
                body: str_of(&pr, &["body"]),
                head_sha: str_of(&pr, &["head", "sha"]),
            }))
        }
        ForgeKind::GitLab => {
            let path = format!(
                "projects/{}/repository/commits/{sha}/merge_requests",
                gitlab_project_id(&forge.repo)
            );
            let Some(list) = api.get(forge, &path)? else {
                return Ok(None);
            };
            let list = list
                .as_array()
                .ok_or_else(|| format!("merge requests of commit {sha} are not a list"))?;
            let merged: Vec<&serde_json::Value> = list
                .iter()
                .filter(|mr| mr.get("state").and_then(|s| s.as_str()) == Some("merged"))
                .collect();
            match merged.as_slice() {
                [] => Ok(None),
                [mr] => Ok(Some(MergedPull {
                    number: mr.get("iid").and_then(|n| n.as_u64()).unwrap_or(0),
                    author: str_of(mr, &["author", "username"]),
                    body: str_of(mr, &["description"]),
                    head_sha: str_of(mr, &["sha"]),
                })),
                many => Err(format!(
                    "commit {sha} belongs to {} merged merge requests; refusing to pick one",
                    many.len()
                )),
            }
        }
    }
}

/// GitLab: usernames that approve merge request `iid` **at** `head_sha`. The merge request's
/// `sha` is its current head; an approval is only read when it is the head being checked
/// (GitLab resets approvals on a new push when the project says so, and the check does not
/// rely on that). The merge request's author is never an approver.
fn gitlab_approvers(
    api: &dyn ForgeApi,
    forge: &Forge,
    iid: u64,
    head_sha: &str,
) -> Result<Vec<String>, String> {
    let project = gitlab_project_id(&forge.repo);
    let mr = api
        .get(forge, &format!("projects/{project}/merge_requests/{iid}"))?
        .ok_or_else(|| format!("merge request !{iid} does not exist or is not visible"))?;
    let sha = mr.get("sha").and_then(|v| v.as_str()).unwrap_or_default();
    if sha != head_sha {
        // The approvals on record are of another head.
        return Ok(Vec::new());
    }
    let author = mr
        .get("author")
        .and_then(|a| a.get("username"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let approvals = api
        .get(
            forge,
            &format!("projects/{project}/merge_requests/{iid}/approvals"),
        )?
        .ok_or_else(|| format!("approvals of merge request !{iid} are not visible"))?;
    let list = approvals
        .get("approved_by")
        .and_then(|a| a.as_array())
        .ok_or_else(|| format!("approvals of merge request !{iid} carry no `approved_by` list"))?;
    Ok(list
        .iter()
        .filter_map(|e| {
            e.get("user")
                .and_then(|u| u.get("username"))
                .and_then(|u| u.as_str())
        })
        .map(|u| u.to_ascii_lowercase())
        .filter(|u| *u != author)
        .collect())
}

/// The most reviews one GitHub request returns; a full page means there may be more.
const REVIEWS_PAGE: usize = 100;

/// Every review of Gitea or Forgejo pull request `number`, in order, or an error when
/// the whole list cannot be read ([`read_all`]: `limit` paging to `X-Total-Count`).
fn gitea_reviews(
    api: &dyn ForgeApi,
    forge: &Forge,
    number: u64,
) -> Result<Vec<serde_json::Value>, String> {
    read_all(
        api,
        forge,
        &format!("repos/{}/pulls/{number}/reviews", forge.repo),
    )
    .map_err(|e| format!("reviews of pull request #{number}: {e}"))
}

/// Logins whose **latest** review of pull request `number` approves `head_sha`.
///
/// An approval of an earlier commit does not count: the head it approved is not the head
/// being checked. A later review by the same login (changes requested, dismissed)
/// withdraws an earlier approval, so the whole list is read or the lookup fails.
pub fn pull_approvers(
    api: &dyn ForgeApi,
    forge: &Forge,
    number: u64,
    head_sha: &str,
) -> Result<Vec<String>, String> {
    let reviews = match forge.kind {
        ForgeKind::GitLab => return gitlab_approvers(api, forge, number, head_sha),
        ForgeKind::Gitea | ForgeKind::Forgejo => gitea_reviews(api, forge, number)?,
        ForgeKind::GitHub => {
            let path = format!(
                "repos/{}/pulls/{number}/reviews?per_page={REVIEWS_PAGE}",
                forge.repo
            );
            let list = api.get(forge, &path)?.ok_or_else(|| {
                format!("pull request #{number} does not exist or is not visible")
            })?;
            let reviews = list
                .as_array()
                .ok_or_else(|| format!("reviews of pull request #{number} are not a list"))?
                .clone();
            if reviews.len() >= REVIEWS_PAGE {
                return Err(format!(
                    "pull request #{number} has {REVIEWS_PAGE} or more reviews; refusing to judge a partial list"
                ));
            }
            reviews
        }
    };
    // The list is chronological; the last entry per login is that login's standing.
    let mut latest: std::collections::BTreeMap<String, (String, String)> = Default::default();
    for r in &reviews {
        let field = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        let login = r
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or_default();
        let state = field("state").to_ascii_uppercase();
        // A plain comment does not change a reviewer's standing.
        if login.is_empty() || state == "COMMENTED" || state == "COMMENT" || state == "PENDING" {
            continue;
        }
        latest.insert(
            login.to_ascii_lowercase(),
            (state, field("commit_id").to_string()),
        );
    }
    Ok(latest
        .into_iter()
        .filter(|(_, (state, sha))| state == "APPROVED" && sha.eq_ignore_ascii_case(head_sha))
        .map(|(login, _)| login)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvers_are_latest_reviews_of_the_checked_head_only() {
        let forge = Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        };
        let review = |login: &str, state: &str, sha: &str| serde_json::json!({"user": {"login": login}, "state": state, "commit_id": sha});
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/pulls/7/reviews?per_page=100".into(),
            serde_json::json!([
                review("Lead", "APPROVED", "head"),
                review("stale", "APPROVED", "older"),
                review("flipped", "APPROVED", "head"),
                review("flipped", "CHANGES_REQUESTED", "head"),
                review("chatty", "APPROVED", "head"),
                review("chatty", "COMMENTED", "head"),
            ]),
        );
        assert_eq!(
            pull_approvers(&api, &forge, 7, "head").unwrap(),
            vec!["chatty".to_string(), "lead".to_string()]
        );
        // Missing pull request, and a forge without the lookup, are errors, not "nobody".
        assert!(pull_approvers(&api, &forge, 8, "head").is_err());
        let gitlab = Forge {
            kind: ForgeKind::GitLab,
            ..forge.clone()
        };
        assert!(pull_approvers(&api, &gitlab, 7, "head").is_err());
    }

    #[test]
    fn gitea_reviews_are_paged_by_limit_so_a_later_withdrawal_counts() {
        let forge = Forge {
            kind: ForgeKind::Gitea,
            url: "https://gitea.example".into(),
            repo: "o/r".into(),
        };
        let review = |login: &str, state: &str| serde_json::json!({"user": {"login": login}, "state": state, "commit_id": "head"});
        // `lead` approves first; 29 comments follow; `lead` then requests changes as review 31.
        let mut first: Vec<_> = vec![review("lead", "APPROVED")];
        first.extend((0..29).map(|i| review(&format!("c{i}"), "COMMENT")));
        let withdrawn = vec![review("lead", "REQUEST_CHANGES")];
        let page = |n: usize| format!("gitea:repos/o/r/pulls/7/reviews?limit=50&page={n}");
        // Sent with X-Total-Count, as Gitea and Forgejo do.
        let counted = |v: &Vec<serde_json::Value>, total: u64| serde_json::json!({"__status": 200, "__headers": {"X-Total-Count": total.to_string()}, "__body": v});
        let pages = |total: u64, p2: &Vec<serde_json::Value>| {
            let mut api = CannedApi::default();
            // Gitea ignores `per_page` and answers with its default page of 30.
            api.responses.insert(
                "gitea:repos/o/r/pulls/7/reviews?per_page=100".into(),
                serde_json::json!(first),
            );
            // A server whose MAX_RESPONSE_ITEMS is 30 clamps `limit=50` to 30: page 1 is short.
            api.responses.insert(page(1), counted(&first, total));
            api.responses.insert(page(2), counted(p2, total));
            api.responses.insert(page(3), counted(&vec![], total));
            api
        };
        let api = pages(31, &withdrawn);
        assert_eq!(
            pull_approvers(&api, &forge, 7, "head").unwrap(),
            Vec::<String>::new(),
            "an approval withdrawn past the first page still counted"
        );

        // Without X-Total-Count, pages are read until one comes back empty.
        let mut uncounted = CannedApi::default();
        uncounted
            .responses
            .insert(page(1), serde_json::json!(first));
        uncounted
            .responses
            .insert(page(2), serde_json::json!(withdrawn));
        uncounted.responses.insert(page(3), serde_json::json!([]));
        assert!(pull_approvers(&uncounted, &forge, 7, "head")
            .unwrap()
            .is_empty());

        // Pages that end before the count, or overshoot it, or a count that moves: refused.
        let short = pages(31, &vec![]);
        assert!(pull_approvers(&short, &forge, 7, "head")
            .unwrap_err()
            .contains("ended after 30 of 31"));
        let over = pages(29, &withdrawn);
        assert!(pull_approvers(&over, &forge, 7, "head")
            .unwrap_err()
            .contains("forge counts 29"));
        let mut moved = pages(31, &withdrawn);
        moved.responses.insert(page(2), counted(&withdrawn, 32));
        assert!(pull_approvers(&moved, &forge, 7, "head")
            .unwrap_err()
            .contains("changed while it was read"));

        // A forge that ignores `page` and states no total repeats page 1: refused, on
        // Gitea and Forgejo alike.
        let mut endless = CannedApi::default();
        for n in 1..=MAX_PAGES {
            endless.responses.insert(page(n), serde_json::json!(first));
        }
        let forgejo = Forge {
            kind: ForgeKind::Forgejo,
            ..forge.clone()
        };
        let mut endless_forgejo = CannedApi::default();
        for (k, v) in &endless.responses {
            endless_forgejo
                .responses
                .insert(k.replacen("gitea:", "forgejo:", 1), v.clone());
        }
        assert!(pull_approvers(&endless, &forge, 7, "head")
            .unwrap_err()
            .contains("refusing to judge a partial list"));
        assert!(pull_approvers(&endless_forgejo, &forgejo, 7, "head")
            .unwrap_err()
            .contains("refusing to judge a partial list"));
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn remotes_parse_in_every_form() {
        let r = parse_remote("https://github.com/o/r.git").unwrap();
        assert_eq!(
            (r.url.as_str(), r.repo.as_str()),
            ("https://github.com", "o/r")
        );
        let r = parse_remote("git@gitlab.com:group/sub/proj.git").unwrap();
        assert_eq!(
            (r.url.as_str(), r.repo.as_str()),
            ("https://gitlab.com", "group/sub/proj")
        );
        let r = parse_remote("ssh://git@codeberg.org:2222/o/r.git").unwrap();
        assert_eq!(
            (r.url.as_str(), r.repo.as_str()),
            ("https://codeberg.org", "o/r")
        );
        let r = parse_remote("http://127.0.0.1:3000/o/r").unwrap();
        assert_eq!(
            (r.url.as_str(), r.repo.as_str()),
            ("http://127.0.0.1:3000", "o/r")
        );
        let r = parse_remote("https://user:secret@git.example.com/o/r.git").unwrap();
        assert!(!r.url.contains("secret"), "{r:?}");
    }

    #[test]
    fn detection_order_is_explicit_then_ci_then_host() {
        let d = |pairs: &[(&str, &str)], origin: Option<&str>| detect(&env(pairs), origin);
        let gh = d(&[], Some("https://github.com/o/r.git")).unwrap();
        assert_eq!(gh.kind, ForgeKind::GitHub);
        assert_eq!(
            d(&[], Some("git@gitlab.com:g/p.git")).unwrap().kind,
            ForgeKind::GitLab
        );
        assert_eq!(
            d(&[], Some("https://codeberg.org/o/r")).unwrap().kind,
            ForgeKind::Forgejo
        );
        assert_eq!(
            d(&[], Some("https://gitea.example.com/o/r")).unwrap().kind,
            ForgeKind::Gitea
        );

        // A self-hosted name says nothing; the runner or an explicit setting does.
        assert!(d(&[], Some("https://git.example.com/o/r")).is_err());
        let gitea = d(
            &[
                ("GITEA_ACTIONS", "true"),
                ("GITHUB_ACTIONS", "true"),
                ("GITHUB_SERVER_URL", "https://git.example.com"),
                ("GITHUB_REPOSITORY", "o/r"),
            ],
            Some("https://git.example.com/o/r"),
        )
        .unwrap();
        assert_eq!(
            gitea,
            Forge {
                kind: ForgeKind::Gitea,
                url: "https://git.example.com".into(),
                repo: "o/r".into()
            }
        );
        let fj = d(
            &[("FORGEJO_ACTIONS", "true"), ("GITEA_ACTIONS", "true")],
            Some("https://git.example.com/o/r"),
        )
        .unwrap();
        assert_eq!(fj.kind, ForgeKind::Forgejo);
        let gl = d(
            &[
                ("GITLAB_CI", "true"),
                ("CI_SERVER_URL", "https://git.example.com"),
                ("CI_PROJECT_PATH", "g/s/p"),
            ],
            None,
        )
        .unwrap();
        assert_eq!((gl.kind, gl.repo.as_str()), (ForgeKind::GitLab, "g/s/p"));
        let explicit = d(
            &[("DISCIPLINE_FORGE", "forgejo")],
            Some("https://git.example.com/o/r"),
        )
        .unwrap();
        assert_eq!(
            (explicit.kind, explicit.url.as_str()),
            (ForgeKind::Forgejo, "https://git.example.com")
        );
        assert!(d(&[("DISCIPLINE_FORGE", "svn")], None).is_err());
        // Credentials in a URL variable are dropped.
        let cred = d(
            &[
                ("DISCIPLINE_FORGE", "gitlab"),
                ("DISCIPLINE_FORGE_URL", "https://oauth2:tok@gl.example.com/"),
            ],
            Some("https://gl.example.com/g/p"),
        )
        .unwrap();
        assert_eq!(cred.url, "https://gl.example.com");
        assert!(d(
            &[
                ("DISCIPLINE_FORGE", "gitea"),
                ("DISCIPLINE_FORGE_REPO", "o/../x")
            ],
            Some("https://g.example/o/r")
        )
        .is_err());
        // Without a remote, GITHUB_REPOSITORY alone still names a GitHub repository.
        let bare = d(&[("GITHUB_REPOSITORY", "o/r")], None).unwrap();
        assert_eq!((bare.kind, bare.repo.as_str()), (ForgeKind::GitHub, "o/r"));
        assert!(d(&[], None).is_err());
    }

    fn forge(kind: ForgeKind) -> Forge {
        Forge {
            kind,
            url: "https://x.example".into(),
            repo: "g/p".into(),
        }
    }

    #[test]
    fn issue_state_reads_each_forge_vocabulary() {
        let mut api = CannedApi::default();
        // Shapes recorded from Gitea 1.24, Forgejo 12 and gitlab.com.
        api.responses.insert(
            "gitea:repos/g/p/issues/1".into(),
            serde_json::json!({"state": "open"}),
        );
        api.responses.insert(
            "forgejo:repos/g/p/issues/2".into(),
            serde_json::json!({"state": "closed"}),
        );
        api.responses.insert(
            "gitlab:projects/g%2Fp/issues/3".into(),
            serde_json::json!({"state": "opened"}),
        );
        api.responses.insert(
            "gitlab:projects/g%2Fp/issues/4".into(),
            serde_json::json!({"state": "closed"}),
        );
        api.responses
            .insert("gitea:repos/g/p/issues/9".into(), serde_json::Value::Null);
        assert_eq!(
            issue_is_open(&api, &forge(ForgeKind::Gitea), "g/p", "1"),
            Ok(true)
        );
        assert_eq!(
            issue_is_open(&api, &forge(ForgeKind::Forgejo), "g/p", "2"),
            Ok(false)
        );
        assert_eq!(
            issue_is_open(&api, &forge(ForgeKind::GitLab), "g/p", "3"),
            Ok(true)
        );
        assert_eq!(
            issue_is_open(&api, &forge(ForgeKind::GitLab), "g/p", "4"),
            Ok(false)
        );
        assert!(issue_is_open(&api, &forge(ForgeKind::Gitea), "g/p", "9")
            .unwrap_err()
            .contains("does not exist"));
        assert!(issue_is_open(&NoApi, &forge(ForgeKind::Gitea), "g/p", "1").is_err());
    }

    #[test]
    fn tokens_and_api_bases_are_chosen_per_forge() {
        let e = env(&[
            ("GITEA_TOKEN", "gt"),
            ("FORGEJO_TOKEN", "ft"),
            ("GITLAB_TOKEN", "lt"),
            ("GH_TOKEN", "ht"),
        ]);
        let api = HttpApi { env: &e };
        let auth = |k| {
            api.headers(k)
                .into_iter()
                .find(|(h, _)| *h == "Authorization")
                .map(|(_, v)| v)
        };
        assert_eq!(auth(ForgeKind::Gitea).as_deref(), Some("token gt"));
        assert_eq!(auth(ForgeKind::Forgejo).as_deref(), Some("token ft"));
        assert_eq!(auth(ForgeKind::GitLab).as_deref(), Some("Bearer lt"));
        assert_eq!(auth(ForgeKind::GitHub).as_deref(), Some("Bearer ht"));
        let none = env(&[]);
        assert!(HttpApi { env: &none }
            .headers(ForgeKind::Gitea)
            .iter()
            .all(|(h, _)| *h != "Authorization"));

        let on = |kind, url: &str| Forge {
            kind,
            url: url.into(),
            repo: "o/r".into(),
        };
        let base = |f: &Forge| api.api_base(f).unwrap();
        assert_eq!(
            base(&on(ForgeKind::GitHub, "https://github.com")),
            "https://api.github.com"
        );
        assert_eq!(
            base(&on(ForgeKind::GitHub, "https://acme.ghe.com")),
            "https://api.acme.ghe.com"
        );
        assert_eq!(
            base(&on(ForgeKind::GitHub, "https://git.acme.io")),
            "https://git.acme.io/api/v3"
        );
        assert_eq!(
            base(&on(ForgeKind::GitLab, "https://gitlab.com")),
            "https://gitlab.com/api/v4"
        );
        assert_eq!(
            base(&on(ForgeKind::Forgejo, "https://codeberg.org")),
            "https://codeberg.org/api/v1"
        );
    }

    #[test]
    fn urls_lose_credentials_and_paths_cannot_walk() {
        assert_eq!(
            clean_url("https://oauth2:secret@gitlab.example.com/").unwrap(),
            "https://gitlab.example.com"
        );
        assert_eq!(
            clean_url("git.example.com").unwrap(),
            "https://git.example.com"
        );
        assert!(clean_url("ftp://x").is_err());
        assert_eq!(
            authority_of("https://u:p@Git.Example.com:8443/a"),
            "git.example.com:8443"
        );
        assert_ne!(
            authority_of("http://127.0.0.1:1/a"),
            authority_of("http://127.0.0.1:2/a")
        );
        assert!(check_api_path("repos/o/r/issues/1").is_ok());
        assert!(check_api_path("projects/g%2Fp/issues/1").is_ok());
        assert!(check_api_path("repos/o/r/actions/runs/1/jobs?per_page=100&page=2").is_ok());
        for bad in [
            "repos/o/../x/issues/1",
            "repos//o",
            "repos/o/.git",
            "repos/o/r issues",
            "repos/o/{a,b}",
        ] {
            assert!(check_api_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn plain_http_and_disabled_network_are_refused_before_any_request() {
        let forge = Forge {
            kind: ForgeKind::Gitea,
            url: "http://git.example.com".into(),
            repo: "o/r".into(),
        };
        let e = env(&[]);
        let err = HttpApi { env: &e }.get(&forge, "repos/o/r").unwrap_err();
        assert!(err.contains("plain HTTP"), "{err}");
        let https = Forge {
            url: "https://git.example.com".into(),
            ..forge
        };
        let off = env(&[("DISCIPLINE_NO_NETWORK", "1")]);
        let err = HttpApi { env: &off }.get(&https, "repos/o/r").unwrap_err();
        assert!(err.contains("DISCIPLINE_NO_NETWORK"), "{err}");
    }

    #[test]
    fn ssh_aliases_resolve_like_openssh() {
        let cfg = "# global\nUser git\n\nHost gitea\n    HostName gitea.example.com\n    Port 2222\n\nHost *.corp !bastion.corp\n  HostName=%h.internal.example\n\nHost gitea\n  HostName shadowed.example\n\nMatch host other\n  HostName never.example\n";
        assert_eq!(
            ssh_hostname(cfg, "gitea").as_deref(),
            Some("gitea.example.com")
        );
        assert_eq!(
            ssh_hostname(cfg, "git.corp").as_deref(),
            Some("git.corp.internal.example")
        );
        assert_eq!(ssh_hostname(cfg, "bastion.corp"), None, "negated pattern");
        assert_eq!(
            ssh_hostname(cfg, "other"),
            None,
            "Match sections are not evaluated"
        );
        assert_eq!(
            ssh_hostname(
                "HostName global.example\nHost x\n HostName x.example\n",
                "x"
            )
            .as_deref(),
            Some("global.example"),
            "first value wins"
        );

        let resolve = |a: &str| ssh_hostname(cfg, a);
        assert_eq!(
            resolve_ssh_alias("git@gitea:o/r.git", &resolve),
            "git@gitea.example.com:o/r.git"
        );
        assert_eq!(
            resolve_ssh_alias("ssh://git@gitea:2222/o/r.git", &resolve),
            "ssh://git@gitea.example.com:2222/o/r.git"
        );
        assert_eq!(
            resolve_ssh_alias("gitea:o/r", &resolve),
            "gitea.example.com:o/r"
        );
        assert_eq!(
            resolve_ssh_alias("https://gitea/o/r.git", &resolve),
            "https://gitea/o/r.git",
            "not an SSH remote"
        );
        assert_eq!(
            resolve_ssh_alias("git@github.com:o/r.git", &resolve),
            "git@github.com:o/r.git",
            "no alias"
        );

        let forge = detect(
            &|_| None,
            Some(&resolve_ssh_alias("git@gitea:o/r.git", &resolve)),
        )
        .unwrap();
        assert_eq!(
            (forge.kind, forge.url.as_str(), forge.repo.as_str()),
            (ForgeKind::Gitea, "https://gitea.example.com", "o/r")
        );
    }

    #[test]
    fn ssh_config_includes_are_followed() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "Include config.d/*\nHost fallback\n HostName fallback.example\n",
        )
        .unwrap();
        std::fs::write(
            ssh.join("config.d/forge"),
            "Host forge\n  HostName forge.example.org\n",
        )
        .unwrap();
        let text = read_ssh_config(&ssh.join("config"), &ssh, 0);
        assert_eq!(
            ssh_hostname(&text, "forge").as_deref(),
            Some("forge.example.org")
        );
        assert_eq!(
            ssh_hostname(&text, "fallback").as_deref(),
            Some("fallback.example")
        );
    }

    #[test]
    fn merged_pull_lookup_per_forge_and_the_direct_push_case() {
        use super::{merged_pull_for_commit, CannedApi, MergedPull};
        let f = |kind: ForgeKind, url: &str| Forge {
            kind,
            url: url.into(),
            repo: "o/r".into(),
        };
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/commits/aaa/pulls".into(),
            serde_json::json!([
                {"number": 3, "merged_at": null, "user": {"login": "x"}, "body": "no", "head": {"sha": "h3"}},
                {"number": 12, "merged_at": "2026-09-21T00:00:00Z", "user": {"login": "agent"}, "body": "allow-agent-instructions: AGENTS.md ok", "head": {"sha": "h12"}}
            ]),
        );
        api.responses.insert(
            "github:repos/o/r/commits/bbb/pulls".into(),
            serde_json::json!([]),
        );
        api.responses.insert(
            "github:repos/o/r/commits/ccc/pulls".into(),
            serde_json::json!([
                {"number": 1, "merged_at": "2026-09-21T00:00:00Z", "user": {"login": "a"}, "body": "", "head": {"sha": "1"}},
                {"number": 2, "merged_at": "2026-09-21T00:00:00Z", "user": {"login": "b"}, "body": "", "head": {"sha": "2"}}
            ]),
        );
        api.responses.insert(
            "gitea:repos/o/r/commits/aaa/pull".into(),
            serde_json::json!({"number": 5, "merged": true, "user": {"login": "agent"}, "body": "removes: x reason", "head": {"sha": "h5"}}),
        );
        api.responses.insert(
            "gitea:repos/o/r/commits/ddd/pull".into(),
            serde_json::json!({"number": 6, "merged": false, "user": {"login": "agent"}, "body": "", "head": {"sha": "h6"}}),
        );
        api.responses.insert(
            "gitea:repos/o/r/commits/bbb/pull".into(),
            serde_json::Value::Null,
        );
        api.responses.insert(
            "gitlab:projects/o%2Fr/repository/commits/aaa/merge_requests".into(),
            serde_json::json!([
                {"iid": 9, "state": "closed", "author": {"username": "x"}, "description": "", "sha": "s9"},
                {"iid": 8, "state": "merged", "author": {"username": "agent"}, "description": "allow-ignore: t reason", "sha": "s8"}
            ]),
        );
        api.responses.insert(
            "github:repos/o/r/commits/eee/pulls".into(),
            serde_json::json!({"__error": "HTTP 401 from api.github.com"}),
        );

        let gh = f(ForgeKind::GitHub, "https://github.com");
        assert_eq!(
            merged_pull_for_commit(&api, &gh, "aaa").unwrap(),
            Some(MergedPull {
                number: 12,
                author: "agent".into(),
                body: "allow-agent-instructions: AGENTS.md ok".into(),
                head_sha: "h12".into()
            })
        );
        assert_eq!(merged_pull_for_commit(&api, &gh, "bbb").unwrap(), None);
        assert!(merged_pull_for_commit(&api, &gh, "ccc")
            .unwrap_err()
            .contains("2 merged pull requests"));
        assert!(merged_pull_for_commit(&api, &gh, "eee")
            .unwrap_err()
            .contains("401"));

        let gt = f(ForgeKind::Gitea, "https://gitea.example");
        assert_eq!(
            merged_pull_for_commit(&api, &gt, "aaa")
                .unwrap()
                .map(|p| p.number),
            Some(5)
        );
        assert_eq!(merged_pull_for_commit(&api, &gt, "ddd").unwrap(), None);
        assert_eq!(merged_pull_for_commit(&api, &gt, "bbb").unwrap(), None);

        let gl = f(ForgeKind::GitLab, "https://gitlab.com");
        let mr = merged_pull_for_commit(&api, &gl, "aaa").unwrap().unwrap();
        assert_eq!(
            (mr.number, mr.author.as_str(), mr.head_sha.as_str()),
            (8, "agent", "s8")
        );
    }

    #[test]
    fn gitlab_approvals_count_only_at_the_head_and_never_the_author() {
        use super::{pull_approvers, CannedApi};
        let forge = Forge {
            kind: ForgeKind::GitLab,
            url: "https://gitlab.com".into(),
            repo: "o/r".into(),
        };
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitlab:projects/o%2Fr/merge_requests/7".into(),
            serde_json::json!({"iid": 7, "sha": "abc123", "author": {"username": "Agent"}}),
        );
        api.responses.insert(
            "gitlab:projects/o%2Fr/merge_requests/7/approvals".into(),
            serde_json::json!({"approved_by": [{"user": {"username": "lead"}}, {"user": {"username": "agent"}}]}),
        );
        assert_eq!(
            pull_approvers(&api, &forge, 7, "abc123").unwrap(),
            vec!["lead"]
        );
        // An approval of an earlier head is refused: the record is of another commit.
        assert!(pull_approvers(&api, &forge, 7, "0ld5ha")
            .unwrap()
            .is_empty());
        api.responses.insert(
            "gitlab:projects/o%2Fr/merge_requests/8".into(),
            serde_json::Value::Null,
        );
        assert!(pull_approvers(&api, &forge, 8, "abc123")
            .unwrap_err()
            .contains("!8"));
    }

    fn gitea() -> Forge {
        Forge {
            kind: ForgeKind::Gitea,
            url: "https://git.example.com".into(),
            repo: "o/r".into(),
        }
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn statuses_are_classified_by_what_the_forge_can_prove() {
        use ForgeErrorKind::*;
        assert_eq!(classify_status(200, &[]), None);
        assert_eq!(classify_status(404, &[]), Some(NotFound));
        assert_eq!(classify_status(401, &[]), Some(Denied));
        assert_eq!(classify_status(403, &[]), Some(Denied));
        // A 403 is a rate limit only when a header says so.
        assert_eq!(
            classify_status(403, &h(&[("X-RateLimit-Remaining", "0")])),
            Some(RateLimited)
        );
        assert_eq!(
            classify_status(403, &h(&[("retry-after", "5")])),
            Some(RateLimited)
        );
        assert_eq!(classify_status(429, &[]), Some(RateLimited));
        for s in [500, 502, 503, 504] {
            assert_eq!(classify_status(s, &[]), Some(Unavailable), "{s}");
        }
        assert_eq!(classify_status(422, &[]), Some(Malformed));
    }

    #[test]
    fn retries_are_bounded_and_rate_limits_are_waited_only_when_short() {
        use ForgeErrorKind::*;
        let ms = |kind, headers: &[(String, String)], attempt, now| {
            retry_delay(kind, headers, attempt, now).map(|d| d.as_millis() as u64)
        };
        assert_eq!(ms(Unavailable, &[], 1, 0), Some(RETRY_BACKOFF_MS[0]));
        assert_eq!(ms(Unavailable, &[], 2, 0), Some(RETRY_BACKOFF_MS[1]));
        assert_eq!(ms(Unavailable, &[], MAX_ATTEMPTS, 0), None);
        for kind in [NotFound, Denied, Malformed, Partial] {
            assert_eq!(ms(kind, &[], 1, 0), None, "{kind:?}");
        }
        assert_eq!(
            ms(RateLimited, &h(&[("Retry-After", "3")]), 1, 0),
            Some(3000)
        );
        assert_eq!(
            ms(RateLimited, &h(&[("x-ratelimit-reset", "1010")]), 1, 1000),
            Some(10_000)
        );
        assert_eq!(
            ms(RateLimited, &h(&[("RateLimit-Reset", "5000")]), 1, 1000),
            None,
            "a wait past the cap gives up at once"
        );
    }

    #[test]
    fn read_all_stops_at_the_stated_total_and_refuses_a_partial_list() {
        let f = gitea();
        let comment = |id: u64| serde_json::json!({"id": id});
        let page = |ids: &[u64], total: &str| {
            serde_json::json!({
                "__status": 200,
                "__headers": {"X-Total-Count": total},
                "__body": ids.iter().map(|i| comment(*i)).collect::<Vec<_>>(),
            })
        };
        // An endpoint that ignores paging answers the whole list on page 1: one read.
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r/issues/1/comments?limit=50&page=1".into(),
            page(&[1, 2, 3], "3"),
        );
        let all = read_all(&api, &f, "repos/o/r/issues/1/comments").unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(api.log().len(), 1);

        // Paged: two pages to reach the total.
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r/pulls/1/reviews?limit=50&page=1".into(),
            page(&[1, 2], "3"),
        );
        api.responses.insert(
            "gitea:repos/o/r/pulls/1/reviews?limit=50&page=2".into(),
            page(&[3], "3"),
        );
        assert_eq!(
            read_all(&api, &f, "repos/o/r/pulls/1/reviews")
                .unwrap()
                .len(),
            3
        );

        // The total cannot be reached: the same page comes back.
        let mut api = CannedApi::default();
        for n in 1..=2 {
            api.responses.insert(
                format!("gitea:repos/o/r/issues/2/comments?limit=50&page={n}"),
                page(&[1, 2], "5"),
            );
        }
        let err = read_all(&api, &f, "repos/o/r/issues/2/comments").unwrap_err();
        assert_eq!(err.kind, ForgeErrorKind::Partial, "{err}");

        // GitHub: no total; the Link header says whether more pages exist.
        let gh = Forge {
            kind: ForgeKind::GitHub,
            url: "https://github.com".into(),
            repo: "o/r".into(),
        };
        let mut api = CannedApi::default();
        api.responses.insert(
            "github:repos/o/r/issues/3/comments?per_page=100&page=1".into(),
            serde_json::json!({"__status": 200,
                "__headers": {"Link": "<https://api.github.com/x?page=2>; rel=\"next\""},
                "__body": [comment(1)]}),
        );
        api.responses.insert(
            "github:repos/o/r/issues/3/comments?per_page=100&page=2".into(),
            serde_json::json!([comment(2)]),
        );
        assert_eq!(
            read_all(&api, &gh, "repos/o/r/issues/3/comments")
                .unwrap()
                .len(),
            2
        );

        // A failure of any page is the failure of the list.
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r/issues/4/comments?limit=50&page=1".into(),
            serde_json::json!({"__status": 500}),
        );
        let err = read_all(&api, &f, "repos/o/r/issues/4/comments").unwrap_err();
        assert_eq!(err.kind, ForgeErrorKind::Unavailable);
    }

    #[test]
    fn canned_sequences_and_graphql_answers() {
        let f = gitea();
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r".into(),
            serde_json::json!({"__sequence": [{"__status": 502}, {"id": 1}]}),
        );
        assert!(api.get(&f, "repos/o/r").is_err());
        assert_eq!(api.get(&f, "repos/o/r").unwrap().unwrap()["id"], 1);
        assert_eq!(api.get(&f, "repos/o/r").unwrap().unwrap()["id"], 1);
        assert!(api.was_called("gitea:repos/o/r"));

        assert_eq!(
            graphql_operation("query Closing($n: Int!) { x }"),
            "Closing"
        );
        assert_eq!(graphql_operation("{ viewer { login } }"), "anonymous");
        assert_eq!(
            graphql_data(serde_json::json!({"data": {"a": 1}})).unwrap(),
            serde_json::json!({"a": 1})
        );
        let err = graphql_data(serde_json::json!({"data": null,
            "errors": [{"type": "NOT_FOUND", "message": "Could not resolve"}]}))
        .unwrap_err();
        assert_eq!(err.kind, ForgeErrorKind::NotFound);
        assert!(graphql_data(serde_json::json!({"data": null})).is_err());
    }

    #[test]
    fn graphql_endpoint_follows_the_rest_base() {
        let gh = |url: &str| Forge {
            kind: ForgeKind::GitHub,
            url: url.into(),
            repo: "o/r".into(),
        };
        let none = |_: &str| None;
        let api = HttpApi { env: &none };
        assert_eq!(
            api.graphql_url(&gh("https://github.com")).unwrap(),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            api.graphql_url(&gh("https://ghe.example.com")).unwrap(),
            "https://ghe.example.com/api/graphql"
        );
        let err = api.graphql(&gitea(), "query X { y }", &serde_json::json!({}));
        assert!(err.is_err());
    }

    #[test]
    fn forge_errors_are_labelled() {
        let mut e = ForgeError::new(ForgeErrorKind::Unavailable, "host answered HTTP 502");
        e.attempts = 3;
        assert_eq!(
            e.to_string(),
            "forge-unavailable: host answered HTTP 502 (after 3 attempts)"
        );
        assert_eq!(page_query(ForgeKind::Forgejo, 2), "limit=50&page=2");
        assert_eq!(page_query(ForgeKind::GitLab, 1), "per_page=100&page=1");
    }
}
