//! Lockfile integrity for `dependency-delta`.
//!
//! A manifest says what a project asks for; the lockfile says what it gets. These checks
//! read the lockfile itself, offline, base side against head side:
//!
//! - an entry resolved from a source host the base lockfile never used (or moved to git),
//! - an entry that lost its integrity hash.
//!
//! Formats read: `Cargo.lock`, `package-lock.json`, `yarn.lock` (v1). Any other lockfile
//! returns `None` from [`parse_lock`] and the gate names it as not analysed.

use std::collections::{BTreeMap, BTreeSet};

/// Registry hosts a lockfile may use without the base side having used them.
const DEFAULT_HOSTS: &[&str] = &[
    "github.com/rust-lang/crates.io-index",
    "index.crates.io",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Workspace member, path dependency, or a link: nothing is fetched.
    Local,
    /// Fetched from a registry on this host.
    Registry(String),
    /// Fetched from a git repository or a bare URL / tarball on this host.
    Direct(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    pub name: String,
    pub version: String,
    pub source: Source,
    pub has_hash: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockFinding {
    pub kind: &'static crate::findings::FindingKind,
    /// Package name: what an `allow-dependency:` reason must name.
    pub package: String,
    pub message: String,
}

/// Host and path prefix of a URL, without scheme, credentials, query or fragment.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let host = rest.split('/').next().unwrap_or(rest).to_ascii_lowercase();
    // crates.io's git index is identified by its path, not by `github.com` alone.
    if host == "github.com" && rest.starts_with("github.com/rust-lang/crates.io-index") {
        return "github.com/rust-lang/crates.io-index".to_string();
    }
    host
}

fn parse_cargo_lock(content: &str) -> Option<Vec<LockEntry>> {
    let doc: toml::Value = toml::from_str(content).ok()?;
    let packages = doc.get("package")?.as_array()?;
    let mut out = Vec::new();
    for p in packages {
        let field = |k: &str| p.get(k).and_then(|v| v.as_str());
        let source = match field("source") {
            None => Source::Local,
            Some(s) if s.starts_with("registry+") || s.starts_with("sparse+") => {
                Source::Registry(host_of(s.split_once('+').map_or(s, |(_, r)| r)))
            }
            Some(s) => Source::Direct(host_of(s.split_once('+').map_or(s, |(_, r)| r))),
        };
        out.push(LockEntry {
            name: field("name")?.to_string(),
            version: field("version").unwrap_or_default().to_string(),
            source,
            has_hash: field("checksum").is_some(),
        });
    }
    Some(out)
}

fn npm_source(resolved: Option<&str>, link: bool) -> Source {
    match resolved {
        _ if link => Source::Local,
        None => Source::Local,
        Some(r) if r.starts_with("file:") => Source::Local,
        Some(r) if r.starts_with("git") || r.contains(".git#") => Source::Direct(host_of(r)),
        // A registry tarball lives under `/-/`; any other URL is a bare tarball.
        Some(r) if r.contains("/-/") => Source::Registry(host_of(r)),
        Some(r) => Source::Direct(host_of(r)),
    }
}

fn parse_package_lock(content: &str) -> Option<Vec<LockEntry>> {
    let doc: serde_json::Value = serde_json::from_str(content).ok()?;
    let mut out = Vec::new();
    fn entry(name: &str, v: &serde_json::Value) -> LockEntry {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str());
        LockEntry {
            name: name.to_string(),
            version: s("version").unwrap_or_default().to_string(),
            source: npm_source(
                s("resolved"),
                v.get("link").and_then(|l| l.as_bool()) == Some(true),
            ),
            has_hash: s("integrity").is_some(),
        }
    }
    if let Some(pkgs) = doc.get("packages").and_then(|p| p.as_object()) {
        for (path, v) in pkgs {
            if path.is_empty() {
                continue; // the project itself
            }
            let name = path
                .rsplit_once("node_modules/")
                .map_or(path.as_str(), |(_, n)| n);
            out.push(entry(name, v));
        }
        return Some(out);
    }
    fn walk(deps: &serde_json::Map<String, serde_json::Value>, out: &mut Vec<LockEntry>) {
        for (name, v) in deps {
            out.push(entry(name, v));
            if let Some(nested) = v.get("dependencies").and_then(|d| d.as_object()) {
                walk(nested, out);
            }
        }
    }
    walk(doc.get("dependencies")?.as_object()?, &mut out);
    Some(out)
}

fn parse_yarn_lock(content: &str) -> Option<Vec<LockEntry>> {
    // Yarn 2+ ("berry") is YAML with `resolution:` / `checksum:`; `parse_yarn_berry` reads it.
    if content.contains("__metadata:") || !content.contains("# yarn lockfile v1") {
        return None;
    }
    let mut out: Vec<LockEntry> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            // `"@scope/name@^1.0.0", "@scope/name@^1.1.0":`
            let first = line.trim_end_matches(':').split(',').next().unwrap_or("");
            let spec = first.trim().trim_matches('"');
            let name = spec.rsplit_once('@').map_or(spec, |(n, _)| n);
            out.push(LockEntry {
                name: name.to_string(),
                version: String::new(),
                source: Source::Local,
                has_hash: false,
            });
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        let t = line.trim();
        let value = |key: &str| {
            t.strip_prefix(key)
                .map(|v| v.trim().trim_matches('"').to_string())
        };
        if let Some(v) = value("version ") {
            cur.version = v;
        } else if let Some(v) = value("resolved ") {
            cur.source = npm_source(Some(&v), false);
        } else if t.starts_with("integrity ") {
            cur.has_hash = true;
        }
    }
    Some(out)
}

fn yaml_str<'a>(v: &'a serde_yaml::Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

/// Yarn 2+ ("berry"): YAML, one entry per descriptor set, `resolution:` names the
/// protocol (`npm:`, `git@`, `https://`), `checksum:` is the hash.
fn parse_yarn_berry(content: &str) -> Option<Vec<LockEntry>> {
    let doc: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
    let map = doc.as_mapping()?;
    let mut out = Vec::new();
    for (k, v) in map {
        let key = k.as_str().unwrap_or("");
        if key == "__metadata" {
            continue;
        }
        let resolution = yaml_str(v, "resolution").unwrap_or("");
        // `name@npm:1.0.0`, `@scope/name@npm:1.0.0`, `name@https://...`, `name@git@...`.
        let (name, locator) = resolution
            .rsplit_once('@')
            .filter(|(n, _)| !n.is_empty())
            .map(|(n, l)| (n.to_string(), l))
            .unwrap_or_else(|| {
                let first = key
                    .split(',')
                    .next()
                    .unwrap_or(key)
                    .trim()
                    .trim_matches('"');
                (
                    first.rsplit_once('@').map_or(first, |(n, _)| n).to_string(),
                    "",
                )
            });
        let source = if locator.starts_with("npm:") {
            Source::Registry("registry.yarnpkg.com".to_string())
        } else if locator.starts_with("workspace:")
            || locator.starts_with("portal:")
            || locator.starts_with("link:")
            || locator.starts_with("file:")
            || locator.is_empty()
        {
            Source::Local
        } else {
            Source::Direct(host_of(
                locator
                    .trim_start_matches("git@")
                    .trim_start_matches("git+"),
            ))
        };
        out.push(LockEntry {
            name,
            version: yaml_str(v, "version").unwrap_or_default().to_string(),
            source,
            has_hash: v.get("checksum").is_some(),
        });
    }
    Some(out)
}

/// pnpm: YAML, `packages:` keyed by `/name@version` (v6) or `name@version` (v9), each
/// with `resolution: {integrity, tarball?}`.
fn parse_pnpm_lock(content: &str) -> Option<Vec<LockEntry>> {
    let doc: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
    let packages = doc.get("packages")?.as_mapping()?;
    let mut out = Vec::new();
    for (k, v) in packages {
        let key = k.as_str().unwrap_or("").trim_start_matches('/');
        // `@scope/name@1.0.0(peer@2)`: the name is everything before the last `@` of the
        // part that precedes any peer suffix.
        let bare = key.split('(').next().unwrap_or(key);
        let (name, version) = bare
            .rsplit_once('@')
            .filter(|(n, _)| !n.is_empty())
            .unwrap_or((bare, ""));
        let resolution = v.get("resolution");
        let tarball = resolution.and_then(|r| yaml_str(r, "tarball"));
        let source = match tarball {
            Some(t) if t.contains("/-/") => Source::Registry(host_of(t)),
            Some(t) => Source::Direct(host_of(t)),
            None => {
                if resolution.and_then(|r| yaml_str(r, "type")) == Some("git")
                    || resolution.and_then(|r| yaml_str(r, "repo")).is_some()
                {
                    Source::Direct(host_of(
                        resolution.and_then(|r| yaml_str(r, "repo")).unwrap_or(""),
                    ))
                } else if resolution.and_then(|r| yaml_str(r, "directory")).is_some() {
                    Source::Local
                } else {
                    Source::Registry("registry.npmjs.org".to_string())
                }
            }
        };
        out.push(LockEntry {
            name: name.to_string(),
            version: version.to_string(),
            source,
            has_hash: resolution.and_then(|r| r.get("integrity")).is_some(),
        });
    }
    Some(out)
}

/// Poetry: TOML `[[package]]` with an optional `[package.source]` (`type` = `git`, `url`,
/// `directory`, `file`, or `legacy` for another index) and `[package.files]` hashes.
fn parse_poetry_lock(content: &str) -> Option<Vec<LockEntry>> {
    let doc: toml::Value = toml::from_str(content).ok()?;
    let packages = doc.get("package")?.as_array()?;
    let mut out = Vec::new();
    for p in packages {
        let s = |k: &str| p.get(k).and_then(|v| v.as_str());
        let src = p.get("source");
        let src_type = src.and_then(|x| x.get("type")).and_then(|v| v.as_str());
        let src_url = src.and_then(|x| x.get("url")).and_then(|v| v.as_str());
        let source = match (src_type, src_url) {
            (None, _) => Source::Registry("pypi.org".to_string()),
            (Some("legacy"), Some(u)) => Source::Registry(host_of(u)),
            (Some("directory"), _) | (Some("file"), _) => Source::Local,
            (Some(_), Some(u)) => Source::Direct(host_of(u)),
            (Some(_), None) => Source::Local,
        };
        let has_hash = p
            .get("files")
            .and_then(|f| f.as_array())
            .is_some_and(|f| f.iter().any(|e| e.get("hash").is_some()));
        out.push(LockEntry {
            name: s("name")?.to_string(),
            version: s("version").unwrap_or_default().to_string(),
            source,
            has_hash,
        });
    }
    Some(out)
}

/// uv: TOML `[[package]]` with `source = { registry | git | url | editable | path | ... }`
/// and `sdist` / `wheels` carrying `hash`.
fn parse_uv_lock(content: &str) -> Option<Vec<LockEntry>> {
    let doc: toml::Value = toml::from_str(content).ok()?;
    let packages = doc.get("package")?.as_array()?;
    let mut out = Vec::new();
    for p in packages {
        let s = |k: &str| p.get(k).and_then(|v| v.as_str());
        let src = p.get("source");
        let field = |k: &str| src.and_then(|x| x.get(k)).and_then(|v| v.as_str());
        let source = if let Some(r) = field("registry") {
            Source::Registry(host_of(r))
        } else if let Some(u) = field("git").or_else(|| field("url")) {
            Source::Direct(host_of(u))
        } else {
            Source::Local
        };
        let hashed = |k: &str| match p.get(k) {
            Some(toml::Value::Array(a)) => a.iter().any(|w| w.get("hash").is_some()),
            Some(t) => t.get("hash").is_some(),
            None => false,
        };
        out.push(LockEntry {
            name: s("name")?.to_string(),
            version: s("version").unwrap_or_default().to_string(),
            source,
            has_hash: hashed("sdist") || hashed("wheels"),
        });
    }
    Some(out)
}

/// Composer: JSON `packages` / `packages-dev`, each with `dist.url` (and `dist.shasum`,
/// empty on Packagist) and `source.url`.
fn parse_composer_lock(content: &str) -> Option<Vec<LockEntry>> {
    let doc: serde_json::Value = serde_json::from_str(content).ok()?;
    let mut out = Vec::new();
    for key in ["packages", "packages-dev"] {
        let Some(list) = doc.get(key).and_then(|p| p.as_array()) else {
            continue;
        };
        for p in list {
            let s = |k: &str| p.get(k).and_then(|v| v.as_str());
            let dist_url = p
                .get("dist")
                .and_then(|d| d.get("url"))
                .and_then(|v| v.as_str());
            let dist_type = p
                .get("dist")
                .and_then(|d| d.get("type"))
                .and_then(|v| v.as_str());
            let source = match (dist_type, dist_url) {
                (Some("path"), _) => Source::Local,
                (_, Some(u)) if u.contains("api.github.com/repos/") => {
                    Source::Registry("packagist.org".to_string())
                }
                (_, Some(u)) => Source::Direct(host_of(u)),
                (_, None) => match p
                    .get("source")
                    .and_then(|x| x.get("url"))
                    .and_then(|v| v.as_str())
                {
                    Some(u) => Source::Direct(host_of(u)),
                    None => Source::Local,
                },
            };
            let has_hash = p
                .get("dist")
                .and_then(|d| d.get("shasum"))
                .and_then(|v| v.as_str())
                .is_some_and(|h| !h.is_empty());
            out.push(LockEntry {
                name: s("name")?.to_string(),
                version: s("version").unwrap_or_default().to_string(),
                source,
                has_hash,
            });
        }
    }
    Some(out)
}

/// Bundler: `GEM` / `GIT` / `PATH` sections with a `remote:` and indented `specs:`; a
/// `CHECKSUMS` section (Bundler 2.6+) carries the hashes.
fn parse_gemfile_lock(content: &str) -> Option<Vec<LockEntry>> {
    let mut out: Vec<LockEntry> = Vec::new();
    let mut section = String::new();
    let mut remote = String::new();
    let mut in_specs = false;
    let mut checksums = std::collections::BTreeSet::new();
    for line in content.lines() {
        if !line.starts_with(' ') {
            section = line.trim().to_string();
            remote.clear();
            in_specs = false;
            continue;
        }
        let t = line.trim();
        if section == "CHECKSUMS" {
            if let Some(name) = t.split(' ').next() {
                checksums.insert(name.to_string());
            }
            continue;
        }
        if let Some(r) = t.strip_prefix("remote:") {
            remote = r.trim().to_string();
        } else if t == "specs:" {
            in_specs = true;
        } else if in_specs && line.starts_with("    ") && !line.starts_with("      ") {
            // `    name (1.2.3)`: four spaces is a spec, six a dependency of it.
            let name = t.split(' ').next().unwrap_or("").to_string();
            let version = t
                .split_once('(')
                .map(|(_, v)| v.trim_end_matches(')').to_string())
                .unwrap_or_default();
            let source = match section.as_str() {
                "GEM" => Source::Registry(host_of(&remote)),
                "GIT" => Source::Direct(host_of(&remote)),
                _ => Source::Local,
            };
            out.push(LockEntry {
                name,
                version,
                source,
                has_hash: false,
            });
        }
    }
    if out.is_empty() {
        return None;
    }
    for e in &mut out {
        e.has_hash = checksums.contains(&e.name);
    }
    Some(out)
}

/// Entries of a lockfile, or `None` when the format is not one this module reads (or the
/// content does not parse as that format).
pub fn parse_lock(file_name: &str, content: &str) -> Option<Vec<LockEntry>> {
    match file_name {
        "Cargo.lock" => parse_cargo_lock(content),
        "package-lock.json" => parse_package_lock(content),
        "yarn.lock" if content.contains("__metadata:") => parse_yarn_berry(content),
        "yarn.lock" => parse_yarn_lock(content),
        "pnpm-lock.yaml" => parse_pnpm_lock(content),
        "poetry.lock" => parse_poetry_lock(content),
        "uv.lock" => parse_uv_lock(content),
        "composer.lock" => parse_composer_lock(content),
        "Gemfile.lock" => parse_gemfile_lock(content),
        _ => None,
    }
}

/// What `head` weakens relative to `base`. An entry the base side already had, with the
/// same source and hash state, is never reported.
pub fn diff_lock(base: &[LockEntry], head: &[LockEntry]) -> Vec<LockFinding> {
    let host = |s: &Source| match s {
        Source::Registry(h) | Source::Direct(h) => Some(h.clone()),
        Source::Local => None,
    };
    let known_hosts: BTreeSet<String> = base
        .iter()
        .filter_map(|e| host(&e.source))
        .chain(DEFAULT_HOSTS.iter().map(|h| h.to_string()))
        .collect();
    let base_sources: BTreeMap<&str, BTreeSet<&Source>> =
        base.iter().fold(BTreeMap::new(), |mut m, e| {
            m.entry(e.name.as_str()).or_default().insert(&e.source);
            m
        });
    let base_hashed: BTreeSet<(&str, &str)> = base
        .iter()
        .filter(|e| e.has_hash)
        .map(|e| (e.name.as_str(), e.version.as_str()))
        .collect();

    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for e in head {
        let same_source_on_base = base_sources
            .get(e.name.as_str())
            .is_some_and(|s| s.contains(&e.source));
        if !same_source_on_base {
            let finding = match &e.source {
                Source::Direct(h) => Some(format!(
                    "`{}` {} is fetched directly from `{h}` (git or a bare URL), not from a registry",
                    e.name, e.version
                )),
                Source::Registry(h) if !known_hosts.contains(h) => Some(format!(
                    "`{}` {} resolves from `{h}`, a registry host the base lockfile does not use",
                    e.name, e.version
                )),
                _ => None,
            };
            if let Some(message) = finding {
                if seen.insert(("source", e.name.clone())) {
                    found.push(LockFinding {
                        kind: &crate::findings::LOCKFILE_ENTRY_FROM_NEW_SOURCE,
                        package: e.name.clone(),
                        message,
                    });
                }
            }
        }
        if !e.has_hash
            && base_hashed.contains(&(e.name.as_str(), e.version.as_str()))
            && seen.insert(("hash", e.name.clone()))
        {
            found.push(LockFinding {
                kind: &crate::findings::LOCKFILE_INTEGRITY_HASH_REMOVED,
                package: e.name.clone(),
                message: format!(
                    "`{}` {} carried an integrity hash on the base side and no longer does",
                    e.name, e.version
                ),
            });
        }
    }
    found
}

/// Lockfile names that pin a manifest of the given file name, most specific first.
pub fn lockfiles_for(manifest_file_name: &str) -> &'static [&'static str] {
    match manifest_file_name {
        "Cargo.toml" => &["Cargo.lock"],
        "package.json" => &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"],
        "pyproject.toml" => &["poetry.lock", "uv.lock"],
        "composer.json" => &["composer.lock"],
        "Gemfile" => &["Gemfile.lock"],
        // `go.sum` is left out: requiring a module that is already an indirect
        // dependency changes `go.mod` and leaves `go.sum` untouched.
        _ => &[],
    }
}

/// The tracked lockfile governing `manifest_path`: in its directory, else the nearest
/// ancestor's (a workspace keeps one lockfile at its root).
pub fn governing_lockfile(manifest_path: &str, tracked: &BTreeSet<String>) -> Option<String> {
    let (mut dir, file) = manifest_path
        .rsplit_once('/')
        .map_or(("", manifest_path), |(d, f)| (d, f));
    let names = lockfiles_for(file);
    loop {
        for n in names {
            let candidate = if dir.is_empty() {
                n.to_string()
            } else {
                format!("{dir}/{n}")
            };
            if tracked.contains(&candidate) {
                return Some(candidate);
            }
        }
        if dir.is_empty() {
            return None;
        }
        dir = dir.rsplit_once('/').map_or("", |(d, _)| d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO_BASE: &str = r#"version = 3
[[package]]
name = "app"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa"

[[package]]
name = "pinned"
version = "0.2.0"
source = "git+https://git.example.com/pinned?rev=abc#abc"
"#;

    fn titles(file: &str, base: &str, head: &str) -> Vec<(&'static str, String)> {
        let b = parse_lock(file, base).unwrap();
        let h = parse_lock(file, head).unwrap();
        diff_lock(&b, &h)
            .into_iter()
            .map(|f| (f.kind.title, f.package))
            .collect()
    }

    #[test]
    fn cargo_lock_new_registry_package_and_existing_git_package_are_silent() {
        let head = format!(
            "{CARGO_BASE}\n[[package]]\nname = \"anyhow\"\nversion = \"1.0.0\"\n\
             source = \"sparse+https://index.crates.io/\"\nchecksum = \"bb\"\n"
        );
        assert!(titles("Cargo.lock", CARGO_BASE, &head).is_empty());
        assert!(titles("Cargo.lock", CARGO_BASE, CARGO_BASE).is_empty());
    }

    #[test]
    fn cargo_lock_source_swap_new_host_and_dropped_checksum_are_reported() {
        let swapped = CARGO_BASE.replace(
            "source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"aa\"",
            "source = \"git+https://github.com/someone/serde#def\"",
        );
        assert_eq!(
            titles("Cargo.lock", CARGO_BASE, &swapped),
            vec![
                ("Lockfile Entry From New Source", "serde".to_string()),
                ("Lockfile Integrity Hash Removed", "serde".to_string()),
            ]
        );
        let mirror = format!(
            "{CARGO_BASE}\n[[package]]\nname = \"left-pad\"\nversion = \"1.0.0\"\n\
             source = \"registry+https://crates.mirror.example/index\"\nchecksum = \"cc\"\n"
        );
        assert_eq!(
            titles("Cargo.lock", CARGO_BASE, &mirror),
            vec![("Lockfile Entry From New Source", "left-pad".to_string())]
        );
        let no_sum = CARGO_BASE.replace("checksum = \"aa\"\n", "");
        assert_eq!(
            titles("Cargo.lock", CARGO_BASE, &no_sum),
            vec![("Lockfile Integrity Hash Removed", "serde".to_string())]
        );
    }

    #[test]
    fn package_lock_private_registry_on_base_is_known_and_a_tarball_url_is_not() {
        let pkg = |name: &str, resolved: &str, integrity: bool| {
            let integ = if integrity {
                r#", "integrity": "sha512-x""#
            } else {
                ""
            };
            format!(
                r#""node_modules/{name}": {{"version": "1.0.0", "resolved": "{resolved}"{integ}}}"#
            )
        };
        let lock = |entries: &[String]| {
            format!(
                r#"{{"lockfileVersion": 3, "packages": {{"": {{"name": "app"}}, {}}}}}"#,
                entries.join(", ")
            )
        };
        let base = lock(&[
            pkg(
                "lodash",
                "https://registry.npmjs.org/lodash/-/lodash-1.0.0.tgz",
                true,
            ),
            pkg(
                "@corp/ui",
                "https://npm.corp.example/@corp/ui/-/ui-1.0.0.tgz",
                true,
            ),
        ]);
        let fine = lock(&[
            pkg(
                "lodash",
                "https://registry.npmjs.org/lodash/-/lodash-1.0.0.tgz",
                true,
            ),
            pkg(
                "@corp/ui",
                "https://npm.corp.example/@corp/ui/-/ui-1.0.0.tgz",
                true,
            ),
            pkg(
                "@corp/api",
                "https://npm.corp.example/@corp/api/-/api-1.0.0.tgz",
                true,
            ),
        ]);
        assert!(titles("package-lock.json", &base, &fine).is_empty());
        let bad = lock(&[
            pkg("lodash", "https://evil.example/lodash.tgz", false),
            pkg(
                "@corp/ui",
                "https://npm.corp.example/@corp/ui/-/ui-1.0.0.tgz",
                true,
            ),
        ]);
        assert_eq!(
            titles("package-lock.json", &base, &bad),
            vec![
                ("Lockfile Entry From New Source", "lodash".to_string()),
                ("Lockfile Integrity Hash Removed", "lodash".to_string()),
            ]
        );
    }

    #[test]
    fn yarn_v1_is_read_and_berry_and_unknown_formats_are_not() {
        let base = "# yarn lockfile v1\n\n\"@babel/core@^7.0.0\":\n  version \"7.1.0\"\n  \
            resolved \"https://registry.yarnpkg.com/@babel/core/-/core-7.1.0.tgz#abc\"\n  integrity sha512-x\n";
        let head = base.replace(
            "https://registry.yarnpkg.com/@babel/core/-/core-7.1.0.tgz#abc",
            "git+https://github.com/someone/core.git#abc",
        );
        assert_eq!(
            titles("yarn.lock", base, &head),
            vec![("Lockfile Entry From New Source", "@babel/core".to_string())]
        );
        // Yarn 2+ is read by its own parser: a metadata-only file has no entries.
        assert_eq!(
            parse_lock("yarn.lock", "__metadata:\n  version: 8\n"),
            Some(vec![])
        );
        assert!(parse_lock("pnpm-lock.yaml", "lockfileVersion: '9.0'\n").is_none());
        assert!(parse_lock("Cargo.lock", "not toml [[").is_none());
    }

    #[test]
    fn a_manifest_is_governed_by_the_nearest_tracked_lockfile() {
        let tracked: BTreeSet<String> = [
            "Cargo.lock",
            "web/package-lock.json",
            "web/packages/a/package.json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            governing_lockfile("crates/core/Cargo.toml", &tracked).as_deref(),
            Some("Cargo.lock")
        );
        assert_eq!(
            governing_lockfile("web/packages/a/package.json", &tracked).as_deref(),
            Some("web/package-lock.json")
        );
        assert_eq!(governing_lockfile("tools/pyproject.toml", &tracked), None);
        assert_eq!(governing_lockfile("go.mod", &tracked), None);
    }

    #[test]
    fn six_more_formats_report_a_source_swap_and_a_dropped_hash() {
        // Each base pins a registry package with a hash; each head moves it to another
        // host and drops the hash. A format that did not parse would report nothing.
        let cases: [(&str, &str, &str); 6] = [
            (
                "pnpm-lock.yaml",
                "lockfileVersion: '9.0'\npackages:\n  left-pad@1.3.0:\n    resolution: {integrity: sha512-abc}\n",
                "lockfileVersion: '9.0'\npackages:\n  left-pad@1.3.0:\n    resolution: {tarball: https://evil.example/left-pad.tgz}\n",
            ),
            (
                "yarn.lock",
                "__metadata:\n  version: 8\n\n\"left-pad@npm:^1.3.0\":\n  version: 1.3.0\n  resolution: \"left-pad@npm:1.3.0\"\n  checksum: abc\n",
                "__metadata:\n  version: 8\n\n\"left-pad@https://evil.example/left-pad.tgz\":\n  version: 1.3.0\n  resolution: \"left-pad@https://evil.example/left-pad.tgz\"\n",
            ),
            (
                "poetry.lock",
                "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\nfiles = [{file = \"requests-2.31.0.tar.gz\", hash = \"sha256:abc\"}]\n",
                "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\nfiles = []\n\n[package.source]\ntype = \"url\"\nurl = \"https://evil.example/requests.tar.gz\"\n",
            ),
            (
                "uv.lock",
                "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\nsource = { registry = \"https://pypi.org/simple\" }\nsdist = { url = \"https://files.pythonhosted.org/r.tar.gz\", hash = \"sha256:abc\" }\n",
                "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\nsource = { url = \"https://evil.example/requests.tar.gz\" }\nsdist = { url = \"https://evil.example/requests.tar.gz\" }\n",
            ),
            (
                "composer.lock",
                "{\"packages\": [{\"name\": \"monolog/monolog\", \"version\": \"3.0.0\", \"dist\": {\"type\": \"zip\", \"url\": \"https://api.github.com/repos/Seldaek/monolog/zipball/abc\", \"shasum\": \"deadbeef\"}}]}",
                "{\"packages\": [{\"name\": \"monolog/monolog\", \"version\": \"3.0.0\", \"dist\": {\"type\": \"zip\", \"url\": \"https://evil.example/monolog.zip\", \"shasum\": \"\"}}]}",
            ),
            (
                "Gemfile.lock",
                "GEM\n  remote: https://rubygems.org/\n  specs:\n    rake (13.0.6)\n\nPLATFORMS\n  ruby\n\nCHECKSUMS\n  rake (13.0.6) sha256=abc\n",
                "GIT\n  remote: https://evil.example/rake.git\n  revision: abc\n  specs:\n    rake (13.0.6)\n\nPLATFORMS\n  ruby\n",
            ),
        ];
        for (file, base, head) in cases {
            let got = titles(file, base, head);
            let names: Vec<&str> = got.iter().map(|(t, _)| *t).collect();
            assert!(
                names.contains(&"Lockfile Entry From New Source"),
                "{file}: {got:?}"
            );
            assert!(
                names.contains(&"Lockfile Integrity Hash Removed"),
                "{file}: {got:?}"
            );
            // Unchanged content reports nothing.
            assert!(titles(file, base, base).is_empty(), "{file}");
        }
    }
}
