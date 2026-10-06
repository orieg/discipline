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

    pub fn mutation_preset(self) -> Option<&'static str> {
        match self {
            TestRunner::CargoNextest | TestRunner::Cargo => Some("cargo-mutants"),
            TestRunner::Pytest => Some("mutmut"),
            TestRunner::Vitest | TestRunner::Jest => Some("stryker"),
            TestRunner::Maven | TestRunner::Gradle => Some("pit"),
            TestRunner::Go => None,
        }
    }

    /// The command to set beside [`Self::mutation_preset`] when the preset's own command
    /// is another build tool's: `pit` runs Maven, so a Gradle project names its own task
    /// (the `pitest` task of the gradle-pitest plugin) and keeps the preset's guards.
    pub fn mutation_command(self) -> Option<&'static str> {
        match self {
            TestRunner::Gradle => Some("gradle pitest"),
            _ => None,
        }
    }

    /// Whether [`Self::mutation_preset`]'s command mutates only what the change touched.
    /// Only `cargo mutants --in-diff` does; the other presets run the tool over the
    /// whole project.
    pub fn mutation_preset_is_diff_scoped(self) -> bool {
        matches!(self, TestRunner::CargoNextest | TestRunner::Cargo)
    }

    pub fn mutation_snippet(self) -> Option<String> {
        self.mutation_preset().map(|preset| {
            let heading = if self.mutation_preset_is_diff_scoped() {
                "# Diff-scoped mutation testing (guards against special-cased test inputs):"
            } else {
                "# Mutation testing (guards against special-cased test inputs; this preset's command runs over the whole project, not only the diff):"
            };
            let command = match self.mutation_command() {
                Some(command) => format!("\n# command = \"{command}\""),
                None => String::new(),
            };
            format!(
                "{heading}\n# [[gates.command.commands]]\n# name = \"mutation\"\n# preset = \"{preset}\"{command}"
            )
        })
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
    // With no runner detected the lines below name tools the repository shows no sign of,
    // so they are written as an example and say so.
    let detected = TestRunner::detect(root);
    let runner = detected.unwrap_or(TestRunner::CargoNextest);
    let example = if detected.is_some() {
        ""
    } else {
        "# No test runner was detected here. The commented lines below are an example for a\n# Cargo project: replace the command, the report path and the preset with your tools'.\n"
    };
    let test_floor_snippet = format!("{example}{}", runner.config_snippet());
    let mutation_part = match runner.mutation_snippet() {
        Some(s) => format!("\n{s}\n"),
        None => String::new(),
    };

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
{mutation_part}
# [gates.pii]
# allowed_users = ["runner", "user", "username"]
# hostname_denylist = ["internal.corp"]

# [gates.time-estimates]
# allow_patterns = ['^timeout: \d+']
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_RUNNERS: &[TestRunner] = &[
        TestRunner::CargoNextest,
        TestRunner::Cargo,
        TestRunner::Pytest,
        TestRunner::Vitest,
        TestRunner::Jest,
        TestRunner::Go,
        TestRunner::Maven,
        TestRunner::Gradle,
    ];

    /// A suggested preset runs with the project's own build tool: its own command starts
    /// with that tool, or the suggestion carries a command that does.
    #[test]
    fn a_suggested_mutation_preset_runs_with_the_project_build_tool() {
        let tool = |runner: TestRunner| match runner {
            TestRunner::CargoNextest | TestRunner::Cargo => "cargo ",
            TestRunner::Pytest => "mutmut ",
            TestRunner::Vitest | TestRunner::Jest => "npx ",
            TestRunner::Maven => "mvn ",
            TestRunner::Gradle => "gradle ",
            TestRunner::Go => "go ",
        };
        for runner in ALL_RUNNERS {
            let Some(preset) = runner.mutation_preset() else {
                assert!(runner.mutation_snippet().is_none());
                continue;
            };
            let def = crate::guards::presets::resolve_preset(preset).expect("a real preset");
            let command = runner.mutation_command().unwrap_or(def.default_command);
            assert!(
                command.starts_with(tool(*runner)),
                "{runner:?}: `{preset}` would run `{command}`"
            );
            let snippet = runner.mutation_snippet().unwrap();
            assert_eq!(
                snippet.contains("# command = "),
                runner.mutation_command().is_some(),
                "{snippet}"
            );
        }
    }

    /// Only a preset whose command takes the diff is called diff-scoped.
    #[test]
    fn only_a_preset_whose_command_takes_the_diff_is_called_diff_scoped() {
        for runner in ALL_RUNNERS {
            let Some(preset) = runner.mutation_preset() else {
                continue;
            };
            let def = crate::guards::presets::resolve_preset(preset).unwrap();
            let takes_the_diff = def.default_command.contains("--in-diff");
            assert_eq!(
                runner.mutation_preset_is_diff_scoped(),
                takes_the_diff,
                "{preset}"
            );
            assert_eq!(
                runner.mutation_snippet().unwrap().contains("Diff-scoped"),
                takes_the_diff,
                "{preset}"
            );
        }
    }
}
