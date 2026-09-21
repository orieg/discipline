//! Which forge hosts the repository, and a read-only way to ask it questions.
//!
//! Two features need platform data that is not in the repository: the open-issue check of
//! `provenance-tags` (`require_open_pending_issues`) and the branch-protection checks of
//! `discipline doctor`. Both go through this module.
//!
//! The binary has no network stack (AGENTS.md §3.3). Requests run through external tools
//! on the bounded command runner: `gh api` for GitHub (`DISCIPLINE_GH`, default `gh`), and
//! `curl` for GitLab, Gitea and Forgejo (`DISCIPLINE_CURL`, default `curl`). A token, when
//! one is set, is handed to `curl` in a mode-0600 config file so it never appears in the
//! process arguments.
//!
//! | Forge   | API base            | Token environment                                   |
//! |---------|---------------------|-----------------------------------------------------|
//! | GitHub  | `gh api`            | whatever `gh` is authenticated with (`GH_TOKEN`)    |
//! | GitLab  | `<url>/api/v4`      | `DISCIPLINE_FORGE_TOKEN`, else `GITLAB_TOKEN`       |
//! | Gitea   | `<url>/api/v1`      | `DISCIPLINE_FORGE_TOKEN`, else `GITEA_TOKEN`        |
//! | Forgejo | `<url>/api/v1`      | `DISCIPLINE_FORGE_TOKEN`, else `FORGEJO_TOKEN`, else `GITEA_TOKEN` |
//!
//! Without a token, requests are anonymous: enough for issue state on public repositories.

use crate::guards::perf::citation::CitationInstruments;
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
        Ok(Forge {
            kind,
            url: url.trim_end_matches('/').to_string(),
            repo: repo.trim_matches('/').to_string(),
        })
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
    detect(
        &|k| std::env::var(k).ok(),
        git.remote_url("origin").as_deref(),
    )
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

/// Live requests: `gh api` for GitHub, `curl` for the others.
pub struct LiveApi<'a> {
    pub gh: &'a dyn CitationInstruments,
    pub root: &'a std::path::Path,
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

impl LiveApi<'_> {
    fn token(&self, kind: ForgeKind) -> Option<(String, String)> {
        let var = |k: &str| (self.env)(k).filter(|v| !v.trim().is_empty());
        let names: &[&str] = match kind {
            ForgeKind::GitHub => return None,
            ForgeKind::GitLab => &["DISCIPLINE_FORGE_TOKEN", "GITLAB_TOKEN"],
            ForgeKind::Gitea => &["DISCIPLINE_FORGE_TOKEN", "GITEA_TOKEN"],
            ForgeKind::Forgejo => &["DISCIPLINE_FORGE_TOKEN", "FORGEJO_TOKEN", "GITEA_TOKEN"],
        };
        let token = names.iter().find_map(|n| var(n))?;
        let header = match kind {
            ForgeKind::GitLab => format!("PRIVATE-TOKEN: {}", token.trim()),
            _ => format!("Authorization: token {}", token.trim()),
        };
        Some((header, token))
    }
}

fn api_base(forge: &Forge) -> String {
    match forge.kind {
        ForgeKind::GitLab => format!("{}/api/v4", forge.url),
        _ => format!("{}/api/v1", forge.url),
    }
}

impl ForgeApi for LiveApi<'_> {
    fn get(&self, forge: &Forge, path: &str) -> Result<Option<serde_json::Value>, String> {
        if forge.kind == ForgeKind::GitHub {
            return match self.gh.gh_api(path) {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.contains("404") || e.contains("Not Found") => Ok(None),
                Err(e) => Err(e),
            };
        }
        let url = format!("{}/{}", api_base(forge), path.trim_start_matches('/'));
        if url.contains(['\'', '"', ' ', '\n']) {
            return Err(format!("refusing an API URL with quotes or spaces: {url}"));
        }
        let curl = (self.env)("DISCIPLINE_CURL")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "curl".to_string());
        // A token goes into a private config file, never onto the command line.
        let config = match self.token(forge.kind) {
            Some((header, _)) => Some(private_curl_config(&header)?),
            None => None,
        };
        let mut cmd = format!(
            "'{curl}' --silent --show-error --location --max-time {API_TIMEOUT_SECS} --header 'Accept: application/json' --write-out '\\n%{{http_code}}'"
        );
        if let Some(cfg) = &config {
            cmd.push_str(&format!(" --config '{}'", cfg.path.display()));
        }
        cmd.push_str(&format!(" '{url}'"));
        let run = crate::guards::command::run_command_bounded(
            "forge api",
            &cmd,
            API_TIMEOUT_SECS + 5,
            self.root,
        )
        .map_err(|e| format!("`curl` could not run: {e:#}"));
        drop(config);
        let run = run?;
        if !run.status.success() {
            let why = run
                .stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no diagnostic");
            return Err(format!("`curl` failed for {url}: {}", why.trim()));
        }
        let (body, code) = run
            .stdout
            .rsplit_once('\n')
            .ok_or_else(|| format!("`curl` returned no status for {url}"))?;
        match code.trim() {
            "404" => Ok(None),
            c if c.starts_with('2') => serde_json::from_str(body)
                .map(Some)
                .map_err(|e| format!("{url} returned non-JSON: {e}")),
            c => Err(format!("{url} answered HTTP {c}")),
        }
    }
}

/// A curl config file readable only by this user, removed on drop.
struct PrivateFile {
    path: std::path::PathBuf,
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn private_curl_config(header: &str) -> Result<PrivateFile, String> {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!(
        "discipline-forge-{}-{}.curlrc",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&path)
        .map_err(|e| format!("cannot create a private curl config: {e}"))?;
    let escaped = header.replace('\\', "\\\\").replace('"', "\\\"");
    writeln!(f, "header = \"{escaped}\"").map_err(|e| format!("cannot write curl config: {e}"))?;
    Ok(PrivateFile { path })
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn tokens_are_chosen_per_forge_and_kept_off_the_command_line() {
        let e = env(&[
            ("GITEA_TOKEN", "gt"),
            ("FORGEJO_TOKEN", "ft"),
            ("GITLAB_TOKEN", "lt"),
        ]);
        let gh = crate::guards::perf::citation::Unavailable;
        let api = LiveApi {
            gh: &gh,
            root: std::path::Path::new("."),
            env: &e,
        };
        assert_eq!(
            api.token(ForgeKind::Gitea).unwrap().0,
            "Authorization: token gt"
        );
        assert_eq!(
            api.token(ForgeKind::Forgejo).unwrap().0,
            "Authorization: token ft"
        );
        assert_eq!(api.token(ForgeKind::GitLab).unwrap().0, "PRIVATE-TOKEN: lt");
        assert!(api.token(ForgeKind::GitHub).is_none());

        let cfg = private_curl_config("Authorization: token s\"ecret").unwrap();
        let text = std::fs::read_to_string(&cfg.path).unwrap();
        assert_eq!(text, "header = \"Authorization: token s\\\"ecret\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cfg.path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let path = cfg.path.clone();
        drop(cfg);
        assert!(!path.exists(), "config must be removed");
    }
}
