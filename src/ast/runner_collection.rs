//! Runner test collection rules for language packs.
//!
//! Determines whether a file is collected and executed by its framework's test runner:
//! - Python (pytest): `python_files`, `python_classes`, `python_functions`, `testpaths`
//!   from `pyproject.toml`, `pytest.ini`, `setup.cfg`, `tox.ini`.
//! - JS / TS (Jest / Vitest): `testMatch`, `testRegex`, `testPathIgnorePatterns`, `roots`,
//!   `include` from `package.json`, `jest.config.json`.
//! - Go: `_test.go` suffix, outside the directories and file names the go tool ignores.
//! - Rust: Cargo's target layout as the owning `Cargo.toml` declares it (`autotests`,
//!   `[lib]`, `[[test]]`), and the `mod` declarations under each test target. Without a
//!   parsed manifest: any `.rs` under a `src/` or `tests/` directory.
//! - Python with no pytest configuration, and every language with no runner model here
//!   (Java, Kotlin, C#, Scala, Swift, Objective-C, Ruby, PHP, C / C++): not determined.
//!   Only a parsed runner configuration excludes a file.
//! - Rust `#[cfg]` on a test: `cfg(feature = "x")` where `x` is not declared in `[features]`,
//!   or `cfg(any())` / `cfg(all(any()))`, treated as an unconditional ignore.

use std::collections::HashSet;
use tree_sitter::Node;

/// Matches pattern against text using globset with literal_separator disabled.
pub fn glob_match(pattern: &str, candidate: &str) -> bool {
    let pat = pattern.trim();
    if pat.is_empty() {
        return false;
    }
    if let Ok(glob) = globset::GlobBuilder::new(pat)
        .literal_separator(false)
        .build()
    {
        glob.compile_matcher().is_match(candidate)
    } else {
        candidate.contains(pat)
    }
}

/// Pytest test collection configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PytestCollectionRules {
    pub python_files: Vec<String>,
    pub python_classes: Vec<String>,
    pub python_functions: Vec<String>,
    pub testpaths: Vec<String>,
    /// A pytest configuration was found: a `[tool.pytest.ini_options]` table, a
    /// `pytest.ini`, or a `[pytest]` / `[tool:pytest]` section. Without one the runner
    /// may be unittest or Django, and pytest's defaults are not known to apply.
    pub configured: bool,
}

impl Default for PytestCollectionRules {
    fn default() -> Self {
        Self {
            python_files: vec!["test_*.py".to_string(), "*_test.py".to_string()],
            python_classes: vec!["Test*".to_string()],
            python_functions: vec!["test_*".to_string()],
            testpaths: Vec::new(),
            configured: false,
        }
    }
}

impl PytestCollectionRules {
    pub fn is_collected(&self, path: &str) -> bool {
        let norm = path.replace('\\', "/");
        if !norm.ends_with(".py") {
            return false;
        }

        // If testpaths is configured, path must fall under one of them
        if !self.testpaths.is_empty() {
            let in_testpath = self.testpaths.iter().any(|tp| testpath_holds(tp, &norm));
            if !in_testpath {
                return false;
            }
        }

        let filename = norm.rsplit('/').next().unwrap_or(&norm);
        let patterns = if self.python_files.is_empty() {
            vec!["test_*.py", "*_test.py"]
        } else {
            self.python_files.iter().map(String::as_str).collect()
        };

        patterns.iter().any(|pat| glob_match(pat, filename))
    }

    pub fn parse_pyproject_toml(content: &str) -> Self {
        let mut rules = Self::default();
        let Ok(val) = toml::from_str::<toml::Value>(content) else {
            return rules;
        };
        let Some(root) = val.as_table() else {
            return rules;
        };

        let ini_opts = root
            .get("tool")
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get("pytest"))
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get("ini_options"))
            .and_then(toml::Value::as_table);

        if let Some(tbl) = ini_opts {
            rules.configured = true;
            Self::extract_from_toml_table(tbl, &mut rules);
        }
        rules
    }

    fn extract_from_toml_table(tbl: &toml::map::Map<String, toml::Value>, rules: &mut Self) {
        if let Some(v) = tbl.get("python_files") {
            let files = toml_val_to_strings(v);
            if !files.is_empty() {
                rules.python_files = files;
            }
        }
        if let Some(v) = tbl.get("python_classes") {
            let classes = toml_val_to_strings(v);
            if !classes.is_empty() {
                rules.python_classes = classes;
            }
        }
        if let Some(v) = tbl.get("python_functions") {
            let funcs = toml_val_to_strings(v);
            if !funcs.is_empty() {
                rules.python_functions = funcs;
            }
        }
        if let Some(v) = tbl.get("testpaths") {
            let paths = toml_val_to_strings(v);
            if !paths.is_empty() {
                rules.testpaths = paths;
            }
        }
    }

    pub fn parse_ini(content: &str) -> Self {
        let mut rules = Self::default();
        let mut in_section = false;
        let mut current_key = String::new();

        for raw in content.lines() {
            let line = raw.trim_end();
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with([';', '#']) {
                continue;
            }
            if let Some(sect) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let s = sect.trim();
                in_section = s == "pytest" || s == "tool:pytest";
                rules.configured |= in_section;
                current_key.clear();
                continue;
            }
            if !in_section {
                continue;
            }

            let is_continuation = raw.starts_with([' ', '\t']);
            if is_continuation && !current_key.is_empty() {
                Self::append_ini_val(&current_key, trimmed, &mut rules);
                continue;
            }

            if let Some((k, v)) = line.split_once(['=', ':']) {
                let key = k.trim();
                current_key = key.to_string();
                Self::set_ini_val(key, v.trim(), &mut rules);
            }
        }
        rules
    }

    fn set_ini_val(key: &str, val: &str, rules: &mut Self) {
        let items: Vec<String> = val
            .split_whitespace()
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if items.is_empty() {
            return;
        }
        match key {
            "python_files" => rules.python_files = items,
            "python_classes" => rules.python_classes = items,
            "python_functions" => rules.python_functions = items,
            "testpaths" => rules.testpaths = items,
            _ => {}
        }
    }

    fn append_ini_val(key: &str, val: &str, rules: &mut Self) {
        let items: Vec<String> = val
            .split_whitespace()
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        match key {
            "python_files" => rules.python_files.extend(items),
            "python_classes" => rules.python_classes.extend(items),
            "python_functions" => rules.python_functions.extend(items),
            "testpaths" => rules.testpaths.extend(items),
            _ => {}
        }
    }
}

/// Whether a pytest `testpaths` entry holds `path`. An entry is a directory or a file
/// relative to the configuration file, written with or without `./`, or a glob in
/// Python's `glob` syntax: `*` stays inside one directory and `**` crosses them. An
/// entry that cannot be read as a repository path excludes nothing.
fn testpath_holds(testpath: &str, path: &str) -> bool {
    let Some(entry) = clean_relative(testpath) else {
        return true;
    };
    if entry.is_empty() {
        return true;
    }
    if !entry.contains(['*', '?', '[']) {
        return path == entry || path.starts_with(&format!("{entry}/"));
    }
    let Ok(glob) = globset::GlobBuilder::new(&entry)
        .literal_separator(true)
        .build()
    else {
        return true;
    };
    let matcher = glob.compile_matcher();
    // The glob names directories: it holds the path when it matches an ancestor.
    let mut end = path.len();
    loop {
        if matcher.is_match(&path[..end]) {
            return true;
        }
        match path[..end].rfind('/') {
            Some(i) => end = i,
            None => return false,
        }
    }
}

/// `raw` as a path relative to where it is written, with `.` and `..` segments
/// resolved. `None` when it is absolute or climbs out.
fn clean_relative(raw: &str) -> Option<String> {
    let s = raw.trim().replace('\\', "/");
    if s.starts_with('/') || s.get(1..2) == Some(":") {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in s.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// `rel` under the repository-relative directory `dir` (empty for the root).
fn join_dir(dir: &str, rel: &str) -> String {
    match (dir.is_empty(), rel.is_empty()) {
        (true, _) => rel.to_string(),
        (false, true) => dir.to_string(),
        (false, false) => format!("{dir}/{rel}"),
    }
}

fn toml_val_to_strings(val: &toml::Value) -> Vec<String> {
    match val {
        toml::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        toml::Value::String(s) => s
            .split_whitespace()
            .flat_map(|part| part.split(','))
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// JS / TS test runner collection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsCollectionResult {
    Collected,
    NotCollected,
    Unknown(String),
}

/// JS / TS test runner (Jest / Vitest) collection configuration.
#[derive(Debug, Clone, Default)]
pub struct JsCollectionRules {
    pub config_parsed: bool,
    pub unparseable_config: Option<String>,
    pub mocha_detected: Option<String>,
    pub invalid_pattern: Option<String>,
    /// What kind of problem `invalid_pattern` is, without the configured value it quotes.
    invalid_kind: Option<&'static str>,
    pub test_match: Vec<String>,
    pub test_regex: Vec<String>,
    pub include: Vec<String>,
    /// Jest `rootDir` as written; `<rootDir>` resolves to it.
    pub root_dir: Option<serde_json::Value>,
    /// Jest `roots` as written: only files under one of them are collected.
    pub roots: Option<serde_json::Value>,
    /// A Vitest `root` or `dir` key: the base `include` resolves against.
    pub vitest_root: Option<String>,
    /// Jest `testPathIgnorePatterns` as written; absent means Jest's default.
    pub ignore_patterns: Option<serde_json::Value>,
    /// A key that moves collection somewhere this model does not read: Jest `projects`,
    /// Vitest `exclude`.
    pub unread_key: Option<&'static str>,
    /// A Jest `preset` is named. It supplies defaults for the keys the configuration
    /// does not set itself, and it is not read.
    pub preset: bool,
    /// A second runner beside Jest / Vitest (Cypress, Playwright): a file Jest's
    /// patterns leave out may be that runner's.
    pub second_runner: Option<String>,
    /// Directories below the root that hold their own runner configuration.
    pub nested_configs: Vec<String>,
    /// Whether any JavaScript package manifest or runner configuration is tracked.
    /// `None` when the tree was not listed.
    pub manifest_seen: Option<bool>,
    /// Each glob with whether it is negated (`!pattern`), in configured order.
    compiled_globs: Vec<(globset::GlobMatcher, bool)>,
    compiled_regexes: Vec<regex::Regex>,
    compiled_ignores: Vec<regex::Regex>,
    /// Repository-relative directories collection is limited to; empty is no limit.
    effective_roots: Vec<String>,
}

impl PartialEq for JsCollectionRules {
    fn eq(&self, other: &Self) -> bool {
        self.config_parsed == other.config_parsed
            && self.unparseable_config == other.unparseable_config
            && self.mocha_detected == other.mocha_detected
            && self.invalid_pattern == other.invalid_pattern
            && self.test_match == other.test_match
            && self.test_regex == other.test_regex
            && self.include == other.include
            && self.root_dir == other.root_dir
            && self.roots == other.roots
            && self.vitest_root == other.vitest_root
            && self.ignore_patterns == other.ignore_patterns
            && self.unread_key == other.unread_key
            && self.preset == other.preset
            && self.second_runner == other.second_runner
            && self.nested_configs == other.nested_configs
            && self.manifest_seen == other.manifest_seen
    }
}

impl Eq for JsCollectionRules {}

/// True when a glob pattern uses extglob operators (`?(`, `*(`, `+(`, `@(`, `!(`),
/// which the glob matcher cannot evaluate. Bracket spans are skipped: `[?()]`
/// is a literal character class, not an operator.
fn has_extglob(pattern: &str) -> bool {
    let mut in_bracket = false;
    let mut prev = '\0';
    for c in pattern.chars() {
        match c {
            '[' if !in_bracket => in_bracket = true,
            ']' if in_bracket => in_bracket = false,
            '(' if !in_bracket && matches!(prev, '?' | '*' | '+' | '@' | '!') => return true,
            _ => {}
        }
        prev = c;
    }
    false
}

/// True when a glob pattern holds a parenthesised group outside a bracket class
/// (`*.(test|spec).js`). The runner's matcher reads it as alternation; the glob matcher
/// here would read the characters literally.
fn has_group(pattern: &str) -> bool {
    let mut in_bracket = false;
    for c in pattern.chars() {
        match c {
            '[' if !in_bracket => in_bracket = true,
            ']' if in_bracket => in_bracket = false,
            '(' if !in_bracket => return true,
            _ => {}
        }
    }
    false
}

/// Runners beside Jest / Vitest / Mocha that a dependency or a configuration file names.
const SECOND_RUNNER_PACKAGES: &[&str] = &["cypress", "@playwright/test"];
const SECOND_RUNNER_CONFIG_STEMS: &[&str] = &["cypress.config", "playwright.config"];
const SCRIPT_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "cjs", "mts", "cts"];

/// Whether `name` is `<stem>.<script extension>` for one of `stems`.
fn is_script_config(name: &str, stems: &[&str]) -> bool {
    stems.iter().any(|stem| {
        name.strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|ext| SCRIPT_EXTENSIONS.contains(&ext))
    })
}

/// Whether a tracked file name is a JavaScript package manifest or a runner
/// configuration: a sign that some JavaScript runner may run in the repository.
fn is_js_runner_sign(name: &str) -> bool {
    matches!(
        name,
        "package.json" | "deno.json" | "deno.jsonc" | "jest.config.json" | "cypress.json"
    ) || name.starts_with(".mocharc")
        || is_script_config(
            name,
            &[
                "jest.config",
                "vitest.config",
                "vite.config",
                "cypress.config",
                "playwright.config",
            ],
        )
}

/// A configured directory as a repository-relative path, or `None` when it cannot be
/// one: absolute, climbing out with `..`, or holding glob or token characters.
fn repo_relative_dir(raw: &str) -> Option<String> {
    let s = raw.trim().replace('\\', "/");
    if s.starts_with('/') || s.get(1..2) == Some(":") || s.contains(['*', '?', '[', '{', '<']) {
        return None;
    }
    let parts: Vec<&str> = s
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.contains(&"..") {
        return None;
    }
    Some(parts.join("/"))
}

/// Replaces a leading Jest `<rootDir>` token with the repository-relative `root_dir`.
fn resolve_root_token(pattern: &str, root_dir: &str) -> String {
    let Some(rest) = pattern.strip_prefix("<rootDir>") else {
        return pattern.to_string();
    };
    let rest = rest.trim_start_matches('/');
    match (root_dir.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_string(),
        (false, true) => root_dir.to_string(),
        (false, false) => format!("{root_dir}/{rest}"),
    }
}

/// Whether a lower-cased path has an extension the JS / TS runner rules apply to.
fn js_runner_extension(lower: &str) -> bool {
    [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

impl JsCollectionRules {
    /// Why collection is not determined, for a report note: the kind of problem, never
    /// the configured value. `None` when `is_collected` decides.
    fn unknown_kind(&self) -> Option<String> {
        if self.invalid_pattern.is_some() {
            return Some(
                self.invalid_kind
                    .unwrap_or("a configured pattern cannot be evaluated statically")
                    .to_string(),
            );
        }
        if let Some(unparseable) = &self.unparseable_config {
            return Some(format!("cannot parse statically: {unparseable}"));
        }
        if let Some(mocha) = &self.mocha_detected {
            return Some(mocha.clone());
        }
        if let Some(second) = &self.second_runner {
            return Some(second.clone());
        }
        (!self.config_parsed).then(|| "no runner config found".to_string())
    }

    /// No runner configuration was found and no JavaScript manifest is tracked anywhere:
    /// nothing in the repository could run a JavaScript test.
    fn no_runner_sign(&self) -> bool {
        self.manifest_seen == Some(false)
            && !self.config_parsed
            && self.invalid_pattern.is_none()
            && self.unparseable_config.is_none()
            && self.mocha_detected.is_none()
            && self.second_runner.is_none()
    }

    /// The directory of a nested runner configuration that holds `norm`, if any.
    fn nested_config_for(&self, norm: &str) -> bool {
        self.nested_configs
            .iter()
            .any(|dir| norm.starts_with(&format!("{dir}/")))
    }

    fn set_invalid(&mut self, detail: String, kind: &'static str) {
        if self.invalid_pattern.is_none() {
            self.invalid_pattern = Some(detail);
            self.invalid_kind = Some(kind);
        }
    }

    pub fn compile_patterns(&mut self) {
        self.compiled_regexes.clear();
        self.compiled_globs.clear();
        self.compiled_ignores.clear();
        self.effective_roots.clear();
        self.invalid_pattern = None;
        self.invalid_kind = None;

        if let Some(key) = self.unread_key {
            let kind = match key {
                "projects" => "jest `projects` configure collection per project",
                _ => "a vitest `exclude` is not evaluated",
            };
            self.set_invalid(format!("`{key}` is not read: {kind}"), kind);
            return;
        }
        // Jest merges the configuration over the preset, key by key: a `testMatch` or
        // `testRegex` of the configuration's own replaces the preset's. With neither,
        // the patterns are the preset's, which is not read.
        if self.preset && self.test_match.is_empty() && self.test_regex.is_empty() {
            let kind = "a jest `preset` may set the collection patterns";
            self.set_invalid(format!("`preset` is not read: {kind}"), kind);
            return;
        }
        if let Some(key) = &self.vitest_root {
            self.invalid_pattern = Some(format!(
                "vitest `{key}` moves where `include` resolves, cannot evaluate statically"
            ));
            self.invalid_kind = Some("a vitest `root` or `dir` moves where `include` resolves");
            return;
        }
        // Jest `rootDir` is relative to the configuration file, which is read at the
        // repository root, so it is the repository-relative prefix `<rootDir>` stands for.
        let root_dir = match &self.root_dir {
            None => String::new(),
            Some(v) => match v.as_str().and_then(repo_relative_dir) {
                Some(dir) => dir,
                None => {
                    self.invalid_pattern = Some(format!(
                        "jest rootDir {v} cannot be resolved to a repository path"
                    ));
                    self.invalid_kind =
                        Some("jest `rootDir` cannot be resolved to a repository path");
                    return;
                }
            },
        };
        // Jest only collects under `roots`, which defaults to `["<rootDir>"]`.
        match &self.roots {
            Some(v) => {
                let resolved: Option<Vec<String>> = v.as_array().and_then(|arr| {
                    arr.iter()
                        .map(|r| {
                            r.as_str().and_then(|s| {
                                let s = s.trim();
                                // A root is resolved against `rootDir`, token or not.
                                if s.starts_with("<rootDir>") {
                                    repo_relative_dir(&resolve_root_token(s, &root_dir))
                                } else {
                                    repo_relative_dir(s).map(|rel| join_dir(&root_dir, &rel))
                                }
                            })
                        })
                        .collect()
                });
                match resolved {
                    // A root that is the repository itself limits nothing.
                    Some(dirs) if !dirs.iter().any(String::is_empty) => self.effective_roots = dirs,
                    Some(_) => {}
                    None => {
                        self.invalid_pattern = Some(format!(
                            "jest roots {v} cannot be resolved to repository paths"
                        ));
                        self.invalid_kind =
                            Some("jest `roots` cannot be resolved to repository paths");
                        return;
                    }
                }
            }
            None if !root_dir.is_empty() => self.effective_roots = vec![root_dir.clone()],
            None => {}
        }

        for r in &self.test_regex {
            match regex::Regex::new(r) {
                Ok(re) => self.compiled_regexes.push(re),
                Err(_) => {
                    if self.invalid_pattern.is_none() {
                        self.invalid_pattern =
                            Some(format!("regex pattern failed to compile: '{r}'"));
                        self.invalid_kind = Some("a configured regex pattern does not compile");
                    }
                }
            }
        }

        // Jest matches `testPathIgnorePatterns` against the absolute path; here the path
        // is matched from the repository root with a leading `/`, so `<rootDir>` is the
        // start of that path (followed by `/` and the configured `rootDir`).
        let ignores: Vec<String> = match &self.ignore_patterns {
            // Under a preset an unset list may be the preset's: nothing is excluded by it.
            None if self.preset => Vec::new(),
            None => vec!["/node_modules/".to_string()],
            Some(v) => {
                let listed: Option<Vec<String>> = v
                    .as_array()
                    .and_then(|arr| arr.iter().map(|p| p.as_str().map(str::to_string)).collect());
                match listed {
                    Some(list) => list,
                    None => {
                        self.set_invalid(
                            format!("jest testPathIgnorePatterns {v} is not a list of patterns"),
                            "jest `testPathIgnorePatterns` is not a list of patterns",
                        );
                        return;
                    }
                }
            }
        };
        let root_prefix = if root_dir.is_empty() {
            "^".to_string()
        } else {
            format!("^/{}", regex::escape(&root_dir))
        };
        for raw in &ignores {
            match regex::Regex::new(&raw.replace("<rootDir>", &root_prefix)) {
                Ok(re) => self.compiled_ignores.push(re),
                Err(_) => self.set_invalid(
                    format!("ignore pattern failed to compile: '{raw}'"),
                    "a configured ignore pattern does not compile",
                ),
            }
        }

        let test_match_len = self.test_match.len();
        let patterns: Vec<String> = self
            .test_match
            .iter()
            .chain(self.include.iter())
            .cloned()
            .collect();
        for (i, pat) in patterns.iter().enumerate() {
            let trimmed = pat.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (negated, body) = match trimmed.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, trimmed),
            };
            let from_test_match = i < test_match_len;
            if negated && !from_test_match {
                self.set_invalid(
                    format!("negated `include` pattern is not evaluated: '{trimmed}'"),
                    "a negated vitest `include` pattern is not evaluated",
                );
                continue;
            }
            // Jest matches `testMatch` against the absolute path: a pattern matches only
            // when it starts at `<rootDir>` or with `**`.
            if from_test_match && !body.starts_with("<rootDir>") && !body.starts_with("**") {
                self.set_invalid(
                    format!("testMatch pattern is not anchored: '{trimmed}'"),
                    "a jest `testMatch` pattern starts at neither `<rootDir>` nor `**`",
                );
                continue;
            }
            let without_root = resolve_root_token(body, &root_dir);
            if has_extglob(&without_root) {
                if self.invalid_pattern.is_none() {
                    self.invalid_pattern = Some(format!(
                        "glob pattern uses extglob, cannot evaluate statically: '{trimmed}'"
                    ));
                    self.invalid_kind = Some("a configured glob pattern uses extglob");
                }
                continue;
            }
            if has_group(&without_root) {
                self.set_invalid(
                    format!("glob pattern uses a group, cannot evaluate statically: '{trimmed}'"),
                    "a configured glob pattern uses a `(a|b)` group",
                );
                continue;
            }
            match globset::GlobBuilder::new(&without_root)
                .literal_separator(false)
                .build()
            {
                Ok(glob) => self.compiled_globs.push((glob.compile_matcher(), negated)),
                Err(_) => {
                    if self.invalid_pattern.is_none() {
                        self.invalid_pattern =
                            Some(format!("glob pattern failed to compile: '{trimmed}'"));
                        self.invalid_kind = Some("a configured glob pattern does not compile");
                    }
                }
            }
        }
    }

    pub fn is_collected(&self, path: &str) -> JsCollectionResult {
        let norm = path.replace('\\', "/");
        let lower = norm.to_ascii_lowercase();
        if !js_runner_extension(&lower) {
            return JsCollectionResult::NotCollected;
        }

        // 1. If any configured pattern regex/glob failed to compile
        if let Some(err) = &self.invalid_pattern {
            return JsCollectionResult::Unknown(err.clone());
        }

        // 2. If a vitest.config.* or jest.config.{js,ts,mjs,cjs} exists (cannot parse statically)
        if let Some(unparseable) = &self.unparseable_config {
            return JsCollectionResult::Unknown(format!("cannot parse statically: {unparseable}"));
        }

        // 3. If Mocha is detected
        if let Some(mocha) = &self.mocha_detected {
            return JsCollectionResult::Unknown(mocha.clone());
        }

        // 4. If no runner config found
        if !self.config_parsed {
            return JsCollectionResult::Unknown("no runner config found".to_string());
        }

        // 5. A nested package runs its own configuration, which is not read
        if self.nested_config_for(&norm) {
            return JsCollectionResult::Unknown(
                "a nested package has its own runner configuration".to_string(),
            );
        }

        // 6. The parsed configuration decides. Beside a second runner, a file it leaves
        // out may be that runner's test: not determined, rather than excluded.
        match (self.configuration_collects(&norm), &self.second_runner) {
            (true, _) => JsCollectionResult::Collected,
            (false, Some(second)) => JsCollectionResult::Unknown(second.clone()),
            (false, None) => JsCollectionResult::NotCollected,
        }
    }

    /// Whether the parsed Jest / Vitest configuration collects `norm`: under a
    /// configured root, not ignored, and matched by the configured patterns or, with
    /// none, by the default conventions.
    fn configuration_collects(&self, norm: &str) -> bool {
        // Outside every configured root the runner never looks.
        if !self.effective_roots.is_empty()
            && !self
                .effective_roots
                .iter()
                .any(|r| norm == *r || norm.starts_with(&format!("{r}/")))
        {
            return false;
        }

        // Jest matches regexes against the absolute path, which this is the tail of.
        let rooted = format!("/{norm}");
        if self.compiled_ignores.iter().any(|re| re.is_match(&rooted)) {
            return false;
        }

        // Config was parsed: check configured patterns or defaults
        let has_custom =
            !self.test_regex.is_empty() || !self.test_match.is_empty() || !self.include.is_empty();

        if has_custom {
            return self.compiled_regexes.iter().any(|re| re.is_match(&rooted))
                || self.globs_keep(norm);
        }

        // Default Jest / Vitest collection conventions
        // 1. Inside __tests__/
        if norm.contains("/__tests__/") || norm.starts_with("__tests__/") {
            return true;
        }

        // 2. Basename is `test.<ext>` / `spec.<ext>` or ends in `.test.<ext>` / `.spec.<ext>`
        let filename = norm.rsplit('/').next().unwrap_or(norm);
        let f_lower = filename.to_ascii_lowercase();
        let stem = f_lower.rsplit_once('.').map_or("", |(stem, _)| stem);
        matches!(stem, "test" | "spec") || stem.ends_with(".test") || stem.ends_with(".spec")
    }

    /// Jest's rule for a list of globs: in configured order, a negated glob that
    /// matches drops the path and a plain glob that matches keeps it; a list of only
    /// negated globs keeps what none of them drops.
    fn globs_keep(&self, norm: &str) -> bool {
        let mut kept = None;
        let mut negatives = 0;
        for (matcher, negated) in &self.compiled_globs {
            let matched = matcher.is_match(norm);
            if *negated {
                negatives += 1;
                if matched {
                    kept = Some(false);
                }
            } else if matched {
                kept = Some(true);
            }
        }
        if !self.compiled_globs.is_empty() && negatives == self.compiled_globs.len() {
            kept != Some(false)
        } else {
            kept == Some(true)
        }
    }

    pub fn merge_package_json(&mut self, content: &str) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
            return;
        };

        // Check mocha in dependencies / devDependencies / peerDependencies
        let has_mocha = ["dependencies", "devDependencies", "peerDependencies"]
            .iter()
            .any(|dep_key| val.get(*dep_key).and_then(|d| d.get("mocha")).is_some());
        if has_mocha {
            self.mocha_detected = Some("mocha detected in package.json dependencies".to_string());
        }
        if let Some(name) = Self::second_runner_dependency(&val) {
            self.second_runner = Some(format!("{name} detected in package.json dependencies"));
        }

        if let Some(jest) = val.get("jest") {
            self.config_parsed = true;
            self.extract_from_json(jest);
        }

        if let Some(vitest) = val.get("vitest") {
            self.config_parsed = true;
            self.extract_from_json(vitest);
            if let Some(test_obj) = vitest.get("test") {
                self.extract_from_json(test_obj);
            }
        }

        self.compile_patterns();
    }

    fn second_runner_dependency(package: &serde_json::Value) -> Option<&'static str> {
        SECOND_RUNNER_PACKAGES.iter().copied().find(|name| {
            ["dependencies", "devDependencies", "peerDependencies"]
                .iter()
                .any(|key| package.get(*key).and_then(|d| d.get(*name)).is_some())
        })
    }

    /// Whether a nested `package.json` configures a runner of its own.
    fn package_configures_runner(content: &str) -> bool {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
            return false;
        };
        val.get("jest").is_some()
            || val.get("vitest").is_some()
            || Self::second_runner_dependency(&val).is_some()
            || ["dependencies", "devDependencies", "peerDependencies"]
                .iter()
                .any(|key| val.get(*key).and_then(|d| d.get("mocha")).is_some())
    }

    pub fn merge_jest_config_json(&mut self, content: &str) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
            return;
        };
        self.config_parsed = true;
        self.extract_from_json(&val);
        self.compile_patterns();
    }

    fn extract_from_json(&mut self, val: &serde_json::Value) {
        if let Some(rd) = val.get("rootDir") {
            self.root_dir = Some(rd.clone());
        }
        if let Some(roots) = val.get("roots") {
            self.roots = Some(roots.clone());
        }
        for key in ["root", "dir"] {
            if val.get(key).is_some() {
                self.vitest_root = Some(key.to_string());
            }
        }
        for key in ["projects", "exclude"] {
            if val.get(key).is_some_and(|v| !v.is_null()) {
                self.unread_key = Some(key);
            }
        }
        if val.get("preset").is_some_and(|v| !v.is_null()) {
            self.preset = true;
        }
        if let Some(ignores) = val.get("testPathIgnorePatterns") {
            self.ignore_patterns = Some(ignores.clone());
        }
        if let Some(tm) = val.get("testMatch").and_then(|v| v.as_array()) {
            let matches: Vec<String> = tm
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if !matches.is_empty() {
                self.test_match = matches;
            }
        }
        if let Some(inc) = val.get("include").and_then(|v| v.as_array()) {
            let includes: Vec<String> = inc
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if !includes.is_empty() {
                self.include = includes;
            }
        }
        if let Some(tr) = val.get("testRegex") {
            if let Some(s) = tr.as_str() {
                self.test_regex = vec![s.to_string()];
            } else if let Some(arr) = tr.as_array() {
                self.test_regex = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
            }
        }
    }
}

/// Result of evaluating a Rust cfg predicate under Kleene three-valued logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgValue {
    True,
    False,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestStatus {
    Package(HashSet<String>),
    WorkspaceOnly,
    ParseError,
}

/// A `[[test]]` target as its manifest declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoTestTarget {
    pub name: Option<String>,
    /// `path`, relative to the package directory, when the manifest gives one.
    pub path: Option<String>,
    /// `test` and `harness` both left on: `cargo test` builds the target with the test
    /// harness, so its `#[test]` functions run.
    pub runs: bool,
}

impl CargoTestTarget {
    /// Whether the target's root file is the package-relative `rel`. Without a `path`,
    /// Cargo looks for `tests/<name>.rs` then `tests/<name>/main.rs`.
    fn names(&self, rel: &str) -> bool {
        match (&self.path, &self.name) {
            (Some(path), _) => path == rel,
            (None, Some(name)) => {
                rel == format!("tests/{name}.rs") || rel == format!("tests/{name}/main.rs")
            }
            (None, None) => false,
        }
    }
}

/// What a package's `Cargo.toml` says about which of its files `cargo test` builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoPackage {
    /// `[package] autotests`: every `tests/*.rs` and `tests/*/main.rs` is a test target.
    pub autotests: bool,
    /// `[lib] test` and `harness` both left on: the library's unit tests run.
    pub lib_tests_run: bool,
    /// `[lib] path`, when the manifest gives one.
    pub lib_path: Option<String>,
    /// `path` of each `[[bin]]` target whose unit tests run (`test` left on).
    pub bin_paths: Vec<String>,
    pub tests: Vec<CargoTestTarget>,
    /// Whether the package builds a binary, whose unit tests run whatever `[lib]` says.
    /// `None` when the manifest names none and the tree was not listed.
    pub has_binary: Option<bool>,
}

/// The files reached from a package's running test targets through `mod` declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestModules {
    pub reached: HashSet<String>,
    /// A reached file declares modules in a way that is not followed (a macro, an
    /// `include!`, a `path` under `cfg_attr` or inside an inline module, a file that
    /// does not parse): a file not reached may still be a module.
    pub open: bool,
}

/// Whether `cargo test` builds and runs the tests of a Rust file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustCollection {
    Collected,
    NotCollected,
    Unknown(&'static str),
}

/// Rust test collection and feature configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RustCollectionRules {
    pub declared_features: HashSet<String>,
    pub custom_test_paths: Vec<String>,
    /// Every `Cargo.toml` on this side, by repository-relative path, read through the
    /// same reader as the rest of the rules (base blob or head tree).
    pub manifests: std::collections::HashMap<String, ManifestStatus>,
    /// The targets of every manifest with a `[package]` table, by package directory
    /// (empty for the repository root).
    pub packages: std::collections::HashMap<String, CargoPackage>,
    /// Module reachability under each package's test targets, by package directory.
    /// Present only when the tree was listed.
    pub test_modules: std::collections::HashMap<String, TestModules>,
}

/// The `mod` declarations of one Rust file that name another file.
struct ModuleScan {
    /// `(enclosing inline modules, name, #[path] value)`.
    declarations: Vec<(Vec<String>, String, Option<String>)>,
    open: bool,
}

fn rust_leaf_is(node: Node, src: &[u8], word: &str) -> bool {
    if node.child_count() == 0 {
        return node.utf8_text(src).ok() == Some(word);
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    children.into_iter().any(|c| rust_leaf_is(c, src, word))
}

/// `(name, value)` of the attributes right above `node`, comments skipped.
fn rust_outer_attributes<'a>(node: Node<'a>, src: &'a [u8]) -> Vec<(String, Node<'a>)> {
    let mut out = Vec::new();
    let mut prev = node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let mut cursor = p.walk();
                let attribute = p.children(&mut cursor).find(|c| c.kind() == "attribute");
                if let Some(attr) = attribute {
                    let name = attr
                        .child(0)
                        .and_then(|c| c.utf8_text(src).ok())
                        .unwrap_or("")
                        .to_string();
                    out.push((name, attr));
                }
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        prev = p.prev_sibling();
    }
    out
}

fn scan_rust_module_items(
    node: Node,
    src: &[u8],
    parents: &mut Vec<String>,
    scan: &mut ModuleScan,
) {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    for child in children {
        let item = if child.kind() == "expression_statement" {
            child.named_child(0).unwrap_or(child)
        } else {
            child
        };
        match item.kind() {
            "mod_item" => {
                let Some(name) = item
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(src).ok())
                else {
                    scan.open = true;
                    continue;
                };
                let mut path = None;
                for (attr_name, attr) in rust_outer_attributes(item, src) {
                    match attr_name.as_str() {
                        "path" => {
                            let value = attr
                                .child_by_field_name("value")
                                .filter(|v| v.kind() == "string_literal")
                                .and_then(|v| v.utf8_text(src).ok())
                                .map(|v| v.trim_matches('"').to_string());
                            match value {
                                Some(v) if !v.contains('\\') => path = Some(v),
                                _ => scan.open = true,
                            }
                        }
                        "cfg_attr" if rust_leaf_is(attr, src, "path") => scan.open = true,
                        _ => {}
                    }
                }
                match item.child_by_field_name("body") {
                    None => scan
                        .declarations
                        .push((parents.clone(), name.to_string(), path)),
                    Some(body) => {
                        if path.is_some() {
                            scan.open = true;
                        }
                        parents.push(name.to_string());
                        scan_rust_module_items(body, src, parents, scan);
                        parents.pop();
                    }
                }
            }
            "macro_invocation" => {
                let name = item
                    .child_by_field_name("macro")
                    .and_then(|n| n.utf8_text(src).ok())
                    .unwrap_or("");
                let last = name.rsplit("::").next().unwrap_or(name);
                if last == "include" || name.contains("automod") || rust_leaf_is(item, src, "mod") {
                    scan.open = true;
                }
            }
            "macro_definition" if rust_leaf_is(item, src, "mod") => scan.open = true,
            _ => {}
        }
    }
}

/// Reads the `mod` declarations of a Rust source file with the tree-sitter grammar.
fn scan_rust_modules(source: &str) -> ModuleScan {
    let mut scan = ModuleScan {
        declarations: Vec::new(),
        open: false,
    };
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        scan.open = true;
        return scan;
    }
    let Some(tree) = parser.parse(source, None) else {
        scan.open = true;
        return scan;
    };
    if tree.root_node().has_error() {
        scan.open = true;
    }
    scan_rust_module_items(
        tree.root_node(),
        source.as_bytes(),
        &mut Vec::new(),
        &mut scan,
    );
    scan
}

/// Whether a package-relative path is a test target Cargo discovers by itself:
/// `tests/<name>.rs` or `tests/<name>/main.rs`.
fn is_auto_test_root(rel: &str) -> bool {
    let Some(rest) = rel.strip_prefix("tests/") else {
        return false;
    };
    match rest.split_once('/') {
        None => true,
        Some((_, file)) => file == "main.rs",
    }
}

impl RustCollectionRules {
    fn manifest_status(content: &str) -> ManifestStatus {
        match toml::from_str::<toml::Value>(content) {
            Ok(val) if val.get("package").and_then(|t| t.as_table()).is_some() => {
                ManifestStatus::Package(Self::parse_cargo_toml(content).declared_features)
            }
            Ok(_) => ManifestStatus::WorkspaceOnly,
            Err(_) => ManifestStatus::ParseError,
        }
    }

    /// Features of the crate owning `test_path`: the nearest ancestor `Cargo.toml` with a
    /// `[package]` table. `(None, Some(manifest))` when that manifest does not parse;
    /// `(None, None)` when no owning manifest is known.
    pub fn find_owning_crate_features(
        &self,
        test_path: &str,
    ) -> (Option<HashSet<String>>, Option<String>) {
        let norm = test_path.replace('\\', "/");
        let mut dir = norm.as_str();
        loop {
            dir = match dir.rfind('/') {
                Some(i) => &dir[..i],
                None => "",
            };
            let candidate = if dir.is_empty() {
                "Cargo.toml".to_string()
            } else {
                format!("{dir}/Cargo.toml")
            };
            match self.manifests.get(&candidate) {
                Some(ManifestStatus::Package(feat)) => return (Some(feat.clone()), None),
                Some(ManifestStatus::ParseError) => return (None, Some(candidate)),
                Some(ManifestStatus::WorkspaceOnly) | None => {}
            }
            if dir.is_empty() {
                return (None, None);
            }
        }
    }

    /// The nearest ancestor directory of `norm` whose `Cargo.toml` has a `[package]`
    /// table that parsed, with what it declares. `None` when there is none, or when a
    /// nearer manifest does not parse.
    fn owning_package(&self, norm: &str) -> Option<(String, &CargoPackage)> {
        let mut dir = norm;
        loop {
            dir = match dir.rfind('/') {
                Some(i) => &dir[..i],
                None => "",
            };
            match self.manifests.get(&join_dir(dir, "Cargo.toml")) {
                Some(ManifestStatus::Package(_)) => {
                    return self.packages.get(dir).map(|pkg| (dir.to_string(), pkg));
                }
                Some(ManifestStatus::ParseError) => return None,
                Some(ManifestStatus::WorkspaceOnly) | None => {}
            }
            if dir.is_empty() {
                return None;
            }
        }
    }

    /// Whether `cargo test` runs the tests of `path`, as the owning manifest declares
    /// its targets. Without a parsed owning manifest the path rule of
    /// [`Self::is_collected`] decides.
    pub fn status(&self, path: &str) -> RustCollection {
        let raw = path.replace('\\', "/");
        let norm = raw.strip_prefix("./").unwrap_or(&raw);
        let by_path = |norm: &str| {
            if self.is_collected(norm) {
                RustCollection::Collected
            } else {
                RustCollection::NotCollected
            }
        };
        if !norm.ends_with(".rs") {
            return RustCollection::NotCollected;
        }
        let Some((dir, pkg)) = self.owning_package(norm) else {
            return by_path(norm);
        };
        let rel = if dir.is_empty() {
            norm
        } else {
            &norm[dir.len() + 1..]
        };
        let modules = self.test_modules.get(&dir);

        // A `[[test]]` target names the file: its `test` and `harness` keys decide.
        if let Some(target) = pkg.tests.iter().find(|t| t.names(rel)) {
            return if target.runs {
                RustCollection::Collected
            } else {
                RustCollection::NotCollected
            };
        }
        if modules.is_some_and(|m| m.reached.contains(norm)) {
            return RustCollection::Collected;
        }
        if rel.starts_with("tests/") {
            if is_auto_test_root(rel) {
                return if pkg.autotests {
                    RustCollection::Collected
                } else {
                    RustCollection::NotCollected
                };
            }
            // Any other file under `tests/` is built only as a module of a target.
            return match modules {
                None => RustCollection::Collected,
                Some(m) if m.open => RustCollection::Unknown(
                    "a Cargo test target declares modules in a way that is not followed",
                ),
                Some(_) => RustCollection::NotCollected,
            };
        }
        let lib_tests = || {
            if pkg.lib_tests_run {
                RustCollection::Collected
            } else if pkg.has_binary == Some(false) {
                RustCollection::NotCollected
            } else {
                RustCollection::Unknown(
                    "`[lib] test = false` in a package that may also build a binary",
                )
            }
        };
        if rel.starts_with("src/") {
            return lib_tests();
        }
        // A target rooted outside `src/` and `tests/`: the root is built, and the files
        // beside it may be its modules.
        if pkg.lib_path.as_deref() == Some(rel) {
            return lib_tests();
        }
        if pkg.bin_paths.iter().any(|p| p == rel) {
            return RustCollection::Collected;
        }
        let beside_a_target = pkg
            .lib_path
            .iter()
            .chain(pkg.bin_paths.iter())
            .chain(pkg.tests.iter().filter_map(|t| t.path.as_ref()))
            .filter(|p| !p.starts_with("src/") && !p.starts_with("tests/"))
            .any(|p| match p.rfind('/') {
                Some(i) => rel.starts_with(&p[..=i]),
                None => true,
            });
        if beside_a_target {
            return RustCollection::Unknown("a Cargo target `path` outside `src/` and `tests/`");
        }
        by_path(norm)
    }

    /// The targets a package manifest declares. `None` without a `[package]` table.
    fn parse_package(content: &str) -> Option<CargoPackage> {
        let val = toml::from_str::<toml::Value>(content).ok()?;
        let package = val.get("package")?.as_table()?;
        let flag = |table: &toml::Value, key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_bool)
                .unwrap_or(true)
        };
        let path_of = |table: &toml::Value| {
            table
                .get("path")
                .and_then(toml::Value::as_str)
                .and_then(clean_relative)
        };
        let tables = |key: &str| -> Vec<toml::Value> {
            val.get(key)
                .and_then(toml::Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let lib = val.get("lib");
        Some(CargoPackage {
            autotests: package
                .get("autotests")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true),
            lib_tests_run: lib.is_none_or(|l| flag(l, "test") && flag(l, "harness")),
            lib_path: lib.and_then(path_of),
            bin_paths: tables("bin")
                .iter()
                .filter(|b| flag(b, "test") && flag(b, "harness"))
                .filter_map(path_of)
                .collect(),
            tests: tables("test")
                .iter()
                .map(|t| CargoTestTarget {
                    name: t
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string),
                    path: path_of(t),
                    runs: flag(t, "test") && flag(t, "harness"),
                })
                .collect(),
            has_binary: (!tables("bin").is_empty()).then_some(true),
        })
    }

    /// Records the manifest at `path` and, when it is a package, its targets.
    fn add_manifest(&mut self, path: &str, content: &str) {
        self.manifests
            .insert(path.to_string(), Self::manifest_status(content));
        if let Some(pkg) = Self::parse_package(content) {
            let dir = path.strip_suffix("Cargo.toml").unwrap_or("");
            self.packages
                .insert(dir.trim_end_matches('/').to_string(), pkg);
        }
    }

    /// With every tracked path known: which packages build a binary, and which files
    /// each package's running test targets reach through `mod` declarations.
    fn read_tree<F>(&mut self, tracked: &[String], reader: &mut F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        let rust_files: HashSet<&str> = tracked
            .iter()
            .map(String::as_str)
            .filter(|p| p.ends_with(".rs"))
            .collect();
        let mut roots: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut binaries: HashSet<String> = HashSet::new();
        for file in &rust_files {
            let Some((dir, pkg)) = self.owning_package(file) else {
                continue;
            };
            let rel = if dir.is_empty() {
                file
            } else {
                &file[dir.len() + 1..]
            };
            if rel == "src/main.rs" || rel.starts_with("src/bin/") {
                binaries.insert(dir.clone());
            }
            let is_root = match pkg.tests.iter().find(|t| t.names(rel)) {
                Some(target) => target.runs,
                None => pkg.autotests && is_auto_test_root(rel),
            };
            if is_root {
                roots.entry(dir).or_default().push((*file).to_string());
            }
        }
        for (dir, pkg) in &mut self.packages {
            if pkg.has_binary.is_none() {
                pkg.has_binary = Some(binaries.contains(dir));
            }
        }
        let dirs: Vec<String> = self.packages.keys().cloned().collect();
        for dir in dirs {
            let mut modules = TestModules::default();
            // `(file, whether its modules live beside it rather than under its stem)`
            let mut queue: Vec<(String, bool)> = roots
                .remove(&dir)
                .unwrap_or_default()
                .into_iter()
                .map(|root| (root, true))
                .collect();
            while let Some((file, owns_dir)) = queue.pop() {
                if !modules.reached.insert(file.clone()) {
                    continue;
                }
                let Some(source) = reader(&file) else {
                    modules.open = true;
                    continue;
                };
                let scan = scan_rust_modules(&source);
                modules.open |= scan.open;
                let (file_dir, name) = match file.rfind('/') {
                    Some(i) => (&file[..i], &file[i + 1..]),
                    None => ("", file.as_str()),
                };
                let owns_dir = owns_dir || matches!(name, "mod.rs" | "main.rs" | "lib.rs");
                let base = if owns_dir {
                    file_dir.to_string()
                } else {
                    join_dir(file_dir, name.strip_suffix(".rs").unwrap_or(name))
                };
                for (parents, module, path) in scan.declarations {
                    if let Some(path) = path {
                        // A `path` on a module of the file itself is relative to the
                        // file's directory; the file it names owns that directory.
                        match clean_relative(&join_dir(file_dir, &path)) {
                            Some(target)
                                if parents.is_empty() && rust_files.contains(target.as_str()) =>
                            {
                                queue.push((target, true));
                            }
                            _ => modules.open = true,
                        }
                        continue;
                    }
                    let mut module_dir = base.clone();
                    for parent in &parents {
                        module_dir = join_dir(&module_dir, parent);
                    }
                    for candidate in [
                        join_dir(&module_dir, &format!("{module}.rs")),
                        join_dir(&module_dir, &format!("{module}/mod.rs")),
                    ] {
                        if rust_files.contains(candidate.as_str()) {
                            queue.push((candidate, false));
                        }
                    }
                }
            }
            self.test_modules.insert(dir, modules);
        }
    }

    /// The path rule used when no parsed manifest owns the file: any `.rs` under a
    /// `src/` or `tests/` directory, or a root-manifest `[[test]] path`.
    pub fn is_collected(&self, path: &str) -> bool {
        let raw = path.replace('\\', "/");
        let norm = raw.strip_prefix("./").unwrap_or(&raw);
        if !norm.ends_with(".rs") {
            return false;
        }

        // Unit test in crate source
        if norm.starts_with("src/") || norm == "src" || norm.contains("/src/") {
            return true;
        }

        // Explicit [[test]] path
        if self.custom_test_paths.iter().any(|p| {
            let p_raw = p.replace('\\', "/");
            let p_norm = p_raw.strip_prefix("./").unwrap_or(&p_raw);
            p_norm == norm || norm.ends_with(&format!("/{p_norm}"))
        }) {
            return true;
        }

        // Integration tests under tests/ at any depth
        if norm.starts_with("tests/") || norm.contains("/tests/") {
            return true;
        }

        false
    }

    pub fn parse_cargo_toml(content: &str) -> Self {
        let mut rules = Self::default();
        let Ok(val) = toml::from_str::<toml::Value>(content) else {
            return rules;
        };
        let Some(root) = val.as_table() else {
            return rules;
        };

        // 1. [features] table
        if let Some(features) = root.get("features").and_then(toml::Value::as_table) {
            for (k, _) in features {
                rules.declared_features.insert(k.clone());
            }
        }

        // 2. Optional dependencies
        for table_name in &["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(deps) = root.get(*table_name).and_then(toml::Value::as_table) {
                for (k, v) in deps {
                    if let Some(tbl) = v.as_table() {
                        if tbl.get("optional").and_then(toml::Value::as_bool) == Some(true) {
                            rules.declared_features.insert(k.clone());
                        }
                    }
                }
            }
        }

        // 2b. Optional dependencies of one target (`[target.'cfg(unix)'.dependencies]`)
        if let Some(targets) = root.get("target").and_then(toml::Value::as_table) {
            for target in targets.values() {
                for table_name in &["dependencies", "build-dependencies"] {
                    if let Some(deps) = target.get(*table_name).and_then(toml::Value::as_table) {
                        for (k, v) in deps {
                            if v.get("optional").and_then(toml::Value::as_bool) == Some(true) {
                                rules.declared_features.insert(k.clone());
                            }
                        }
                    }
                }
            }
        }

        // 3. [[test]] path
        if let Some(tests) = root.get("test").and_then(toml::Value::as_array) {
            for t in tests {
                if let Some(p) = t.get("path").and_then(toml::Value::as_str) {
                    rules.custom_test_paths.push(p.to_string());
                }
            }
        }

        rules
    }
}

/// Extracts the inner nodes of a token_tree (excluding delimiters like `(` `)` or `[` `]`).
fn token_tree_inner<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let count = node.child_count();
    if count <= 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 1..count - 1 {
        if let Some(c) = node.child(i) {
            if !matches!(c.kind(), "line_comment" | "block_comment") {
                out.push(c);
            }
        }
    }
    out
}

/// Splits child nodes by comma at the top level of this token slice.
fn split_by_comma<'a>(nodes: &[Node<'a>]) -> Vec<Vec<Node<'a>>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for &n in nodes {
        if n.kind() == "," {
            if !current.is_empty() {
                groups.push(current);
                current = Vec::new();
            }
        } else {
            current.push(n);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// Extracts predicate nodes from an attribute node or token_tree.
fn get_predicate_nodes<'a>(node: Node<'a>, src: &[u8]) -> Vec<Node<'a>> {
    if matches!(node.kind(), "attribute_item" | "inner_attribute_item") {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == "attribute" {
                    return get_predicate_nodes(child, src);
                }
            }
        }
    }
    if node.kind() == "attribute" {
        let is_cfg_attr = node
            .child(0)
            .and_then(|c| c.utf8_text(src).ok())
            .map(|s| s == "cfg_attr")
            .unwrap_or(false);
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == "token_tree" {
                    let inner = token_tree_inner(child);
                    if is_cfg_attr {
                        return inner.into_iter().take_while(|n| n.kind() != ",").collect();
                    } else {
                        return inner;
                    }
                }
            }
        }
    }
    if node.kind() == "token_tree" {
        let text = node.utf8_text(src).unwrap_or("");
        if text.starts_with('[') {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.kind() == "attribute" {
                        return get_predicate_nodes(child, src);
                    }
                    if child.kind() == "token_tree"
                        && child.utf8_text(src).unwrap_or("").starts_with('(')
                    {
                        let is_cfg_attr = node.children(&mut node.walk()).any(|c| {
                            c.kind() == "identifier" && c.utf8_text(src).ok() == Some("cfg_attr")
                        });
                        let inner = token_tree_inner(child);
                        if is_cfg_attr {
                            return inner.into_iter().take_while(|n| n.kind() != ",").collect();
                        } else {
                            return inner;
                        }
                    }
                }
            }
        }
        return token_tree_inner(node);
    }
    Vec::new()
}

fn node_slice_text(nodes: &[Node], src: &[u8]) -> String {
    if nodes.is_empty() {
        return String::new();
    }
    let start = nodes.first().unwrap().start_byte();
    let end = nodes.last().unwrap().end_byte();
    if start <= end && end <= src.len() {
        String::from_utf8_lossy(&src[start..end]).trim().to_string()
    } else {
        nodes
            .iter()
            .filter_map(|n| n.utf8_text(src).ok())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn eval_predicate(
    nodes: &[Node],
    src: &[u8],
    declared_features: Option<&HashSet<String>>,
) -> (CfgValue, String) {
    let items: Vec<Node> = nodes
        .iter()
        .copied()
        .filter(|n| !matches!(n.kind(), "line_comment" | "block_comment"))
        .collect();

    if items.is_empty() {
        return (CfgValue::Unknown, String::new());
    }

    let full_text = node_slice_text(&items, src);

    // 1. not(...)
    if items[0].kind() == "identifier"
        && items[0].utf8_text(src).ok() == Some("not")
        && items.len() >= 2
        && items[1].kind() == "token_tree"
    {
        let inner = token_tree_inner(items[1]);
        let (sub_val, sub_str) = eval_predicate(&inner, src, declared_features);
        let val = match sub_val {
            CfgValue::True => CfgValue::False,
            CfgValue::False => CfgValue::True,
            CfgValue::Unknown => CfgValue::Unknown,
        };
        let cond_str = if sub_str.is_empty() {
            "not()".to_string()
        } else {
            format!("not({sub_str})")
        };
        return (val, cond_str);
    }

    // 2. all(...)
    if items[0].kind() == "identifier"
        && items[0].utf8_text(src).ok() == Some("all")
        && items.len() >= 2
        && items[1].kind() == "token_tree"
    {
        let inner = token_tree_inner(items[1]);
        let sub_args = split_by_comma(&inner);
        if sub_args.is_empty() {
            return (CfgValue::True, "all()".to_string());
        }
        let mut has_false = false;
        let mut has_unknown = false;
        let mut arg_strings = Vec::new();
        for arg in sub_args {
            let (v, s) = eval_predicate(&arg, src, declared_features);
            arg_strings.push(s);
            match v {
                CfgValue::False => has_false = true,
                CfgValue::Unknown => has_unknown = true,
                CfgValue::True => {}
            }
        }
        let val = if has_false {
            CfgValue::False
        } else if has_unknown {
            CfgValue::Unknown
        } else {
            CfgValue::True
        };
        return (val, format!("all({})", arg_strings.join(", ")));
    }

    // 3. any(...)
    if items[0].kind() == "identifier"
        && items[0].utf8_text(src).ok() == Some("any")
        && items.len() >= 2
        && items[1].kind() == "token_tree"
    {
        let inner = token_tree_inner(items[1]);
        let sub_args = split_by_comma(&inner);
        if sub_args.is_empty() {
            return (CfgValue::False, "any()".to_string());
        }
        let mut has_true = false;
        let mut has_unknown = false;
        let mut arg_strings = Vec::new();
        for arg in sub_args {
            let (v, s) = eval_predicate(&arg, src, declared_features);
            arg_strings.push(s);
            match v {
                CfgValue::True => has_true = true,
                CfgValue::Unknown => has_unknown = true,
                CfgValue::False => {}
            }
        }
        let val = if has_true {
            CfgValue::True
        } else if has_unknown {
            CfgValue::Unknown
        } else {
            CfgValue::False
        };
        return (val, format!("any({})", arg_strings.join(", ")));
    }

    // 4. feature = "x"
    if items.len() >= 3
        && items[0].kind() == "identifier"
        && items[0].utf8_text(src).ok() == Some("feature")
        && items[1].kind() == "="
        && items[2].kind() == "string_literal"
    {
        let raw_str = items[2].utf8_text(src).unwrap_or("");
        let feat = raw_str.trim_matches(|c| c == '"' || c == '\'');
        let cond_str = format!("feature = \"{feat}\"");
        // A declared feature can be on or off in a given run; an undeclared one never is.
        let val = match declared_features {
            Some(declared) if !declared.contains(feat) => CfgValue::False,
            _ => CfgValue::Unknown,
        };
        return (val, cond_str);
    }

    // 5. `test`: tests are only compiled with it set.
    if items.len() == 1
        && items[0].kind() == "identifier"
        && items[0].utf8_text(src).ok() == Some("test")
    {
        return (CfgValue::True, "test".to_string());
    }

    // 5b. The boolean literals: `cfg(false)` is never built, `cfg(true)` always is.
    if items.len() == 1 {
        match items[0].utf8_text(src).ok() {
            Some("false") => return (CfgValue::False, "false".to_string()),
            Some("true") => return (CfgValue::True, "true".to_string()),
            _ => {}
        }
    }

    // 6. Any other predicate (target_feature, unix, debug_assertions, miri, custom): Unknown
    (CfgValue::Unknown, full_text)
}

/// Whether the attribute names a Cargo feature (`feature = "..."`) anywhere in its predicate.
pub fn cfg_mentions_feature(node: Node, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier"
            && n.utf8_text(src).ok() == Some("feature")
            && n.next_sibling().is_some_and(|s| s.kind() == "=")
        {
            return true;
        }
        stack.extend(n.children(&mut cursor));
    }
    false
}

/// Evaluates a Rust `#[cfg(...)]` attribute node using tree-sitter AST traversal.
/// Returns `(CfgValue, cond_str)`.
pub fn evaluate_rust_cfg(
    node: Node,
    src: &[u8],
    declared_features: Option<&HashSet<String>>,
) -> (CfgValue, String) {
    let pred_nodes = get_predicate_nodes(node, src);
    eval_predicate(&pred_nodes, src, declared_features)
}

/// Whether a CI cfg decides the predicate of a `#[cfg_attr(<predicate>, ignore)]`
/// attribute node, and which way. `None` when the predicate names no CI cfg.
pub fn rust_cfg_ci_verdict(node: Node, src: &[u8]) -> Option<super::ci_condition::CiVerdict> {
    super::ci_condition::rust_cfg_predicate(&get_predicate_nodes(node, src), src)
}

/// Whether `node` is a `#[cfg(<predicate>)]` attribute whose item is left out of a CI
/// build. Any other attribute (`cfg_attr`, `doc`) is not one, whatever its text.
pub fn rust_cfg_leaves_out_in_ci(node: Node, src: &[u8]) -> bool {
    is_cfg_attribute(node, src)
        && super::ci_condition::rust_cfg_leaves_out_in_ci(&get_predicate_nodes(node, src), src)
}

/// Whether `node` is a `#[cfg(<predicate>)]` attribute whose item is left out of a test
/// build (`not(test)`).
pub fn rust_cfg_leaves_out_of_tests(node: Node, src: &[u8]) -> bool {
    is_cfg_attribute(node, src)
        && super::ci_condition::rust_cfg_leaves_out_of_tests(&get_predicate_nodes(node, src), src)
}

/// Whether an attribute node (`attribute_item` or `attribute`) is named `cfg`.
fn is_cfg_attribute(node: Node, src: &[u8]) -> bool {
    let attribute = if node.kind() == "attribute" {
        Some(node)
    } else {
        let mut cursor = node.walk();
        let found = node.children(&mut cursor).find(|c| c.kind() == "attribute");
        found
    };
    attribute
        .and_then(|a| a.child(0))
        .and_then(|name| name.utf8_text(src).ok())
        == Some("cfg")
}

/// Combined collection rules across supported runners.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerCollectionRules {
    pub pytest: PytestCollectionRules,
    pub js: JsCollectionRules,
    pub rust: RustCollectionRules,
}

impl RunnerCollectionRules {
    #[cfg(test)]
    pub fn from_files<F>(reader: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        Self::from_files_with_manifests(reader, &["Cargo.toml".to_string()])
    }

    /// As `Self::from_files`, with every tracked path of the side being read: each
    /// `Cargo.toml` and nested JavaScript manifest is read, test modules are followed
    /// from their targets, and a language with no manifest anywhere is known as such.
    pub fn from_tree<F>(mut reader: F, tracked: &[String]) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let manifests: Vec<String> = tracked
            .iter()
            .filter(|f| *f == "Cargo.toml" || f.ends_with("/Cargo.toml"))
            .cloned()
            .collect();
        let mut rules = Self::from_files_with_manifests(&mut reader, &manifests);
        rules.rust.read_tree(tracked, &mut reader);

        let mut sign = false;
        for path in tracked {
            let (dir, name) = match path.rfind('/') {
                Some(i) => (&path[..i], &path[i + 1..]),
                None => ("", path.as_str()),
            };
            if !is_js_runner_sign(name) || format!("/{path}").contains("/node_modules/") {
                continue;
            }
            sign = true;
            if dir.is_empty() {
                continue;
            }
            let configures = match name {
                "package.json" => reader(path)
                    .is_some_and(|src| JsCollectionRules::package_configures_runner(&src)),
                "deno.json" | "deno.jsonc" => false,
                _ => !is_script_config(name, &["vite.config"]),
            };
            if configures && !rules.js.nested_configs.iter().any(|d| d == dir) {
                rules.js.nested_configs.push(dir.to_string());
            }
        }
        rules.js.manifest_seen = Some(sign);
        rules
    }

    /// As `Self::from_files`, also reading each `Cargo.toml` in `manifest_paths` so
    /// feature-gated tests resolve against their own crate.
    pub fn from_files_with_manifests<F>(mut reader: F, manifest_paths: &[String]) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut rules = Self::default();

        // 1. Pytest
        // pytest reads one file, the first of these that configures it. A `pytest.ini`
        // does even when it is empty, so it shadows every other file.
        for file in &["pytest.ini", "pyproject.toml", "tox.ini", "setup.cfg"] {
            let Some(src) = reader(file) else {
                continue;
            };
            let mut parsed = if *file == "pyproject.toml" {
                PytestCollectionRules::parse_pyproject_toml(&src)
            } else {
                PytestCollectionRules::parse_ini(&src)
            };
            parsed.configured |= *file == "pytest.ini";
            if parsed.configured {
                rules.pytest = parsed;
                break;
            }
        }

        // 2. JS / Jest / Vitest / Mocha
        if let Some(src) = reader("package.json") {
            rules.js.merge_package_json(&src);
        }
        if let Some(src) = reader("jest.config.json") {
            rules.js.merge_jest_config_json(&src);
        }

        // Check dynamic / unparseable Jest configs
        for f in &[
            "jest.config.js",
            "jest.config.ts",
            "jest.config.mjs",
            "jest.config.cjs",
            "jest.config.mts",
            "jest.config.cts",
        ] {
            if reader(f).is_some() {
                rules.js.unparseable_config = Some((*f).to_string());
                break;
            }
        }

        // Check dynamic / unparseable Vitest configs
        for f in &[
            "vitest.config.ts",
            "vitest.config.js",
            "vitest.config.mjs",
            "vitest.config.cjs",
            "vitest.config.mts",
            "vitest.config.cts",
            "vitest.config.json",
        ] {
            if reader(f).is_some() {
                rules.js.unparseable_config = Some((*f).to_string());
                break;
            }
        }

        // Check Mocha config files (.mocharc*)
        for f in &[
            ".mocharc.json",
            ".mocharc.js",
            ".mocharc.cjs",
            ".mocharc.mjs",
            ".mocharc.yaml",
            ".mocharc.yml",
            ".mocharc.jsonc",
            ".mocharc",
        ] {
            if reader(f).is_some() {
                rules.js.mocha_detected = Some(format!("mocha configuration file detected: {f}"));
                break;
            }
        }

        // Check a second runner's configuration file (Cypress, Playwright)
        for stem in SECOND_RUNNER_CONFIG_STEMS {
            for ext in SCRIPT_EXTENSIONS {
                let f = format!("{stem}.{ext}");
                if rules.js.second_runner.is_none() && reader(&f).is_some() {
                    rules.js.second_runner = Some(format!("{stem} file detected: {f}"));
                }
            }
        }

        // 3. Rust Cargo.toml
        let root_manifest = reader("Cargo.toml");
        if let Some(src) = &root_manifest {
            rules.rust = RustCollectionRules::parse_cargo_toml(src);
            rules.rust.add_manifest("Cargo.toml", src);
        }
        for path in manifest_paths.iter().filter(|p| p.as_str() != "Cargo.toml") {
            if let Some(src) = reader(path) {
                rules.rust.add_manifest(path, &src);
            }
        }

        rules
    }
}

/// Detailed status of checking runner collection for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerCollectionStatus {
    Collected,
    NotCollected,
    Unknown(String),
    /// Nothing tracked in the repository could run a test in this language: the file is
    /// left out of the count, and a note says so with this reason.
    NoRunner(String),
}

/// Whether the go tool ignores a `.go` file whatever its content: a file or directory
/// name starting with `.` or `_`, a directory named `testdata`, or a package below a
/// `vendor` directory, which `./...` never matches. Code directly inside a directory
/// named `vendor` is an ordinary package.
fn go_tool_ignores(norm: &str) -> bool {
    let mut parts: Vec<&str> = norm.split('/').collect();
    let file = parts.pop().unwrap_or("");
    if file.starts_with(['.', '_']) {
        return true;
    }
    let depth = parts.len();
    parts.iter().enumerate().any(|(i, dir)| {
        dir.starts_with('_')
            || (dir.starts_with('.') && !matches!(*dir, "." | ".."))
            || *dir == "testdata"
            || (*dir == "vendor" && i + 1 < depth)
    })
}

/// Evaluates whether a file path is collected by its language runner given repository vocabulary.
///
/// `NotCollected` only when a parsed runner configuration, or a rule the language fixes
/// (`_test.go`, Cargo's target layout), excludes the file. `NoRunner` only when the
/// listed tree holds no manifest a runner of the language needs. Everything else that is
/// not known to be collected is `Unknown`, with a reason that quotes no configured value.
pub fn check_runner_collected(
    path: &str,
    vocab: &crate::ast::AssertVocabulary,
) -> RunnerCollectionStatus {
    // 1. Explicit discipline.toml override
    if crate::ast::functions::declared_test_path(path, &vocab.test_paths) {
        return RunnerCollectionStatus::Collected;
    }

    let norm = path.replace('\\', "/");
    let lower = norm.to_ascii_lowercase();

    if lower.ends_with(".py") {
        let pytest = &vocab.runner_rules.pytest;
        if pytest.is_collected(&norm) {
            RunnerCollectionStatus::Collected
        } else if pytest.configured {
            RunnerCollectionStatus::NotCollected
        } else {
            // The runner may be unittest or Django, which collect by other rules.
            RunnerCollectionStatus::Unknown("no pytest configuration found".to_string())
        }
    } else if js_runner_extension(&lower) {
        let js = &vocab.runner_rules.js;
        match js.is_collected(&norm) {
            JsCollectionResult::Collected => RunnerCollectionStatus::Collected,
            JsCollectionResult::NotCollected => RunnerCollectionStatus::NotCollected,
            JsCollectionResult::Unknown(_) if js.no_runner_sign() => {
                RunnerCollectionStatus::NoRunner(
                    "no JavaScript package manifest or runner configuration in the repository"
                        .to_string(),
                )
            }
            JsCollectionResult::Unknown(reason) => {
                RunnerCollectionStatus::Unknown(js.unknown_kind().unwrap_or(reason))
            }
        }
    } else if lower.ends_with(".rs") {
        match vocab.runner_rules.rust.status(&norm) {
            RustCollection::Collected => RunnerCollectionStatus::Collected,
            RustCollection::NotCollected => RunnerCollectionStatus::NotCollected,
            RustCollection::Unknown(reason) => RunnerCollectionStatus::Unknown(reason.to_string()),
        }
    } else if lower.ends_with(".go") {
        if lower.ends_with("_test.go") && !go_tool_ignores(&norm) {
            RunnerCollectionStatus::Collected
        } else {
            RunnerCollectionStatus::NotCollected
        }
    } else {
        // Java, Kotlin, C#, Scala, Swift, Objective-C, Ruby, PHP, C / C++: no runner
        // configuration is read for these, so nothing here can exclude the file.
        RunnerCollectionStatus::Unknown("no runner model for this language".to_string())
    }
}

/// Whether the tests a pack finds in the file count: yes unless a parsed runner
/// configuration excludes it. A file whose collection is not determined counts.
pub fn is_runner_collected(path: &str, vocab: &crate::ast::AssertVocabulary) -> bool {
    !matches!(
        check_runner_collected(path, vocab),
        RunnerCollectionStatus::NotCollected | RunnerCollectionStatus::NoRunner(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pytest_defaults_and_custom() {
        let def = PytestCollectionRules::default();
        assert!(def.is_collected("tests/test_o.py"));
        assert!(def.is_collected("tests/o_test.py"));
        assert!(!def.is_collected("tests/o_checks.py"));
        assert!(!def.is_collected("tests/helpers.py"));

        // Custom python_files
        let custom_toml = r#"
[tool.pytest.ini_options]
python_files = ["*_checks.py", "test_*.py"]
"#;
        let rules = PytestCollectionRules::parse_pyproject_toml(custom_toml);
        assert!(rules.is_collected("tests/o_checks.py"));
        assert!(rules.is_collected("tests/test_o.py"));
        assert!(!rules.is_collected("tests/helpers.py"));

        // Custom testpaths
        let testpaths_ini = r#"
[pytest]
testpaths = tests integration
python_files = test_*.py
"#;
        let ini_rules = PytestCollectionRules::parse_ini(testpaths_ini);
        assert!(ini_rules.is_collected("tests/test_o.py"));
        assert!(ini_rules.is_collected("integration/test_o.py"));
        assert!(!ini_rules.is_collected("other/test_o.py"));
    }

    #[test]
    fn test_js_defaults_and_custom() {
        // Default rules with no config parsed -> Unknown("no runner config found")
        let def = JsCollectionRules::default();
        assert_eq!(
            def.is_collected("add.test.js"),
            JsCollectionResult::Unknown("no runner config found".to_string())
        );

        // Parsed Jest config with empty options uses defaults
        let pkg_default = r#"{"jest": {}}"#;
        let mut jest_def = JsCollectionRules::default();
        jest_def.merge_package_json(pkg_default);
        assert_eq!(
            jest_def.is_collected("add.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            jest_def.is_collected("src/calc.spec.ts"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            jest_def.is_collected("__tests__/helper.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            jest_def.is_collected("add.check.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            jest_def.is_collected("src/index.js"),
            JsCollectionResult::NotCollected
        );

        // Custom testMatch in package.json
        let pkg = r#"{"jest": {"testMatch": ["**/*.check.js"]}}"#;
        let mut custom = JsCollectionRules::default();
        custom.merge_package_json(pkg);
        assert_eq!(
            custom.is_collected("add.check.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            custom.is_collected("add.test.js"),
            JsCollectionResult::NotCollected
        );
    }

    #[test]
    fn test_jest_testmatch_rootdir_and_extglob() {
        // `<rootDir>/` is the config directory: repo-relative paths match it.
        let mut rootdir = JsCollectionRules::default();
        rootdir.merge_package_json(r#"{"jest": {"testMatch": ["<rootDir>/test/**/*.test.js"]}}"#);
        rootdir.compile_patterns();
        assert_eq!(
            rootdir.is_collected("test/a.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rootdir.is_collected("src/a.test.js"),
            JsCollectionResult::NotCollected
        );

        // Extglob cannot be evaluated: Unknown with a reason, never a silent drop.
        let mut extglob = JsCollectionRules::default();
        extglob
            .merge_package_json(r#"{"jest": {"testMatch": ["**/?(*.)+(spec|test).[jt]s?(x)"]}}"#);
        extglob.compile_patterns();
        assert_eq!(
            extglob.is_collected("test/a.test.js"),
            JsCollectionResult::Unknown(
                "glob pattern uses extglob, cannot evaluate statically: '**/?(*.)+(spec|test).[jt]s?(x)'"
                    .to_string()
            )
        );

        // A bracket class holding parens is not extglob.
        assert!(!has_extglob("test/[?()].test.js"));
        assert!(has_extglob("**/*.+(test).js"));

        // Control: a plain glob still decides.
        let mut plain = JsCollectionRules::default();
        plain.merge_package_json(r#"{"jest": {"testMatch": ["**/test/**/*.test.js"]}}"#);
        plain.compile_patterns();
        assert_eq!(
            plain.is_collected("test/a.test.js"),
            JsCollectionResult::Collected
        );
    }

    #[test]
    fn test_jest_root_dir_and_roots() {
        let rules = |pkg: &str| {
            let mut r = JsCollectionRules::default();
            r.merge_package_json(pkg);
            r
        };

        // `<rootDir>` stands for the configured `rootDir`, not the repository root.
        let rd = rules(
            r#"{"jest": {"rootDir": "packages/a", "testMatch": ["<rootDir>/test/**/*.test.js"]}}"#,
        );
        assert_eq!(
            rd.is_collected("packages/a/test/x.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rd.is_collected("test/x.test.js"),
            JsCollectionResult::NotCollected
        );

        // Default conventions apply only under `rootDir` (the default `roots`).
        let rd_default = rules(r#"{"jest": {"rootDir": "./packages/a/"}}"#);
        assert_eq!(
            rd_default.is_collected("packages/a/src/x.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rd_default.is_collected("packages/b/src/x.test.js"),
            JsCollectionResult::NotCollected
        );

        // `roots` limits collection to the listed directories.
        let roots = rules(r#"{"jest": {"roots": ["<rootDir>/src", "lib"]}}"#);
        assert_eq!(
            roots.is_collected("src/a.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            roots.is_collected("lib/b.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            roots.is_collected("test/a.test.js"),
            JsCollectionResult::NotCollected
        );
        // `<rootDir>` itself as a root limits nothing.
        let whole = rules(r#"{"jest": {"roots": ["<rootDir>"]}}"#);
        assert_eq!(
            whole.is_collected("test/a.test.js"),
            JsCollectionResult::Collected
        );

        // Anything that cannot be a repository path is Unknown, never a silent drop.
        for pkg in [
            r#"{"jest": {"rootDir": "../shared"}}"#,
            r#"{"jest": {"rootDir": "/abs/path"}}"#,
            r#"{"jest": {"rootDir": 3}}"#,
            r#"{"jest": {"roots": "src"}}"#,
            r#"{"jest": {"roots": ["src/*"]}}"#,
            r#"{"vitest": {"root": "web"}}"#,
        ] {
            assert!(
                matches!(
                    rules(pkg).is_collected("test/a.test.js"),
                    JsCollectionResult::Unknown(_)
                ),
                "{pkg} must be Unknown"
            );
        }
    }

    #[test]
    fn test_js_unknown_reasons() {
        // 1. No runner config found
        let def = JsCollectionRules::default();
        assert_eq!(
            def.is_collected("test/foo.js"),
            JsCollectionResult::Unknown("no runner config found".to_string())
        );

        // 2. Mocha detected in package.json dependencies
        let pkg_mocha = r#"{"devDependencies": {"mocha": "^10.0.0"}}"#;
        let mut rules_mocha = JsCollectionRules::default();
        rules_mocha.merge_package_json(pkg_mocha);
        assert_eq!(
            rules_mocha.is_collected("test/foo.js"),
            JsCollectionResult::Unknown("mocha detected in package.json dependencies".to_string())
        );

        // 3. Mocha detected via .mocharc* file
        let runner_rules = RunnerCollectionRules::from_files(|path| {
            if path == ".mocharc.json" {
                Some("{}".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            runner_rules.js.is_collected("test/foo.js"),
            JsCollectionResult::Unknown(
                "mocha configuration file detected: .mocharc.json".to_string()
            )
        );

        // 4. vitest.config.* exists (cannot parse statically)
        let runner_rules_vitest = RunnerCollectionRules::from_files(|path| {
            if path == "vitest.config.ts" {
                Some("export default {}".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            runner_rules_vitest.js.is_collected("test/foo.ts"),
            JsCollectionResult::Unknown("cannot parse statically: vitest.config.ts".to_string())
        );

        // 5. jest.config.js exists (cannot parse statically)
        let runner_rules_jest = RunnerCollectionRules::from_files(|path| {
            if path == "jest.config.js" {
                Some("module.exports = {}".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            runner_rules_jest.js.is_collected("test/foo.js"),
            JsCollectionResult::Unknown("cannot parse statically: jest.config.js".to_string())
        );

        // 6. Configured regex fails to compile
        let mut bad_regex = JsCollectionRules::default();
        bad_regex.merge_package_json(r#"{"jest": {"testRegex": ["[unclosed"]}}"#);
        assert!(matches!(
            bad_regex.is_collected("test/foo.js"),
            JsCollectionResult::Unknown(reason) if reason.contains("[unclosed")
        ));

        // 7. Configured glob fails to compile
        let mut bad_glob = JsCollectionRules::default();
        bad_glob.merge_package_json(r#"{"jest": {"testMatch": ["***["]}}"#);
        assert!(matches!(
            bad_glob.is_collected("test/foo.js"),
            JsCollectionResult::Unknown(reason) if reason.contains("***[")
        ));

        // 8. Known Jest config excluding a file -> NotCollected
        let mut known_jest = JsCollectionRules::default();
        known_jest.merge_jest_config_json(r#"{"testMatch": ["**/*.spec.js"]}"#);
        assert_eq!(
            known_jest.is_collected("test/foo.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            known_jest.is_collected("test/foo.spec.js"),
            JsCollectionResult::Collected
        );
    }

    fn vocab_with(files: &[(&str, &str)]) -> crate::ast::AssertVocabulary {
        crate::ast::AssertVocabulary {
            runner_rules: RunnerCollectionRules::from_files(|p| {
                files
                    .iter()
                    .find(|(name, _)| *name == p)
                    .map(|(_, content)| content.to_string())
            }),
            ..Default::default()
        }
    }

    fn is_unknown(path: &str, vocab: &crate::ast::AssertVocabulary) -> bool {
        matches!(
            check_runner_collected(path, vocab),
            RunnerCollectionStatus::Unknown(_)
        )
    }

    #[test]
    fn test_language_without_a_runner_model_is_unknown() {
        let vocab = vocab_with(&[]);
        // No runner model: never a decision, whatever the path looks like.
        for path in [
            "MyAppTests/LoginTests.swift",
            "Sources/App/Login.swift",
            "src/test/java/FooTest.java",
            "src/main/java/Foo.java",
            "app/src/test/kotlin/FooTest.kt",
            "Tests/FooTests.cs",
            "src/test/scala/FooSpec.scala",
            "AppTests/FooTests.m",
            "spec/foo_spec.rb",
            "tests/FooTest.php",
            "tests/foo_test.c",
            "tests/foo_test.cpp",
        ] {
            assert!(is_unknown(path, &vocab), "{path} must be Unknown");
        }
        // Control: a language with a runner model still decides.
        assert_eq!(
            check_runner_collected("pkg/service.go", &vocab),
            RunnerCollectionStatus::NotCollected
        );
        assert_eq!(
            check_runner_collected("pkg/service_test.go", &vocab),
            RunnerCollectionStatus::Collected
        );
        // Control: a declared test path is collected in any language.
        let declared = crate::ast::AssertVocabulary {
            test_paths: vec!["MyAppTests/**".to_string()],
            ..Default::default()
        };
        assert_eq!(
            check_runner_collected("MyAppTests/LoginTests.swift", &declared),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_mts_and_cts_follow_the_js_rules() {
        let jest = vocab_with(&[("package.json", r#"{"jest": {}}"#)]);
        for ext in ["mts", "cts"] {
            assert_eq!(
                check_runner_collected(&format!("src/a.test.{ext}"), &jest),
                RunnerCollectionStatus::Collected,
                "{ext}"
            );
            assert_eq!(
                check_runner_collected(&format!("src/index.{ext}"), &jest),
                RunnerCollectionStatus::NotCollected,
                "{ext}"
            );
            // No runner configuration: unknown, as for `.ts`.
            assert_eq!(
                check_runner_collected(&format!("src/a.test.{ext}"), &vocab_with(&[])),
                RunnerCollectionStatus::Unknown("no runner config found".to_string()),
                "{ext}"
            );
        }
    }

    #[test]
    fn test_python_without_pytest_configuration_is_unknown() {
        // No pytest configuration: the runner may be unittest or Django.
        for files in [
            vec![],
            vec![("pyproject.toml", "[project]\nname = \"p\"\n")],
            vec![("setup.cfg", "[metadata]\nname = p\n")],
            vec![("tox.ini", "[tox]\nenvlist = py3\n")],
        ] {
            let vocab = vocab_with(&files);
            assert!(is_unknown("polls/tests.py", &vocab), "{files:?}");
            assert!(is_unknown("tests/helpers.py", &vocab), "{files:?}");
            // A file every Python runner's defaults pick up is collected either way.
            assert_eq!(
                check_runner_collected("tests/test_o.py", &vocab),
                RunnerCollectionStatus::Collected,
                "{files:?}"
            );
        }

        // A pytest configuration is present: its rules decide, defaults included.
        for files in [
            vec![(
                "pyproject.toml",
                "[tool.pytest.ini_options]\naddopts = \"-q\"\n",
            )],
            vec![("pytest.ini", "")],
            vec![("pytest.ini", "[pytest]\naddopts = -q\n")],
            vec![("setup.cfg", "[tool:pytest]\naddopts = -q\n")],
            vec![("tox.ini", "[pytest]\naddopts = -q\n")],
            vec![(
                "pyproject.toml",
                "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
            )],
        ] {
            let vocab = vocab_with(&files);
            assert_eq!(
                check_runner_collected("polls/tests.py", &vocab),
                RunnerCollectionStatus::NotCollected,
                "{files:?}"
            );
            assert_eq!(
                check_runner_collected("tests/test_o.py", &vocab),
                RunnerCollectionStatus::Collected,
                "{files:?}"
            );
        }
    }

    #[test]
    fn test_unknown_reason_does_not_quote_a_configured_value() {
        for pkg in [
            r#"{"jest": {"testRegex": ["[marker-value"]}}"#,
            r#"{"jest": {"testMatch": ["***[marker-value"]}}"#,
            r#"{"jest": {"testMatch": ["**/+(marker-value).js"]}}"#,
            r#"{"jest": {"rootDir": "../marker-value"}}"#,
            r#"{"jest": {"roots": ["marker-value/*"]}}"#,
        ] {
            let vocab = vocab_with(&[("package.json", pkg)]);
            match check_runner_collected("test/a.test.js", &vocab) {
                RunnerCollectionStatus::Unknown(reason) => {
                    assert!(!reason.contains("marker-value"), "{pkg}: {reason}");
                    assert!(!reason.is_empty(), "{pkg}");
                }
                other => panic!("{pkg} must be Unknown, got {other:?}"),
            }
        }
    }

    fn eval_cfg_str(code: &str, features: Option<&HashSet<String>>) -> (CfgValue, String) {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let attr = tree.root_node().child(0).unwrap();
        evaluate_rust_cfg(attr, code.as_bytes(), features)
    }

    #[test]
    fn test_rust_cfg_rules() {
        let mut features = HashSet::new();
        features.insert("declared_feat".to_string());

        // 1. feature = "x"
        // Declared feature -> may be on or off
        let (val, _) = eval_cfg_str(r#"#[cfg(feature = "declared_feat")]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        // Undeclared feature -> False (unconditional ignore)
        let (val, _) = eval_cfg_str(r#"#[cfg(feature = "undeclared")]"#, Some(&features));
        assert_eq!(val, CfgValue::False);

        // Unknown manifest -> Unknown
        let (val, _) = eval_cfg_str(r#"#[cfg(feature = "declared_feat")]"#, None);
        assert_eq!(val, CfgValue::Unknown);

        // 2. not(p)
        // not(feature = "undeclared"): undeclared is False -> not(False) is True -> NOT an unconditional ignore!
        let (val, _) = eval_cfg_str(r#"#[cfg(not(feature = "undeclared"))]"#, Some(&features));
        assert_eq!(val, CfgValue::True);

        // not(feature = "declared_feat"): the feature may be off, so the test may run -> Unknown
        let (val, _) = eval_cfg_str(r#"#[cfg(not(feature = "declared_feat"))]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        // not(unix): unix is Unknown -> not(Unknown) is Unknown
        let (val, _) = eval_cfg_str(r#"#[cfg(not(unix))]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        // 3. all and any Kleene three-valued logic
        // all() -> True
        let (val, _) = eval_cfg_str(r#"#[cfg(all())]"#, Some(&features));
        assert_eq!(val, CfgValue::True);

        // any() -> False
        let (val, _) = eval_cfg_str(r#"#[cfg(any())]"#, Some(&features));
        assert_eq!(val, CfgValue::False);

        // all(any()) -> False
        let (val, _) = eval_cfg_str(r#"#[cfg(all(any()))]"#, Some(&features));
        assert_eq!(val, CfgValue::False);

        // any(unix, any()) -> Unknown (neither True, but unix is Unknown)
        let (val, _) = eval_cfg_str(r#"#[cfg(any(unix, any()))]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        // all(feature = "undeclared", unix) -> False (False AND Unknown is False in Kleene)
        let (val, _) = eval_cfg_str(
            r#"#[cfg(all(feature = "undeclared", unix))]"#,
            Some(&features),
        );
        assert_eq!(val, CfgValue::False);

        // all(feature = "declared_feat", unix) -> Unknown
        let (val, _) = eval_cfg_str(
            r#"#[cfg(all(feature = "declared_feat", unix))]"#,
            Some(&features),
        );
        assert_eq!(val, CfgValue::Unknown);

        // 4. Other predicates -> Unknown
        let (val, _) = eval_cfg_str(r#"#[cfg(target_feature = "avx2")]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        let (val, _) = eval_cfg_str(r#"#[cfg(unix)]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        let (val, _) = eval_cfg_str(r#"#[cfg(test)]"#, Some(&features));
        assert_eq!(val, CfgValue::True);
        let (val, _) = eval_cfg_str(r#"#[cfg(not(test))]"#, Some(&features));
        assert_eq!(val, CfgValue::False);

        let (val, _) = eval_cfg_str(r#"#[cfg(debug_assertions)]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);

        let (val, _) = eval_cfg_str(r#"#[cfg(miri)]"#, Some(&features));
        assert_eq!(val, CfgValue::Unknown);
    }

    #[test]
    fn test_rust_virtual_workspace_owning_crate() {
        use crate::ast::LanguagePack;
        let files: std::collections::HashMap<&str, &str> = [
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n",
            ),
            (
                "crates/a/Cargo.toml",
                "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[features]\nx = []\n",
            ),
            ("crates/b/Cargo.toml", "[package\nname = "),
        ]
        .into_iter()
        .collect();
        let manifests: Vec<String> = files.keys().map(|k| k.to_string()).collect();
        let vocab = crate::ast::AssertVocabulary {
            runner_rules: RunnerCollectionRules::from_files_with_manifests(
                |p| files.get(p).map(|s| s.to_string()),
                &manifests,
            ),
            ..Default::default()
        };
        let pack = crate::ast::rust::RustPack;
        let extract = |path: &str, feat: &str| {
            let src = format!(
                "#[cfg(feature = \"{feat}\")]\n#[test]\nfn test_it() {{ assert_eq!(1, 1); }}\n"
            );
            pack.extract(path, &src, &vocab).unwrap()
        };

        // Declared in the member crate: built when the feature is on, a conditional skip.
        let facts = extract("crates/a/tests/t.rs", "x");
        assert_eq!(facts.tests.len(), 1);
        assert!(!facts.tests[0].ignored);
        assert_eq!(
            facts.tests[0].conditional_ignore.as_deref(),
            Some(r#"feature = "x""#)
        );

        // Not declared in the member crate, whose manifest was read: never compiled.
        let facts = extract("crates/a/tests/t.rs", "y");
        assert!(facts.tests[0].ignored);

        // The member's manifest does not parse: unknown, a conditional skip with a note.
        let facts = extract("crates/b/tests/t.rs", "x");
        assert!(!facts.tests[0].ignored);
        assert!(facts.tests[0].conditional_ignore.is_some());
        assert!(facts
            .notes
            .iter()
            .any(|n| n.contains("crates/b/Cargo.toml")));

        // No owning package manifest (the root is a virtual workspace): unknown.
        let facts = extract("tests/t.rs", "x");
        assert!(!facts.tests[0].ignored);
        assert!(facts.tests[0].conditional_ignore.is_some());
    }

    #[test]
    fn test_rust_cfg_without_feature_does_not_skip() {
        use crate::ast::LanguagePack;
        let src = r#"
#[cfg(test)]
mod tests {
    #[test]
    fn in_cfg_test_mod() { assert_eq!(1, 1); }

    #[cfg(unix)]
    #[test]
    fn platform() { assert_eq!(1, 1); }

    #[cfg(target_feature = "avx2")]
    #[test]
    fn simd() { assert_eq!(1, 1); }

    #[cfg(not(test))]
    #[test]
    fn never_built() { assert_eq!(1, 1); }

    #[cfg_attr(test, ignore)]
    #[test]
    fn always_ignored() { assert_eq!(1, 1); }
}
"#;
        let vocab = crate::ast::AssertVocabulary {
            runner_rules: RunnerCollectionRules::from_files(|p| {
                (p == "Cargo.toml")
                    .then(|| "[package]\nname = \"p\"\nversion = \"0.1.0\"\n".to_string())
            }),
            ..Default::default()
        };
        let facts = crate::ast::rust::RustPack
            .extract("src/lib.rs", src, &vocab)
            .unwrap();
        let get = |n: &str| {
            facts
                .tests
                .iter()
                .find(|t| t.name.rsplit("::").next() == Some(n))
                .unwrap()
        };
        for name in ["in_cfg_test_mod", "platform", "simd"] {
            assert!(!get(name).ignored, "{name}");
            assert_eq!(get(name).conditional_ignore, None, "{name}");
        }
        assert!(get("never_built").ignored);
        assert!(get("always_ignored").ignored);
    }

    #[test]
    fn test_rust_integration_and_cfg_features() {
        let cargo = r#"
[package]
name = "p"
version = "0.1.0"

[features]
default = []
declared_feat = []

[dependencies]
opt_dep = { version = "1.0", optional = true }

[[test]]
name = "custom"
path = "tests/custom/entry.rs"
"#;
        let rules = RustCollectionRules::parse_cargo_toml(cargo);
        assert!(rules.is_collected("src/lib.rs"));
        assert!(rules.is_collected("src/models/user.rs"));
        assert!(rules.is_collected("tests/test_a.rs"));
        assert!(rules.is_collected("tests/foo/main.rs"));
        assert!(rules.is_collected("tests/custom/entry.rs"));
        assert!(rules.is_collected("tests/common/mod.rs"));
        assert!(rules.is_collected("tests/common/helpers.rs"));
        assert!(rules.is_collected("tests/it/foo.rs"));
        assert!(rules.is_collected("tests/helpers.rs"));
        assert!(!rules.is_collected("other/helpers.rs"));

        // Cfg features
        assert!(rules.declared_features.contains("declared_feat"));
        assert!(rules.declared_features.contains("opt_dep"));
        assert!(!rules.declared_features.contains("never"));

        let features = Some(&rules.declared_features);
        // Undeclared feature -> unconditional ignore
        assert_eq!(
            eval_cfg_str(r#"#[cfg(feature = "never")]"#, features).0,
            CfgValue::False
        );
        // Declared feature -> may be on or off
        assert_eq!(
            eval_cfg_str(r#"#[cfg(feature = "declared_feat")]"#, features).0,
            CfgValue::Unknown
        );
        // Optional dependency is an implicit feature
        assert_eq!(
            eval_cfg_str(r#"#[cfg(feature = "opt_dep")]"#, features).0,
            CfgValue::Unknown
        );
        // any() -> unconditional ignore
        assert_eq!(
            eval_cfg_str(r#"#[cfg(any())]"#, features).0,
            CfgValue::False
        );
        // all(any()) -> unconditional ignore
        assert_eq!(
            eval_cfg_str(r#"#[cfg(all(any()))]"#, features).0,
            CfgValue::False
        );
        // not(undeclared) -> compiled
        assert_eq!(
            eval_cfg_str(r#"#[cfg(not(feature = "never"))]"#, features).0,
            CfgValue::True
        );
    }

    // ---- #557 / #577: configuration shapes the model read differently from the runner

    fn tree(files: &[(&str, &str)]) -> crate::ast::AssertVocabulary {
        let tracked: Vec<String> = files.iter().map(|(name, _)| name.to_string()).collect();
        crate::ast::AssertVocabulary {
            runner_rules: RunnerCollectionRules::from_tree(
                |p| {
                    files
                        .iter()
                        .find(|(name, _)| *name == p)
                        .map(|(_, content)| content.to_string())
                },
                &tracked,
            ),
            ..Default::default()
        }
    }

    fn jest_rules(config: &str) -> JsCollectionRules {
        let mut rules = JsCollectionRules::default();
        rules.merge_package_json(&format!(r#"{{"jest": {config}}}"#));
        rules
    }

    const PKG: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n";

    #[test]
    fn test_pytest_testpaths_forms() {
        // `./tests` and `tests/` are the directory `tests`.
        assert!(testpath_holds("./tests", "tests/test_a.py"));
        assert!(testpath_holds("tests/", "tests/sub/test_a.py"));
        assert!(!testpath_holds("./tests", "other/test_a.py"));
        // A longer name is not under the directory.
        assert!(!testpath_holds("tests", "tests_old/test_a.py"));
        // A glob names directories; `*` stays in one, `**` crosses them.
        assert!(testpath_holds("pkgs/*/tests", "pkgs/x/tests/test_a.py"));
        assert!(!testpath_holds("pkgs/*/tests", "pkgs/x/y/tests/test_a.py"));
        assert!(testpath_holds("pkgs/**/tests", "pkgs/x/y/tests/test_a.py"));
        assert!(!testpath_holds("pkgs/*/tests", "other/test_a.py"));
        // A file entry.
        assert!(testpath_holds("tests/test_a.py", "tests/test_a.py"));
        // An entry that is not a repository path excludes nothing.
        assert!(testpath_holds("/abs/tests", "other/test_a.py"));
        assert!(testpath_holds("../shared", "other/test_a.py"));
    }

    #[test]
    fn test_pytest_reads_the_first_file_that_configures_it() {
        let pyproject = "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n";
        let other = "testpaths = other\n";
        let collected = |files: &[(&str, &str)], path: &str| {
            check_runner_collected(path, &vocab_with(files)) == RunnerCollectionStatus::Collected
        };
        // An empty `pytest.ini` shadows `pyproject.toml`.
        let files = [("pytest.ini", ""), ("pyproject.toml", pyproject)];
        assert!(collected(&files, "other/test_o.py"));
        // `pyproject.toml` wins over `tox.ini` and `setup.cfg`.
        let tox = format!("[pytest]\n{other}");
        let cfg = format!("[tool:pytest]\n{other}");
        let files = [
            ("pyproject.toml", pyproject),
            ("tox.ini", tox.as_str()),
            ("setup.cfg", cfg.as_str()),
        ];
        assert!(collected(&files, "tests/test_a.py"));
        assert!(!collected(&files, "other/test_o.py"));
        // `tox.ini` wins over `setup.cfg`.
        let tox_tests = "[pytest]\ntestpaths = tests\n";
        let files = [("tox.ini", tox_tests), ("setup.cfg", cfg.as_str())];
        assert!(collected(&files, "tests/test_a.py"));
        assert!(!collected(&files, "other/test_o.py"));
        // A `pyproject.toml` with no pytest table configures nothing: the next file does.
        let files = [
            ("pyproject.toml", "[project]\nname = \"p\"\n"),
            ("setup.cfg", cfg.as_str()),
        ];
        assert!(collected(&files, "other/test_o.py"));
        assert!(!collected(&files, "tests/test_a.py"));
    }

    #[test]
    fn test_jest_regex_matches_the_path_from_a_leading_slash() {
        let rules = jest_rules(r#"{"testRegex": "/tests/.*\\.js$"}"#);
        assert_eq!(
            rules.is_collected("tests/a.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rules.is_collected("src/tests/a.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rules.is_collected("mytests/a.js"),
            JsCollectionResult::NotCollected
        );
        // Jest's own default, written as a regex.
        let default =
            jest_rules(r#"{"testRegex": "(/__tests__/.*|(\\.|/)(test|spec))\\.[jt]sx?$"}"#);
        assert_eq!(
            default.is_collected("test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            default.is_collected("__tests__/a.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            default.is_collected("src/contest.js"),
            JsCollectionResult::NotCollected
        );
    }

    #[test]
    fn test_jest_glob_shapes_that_cannot_be_evaluated_are_unknown() {
        for config in [
            // A group is alternation to the runner and literal characters here.
            r#"{"testMatch": ["**/*.(test|spec).(ts|js)"]}"#,
            // Jest matches the absolute path: this pattern is anchored nowhere.
            r#"{"testMatch": ["tests/**/*.test.js"]}"#,
            r#"{"testMatch": ["!tests/**"]}"#,
            // Keys that move collection somewhere that is not read.
            r#"{"projects": ["<rootDir>/packages/*"]}"#,
            r#"{"preset": "some-preset"}"#,
            r#"{"testPathIgnorePatterns": "/legacy/"}"#,
            r#"{"testPathIgnorePatterns": ["[unclosed"]}"#,
        ] {
            let rules = jest_rules(config);
            assert!(
                matches!(
                    rules.is_collected("tests/a.test.js"),
                    JsCollectionResult::Unknown(_)
                ),
                "{config} must be Unknown"
            );
            assert!(rules.unknown_kind().is_some(), "{config}");
        }
        assert!(has_group("**/*.(test|spec).js"));
        assert!(!has_group("**/[(]x[)].js"));

        let mut vitest = JsCollectionRules::default();
        vitest.merge_package_json(r#"{"vitest": {"exclude": ["**/legacy/**"]}}"#);
        assert!(matches!(
            vitest.is_collected("legacy/a.test.js"),
            JsCollectionResult::Unknown(_)
        ));
        let mut negated = JsCollectionRules::default();
        negated.merge_package_json(r#"{"vitest": {"include": ["**/*.test.js", "!**/legacy/**"]}}"#);
        assert!(matches!(
            negated.is_collected("legacy/a.test.js"),
            JsCollectionResult::Unknown(_)
        ));

        // Control: an anchored plain glob decides, and `null` is an absent key.
        let plain = jest_rules(r#"{"testMatch": ["**/*.test.js"], "preset": null}"#);
        assert_eq!(
            plain.is_collected("tests/a.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            plain.is_collected("tests/a.spec.js"),
            JsCollectionResult::NotCollected
        );
    }

    #[test]
    fn test_jest_default_names() {
        let rules = jest_rules("{}");
        for path in [
            "test.js",
            "src/spec.ts",
            "src/a.test.js",
            "src/a.spec.tsx",
            "__tests__/helper.js",
        ] {
            assert_eq!(
                rules.is_collected(path),
                JsCollectionResult::Collected,
                "{path}"
            );
        }
        for path in [
            "src/a.test.helper.js",
            "src/contest.js",
            "src/latest.js",
            "src/index.js",
            // Jest's default `testPathIgnorePatterns`.
            "node_modules/dep/a.test.js",
        ] {
            assert_eq!(
                rules.is_collected(path),
                JsCollectionResult::NotCollected,
                "{path}"
            );
        }
    }

    #[test]
    fn test_jest_relative_roots_resolve_against_root_dir() {
        let rules = jest_rules(r#"{"rootDir": "packages/a", "roots": ["src", "<rootDir>/lib"]}"#);
        assert_eq!(
            rules.is_collected("packages/a/src/x.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rules.is_collected("packages/a/lib/x.test.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            rules.is_collected("src/x.test.js"),
            JsCollectionResult::NotCollected
        );
        // Control: with no `rootDir` a relative root is repository-relative, as before.
        let plain = jest_rules(r#"{"roots": ["src"]}"#);
        assert_eq!(
            plain.is_collected("src/x.test.js"),
            JsCollectionResult::Collected
        );
        // A root that climbs out of the repository cannot be resolved.
        let out = jest_rules(r#"{"rootDir": "packages/a", "roots": ["../../../x"]}"#);
        assert!(matches!(
            out.is_collected("src/x.test.js"),
            JsCollectionResult::Unknown(_)
        ));
    }

    #[test]
    fn test_jest_ignore_patterns_and_negated_globs() {
        let ignore = jest_rules(r#"{"testPathIgnorePatterns": ["/legacy/", "<rootDir>/build/"]}"#);
        assert_eq!(
            ignore.is_collected("legacy/a.test.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            ignore.is_collected("build/a.test.js"),
            JsCollectionResult::NotCollected
        );
        // `<rootDir>/build/` is the top-level directory only.
        assert_eq!(
            ignore.is_collected("src/build/a.test.js"),
            JsCollectionResult::Collected
        );
        // A configured list replaces Jest's default.
        assert_eq!(
            ignore.is_collected("node_modules/dep/a.test.js"),
            JsCollectionResult::Collected
        );
        let under_root = jest_rules(
            r#"{"rootDir": "packages/a", "testPathIgnorePatterns": ["<rootDir>/build/"]}"#,
        );
        assert_eq!(
            under_root.is_collected("packages/a/build/a.test.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            under_root.is_collected("packages/a/src/a.test.js"),
            JsCollectionResult::Collected
        );

        // A negated glob drops what an earlier glob kept; a later plain glob keeps again.
        let negated = jest_rules(r#"{"testMatch": ["**/*.test.js", "!**/legacy/**"]}"#);
        assert_eq!(
            negated.is_collected("legacy/a.test.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            negated.is_collected("src/a.test.js"),
            JsCollectionResult::Collected
        );
        let reordered = jest_rules(r#"{"testMatch": ["!**/legacy/**", "**/*.test.js"]}"#);
        assert_eq!(
            reordered.is_collected("legacy/a.test.js"),
            JsCollectionResult::Collected
        );
        // Only negated globs: everything they do not drop.
        let only = jest_rules(r#"{"testMatch": ["!**/legacy/**"]}"#);
        assert_eq!(
            only.is_collected("src/anything.js"),
            JsCollectionResult::Collected
        );
        assert_eq!(
            only.is_collected("legacy/a.test.js"),
            JsCollectionResult::NotCollected
        );
    }

    #[test]
    fn test_second_runner_and_script_configurations_are_unknown() {
        let jest = r#""jest": {"testMatch": ["**/src/**/*.test.js"]}"#;
        for dep in ["cypress", "@playwright/test"] {
            let pkg = format!(r#"{{{jest}, "devDependencies": {{"{dep}": "1"}}}}"#);
            assert!(
                is_unknown("e2e/login.cy.js", &vocab_with(&[("package.json", &pkg)])),
                "{dep}"
            );
        }
        let pkg = format!("{{{jest}}}");
        for config in [
            "playwright.config.ts",
            "cypress.config.js",
            "jest.config.cts",
            "jest.config.mts",
        ] {
            assert!(
                is_unknown(
                    "e2e/login.cy.js",
                    &vocab_with(&[("package.json", &pkg), (config, "export default {}")])
                ),
                "{config}"
            );
        }
        // Control: Jest alone decides, and an unrelated dependency changes nothing.
        let pkg = format!(r#"{{{jest}, "devDependencies": {{"typescript": "5"}}}}"#);
        assert_eq!(
            check_runner_collected("e2e/login.cy.js", &vocab_with(&[("package.json", &pkg)])),
            RunnerCollectionStatus::NotCollected
        );
    }

    #[test]
    fn test_jest_preset_leaves_the_configurations_own_patterns_in_charge() {
        // The configuration's own `testMatch` / `testRegex` replaces the preset's.
        for config in [
            r#"{"preset": "ts-jest", "testMatch": ["**/src/**/*.test.js"]}"#,
            r#"{"preset": "ts-jest", "testRegex": "/src/.*\\.test\\.js$"}"#,
        ] {
            let rules = jest_rules(config);
            assert_eq!(
                rules.is_collected("src/a.test.js"),
                JsCollectionResult::Collected,
                "{config}"
            );
            assert_eq!(
                rules.is_collected("parked/a.test.js"),
                JsCollectionResult::NotCollected,
                "{config}"
            );
        }
        // With neither, the patterns may be the preset's.
        for config in [
            r#"{"preset": "ts-jest"}"#,
            r#"{"preset": "ts-jest", "roots": ["src"]}"#,
            r#"{"preset": "ts-jest", "testPathIgnorePatterns": ["/legacy/"]}"#,
        ] {
            assert!(
                matches!(
                    jest_rules(config).is_collected("src/a.test.js"),
                    JsCollectionResult::Unknown(_)
                ),
                "{config}"
            );
        }
        // An ignore list or roots the configuration sets itself still apply.
        let own = jest_rules(
            r#"{"preset": "ts-jest", "testMatch": ["**/*.test.js"], "testPathIgnorePatterns": ["/legacy/"], "roots": ["src", "legacy"]}"#,
        );
        assert_eq!(
            own.is_collected("legacy/a.test.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            own.is_collected("lib/a.test.js"),
            JsCollectionResult::NotCollected
        );
        assert_eq!(
            own.is_collected("src/a.test.js"),
            JsCollectionResult::Collected
        );
        // An unset ignore list may be the preset's: Jest's default is not assumed, so a
        // matched file stays collected. Without a preset the default applies.
        let unset = jest_rules(r#"{"preset": "ts-jest", "testMatch": ["**/*.test.js"]}"#);
        assert_eq!(
            unset.is_collected("node_modules/dep/a.test.js"),
            JsCollectionResult::Collected
        );
        let plain = jest_rules(r#"{"testMatch": ["**/*.test.js"]}"#);
        assert_eq!(
            plain.is_collected("node_modules/dep/a.test.js"),
            JsCollectionResult::NotCollected
        );
    }

    #[test]
    fn test_second_runner_only_changes_what_jest_leaves_out() {
        let pkg = r#"{"jest": {"testMatch": ["**/src/**/*.test.js"], "testPathIgnorePatterns": ["/legacy/"], "roots": ["src", "e2e"]}, "devDependencies": {"@playwright/test": "1"}}"#;
        let vocab = vocab_with(&[("package.json", pkg)]);
        assert_eq!(
            check_runner_collected("src/a.test.js", &vocab),
            RunnerCollectionStatus::Collected
        );
        // Not matched, ignored, or outside the roots: the other runner may run it.
        for path in [
            "e2e/login.spec.js",
            "src/legacy/a.test.js",
            "pw/login.spec.js",
        ] {
            assert!(is_unknown(path, &vocab), "{path}");
        }
        // Default conventions decide the same way.
        let defaults = vocab_with(&[(
            "package.json",
            r#"{"jest": {}, "devDependencies": {"cypress": "1"}}"#,
        )]);
        assert_eq!(
            check_runner_collected("src/a.test.js", &defaults),
            RunnerCollectionStatus::Collected
        );
        assert!(is_unknown("cypress/e2e/login.cy.js", &defaults));
        // With no Jest configuration at all, nothing decides.
        let alone = vocab_with(&[("package.json", r#"{"devDependencies": {"cypress": "1"}}"#)]);
        assert!(is_unknown("src/a.test.js", &alone));
    }

    #[test]
    fn test_nested_runner_configuration_is_unknown() {
        let root = r#"{"jest": {"testMatch": ["**/*.spec.js"]}}"#;
        let vocab = tree(&[
            ("package.json", root),
            ("packages/a/package.json", r#"{"jest": {}}"#),
            ("packages/b/package.json", r#"{"name": "b"}"#),
            ("packages/c/vitest.config.ts", "export default {}"),
            ("node_modules/dep/package.json", r#"{"jest": {}}"#),
        ]);
        assert!(is_unknown("packages/a/src/x.test.js", &vocab));
        assert!(is_unknown("packages/c/src/x.test.js", &vocab));
        // A nested manifest that configures no runner leaves the root one in charge.
        assert_eq!(
            check_runner_collected("packages/b/src/x.test.js", &vocab),
            RunnerCollectionStatus::NotCollected
        );
        assert_eq!(
            check_runner_collected("src/x.spec.js", &vocab),
            RunnerCollectionStatus::Collected
        );
        // A similarly named sibling directory is not under the nested package.
        assert_eq!(
            check_runner_collected("packages/ab/src/x.test.js", &vocab),
            RunnerCollectionStatus::NotCollected
        );
    }

    #[test]
    fn test_javascript_with_no_manifest_in_the_tree_has_no_runner() {
        let no_runner = |vocab: &crate::ast::AssertVocabulary| {
            matches!(
                check_runner_collected("site/assets/bundle.js", vocab),
                RunnerCollectionStatus::NoRunner(_)
            )
        };
        assert!(no_runner(&tree(&[("pytest.ini", "[pytest]\n")])));
        assert!(!is_runner_collected(
            "site/assets/bundle.js",
            &tree(&[("pytest.ini", "[pytest]\n")])
        ));
        // Any manifest or runner configuration, at any depth, is a sign of a runner.
        for sign in [
            "package.json",
            "web/package.json",
            "web/deno.json",
            "web/vite.config.ts",
            "web/.mocharc.yml",
            "web/jest.config.js",
        ] {
            let vocab = tree(&[(sign, "{}")]);
            assert!(!no_runner(&vocab), "{sign}");
            assert!(is_unknown("site/assets/bundle.js", &vocab), "{sign}");
        }
        // A manifest inside `node_modules` is a dependency's, not the repository's.
        assert!(no_runner(&tree(&[("node_modules/dep/package.json", "{}")])));
        // When the tree was not listed, nothing is known about manifests: still counted.
        assert!(is_unknown("site/assets/bundle.js", &vocab_with(&[])));
        // A declared test path counts whatever the tree holds.
        let mut declared = tree(&[]);
        declared.test_paths = vec!["site/**".to_string()];
        assert_eq!(
            check_runner_collected("site/assets/bundle.js", &declared),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_go_tool_ignored_names() {
        let vocab = vocab_with(&[]);
        for path in [
            "vendor/dep/a_test.go",
            "cmd/vendor/dep/a_test.go",
            "pkg/testdata/a_test.go",
            "testdata/x/a_test.go",
            "_old/a_test.go",
            "pkg/_old/a_test.go",
            ".hidden/a_test.go",
            "pkg/_a_test.go",
            "pkg/.a_test.go",
        ] {
            assert_eq!(
                check_runner_collected(path, &vocab),
                RunnerCollectionStatus::NotCollected,
                "{path}"
            );
        }
        for path in [
            "pkg/a_test.go",
            // Code directly in a directory named `vendor` is an ordinary package.
            "vendor/a_test.go",
            "cmd/vendor/a_test.go",
            // Only whole names are special.
            "vendored/dep/a_test.go",
            "pkg/testdata_gen/a_test.go",
            "pkg/a_b_test.go",
        ] {
            assert_eq!(
                check_runner_collected(path, &vocab),
                RunnerCollectionStatus::Collected,
                "{path}"
            );
        }
    }

    #[test]
    fn test_cargo_package_targets_are_parsed() {
        let manifest = format!(
            "{PKG}autotests = false\n\n[lib]\npath = \"./lib.rs\"\ntest = false\n\n\
             [[bin]]\nname = \"tool\"\npath = \"tool/main.rs\"\n\n\
             [[bin]]\nname = \"quiet\"\npath = \"quiet/main.rs\"\ntest = false\n\n\
             [[test]]\nname = \"off\"\ntest = false\n\n\
             [[test]]\nname = \"custom\"\npath = \"checks/x.rs\"\nharness = false\n\n\
             [[test]]\nname = \"on\"\npath = \"tests/on.rs\"\n"
        );
        let pkg = RustCollectionRules::parse_package(&manifest).unwrap();
        assert!(!pkg.autotests);
        assert!(!pkg.lib_tests_run);
        assert_eq!(pkg.lib_path.as_deref(), Some("lib.rs"));
        assert_eq!(pkg.bin_paths, vec!["tool/main.rs".to_string()]);
        assert_eq!(pkg.has_binary, Some(true));
        let runs: Vec<bool> = pkg.tests.iter().map(|t| t.runs).collect();
        assert_eq!(runs, vec![false, false, true]);
        // A target with no `path` is found by its name.
        assert!(pkg.tests[0].names("tests/off.rs"));
        assert!(pkg.tests[0].names("tests/off/main.rs"));
        assert!(!pkg.tests[0].names("tests/offline.rs"));

        // Defaults, and a manifest that is only a workspace.
        let plain = RustCollectionRules::parse_package(PKG).unwrap();
        assert!(plain.autotests && plain.lib_tests_run);
        assert_eq!(plain.has_binary, None);
        assert!(RustCollectionRules::parse_package("[workspace]\nmembers = []\n").is_none());
    }

    fn rust_status(files: &[(&str, &str)], path: &str) -> RunnerCollectionStatus {
        check_runner_collected(path, &tree(files))
    }

    #[test]
    fn test_cargo_test_targets_decide_what_runs() {
        let off =
            format!("{PKG}\n[[test]]\nname = \"off\"\npath = \"tests/off.rs\"\ntest = false\n");
        let files = [
            ("Cargo.toml", off.as_str()),
            ("tests/off.rs", "mod part;\n"),
            ("tests/off/part.rs", ""),
            ("tests/on.rs", ""),
        ];
        assert_eq!(
            rust_status(&files, "tests/off.rs"),
            RunnerCollectionStatus::NotCollected
        );
        // A module only a switched-off target declares does not run either.
        assert_eq!(
            rust_status(&files, "tests/off/part.rs"),
            RunnerCollectionStatus::NotCollected
        );
        assert_eq!(
            rust_status(&files, "tests/on.rs"),
            RunnerCollectionStatus::Collected
        );

        let no_harness = off.replace("test = false", "harness = false");
        let files = [("Cargo.toml", no_harness.as_str()), ("tests/off.rs", "")];
        assert_eq!(
            rust_status(&files, "tests/off.rs"),
            RunnerCollectionStatus::NotCollected
        );

        let manual = format!(
            "{PKG}autotests = false\n\n[[test]]\nname = \"listed\"\npath = \"tests/listed.rs\"\n"
        );
        let files = [
            ("Cargo.toml", manual.as_str()),
            ("tests/listed.rs", ""),
            ("tests/unlisted.rs", ""),
            ("tests/dir/main.rs", ""),
        ];
        assert_eq!(
            rust_status(&files, "tests/listed.rs"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            rust_status(&files, "tests/unlisted.rs"),
            RunnerCollectionStatus::NotCollected
        );
        assert_eq!(
            rust_status(&files, "tests/dir/main.rs"),
            RunnerCollectionStatus::NotCollected
        );
    }

    #[test]
    fn test_cargo_lib_test_false() {
        let manifest = format!("{PKG}\n[lib]\ntest = false\n");
        let lib_only = [("Cargo.toml", manifest.as_str()), ("src/lib.rs", "")];
        assert_eq!(
            rust_status(&lib_only, "src/lib.rs"),
            RunnerCollectionStatus::NotCollected
        );
        // A binary's unit tests run, and a file under `src/` may be its module.
        for bin in ["src/main.rs", "src/bin/tool.rs"] {
            let files = [
                ("Cargo.toml", manifest.as_str()),
                ("src/lib.rs", ""),
                (bin, ""),
            ];
            assert!(
                matches!(
                    rust_status(&files, "src/lib.rs"),
                    RunnerCollectionStatus::Unknown(_)
                ),
                "{bin}"
            );
        }
        // Integration tests are another target.
        let files = [("Cargo.toml", manifest.as_str()), ("tests/it.rs", "")];
        assert_eq!(
            rust_status(&files, "tests/it.rs"),
            RunnerCollectionStatus::Collected
        );
        // When the tree was not listed, a binary cannot be ruled out.
        assert!(is_unknown(
            "src/lib.rs",
            &vocab_with(&[("Cargo.toml", manifest.as_str())])
        ));
    }

    #[test]
    fn test_files_under_tests_count_only_as_modules_of_a_target() {
        let files = [
            ("Cargo.toml", PKG),
            ("tests/a.rs", "mod common;\nmod inline { mod deep; }\n"),
            ("tests/common/mod.rs", "mod util;\n"),
            ("tests/common/util.rs", ""),
            ("tests/inline/deep.rs", ""),
            (
                "tests/it/main.rs",
                "mod foo;\n#[path = \"../shared/cases.rs\"]\nmod cases;\n",
            ),
            ("tests/it/foo.rs", "mod bar;\n"),
            ("tests/it/foo/bar.rs", ""),
            ("tests/shared/cases.rs", "mod extra;\n"),
            ("tests/shared/extra.rs", ""),
            ("tests/it/orphan.rs", ""),
            ("tests/disabled/b.rs", ""),
            ("tests/fixtures/sample/src/lib.rs", ""),
        ];
        for path in [
            "tests/a.rs",
            "tests/common/mod.rs",
            "tests/common/util.rs",
            "tests/inline/deep.rs",
            "tests/it/main.rs",
            "tests/it/foo.rs",
            "tests/it/foo/bar.rs",
            "tests/shared/cases.rs",
            "tests/shared/extra.rs",
        ] {
            assert_eq!(
                rust_status(&files, path),
                RunnerCollectionStatus::Collected,
                "{path}"
            );
        }
        for path in [
            "tests/it/orphan.rs",
            "tests/disabled/b.rs",
            "tests/fixtures/sample/src/lib.rs",
        ] {
            assert_eq!(
                rust_status(&files, path),
                RunnerCollectionStatus::NotCollected,
                "{path}"
            );
        }
    }

    #[test]
    fn test_module_declarations_that_are_not_followed_leave_the_rest_unknown() {
        for root in [
            "automod::dir!(\"tests/cases\");\n",
            "include!(\"cases/one.rs\");\n",
            "macro_rules! cases { ($n:ident) => { mod $n; }; }\ncases!(one);\n",
            "#[cfg_attr(unix, path = \"cases/one.rs\")]\nmod one;\n",
            "mod outer {\n    #[path = \"one.rs\"]\n    mod one;\n}\n",
            "#[path = \"../../outside.rs\"]\nmod one;\n",
            "mod broken {\n",
        ] {
            let files = [
                ("Cargo.toml", PKG),
                ("tests/it.rs", root),
                ("tests/cases/one.rs", ""),
            ];
            assert!(
                matches!(
                    rust_status(&files, "tests/cases/one.rs"),
                    RunnerCollectionStatus::Unknown(_)
                ),
                "{root:?}"
            );
        }
        // Control: a macro inside a function body declares no file module.
        let files = [
            ("Cargo.toml", PKG),
            ("tests/it.rs", "#[test]\nfn t() { assert_eq!(1, 1); }\n"),
            ("tests/cases/one.rs", ""),
        ];
        assert_eq!(
            rust_status(&files, "tests/cases/one.rs"),
            RunnerCollectionStatus::NotCollected
        );
    }

    #[test]
    fn test_cargo_targets_outside_src_and_tests() {
        let member = format!(
            "{PKG}\n[lib]\npath = \"lib.rs\"\n\n[[test]]\nname = \"x\"\npath = \"checks/x.rs\"\n"
        );
        let files = [
            ("Cargo.toml", "[workspace]\nmembers = [\"crates/m\"]\n"),
            ("crates/m/Cargo.toml", member.as_str()),
            ("crates/m/lib.rs", ""),
            ("crates/m/util.rs", ""),
            ("crates/m/checks/x.rs", "mod part;\n"),
            ("crates/m/checks/part.rs", ""),
            ("crates/m/checks/loose.rs", ""),
            ("other/helper.rs", ""),
        ];
        for path in [
            "crates/m/lib.rs",
            "crates/m/checks/x.rs",
            "crates/m/checks/part.rs",
        ] {
            assert_eq!(
                rust_status(&files, path),
                RunnerCollectionStatus::Collected,
                "{path}"
            );
        }
        // Beside a target root: may be its module.
        for path in ["crates/m/util.rs", "crates/m/checks/loose.rs"] {
            assert!(
                matches!(
                    rust_status(&files, path),
                    RunnerCollectionStatus::Unknown(_)
                ),
                "{path}"
            );
        }
        // No package owns this one: the path rule decides, as before.
        assert_eq!(
            rust_status(&files, "other/helper.rs"),
            RunnerCollectionStatus::NotCollected
        );
        // Control: without a parsed manifest every file under `tests/` counts.
        assert_eq!(
            rust_status(&[("tests/disabled/b.rs", "")], "tests/disabled/b.rs"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            rust_status(
                &[("Cargo.toml", "[package\n"), ("tests/disabled/b.rs", "")],
                "tests/disabled/b.rs"
            ),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_rust_cfg_boolean_literals_and_target_optional_dependencies() {
        assert_eq!(eval_cfg_str("#[cfg(false)]", None).0, CfgValue::False);
        assert_eq!(eval_cfg_str("#[cfg(true)]", None).0, CfgValue::True);
        assert_eq!(
            eval_cfg_str("#[cfg(any(false, unix))]", None).0,
            CfgValue::Unknown
        );
        assert_eq!(
            eval_cfg_str("#[cfg(all(false, unix))]", None).0,
            CfgValue::False
        );
        assert_eq!(
            eval_cfg_str("#[cfg(any(not(test), unix))]", None).0,
            CfgValue::Unknown
        );
        // Control: an identifier that only resembles the literal.
        assert_eq!(eval_cfg_str("#[cfg(falsey)]", None).0, CfgValue::Unknown);

        let rules = RustCollectionRules::parse_cargo_toml(
            "[package]\nname = \"p\"\n\n[target.'cfg(unix)'.dependencies]\n\
             foo = { version = \"1\", optional = true }\nbar = \"1\"\n",
        );
        assert!(rules.declared_features.contains("foo"));
        assert!(!rules.declared_features.contains("bar"));
    }
}
