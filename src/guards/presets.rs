//! Turnkey data-driven presets for the universal `command` gate.
//!
//! Presets are structured data configurations that define defaults for verification
//! tools across mutation testing, diff coverage, semver/API snapshotting,
//! supply-chain audits, and deterministic concurrency testing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetDefinition {
    pub id: &'static str,
    pub category: &'static str,
    pub default_command: &'static str,
    pub default_timeout_seconds: u64,
    pub zero_items_pattern: Option<&'static str>,
    pub forbid_output: &'static [&'static str],
    pub canary_command: Option<&'static str>,
    pub canary_expected_diagnostic: Option<&'static str>,
    pub policy_files: &'static [&'static str],
    pub description: &'static str,
}

pub static PRESETS: &[PresetDefinition] = &[
    // 1. Diff-Scoped Mutation Testing
    PresetDefinition {
        id: "cargo-mutants",
        category: "mutation",
        default_command: "cargo mutants --in-diff",
        default_timeout_seconds: 300,
        zero_items_pattern: Some("0 mutants tested"),
        forbid_output: &["survived", "MISSED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[".cargo/mutants.toml"],
        description: "Rust mutation testing scoped to diff changes using cargo-mutants",
    },
    PresetDefinition {
        id: "mutmut",
        category: "mutation",
        default_command: "mutmut run",
        default_timeout_seconds: 300,
        zero_items_pattern: Some("0 mutants tested"),
        forbid_output: &["survived", "SURVIVED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["setup.cfg", "pyproject.toml"],
        description: "Python mutation testing scoped to changed code using mutmut",
    },
    PresetDefinition {
        id: "stryker",
        category: "mutation",
        default_command: "npx stryker run",
        default_timeout_seconds: 300,
        zero_items_pattern: Some("0 mutants tested"),
        forbid_output: &["Survived", "SURVIVED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["stryker.config.json", "stryker.conf.json"],
        description: "JavaScript/TypeScript mutation testing using Stryker",
    },
    PresetDefinition {
        id: "pit",
        category: "mutation",
        default_command: "mvn org.pitest:pitest-maven:mutationCoverage",
        default_timeout_seconds: 300,
        zero_items_pattern: Some("0 mutants tested"),
        forbid_output: &["SURVIVED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["pom.xml"],
        description: "Java/JVM mutation testing using PIT",
    },
    // 2. Diff Coverage
    PresetDefinition {
        id: "lcov",
        category: "coverage",
        default_command: "lcov --summary lcov.info",
        default_timeout_seconds: 60,
        zero_items_pattern: Some(r"lines\.\.\.\.\.\.: 0%"),
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["lcov.info"],
        description: "LCOV code coverage report summary inspector",
    },
    PresetDefinition {
        id: "cobertura",
        category: "coverage",
        default_command:
            "python3 -c \"import xml.etree.ElementTree as ET; ET.parse('coverage.xml')\"",
        default_timeout_seconds: 60,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["coverage.xml"],
        description: "Cobertura XML code coverage report validator",
    },
    // 3. Semver & API Compatibility
    PresetDefinition {
        id: "cargo-semver-checks",
        category: "semver",
        default_command: "cargo semver-checks check-release",
        default_timeout_seconds: 180,
        zero_items_pattern: None,
        forbid_output: &["semver-checks failed", "breaking change"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[],
        description: "Rust semver and public API compatibility enforcement",
    },
    PresetDefinition {
        id: "api-snapshot",
        category: "semver",
        default_command: "git diff --exit-code api.snapshot",
        default_timeout_seconds: 30,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["api.snapshot"],
        description: "Language-neutral public API surface snapshot discrepancy checker",
    },
    // 4. Supply Chain & Advisory Wrappers
    PresetDefinition {
        id: "cargo-deny",
        category: "supply-chain",
        default_command: "cargo deny check",
        default_timeout_seconds: 60,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["deny.toml"],
        description: "Cargo dependency policy, advisory, and license enforcement",
    },
    PresetDefinition {
        id: "pip-audit",
        category: "supply-chain",
        default_command: "pip-audit",
        default_timeout_seconds: 60,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[],
        description: "Python PyPI known vulnerability and dependency audit",
    },
    PresetDefinition {
        id: "npm-audit",
        category: "supply-chain",
        default_command: "npm audit --audit-level=high",
        default_timeout_seconds: 60,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["package-lock.json"],
        description: "Node.js npm dependency vulnerability audit",
    },
    PresetDefinition {
        id: "govulncheck",
        category: "supply-chain",
        default_command: "govulncheck ./...",
        default_timeout_seconds: 60,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["go.mod"],
        description: "Go vulnerability database inspection",
    },
    // 5. Deterministic Concurrency Testing
    PresetDefinition {
        id: "loom",
        category: "concurrency",
        default_command: "cargo test --test loom -- --nocapture",
        default_timeout_seconds: 300,
        zero_items_pattern: Some("running 0 tests"),
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[],
        description: "Rust loom deterministic concurrency permutation test runner",
    },
    PresetDefinition {
        id: "miri",
        category: "concurrency",
        default_command: "cargo miri test",
        default_timeout_seconds: 600,
        zero_items_pattern: Some("running 0 tests"),
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[],
        description: "Rust Undefined Behavior detection with Miri and zero-tests guard",
    },
    PresetDefinition {
        id: "cargo-public-api",
        category: "semver",
        default_command: "cargo public-api diff",
        default_timeout_seconds: 180,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["public-api.txt"],
        description: "Rust public API surface diff inspector using cargo-public-api",
    },
    // 6. Runtime Sanitizers
    PresetDefinition {
        id: "sanitizers",
        category: "sanitizer",
        default_command: "cargo test -Zsanitizer=address",
        default_timeout_seconds: 300,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: Some("cargo test --test race_canary"),
        canary_expected_diagnostic: Some("ThreadSanitizer: data race"),
        policy_files: &[],
        description: "Runtime address and thread sanitizer runner with diagnostic verification",
    },
];

/// Resolves a preset by its unique identifier.
pub fn resolve_preset(id: &str) -> Option<&'static PresetDefinition> {
    PRESETS.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_known_presets() {
        let expected_ids = [
            "cargo-mutants",
            "mutmut",
            "stryker",
            "pit",
            "lcov",
            "cobertura",
            "cargo-semver-checks",
            "api-snapshot",
            "cargo-deny",
            "pip-audit",
            "npm-audit",
            "govulncheck",
            "loom",
            "miri",
            "cargo-public-api",
            "sanitizers",
        ];

        for id in expected_ids {
            let preset = resolve_preset(id);
            assert!(preset.is_some(), "expected preset `{id}` to resolve");
            let p = preset.unwrap();
            assert_eq!(p.id, id);
            assert!(!p.category.is_empty());
            assert!(!p.default_command.is_empty());
            assert!(!p.description.is_empty());
            assert!(p.default_timeout_seconds > 0);
        }
    }

    #[test]
    fn test_resolve_unknown_preset() {
        assert!(resolve_preset("nonexistent-tool").is_none());
        assert!(resolve_preset("").is_none());
    }

    #[test]
    fn test_preset_categories_and_invariants() {
        let cargo_mutants = resolve_preset("cargo-mutants").unwrap();
        assert_eq!(cargo_mutants.category, "mutation");
        assert_eq!(cargo_mutants.zero_items_pattern, Some("0 mutants tested"));
        assert!(cargo_mutants.forbid_output.contains(&"survived"));

        let loom = resolve_preset("loom").unwrap();
        assert_eq!(loom.category, "concurrency");
        assert_eq!(loom.zero_items_pattern, Some("running 0 tests"));

        let cargo_deny = resolve_preset("cargo-deny").unwrap();
        assert_eq!(cargo_deny.category, "supply-chain");
        assert_eq!(cargo_deny.policy_files, &["deny.toml"]);
    }
}
