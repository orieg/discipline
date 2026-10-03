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

/// JS / TS test runner (Jest / Vitest) collection configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JsCollectionRules {
    pub test_match: Vec<String>,
    pub test_regex: Vec<String>,
    pub include: Vec<String>,
}

impl JsCollectionRules {
    pub fn is_collected(&self, path: &str) -> bool {
        let norm = path.replace('\\', "/");
        let lower = norm.to_ascii_lowercase();
        let valid_ext = lower.ends_with(".js")
            || lower.ends_with(".jsx")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".mjs")
            || lower.ends_with(".cjs");
        if !valid_ext {
            return false;
        }

        // If custom testRegex is configured
        if !self.test_regex.is_empty() {
            let matched = self
                .test_regex
                .iter()
                .any(|r| regex::Regex::new(r).is_ok_and(|re| re.is_match(&norm)));
            if matched {
                return true;
            }
        }

        // If custom testMatch or include is configured
        let mut custom_globs = Vec::new();
        custom_globs.extend(self.test_match.iter().map(String::as_str));
        custom_globs.extend(self.include.iter().map(String::as_str));

        if !custom_globs.is_empty() {
            return custom_globs.iter().any(|g| glob_match(g, &norm));
        }

        // Default Jest / Vitest collection conventions
        // 1. Inside __tests__/
        if norm.contains("/__tests__/") || norm.starts_with("__tests__/") {
            return true;
        }

        // 2. Basename ends with .test.<ext> or .spec.<ext>
        let filename = norm.rsplit('/').next().unwrap_or(&norm);
        let f_lower = filename.to_ascii_lowercase();
        f_lower.contains(".test.")
            || f_lower.contains(".spec.")
            || f_lower.ends_with(".test.js")
            || f_lower.ends_with(".test.ts")
            || f_lower.ends_with(".test.jsx")
            || f_lower.ends_with(".test.tsx")
            || f_lower.ends_with(".test.mjs")
            || f_lower.ends_with(".test.cjs")
            || f_lower.ends_with(".spec.js")
            || f_lower.ends_with(".spec.ts")
            || f_lower.ends_with(".spec.jsx")
            || f_lower.ends_with(".spec.tsx")
            || f_lower.ends_with(".spec.mjs")
            || f_lower.ends_with(".spec.cjs")
    }

    pub fn merge_package_json(&mut self, content: &str) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
            return;
        };
        if let Some(jest) = val.get("jest") {
            self.extract_from_json(jest);
        }
    }

    pub fn merge_jest_config_json(&mut self, content: &str) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
            return;
        };
        self.extract_from_json(&val);
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

/// Rust test collection and feature configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RustCollectionRules {
    pub declared_features: HashSet<String>,
    pub custom_test_paths: Vec<String>,
}

impl RustCollectionRules {
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

        // Integration tests directly under tests/ or tests/*/main.rs
        let rel_opt = norm.strip_prefix("tests/").or_else(|| {
            norm.find("/tests/")
                .map(|idx| &norm[idx + "/tests/".len()..])
        });
        if let Some(rel) = rel_opt {
            // tests/*.rs (e.g. tests/a.rs)
            if !rel.contains('/') {
                return true;
            }
            // tests/<dir>/main.rs (e.g. tests/foo/main.rs)
            if rel.ends_with("/main.rs") && rel.matches('/').count() == 1 {
                return true;
            }
            return false;
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

/// Evaluates a Rust `#[cfg(...)]` attribute to determine if it is an unconditional ignore or conditional skip.
/// Returns `Some((is_unconditional, cond_str))` if the attribute contains a feature or `any()` condition.
pub fn evaluate_rust_cfg(
    attr_text: &str,
    declared_features: &HashSet<String>,
) -> Option<(bool, String)> {
    let normalized: String = attr_text.chars().filter(|c| !c.is_whitespace()).collect();

    // 1. Check for any() with no arguments or all(any())
    if normalized.contains("any()") && !normalized.contains("not(any())") {
        return Some((true, "any()".to_string()));
    }

    // 2. Check for feature = "..."
    let mut features = Vec::new();
    let mut rest = normalized.as_str();
    while let Some(idx) = rest.find("feature=") {
        let after = &rest[idx + "feature=".len()..];
        if let Some(quote) = after.chars().next() {
            if quote == '"' || quote == '\'' {
                let inside = &after[1..];
                if let Some(end_quote) = inside.find(quote) {
                    let feat = &inside[..end_quote];
                    features.push(feat.to_string());
                    rest = &inside[end_quote + 1..];
                    continue;
                }
            }
        }
        break;
    }

    if !features.is_empty() {
        let is_any_combinator = normalized.contains("any(");
        let is_unconditional = if is_any_combinator {
            // For any(feature = "a", feature = "b"), it's unconditional only if all are undeclared
            features.iter().all(|f| !declared_features.contains(f))
        } else {
            // For feature = "a" or all(feature = "a", ...), it's unconditional if any is undeclared
            features.iter().any(|f| !declared_features.contains(f))
        };
        if is_unconditional {
            let undeclared_list = features
                .iter()
                .filter(|f| !declared_features.contains(*f))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            return Some((true, format!("feature = \"{undeclared_list}\"")));
        } else {
            let feats = features.join(", ");
            return Some((false, format!("feature = \"{feats}\"")));
        }
    }

    None
}

/// Combined collection rules across supported runners.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunnerCollectionRules {
    pub pytest: PytestCollectionRules,
    pub js: JsCollectionRules,
    pub rust: RustCollectionRules,
}

impl RunnerCollectionRules {
    pub fn from_files<F>(mut reader: F) -> Self
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

        // 2. JS / Jest / Vitest
        if let Some(src) = reader("package.json") {
            rules.js.merge_package_json(&src);
        }
        if let Some(src) = reader("jest.config.json") {
            rules.js.merge_jest_config_json(&src);
        }

        // 3. Rust Cargo.toml
        if let Some(src) = reader("Cargo.toml") {
            rules.rust = RustCollectionRules::parse_cargo_toml(&src);
        }

        rules
    }
}

/// Evaluates whether a file path is collected by its language runner given repository vocabulary.
pub fn is_runner_collected(path: &str, vocab: &crate::ast::AssertVocabulary) -> bool {
    // 1. Explicit discipline.toml override
    if crate::ast::functions::declared_test_path(path, &vocab.test_paths) {
        return true;
    }

    let norm = path.replace('\\', "/");
    let lower = norm.to_ascii_lowercase();

    if lower.ends_with(".py") {
        vocab.runner_rules.pytest.is_collected(&norm)
    } else if lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
    {
        vocab.runner_rules.js.is_collected(&norm)
    } else if lower.ends_with(".rs") {
        vocab.runner_rules.rust.is_collected(&norm)
    } else if lower.ends_with(".go") {
        lower.ends_with("_test.go")
    } else {
        // Fallback for other languages: check standard test_path
        crate::ast::functions::test_path(&norm)
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
        let def = JsCollectionRules::default();
        assert!(def.is_collected("add.test.js"));
        assert!(def.is_collected("src/calc.spec.ts"));
        assert!(def.is_collected("__tests__/helper.js"));
        assert!(!def.is_collected("add.check.js"));
        assert!(!def.is_collected("src/index.js"));

        // Custom testMatch in package.json
        let pkg = r#"{"jest": {"testMatch": ["**/*.check.js"]}}"#;
        let mut custom = JsCollectionRules::default();
        custom.merge_package_json(pkg);
        assert!(custom.is_collected("add.check.js"));
        assert!(!custom.is_collected("add.test.js"));
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
        assert!(!rules.is_collected("tests/common/mod.rs"));
        assert!(!rules.is_collected("tests/common/helpers.rs"));
        assert!(rules.is_collected("tests/helpers.rs"));

        // Cfg features
        assert!(rules.declared_features.contains("declared_feat"));
        assert!(rules.declared_features.contains("opt_dep"));
        assert!(!rules.declared_features.contains("never"));

        // Undeclared feature -> unconditional ignore
        let (uncond, _) =
            evaluate_rust_cfg(r#"#[cfg(feature = "never")]"#, &rules.declared_features).unwrap();
        assert!(uncond);

        // Declared feature -> conditional skip
        let (uncond, _) = evaluate_rust_cfg(
            r#"#[cfg(feature = "declared_feat")]"#,
            &rules.declared_features,
        )
        .unwrap();
        assert!(!uncond);

        // any() -> unconditional ignore
        let (uncond, _) = evaluate_rust_cfg(r#"#[cfg(any())]"#, &rules.declared_features).unwrap();
        assert!(uncond);

        // all(any()) -> unconditional ignore
        let (uncond, _) =
            evaluate_rust_cfg(r#"#[cfg(all(any()))]"#, &rules.declared_features).unwrap();
        assert!(uncond);
    }
}
