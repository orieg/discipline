//! Which forge hosts the repository, and a read-only way to ask it questions.
//!
//! Three opt-in features need platform data that is not in the repository: the open-issue
//! check of `provenance-tags` (`require_open_pending_issues`), bench-regression citation
//! freshness, and the branch-protection checks of `discipline doctor`. They go through
//! this module. No gate needs the network otherwise.
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

fn host_of(url: &str) -> String {
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

/// Read-only access to a forge's REST API.
pub trait ForgeApi {
    /// GET `path` (relative to the forge's API base) parsed as JSON. `Ok(None)` for 404.
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String>;
}

/// Answers from canned responses keyed by `"<forge label>:<path>"`.
#[derive(Debug, Clone, Default)]
pub struct CannedApi {
    pub responses: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ForgeApi for CannedApi {
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String> {
        let key = format!("{}:{path}", forge.kind.label());
        match self.responses.get(&key) {
            Some(serde_json::Value::Null) => Ok(None),
            Some(v) if v.get("__error").is_some() => {
                Err(v["__error"].as_str().unwrap_or("error").to_string())
            }
            Some(v) => Ok(Some(v.clone())),
            None => Err(format!("no recorded response for `{key}`")),
        }
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

impl ForgeApi for HttpApi<'_> {
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String> {
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
        let agent = ureq::Agent::new_with_config(config);
        let headers = self.headers(forge.kind);

        let mut url = format!("{base}/{path}");
        for _ in 0..=MAX_REDIRECTS {
            let mut req = agent.get(&url);
            for (k, v) in &headers {
                req = req.header(*k, v);
            }
            let mut resp = req
                .call()
                .map_err(|e| format!("request to {base_host} failed: {e}"))?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| format!("{base_host} redirected without a location"))?;
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
                    return Err(format!(
                        "{base_host} redirected to another host or scheme; not following with credentials"
                    ));
                }
                url = next;
                continue;
            }
            if status == 404 {
                return Ok(None);
            }
            let body = resp
                .body_mut()
                .with_config()
                .limit(MAX_BODY_BYTES)
                .read_to_string()
                .map_err(|e| format!("reading the response from {base_host} failed: {e}"))?;
            if !(200..300).contains(&status) {
                let message = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.get("message")
                            .and_then(|m| m.as_str())
                            .map(str::to_string)
                    })
                    .map(|m| m.chars().take(200).collect::<String>())
                    .unwrap_or_default();
                return Err(format!("{base_host} answered HTTP {status} {message}")
                    .trim_end()
                    .to_string());
            }
            return serde_json::from_str(&body)
                .map(Some)
                .map_err(|e| format!("{base_host} returned non-JSON: {e}"));
        }
        Err(format!(
            "{base_host} redirected more than {MAX_REDIRECTS} times"
        ))
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

/// The most reviews one request returns; a full page means there may be more.
const REVIEWS_PAGE: usize = 100;

/// Logins whose **latest** review of pull request `number` approves `head_sha`.
///
/// An approval of an earlier commit does not count: the head it approved is not the head
/// being checked. A later review by the same login (changes requested, dismissed)
/// withdraws an earlier approval.
pub fn pull_approvers(
    api: &dyn ForgeApi,
    forge: &Forge,
    number: u64,
    head_sha: &str,
) -> Result<Vec<String>, String> {
    if forge.kind == ForgeKind::GitLab {
        return Err("review approval lookup is not implemented for GitLab".to_string());
    }
    let path = format!(
        "repos/{}/pulls/{number}/reviews?per_page={REVIEWS_PAGE}",
        forge.repo
    );
    let reviews = api
        .get(forge, &path)?
        .ok_or_else(|| format!("pull request #{number} does not exist or is not visible"))?;
    let reviews = reviews
        .as_array()
        .ok_or_else(|| format!("reviews of pull request #{number} are not a list"))?;
    if reviews.len() >= REVIEWS_PAGE {
        return Err(format!(
            "pull request #{number} has {REVIEWS_PAGE} or more reviews; refusing to judge a partial list"
        ));
    }
    // The list is chronological; the last entry per login is that login's standing.
    let mut latest: std::collections::BTreeMap<String, (String, String)> = Default::default();
    for r in reviews {
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
}
