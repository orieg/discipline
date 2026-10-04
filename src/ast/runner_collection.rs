//! Runner test collection rules for language packs.
//!
//! Determines whether a file is collected and executed by its framework's test runner:
//! - Python (pytest): `python_files`, `python_classes`, `python_functions`, `testpaths`
//!   from `pyproject.toml`, `pytest.ini`, `setup.cfg`, `tox.ini`.
//! - JS / TS (Jest / Vitest): `testMatch`, `testRegex`, `include` from `package.json`, `jest.config.*`.
//! - Go: `_test.go` suffix.
//! - Rust: `tests/*.rs`, `tests/*/main.rs`, and explicit `[[test]] path` in `Cargo.toml`.
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
}

impl Default for PytestCollectionRules {
    fn default() -> Self {
        Self {
            python_files: vec!["test_*.py".to_string(), "*_test.py".to_string()],
            python_classes: vec!["Test*".to_string()],
            python_functions: vec!["test_*".to_string()],
            testpaths: Vec::new(),
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
            let in_testpath = self.testpaths.iter().any(|tp| {
                let trimmed = tp.trim().trim_matches('/');
                if trimmed.is_empty() {
                    return true;
                }
                norm == trimmed
                    || norm.starts_with(&format!("{trimmed}/"))
                    || glob_match(trimmed, &norm)
            });
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
    pub test_match: Vec<String>,
    pub test_regex: Vec<String>,
    pub include: Vec<String>,
    compiled_globs: Vec<globset::GlobMatcher>,
    compiled_regexes: Vec<regex::Regex>,
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

impl JsCollectionRules {
    pub fn compile_patterns(&mut self) {
        self.compiled_regexes.clear();
        self.compiled_globs.clear();
        self.invalid_pattern = None;

        for r in &self.test_regex {
            match regex::Regex::new(r) {
                Ok(re) => self.compiled_regexes.push(re),
                Err(_) => {
                    if self.invalid_pattern.is_none() {
                        self.invalid_pattern =
                            Some(format!("regex pattern failed to compile: '{r}'"));
                    }
                }
            }
        }

        for pat in self.test_match.iter().chain(self.include.iter()) {
            let trimmed = pat.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Jest `<rootDir>` is the directory holding the configuration; paths here
            // are repository-relative, so it is the empty prefix.
            let without_root = trimmed
                .strip_prefix("<rootDir>/")
                .or_else(|| trimmed.strip_prefix("<rootDir>"))
                .unwrap_or(trimmed);
            if has_extglob(without_root) {
                if self.invalid_pattern.is_none() {
                    self.invalid_pattern = Some(format!(
                        "glob pattern uses extglob, cannot evaluate statically: '{trimmed}'"
                    ));
                }
                continue;
            }
            match globset::GlobBuilder::new(without_root)
                .literal_separator(false)
                .build()
            {
                Ok(glob) => self.compiled_globs.push(glob.compile_matcher()),
                Err(_) => {
                    if self.invalid_pattern.is_none() {
                        self.invalid_pattern =
                            Some(format!("glob pattern failed to compile: '{trimmed}'"));
                    }
                }
            }
        }
    }

    pub fn is_collected(&self, path: &str) -> JsCollectionResult {
        let norm = path.replace('\\', "/");
        let lower = norm.to_ascii_lowercase();
        let valid_ext = lower.ends_with(".js")
            || lower.ends_with(".jsx")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs");
        if !valid_ext {
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

        // 5. Config was parsed: check configured patterns or defaults
        let has_custom =
            !self.test_regex.is_empty() || !self.test_match.is_empty() || !self.include.is_empty();

        if has_custom {
            if self.compiled_regexes.iter().any(|re| re.is_match(&norm)) {
                return JsCollectionResult::Collected;
            }
            if self.compiled_globs.iter().any(|m| m.is_match(&norm)) {
                return JsCollectionResult::Collected;
            }
            return JsCollectionResult::NotCollected;
        }

        // Default Jest / Vitest collection conventions
        // 1. Inside __tests__/
        if norm.contains("/__tests__/") || norm.starts_with("__tests__/") {
            return JsCollectionResult::Collected;
        }

        // 2. Basename contains .test. or .spec.
        let filename = norm.rsplit('/').next().unwrap_or(&norm);
        let f_lower = filename.to_ascii_lowercase();
        if f_lower.contains(".test.") || f_lower.contains(".spec.") {
            return JsCollectionResult::Collected;
        }

        JsCollectionResult::NotCollected
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

    pub fn merge_jest_config_json(&mut self, content: &str) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
            return;
        };
        self.config_parsed = true;
        self.extract_from_json(&val);
        self.compile_patterns();
    }

    fn extract_from_json(&mut self, val: &serde_json::Value) {
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

/// Rust test collection and feature configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RustCollectionRules {
    pub declared_features: HashSet<String>,
    pub custom_test_paths: Vec<String>,
    /// Every `Cargo.toml` on this side, by repository-relative path, read through the
    /// same reader as the rest of the rules (base blob or head tree).
    pub manifests: std::collections::HashMap<String, ManifestStatus>,
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
    if node.kind() == "attribute_item" {
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

/// Combined collection rules across supported runners.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerCollectionRules {
    pub pytest: PytestCollectionRules,
    pub js: JsCollectionRules,
    pub rust: RustCollectionRules,
}

impl RunnerCollectionRules {
    pub fn from_files<F>(reader: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        Self::from_files_with_manifests(reader, &["Cargo.toml".to_string()])
    }

    /// As [`Self::from_files`], also reading each `Cargo.toml` in `manifest_paths` so
    /// feature-gated tests resolve against their own crate.
    pub fn from_files_with_manifests<F>(mut reader: F, manifest_paths: &[String]) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut rules = Self::default();

        // 1. Pytest
        if let Some(src) = reader("pyproject.toml") {
            let parsed = PytestCollectionRules::parse_pyproject_toml(&src);
            if parsed != PytestCollectionRules::default() {
                rules.pytest = parsed;
            }
        }
        for ini_file in &["pytest.ini", "setup.cfg", "tox.ini"] {
            if let Some(src) = reader(ini_file) {
                let parsed = PytestCollectionRules::parse_ini(&src);
                if parsed != PytestCollectionRules::default() {
                    rules.pytest = parsed;
                    break;
                }
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

        // 3. Rust Cargo.toml
        let root_manifest = reader("Cargo.toml");
        if let Some(src) = &root_manifest {
            rules.rust = RustCollectionRules::parse_cargo_toml(src);
            rules.rust.manifests.insert(
                "Cargo.toml".to_string(),
                RustCollectionRules::manifest_status(src),
            );
        }
        for path in manifest_paths.iter().filter(|p| p.as_str() != "Cargo.toml") {
            if let Some(src) = reader(path) {
                rules
                    .rust
                    .manifests
                    .insert(path.clone(), RustCollectionRules::manifest_status(&src));
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
}

/// Evaluates whether a file path is collected by its language runner given repository vocabulary.
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
        if vocab.runner_rules.pytest.is_collected(&norm) {
            RunnerCollectionStatus::Collected
        } else {
            RunnerCollectionStatus::NotCollected
        }
    } else if lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
    {
        match vocab.runner_rules.js.is_collected(&norm) {
            JsCollectionResult::Collected => RunnerCollectionStatus::Collected,
            JsCollectionResult::NotCollected => RunnerCollectionStatus::NotCollected,
            JsCollectionResult::Unknown(reason) => RunnerCollectionStatus::Unknown(reason),
        }
    } else if lower.ends_with(".rs") {
        if vocab.runner_rules.rust.is_collected(&norm) {
            RunnerCollectionStatus::Collected
        } else {
            RunnerCollectionStatus::NotCollected
        }
    } else if lower.ends_with(".go") {
        if lower.ends_with("_test.go") {
            RunnerCollectionStatus::Collected
        } else {
            RunnerCollectionStatus::NotCollected
        }
    } else {
        if crate::ast::functions::test_path(&norm) {
            RunnerCollectionStatus::Collected
        } else {
            RunnerCollectionStatus::NotCollected
        }
    }
}

pub fn is_runner_collected(path: &str, vocab: &crate::ast::AssertVocabulary) -> bool {
    match check_runner_collected(path, vocab) {
        RunnerCollectionStatus::Collected => true,
        RunnerCollectionStatus::NotCollected => false,
        RunnerCollectionStatus::Unknown(_) => {
            let norm = path.replace('\\', "/");
            crate::ast::functions::test_path(&norm)
        }
    }
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
}
