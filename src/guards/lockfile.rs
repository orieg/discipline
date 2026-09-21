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
    pub title: &'static str,
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
    // Yarn 2+ ("berry") is YAML with `resolution:` / `checksum:`; not read here.
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

/// Entries of a lockfile, or `None` when the format is not one this module reads (or the
/// content does not parse as that format).
pub fn parse_lock(file_name: &str, content: &str) -> Option<Vec<LockEntry>> {
    match file_name {
        "Cargo.lock" => parse_cargo_lock(content),
        "package-lock.json" => parse_package_lock(content),
        "yarn.lock" => parse_yarn_lock(content),
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
                        title: "Lockfile Entry From New Source",
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
                title: "Lockfile Integrity Hash Dropped",
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
            .map(|f| (f.title, f.package))
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
                ("Lockfile Integrity Hash Dropped", "serde".to_string()),
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
            vec![("Lockfile Integrity Hash Dropped", "serde".to_string())]
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
                ("Lockfile Integrity Hash Dropped", "lodash".to_string()),
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
        assert!(parse_lock("yarn.lock", "__metadata:\n  version: 8\n").is_none());
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
}
