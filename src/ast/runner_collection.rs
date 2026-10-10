//! Runner test collection rules for language packs.
//!
//! Determines whether a file is collected and executed by its framework's test runner:
//! - Python (pytest): `python_files`, `python_classes`, `python_functions`, `testpaths`
//!   from `pyproject.toml`, `pytest.ini`, `setup.cfg`, `tox.ini`.
//! - JS / TS (Jest / Vitest): `testMatch`, `testRegex`, `testPathIgnorePatterns`, `roots`,
//!   `include` from `package.json`, `jest.config.json`.
//! - Go: `_test.go` suffix, outside the directories and file names the go tool ignores,
//!   with the file's build constraint and the module it belongs to.
//! - Any language: `linguist-vendored` / `linguist-generated` in `.gitattributes`.
//! - Rust: Cargo's target layout as the owning `Cargo.toml` declares it (`autotests`,
//!   `[lib]`, `[[test]]`), and the `mod` declarations under each test target. Without a
//!   parsed manifest: any `.rs` under a `src/` or `tests/` directory.
//! - Python with no pytest configuration, and every language with no runner model here
//!   (Java, Kotlin, C#, Scala, Swift, Objective-C, Ruby, PHP, C / C++): not determined.
//!   Only a parsed runner configuration excludes a file.
//! - Rust `#[cfg]` on a test: `cfg(feature = "x")` where `x` is not declared in `[features]`,
//!   or `cfg(any())` / `cfg(all(any()))`, treated as an unconditional ignore.

use super::ancestry::Ancestry;
use super::gitattributes::{wildmatch, GitAttributes};
use super::go_build::{go_build_constraint, go_build_constraint_line, GoBuild};
use super::go_work::{parse_go_work, GoWork};
use super::runner_config::{
    deno_entry_is_glob, imports_node_test, jest_setup_files, parse_conftest,
    parse_conftest_plugins, parse_deno_config, parse_vitest_config, plain_semver_major,
    read_jest_scripts, script_setup_files, scripts_run_node_test, ConftestIgnores, DenoTest,
    SetupFiles, VitestConfig,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use tree_sitter::Node;

/// Whether the glob `pattern` matches `candidate` (globset, `*` crossing `/`). `None` for
/// a pattern that does not compile as a glob: it is never matched as plain text, since
/// text found inside a name says nothing about what the runner that reads the pattern
/// does with it.
pub fn glob_match(pattern: &str, candidate: &str) -> Option<bool> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Some(false);
    }
    globset::GlobBuilder::new(pat)
        .literal_separator(false)
        .build()
        .ok()
        .map(|glob| glob.compile_matcher().is_match(candidate))
}

/// Why a Python file's collection is not known when a `python_files` entry of the pytest
/// configuration does not compile as a glob and no other entry matches the file. The
/// entry is not quoted.
pub const PYTHON_FILES_UNREAD: &str =
    "a `python_files` entry of the pytest configuration is not a glob that can be read";

/// Pytest test collection configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PytestCollectionRules {
    pub python_files: Vec<String>,
    pub python_classes: Vec<String>,
    pub python_functions: Vec<String>,
    pub testpaths: Vec<String>,
    /// A pytest configuration was found: a `[tool.pytest.ini_options]` table, a
    /// `pytest.ini` or `.pytest.ini`, a `[pytest]` section of `tox.ini`, or a
    /// `[tool:pytest]` section of `setup.cfg`. Without one the runner may be unittest or
    /// Django, and pytest's defaults are not known to apply.
    pub configured: bool,
    /// The configuration sets `python_functions`: the names in it replace `test*`.
    pub functions_configured: bool,
    /// The configuration sets `python_classes`: the names in it replace `Test*`.
    pub classes_configured: bool,
    /// `norecursedirs` as configured; `None` is pytest's default list.
    pub norecursedirs: Option<Vec<String>>,
    /// The ignore lists of each tracked `conftest.py`, by its directory. Present only
    /// when the tree was listed.
    pub conftests: Vec<(String, ConftestIgnores)>,
    /// The file the configuration was read from.
    pub source: Option<String>,
    /// The configuration file could not be parsed.
    pub unparseable: Option<String>,
}

/// pytest's default `norecursedirs` (`_pytest/main.py`, pytest 8.4). `__pycache__` is
/// skipped whatever the list says.
const PYTEST_DEFAULT_NORECURSEDIRS: &[&str] = &[
    "*.egg",
    ".*",
    "_darcs",
    "build",
    "CVS",
    "dist",
    "node_modules",
    "venv",
    "{arch}",
];

/// What pytest's directory rules say about a file its name patterns collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PytestDirectories {
    Clear,
    Excluded,
    Unknown(&'static str),
}

/// pytest's rule for `python_functions` and `python_classes`: an entry is a name
/// prefix, or a glob when it holds a glob character.
fn prefix_or_glob(options: &[String], name: &str) -> bool {
    options.iter().any(|option| {
        name.starts_with(option.as_str())
            || (option.contains(['*', '?', '[']) && wildmatch(option, name, false))
    })
}

impl Default for PytestCollectionRules {
    fn default() -> Self {
        Self {
            python_files: vec!["test_*.py".to_string(), "*_test.py".to_string()],
            python_classes: vec!["Test*".to_string()],
            python_functions: vec!["test_*".to_string()],
            testpaths: Vec::new(),
            configured: false,
            functions_configured: false,
            classes_configured: false,
            norecursedirs: None,
            conftests: Vec::new(),
            source: None,
            unparseable: None,
        }
    }
}

impl PytestCollectionRules {
    /// Whether `python_files` and `testpaths` are known to collect `path`.
    pub fn is_collected(&self, path: &str) -> bool {
        self.collection(path) == Some(true)
    }

    /// Whether `python_files` and `testpaths` collect `path`. `None` when that is not
    /// known: the file is under `testpaths`, no `python_files` entry that compiles
    /// matches it, and an entry does not compile ([`glob_match`]).
    pub fn collection(&self, path: &str) -> Option<bool> {
        let norm = path.replace('\\', "/");
        if !norm.ends_with(".py") {
            return Some(false);
        }

        // If testpaths is configured, path must fall under one of them
        if !self.testpaths.is_empty() {
            let in_testpath = self.testpaths.iter().any(|tp| testpath_holds(tp, &norm));
            if !in_testpath {
                return Some(false);
            }
        }

        let filename = norm.rsplit('/').next().unwrap_or(&norm);
        let patterns = if self.python_files.is_empty() {
            vec!["test_*.py", "*_test.py"]
        } else {
            self.python_files.iter().map(String::as_str).collect()
        };

        let verdicts: Vec<Option<bool>> = patterns
            .iter()
            .map(|pat| glob_match(pat, filename))
            .collect();
        if verdicts.contains(&Some(true)) {
            Some(true)
        } else if verdicts.contains(&None) {
            None
        } else {
            Some(false)
        }
    }

    pub fn parse_pyproject_toml(content: &str) -> Self {
        let mut rules = Self::default();
        let Ok(val) = toml::from_str::<toml::Value>(content) else {
            rules.unparseable = Some("pyproject.toml".to_string());
            return rules;
        };
        let Some(root) = val.as_table() else {
            rules.unparseable = Some("pyproject.toml".to_string());
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
                rules.classes_configured = true;
            }
        }
        if let Some(v) = tbl.get("python_functions") {
            let funcs = toml_val_to_strings(v);
            if !funcs.is_empty() {
                rules.python_functions = funcs;
                rules.functions_configured = true;
            }
        }
        if let Some(v) = tbl.get("norecursedirs") {
            rules.norecursedirs = Some(toml_val_to_strings(v));
        }
        if let Some(v) = tbl.get("testpaths") {
            let paths = toml_val_to_strings(v);
            if !paths.is_empty() {
                rules.testpaths = paths;
            }
        }
    }

    /// A `pytest.ini`, `.pytest.ini` or `tox.ini`: the `[pytest]` section.
    pub fn parse_ini(content: &str) -> Self {
        Self::parse_ini_sections(content, &["pytest", "tool:pytest"])
    }

    /// A `setup.cfg`: only `[tool:pytest]`. pytest refuses a `[pytest]` section there
    /// (`[pytest] section in setup.cfg files is no longer supported`), so it configures
    /// nothing.
    pub fn parse_setup_cfg(content: &str) -> Self {
        Self::parse_ini_sections(content, &["tool:pytest"])
    }

    fn parse_ini_sections(content: &str, sections: &[&str]) -> Self {
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
                in_section = sections.contains(&s);
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
            "python_classes" => {
                rules.python_classes = items;
                rules.classes_configured = true;
            }
            "python_functions" => {
                rules.python_functions = items;
                rules.functions_configured = true;
            }
            "testpaths" => rules.testpaths = items,
            "norecursedirs" => rules.norecursedirs = Some(items),
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
            // A value that starts on the next line replaces the default names.
            "python_classes" => {
                if !rules.classes_configured {
                    rules.python_classes.clear();
                    rules.classes_configured = true;
                }
                rules.python_classes.extend(items);
            }
            "python_functions" => {
                if !rules.functions_configured {
                    rules.python_functions.clear();
                    rules.functions_configured = true;
                }
                rules.python_functions.extend(items);
            }
            "testpaths" => rules.testpaths.extend(items),
            "norecursedirs" => rules
                .norecursedirs
                .get_or_insert_with(Vec::new)
                .extend(items),
            _ => {}
        }
    }

    /// Whether a configured `python_functions` names `name` as a test. `None` when the
    /// configuration does not set the key: the pack's default naming applies.
    pub fn function_name_override(&self, name: &str) -> Option<bool> {
        (self.configured && self.functions_configured)
            .then(|| prefix_or_glob(&self.python_functions, name))
    }

    /// As [`Self::function_name_override`], for `python_classes`.
    pub fn class_name_override(&self, name: &str) -> Option<bool> {
        (self.configured && self.classes_configured)
            .then(|| prefix_or_glob(&self.python_classes, name))
    }

    /// Whether pytest reaches a file its name patterns collect: not below a directory
    /// `norecursedirs` names, and not named by a `conftest.py` ignore list. pytest
    /// applies both while it recurses, so the directories of a `testpaths` entry itself
    /// are not checked.
    pub fn directories(&self, path: &str) -> PytestDirectories {
        let (hit, start, dynamic) = self.directory_rule(path);
        match (hit.is_some(), start) {
            (true, Some(_)) => PytestDirectories::Excluded,
            (true, None) => PytestDirectories::Unknown(
                "a pytest `norecursedirs` or `conftest.py` ignore below a `testpaths` glob",
            ),
            (false, _) if dynamic => PytestDirectories::Unknown(
                "a `conftest.py` sets `collect_ignore` in a way that is not read",
            ),
            (false, _) => PytestDirectories::Clear,
        }
    }

    /// The directory rule that keeps pytest from reaching `path` (the first found:
    /// `norecursedirs`, then a `conftest.py` list), where recursion starts for it
    /// (`None` when only a `testpaths` glob holds it), and whether a `conftest.py`
    /// above it builds its list at run time.
    fn directory_rule(&self, path: &str) -> (Option<PytestDirectoryRule>, Option<usize>, bool) {
        let norm = path.replace('\\', "/");
        // Where recursion starts: the `testpaths` entry that holds the file, as a
        // number of leading path segments. `None` when only a glob entry holds it.
        let start: Option<usize> = if self.testpaths.is_empty() {
            Some(0)
        } else {
            self.testpaths
                .iter()
                .filter_map(|tp| clean_relative(tp))
                .filter(|entry| !entry.contains(['*', '?', '[']))
                .filter(|entry| {
                    entry.is_empty() || norm == *entry || norm.starts_with(&format!("{entry}/"))
                })
                .map(|entry| entry.split('/').filter(|s| !s.is_empty()).count())
                .max()
        };
        let segments: Vec<&str> = norm.split('/').collect();
        // Each path pytest decides on while recursing: the directories below the
        // start, then the file.
        let from = start.unwrap_or(0);
        let mut hit: Option<PytestDirectoryRule> = None;
        let default_list: Vec<String>;
        let patterns: &[String] = match &self.norecursedirs {
            Some(list) => list,
            None => {
                default_list = PYTEST_DEFAULT_NORECURSEDIRS
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                &default_list
            }
        };
        for end in (from + 1)..segments.len() {
            let name = segments[end - 1];
            let dir = segments[..end].join("/");
            let named = name == "__pycache__"
                || patterns.iter().any(|pattern| {
                    if pattern.contains('/') {
                        wildmatch(&format!("*/{pattern}"), &format!("/{dir}"), false)
                    } else {
                        wildmatch(pattern, name, false)
                    }
                });
            if named {
                hit.get_or_insert(PytestDirectoryRule::NoRecurse);
            }
        }
        let mut dynamic = false;
        for (conftest_dir, ignores) in &self.conftests {
            let below = conftest_dir.is_empty() || norm.starts_with(&format!("{conftest_dir}/"));
            if !below {
                continue;
            }
            let depth = conftest_dir.split('/').filter(|s| !s.is_empty()).count();
            match ignores {
                ConftestIgnores::Dynamic => dynamic = true,
                ConftestIgnores::Literal { paths, globs } => {
                    for end in (depth.max(from) + 1)..=segments.len() {
                        let candidate = segments[..end].join("/");
                        let named = paths.iter().any(|entry| {
                            clean_relative(&join_dir(conftest_dir, entry)).as_deref()
                                == Some(candidate.as_str())
                        }) || globs.iter().any(|glob| {
                            wildmatch(&join_dir(conftest_dir, glob), &candidate, false)
                        });
                        if named {
                            hit.get_or_insert_with(|| {
                                PytestDirectoryRule::Conftest(join_dir(conftest_dir, "conftest.py"))
                            });
                        }
                    }
                }
            }
        }
        (hit, start, dynamic)
    }

    /// The rules of this configuration that leave `norm` out, for
    /// [`RunnerCollectionRules::mechanisms`].
    fn mechanisms(&self, norm: &str, out: &mut Vec<Mechanism>) {
        if !self.configured || !norm.ends_with(".py") {
            return;
        }
        let source = self.source.clone().unwrap_or_default();
        let in_testpaths =
            self.testpaths.is_empty() || self.testpaths.iter().any(|tp| testpath_holds(tp, norm));
        if !in_testpaths {
            out.push(Mechanism::new(&source, "testpaths", "`testpaths` in"));
            return;
        }
        if !self.is_collected(norm) {
            out.push(Mechanism::new(&source, "python_files", "`python_files` in"));
            return;
        }
        match self.directory_rule(norm).0 {
            Some(PytestDirectoryRule::NoRecurse) => {
                out.push(Mechanism::new(
                    &source,
                    "norecursedirs",
                    "`norecursedirs` in",
                ));
            }
            Some(PytestDirectoryRule::Conftest(file)) => out.push(Mechanism::new(
                &file,
                "collect_ignore",
                "`collect_ignore` / `collect_ignore_glob` in",
            )),
            None => {}
        }
    }
}

/// Which of pytest's directory rules keeps it from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PytestDirectoryRule {
    /// `norecursedirs`, or the `__pycache__` pytest always skips.
    NoRecurse,
    /// A literal `collect_ignore` / `collect_ignore_glob` list, with the `conftest.py`.
    Conftest(String),
}

/// A rule that takes a test file out of the default run, as a change can add it: the
/// file that holds it and what it is. Never a configured value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mechanism {
    /// The file that holds the rule.
    pub file: String,
    /// A short stable name of the rule, which tells two rules of one file apart.
    pub key: &'static str,
    /// The rule in words that the path of `file` completes: "`testpaths` in".
    pub what: &'static str,
    /// The line of the rule in `file`, when it is one line.
    pub line: Option<usize>,
}

impl Mechanism {
    fn new(file: &str, key: &'static str, what: &'static str) -> Self {
        Self {
            file: file.to_string(),
            key,
            what,
            line: None,
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
    /// The parsed configuration is a Vitest one (a literal `vitest.config.*`, or a
    /// `vite.config.*` with a literal `test` block): Vitest's default patterns apply.
    pub vitest: bool,
    /// A literal Vitest `exclude`; `None` is Vitest's default list.
    pub vitest_exclude: Option<Vec<String>>,
    /// The repository-relative directory a literal Vitest `root` / `dir` names.
    pub vitest_base: Option<String>,
    /// The Jest major version the manifest pins, when its range is a plain one.
    pub jest_major: Option<u32>,
    /// The `package.json` scripts run Jest in a way that is not read, with the kind.
    pub script_problem: Option<&'static str>,
    /// The file the parsed Jest or Vitest configuration was read from.
    pub config_source: Option<String>,
    /// Each tracked file that imports `node:test`: a `node --test` test, whatever the
    /// Jest or Vitest patterns say. Present only when the tree was listed.
    pub node_test_files: HashSet<String>,
    /// A script of the root `package.json` runs `node --test`.
    pub node_test_script: bool,
    /// The Vitest major version the manifest pins, when its range is a plain one.
    pub vitest_major: Option<u32>,
    /// The lists of the root `deno.json` / `deno.jsonc`, with the file's name, in a
    /// repository with no root `package.json`. `None` when there is no such file or
    /// its lists cannot be told.
    pub deno: Option<(String, DenoTest)>,
    /// The root Deno configuration whose lists cannot be told, with the reason.
    pub deno_problem: Option<(String, &'static str)>,
    /// Directories below the root that hold a `deno.json` / `deno.jsonc` of their own.
    pub nested_deno: Vec<String>,
    /// Jest refuses to run with the configuration found, with the conflict.
    pub conflict: Option<&'static str>,
    /// Directories below the root whose `package.json` has a script that runs
    /// `node --test`.
    pub nested_node_test: Vec<String>,
    /// Each glob with whether it is negated (`!pattern`), in configured order. `None`
    /// is a pattern that matches no path.
    compiled_globs: Vec<(Option<globset::GlobMatcher>, bool)>,
    compiled_regexes: Vec<regex::Regex>,
    compiled_ignores: Vec<regex::Regex>,
    /// A literal Vitest `exclude`, compiled.
    compiled_excludes: Vec<globset::GlobMatcher>,
    /// Repository-relative directories collection is limited to; empty is no limit.
    effective_roots: Vec<String>,
}

/// Why a file with one of the module extensions is not determined under Jest's default
/// patterns: they match `.mjs` / `.cjs` / `.mts` / `.cts` from Jest 30 and not before.
const JEST_VERSION_UNKNOWN: &str =
    "jest's default patterns for `.mjs` / `.cjs` / `.mts` / `.cts` depend on its version, which the manifest does not pin";
/// The first Jest major whose default `testMatch` holds the module extensions.
const JEST_MODULE_EXTENSIONS_SINCE: u32 = 30;
/// Why a file below one of the directories Vitest left out by default before Vitest 4
/// is not determined.
const VITEST_VERSION_UNKNOWN: &str =
    "vitest's default `exclude` for `dist`, `cypress` and the `.idea` / `.git` / `.cache` / `.output` / `.temp` directories depends on its version, which the manifest does not pin";
/// The first Vitest major whose default `exclude` holds `node_modules` and `.git` only.
const VITEST_SHORT_DEFAULT_EXCLUDE_SINCE: u32 = 4;
/// The directories Vitest 1 to 3 leave out by default, at any depth, beside
/// `node_modules` (run with Vitest 1.6 and 3.1; Vitest 2 is from its documentation).
const VITEST_OLD_DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    "dist", "cypress", ".idea", ".git", ".cache", ".output", ".temp",
];
const JEST_SCRIPT_CONFIG_UNREAD: &str =
    "the jest configuration a package script names is not a tracked JSON file";
/// Why a test file named like a tool's configuration file is not determined: Vitest's
/// default `exclude` held those names before Vitest 4.
const VITEST_CONFIG_NAMES_UNKNOWN: &str =
    "vitest's default `exclude` for a file named like a tool's configuration (`vite.config.*`, `jest.config.*` and the like) depends on its version, which the manifest does not pin";
/// The same under Vitest 2, which was not available to compare with.
const VITEST_2_CONFIG_NAMES: &str =
    "vitest 2 was not compared with for a file named like a tool's configuration (`vite.config.*`, `jest.config.*` and the like), which vitest 1 and 3 leave out by default";
/// The tools whose `<name>.config.*` files Vitest 1 to 3 leave out by default, at any
/// depth (run with Vitest 1.6 and 3.1; Vitest 4.1 runs them).
const VITEST_OLD_DEFAULT_EXCLUDED_CONFIGS: &[&str] = &[
    "karma", "rollup", "webpack", "vite", "vitest", "jest", "ava", "babel", "nyc", "cypress",
    "tsup", "build", "eslint", "prettier",
];
/// Jest 27.5 and 29.7 stop with `Configuration options testMatch and testRegex cannot
/// be used together` (an empty `testMatch` list included, an empty `testRegex` not).
const JEST_MATCH_AND_REGEX: &str =
    "the jest configuration sets both `testMatch` and `testRegex`, which jest refuses to run with";
/// Jest 29.5 and 29.7 stop with `Multiple configurations found`; Jest 27.5 warns and
/// reads one of them.
const JEST_SEVERAL_CONFIGS: &str =
    "more than one jest configuration is found (the `jest` key of `package.json`, `jest.config.json`, a `jest.config.*` script), which jest 29 refuses to run with";
const DENO_ENTRY_UNRESOLVED: &str =
    "an entry of the Deno configuration's lists is not a path inside the repository";
const DENO_TASK_PATHS: &str =
    "a task of the Deno configuration passes `deno test` paths of its own, which are not read";
const DENO_WORKSPACE_MEMBER: &str =
    "the file is below a directory with its own Deno configuration in a Deno workspace, which is not read";
const DENO_NODE_MODULES: &str =
    "a `test.include` entry of the Deno configuration names a file below `node_modules` by a glob";
const DENO_VENDOR_INCLUDED: &str =
    "the Deno configuration sets `vendor`, and a `test.include` entry holds the `vendor` directory";
const NESTED_RUNNER_CONFIG: &str = "a nested package has its own runner configuration";

/// The reasons [`JsCollectionRules::is_collected`] gives that name the cause themselves:
/// a report shows them as they are, and not the kind of the configuration problem.
const JS_SPECIFIC_REASONS: &[&str] = &[
    JEST_VERSION_UNKNOWN,
    VITEST_VERSION_UNKNOWN,
    VITEST_CONFIG_NAMES_UNKNOWN,
    VITEST_2_CONFIG_NAMES,
    DENO_ENTRY_UNRESOLVED,
    DENO_TASK_PATHS,
    DENO_WORKSPACE_MEMBER,
    DENO_NODE_MODULES,
    DENO_VENDOR_INCLUDED,
    NESTED_RUNNER_CONFIG,
    crate::ast::runner_config::DENO_UNPARSED,
    crate::ast::runner_config::DENO_OTHER_CONFIG,
    crate::ast::runner_config::DENO_NEGATED_GLOB,
    crate::ast::runner_config::DENO_NEGATION_UNREACHED,
];

/// What a root Deno configuration says about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DenoStatus {
    Collected,
    /// Left out, by the key that does it; `None` for Deno's own rules (its default
    /// names, a declaration file, `node_modules`).
    LeftOut(Option<&'static str>),
    NotDetermined(&'static str),
}

/// Whether `deno test` reads the file at `norm` as a test module by its name alone
/// (deno 2.6, run on a directory of such names): the name without its extension is
/// `test`, or ends in `_test` or `.test`, or a directory above the file is named
/// `__tests__`. The stem is matched as written; a declaration file (`.d.ts`, `.d.mts`,
/// `.d.cts`) runs no test.
fn deno_default_name(norm: &str) -> bool {
    let (dirs, name) = match norm.rfind('/') {
        Some(i) => (&norm[..i], &norm[i + 1..]),
        None => ("", norm),
    };
    let Some((stem, _)) = name.rsplit_once('.') else {
        return false;
    };
    stem == "test"
        || stem.ends_with("_test")
        || stem.ends_with(".test")
        || dirs.split('/').any(|dir| dir == "__tests__")
}

/// A TypeScript declaration file, which runs nothing.
fn is_declaration_file(lower: &str) -> bool {
    [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|end| lower.ends_with(end))
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
            && self.vitest == other.vitest
            && self.vitest_exclude == other.vitest_exclude
            && self.vitest_base == other.vitest_base
            && self.jest_major == other.jest_major
            && self.script_problem == other.script_problem
            && self.config_source == other.config_source
            && self.node_test_files == other.node_test_files
            && self.node_test_script == other.node_test_script
            && self.vitest_major == other.vitest_major
            && self.deno == other.deno
            && self.deno_problem == other.deno_problem
            && self.nested_deno == other.nested_deno
            && self.conflict == other.conflict
            && self.nested_node_test == other.nested_node_test
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
/// `deno.json` / `deno.jsonc` and `bunfig.toml` are the configuration of `deno test`
/// and `bun test`. A root Deno configuration decides for the files of a Deno-only
/// repository ([`JsCollectionRules::deno`]); `bunfig.toml` is not read (no `bun` was
/// available to compare with), so the files of such a repository are counted with
/// collection not determined.
fn is_js_runner_sign(name: &str) -> bool {
    matches!(
        name,
        "package.json"
            | "deno.json"
            | "deno.jsonc"
            | "bunfig.toml"
            | "jest.config.json"
            | "cypress.json"
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
        if let Some(conflict) = self.conflict {
            return Some(conflict.to_string());
        }
        if self.invalid_pattern.is_some() {
            return Some(
                self.invalid_kind
                    .unwrap_or("a configured pattern cannot be evaluated statically")
                    .to_string(),
            );
        }
        if let Some(problem) = self.script_problem {
            return Some(problem.to_string());
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
            && self.conflict.is_none()
            && self.invalid_pattern.is_none()
            && self.script_problem.is_none()
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
        self.compiled_excludes.clear();
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
        // A literal Vitest `root` / `dir`: the directory its patterns resolve against,
        // and the only one it scans.
        let vitest_base = match &self.vitest_base {
            None => String::new(),
            Some(raw) => match repo_relative_dir(raw) {
                Some(dir) => dir,
                None => {
                    self.set_invalid(
                        "vitest `root` / `dir` cannot be resolved to a repository path".to_string(),
                        "a vitest `root` or `dir` is not a repository-relative path",
                    );
                    return;
                }
            },
        };
        if !vitest_base.is_empty() {
            self.effective_roots = vec![vitest_base.clone()];
        }
        for raw in self.vitest_exclude.clone().unwrap_or_default() {
            let pattern = join_dir(&vitest_base, raw.trim().trim_start_matches("./"));
            if has_extglob(&pattern) || has_group(&pattern) || pattern.starts_with('!') {
                self.set_invalid(
                    "vitest `exclude` pattern cannot be evaluated statically".to_string(),
                    "a vitest `exclude` pattern uses extglob, a group or a negation",
                );
                continue;
            }
            match globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
            {
                Ok(glob) => self.compiled_excludes.push(glob.compile_matcher()),
                Err(_) => self.set_invalid(
                    "vitest `exclude` pattern failed to compile".to_string(),
                    "a configured glob pattern does not compile",
                ),
            }
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
            // A Vitest `exclude` replaces Vitest's default list, `node_modules` included.
            None if self.vitest && self.vitest_exclude.is_some() => Vec::new(),
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
                // A pattern that starts with a literal character other than `/` cannot
                // match an absolute path: Jest 27.5 and 29.7 list nothing for
                // `src/**/*.js` or `./src/**/*.js`. A negated one is not read that way,
                // and neither is one that starts with a wildcard (`*/**/a.test.js`
                // matches) or a drive letter.
                let mut chars = body.chars();
                let literal_start = chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                    && chars.next() != Some(':');
                if literal_start && !negated {
                    self.compiled_globs.push((None, false));
                    continue;
                }
                self.set_invalid(
                    format!("testMatch pattern is not anchored: '{trimmed}'"),
                    "a jest `testMatch` pattern starts at neither `<rootDir>` nor `**`",
                );
                continue;
            }
            let without_root = if from_test_match {
                resolve_root_token(body, &root_dir)
            } else {
                join_dir(&vitest_base, body.trim_start_matches("./"))
            };
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
                Ok(glob) => self
                    .compiled_globs
                    .push((Some(glob.compile_matcher()), negated)),
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

        // 0. The package scripts run Jest in a way that is not read
        if let Some(problem) = self.script_problem {
            return JsCollectionResult::Unknown(problem.to_string());
        }

        // 0b. Jest refuses to run with the configuration found
        if let Some(conflict) = self.conflict {
            return JsCollectionResult::Unknown(conflict.to_string());
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

        // 4. If no runner config found. A root Deno configuration still says what
        // `deno test` leaves out.
        if !self.config_parsed {
            return match self.deno_status(&norm) {
                Some(DenoStatus::Collected) => JsCollectionResult::Collected,
                Some(DenoStatus::LeftOut(_)) => JsCollectionResult::NotCollected,
                Some(DenoStatus::NotDetermined(reason)) => {
                    JsCollectionResult::Unknown(reason.to_string())
                }
                None => JsCollectionResult::Unknown("no runner config found".to_string()),
            };
        }

        // 5. A nested package runs its own configuration, which is not read
        if self.nested_config_for(&norm) {
            return JsCollectionResult::Unknown(NESTED_RUNNER_CONFIG.to_string());
        }

        // 6. The parsed configuration decides. Beside a second runner, a file it leaves
        // out may be that runner's test: not determined, rather than excluded.
        match (self.configuration_collects(&norm), &self.second_runner) {
            (true, _) if self.default_exclude_depends_on_version(&norm) => {
                match self.vitest_major {
                    Some(major) if major >= VITEST_SHORT_DEFAULT_EXCLUDE_SINCE => {
                        JsCollectionResult::Collected
                    }
                    Some(major) if major >= 1 => JsCollectionResult::NotCollected,
                    _ => JsCollectionResult::Unknown(VITEST_VERSION_UNKNOWN.to_string()),
                }
            }
            (true, _) if self.default_exclude_names_configuration(&norm) => {
                match self.vitest_major {
                    Some(major) if major >= VITEST_SHORT_DEFAULT_EXCLUDE_SINCE => {
                        JsCollectionResult::Collected
                    }
                    Some(1 | 3) => JsCollectionResult::NotCollected,
                    Some(2) => JsCollectionResult::Unknown(VITEST_2_CONFIG_NAMES.to_string()),
                    _ => JsCollectionResult::Unknown(VITEST_CONFIG_NAMES_UNKNOWN.to_string()),
                }
            }
            (true, _) if self.default_patterns_depend_on_version(&lower) => match self.jest_major {
                Some(major) if major >= JEST_MODULE_EXTENSIONS_SINCE => {
                    JsCollectionResult::Collected
                }
                Some(_) => JsCollectionResult::NotCollected,
                None => JsCollectionResult::Unknown(JEST_VERSION_UNKNOWN.to_string()),
            },
            (true, _) => JsCollectionResult::Collected,
            (false, Some(second)) => JsCollectionResult::Unknown(second.clone()),
            (false, None) => JsCollectionResult::NotCollected,
        }
    }

    /// Whether a literal Vitest `exclude` leaves `norm` out: a pattern that matches the
    /// file, or a directory above it (`exclude: ['legacy']` leaves out what `legacy/`
    /// holds).
    fn vitest_excludes(&self, norm: &str) -> bool {
        let mut end = norm.len();
        loop {
            if self
                .compiled_excludes
                .iter()
                .any(|glob| glob.is_match(&norm[..end]))
            {
                return true;
            }
            match norm[..end].rfind('/') {
                Some(i) => end = i,
                None => return false,
            }
        }
    }

    /// Whether Vitest's default `exclude` decides for this file and differs by Vitest
    /// version: no `exclude` is configured, and the file is below a directory the
    /// default list held before Vitest 4.
    fn default_exclude_depends_on_version(&self, norm: &str) -> bool {
        self.vitest
            && self.vitest_exclude.is_none()
            && norm
                .split('/')
                .rev()
                .skip(1)
                .any(|dir| VITEST_OLD_DEFAULT_EXCLUDED_DIRS.contains(&dir))
    }

    /// Whether Vitest's default `exclude` decides for this file by its name and differs
    /// by Vitest version: no `exclude` is configured, and the file is named
    /// `<tool>.config.<anything>` for one of the tools the default list named before
    /// Vitest 4.
    fn default_exclude_names_configuration(&self, norm: &str) -> bool {
        let name = norm.rsplit('/').next().unwrap_or(norm);
        self.vitest
            && self.vitest_exclude.is_none()
            && VITEST_OLD_DEFAULT_EXCLUDED_CONFIGS.iter().any(|tool| {
                name.strip_prefix(tool)
                    .is_some_and(|rest| rest.starts_with(".config."))
            })
    }

    /// What the root Deno configuration says about `norm`, in a repository where no
    /// other runner configuration was found. `None` when Deno is not the runner here:
    /// there is no root Deno configuration, or another runner's was found.
    ///
    /// As `deno test` (deno 2.6) with no path argument does: the last `exclude` entry
    /// that holds the file decides (a negated one brings it back); with `test.include`
    /// set, a file an entry names, or a glob entry matches, runs whatever its name, a
    /// file below a directory entry runs when its name is one of Deno's default names,
    /// and any other file does not; with none, the default names decide for every file.
    fn deno_status(&self, norm: &str) -> Option<DenoStatus> {
        if self.config_parsed || self.second_runner.is_some() {
            return None;
        }
        if let Some((_, reason)) = &self.deno_problem {
            return Some(DenoStatus::NotDetermined(reason));
        }
        let (_, deno) = self.deno.as_ref()?;
        if self.nested_config_for(norm) {
            return Some(DenoStatus::NotDetermined(NESTED_RUNNER_CONFIG));
        }
        let below_nested_deno = self
            .nested_deno
            .iter()
            .any(|dir| norm.starts_with(&format!("{dir}/")));
        if deno.workspace && below_nested_deno {
            return Some(DenoStatus::NotDetermined(DENO_WORKSPACE_MEMBER));
        }
        // An entry is a path relative to the file, or a glob.
        let glob = |entry: &str| {
            globset::GlobBuilder::new(entry)
                .literal_separator(true)
                .build()
                .ok()
                .map(|g| g.compile_matcher())
        };
        let below = |entry: &str| entry.is_empty() || norm == entry || dir_holds(entry, norm);
        let unresolved = DenoStatus::NotDetermined(DENO_ENTRY_UNRESOLVED);
        let mut excluded = false;
        for exclude in deno.exclude.iter().rev() {
            let Some(entry) = clean_relative(&exclude.entry) else {
                return Some(unresolved);
            };
            let holds = if deno_entry_is_glob(&entry) {
                // A glob that matches a directory leaves out what it holds.
                let Some(matcher) = glob(&entry) else {
                    return Some(unresolved);
                };
                let mut end = norm.len();
                loop {
                    if matcher.is_match(&norm[..end]) {
                        break true;
                    }
                    match norm[..end].rfind('/') {
                        Some(i) => end = i,
                        None => break false,
                    }
                }
            } else {
                below(&entry)
            };
            if holds {
                excluded = !exclude.negated;
                break;
            }
        }
        if excluded {
            return Some(DenoStatus::LeftOut(Some("exclude")));
        }
        let lower = norm.to_ascii_lowercase();
        let in_node_modules = format!("/{norm}").contains("/node_modules/");
        let by_name = || {
            if in_node_modules || !deno_default_name(norm) {
                DenoStatus::LeftOut(None)
            } else if deno.vendor && norm.starts_with("vendor/") {
                DenoStatus::LeftOut(Some("vendor"))
            } else {
                DenoStatus::Collected
            }
        };
        // A declaration file runs nothing, named or not.
        if is_declaration_file(&lower) {
            return Some(DenoStatus::LeftOut(None));
        }
        if deno.task_paths {
            return Some(DenoStatus::NotDetermined(DENO_TASK_PATHS));
        }
        let Some(include) = &deno.include else {
            return Some(by_name());
        };
        let (mut named, mut below_directory) = (false, false);
        for raw in include {
            let Some(entry) = clean_relative(raw) else {
                return Some(unresolved);
            };
            if deno_entry_is_glob(&entry) {
                // A glob names files: one that matches a directory finds nothing in it.
                let Some(matcher) = glob(&entry) else {
                    return Some(unresolved);
                };
                named |= matcher.is_match(norm);
            } else if entry == norm {
                named = true;
            } else {
                below_directory |= below(&entry);
            }
        }
        let named_by_path = include
            .iter()
            .any(|raw| clean_relative(raw).as_deref() == Some(norm));
        Some(if named && in_node_modules && !named_by_path {
            DenoStatus::NotDetermined(DENO_NODE_MODULES)
        } else if named {
            DenoStatus::Collected
        } else if below_directory && deno.vendor && norm.starts_with("vendor/") {
            DenoStatus::NotDetermined(DENO_VENDOR_INCLUDED)
        } else if below_directory {
            by_name()
        } else {
            DenoStatus::LeftOut(Some("include"))
        })
    }

    /// Whether Jest's default patterns decide for this file and differ by Jest version:
    /// no `testMatch` / `testRegex` is configured, and the extension is one of `.mjs`,
    /// `.cjs`, `.mts`, `.cts`.
    fn default_patterns_depend_on_version(&self, lower: &str) -> bool {
        !self.vitest
            && self.test_regex.is_empty()
            && self.test_match.is_empty()
            && self.include.is_empty()
            && [".mjs", ".cjs", ".mts", ".cts"]
                .iter()
                .any(|ext| lower.ends_with(ext))
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
        // Jest's file map holds no path below `node_modules` (`haste.retainAllFiles`
        // is off unless set), so no pattern can collect one.
        if !self.vitest && rooted.contains("/node_modules/") {
            return false;
        }
        if self.compiled_ignores.iter().any(|re| re.is_match(&rooted)) {
            return false;
        }
        if self.vitest_excludes(norm) {
            return false;
        }

        // Config was parsed: check configured patterns or defaults
        let has_custom =
            !self.test_regex.is_empty() || !self.test_match.is_empty() || !self.include.is_empty();

        if has_custom {
            return self.compiled_regexes.iter().any(|re| re.is_match(&rooted))
                || self.globs_keep(norm);
        }

        let filename = norm.rsplit('/').next().unwrap_or(norm);
        let f_lower = filename.to_ascii_lowercase();
        let stem = f_lower.rsplit_once('.').map_or("", |(stem, _)| stem);
        if self.vitest {
            // Vitest's default `include` is `**/*.{test,spec}.?(c|m)[jt]s?(x)`: a
            // `__tests__` directory means nothing to it, and the name needs a stem.
            return [".test", ".spec"]
                .iter()
                .any(|end| stem.strip_suffix(end).is_some_and(|rest| !rest.is_empty()));
        }

        // Default Jest collection conventions
        // 1. Inside __tests__/
        if norm.contains("/__tests__/") || norm.starts_with("__tests__/") {
            return true;
        }

        // 2. Basename is `test.<ext>` / `spec.<ext>` or ends in `.test.<ext>` / `.spec.<ext>`
        matches!(stem, "test" | "spec") || stem.ends_with(".test") || stem.ends_with(".spec")
    }

    /// The rules of the parsed configuration that leave `norm` out, for
    /// [`RunnerCollectionRules::mechanisms`]: none when the configuration is not one
    /// this model evaluates, or when only the runner's default names leave the file out.
    fn mechanisms(&self, norm: &str, out: &mut Vec<Mechanism>) {
        let lower = norm.to_ascii_lowercase();
        let evaluated = js_runner_extension(&lower)
            && self.script_problem.is_none()
            && self.conflict.is_none()
            && self.invalid_pattern.is_none()
            && self.unparseable_config.is_none()
            && self.mocha_detected.is_none()
            && self.config_parsed
            && !self.nested_config_for(norm);
        if js_runner_extension(&lower)
            && self.script_problem.is_none()
            && self.invalid_pattern.is_none()
            && self.unparseable_config.is_none()
            && self.mocha_detected.is_none()
        {
            if let (Some(DenoStatus::LeftOut(Some(key))), Some((source, _))) =
                (self.deno_status(norm), &self.deno)
            {
                let (key, what) = match key {
                    "exclude" => ("deno-exclude", "`exclude` / `test.exclude` in"),
                    "vendor" => ("deno-vendor", "`vendor` in"),
                    _ => ("deno-include", "`test.include` in"),
                };
                out.push(Mechanism::new(source, key, what));
                return;
            }
        }
        let Some(source) = self.config_source.as_deref().filter(|_| evaluated) else {
            return;
        };
        if !self.effective_roots.is_empty()
            && !self
                .effective_roots
                .iter()
                .any(|r| norm == *r || norm.starts_with(&format!("{r}/")))
        {
            let (key, what) = if self.vitest {
                ("root", "`root` / `dir` in")
            } else {
                ("roots", "`roots` in")
            };
            out.push(Mechanism::new(source, key, what));
            return;
        }
        let rooted = format!("/{norm}");
        if self.compiled_ignores.iter().any(|re| re.is_match(&rooted)) {
            // Jest's own default leaves `node_modules` out with no key written.
            if self.ignore_patterns.is_some() {
                out.push(Mechanism::new(
                    source,
                    "testPathIgnorePatterns",
                    "`testPathIgnorePatterns` in",
                ));
            }
            return;
        }
        if self.vitest_excludes(norm) {
            if self.vitest_exclude.is_some() {
                out.push(Mechanism::new(source, "exclude", "`exclude` in"));
            }
            return;
        }
        let has_custom =
            !self.test_regex.is_empty() || !self.test_match.is_empty() || !self.include.is_empty();
        if has_custom && !self.configuration_collects(norm) {
            let (key, what) = if self.vitest {
                ("include", "`include` in")
            } else {
                ("testMatch", "`testMatch` / `testRegex` in")
            };
            out.push(Mechanism::new(source, key, what));
        }
    }

    /// Jest's rule for a list of globs: in configured order, a negated glob that
    /// matches drops the path and a plain glob that matches keeps it; a list of only
    /// negated globs keeps what none of them drops.
    fn globs_keep(&self, norm: &str) -> bool {
        let mut kept = None;
        let mut negatives = 0;
        for (matcher, negated) in &self.compiled_globs {
            let matched = matcher.as_ref().is_some_and(|m| m.is_match(norm));
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

        // The Jest major the manifest pins decides Jest's default patterns for the
        // module extensions.
        self.jest_major = ["devDependencies", "dependencies"]
            .iter()
            .find_map(|key| val.get(*key).and_then(|d| d.get("jest")))
            .and_then(|range| range.as_str())
            .and_then(plain_semver_major);
        // And the Vitest major decides Vitest's default `exclude`.
        self.vitest_major = ["devDependencies", "dependencies"]
            .iter()
            .find_map(|key| val.get(*key).and_then(|d| d.get("vitest")))
            .and_then(|range| range.as_str())
            .and_then(plain_semver_major);

        // Jest reads a `jest` key of `package.json`. Vitest reads no key there: its
        // configuration is a `vitest.config.*` or `vite.config.*` file.
        if let Some(jest) = val.get("jest") {
            self.config_parsed = true;
            self.config_source = Some("package.json".to_string());
            self.extract_from_json(jest);
        }
        self.node_test_script = scripts_run_node_test(&val);

        self.compile_patterns();
    }

    /// Forgets what the default Jest configuration sources said: Jest run with
    /// `--config` reads that file and nothing else.
    fn reset_jest_configuration(&mut self) {
        self.config_parsed = false;
        self.config_source = None;
        self.unparseable_config = None;
        self.test_match.clear();
        self.test_regex.clear();
        self.include.clear();
        self.root_dir = None;
        self.roots = None;
        self.vitest_root = None;
        self.ignore_patterns = None;
        self.unread_key = None;
        self.preset = false;
        self.conflict = None;
    }

    /// Applies how the `package.json` scripts run Jest: the configuration `--config`
    /// names replaces the default sources, and `--rootDir` the configured one.
    fn apply_jest_scripts<F>(&mut self, package: &serde_json::Value, reader: &mut F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        let scripts = read_jest_scripts(package);
        self.script_problem = scripts.unreadable;
        if scripts.other && self.second_runner.is_none() {
            self.second_runner =
                Some("a package script runs jest with another configuration".to_string());
        }
        if let Some(named) = &scripts.config {
            self.reset_jest_configuration();
            let inline = named.trim_start().starts_with('{');
            let path = clean_relative(named).filter(|p| p.ends_with(".json") && !inline);
            let content = if inline {
                Some(named.clone())
            } else {
                path.as_deref()
                    .filter(|p| *p != "package.json" && !p.ends_with("/package.json"))
                    .and_then(&mut *reader)
            };
            match content.and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok()) {
                Some(config) => {
                    self.config_parsed = true;
                    // An object on the command line is written in the manifest.
                    self.config_source =
                        Some(path.clone().unwrap_or_else(|| "package.json".to_string()));
                    self.extract_from_json(&config);
                    // `rootDir` is relative to the configuration file, and defaults to
                    // its directory.
                    let dir = path
                        .as_deref()
                        .and_then(|p| p.rfind('/').map(|i| p[..i].to_string()))
                        .unwrap_or_default();
                    if !dir.is_empty() {
                        let configured = match &self.root_dir {
                            None => Some(String::new()),
                            Some(v) => v.as_str().map(str::to_string),
                        };
                        let resolved = configured
                            .filter(|c| !c.starts_with('/') && !c.contains('<'))
                            .and_then(|c| clean_relative(&join_dir(&dir, &c)));
                        self.root_dir = Some(match resolved {
                            Some(r) => serde_json::Value::String(r),
                            // Not a repository path: `compile_patterns` says so.
                            None => serde_json::Value::String("..".to_string()),
                        });
                    }
                }
                None => self.script_problem = Some(JEST_SCRIPT_CONFIG_UNREAD),
            }
        }
        if let Some(root_dir) = &scripts.root_dir {
            self.root_dir = Some(serde_json::Value::String(root_dir.clone()));
        }
        self.compile_patterns();
    }

    /// Applies a root `vitest.config.*` or `vite.config.*` file named `name`.
    fn apply_vitest_config(&mut self, name: &str, source: &str, vitest_dependency: bool) {
        let is_vite = name.starts_with("vite.");
        match parse_vitest_config(name, source) {
            VitestConfig::Literal(literal) => {
                self.config_parsed = true;
                self.config_source = Some(name.to_string());
                self.vitest = true;
                if let Some(include) = literal.include {
                    self.include = include;
                }
                self.vitest_exclude = literal.exclude;
                self.vitest_base = literal.base;
            }
            // A `vitest.config.*` with no `test` block runs Vitest with its defaults. A
            // `vite.config.*` with none configures nothing by itself: whether Vitest
            // runs is read from the dependencies, as it is with no file at all.
            VitestConfig::NoTestBlock if !is_vite => {
                self.config_parsed = true;
                self.vitest = true;
            }
            VitestConfig::NoTestBlock => {}
            // A computed `vite.config.*` may hold a `test` block only where Vitest is
            // a dependency.
            VitestConfig::Dynamic if is_vite && !vitest_dependency => {}
            VitestConfig::Dynamic => self.unparseable_config = Some(name.to_string()),
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
        self.config_source = Some("jest.config.json".to_string());
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
        let regex_set = match val.get("testRegex") {
            Some(serde_json::Value::String(regex)) => !regex.is_empty(),
            Some(serde_json::Value::Array(list)) => !list.is_empty(),
            _ => false,
        };
        if regex_set && val.get("testMatch").is_some_and(|v| v.is_array()) {
            self.conflict = Some(JEST_MATCH_AND_REGEX);
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
                // An empty `testRegex` is none: Jest 27.5 and 29.7 list what
                // `testMatch` or the defaults match.
                self.test_regex = if s.is_empty() {
                    Vec::new()
                } else {
                    vec![s.to_string()]
                };
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
    /// `test` left on: `cargo test` builds and runs the target.
    pub tested: bool,
    /// `harness` left on. With it off, the target's own `main` is the test run.
    pub harness: bool,
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
    /// `[package] name`.
    pub name: Option<String>,
    /// `[package] autobins`: `src/main.rs` and the files of `src/bin/` are binaries.
    pub autobins: bool,
    /// `[package] autolib`: `src/lib.rs` is the library.
    pub autolib: bool,
    /// Every `[[bin]]` target, running or not.
    pub bins: Vec<CargoBinTarget>,
}

/// A `[[bin]]` target as its manifest declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoBinTarget {
    pub name: Option<String>,
    pub path: Option<String>,
    /// `test` and `harness` both left on: the binary's unit tests run.
    pub runs: bool,
}

/// The files of a package reached from its crate roots through `mod` declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceModules {
    /// From the library root.
    pub lib: TestModules,
    /// From the roots of the binaries whose unit tests run.
    pub bins: TestModules,
    /// A crate root of the package is tracked. Without one there is nothing to follow
    /// modules from, and the files under `src/` count as the manifest alone says.
    pub has_root: bool,
}

/// The files reached from a package's running test targets through `mod` declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestModules {
    pub reached: HashSet<String>,
    /// A reached file declares modules in a way that is not followed (a macro, an
    /// `include!`, a `path` under `cfg_attr` or inside an inline module, a file that
    /// does not parse): a file not reached may still be a module.
    pub open: bool,
    /// The file whose `mod` declaration reaches each reached file that is not a root.
    pub parents: HashMap<String, String>,
}

/// The files a target reaches that its manifest keeps `cargo test` from running: the
/// key that does it, in the words of a [`Mechanism`], and the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchedOff {
    pub key: &'static str,
    pub what: &'static str,
    pub reached: HashSet<String>,
}

/// Whether `cargo test` builds and runs the tests of a Rust file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustCollection {
    Collected,
    NotCollected,
    Unknown(String),
    /// A file under `src/` that no crate root reaches, or a file under `tests/` that
    /// no test target reaches, through the `mod` declarations read here: not compiled
    /// as far as this model can tell, and left out with a note that gives this reason.
    Unreached(&'static str),
}

/// Why a file under `tests/` that no test target reaches is left out, for the note.
const RUST_TEST_MODULE_UNREACHED: &str =
    "a Rust file under `tests/` that no test target reaches through a `mod` declaration read here is not compiled, unless a macro of another crate declares it, which is not followed";

/// Why a file under `src/` that no crate root reaches is left out, for the note.
const RUST_SOURCE_UNREACHED: &str =
    "a Rust file under `src/` that no crate root reaches through a `mod` declaration read here is not compiled, unless a macro of another crate declares it, which is not followed";

const RUST_SOURCE_MODULES_OPEN: &str =
    "a Cargo crate root declares modules in a way that is not followed";

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
    /// Module reachability from each package's library and binary roots, by package
    /// directory. Present only when the tree was listed.
    pub source_modules: std::collections::HashMap<String, SourceModules>,
    /// The `exclude` list of every manifest with a `[workspace]` table, by its
    /// directory (empty for the repository root).
    pub workspaces: std::collections::HashMap<String, Vec<String>>,
    /// The files of each package that a target its manifest switches off reaches, by
    /// package directory. Present only when the tree was listed.
    pub switched_off: std::collections::HashMap<String, Vec<SwitchedOff>>,
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
fn rust_outer_attributes<'a>(
    node: Node<'a>,
    anc: &Ancestry<'a>,
    src: &[u8],
) -> Vec<(String, Node<'a>)> {
    let mut out = Vec::new();
    let mut prev = anc.prev_sibling(node);
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
        prev = anc.prev_sibling(p);
    }
    out
}

fn scan_rust_module_items<'t>(
    node: Node<'t>,
    anc: &Ancestry<'t>,
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
                // A raw identifier names the file without its `r#` (`mod r#go;` is
                // `go.rs`).
                let name = name.strip_prefix("r#").unwrap_or(name);
                let mut path = None;
                for (attr_name, attr) in rust_outer_attributes(item, anc, src) {
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
                        scan_rust_module_items(body, anc, src, parents, scan);
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
    let Ok(tree) = crate::ast::source_text::parse(&mut parser, source) else {
        scan.open = true;
        return scan;
    };
    if tree.root_node().has_error() {
        scan.open = true;
    }
    scan_rust_module_items(
        tree.root_node(),
        &Ancestry::new(tree.root_node()),
        source.as_bytes(),
        &mut Vec::new(),
        &mut scan,
    );
    scan
}

/// The files reached from `roots` through `mod` declarations, as rustc resolves them:
/// `mod x;` is `x.rs` or `x/mod.rs` beside a crate root, a `mod.rs`, a `main.rs` or a
/// `lib.rs`, and under the directory named after any other file; `#[path]` on a
/// module of the file itself is relative to the file's directory. `skim` skips the
/// parse of a file that cannot declare a module (neither `mod` nor `include` in it).
fn follow_rust_modules<F>(
    roots: Vec<String>,
    rust_files: &HashSet<&str>,
    reader: &mut F,
    skim: bool,
) -> TestModules
where
    F: FnMut(&str) -> Option<String>,
{
    let mut modules = TestModules::default();
    // `(file, whether its modules live beside it rather than under its stem)`
    let mut queue: Vec<(String, bool)> = roots.into_iter().map(|root| (root, true)).collect();
    while let Some((file, owns_dir)) = queue.pop() {
        if !modules.reached.insert(file.clone()) {
            continue;
        }
        let Some(source) = reader(&file) else {
            modules.open = true;
            continue;
        };
        if skim && !source.contains("mod") && !source.contains("include") {
            continue;
        }
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
                    Some(target) if parents.is_empty() && rust_files.contains(target.as_str()) => {
                        modules
                            .parents
                            .entry(target.clone())
                            .or_insert_with(|| file.clone());
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
                    modules
                        .parents
                        .entry(candidate.clone())
                        .or_insert_with(|| file.clone());
                    queue.push((candidate, false));
                }
            }
        }
    }
    modules
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

    /// The workspace that leaves the package at `dir` out through its `exclude` list:
    /// the nearest manifest above it with a `[workspace]` table. A package that is a
    /// workspace root itself belongs to no other.
    fn excluding_workspace(&self, dir: &str) -> Option<String> {
        if self.workspaces.contains_key(dir) || dir.is_empty() {
            return None;
        }
        let mut above = dir;
        loop {
            above = match above.rfind('/') {
                Some(i) => &above[..i],
                None => "",
            };
            if let Some(excluded) = self.workspaces.get(above) {
                let named = excluded.iter().any(|entry| {
                    clean_relative(&join_dir(above, entry)).is_some_and(|path| {
                        !path.is_empty() && (dir == path || dir.starts_with(&format!("{path}/")))
                    })
                });
                return named.then(|| join_dir(above, "Cargo.toml"));
            }
            if above.is_empty() {
                return None;
            }
        }
    }

    /// The rules that keep `cargo test` from running the tests of `norm`, for
    /// [`RunnerCollectionRules::mechanisms`]: the workspace manifest whose `exclude`
    /// list names its package, the key of its own manifest that switches off the target
    /// that reaches it (`test = false` or `harness = false` on a `[[test]]`, `[lib]` or
    /// `[[bin]]`, `autotests = false`), and the file a `mod` declaration that reached it
    /// on the base side (`before`) is gone from.
    fn mechanisms(&self, norm: &str, before: Option<&Self>, out: &mut Vec<Mechanism>) {
        if !norm.ends_with(".rs") {
            return;
        }
        let Some((dir, _)) = self.owning_package(norm) else {
            return;
        };
        if let Some(manifest) = self.excluding_workspace(&dir) {
            out.push(Mechanism::new(
                &manifest,
                "workspace-exclude",
                "The `exclude` list of the workspace in",
            ));
        }
        let manifest = join_dir(&dir, "Cargo.toml");
        let mut switched = false;
        for off in self.switched_off.get(&dir).into_iter().flatten() {
            if off.reached.contains(norm) {
                switched = true;
                out.push(Mechanism::new(&manifest, off.key, off.what));
            }
        }
        if switched || self.reaches(&dir, norm) {
            return;
        }
        // No target of the head side reaches the file. The declaration that is gone
        // was in the nearest file up the base side's chain that the head side still
        // reaches.
        let Some(before) = before else {
            return;
        };
        let mut child = norm.to_string();
        let mut steps = 0;
        while let Some(parent) = before.parent_of(&dir, &child) {
            if self.reaches(&dir, &parent) {
                out.push(Mechanism::new(
                    &parent,
                    "mod-removed",
                    "The `mod` declaration no longer in",
                ));
                return;
            }
            child = parent;
            steps += 1;
            if steps > 256 {
                return;
            }
        }
    }

    /// Whether a target of the package at `dir` whose tests run reaches `norm`.
    fn reaches(&self, dir: &str, norm: &str) -> bool {
        self.test_modules
            .get(dir)
            .is_some_and(|m| m.reached.contains(norm))
            || self
                .source_modules
                .get(dir)
                .is_some_and(|s| s.lib.reached.contains(norm) || s.bins.reached.contains(norm))
    }

    /// The file whose `mod` declaration reaches `norm` in the package at `dir`.
    fn parent_of(&self, dir: &str, norm: &str) -> Option<String> {
        let tests = self.test_modules.get(dir).map(|m| &m.parents);
        let source = self.source_modules.get(dir);
        tests
            .into_iter()
            .chain(source.map(|s| &s.lib.parents))
            .chain(source.map(|s| &s.bins.parents))
            .find_map(|parents| parents.get(norm).cloned())
    }

    /// Whether `cargo test` runs the tests of `path`, as the owning manifest declares
    /// its targets. Without a parsed owning manifest the path rule of
    /// [`Self::is_collected`] decides. A file its own package runs, in a package the
    /// workspace above it excludes, is not determined: `cargo test` at the workspace
    /// root does not build the package, and its own invocation does.
    pub fn status(&self, path: &str) -> RustCollection {
        let raw = path.replace('\\', "/");
        let norm = raw.strip_prefix("./").unwrap_or(&raw);
        let status = self.status_in_package(norm);
        if status != RustCollection::Collected {
            return status;
        }
        let Some((dir, _)) = self.owning_package(norm) else {
            return status;
        };
        match self.excluding_workspace(&dir) {
            Some(workspace) => RustCollection::Unknown(format!(
                "the package in `{dir}` is in the `exclude` list of the workspace in `{workspace}`, so `cargo test` in that workspace does not run its tests"
            )),
            None => status,
        }
    }

    fn status_in_package(&self, norm: &str) -> RustCollection {
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
                    "a Cargo test target declares modules in a way that is not followed"
                        .to_string(),
                ),
                // A target the manifest switches off reaches it: the manifest decides.
                Some(_)
                    if self
                        .switched_off
                        .get(&dir)
                        .is_some_and(|offs| offs.iter().any(|off| off.reached.contains(norm))) =>
                {
                    RustCollection::NotCollected
                }
                Some(_) => RustCollection::Unreached(RUST_TEST_MODULE_UNREACHED),
            };
        }
        let lib_tests = || {
            if pkg.lib_tests_run {
                RustCollection::Collected
            } else if pkg.has_binary == Some(false) {
                RustCollection::NotCollected
            } else {
                RustCollection::Unknown(
                    "`[lib] test = false` in a package that may also build a binary".to_string(),
                )
            }
        };
        if rel.starts_with("src/") {
            // A file under `src/` is built as a module of the library or of a binary:
            // one that no crate root reaches is not compiled, so its tests do not run.
            let Some(source) = self.source_modules.get(&dir).filter(|s| s.has_root) else {
                return lib_tests();
            };
            if source.bins.reached.contains(norm)
                || (pkg.lib_tests_run && source.lib.reached.contains(norm))
            {
                return RustCollection::Collected;
            }
            // Only a root whose tests run could still reach it, through a declaration
            // that is not followed.
            if source.bins.open || (pkg.lib_tests_run && source.lib.open) {
                return RustCollection::Unknown(RUST_SOURCE_MODULES_OPEN.to_string());
            }
            // A target the manifest switches off reaches it: the manifest decides.
            let switched_off = source.lib.reached.contains(norm)
                || self
                    .switched_off
                    .get(&dir)
                    .is_some_and(|offs| offs.iter().any(|off| off.reached.contains(norm)));
            return if switched_off {
                RustCollection::NotCollected
            } else {
                RustCollection::Unreached(RUST_SOURCE_UNREACHED)
            };
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
            return RustCollection::Unknown(
                "a Cargo target `path` outside `src/` and `tests/`".to_string(),
            );
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
                    tested: flag(t, "test"),
                    harness: flag(t, "harness"),
                })
                .collect(),
            has_binary: (!tables("bin").is_empty()).then_some(true),
            name: package
                .get("name")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            autobins: flag(&val["package"], "autobins"),
            autolib: flag(&val["package"], "autolib"),
            bins: tables("bin")
                .iter()
                .map(|b| CargoBinTarget {
                    name: b
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string),
                    path: path_of(b),
                    runs: flag(b, "test") && flag(b, "harness"),
                })
                .collect(),
        })
    }

    /// Records the manifest at `path` and, when it is a package, its targets.
    fn add_manifest(&mut self, path: &str, content: &str) {
        self.manifests
            .insert(path.to_string(), Self::manifest_status(content));
        let dir = path.strip_suffix("Cargo.toml").unwrap_or("");
        let workspace = toml::from_str::<toml::Value>(content)
            .ok()
            .and_then(|val| val.get("workspace").cloned());
        if let Some(workspace) = workspace.filter(toml::Value::is_table) {
            let excluded = workspace
                .get("exclude")
                .and_then(toml::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|e| e.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            self.workspaces
                .insert(dir.trim_end_matches('/').to_string(), excluded);
        }
        if let Some(pkg) = Self::parse_package(content) {
            let dir = path.strip_suffix("Cargo.toml").unwrap_or("");
            self.packages
                .insert(dir.trim_end_matches('/').to_string(), pkg);
        }
    }

    /// With every tracked path known: which packages build a binary, and which files
    /// each package's running test targets, its library and its binaries reach through
    /// `mod` declarations.
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
        let mut auto_bins: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut binaries: HashSet<String> = HashSet::new();
        // The test targets a manifest switches off, by package: those a `[[test]]`
        // entry names, and those `autotests = false` leaves undiscovered.
        let mut targets_off: std::collections::HashMap<String, Vec<String>> = Default::default();
        let mut undiscovered: std::collections::HashMap<String, Vec<String>> = Default::default();
        // In path order: the roots of a package are followed in the order they are
        // listed here, and the first to reach a file is recorded as its parent
        // (`parent_of`), which a finding names.
        let mut in_order: Vec<&str> = rust_files.iter().copied().collect();
        in_order.sort_unstable();
        for file in &in_order {
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
            // The binaries Cargo discovers by itself: `src/main.rs`, `src/bin/<name>.rs`
            // and `src/bin/<name>/main.rs`.
            let auto_bin = rel == "src/main.rs"
                || rel
                    .strip_prefix("src/bin/")
                    .is_some_and(|rest| match rest.split_once('/') {
                        None => true,
                        Some((_, file)) => file == "main.rs",
                    });
            if auto_bin && pkg.autobins {
                auto_bins
                    .entry(dir.clone())
                    .or_default()
                    .push((*file).to_string());
            }
            let named = pkg.tests.iter().find(|t| t.names(rel));
            let is_root = match named {
                Some(target) => target.runs,
                None => pkg.autotests && is_auto_test_root(rel),
            };
            if is_root {
                roots.entry(dir).or_default().push((*file).to_string());
            } else if named.is_some() {
                targets_off
                    .entry(dir)
                    .or_default()
                    .push((*file).to_string());
            } else if is_auto_test_root(rel) {
                undiscovered
                    .entry(dir)
                    .or_default()
                    .push((*file).to_string());
            }
        }
        for (dir, pkg) in &mut self.packages {
            if pkg.has_binary.is_none() {
                pkg.has_binary = Some(binaries.contains(dir));
            }
        }
        // In path order: the files of a package are read as it is reached, and a caller
        // that keeps the first read errors names the same ones on every run.
        let mut dirs: Vec<String> = self.packages.keys().cloned().collect();
        dirs.sort_unstable();
        for dir in dirs {
            let test_roots = roots.remove(&dir).unwrap_or_default();
            let modules = follow_rust_modules(test_roots, &rust_files, reader, false);
            self.test_modules.insert(dir.clone(), modules);
            let mut switched_off = Vec::new();
            for (roots, key, what) in [
                (
                    targets_off.remove(&dir),
                    "test-target-off",
                    "`test = false` or `harness = false` on a `[[test]]` target in",
                ),
                (
                    undiscovered.remove(&dir),
                    "autotests-off",
                    "`autotests = false` in",
                ),
            ] {
                let Some(roots) = roots else {
                    continue;
                };
                let reached = follow_rust_modules(roots, &rust_files, reader, false).reached;
                switched_off.push(SwitchedOff { key, what, reached });
            }

            let Some(pkg) = self.packages.get(&dir) else {
                continue;
            };
            let tracked_in_package = |rel: &str| {
                let path = join_dir(&dir, rel);
                rust_files.contains(path.as_str()).then_some(path)
            };
            let lib_root = match &pkg.lib_path {
                Some(path) => tracked_in_package(path),
                None if pkg.autolib => tracked_in_package("src/lib.rs"),
                None => None,
            };
            // A `[[bin]]` target without a `path` is the file Cargo infers from its name.
            let mut unresolved_bin = false;
            let mut declared: Vec<(String, bool)> = Vec::new();
            for bin in &pkg.bins {
                let root = match (&bin.path, &bin.name) {
                    (Some(path), _) => tracked_in_package(path),
                    (None, Some(name)) => tracked_in_package(&format!("src/bin/{name}.rs"))
                        .or_else(|| tracked_in_package(&format!("src/bin/{name}/main.rs")))
                        .or_else(|| {
                            (pkg.name.as_deref() == Some(name.as_str()))
                                .then(|| tracked_in_package("src/main.rs"))
                                .flatten()
                        }),
                    (None, None) => None,
                };
                match root {
                    Some(root) => declared.push((root, bin.runs)),
                    None => unresolved_bin = true,
                }
            }
            let mut running_bins: Vec<String> = declared
                .iter()
                .filter(|(_, runs)| *runs)
                .map(|(root, _)| root.clone())
                .collect();
            let bins_off: Vec<String> = declared
                .iter()
                .filter(|(_, runs)| !*runs)
                .map(|(root, _)| root.clone())
                .collect();
            for root in auto_bins.remove(&dir).unwrap_or_default() {
                if !declared.iter().any(|(named, _)| *named == root) {
                    running_bins.push(root);
                }
            }
            let has_root = lib_root.is_some() || !declared.is_empty() || !running_bins.is_empty();
            let lib =
                follow_rust_modules(lib_root.into_iter().collect(), &rust_files, reader, true);
            let mut bins = follow_rust_modules(running_bins, &rust_files, reader, true);
            // A binary whose root file is not found may be the one that reaches a file.
            bins.open |= unresolved_bin;
            if !pkg.lib_tests_run && !lib.reached.is_empty() {
                switched_off.push(SwitchedOff {
                    key: "lib-test-off",
                    what: "`test = false` or `harness = false` under `[lib]` in",
                    reached: lib.reached.clone(),
                });
            }
            if !bins_off.is_empty() {
                switched_off.push(SwitchedOff {
                    key: "bin-test-off",
                    what: "`test = false` or `harness = false` on a `[[bin]]` target in",
                    reached: follow_rust_modules(bins_off, &rust_files, reader, true).reached,
                });
            }
            if !switched_off.is_empty() {
                self.switched_off.insert(dir.clone(), switched_off);
            }
            self.source_modules.insert(
                dir,
                SourceModules {
                    lib,
                    bins,
                    has_root,
                },
            );
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
pub fn cfg_mentions_feature<'t>(node: Node<'t>, anc: &Ancestry<'t>, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier"
            && n.utf8_text(src).ok() == Some("feature")
            && anc.next_sibling(n).is_some_and(|s| s.kind() == "=")
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
    pub go: GoCollectionRules,
    /// `linguist-vendored` / `linguist-generated`, from the tracked `.gitattributes`.
    pub attributes: GitAttributes,
}

/// What the content and the place of a Go test file say about a default `go test`.
/// Present only when the tree was listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoCollectionRules {
    /// The build constraint of each `_test.go` file a default build does not select.
    pub constraints: HashMap<String, GoBuild>,
    /// The 1-based line of each constraint in `constraints`.
    pub constraint_lines: HashMap<String, usize>,
    /// The directory of every tracked `go.mod` (empty for the repository root).
    pub modules: Vec<String>,
    /// Every tracked `go.work`, by its directory (empty for the repository root).
    pub works: HashMap<String, GoWork>,
}

/// Whether the directory `dir` (empty for the repository root) holds `path`.
fn dir_holds(dir: &str, path: &str) -> bool {
    dir.is_empty() || path.starts_with(&format!("{dir}/"))
}

/// A directory as a report names it.
fn named_dir(dir: &str) -> String {
    if dir.is_empty() {
        "at the repository root".to_string()
    } else {
        format!("in `{dir}`")
    }
}

impl GoCollectionRules {
    /// The module that holds the file at `norm`: the nearest `go.mod` above it.
    fn module_of(&self, norm: &str) -> Option<&String> {
        self.modules
            .iter()
            .filter(|m| dir_holds(m, norm))
            .max_by_key(|m| m.len())
    }

    /// The module of `norm` and the module it is nested below, when there is one.
    /// `./...` in a module never matches a package of a module nested below it.
    fn nesting(&self, norm: &str) -> Option<(&String, &String)> {
        let own = self.module_of(norm).filter(|m| !m.is_empty())?;
        let parent = self
            .modules
            .iter()
            .filter(|m| m.len() < own.len() && dir_holds(m, &format!("{own}/")))
            .max_by_key(|m| m.len())?;
        Some((own, parent))
    }

    /// The `go.work` in force for the module of `norm`, by its directory: the nearest
    /// one at or above the module. `None` for a file in no module.
    fn work_of(&self, norm: &str) -> Option<(&String, &GoWork, &String)> {
        let own = self.module_of(norm)?;
        self.works
            .iter()
            .filter(|(dir, _)| dir.is_empty() || *dir == own || dir_holds(dir, &format!("{own}/")))
            .max_by_key(|(dir, _)| dir.len())
            .map(|(dir, work)| (dir, work, own))
    }

    /// The `go.work` that leaves the module of `norm` out of its `use` list, by its
    /// path. `Err` with the path when that file cannot be read.
    fn unused_by_work(&self, norm: &str) -> Result<Option<String>, String> {
        let Some((dir, work, own)) = self.work_of(norm) else {
            return Ok(None);
        };
        let file = join_dir(dir, "go.work");
        match work {
            GoWork::Unreadable => Err(file),
            GoWork::Uses(uses) => {
                let used = uses
                    .iter()
                    .filter_map(|entry| clean_relative(&join_dir(dir, entry)))
                    .any(|module| module == *own);
                Ok((!used).then_some(file))
            }
        }
    }

    /// The rules that take the `_test.go` file at `norm` out of a default `go test`,
    /// for [`RunnerCollectionRules::mechanisms`]. `before` is the base side, which says
    /// which `go.mod` of a nested pair the change added.
    fn mechanisms(&self, norm: &str, before: Option<&Self>, out: &mut Vec<Mechanism>) {
        if !norm.ends_with("_test.go") || go_tool_ignores(norm) {
            return;
        }
        if matches!(
            self.constraints.get(norm),
            Some(GoBuild::Never | GoBuild::NeedsTags(_))
        ) {
            let mut mechanism = Mechanism::new(norm, "build-constraint", "The build constraint of");
            mechanism.line = self.constraint_lines.get(norm).copied();
            out.push(mechanism);
        }
        if let Ok(Some(file)) = self.unused_by_work(norm) {
            out.push(Mechanism::new(
                &file,
                "go-work-use",
                "The `use` list, which leaves the module out, of",
            ));
        }
        if let Some((own, parent)) = self.nesting(norm) {
            // The `go.mod` the change added: the nested one, unless it was there.
            let nested_is_old = before.is_some_and(|b| b.modules.contains(own));
            let dir = if nested_is_old { parent } else { own };
            out.push(Mechanism::new(
                &join_dir(dir, "go.mod"),
                "nested-module",
                "The nesting of one Go module below another made by",
            ));
        }
    }

    /// Whether a default `go test ./...` builds the `_test.go` file at `norm`.
    fn status(&self, norm: &str) -> RunnerCollectionStatus {
        match self.constraints.get(norm) {
            Some(GoBuild::Never) => return RunnerCollectionStatus::NotCollected,
            Some(GoBuild::NeedsTags(tags)) => {
                let named: Vec<String> = tags.iter().map(|t| format!("`{t}`")).collect();
                let needs = match named.as_slice() {
                    [one] => format!("needs the tag {one}"),
                    several => format!("needs one of the tags {}", several.join(", ")),
                };
                return RunnerCollectionStatus::Unknown(format!(
                    "a Go build constraint {needs}, so a default `go test` does not run the file's tests and only `go test -tags` does"
                ));
            }
            Some(GoBuild::Unreadable) => {
                return RunnerCollectionStatus::Unknown(
                    "a Go build constraint that cannot be read".to_string(),
                )
            }
            Some(GoBuild::Built) | None => {}
        }
        match self.unused_by_work(norm) {
            Err(file) => {
                return RunnerCollectionStatus::Unknown(format!(
                    "the workspace file `{file}` cannot be read, so whether `go test` runs in the modules below it is not known"
                ))
            }
            Ok(Some(file)) => {
                let own = self.module_of(norm).map_or("", String::as_str);
                return RunnerCollectionStatus::Unknown(format!(
                    "the Go module {} is not in the `use` list of `{file}`, so `go test` in that module fails while the workspace file is in force",
                    named_dir(own)
                ));
            }
            Ok(None) => {}
        }
        if let Some((own, parent)) = self.nesting(norm) {
            let above = if parent.is_empty() {
                "at the repository root".to_string()
            } else {
                format!("in `{parent}`")
            };
            return RunnerCollectionStatus::Unknown(format!(
                "the Go module in `{own}` is below the module {above}, so `go test ./...` there does not run its tests"
            ));
        }
        RunnerCollectionStatus::Collected
    }
}

impl RunnerCollectionRules {
    /// Every rule read here that takes the file at `norm` out of the default run under
    /// these rules: a Cargo workspace `exclude` entry, a Cargo target its manifest
    /// switches off, a `mod` declaration that is gone, a Go build constraint, a nested
    /// `go.mod`, a `go.work` `use` list, pytest `testpaths` / `python_files` /
    /// `norecursedirs` / `collect_ignore`, Jest `roots` / `testPathIgnorePatterns` /
    /// `testMatch` / `testRegex`, Vitest `root` / `exclude` / `include`, the lists of a
    /// root Deno configuration, and a `.gitattributes` attribute. `[tests] paths` is not consulted: a rule applies to a
    /// file whether or not the configuration declares the file a test path.
    pub fn mechanisms(&self, norm: &str, before: Option<&Self>) -> Vec<Mechanism> {
        let mut out = Vec::new();
        if let Some((attribute, file)) = self.attributes.set_on(norm) {
            let (key, what) = if attribute == "linguist-vendored" {
                ("linguist-vendored", "`linguist-vendored` in")
            } else {
                ("linguist-generated", "`linguist-generated` in")
            };
            out.push(Mechanism::new(&file, key, what));
        }
        self.pytest.mechanisms(norm, &mut out);
        if !self.js.node_test_files.contains(norm) {
            self.js.mechanisms(norm, &mut out);
        }
        self.rust
            .mechanisms(norm, before.map(|b| &b.rust), &mut out);
        self.go.mechanisms(norm, before.map(|b| &b.go), &mut out);
        out
    }

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

        for path in tracked {
            let (dir, name) = match path.rfind('/') {
                Some(i) => (&path[..i], &path[i + 1..]),
                None => ("", path.as_str()),
            };
            match name {
                ".gitattributes" => {
                    if let Some(src) = reader(path) {
                        rules.attributes.add_file(path, &src);
                    }
                }
                "go.mod" => rules.go.modules.push(dir.to_string()),
                "go.work" => {
                    let work = reader(path).map_or(GoWork::Unreadable, |src| parse_go_work(&src));
                    rules.go.works.insert(dir.to_string(), work);
                }
                "conftest.py" => {
                    if let Some(src) = reader(path) {
                        rules
                            .pytest
                            .conftests
                            .push((dir.to_string(), parse_conftest(&src)));
                    }
                }
                _ if js_runner_extension(&name.to_ascii_lowercase())
                    && !format!("/{path}").contains("/node_modules/") =>
                {
                    if reader(path).is_some_and(|src| imports_node_test(name, &src)) {
                        rules.js.node_test_files.insert(path.clone());
                    }
                }
                _ if name.ends_with("_test.go") && !go_tool_ignores(path) => {
                    let Some(src) = reader(path) else {
                        continue;
                    };
                    let build = go_build_constraint(&src);
                    if build != GoBuild::Built {
                        if let Some(line) = go_build_constraint_line(&src) {
                            rules.go.constraint_lines.insert(path.clone(), line);
                        }
                        rules.go.constraints.insert(path.clone(), build);
                    }
                }
                _ => {}
            }
        }

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
                "package.json" => {
                    let src = reader(path);
                    let runs_node_test = src
                        .as_deref()
                        .and_then(|src| serde_json::from_str::<serde_json::Value>(src).ok())
                        .is_some_and(|package| scripts_run_node_test(&package));
                    if runs_node_test {
                        rules.js.nested_node_test.push(dir.to_string());
                    }
                    src.is_some_and(|src| JsCollectionRules::package_configures_runner(&src))
                }
                "deno.json" | "deno.jsonc" => {
                    if !rules.js.nested_deno.iter().any(|d| d == dir) {
                        rules.js.nested_deno.push(dir.to_string());
                    }
                    false
                }
                "bunfig.toml" => false,
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
        for file in &[
            "pytest.ini",
            ".pytest.ini",
            "pyproject.toml",
            "tox.ini",
            "setup.cfg",
        ] {
            let Some(src) = reader(file) else {
                continue;
            };
            let mut parsed = match *file {
                "pyproject.toml" => PytestCollectionRules::parse_pyproject_toml(&src),
                "setup.cfg" => PytestCollectionRules::parse_setup_cfg(&src),
                _ => PytestCollectionRules::parse_ini(&src),
            };
            parsed.configured |= matches!(*file, "pytest.ini" | ".pytest.ini");
            if parsed.configured || parsed.unparseable.is_some() {
                parsed.source = Some((*file).to_string());
                rules.pytest = parsed;
                break;
            }
        }

        // 2. JS / Jest / Vitest / Mocha
        let package_src = reader("package.json");
        let package: Option<serde_json::Value> = package_src
            .as_deref()
            .and_then(|src| serde_json::from_str(src).ok());
        if let Some(src) = &package_src {
            rules.js.merge_package_json(src);
        }
        // Each place Jest looks for a configuration by itself.
        let mut jest_sources =
            usize::from(package.as_ref().is_some_and(|p| p.get("jest").is_some()));
        if let Some(src) = reader("jest.config.json") {
            jest_sources += 1;
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
                jest_sources += 1;
                rules.js.unparseable_config = Some((*f).to_string());
                break;
            }
        }
        if jest_sources > 1 {
            rules.js.conflict = Some(JEST_SEVERAL_CONFIGS);
        }

        // The words of the scripts that run Jest: `--config` replaces all of the above.
        if let Some(package) = &package {
            rules.js.apply_jest_scripts(package, &mut reader);
        }

        // Vitest: a `vitest.config.*` wins over a `vite.config.*`. Only a literal
        // configuration is read; `vitest.config.json` is not a file Vitest reads as data.
        let vitest_dependency = package.as_ref().is_some_and(|p| {
            ["dependencies", "devDependencies", "peerDependencies"]
                .iter()
                .any(|key| p.get(*key).and_then(|d| d.get("vitest")).is_some())
        });
        let mut vitest_file = None;
        'vitest: for stem in ["vitest.config", "vite.config"] {
            for ext in ["ts", "js", "mjs", "cjs", "mts", "cts"] {
                let name = format!("{stem}.{ext}");
                if let Some(src) = reader(&name) {
                    vitest_file = Some((name, src));
                    break 'vitest;
                }
            }
        }
        let dependency = |name: &str| {
            package.as_ref().is_some_and(|p| {
                ["dependencies", "devDependencies"]
                    .iter()
                    .any(|key| p.get(*key).and_then(|d| d.get(name)).is_some())
            })
        };
        match vitest_file {
            Some((name, src)) => rules.js.apply_vitest_config(&name, &src, vitest_dependency),
            None => {
                if reader("vitest.config.json").is_some() {
                    rules.js.unparseable_config = Some("vitest.config.json".to_string());
                }
            }
        }
        // A runner that is a dependency and has no configuration anywhere runs with its
        // defaults: Jest with a `package.json` that has no `jest` key (run with Jest
        // 27.5 and 29.7), Vitest with no configuration file or with a `vite.config.*`
        // that has no `test` block (run with Vitest 1.6, 3.1 and 4.1). With both as
        // dependencies and neither configured, which one the files belong to is not
        // known, and nothing is decided.
        let unconfigured = !rules.js.config_parsed
            && rules.js.unparseable_config.is_none()
            && rules.js.script_problem.is_none()
            && rules.js.conflict.is_none();
        if unconfigured {
            match (dependency("jest"), dependency("vitest")) {
                (true, false) => rules.js.config_parsed = true,
                (false, true) => {
                    rules.js.config_parsed = true;
                    rules.js.vitest = true;
                }
                _ => {}
            }
            rules.js.compile_patterns();
        }

        // Deno: `deno.json` wins over `deno.jsonc`. In a repository with a root
        // `package.json` the runner may be another one, and the lists are not applied.
        if package_src.is_none() {
            for name in ["deno.json", "deno.jsonc"] {
                if let Some(src) = reader(name) {
                    match parse_deno_config(&src) {
                        Ok(deno) => rules.js.deno = Some((name.to_string(), deno)),
                        Err(reason) => rules.js.deno_problem = Some((name.to_string(), reason)),
                    }
                    break;
                }
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
    /// The file is left out of the count, and a note says so with this reason: nothing
    /// tracked in the repository could run a test in its language, or a tracked
    /// `.gitattributes` marks it as vendored or generated.
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

/// The files a runner loads around the tests because of where they are or what they are
/// called, read from the path alone: the name is how the runner finds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedHarness {
    /// A `conftest.py`: pytest imports each one in the directories it collects.
    Conftest,
    /// `sitecustomize.py` / `usercustomize.py`: the interpreter imports the one on its
    /// path when it starts.
    PythonStartup,
    /// A `_test.go` file the go tool does not ignore: where a package's `TestMain` is.
    GoTest,
    /// A Jest or Vitest configuration written as code, which the runner executes.
    JsRunnerConfig,
}

/// What kind of harness file `path` is by its name, if any.
pub fn named_harness(path: &str) -> Option<NamedHarness> {
    let norm = path.replace('\\', "/");
    let name = norm.rsplit('/').next().unwrap_or(&norm);
    match name {
        "conftest.py" => Some(NamedHarness::Conftest),
        "sitecustomize.py" | "usercustomize.py" => Some(NamedHarness::PythonStartup),
        _ if name.ends_with("_test.go") && !go_tool_ignores(&norm) => Some(NamedHarness::GoTest),
        _ if is_script_config(name, &["jest.config", "vitest.config", "vite.config"])
            && !format!("/{norm}").contains("/node_modules/") =>
        {
            Some(NamedHarness::JsRunnerConfig)
        }
        _ => None,
    }
}

/// A runner configuration whose list of harness files could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadHarnessConfig {
    /// The configuration file.
    pub config: String,
    /// Its directory (empty for the repository root): the files below it may be loaded.
    pub dir: String,
    /// The extensions of the files it could name.
    pub extensions: &'static [&'static str],
}

/// The files a runner loads around the tests because a tracked configuration names
/// them: the set-up and global set-up / tear-down files of a Jest or Vitest
/// configuration, and the root file of each Cargo test target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredHarness {
    /// Each JavaScript or TypeScript set-up file, with the configuration that names it.
    pub js_setup: BTreeMap<String, String>,
    /// The root file of each Cargo test target `cargo test` runs, with whether the
    /// target sets `harness = false` (its `main` is then the whole test run).
    pub rust_targets: BTreeMap<String, bool>,
    /// The Rust modules reached from a Cargo test target through `mod` declarations,
    /// with the root target that reaches them.
    pub rust_modules: BTreeMap<String, String>,
    /// Each Python file a `conftest.py` names in `pytest_plugins`, with the `conftest.py`
    /// that names it.
    pub pytest_plugins: BTreeMap<String, String>,
    /// The configurations whose list could not be read.
    pub unread: Vec<UnreadHarnessConfig>,
}

const JS_HARNESS_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "cjs", "mts", "cts", "jsx", "tsx"];

/// The tracked file a set-up entry names. `<rootDir>/x`, `./x` and `x` resolve against
/// `root`; an entry may leave out its extension or name a directory with an `index`
/// file. `None` for a name that is no tracked file: a package, which is outside the
/// repository.
fn resolve_setup_entry(root: &str, entry: &str, tracked: &HashSet<&str>) -> Option<String> {
    let rest = entry.strip_prefix("<rootDir>").unwrap_or(entry);
    let path = clean_relative(&join_dir(root, rest.trim_start_matches('/')))?;
    if tracked.contains(path.as_str()) {
        return Some(path);
    }
    JS_HARNESS_EXTENSIONS
        .iter()
        .flat_map(|ext| [format!("{path}.{ext}"), format!("{path}/index.{ext}")])
        .find(|candidate| tracked.contains(candidate.as_str()))
}

impl ConfiguredHarness {
    /// Reads every Jest and Vitest configuration and every `Cargo.toml` among `tracked`
    /// through `reader`.
    pub fn from_tree<F>(mut reader: F, tracked: &[String]) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let known: HashSet<&str> = tracked.iter().map(String::as_str).collect();
        let mut harness = Self::default();
        let mut packages: HashMap<String, CargoPackage> = HashMap::new();
        for path in tracked {
            let (dir, name) = match path.rfind('/') {
                Some(i) => (&path[..i], &path[i + 1..]),
                None => ("", path.as_str()),
            };
            if name == "Cargo.toml" {
                match reader(path).map(|src| {
                    let parses = toml::from_str::<toml::Value>(&src).is_ok();
                    (parses, RustCollectionRules::parse_package(&src))
                }) {
                    Some((true, Some(package))) => {
                        packages.insert(dir.to_string(), package);
                    }
                    Some((true, None)) => {}
                    Some((false, _)) | None => harness.unread.push(UnreadHarnessConfig {
                        config: path.clone(),
                        dir: dir.to_string(),
                        extensions: &["rs"],
                    }),
                }
                continue;
            }
            if name == "conftest.py" {
                if let Some(src) = reader(path) {
                    for plugin in parse_conftest_plugins(&src) {
                        let rel = plugin.replace('.', "/");
                        let candidates = [
                            join_dir(dir, &format!("{rel}.py")),
                            join_dir(dir, &format!("{rel}/__init__.py")),
                            format!("{rel}.py"),
                            format!("{rel}/__init__.py"),
                        ];
                        for cand in candidates {
                            if let Some(clean) = clean_relative(&cand) {
                                if known.contains(clean.as_str()) {
                                    harness.pytest_plugins.insert(clean, path.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                continue;
            }
            if format!("/{path}").contains("/node_modules/") {
                continue;
            }
            let json = |src: String, key: Option<&str>| {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&src) else {
                    return SetupFiles::Dynamic;
                };
                match key {
                    None => jest_setup_files(&value),
                    Some(key) => match value.get(key) {
                        Some(config) if config.is_object() => jest_setup_files(config),
                        _ => SetupFiles::Literal {
                            root: None,
                            entries: Vec::new(),
                        },
                    },
                }
            };
            let setup = match name {
                "package.json" => reader(path).map(|src| json(src, Some("jest"))),
                "jest.config.json" => reader(path).map(|src| json(src, None)),
                _ if named_harness(path) == Some(NamedHarness::JsRunnerConfig) => {
                    reader(path).map(|src| script_setup_files(name, &src))
                }
                _ => continue,
            };
            match setup {
                Some(SetupFiles::Literal { root, entries }) => {
                    let root = join_dir(dir, root.as_deref().unwrap_or(""));
                    for entry in entries {
                        if let Some(file) = resolve_setup_entry(&root, &entry, &known) {
                            harness.js_setup.entry(file).or_insert_with(|| path.clone());
                        }
                    }
                }
                Some(SetupFiles::Dynamic) | None => harness.unread.push(UnreadHarnessConfig {
                    config: path.clone(),
                    dir: dir.to_string(),
                    extensions: JS_HARNESS_EXTENSIONS,
                }),
            }
        }
        // The targets a manifest declares, then the ones Cargo finds by itself.
        for (dir, package) in &packages {
            for target in package.tests.iter().filter(|t| t.tested) {
                let candidates = match (&target.path, &target.name) {
                    (Some(path), _) => vec![path.clone()],
                    (None, Some(name)) => {
                        vec![format!("tests/{name}.rs"), format!("tests/{name}/main.rs")]
                    }
                    (None, None) => Vec::new(),
                };
                if let Some(root) = candidates
                    .iter()
                    .map(|rel| join_dir(dir, rel))
                    .find(|path| known.contains(path.as_str()))
                {
                    // A file two packages name as a target keeps `harness = false`
                    // from either, whatever order the packages are read in.
                    *harness.rust_targets.entry(root).or_insert(false) |= !target.harness;
                }
            }
        }
        for path in tracked.iter().filter(|p| p.ends_with(".rs")) {
            // Each `tests/` component may be the one a package's target roots are under.
            let auto = path.match_indices("tests/").any(|(at, _)| {
                let component = at == 0 || path[..at].ends_with('/');
                let dir = path[..at].trim_end_matches('/');
                component
                    && is_auto_test_root(&path[at..])
                    && packages.get(dir).is_some_and(|p| p.autotests)
            });
            if auto && !harness.rust_targets.contains_key(path) {
                harness.rust_targets.insert(path.clone(), false);
            }
        }
        let rust_files: HashSet<&str> = tracked
            .iter()
            .filter(|p| p.ends_with(".rs"))
            .map(String::as_str)
            .collect();
        let target_roots: Vec<String> = harness.rust_targets.keys().cloned().collect();
        for target in target_roots {
            let modules = follow_rust_modules(vec![target.clone()], &rust_files, &mut reader, true);
            for reached in modules.reached {
                if reached != target {
                    harness.rust_modules.insert(reached, target.clone());
                }
            }
        }
        harness
    }
}

/// Why a file that imports `node:test` is not counted.
const NODE_TEST_NOT_RUN: &str =
    "a file that imports `node:test` is run by `node --test`, which no script of the root `package.json` or of a `package.json` above the file runs (it counts when `[tests] paths` names it)";

/// The rules the change adds that take `path` out of the default run: those that apply
/// to it on the head side and did not on the base side, when the base side's default
/// run collected the file and the head side's does not. Empty for a file the base side
/// did not collect, one the head side still collects (a `[tests] paths` entry
/// included), and one that left collection for a reason that is not one of
/// [`RunnerCollectionRules::mechanisms`].
pub fn moved_out_by(
    path: &str,
    base: &crate::ast::AssertVocabulary,
    head: &crate::ast::AssertVocabulary,
) -> Vec<Mechanism> {
    if check_runner_collected(path, base) != RunnerCollectionStatus::Collected
        || check_runner_collected(path, head) == RunnerCollectionStatus::Collected
    {
        return Vec::new();
    }
    let norm = path.replace('\\', "/");
    let before = base.runner_rules.mechanisms(&norm, None);
    head.runner_rules
        .mechanisms(&norm, Some(&base.runner_rules))
        .into_iter()
        .filter(|m| !before.iter().any(|b| b.file == m.file && b.key == m.key))
        .collect()
}

/// Evaluates whether a file path is collected by its language runner given repository vocabulary.
///
/// `NotCollected` only when a parsed runner configuration, or a rule the language fixes
/// (`_test.go`, a build constraint no build satisfies, Cargo's target layout), excludes
/// the file. `NoRunner` only when the listed tree holds no manifest a runner of the
/// language needs, or a tracked `.gitattributes` sets `linguist-vendored` or
/// `linguist-generated` on the file. Everything else that is not known to be collected
/// is `Unknown`, with a reason that quotes no configured value (a build tag, a package
/// directory and a module directory are named: they are identifiers and tracked paths).
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

    // 2. A vendored or generated file is not the project's own test, in any language
    if let Some((attribute, file)) = vocab.runner_rules.attributes.set_on(&norm) {
        return RunnerCollectionStatus::NoRunner(format!("`{attribute}` is set in `{file}`"));
    }

    if lower.ends_with(".py") {
        let pytest = &vocab.runner_rules.pytest;
        if let Some(unparseable) = &pytest.unparseable {
            return RunnerCollectionStatus::Unknown(format!(
                "the pytest configuration `{unparseable}` cannot be read"
            ));
        }
        match (pytest.collection(&norm), pytest.configured) {
            // An entry that is not a glob decides nothing, in either direction.
            (None, _) => RunnerCollectionStatus::Unknown(PYTHON_FILES_UNREAD.to_string()),
            (Some(true), true) => match pytest.directories(&norm) {
                PytestDirectories::Clear => RunnerCollectionStatus::Collected,
                PytestDirectories::Excluded => RunnerCollectionStatus::NotCollected,
                PytestDirectories::Unknown(reason) => {
                    RunnerCollectionStatus::Unknown(reason.to_string())
                }
            },
            // The directory rules are pytest's: without its configuration they are not
            // known to apply.
            (Some(true), false) => RunnerCollectionStatus::Collected,
            (Some(false), true) => RunnerCollectionStatus::NotCollected,
            // The runner may be unittest or Django, which collect by other rules.
            (Some(false), false) => {
                RunnerCollectionStatus::Unknown("no pytest configuration found".to_string())
            }
        }
    } else if js_runner_extension(&lower) {
        let js = &vocab.runner_rules.js;
        // A file that imports `node:test` is run by `node --test`, not by Jest or Vitest.
        if js.node_test_files.contains(&norm) {
            // A script of the root manifest, or of a nested one for the files below it.
            let run_by_script = js.node_test_script
                || js
                    .nested_node_test
                    .iter()
                    .any(|dir| norm.starts_with(&format!("{dir}/")));
            return if run_by_script {
                RunnerCollectionStatus::Collected
            } else {
                RunnerCollectionStatus::NoRunner(NODE_TEST_NOT_RUN.to_string())
            };
        }
        match js.is_collected(&norm) {
            JsCollectionResult::Collected => RunnerCollectionStatus::Collected,
            JsCollectionResult::NotCollected => RunnerCollectionStatus::NotCollected,
            JsCollectionResult::Unknown(_) if js.no_runner_sign() => {
                RunnerCollectionStatus::NoRunner(
                    "no JavaScript package manifest or runner configuration in the repository (a suite that runs without one, such as `node --test`, counts when `[tests] paths` names it)"
                        .to_string(),
                )
            }
            JsCollectionResult::Unknown(reason)
                if JS_SPECIFIC_REASONS.contains(&reason.as_str()) =>
            {
                RunnerCollectionStatus::Unknown(reason)
            }
            JsCollectionResult::Unknown(reason) => {
                RunnerCollectionStatus::Unknown(js.unknown_kind().unwrap_or(reason))
            }
        }
    } else if lower.ends_with(".rs") {
        match vocab.runner_rules.rust.status(&norm) {
            RustCollection::Collected => RunnerCollectionStatus::Collected,
            RustCollection::NotCollected => RunnerCollectionStatus::NotCollected,
            RustCollection::Unknown(reason) => RunnerCollectionStatus::Unknown(reason),
            RustCollection::Unreached(reason) => {
                RunnerCollectionStatus::NoRunner(reason.to_string())
            }
        }
    } else if lower.ends_with(".go") {
        if lower.ends_with("_test.go") && !go_tool_ignores(&norm) {
            vocab.runner_rules.go.status(&norm)
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

    fn configured_harness(files: &[(&str, &str)]) -> ConfiguredHarness {
        let tracked: Vec<String> = files.iter().map(|(path, _)| path.to_string()).collect();
        ConfiguredHarness::from_tree(
            |path| {
                files
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, src)| src.to_string())
            },
            &tracked,
        )
    }

    #[test]
    fn harness_files_named_by_a_js_configuration_resolve_to_tracked_files() {
        let harness = configured_harness(&[
            (
                "package.json",
                r#"{"jest": {"setupFiles": ["<rootDir>/test/setup.js", "./test/env", "jest-extended/all", "missing.js"], "globalTeardown": "test/down"}}"#,
            ),
            ("test/setup.js", ""),
            ("test/env.ts", ""),
            ("test/down/index.mjs", ""),
            ("test/unnamed.js", ""),
            (
                "web/jest.config.json",
                r#"{"rootDir": "..", "globalSetup": "<rootDir>/shared/up.js"}"#,
            ),
            ("shared/up.js", ""),
            (
                "app/vitest.config.ts",
                "export default { root: 'src', test: { setupFiles: ['./setup.ts'] } };\n",
            ),
            ("app/src/setup.ts", ""),
            ("app/setup.ts", ""),
            // Not read: a dependency's own configuration.
            (
                "node_modules/dep/jest.config.json",
                r#"{"globalSetup": "./steal.js"}"#,
            ),
            ("node_modules/dep/steal.js", ""),
            // A package manifest with no `jest` key names nothing.
            ("lib/package.json", r#"{"name": "lib"}"#),
        ]);
        let named: Vec<(&str, &str)> = harness
            .js_setup
            .iter()
            .map(|(file, config)| (file.as_str(), config.as_str()))
            .collect();
        assert_eq!(
            named,
            vec![
                ("app/src/setup.ts", "app/vitest.config.ts"),
                ("shared/up.js", "web/jest.config.json"),
                ("test/down/index.mjs", "package.json"),
                ("test/env.ts", "package.json"),
                ("test/setup.js", "package.json"),
            ]
        );
        assert!(harness.unread.is_empty(), "{:?}", harness.unread);
        assert!(harness.rust_targets.is_empty());
    }

    #[test]
    fn a_js_configuration_that_cannot_be_read_is_recorded_as_unread() {
        let harness = configured_harness(&[
            ("web/jest.config.js", "module.exports = make();\n"),
            ("web/test/setup.js", ""),
            ("api/package.json", "{ not json"),
            ("ok/jest.config.json", "{}"),
        ]);
        assert!(harness.js_setup.is_empty());
        let unread: Vec<(&str, &str)> = harness
            .unread
            .iter()
            .map(|u| (u.config.as_str(), u.dir.as_str()))
            .collect();
        assert_eq!(
            unread,
            vec![("web/jest.config.js", "web"), ("api/package.json", "api")]
        );
        assert!(harness.unread[0].extensions.contains(&"ts"));
    }

    #[test]
    fn the_root_of_each_cargo_test_target_cargo_runs_is_a_harness_file() {
        let package = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n";
        let harness = configured_harness(&[
            (
                "Cargo.toml",
                &format!("{package}\n[[test]]\nname = \"own\"\npath = \"checks/own.rs\"\nharness = false\n\n[[test]]\nname = \"named\"\n\n[[test]]\nname = \"off\"\npath = \"checks/off.rs\"\ntest = false\n\n[[test]]\nname = \"gone\"\npath = \"checks/gone.rs\"\n"),
            ),
            ("checks/own.rs", ""),
            ("checks/off.rs", ""),
            ("tests/named/main.rs", ""),
            ("tests/it.rs", ""),
            ("tests/common/mod.rs", ""),
            ("tests/data/sample.txt", ""),
            ("src/main.rs", ""),
            ("benches/b.rs", ""),
            // A nested package with automatic targets off, and one with none declared.
            (
                "crates/a/Cargo.toml",
                "[package]\nname = \"a\"\nversion = \"0.1.0\"\nautotests = false\n",
            ),
            ("crates/a/tests/skipped.rs", ""),
            ("crates/b/Cargo.toml", package),
            ("crates/b/tests/tests/main.rs", ""),
            ("crates/b/tests/deep/inner/main.rs", ""),
            // A directory with no manifest of its own is no package.
            ("tools/tests/loose.rs", ""),
            // A workspace manifest declares no target.
            ("ws/Cargo.toml", "[workspace]\nmembers = []\n"),
            ("ws/tests/none.rs", ""),
        ]);
        let targets: Vec<(&str, bool)> = harness
            .rust_targets
            .iter()
            .map(|(file, own_main)| (file.as_str(), *own_main))
            .collect();
        assert_eq!(
            targets,
            vec![
                ("checks/own.rs", true),
                ("crates/b/tests/tests/main.rs", false),
                ("tests/it.rs", false),
                ("tests/named/main.rs", false),
            ]
        );
        assert!(harness.unread.is_empty());

        let broken =
            configured_harness(&[("svc/Cargo.toml", "[package\n"), ("svc/tests/it.rs", "")]);
        assert!(broken.rust_targets.is_empty());
        assert_eq!(broken.unread.len(), 1);
        assert_eq!(
            (broken.unread[0].dir.as_str(), broken.unread[0].extensions),
            ("svc", &["rs"][..])
        );
    }

    #[test]
    fn conftest_plugins_and_rust_test_modules_are_harness_files() {
        let package = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n";
        let harness = configured_harness(&[
            ("Cargo.toml", package),
            ("tests/it.rs", "mod helper;\n"),
            ("tests/helper.rs", "pub fn help() {}\n"),
            ("conftest.py", "pytest_plugins = ['tamper_mod']\n"),
            ("tamper_mod.py", "def pytest_runtest_makereport(): pass\n"),
        ]);
        assert_eq!(
            harness
                .pytest_plugins
                .get("tamper_mod.py")
                .map(String::as_str),
            Some("conftest.py")
        );
        assert_eq!(
            harness
                .rust_modules
                .get("tests/helper.rs")
                .map(String::as_str),
            Some("tests/it.rs")
        );
    }

    #[test]
    fn a_harness_file_by_name_is_what_its_runner_looks_for() {
        for (path, kind) in [
            ("conftest.py", Some(NamedHarness::Conftest)),
            ("a/b/conftest.py", Some(NamedHarness::Conftest)),
            ("sitecustomize.py", Some(NamedHarness::PythonStartup)),
            ("env/usercustomize.py", Some(NamedHarness::PythonStartup)),
            ("pkg/x_test.go", Some(NamedHarness::GoTest)),
            ("vendor/x_test.go", Some(NamedHarness::GoTest)),
            ("web/jest.config.cjs", Some(NamedHarness::JsRunnerConfig)),
            ("vitest.config.mts", Some(NamedHarness::JsRunnerConfig)),
            ("vite.config.ts", Some(NamedHarness::JsRunnerConfig)),
            ("my_conftest.py", None),
            ("conftest.pyi", None),
            ("conftest/x.py", None),
            ("pkg/testdata/x_test.go", None),
            ("pkg/_x_test.go", None),
            ("vendor/dep/x_test.go", None),
            ("pkg/x_test.go.txt", None),
            ("jest.config.json", None),
            ("jest.config.js.bak", None),
            ("node_modules/a/vitest.config.ts", None),
            ("jest.setup.js", None),
            ("test/setup.js", None),
        ] {
            assert_eq!(named_harness(path), kind, "{path}");
        }
    }

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
        assert!(rules.unparseable.is_none());

        // Unparseable pyproject.toml
        let broken = PytestCollectionRules::parse_pyproject_toml("invalid toml [");
        assert_eq!(broken.unparseable.as_deref(), Some("pyproject.toml"));
        assert!(!broken.configured);

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
                Some("export default mergeConfig(base, { test: { ...shared } })".to_string())
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
        // Jest's default `testMatch` holds the module extensions from Jest 30
        // (`**/?(*.)+(spec|test).?([mc])[jt]s?(x)`) and not before
        // (`**/?(*.)+(spec|test).[jt]s?(x)`).
        let jest = vocab_with(&[(
            "package.json",
            r#"{"jest": {}, "devDependencies": {"jest": "^30.0.0"}}"#,
        )]);
        let earlier = vocab_with(&[(
            "package.json",
            r#"{"jest": {}, "devDependencies": {"jest": "^29.7.0"}}"#,
        )]);
        let unpinned = vocab_with(&[("package.json", r#"{"jest": {}}"#)]);
        for ext in ["mts", "cts"] {
            assert_eq!(
                check_runner_collected(&format!("src/a.test.{ext}"), &jest),
                RunnerCollectionStatus::Collected,
                "{ext}"
            );
            assert_eq!(
                check_runner_collected(&format!("src/a.test.{ext}"), &earlier),
                RunnerCollectionStatus::NotCollected,
                "{ext}"
            );
            assert_eq!(
                check_runner_collected(&format!("src/a.test.{ext}"), &unpinned),
                RunnerCollectionStatus::Unknown(JEST_VERSION_UNKNOWN.to_string()),
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

        // An unparseable pyproject.toml leaves collection unknown even for default test paths.
        let broken = vocab_with(&[("pyproject.toml", "[tool.pytest.ini_options\ninvalid toml")]);
        assert_eq!(
            check_runner_collected("tests/test_o.py", &broken),
            RunnerCollectionStatus::Unknown(
                "the pytest configuration `pyproject.toml` cannot be read".to_string()
            )
        );
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
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
            // Jest matches the absolute path: these patterns are anchored nowhere.
            r#"{"testMatch": ["*/**/a.test.js"]}"#,
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
        // A configured list replaces Jest's default, and a file below `node_modules`
        // stays out all the same: Jest's file map does not hold it.
        assert_eq!(
            ignore.is_collected("node_modules/dep/a.test.js"),
            JsCollectionResult::NotCollected
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
        // An unset ignore list may be the preset's, and Jest's default is not assumed
        // for it. A file below `node_modules` is out whatever the list is, with a
        // preset as without one.
        let unset = jest_rules(r#"{"preset": "ts-jest", "testMatch": ["**/*.test.js"]}"#);
        assert_eq!(
            unset.is_collected("node_modules/dep/a.test.js"),
            JsCollectionResult::NotCollected
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
            ("tests/part/mod.rs", ""),
            ("tests/off/part.rs", ""),
            ("tests/on.rs", ""),
        ];
        assert_eq!(
            rust_status(&files, "tests/off.rs"),
            RunnerCollectionStatus::NotCollected
        );
        // A module only a switched-off target declares does not run either.
        assert_eq!(
            rust_status(&files, "tests/part/mod.rs"),
            RunnerCollectionStatus::NotCollected
        );
        // A file no target reaches is left out with a note (a crate root's modules are
        // beside it, not under a directory of its name).
        assert_eq!(
            rust_status(&files, "tests/off/part.rs"),
            RunnerCollectionStatus::NoRunner(RUST_TEST_MODULE_UNREACHED.to_string())
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
        // A binary's unit tests run, with the modules it declares; what only the
        // library reaches stays out.
        for (bin, part) in [
            ("src/main.rs", "src/part.rs"),
            ("src/bin/tool.rs", "src/bin/part.rs"),
        ] {
            let files = [
                ("Cargo.toml", manifest.as_str()),
                ("src/lib.rs", ""),
                (bin, "mod part;\n"),
                (part, ""),
            ];
            assert_eq!(
                rust_status(&files, "src/lib.rs"),
                RunnerCollectionStatus::NotCollected,
                "{bin}"
            );
            for file in [bin, part] {
                assert_eq!(
                    rust_status(&files, file),
                    RunnerCollectionStatus::Collected,
                    "{file}"
                );
            }
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
            // Left out, with the note for a file no test target reaches.
            assert_eq!(
                rust_status(&files, path),
                RunnerCollectionStatus::NoRunner(RUST_TEST_MODULE_UNREACHED.to_string()),
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
            RunnerCollectionStatus::NoRunner(RUST_TEST_MODULE_UNREACHED.to_string())
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

    // ---------------------------------------------------------------- #593

    const GO_TAGGED: &str = "//go:build integration\n\npackage a\n";
    const GO_IGNORED: &str = "//go:build ignore\n\npackage a\n";
    const GO_PLATFORM: &str = "//go:build linux || darwin\n\npackage a\n";
    const GO_TWO_TAGS: &str = "//go:build e2e || slow\n\npackage a\n";
    const GO_BROKEN: &str = "//go:build linux &&\n\npackage a\n";
    const GO_MOD_FILE: &str = "module example.test/m\n";

    fn status(files: &[(&str, &str)], path: &str) -> RunnerCollectionStatus {
        check_runner_collected(path, &tree(files))
    }

    fn unknown_reason(files: &[(&str, &str)], path: &str) -> String {
        match status(files, path) {
            RunnerCollectionStatus::Unknown(reason) => reason,
            other => panic!("{path}: {other:?}"),
        }
    }

    #[test]
    fn test_go_build_constraints_decide_collection() {
        let files = [
            ("a/tagged_test.go", GO_TAGGED),
            ("a/ignored_test.go", GO_IGNORED),
            ("a/platform_test.go", GO_PLATFORM),
            ("a/two_test.go", GO_TWO_TAGS),
            ("a/broken_test.go", GO_BROKEN),
            ("a/plain_test.go", "package a\n"),
            ("a/testdata/ignored_test.go", GO_TAGGED),
        ];
        assert_eq!(
            status(&files, "a/ignored_test.go"),
            RunnerCollectionStatus::NotCollected
        );
        assert_eq!(
            status(&files, "a/platform_test.go"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            status(&files, "a/plain_test.go"),
            RunnerCollectionStatus::Collected
        );
        let tagged = unknown_reason(&files, "a/tagged_test.go");
        assert!(
            tagged.contains("needs the tag `integration`") && tagged.contains("-tags"),
            "{tagged}"
        );
        let two = unknown_reason(&files, "a/two_test.go");
        assert!(two.contains("one of the tags `e2e`, `slow`"), "{two}");
        let broken = unknown_reason(&files, "a/broken_test.go");
        assert!(broken.contains("cannot be read"), "{broken}");
        // A directory the go tool ignores decides before any constraint.
        assert_eq!(
            status(&files, "a/testdata/ignored_test.go"),
            RunnerCollectionStatus::NotCollected
        );
        // When the tree was not listed no content was read: the path rule decides.
        assert_eq!(
            check_runner_collected("a/ignored_test.go", &vocab_with(&[])),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_go_nested_modules() {
        let files = [
            ("go.mod", GO_MOD_FILE),
            ("pkg/a_test.go", "package pkg\n"),
            ("sub/go.mod", GO_MOD_FILE),
            ("sub/pkg/b_test.go", "package pkg\n"),
            ("sub/deep/go.mod", GO_MOD_FILE),
            ("sub/deep/c_test.go", "package deep\n"),
            ("subway/d_test.go", "package subway\n"),
        ];
        assert_eq!(
            status(&files, "pkg/a_test.go"),
            RunnerCollectionStatus::Collected
        );
        // A directory whose name only starts like the module's is not inside it.
        assert_eq!(
            status(&files, "subway/d_test.go"),
            RunnerCollectionStatus::Collected
        );
        let nested = unknown_reason(&files, "sub/pkg/b_test.go");
        assert!(
            nested.contains("module in `sub`") && nested.contains("at the repository root"),
            "{nested}"
        );
        let deeper = unknown_reason(&files, "sub/deep/c_test.go");
        assert!(
            deeper.contains("module in `sub/deep`") && deeper.contains("in `sub`"),
            "{deeper}"
        );
        // Modules side by side, with none above them, are each their own root.
        let siblings = [
            ("a/go.mod", GO_MOD_FILE),
            ("a/a_test.go", "package a\n"),
            ("b/go.mod", GO_MOD_FILE),
            ("b/b_test.go", "package b\n"),
        ];
        for path in ["a/a_test.go", "b/b_test.go"] {
            assert_eq!(
                status(&siblings, path),
                RunnerCollectionStatus::Collected,
                "{path}"
            );
        }
    }

    #[test]
    fn test_gitattributes_leave_a_file_out_in_any_language() {
        let files = [
            (
                ".gitattributes",
                "vendor/** linguist-vendored\n*.pb.go linguist-generated\n",
            ),
            ("vendor/keep/.gitattributes", "* -linguist-vendored\n"),
            ("package.json", "{}"),
        ];
        for (path, attribute, file) in [
            (
                "vendor/lib/a.test.js",
                "linguist-vendored",
                ".gitattributes",
            ),
            ("vendor/x/test_a.py", "linguist-vendored", ".gitattributes"),
            (
                "vendor/x/tests/it.rs",
                "linguist-vendored",
                ".gitattributes",
            ),
            ("api/a.pb.go", "linguist-generated", ".gitattributes"),
            (
                "ATests/A.swift.pb.go",
                "linguist-generated",
                ".gitattributes",
            ),
        ] {
            assert_eq!(
                status(&files, path),
                RunnerCollectionStatus::NoRunner(format!("`{attribute}` is set in `{file}`")),
                "{path}"
            );
        }
        // Reset by the nearer file, and never set: the language's own rules decide.
        assert!(matches!(
            status(&files, "vendor/keep/a.test.js"),
            RunnerCollectionStatus::Unknown(_)
        ));
        assert!(matches!(
            status(&files, "src/a.test.js"),
            RunnerCollectionStatus::Unknown(_)
        ));
        // A declared test path counts whatever its attributes.
        let mut declared = tree(&files);
        declared.test_paths = vec!["vendor/**".to_string()];
        assert_eq!(
            check_runner_collected("vendor/lib/a.test.js", &declared),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_cargo_workspace_exclude_is_not_determined() {
        let workspace = "[workspace]\nmembers = [\"member\"]\nexclude = [\"out\", \"tools/gen\"]\n";
        let off = format!("{PKG}\n[[test]]\nname = \"off\"\ntest = false\n");
        let files = [
            ("Cargo.toml", workspace),
            ("member/Cargo.toml", PKG),
            ("member/tests/it.rs", ""),
            ("out/Cargo.toml", off.as_str()),
            ("out/tests/it.rs", ""),
            ("out/tests/off.rs", ""),
            ("tools/gen/sub/Cargo.toml", PKG),
            ("tools/gen/sub/tests/it.rs", ""),
            ("outer/Cargo.toml", PKG),
            ("outer/tests/it.rs", ""),
        ];
        assert_eq!(
            status(&files, "member/tests/it.rs"),
            RunnerCollectionStatus::Collected
        );
        // A name that only starts like an excluded one is not excluded.
        assert_eq!(
            status(&files, "outer/tests/it.rs"),
            RunnerCollectionStatus::Collected
        );
        let reason = unknown_reason(&files, "out/tests/it.rs");
        assert!(
            reason.contains("package in `out`")
                && reason.contains("`exclude`")
                && reason.contains("`Cargo.toml`"),
            "{reason}"
        );
        // A package below an excluded directory is excluded with it.
        let below = unknown_reason(&files, "tools/gen/sub/tests/it.rs");
        assert!(below.contains("package in `tools/gen/sub`"), "{below}");
        // What the package itself does not run stays out.
        assert_eq!(
            status(&files, "out/tests/off.rs"),
            RunnerCollectionStatus::NotCollected
        );
        // A package that is a workspace root itself belongs to no workspace above it.
        let own_root = format!("{PKG}\n[workspace]\n");
        let files = [
            ("Cargo.toml", workspace),
            ("out/Cargo.toml", own_root.as_str()),
            ("out/tests/it.rs", ""),
        ];
        assert_eq!(
            status(&files, "out/tests/it.rs"),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_cargo_source_modules_are_followed_from_the_crate_roots() {
        let files = [
            ("Cargo.toml", PKG),
            (
                "src/lib.rs",
                "pub mod used;\n#[path = \"moved/there.rs\"]\nmod there;\nmod outer {\n    mod inner;\n}\n#[cfg(test)]\nmod tests;\npub mod r#match;\n",
            ),
            ("src/used.rs", "mod deeper;\n"),
            ("src/used/deeper.rs", ""),
            ("src/moved/there.rs", ""),
            ("src/outer/inner.rs", ""),
            ("src/tests.rs", ""),
            ("src/match.rs", ""),
            ("src/orphan.rs", ""),
            ("src/used/orphan.rs", ""),
            ("src/main.rs", "mod cli;\nfn main() {}\n"),
            ("src/cli/mod.rs", "mod args;\n"),
            ("src/cli/args.rs", ""),
            ("src/bin/tool.rs", "mod part;\nfn main() {}\n"),
            ("src/bin/part.rs", ""),
            ("src/bin/multi/main.rs", "mod sub;\nfn main() {}\n"),
            ("src/bin/multi/sub.rs", ""),
            ("src/bin/multi/orphan.rs", ""),
        ];
        for path in [
            "src/lib.rs",
            "src/used.rs",
            "src/used/deeper.rs",
            "src/moved/there.rs",
            "src/outer/inner.rs",
            "src/tests.rs",
            "src/match.rs",
            "src/main.rs",
            "src/cli/mod.rs",
            "src/cli/args.rs",
            "src/bin/tool.rs",
            "src/bin/part.rs",
            "src/bin/multi/main.rs",
            "src/bin/multi/sub.rs",
        ] {
            assert_eq!(
                status(&files, path),
                RunnerCollectionStatus::Collected,
                "{path}"
            );
        }
        for path in [
            "src/orphan.rs",
            "src/used/orphan.rs",
            "src/bin/multi/orphan.rs",
        ] {
            // Left out, with the note for a file no crate root reaches.
            assert_eq!(
                status(&files, path),
                RunnerCollectionStatus::NoRunner(RUST_SOURCE_UNREACHED.to_string()),
                "{path}"
            );
        }
    }

    #[test]
    fn test_cargo_source_modules_that_are_not_followed_leave_the_rest_unknown() {
        for root in [
            "macro_rules! m { ($n:ident) => { mod $n; }; }\nm!(used);\n",
            "include!(\"generated.rs\");\n",
            "#[cfg_attr(unix, path = \"unix.rs\")]\nmod sys;\n",
            "mod outer {\n    #[path = \"x.rs\"]\n    mod inner;\n}\n",
            "mod broken {\n",
        ] {
            let files = [
                ("Cargo.toml", PKG),
                ("src/lib.rs", root),
                ("src/used.rs", ""),
            ];
            assert_eq!(
                status(&files, "src/used.rs"),
                RunnerCollectionStatus::Unknown(RUST_SOURCE_MODULES_OPEN.to_string()),
                "{root}"
            );
        }
        // With no crate root tracked there is nothing to follow: the manifest decides.
        let files = [("Cargo.toml", PKG), ("src/checks.rs", "")];
        assert_eq!(
            status(&files, "src/checks.rs"),
            RunnerCollectionStatus::Collected
        );
        // When the tree was not listed, as before.
        assert_eq!(
            check_runner_collected("src/orphan.rs", &vocab_with(&[("Cargo.toml", PKG)])),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_cargo_lib_and_bin_test_flags_decide_under_src() {
        let lib_off = format!("{PKG}\n[lib]\ntest = false\n");
        let files = [
            ("Cargo.toml", lib_off.as_str()),
            ("src/lib.rs", "pub mod libpart;\npub mod shared;\n"),
            ("src/libpart.rs", ""),
            ("src/shared.rs", ""),
            (
                "src/main.rs",
                "mod binpart;\n#[path = \"shared.rs\"]\nmod shared;\nfn main() {}\n",
            ),
            ("src/binpart.rs", ""),
        ];
        // The library's unit tests do not run; the binary's do, with its modules.
        for (path, expected) in [
            ("src/lib.rs", RunnerCollectionStatus::NotCollected),
            ("src/libpart.rs", RunnerCollectionStatus::NotCollected),
            ("src/main.rs", RunnerCollectionStatus::Collected),
            ("src/binpart.rs", RunnerCollectionStatus::Collected),
            ("src/shared.rs", RunnerCollectionStatus::Collected),
        ] {
            assert_eq!(status(&files, path), expected, "{path}");
        }
        // `[[bin]] test = false`, by `path` and by the name Cargo infers the file from.
        for bin in [
            "[[bin]]\nname = \"p\"\npath = \"src/main.rs\"\ntest = false\n",
            "[[bin]]\nname = \"p\"\ntest = false\n",
        ] {
            let both_off = format!("{lib_off}\n{bin}");
            let mut files = files;
            files[0] = ("Cargo.toml", both_off.as_str());
            for path in [
                "src/main.rs",
                "src/binpart.rs",
                "src/shared.rs",
                "src/lib.rs",
            ] {
                assert_eq!(
                    status(&files, path),
                    RunnerCollectionStatus::NotCollected,
                    "{bin} {path}"
                );
            }
        }
        // `autobins = false`: `src/main.rs` is no binary unless the manifest lists it.
        let no_auto = format!("{PKG}autobins = false\n\n[lib]\ntest = false\n");
        let mut manual = files;
        manual[0] = ("Cargo.toml", no_auto.as_str());
        assert_eq!(
            status(&manual, "src/binpart.rs"),
            RunnerCollectionStatus::NoRunner(RUST_SOURCE_UNREACHED.to_string())
        );
        // A binary whose root file is not found may reach anything.
        let unresolved = format!("{lib_off}\n[[bin]]\nname = \"other\"\n");
        let mut files = files;
        files[0] = ("Cargo.toml", unresolved.as_str());
        assert_eq!(
            status(&files, "src/libpart.rs"),
            RunnerCollectionStatus::Unknown(RUST_SOURCE_MODULES_OPEN.to_string())
        );
    }

    #[test]
    fn test_pytest_function_and_class_names() {
        let unset = PytestCollectionRules::parse_ini("[pytest]\n");
        assert_eq!(unset.function_name_override("check_a"), None);
        assert_eq!(unset.class_name_override("SuiteA"), None);

        let ini = PytestCollectionRules::parse_ini(
            "[pytest]\npython_functions = check_* it\npython_classes =\n    Suite*\n    Describe\n",
        );
        for (name, expected) in [
            ("check_a", true),
            ("it_works", true),
            ("test_a", false),
            ("checks", false),
        ] {
            assert_eq!(ini.function_name_override(name), Some(expected), "{name}");
        }
        for (name, expected) in [("SuiteA", true), ("DescribeThing", true), ("TestA", false)] {
            assert_eq!(ini.class_name_override(name), Some(expected), "{name}");
        }

        let toml = PytestCollectionRules::parse_pyproject_toml(
            "[tool.pytest.ini_options]\npython_functions = [\"check_*\"]\n",
        );
        assert_eq!(toml.function_name_override("check_a"), Some(true));
        assert_eq!(toml.function_name_override("test_a"), Some(false));
        assert_eq!(toml.class_name_override("TestA"), None);

        // Without a pytest configuration the keys mean nothing.
        let loose = PytestCollectionRules {
            functions_configured: true,
            ..Default::default()
        };
        assert_eq!(loose.function_name_override("test_a"), None);
    }

    #[test]
    fn test_pytest_norecursedirs() {
        let default = PytestCollectionRules::parse_ini("[pytest]\n");
        for (path, excluded) in [
            ("build/test_a.py", true),
            ("a/dist/test_a.py", true),
            ("pkg.egg/test_a.py", true),
            (".tox/test_a.py", true),
            ("venv/lib/test_a.py", true),
            ("node_modules/x/test_a.py", true),
            ("{arch}/test_a.py", true),
            ("a/__pycache__/test_a.py", true),
            // A name that only resembles one, and a brace pattern read literally.
            ("builds/test_a.py", false),
            ("arch/test_a.py", false),
            ("tests/test_a.py", false),
            // The file's own name is not a directory.
            ("tests/build", false),
        ] {
            let expected = if excluded {
                PytestDirectories::Excluded
            } else {
                PytestDirectories::Clear
            };
            assert_eq!(default.directories(path), expected, "{path}");
        }

        // A configured list replaces the default one; a pattern with a `/` is matched
        // against the directory's path.
        let custom =
            PytestCollectionRules::parse_ini("[pytest]\nnorecursedirs = parked old_* a/skip\n");
        for (path, excluded) in [
            ("parked/test_a.py", true),
            ("x/old_suite/test_a.py", true),
            ("x/a/skip/test_a.py", true),
            ("x/b/skip/test_a.py", false),
            ("build/test_a.py", false),
        ] {
            let expected = if excluded {
                PytestDirectories::Excluded
            } else {
                PytestDirectories::Clear
            };
            assert_eq!(custom.directories(path), expected, "{path}");
        }

        // Recursion starts at the `testpaths` entry: its own directories are not checked.
        let rooted = PytestCollectionRules::parse_ini("[pytest]\ntestpaths = build/checks\n");
        assert_eq!(
            rooted.directories("build/checks/test_a.py"),
            PytestDirectories::Clear
        );
        assert_eq!(
            rooted.directories("build/checks/dist/test_a.py"),
            PytestDirectories::Excluded
        );
        // Below a glob entry the start is not known.
        let globbed = PytestCollectionRules::parse_ini("[pytest]\ntestpaths = pkgs/*/build\n");
        assert!(matches!(
            globbed.directories("pkgs/x/build/test_a.py"),
            PytestDirectories::Unknown(_)
        ));
        assert_eq!(
            globbed.directories("pkgs/x/checks/test_a.py"),
            PytestDirectories::Clear
        );
    }

    #[test]
    fn test_pytest_conftest_ignores() {
        let files = [
            ("pytest.ini", "[pytest]\n"),
            (
                "checks/conftest.py",
                "collect_ignore = [\"test_parked.py\", \"deep\", \"./sub/test_x.py\"]\ncollect_ignore_glob = [\"*_skip.py\", \"gen/test_*.py\"]\n",
            ),
            ("dyn/conftest.py", "collect_ignore = build_list()\n"),
            ("plain/conftest.py", "import pytest\n"),
        ];
        for (path, expected) in [
            (
                "checks/test_parked.py",
                RunnerCollectionStatus::NotCollected,
            ),
            (
                "checks/deep/test_a.py",
                RunnerCollectionStatus::NotCollected,
            ),
            (
                "checks/deep/er/test_a.py",
                RunnerCollectionStatus::NotCollected,
            ),
            ("checks/sub/test_x.py", RunnerCollectionStatus::NotCollected),
            (
                "checks/more/test_a_skip.py",
                RunnerCollectionStatus::NotCollected,
            ),
            ("checks/gen/test_a.py", RunnerCollectionStatus::NotCollected),
            ("checks/test_kept.py", RunnerCollectionStatus::Collected),
            (
                "checks/sub/test_parked.py",
                RunnerCollectionStatus::Collected,
            ),
            (
                "checks/other/gen/test_a.py",
                RunnerCollectionStatus::Collected,
            ),
            // A list is relative to its own `conftest.py`.
            ("other/test_parked.py", RunnerCollectionStatus::Collected),
            ("plain/test_parked.py", RunnerCollectionStatus::Collected),
        ] {
            assert_eq!(status(&files, path), expected, "{path}");
        }
        let reason = unknown_reason(&files, "dyn/test_a.py");
        assert!(reason.contains("conftest.py"), "{reason}");
        // Without a pytest configuration the lists are not known to apply.
        assert_eq!(
            status(&files[1..], "checks/test_parked.py"),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_pytest_configuration_files() {
        // `setup.cfg` configures pytest only through `[tool:pytest]`.
        let refused = tree(&[("setup.cfg", "[pytest]\ntestpaths = other\n")]);
        assert!(!refused.runner_rules.pytest.configured);
        assert!(is_unknown("tests/a_helper.py", &refused));
        let read = tree(&[("setup.cfg", "[tool:pytest]\ntestpaths = other\n")]);
        assert!(read.runner_rules.pytest.configured);
        // `tox.ini` and `pytest.ini` are read with `[pytest]`.
        for file in ["tox.ini", "pytest.ini", ".pytest.ini"] {
            let rules = tree(&[(file, "[pytest]\ntestpaths = other\n")]);
            assert_eq!(
                rules.runner_rules.pytest.testpaths,
                vec!["other".to_string()],
                "{file}"
            );
        }
        // `pytest.ini` comes before `.pytest.ini`, which comes before `pyproject.toml`.
        let both = tree(&[
            ("pytest.ini", "[pytest]\ntestpaths = first\n"),
            (".pytest.ini", "[pytest]\ntestpaths = second\n"),
        ]);
        assert_eq!(
            both.runner_rules.pytest.testpaths,
            vec!["first".to_string()]
        );
        let hidden = tree(&[
            (".pytest.ini", ""),
            (
                "pyproject.toml",
                "[tool.pytest.ini_options]\ntestpaths = [\"third\"]\n",
            ),
        ]);
        assert!(hidden.runner_rules.pytest.testpaths.is_empty());
        assert!(hidden.runner_rules.pytest.configured);
    }

    const JEST_BY_SCRIPT: &str = r#"{"jest": {"testMatch": ["**/ignored/**"]}, "scripts": {"test": "jest --config config/jest.json"}}"#;
    const JEST_CONFIG_FILE: &str =
        r#"{"rootDir": "..", "testMatch": ["<rootDir>/checks/**/*.js"]}"#;

    #[test]
    fn test_jest_configuration_named_by_a_script() {
        let files = [
            ("package.json", JEST_BY_SCRIPT),
            ("config/jest.json", JEST_CONFIG_FILE),
            ("jest.config.js", "module.exports = {};\n"),
        ];
        // The named file replaces the `jest` key and the default `jest.config.js`.
        assert_eq!(
            status(&files, "checks/a.js"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            status(&files, "ignored/a.js"),
            RunnerCollectionStatus::NotCollected
        );
        // `rootDir` defaults to the directory of the configuration file.
        let files = [
            (
                "package.json",
                r#"{"scripts": {"test": "jest -c config/jest.json"}}"#,
            ),
            ("config/jest.json", "{}"),
        ];
        assert_eq!(
            status(&files, "config/a.test.js"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            status(&files, "src/a.test.js"),
            RunnerCollectionStatus::NotCollected
        );
        // A configuration written on the command line.
        let files = [(
            "package.json",
            r#"{"scripts": {"test": "jest --config '{\"testMatch\": [\"**/checks/**\"]}'"}}"#,
        )];
        assert_eq!(
            status(&files, "checks/a.js"),
            RunnerCollectionStatus::Collected
        );
        // `--rootDir` on the command line wins over the configured one.
        let files = [(
            "package.json",
            r#"{"jest": {"rootDir": "lib"}, "scripts": {"test": "jest --rootDir web"}}"#,
        )];
        assert_eq!(
            status(&files, "web/a.test.js"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            status(&files, "lib/a.test.js"),
            RunnerCollectionStatus::NotCollected
        );
    }

    #[test]
    fn test_jest_scripts_that_are_not_read_leave_collection_unknown() {
        for package in [
            // A script file, a file that is not tracked, the manifest itself.
            r#"{"scripts": {"test": "jest --config jest.unit.config.js"}}"#,
            r#"{"scripts": {"test": "jest --config missing.json"}}"#,
            r#"{"scripts": {"test": "jest --config package.json"}}"#,
            r#"{"scripts": {"test": "jest --config ../outside.json"}}"#,
            // A command list that carries a configuration, and a flag that is not read.
            r#"{"jest": {}, "scripts": {"test": "tsc && jest --config a.json"}}"#,
            r#"{"jest": {}, "scripts": {"test": "jest --testPathPattern unit"}}"#,
            r#"{"jest": {}, "scripts": {"unit": "jest -c a.json", "e2e": "jest -c b.json"}}"#,
        ] {
            let files = [
                ("package.json", package),
                ("a.json", "{}"),
                ("b.json", "{}"),
            ];
            let reason = unknown_reason(&files, "src/a.test.js");
            assert!(reason.contains("package script"), "{package}: {reason}");
            // The reason names the kind of problem, never the configured path.
            assert!(
                !reason.contains(".json") && !reason.contains(".js"),
                "{reason}"
            );
        }
        // Another script with its own configuration: what the `test` script's
        // configuration leaves out may be that one's.
        let files = [(
            "package.json",
            r#"{"jest": {}, "scripts": {"test": "jest", "e2e": "jest -c e2e.json"}}"#,
        )];
        assert_eq!(
            status(&files, "src/a.test.js"),
            RunnerCollectionStatus::Collected
        );
        let other = unknown_reason(&files, "e2e/a.js");
        assert!(other.contains("another configuration"), "{other}");
    }

    #[test]
    fn test_jest_default_patterns_by_major() {
        for (dependency, expected) in [
            (r#""devDependencies": {"jest": "^29.7.0"}"#, Some(false)),
            (r#""dependencies": {"jest": "28.1.3"}"#, Some(false)),
            (r#""devDependencies": {"jest": "^30.0.0"}"#, Some(true)),
            (r#""devDependencies": {"jest": "~31.2"}"#, Some(true)),
            (r#""devDependencies": {"jest": ">=29"}"#, None),
            (r#""devDependencies": {"ts-jest": "^29.0.0"}"#, None),
        ] {
            let package = format!(r#"{{"jest": {{}}, {dependency}}}"#);
            let files = [("package.json", package.as_str())];
            for ext in ["mjs", "cjs", "mts", "cts"] {
                for name in ["src/a.test", "src/__tests__/a"] {
                    let path = format!("{name}.{ext}");
                    let got = status(&files, &path);
                    let want = match expected {
                        Some(true) => RunnerCollectionStatus::Collected,
                        Some(false) => RunnerCollectionStatus::NotCollected,
                        None => RunnerCollectionStatus::Unknown(JEST_VERSION_UNKNOWN.to_string()),
                    };
                    assert_eq!(got, want, "{dependency} {path}");
                }
                // A file the default patterns do not name is out whatever the version.
                assert_eq!(
                    status(&files, &format!("src/index.{ext}")),
                    RunnerCollectionStatus::NotCollected,
                    "{dependency} {ext}"
                );
            }
            assert_eq!(
                status(&files, "src/a.test.ts"),
                RunnerCollectionStatus::Collected,
                "{dependency}"
            );
        }
        // A configured pattern decides whatever the version.
        let files = [(
            "package.json",
            r#"{"jest": {"testMatch": ["**/*.test.mjs"]}, "devDependencies": {"jest": "^29.0.0"}}"#,
        )];
        assert_eq!(
            status(&files, "src/a.test.mjs"),
            RunnerCollectionStatus::Collected
        );
    }

    const VITEST_LITERAL: &str = "export default defineConfig({\n  test: {\n    include: ['checks/**/*.test.ts'],\n    exclude: ['**/parked/**'],\n  },\n});\n";
    const VITEST_DEFAULTS: &str = "export default defineConfig({ test: { globals: true } });\n";
    const VITEST_ROOTED: &str =
        "export default defineConfig({ test: { root: './web', include: ['**/*.check.ts'] } });\n";
    const VITEST_COMPUTED: &str = "export default defineConfig(({ mode }) => ({ test: {} }));\n";
    const VITE_PLAIN: &str = "export default defineConfig({ plugins: [] });\n";

    #[test]
    fn test_vitest_literal_configuration() {
        let files = [("package.json", "{}"), ("vitest.config.ts", VITEST_LITERAL)];
        for (path, expected) in [
            ("checks/a.test.ts", RunnerCollectionStatus::Collected),
            ("checks/deep/a.test.ts", RunnerCollectionStatus::Collected),
            (
                "checks/parked/a.test.ts",
                RunnerCollectionStatus::NotCollected,
            ),
            ("src/a.test.ts", RunnerCollectionStatus::NotCollected),
        ] {
            assert_eq!(status(&files, path), expected, "{path}");
        }
        // Vitest's own defaults: `.test.` / `.spec.` names with the module extensions,
        // no `__tests__` convention, nothing under `node_modules`.
        let files = [
            ("package.json", "{}"),
            ("vitest.config.mts", VITEST_DEFAULTS),
        ];
        for (path, expected) in [
            ("src/a.test.ts", RunnerCollectionStatus::Collected),
            ("src/a.spec.mjs", RunnerCollectionStatus::Collected),
            ("src/a.test.cts", RunnerCollectionStatus::Collected),
            ("src/__tests__/a.ts", RunnerCollectionStatus::NotCollected),
            ("src/test.ts", RunnerCollectionStatus::NotCollected),
            (
                "node_modules/dep/a.test.js",
                RunnerCollectionStatus::NotCollected,
            ),
        ] {
            assert_eq!(status(&files, path), expected, "{path}");
        }
        // An empty literal object is a configuration that is read: Vitest's defaults.
        let files = [("vitest.config.ts", "export default {}")];
        assert_eq!(
            status(&files, "test/foo.ts"),
            RunnerCollectionStatus::NotCollected
        );
        assert_eq!(
            status(&files, "test/foo.test.ts"),
            RunnerCollectionStatus::Collected
        );
        // A literal `root` is where the patterns resolve and the only place scanned.
        let files = [("package.json", "{}"), ("vite.config.js", VITEST_ROOTED)];
        assert_eq!(
            status(&files, "web/src/a.check.ts"),
            RunnerCollectionStatus::Collected
        );
        assert_eq!(
            status(&files, "src/a.check.ts"),
            RunnerCollectionStatus::NotCollected
        );
        // `vitest.config.*` wins over `vite.config.*`.
        let files = [
            ("package.json", "{}"),
            ("vitest.config.ts", VITEST_LITERAL),
            ("vite.config.ts", VITEST_ROOTED),
        ];
        assert_eq!(
            status(&files, "checks/a.test.ts"),
            RunnerCollectionStatus::Collected
        );
    }

    #[test]
    fn test_vitest_configuration_that_is_not_read() {
        let files = [
            ("package.json", "{}"),
            ("vitest.config.ts", VITEST_COMPUTED),
        ];
        assert_eq!(
            unknown_reason(&files, "src/a.test.ts"),
            "cannot parse statically: vitest.config.ts"
        );
        // A `vite.config.*` with no `test` block configures no test runner; a computed
        // one may hold a `test` block only where Vitest is a dependency.
        for config in [VITE_PLAIN, VITEST_COMPUTED] {
            let files = [("package.json", "{}"), ("vite.config.ts", config)];
            assert_eq!(
                unknown_reason(&files, "src/a.test.ts"),
                "no runner config found",
                "{config}"
            );
        }
        let files = [
            (
                "package.json",
                r#"{"devDependencies": {"vitest": "^3.0.0"}}"#,
            ),
            ("vite.config.ts", VITEST_COMPUTED),
        ];
        assert_eq!(
            unknown_reason(&files, "src/a.test.ts"),
            "cannot parse statically: vite.config.ts"
        );
        // Vitest reads no key of `package.json`.
        let files = [("package.json", r#"{"vitest": {"include": ["checks/**"]}}"#)];
        assert_eq!(
            unknown_reason(&files, "src/a.test.ts"),
            "no runner config found"
        );
        // A pattern the glob matcher cannot evaluate.
        let extglob = "export default { test: { exclude: ['**/*.+(old|new).ts'] } };\n";
        let files = [("package.json", "{}"), ("vitest.config.ts", extglob)];
        assert!(is_unknown("src/a.test.ts", &tree(&files)));
    }

    #[test]
    fn test_a_bun_or_deno_configuration_is_a_runner_sign() {
        for sign in ["bunfig.toml", "tools/bunfig.toml"] {
            assert_eq!(
                unknown_reason(&[(sign, "")], "checks/a.test.ts"),
                "no runner config found",
                "{sign}"
            );
        }
        // A root Deno configuration that does not parse says so; one that does decides.
        for sign in ["deno.json", "deno.jsonc"] {
            assert_eq!(
                unknown_reason(&[(sign, "")], "checks/a.test.ts"),
                crate::ast::runner_config::DENO_UNPARSED,
                "{sign}"
            );
            assert_eq!(
                status(&[(sign, "{}")], "checks/a.test.ts"),
                RunnerCollectionStatus::Collected,
                "{sign}"
            );
            assert_eq!(
                status(&[(sign, "{}")], "checks/plain.ts"),
                RunnerCollectionStatus::NotCollected,
                "{sign}"
            );
        }
        // With none, the note says how to declare a suite that needs no manifest.
        match status(&[("pytest.ini", "")], "test/a.test.mjs") {
            RunnerCollectionStatus::NoRunner(reason) => {
                assert!(reason.contains("`[tests] paths`"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// Twelve packages whose test target cannot be read. The packages are read in path
    /// order, so the read errors a recorder keeps, and the text it gives, are the same on
    /// every run: the first [`ReadRecorder::MAX_KEPT`] packages in path order.
    #[test]
    fn packages_are_read_in_path_order_so_the_kept_read_errors_do_not_vary() {
        use crate::gitctx::ReadRecorder;
        let packages: Vec<String> = (0..12).map(|i| format!("crates/p{i:02}")).collect();
        let mut tracked = Vec::new();
        // Listed in an order that is not the sorted one.
        for dir in packages.iter().rev() {
            tracked.push(format!("{dir}/Cargo.toml"));
            tracked.push(format!("{dir}/tests/it.rs"));
        }
        let reads = ReadRecorder::new();
        let mut read_in_order: Vec<String> = Vec::new();
        RunnerCollectionRules::from_tree(
            |path: &str| {
                if path.ends_with("/Cargo.toml") {
                    return reads.keep(Ok(Some(
                        "[package]\nname = \"p\"\nversion = \"0.0.0\"\n".to_string(),
                    )));
                }
                if path.ends_with(".rs") {
                    read_in_order.push(path.to_string());
                    return reads.keep(Err(anyhow::anyhow!("could not read `{path}`")));
                }
                None
            },
            &tracked,
        );
        let expected: Vec<String> = packages
            .iter()
            .map(|d| format!("{d}/tests/it.rs"))
            .collect();
        assert_eq!(read_in_order, expected);
        let shown = format!("{:#}", reads.finish().unwrap_err());
        assert_eq!(
            shown,
            "12 reads failed; the others: could not read `crates/p01/tests/it.rs`; \
             could not read `crates/p02/tests/it.rs`; could not read `crates/p03/tests/it.rs`; \
             could not read `crates/p04/tests/it.rs`, and 7 more not listed; the first: \
             could not read `crates/p00/tests/it.rs`"
        );
    }
}
