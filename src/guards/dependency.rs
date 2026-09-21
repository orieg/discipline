//! Universal manifest diff inspection and dependency delta sentinel.
//!
//! Enforces:
//! - Manifest diff inspection against merge-base ref
//! - Zero wildcard versions (`*`, `latest`, empty/unconstrained)
//! - Immutable commit/tag pins on git dependencies (no unpinned branches or floating refs)
//! - `deny.toml` verification (bans, allowlists, git source restrictions)
//! - Configured `allow_dependencies` and `deny_dependencies`
//! - Scoped `allow-dependency` escape hatches

use crate::config::{GateSettings, Severity};
use crate::guards::{lockfile, Context, GateOutcome};
use crate::tokens;
use anyhow::{Context as _, Result};
use globset::{Glob, GlobSetBuilder};
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::path::Path;
use toml::Value as TomlValue;

pub const GATE: &str = "dependency-delta";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyRecord {
    pub name: String,
    pub version: Option<String>,
    pub git_url: Option<String>,
    pub git_pin: Option<String>,
    pub is_wildcard: bool,
    pub is_git: bool,
    pub is_path: bool,
    pub manifest_path: String,
    pub line: Option<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct DenyPolicy {
    pub deny_bans: HashSet<String>,
    pub allow_bans: HashSet<String>,
    pub wildcards_denied: bool,
    pub allow_git: Vec<String>,
    pub deny_unknown_git: bool,
}

/// Parses a `deny.toml` string into a structured `DenyPolicy`.
pub fn parse_deny_toml(content: &str) -> Result<DenyPolicy> {
    let mut policy = DenyPolicy::default();
    let val: TomlValue = toml::from_str(content).context("failed to parse deny.toml")?;

    if let Some(bans) = val.get("bans").and_then(TomlValue::as_table) {
        if let Some(deny_list) = bans.get("deny").and_then(TomlValue::as_array) {
            for item in deny_list {
                if let Some(name) = item.as_str() {
                    policy.deny_bans.insert(name.to_string());
                } else if let Some(name) = item.get("name").and_then(TomlValue::as_str) {
                    policy.deny_bans.insert(name.to_string());
                }
            }
        }
        if let Some(allow_list) = bans.get("allow").and_then(TomlValue::as_array) {
            for item in allow_list {
                if let Some(name) = item.as_str() {
                    policy.allow_bans.insert(name.to_string());
                } else if let Some(name) = item.get("name").and_then(TomlValue::as_str) {
                    policy.allow_bans.insert(name.to_string());
                }
            }
        }
        if let Some(wild) = bans.get("wildcards").and_then(TomlValue::as_str) {
            if wild == "deny" {
                policy.wildcards_denied = true;
            }
        }
    }

    if let Some(sources) = val.get("sources").and_then(TomlValue::as_table) {
        if let Some(unk_git) = sources.get("unknown-git").and_then(TomlValue::as_str) {
            if unk_git == "deny" {
                policy.deny_unknown_git = true;
            }
        }
        if let Some(allow_git) = sources.get("allow-git").and_then(TomlValue::as_array) {
            for item in allow_git {
                if let Some(url) = item.as_str() {
                    policy.allow_git.push(url.to_string());
                }
            }
        }
    }

    Ok(policy)
}

fn is_wildcard_str(v: &str) -> bool {
    let trimmed = v.trim();
    trimmed == "*"
        || trimmed == "latest"
        || trimmed.is_empty()
        || trimmed == "x"
        || trimmed == "X"
        || trimmed == "^*"
        || trimmed == "~*"
}

/// Parses dependencies from a `Cargo.toml` file content.
pub fn parse_cargo_toml(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let val: TomlValue = match toml::from_str(content) {
        Ok(v) => v,
        Err(_) => return records,
    };

    let Some(root) = val.as_table() else {
        return records;
    };

    let tables_to_check = vec!["dependencies", "dev-dependencies", "build-dependencies"];

    // Check target tables: target.<target>.dependencies
    if let Some(target) = root.get("target").and_then(TomlValue::as_table) {
        for (_target_name, target_val) in target {
            if let Some(target_tbl) = target_val.as_table() {
                for tbl_name in &tables_to_check {
                    if let Some(deps) = target_tbl.get(*tbl_name).and_then(TomlValue::as_table) {
                        extract_cargo_deps(deps, path, &mut records);
                    }
                }
            }
        }
    }

    // Check workspace.dependencies
    if let Some(ws) = root.get("workspace").and_then(TomlValue::as_table) {
        for tbl_name in &tables_to_check {
            if let Some(deps) = ws.get(*tbl_name).and_then(TomlValue::as_table) {
                extract_cargo_deps(deps, path, &mut records);
            }
        }
    }

    // Top-level dependency tables
    for tbl_name in tables_to_check {
        if let Some(deps) = root.get(tbl_name).and_then(TomlValue::as_table) {
            extract_cargo_deps(deps, path, &mut records);
        }
    }

    records
}

fn extract_cargo_deps(
    deps: &toml::map::Map<String, TomlValue>,
    path: &str,
    records: &mut Vec<DependencyRecord>,
) {
    for (name, dep_val) in deps {
        match dep_val {
            TomlValue::String(ver) => {
                records.push(DependencyRecord {
                    name: name.clone(),
                    version: Some(ver.clone()),
                    git_url: None,
                    git_pin: None,
                    is_wildcard: is_wildcard_str(ver),
                    is_git: false,
                    is_path: false,
                    manifest_path: path.to_string(),
                    line: None,
                });
            }
            TomlValue::Table(tbl) => {
                // If renamed: `my_alias = { package = "real_pkg", version = "..." }`
                let resolved_name = tbl
                    .get("package")
                    .and_then(TomlValue::as_str)
                    .unwrap_or(name);

                let ver = tbl
                    .get("version")
                    .and_then(TomlValue::as_str)
                    .map(str::to_string);
                let git_url = tbl
                    .get("git")
                    .and_then(TomlValue::as_str)
                    .map(str::to_string);
                let is_git = git_url.is_some();
                let is_path = tbl.contains_key("path");

                // Check immutable git pin (rev, tag, commit sha)
                let git_pin = tbl
                    .get("rev")
                    .or_else(|| tbl.get("tag"))
                    .and_then(TomlValue::as_str)
                    .map(str::to_string);

                let is_wildcard = match &ver {
                    Some(v) => is_wildcard_str(v),
                    None if !is_git && !is_path && !tbl.contains_key("workspace") => true,
                    _ => false,
                };

                records.push(DependencyRecord {
                    name: resolved_name.to_string(),
                    version: ver,
                    git_url,
                    git_pin,
                    is_wildcard,
                    is_git,
                    is_path,
                    manifest_path: path.to_string(),
                    line: None,
                });
            }
            _ => {}
        }
    }
}

/// Parses dependencies from a `package.json` file content.
pub fn parse_package_json(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let val: JsonValue = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return records,
    };

    let Some(root) = val.as_object() else {
        return records;
    };

    let tables = [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ];

    for tbl_name in tables {
        if let Some(deps) = root.get(tbl_name).and_then(JsonValue::as_object) {
            for (name, dep_val) in deps {
                if let Some(v_str) = dep_val.as_str() {
                    let is_git = v_str.starts_with("git+")
                        || v_str.starts_with("git://")
                        || v_str.starts_with("github:");
                    let is_wildcard = is_wildcard_str(v_str);

                    let (git_url, git_pin) = if is_git {
                        if let Some((url, pin)) = v_str.split_once('#') {
                            let pin_trimmed = pin.trim();
                            let is_pinned = !pin_trimmed.is_empty()
                                && pin_trimmed != "main"
                                && pin_trimmed != "master"
                                && pin_trimmed != "head"
                                && pin_trimmed != "HEAD";
                            (
                                Some(url.to_string()),
                                if is_pinned {
                                    Some(pin_trimmed.to_string())
                                } else {
                                    None
                                },
                            )
                        } else {
                            (Some(v_str.to_string()), None)
                        }
                    } else {
                        (None, None)
                    };

                    records.push(DependencyRecord {
                        name: name.clone(),
                        version: Some(v_str.to_string()),
                        git_url,
                        git_pin,
                        is_wildcard,
                        is_git,
                        is_path: v_str.starts_with("file:") || v_str.starts_with("link:"),
                        manifest_path: path.to_string(),
                        line: None,
                    });
                }
            }
        }
    }

    records
}

/// Parses dependencies from a `pyproject.toml` file content.
pub fn parse_pyproject_toml(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let val: TomlValue = match toml::from_str(content) {
        Ok(v) => v,
        Err(_) => return records,
    };

    let Some(root) = val.as_table() else {
        return records;
    };

    // 1. PEP 621: project.dependencies
    if let Some(project) = root.get("project").and_then(TomlValue::as_table) {
        if let Some(deps) = project.get("dependencies").and_then(TomlValue::as_array) {
            for dep in deps {
                if let Some(s) = dep.as_str() {
                    parse_pep508_string(s, path, &mut records);
                }
            }
        }
        if let Some(opt) = project
            .get("optional-dependencies")
            .and_then(TomlValue::as_table)
        {
            for (_group, deps) in opt {
                if let Some(arr) = deps.as_array() {
                    for dep in arr {
                        if let Some(s) = dep.as_str() {
                            parse_pep508_string(s, path, &mut records);
                        }
                    }
                }
            }
        }
    }

    // 2. Poetry: tool.poetry.dependencies, dev-dependencies, group.<grp>.dependencies
    if let Some(tool) = root.get("tool").and_then(TomlValue::as_table) {
        if let Some(poetry) = tool.get("poetry").and_then(TomlValue::as_table) {
            if let Some(deps) = poetry.get("dependencies").and_then(TomlValue::as_table) {
                extract_poetry_deps(deps, path, &mut records);
            }
            if let Some(dev_deps) = poetry.get("dev-dependencies").and_then(TomlValue::as_table) {
                extract_poetry_deps(dev_deps, path, &mut records);
            }
            if let Some(groups) = poetry.get("group").and_then(TomlValue::as_table) {
                for (_grp_name, grp_val) in groups {
                    if let Some(grp_tbl) = grp_val.as_table() {
                        if let Some(deps) =
                            grp_tbl.get("dependencies").and_then(TomlValue::as_table)
                        {
                            extract_poetry_deps(deps, path, &mut records);
                        }
                    }
                }
            }
        }
    }

    records
}

fn extract_poetry_deps(
    deps: &toml::map::Map<String, TomlValue>,
    path: &str,
    records: &mut Vec<DependencyRecord>,
) {
    for (name, val) in deps {
        if name == "python" {
            continue;
        }
        match val {
            TomlValue::String(ver) => {
                records.push(DependencyRecord {
                    name: name.clone(),
                    version: Some(ver.clone()),
                    git_url: None,
                    git_pin: None,
                    is_wildcard: is_wildcard_str(ver),
                    is_git: false,
                    is_path: false,
                    manifest_path: path.to_string(),
                    line: None,
                });
            }
            TomlValue::Table(tbl) => {
                let ver = tbl
                    .get("version")
                    .and_then(TomlValue::as_str)
                    .map(str::to_string);
                let git_url = tbl
                    .get("git")
                    .and_then(TomlValue::as_str)
                    .map(str::to_string);
                let git_pin = tbl
                    .get("rev")
                    .or_else(|| tbl.get("tag"))
                    .and_then(TomlValue::as_str)
                    .map(str::to_string);
                let is_git = git_url.is_some();
                let is_wildcard = match &ver {
                    Some(v) => is_wildcard_str(v),
                    None if !is_git => true,
                    _ => false,
                };
                records.push(DependencyRecord {
                    name: name.clone(),
                    version: ver,
                    git_url,
                    git_pin,
                    is_wildcard,
                    is_git,
                    is_path: tbl.contains_key("path"),
                    manifest_path: path.to_string(),
                    line: None,
                });
            }
            _ => {}
        }
    }
}

fn parse_pep508_string(s: &str, path: &str, records: &mut Vec<DependencyRecord>) {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return;
    }
    // Take part before ';' (environment markers)
    let spec = trimmed.split(';').next().unwrap_or(trimmed).trim();

    // Look for version operators: ==, >=, <=, ~=, !=, >, <, @
    let ops = ["==", ">=", "<=", "~=", "!=", ">", "<", "@"];
    let mut found_op = None;
    for op in ops {
        if let Some(idx) = spec.find(op) {
            found_op = Some((idx, op));
            break;
        }
    }

    if let Some((idx, op)) = found_op {
        let name = spec[..idx].trim().to_string();
        let ver_part = spec[idx + op.len()..].trim();
        let is_git = ver_part.starts_with("git+") || ver_part.starts_with("git://");
        let (git_url, git_pin) = if is_git {
            if let Some((url, pin)) = ver_part.split_once('@') {
                (Some(url.to_string()), Some(pin.to_string()))
            } else {
                (Some(ver_part.to_string()), None)
            }
        } else {
            (None, None)
        };
        let is_wildcard = is_wildcard_str(ver_part) || ver_part == "*";
        records.push(DependencyRecord {
            name,
            version: Some(ver_part.to_string()),
            git_url,
            git_pin,
            is_wildcard,
            is_git,
            is_path: false,
            manifest_path: path.to_string(),
            line: None,
        });
    } else {
        // No version constraint specified -> unconstrained / wildcard!
        let name = spec
            .split_whitespace()
            .next()
            .unwrap_or(spec)
            .trim()
            .to_string();
        records.push(DependencyRecord {
            name,
            version: None,
            git_url: None,
            git_pin: None,
            is_wildcard: true,
            is_git: false,
            is_path: false,
            manifest_path: path.to_string(),
            line: None,
        });
    }
}

/// Parses dependencies from `requirements.txt`.
pub fn parse_requirements_txt(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with('-')
            || trimmed.starts_with("--")
        {
            continue;
        }
        parse_pep508_string(trimmed, path, &mut records);
    }
    records
}

/// Parses dependencies from `go.mod`.
pub fn parse_go_mod(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let mut in_require_block = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "require (" {
            in_require_block = true;
            continue;
        }
        if in_require_block && trimmed == ")" {
            in_require_block = false;
            continue;
        }

        let line_content = if trimmed.starts_with("require ") {
            trimmed.trim_start_matches("require ").trim()
        } else if in_require_block {
            trimmed
        } else {
            continue;
        };

        let parts: Vec<&str> = line_content.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let name = parts[0].to_string();
        let version = if parts.len() > 1 {
            Some(parts[1].to_string())
        } else {
            None
        };
        let is_wildcard = match &version {
            Some(v) => is_wildcard_str(v),
            None => true,
        };

        records.push(DependencyRecord {
            name,
            version,
            git_url: None,
            git_pin: None,
            is_wildcard,
            is_git: false,
            is_path: false,
            manifest_path: path.to_string(),
            line: None,
        });
    }

    records
}

/// Parses dependencies from `composer.json`.
pub fn parse_composer_json(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let val: JsonValue = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return records,
    };

    let Some(root) = val.as_object() else {
        return records;
    };

    for tbl_name in ["require", "require-dev"] {
        if let Some(deps) = root.get(tbl_name).and_then(JsonValue::as_object) {
            for (name, dep_val) in deps {
                if name == "php" {
                    continue;
                }
                if let Some(v_str) = dep_val.as_str() {
                    let is_wildcard = is_wildcard_str(v_str) || v_str == "dev-master";
                    records.push(DependencyRecord {
                        name: name.clone(),
                        version: Some(v_str.to_string()),
                        git_url: None,
                        git_pin: None,
                        is_wildcard,
                        is_git: false,
                        is_path: false,
                        manifest_path: path.to_string(),
                        line: None,
                    });
                }
            }
        }
    }

    records
}

/// Parses dependencies from `Gemfile`.
pub fn parse_gemfile(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let gem_re = regex::Regex::new(r#"^\s*gem\s+['"]([^'"]+)['"](?:\s*,\s*['"]([^'"]+)['"])?"#)
        .expect("static regex");
    let git_re = regex::Regex::new(r#"git:\s*['"]([^'"]+)['"]"#).expect("static regex");
    let ref_re = regex::Regex::new(r#"(?:ref|tag):\s*['"]([^'"]+)['"]"#).expect("static regex");

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(caps) = gem_re.captures(trimmed) {
            let name = caps.get(1).map(|m| m.as_str().to_string()).unwrap();
            let version = caps.get(2).map(|m| m.as_str().to_string());
            let git_url = git_re
                .captures(trimmed)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            let git_pin = ref_re
                .captures(trimmed)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            let is_git = git_url.is_some();
            let is_wildcard = match &version {
                Some(v) => is_wildcard_str(v),
                None if !is_git => true,
                _ => false,
            };

            records.push(DependencyRecord {
                name,
                version,
                git_url,
                git_pin,
                is_wildcard,
                is_git,
                is_path: false,
                manifest_path: path.to_string(),
                line: None,
            });
        }
    }

    records
}

/// Parses dependencies from `*.csproj` or `Directory.Packages.props`.
pub fn parse_csproj(content: &str, path: &str) -> Vec<DependencyRecord> {
    let mut records = Vec::new();
    let pkg_re = regex::Regex::new(r#"<PackageReference\s+[^>]*Include=["']([^"']+)["'][^>]*/>"#)
        .expect("static regex");
    let ver_re = regex::Regex::new(r#"Version=["']([^"']+)["']"#).expect("static regex");

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<!--") {
            continue;
        }
        if let Some(caps) = pkg_re.captures(trimmed) {
            let name = caps.get(1).map(|m| m.as_str().to_string()).unwrap();
            let version = ver_re
                .captures(trimmed)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            let is_wildcard = match &version {
                Some(v) => is_wildcard_str(v),
                None => true,
            };

            records.push(DependencyRecord {
                name,
                version,
                git_url: None,
                git_pin: None,
                is_wildcard,
                is_git: false,
                is_path: false,
                manifest_path: path.to_string(),
                line: None,
            });
        }
    }

    records
}

/// Parses any supported manifest based on filename.
pub fn parse_manifest(content: &str, path: &str) -> Vec<DependencyRecord> {
    let p = Path::new(path);
    let file_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");

    if file_name == "Cargo.toml" {
        parse_cargo_toml(content, path)
    } else if file_name == "package.json" {
        parse_package_json(content, path)
    } else if file_name == "pyproject.toml" {
        parse_pyproject_toml(content, path)
    } else if file_name.starts_with("requirements") && file_name.ends_with(".txt") {
        parse_requirements_txt(content, path)
    } else if file_name == "go.mod" {
        parse_go_mod(content, path)
    } else if file_name == "composer.json" {
        parse_composer_json(content, path)
    } else if file_name == "Gemfile" {
        parse_gemfile(content, path)
    } else if file_name.ends_with(".csproj") || file_name == "Directory.Packages.props" {
        parse_csproj(content, path)
    } else {
        Vec::new()
    }
}

/// Record a lockfile finding unless an `allow-dependency:` override names `subject`.
fn lock_violation(
    ctx: &Context,
    outcome: &mut GateOutcome,
    severity: Severity,
    title: &str,
    file: &str,
    subject: &str,
    message: String,
) {
    if let Some(rec) = ctx.find_override(GATE, tokens::ALLOW_DEPENDENCY, subject) {
        outcome.overrides.push(rec);
        return;
    }
    outcome.push(
        ctx.overridable(severity),
        title,
        Some(file),
        None,
        message,
        &format!(
            "Regenerate the lockfile from the manifest, or justify it: `allow-dependency: {subject} <reason>`."
        ),
    );
}

fn count_lockfile_entries(content: &str, file_name: &str) -> usize {
    if file_name == "Cargo.lock" || file_name == "poetry.lock" {
        content
            .lines()
            .filter(|l| l.trim() == "[[package]]")
            .count()
    } else if file_name == "package-lock.json" {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(pkgs) = val.get("packages").and_then(|p| p.as_object()) {
                pkgs.len()
            } else if let Some(deps) = val.get("dependencies").and_then(|d| d.as_object()) {
                deps.len()
            } else {
                0
            }
        } else {
            0
        }
    } else if file_name == "go.sum" {
        let mut mods = HashSet::new();
        for line in content.lines() {
            if let Some(first) = line.split_whitespace().next() {
                mods.insert(first);
            }
        }
        mods.len()
    } else if file_name == "yarn.lock" {
        content
            .lines()
            .filter(|l| !l.starts_with(' ') && !l.starts_with('#') && l.ends_with(':'))
            .count()
    } else if file_name == "pnpm-lock.yaml" {
        content
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                (t.starts_with('\'') || t.starts_with('/')) && t.ends_with(':')
            })
            .count()
    } else {
        0
    }
}

fn is_constraint_loosened(base_ver: Option<&str>, head_ver: Option<&str>) -> bool {
    let (Some(bv), Some(hv)) = (base_ver, head_ver) else {
        return false;
    };
    let bv = bv.trim();
    let hv = hv.trim();
    if bv == hv {
        return false;
    }
    // Wildcard head is loosened if base was not wildcard
    if is_wildcard_str(hv) && !is_wildcard_str(bv) {
        return true;
    }
    // Exact pinned base changed to prefix / range
    let is_exact = bv.starts_with('=')
        || bv.starts_with("==")
        || (!bv.starts_with('^')
            && !bv.starts_with('~')
            && !bv.starts_with('>')
            && !bv.starts_with('<')
            && !bv.starts_with('*'));
    let is_range = hv.starts_with('^')
        || hv.starts_with('~')
        || hv.starts_with('>')
        || hv.starts_with('<')
        || hv.starts_with('*')
        || hv.contains("||");
    if is_exact && is_range {
        return true;
    }
    // Tilde ~1.2.3 loosened to caret ^1.2.3 or >=
    if bv.starts_with('~') && (hv.starts_with('^') || hv.starts_with(">=") || hv.starts_with('>')) {
        return true;
    }
    // Pinned =1.2 loosened to 1.2
    if bv.starts_with('=') && !hv.starts_with('=') {
        return true;
    }
    false
}

/// Evaluates the `dependency-delta` gate.
pub fn evaluate_dependency_delta(ctx: &Context) -> Result<GateOutcome> {
    let mut outcome = GateOutcome::new(GATE);
    let gate = &ctx.config.gates.dependency_delta;

    if !gate.enabled() {
        outcome.enabled = false;
        return Ok(outcome);
    }

    // Build manifest matcher
    let mut builder = GlobSetBuilder::new();
    for pattern in &gate.manifests {
        builder.add(Glob::new(pattern)?);
        if let Some(stripped) = pattern.strip_prefix("**/") {
            builder.add(Glob::new(stripped)?);
        }
    }
    let manifest_matcher = builder.build()?;

    // Build exemptions matcher
    let mut ex_builder = GlobSetBuilder::new();
    for pattern in &gate.exempt_paths {
        ex_builder.add(Glob::new(pattern)?);
    }
    let ex_matcher = ex_builder.build()?;

    // Load deny.toml policy if configured or auto-detected
    let deny_policy = if let Some(ref deny_path) = gate.deny_file {
        if let Ok(Some(content)) = ctx.git.head_content(deny_path) {
            parse_deny_toml(&content).unwrap_or_default()
        } else {
            DenyPolicy::default()
        }
    } else {
        DenyPolicy::default()
    };

    let changed = ctx.git.changed_files()?;
    let mut manifest_files = Vec::new();
    let lockfile_names = [
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "poetry.lock",
        "go.sum",
    ];
    let mut lock_files = Vec::new();

    for f in &changed {
        if ex_matcher.is_match(&f.path) {
            continue;
        }
        let p = Path::new(&f.path);
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if lockfile_names.contains(&fname) {
            lock_files.push((f, fname));
        }
        if manifest_matcher.is_match(&f.path) {
            manifest_files.push(f);
        }
    }

    // Lockfiles: growth is a note; a deleted lockfile, an entry from a new source, and a
    // dropped integrity hash are findings (`lockfile.rs`).
    for (f, fname) in &lock_files {
        outcome.examined += 1;
        if f.kind == crate::gitctx::ChangeKind::Deleted {
            lock_violation(
                ctx,
                &mut outcome,
                gate.severity(),
                "Lockfile Deleted",
                &f.path,
                &f.path,
                format!(
                    "Lockfile `{}` was deleted; dependency versions are no longer pinned.",
                    f.path
                ),
            );
            continue;
        }
        let head_raw = ctx.git.head_content(&f.path)?;
        let base_raw = ctx.git.base_content(&f.old_path)?;
        let head_count = head_raw
            .as_deref()
            .map(|c| count_lockfile_entries(c, fname))
            .unwrap_or(0);
        let base_count = base_raw
            .as_deref()
            .map(|c| count_lockfile_entries(c, fname))
            .unwrap_or(0);
        let diff = head_count as i64 - base_count as i64;
        outcome.notes.push(format!(
            "lockfile `{}` package count: base {}, head {} ({:+})",
            f.path, base_count, head_count, diff
        ));

        let Some(head_entries) = head_raw
            .as_deref()
            .and_then(|c| lockfile::parse_lock(fname, c))
        else {
            outcome.notes.push(format!(
                "lockfile `{}`: entry sources and integrity hashes not analysed (format not read, or it does not parse)",
                f.path
            ));
            continue;
        };
        // No base side (a new lockfile): every entry is judged against the default hosts.
        let base_entries = base_raw
            .as_deref()
            .and_then(|c| lockfile::parse_lock(fname, c))
            .unwrap_or_default();
        for finding in lockfile::diff_lock(&base_entries, &head_entries) {
            lock_violation(
                ctx,
                &mut outcome,
                gate.severity(),
                finding.title,
                &f.path,
                &finding.package,
                format!("{} in `{}`.", finding.message, f.path),
            );
        }
    }

    if manifest_files.is_empty() && lock_files.is_empty() {
        outcome
            .notes
            .push("no dependency manifests or lockfiles modified in this diff".to_string());
        return Ok(outcome);
    }

    let tracked: std::collections::BTreeSet<String> =
        ctx.git.tracked_files()?.into_iter().collect();
    for f in manifest_files {
        let head_raw = ctx.git.head_content(&f.path)?;
        let Some(head_content) = head_raw else {
            continue;
        };

        let base_raw = ctx.git.base_content(&f.old_path)?;
        let head_deps = parse_manifest(&head_content, &f.path);
        let base_deps = match base_raw {
            Some(ref b) => parse_manifest(b, &f.old_path),
            None => Vec::new(),
        };

        // A manifest whose dependency set changed while its tracked lockfile did not.
        let dep_key = |d: &DependencyRecord| {
            (
                d.name.clone(),
                d.version.clone(),
                d.git_url.clone(),
                d.git_pin.clone(),
                d.is_path,
            )
        };
        let base_set: std::collections::BTreeSet<_> = base_deps.iter().map(dep_key).collect();
        let head_set: std::collections::BTreeSet<_> = head_deps.iter().map(dep_key).collect();
        if base_raw.is_some() && base_set != head_set {
            if let Some(lock) = lockfile::governing_lockfile(&f.path, &tracked) {
                if !changed.iter().any(|c| c.path == lock) {
                    lock_violation(
                        ctx,
                        &mut outcome,
                        gate.severity(),
                        "Manifest Changed Without Lockfile",
                        &f.path,
                        &lock,
                        format!(
                            "Dependencies in `{}` changed and the lockfile that pins them, `{lock}`, did not.",
                            f.path
                        ),
                    );
                }
            }
        }

        // Find deltas: new dependencies or modified existing dependencies
        for h in &head_deps {
            let matching_base = base_deps.iter().find(|b| b.name == h.name);
            let is_delta = match matching_base {
                None => true, // newly added dependency
                Some(b) => {
                    b.version != h.version
                        || b.git_url != h.git_url
                        || b.git_pin != h.git_pin
                        || b.is_wildcard != h.is_wildcard
                        || b.is_path != h.is_path
                        || b.is_git != h.is_git
                }
            };

            if !is_delta {
                continue;
            }

            let mut dep_violations = Vec::new();

            // 0. Core rule: New direct dependency check
            if matching_base.is_none() {
                let allowed = gate.allow_dependencies.contains(&h.name)
                    || deny_policy.allow_bans.contains(&h.name);
                if !allowed {
                    dep_violations.push((
                        "New Direct Dependency Added",
                        format!(
                            "Direct dependency `{}` was newly added to `{}`.",
                            h.name, f.path
                        ),
                        "Obtain approval and add to allow_dependencies in discipline.toml or provide `allow-dependency: <name> <reason>` in PR body or commit messages.",
                    ));
                }
            } else if let Some(b) = matching_base {
                // Check loosened version constraint
                if is_constraint_loosened(b.version.as_deref(), h.version.as_deref()) {
                    dep_violations.push((
                        "Loosened Dependency Constraint",
                        format!(
                            "Dependency `{}` in `{}` loosened version constraint from `{}` to `{}`.",
                            h.name,
                            f.path,
                            b.version.as_deref().unwrap_or(""),
                            h.version.as_deref().unwrap_or("")
                        ),
                        "Retain the pinned version constraint or document the relaxation with `allow-dependency: <name> <reason>`.",
                    ));
                }
                // Check source shift (registry -> git/path, git -> registry/path, etc.)
                let source_shifted = (b.is_git != h.is_git)
                    || (b.is_path != h.is_path)
                    || (b.is_git && h.is_git && b.git_url != h.git_url);
                if source_shifted {
                    dep_violations.push((
                        "Dependency Source Modified",
                        format!(
                            "Dependency `{}` in `{}` modified its source specification (git: {} -> {}, path: {} -> {}).",
                            h.name, f.path, b.is_git, h.is_git, b.is_path, h.is_path
                        ),
                        "Obtain approval for dependency source shift or provide `allow-dependency: <name> <reason>`.",
                    ));
                }
            }

            // 1. Wildcard check
            let enforce_wildcards = !gate.allow_wildcards || deny_policy.wildcards_denied;
            if enforce_wildcards && h.is_wildcard {
                dep_violations.push((
                    "Wildcard Dependency Version",
                    format!(
                        "Dependency `{}` in `{}` specifies a wildcard or unconstrained version `{}`.",
                        h.name,
                        f.path,
                        h.version.as_deref().unwrap_or("*")
                    ),
                    "Pin an explicit version or non-wildcard version range.",
                ));
            }

            // 2. Git pin check
            if gate.require_git_pins && h.is_git && h.git_pin.is_none() {
                dep_violations.push((
                    "Unpinned Git Dependency",
                    format!(
                        "Git dependency `{}` in `{}` does not specify an immutable commit or tag pin.",
                        h.name, f.path
                    ),
                    "Pin an immutable commit SHA (`rev = \"...\"`) or release tag.",
                ));
            }

            // 3. Deny / Banned dependency checks
            if gate.deny_dependencies.contains(&h.name) || deny_policy.deny_bans.contains(&h.name) {
                dep_violations.push((
                    "Banned Dependency",
                    format!(
                        "Dependency `{}` in `{}` is banned by repository policy.",
                        h.name, f.path
                    ),
                    "Remove the banned dependency or replace it with an approved alternative.",
                ));
            }

            // 4. Allowlist checks
            if !gate.allow_dependencies.is_empty() && !gate.allow_dependencies.contains(&h.name) {
                dep_violations.push((
                    "Dependency Outside Allowlist",
                    format!(
                        "Dependency `{}` in `{}` is not in the allowed dependencies list.",
                        h.name, f.path
                    ),
                    "Add the dependency to allow_dependencies with rationale or obtain approval.",
                ));
            }
            if !deny_policy.allow_bans.is_empty() && !deny_policy.allow_bans.contains(&h.name) {
                dep_violations.push((
                    "Dependency Outside Allowlist",
                    format!(
                        "Dependency `{}` in `{}` is not in deny.toml allow list.",
                        h.name, f.path
                    ),
                    "Verify licensing and supply chain safety before adding to deny.toml allow list.",
                ));
            }

            // 5. Git source restriction
            if deny_policy.deny_unknown_git && h.is_git {
                if let Some(ref url) = h.git_url {
                    let allowed = deny_policy.allow_git.iter().any(|a| url.starts_with(a));
                    if !allowed {
                        dep_violations.push((
                            "Unauthorized Git Repository Source",
                            format!(
                                "Git dependency `{}` from source `{}` is not in deny.toml allow-git sources.",
                                h.name, url
                            ),
                            "Add git repository URL to deny.toml [sources].allow-git or use an approved registry.",
                        ));
                    }
                }
            }

            // Check override directive covering this dependency
            if !dep_violations.is_empty() {
                if let Some(rec) = ctx.find_override(GATE, tokens::ALLOW_DEPENDENCY, &h.name) {
                    outcome.overrides.push(rec);
                } else {
                    for (title, msg, rem) in dep_violations {
                        outcome.push(
                            ctx.overridable(gate.severity()),
                            title,
                            Some(&f.path),
                            h.line,
                            msg,
                            rem,
                        );
                    }
                }
            }

            outcome.examined += 1;
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cargo_toml_dependencies() {
        let sample = r#"
[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0"
bad_wildcard = "*"
unpinned_git = { git = "https://github.com/foo/bar.git" }
pinned_git = { git = "https://github.com/foo/baz.git", rev = "1234567890abcdef" }
path_dep = { path = "../local" }

[dev-dependencies]
tokio = { version = "1.0" }
"#;
        let deps = parse_cargo_toml(sample, "Cargo.toml");
        assert_eq!(deps.len(), 6);

        let serde_dep = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde_dep.version.as_deref(), Some("1.0"));
        assert!(!serde_dep.is_wildcard);

        let wild_dep = deps.iter().find(|d| d.name == "bad_wildcard").unwrap();
        assert!(wild_dep.is_wildcard);

        let unpinned = deps.iter().find(|d| d.name == "unpinned_git").unwrap();
        assert!(unpinned.is_git);
        assert_eq!(unpinned.git_pin, None);

        let pinned = deps.iter().find(|d| d.name == "pinned_git").unwrap();
        assert!(pinned.is_git);
        assert_eq!(pinned.git_pin.as_deref(), Some("1234567890abcdef"));
    }

    #[test]
    fn test_parse_package_json_dependencies() {
        let sample = r#"{
  "dependencies": {
    "react": "^18.2.0",
    "lodash": "*",
    "unpinned-pkg": "git+https://github.com/org/pkg.git",
    "pinned-pkg": "git+https://github.com/org/pkg.git#v1.0.0"
  },
  "devDependencies": {
    "typescript": "latest"
  }
}"#;
        let deps = parse_package_json(sample, "package.json");
        assert_eq!(deps.len(), 5);

        let lodash = deps.iter().find(|d| d.name == "lodash").unwrap();
        assert!(lodash.is_wildcard);

        let ts = deps.iter().find(|d| d.name == "typescript").unwrap();
        assert!(ts.is_wildcard);

        let unpinned = deps.iter().find(|d| d.name == "unpinned-pkg").unwrap();
        assert!(unpinned.is_git);
        assert_eq!(unpinned.git_pin, None);

        let pinned = deps.iter().find(|d| d.name == "pinned-pkg").unwrap();
        assert!(pinned.is_git);
        assert_eq!(pinned.git_pin.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn test_parse_pyproject_toml_dependencies() {
        let sample = r#"
[project]
name = "py-app"
dependencies = [
    "requests>=2.28.0",
    "unconstrained-pkg",
    "wildcard-pkg==*"
]

[tool.poetry.dependencies]
python = "^3.11"
flask = "*"
"#;
        let deps = parse_pyproject_toml(sample, "pyproject.toml");
        assert_eq!(deps.len(), 4);

        let req = deps.iter().find(|d| d.name == "requests").unwrap();
        assert!(!req.is_wildcard);

        let unconstrained = deps.iter().find(|d| d.name == "unconstrained-pkg").unwrap();
        assert!(unconstrained.is_wildcard);

        let wild = deps.iter().find(|d| d.name == "wildcard-pkg").unwrap();
        assert!(wild.is_wildcard);

        let flask = deps.iter().find(|d| d.name == "flask").unwrap();
        assert!(flask.is_wildcard);
    }

    #[test]
    fn test_parse_go_mod_dependencies() {
        let sample = r#"module example.com/app

go 1.22

require (
    github.com/stretchr/testify v1.9.0
    github.com/gin-gonic/gin *
)
"#;
        let deps = parse_go_mod(sample, "go.mod");
        assert_eq!(deps.len(), 2);

        let gin = deps
            .iter()
            .find(|d| d.name == "github.com/gin-gonic/gin")
            .unwrap();
        assert!(gin.is_wildcard);
    }

    #[test]
    fn test_parse_deny_toml() {
        let sample = r#"
[bans]
deny = ["openssl", "ring"]
allow = ["tree-sitter"]
wildcards = "deny"

[sources]
unknown-git = "deny"
allow-git = ["https://github.com/orieg/"]
"#;
        let policy = parse_deny_toml(sample).unwrap();
        assert!(policy.deny_bans.contains("openssl"));
        assert!(policy.deny_bans.contains("ring"));
        assert!(policy.allow_bans.contains("tree-sitter"));
        assert!(policy.wildcards_denied);
        assert!(policy.deny_unknown_git);
        assert_eq!(policy.allow_git, vec!["https://github.com/orieg/"]);
    }
}
