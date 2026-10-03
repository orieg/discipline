//! `discipline init`: initialize a starter `discipline.toml` configuration.
//!
//! Detects common test runners (cargo/nextest, pytest, jest/vitest, go test, Maven/Gradle)
//! and emits a commented `[gates.test-floor]` block with `test_command` and `test_report`
//! pre-configured for JUnit identity ratcheting.

use std::path::Path;

/// Recognized test runners that produce JUnit XML test reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunner {
    CargoNextest,
    Cargo,
    Pytest,
    Vitest,
    Jest,
    Go,
    Maven,
    Gradle,
}

impl TestRunner {
    pub fn name(self) -> &'static str {
        match self {
            TestRunner::CargoNextest => "cargo nextest",
            TestRunner::Cargo => "cargo test",
            TestRunner::Pytest => "pytest",
            TestRunner::Vitest => "vitest",
            TestRunner::Jest => "jest",
            TestRunner::Go => "go test",
            TestRunner::Maven => "Maven",
            TestRunner::Gradle => "Gradle",
        }
    }

    pub fn test_command(self) -> &'static str {
        match self {
            TestRunner::CargoNextest | TestRunner::Cargo => "cargo nextest run --profile ci",
            TestRunner::Pytest => "pytest --junitxml=reports/junit.xml",
            TestRunner::Vitest => "npx vitest run --reporter=junit --outputFile=reports/junit.xml",
            TestRunner::Jest => "npm test -- --reporters=jest-junit",
            TestRunner::Go => "gotestsum --junitfile reports/junit.xml",
            TestRunner::Maven => "mvn test",
            TestRunner::Gradle => "gradle test",
        }
    }

    pub fn test_report(self) -> &'static str {
        match self {
            TestRunner::CargoNextest | TestRunner::Cargo => "target/nextest/ci/junit.xml",
            TestRunner::Pytest | TestRunner::Vitest | TestRunner::Jest | TestRunner::Go => {
                "reports/junit.xml"
            }
            TestRunner::Maven => "target/surefire-reports/junit.xml",
            TestRunner::Gradle => "build/test-results/test/junit.xml",
        }
    }

    pub fn config_snippet(self) -> String {
        format!(
            "# [gates.test-floor]\n# test_command = \"{}\"\n# test_report = \"{}\"",
            self.test_command(),
            self.test_report()
        )
    }

    /// Detect test runner present in the directory `root`.
    pub fn detect(root: &Path) -> Option<Self> {
        // Rust
        if root.join("Cargo.toml").exists() {
            if root.join(".config/nextest.toml").exists() {
                return Some(TestRunner::CargoNextest);
            }
            if let Ok(content) = std::fs::read_to_string(root.join("Cargo.toml")) {
                if content.contains("nextest") {
                    return Some(TestRunner::CargoNextest);
                }
            }
            return Some(TestRunner::Cargo);
        }

        // Python
        if root.join("pytest.ini").exists()
            || root.join("conftest.py").exists()
            || root.join("tests/conftest.py").exists()
        {
            return Some(TestRunner::Pytest);
        }
        if let Ok(content) = std::fs::read_to_string(root.join("pyproject.toml")) {
            if content.contains("pytest") {
                return Some(TestRunner::Pytest);
            }
        }
        if let Ok(content) = std::fs::read_to_string(root.join("setup.cfg")) {
            if content.contains("pytest") {
                return Some(TestRunner::Pytest);
            }
        }

        // Go
        if root.join("go.mod").exists() {
            return Some(TestRunner::Go);
        }

        // JS / TS
        if root.join("vitest.config.ts").exists()
            || root.join("vitest.config.js").exists()
            || root.join("vitest.config.mts").exists()
            || root.join("vitest.config.mjs").exists()
        {
            return Some(TestRunner::Vitest);
        }
        if root.join("jest.config.js").exists()
            || root.join("jest.config.ts").exists()
            || root.join("jest.config.json").exists()
            || root.join("jest.config.mjs").exists()
            || root.join("jest.config.cjs").exists()
        {
            return Some(TestRunner::Jest);
        }
        if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
            if content.contains("\"vitest\"") {
                return Some(TestRunner::Vitest);
            }
            if content.contains("\"jest\"") || content.contains("\"test\"") {
                return Some(TestRunner::Jest);
            }
        }

        // Java / Kotlin / JVM
        if root.join("pom.xml").exists() {
            return Some(TestRunner::Maven);
        }
        if root.join("build.gradle").exists() || root.join("build.gradle.kts").exists() {
            return Some(TestRunner::Gradle);
        }

        // Fallback for Python repositories with requirements or setup.py
        if root.join("requirements.txt").exists()
            || root.join("setup.py").exists()
            || root.join("pyproject.toml").exists()
        {
            return Some(TestRunner::Pytest);
        }

        None
    }
}

/// Generates the starter `discipline.toml` content for a project.
pub fn generate_starter(project_name: &str, root: &Path) -> String {
    let runner = TestRunner::detect(root).unwrap_or(TestRunner::CargoNextest);
    let test_floor_snippet = runner.config_snippet();

    format!(
        r#"# discipline.toml — configuration for Discipline CI gatekeeper.
#
# Schema version 1. Gates run at their built-in default enablement and severity.
# You only need to specify settings that differ from the defaults.
# Run `discipline gates` to view the effective status of all gates.

[meta]
version = 1
name = "{project_name}"
# description = "Brief description of the project"

# [directives]
# sources = ["pr-body", "commits", "merged-pr-body"]
# allow_hidden = false
# fail_on_overrides = false

# Gate customizations (examples):
# [gates.assertion-reduction]
# severity = "error"
# exempt_paths = ["tests/legacy/**"]

{test_floor_snippet}

# [gates.pii]
# allowed_users = ["runner", "user", "username"]
# hostname_denylist = ["internal.corp"]

# [gates.time-estimates]
# allow_patterns = ['^timeout: \d+']
"#
    )
}
