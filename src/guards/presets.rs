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
    /// Committed file the command's stdout must match; it is also a policy file.
    pub snapshot: Option<&'static str>,
    /// Regexes for lines left out of the snapshot comparison.
    pub snapshot_ignore: &'static [&'static str],
    pub description: &'static str,
}

/// The mutation presets' zero-items guard. `\b` keeps it to a count of exactly zero: a
/// `0` with a digit before it ends a larger count (`10 mutants tested`), which is not an
/// empty run (#847).
pub const ZERO_MUTANTS_TESTED: &str = r"\b0 mutants tested";

pub static PRESETS: &[PresetDefinition] = &[
    // 1. Diff-Scoped Mutation Testing
    PresetDefinition {
        id: "cargo-mutants",
        category: "mutation",
        default_command: "cargo mutants --in-diff",
        default_timeout_seconds: 300,
        zero_items_pattern: Some(ZERO_MUTANTS_TESTED),
        forbid_output: &["survived", "MISSED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[".cargo/mutants.toml"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "Rust mutation testing scoped to diff changes using cargo-mutants",
    },
    PresetDefinition {
        id: "mutmut",
        category: "mutation",
        default_command: "mutmut run",
        default_timeout_seconds: 300,
        zero_items_pattern: Some(ZERO_MUTANTS_TESTED),
        forbid_output: &["survived", "SURVIVED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["setup.cfg", "pyproject.toml"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "Python mutation testing scoped to changed code using mutmut",
    },
    PresetDefinition {
        id: "stryker",
        category: "mutation",
        default_command: "npx stryker run",
        default_timeout_seconds: 300,
        zero_items_pattern: Some(ZERO_MUTANTS_TESTED),
        forbid_output: &["Survived", "SURVIVED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["stryker.config.json", "stryker.conf.json"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "JavaScript/TypeScript mutation testing using Stryker",
    },
    PresetDefinition {
        id: "pit",
        category: "mutation",
        default_command: "mvn org.pitest:pitest-maven:mutationCoverage",
        default_timeout_seconds: 300,
        zero_items_pattern: Some(ZERO_MUTANTS_TESTED),
        forbid_output: &["SURVIVED"],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["pom.xml"],
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
        description: "Go vulnerability database inspection",
    },
    PresetDefinition {
        id: "cargo-resolve-locked",
        category: "supply-chain",
        default_command: "cargo metadata --locked --format-version 1",
        default_timeout_seconds: 120,
        zero_items_pattern: None,
        forbid_output: &[
            "no matching package named",
            "failed to select a version for",
            "needs to be updated but --locked was passed",
        ],
        canary_command: None,
        canary_expected_diagnostic: Some("failed to select a version for"),
        policy_files: &["Cargo.toml", "Cargo.lock"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "Cargo manifest and lockfile resolution without executing build scripts",
    },
    PresetDefinition {
        id: "npm-resolve-locked",
        category: "supply-chain",
        default_command: "npm ci --ignore-scripts --dry-run",
        default_timeout_seconds: 120,
        zero_items_pattern: None,
        forbid_output: &[
            "ETARGET",
            "E404",
            "No matching version found for",
            "can only install packages when your package.json and package-lock.json",
        ],
        canary_command: None,
        canary_expected_diagnostic: Some("No matching version found for"),
        policy_files: &["package.json", "package-lock.json"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "Node.js npm lockfile resolution dry-run with install scripts disabled",
    },
    PresetDefinition {
        id: "uv-resolve-locked",
        category: "supply-chain",
        default_command: "uv lock --check",
        default_timeout_seconds: 120,
        zero_items_pattern: None,
        forbid_output: &[
            "No solution found when resolving dependencies",
            "was not found in the package registry",
            "The lockfile at `uv.lock` needs to be updated",
        ],
        canary_command: None,
        canary_expected_diagnostic: Some("was not found in the package registry"),
        policy_files: &["pyproject.toml", "uv.lock"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "Python uv manifest and lockfile consistency and resolution check",
    },
    PresetDefinition {
        id: "go-resolve-locked",
        category: "supply-chain",
        default_command: "go mod verify",
        default_timeout_seconds: 120,
        zero_items_pattern: None,
        forbid_output: &[
            "unknown revision",
            "no matching versions for query",
            "SECURITY ERROR",
        ],
        canary_command: None,
        canary_expected_diagnostic: Some("no matching versions for query"),
        policy_files: &["go.mod", "go.sum"],
        snapshot: None,
        snapshot_ignore: &[],
        description: "Go module lockfile and checksum verification",
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
        snapshot: None,
        snapshot_ignore: &[],
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
        snapshot: None,
        snapshot_ignore: &[],
        description: "Rust Undefined Behavior detection with Miri and zero-tests guard",
    },
    PresetDefinition {
        id: "cargo-public-api",
        category: "semver",
        default_command: "cargo public-api --simplified",
        default_timeout_seconds: 180,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &["public-api.txt"],
        snapshot: Some("public-api.txt"),
        snapshot_ignore: &["^#"],
        description: "Rust public API surface compared with the committed public-api.txt (cargo-public-api, nightly)",
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
        snapshot: None,
        snapshot_ignore: &[],
        description: "Runtime address and thread sanitizer runner with diagnostic verification",
    },
    // 7. Base Branch Test Overlay
    PresetDefinition {
        id: "base-tests",
        category: "behavior",
        default_command: "cargo test -- --format=junit",
        default_timeout_seconds: 300,
        zero_items_pattern: None,
        forbid_output: &[],
        canary_command: None,
        canary_expected_diagnostic: None,
        policy_files: &[],
        snapshot: None,
        snapshot_ignore: &[],
        description:
            "Executes base branch test files against head code to detect behavior regressions",
    },
];

impl PresetDefinition {
    /// The value this preset supplies for `key` when the configuration leaves it unset,
    /// for the keys where a configured value replaces the preset's instead of adding to
    /// it and nothing is executed: `zero_items_pattern`, `canary_expected_diagnostic` and
    /// `snapshot`. `forbid_output` and `snapshot_ignore` are added to the configured
    /// lists, and no preset supplies `count_pattern` or `min_count`.
    pub fn replaced_default(&self, key: &str) -> Option<&'static str> {
        match key {
            "zero_items_pattern" => self.zero_items_pattern,
            "canary_expected_diagnostic" => self.canary_expected_diagnostic,
            "snapshot" => self.snapshot,
            _ => None,
        }
    }

    /// The value this preset supplies for `key` when the configuration leaves it unset,
    /// for every key where a configured value takes the preset's place: the three of
    /// [`Self::replaced_default`], and the two that are executed, `command` and
    /// `canary_command`.
    pub fn supplied_default(&self, key: &str) -> Option<&'static str> {
        match key {
            "command" => Some(self.default_command),
            "canary_command" => self.canary_command,
            _ => self.replaced_default(key),
        }
    }
}

/// Resolves a preset by its unique identifier.
pub fn resolve_preset(id: &str) -> Option<&'static PresetDefinition> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Whether `id` is one of the package-manager lockfile resolution presets (#704).
pub fn is_resolve_locked_preset(id: &str) -> bool {
    matches!(
        id,
        "cargo-resolve-locked" | "npm-resolve-locked" | "uv-resolve-locked" | "go-resolve-locked"
    )
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
            "cargo-resolve-locked",
            "npm-resolve-locked",
            "uv-resolve-locked",
            "go-resolve-locked",
            "loom",
            "miri",
            "cargo-public-api",
            "sanitizers",
            "base-tests",
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
    fn replaced_default_names_only_the_keys_a_configured_value_replaces() {
        let mutants = resolve_preset("cargo-mutants").unwrap();
        assert_eq!(
            mutants.replaced_default("zero_items_pattern"),
            Some(r"\b0 mutants tested")
        );
        // Merged with the configured list, so there is no default to replace.
        assert_eq!(mutants.replaced_default("forbid_output"), None);
        assert_eq!(mutants.replaced_default("snapshot"), None);
        let api = resolve_preset("cargo-public-api").unwrap();
        assert_eq!(api.replaced_default("snapshot"), Some("public-api.txt"));
        assert_eq!(api.replaced_default("snapshot_ignore"), None);
        let san = resolve_preset("sanitizers").unwrap();
        assert_eq!(
            san.replaced_default("canary_expected_diagnostic"),
            Some("ThreadSanitizer: data race")
        );
        // Executed, so the `command` gate refuses a change to it; not judged here.
        assert_eq!(san.replaced_default("canary_command"), None);
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
        assert_eq!(
            cargo_mutants.zero_items_pattern,
            Some(r"\b0 mutants tested")
        );
        assert!(cargo_mutants.forbid_output.contains(&"survived"));

        let loom = resolve_preset("loom").unwrap();
        assert_eq!(loom.category, "concurrency");
        assert_eq!(loom.zero_items_pattern, Some("running 0 tests"));

        let cargo_deny = resolve_preset("cargo-deny").unwrap();
        assert_eq!(cargo_deny.category, "supply-chain");
        assert_eq!(cargo_deny.policy_files, &["deny.toml"]);

        for id in [
            "cargo-resolve-locked",
            "npm-resolve-locked",
            "uv-resolve-locked",
            "go-resolve-locked",
        ] {
            let p = resolve_preset(id).unwrap();
            assert_eq!(p.category, "supply-chain");
            assert!(is_resolve_locked_preset(id));
            assert!(!p.forbid_output.is_empty());
            assert!(p.canary_expected_diagnostic.is_some());
            assert_eq!(p.policy_files.len(), 2);
        }
        assert!(!is_resolve_locked_preset("cargo-deny"));
    }
    /// `forbid_output`, `zero_items_pattern` and `canary_expected_diagnostic` are regular
    /// expressions with no literal fallback (#600). Every built-in default must compile,
    /// and none may depend on being read as text: each matches its own text. The
    /// zero-items guards, written as expressions, are checked against tool output in
    /// `every_zero_items_pattern_matches_only_a_whole_count_of_zero`.
    #[test]
    fn every_preset_output_pattern_compiles_and_matches_its_own_text() {
        let mut checked = 0;
        for preset in PRESETS {
            let patterns = preset
                .forbid_output
                .iter()
                .copied()
                .chain(preset.canary_expected_diagnostic);
            for pattern in patterns {
                let re = crate::guards::command::output_pattern(pattern, preset.id)
                    .unwrap_or_else(|e| panic!("preset `{}`: {e:#}", preset.id));
                // The text a literal match would have looked for.
                let text = pattern.replace('\\', "");
                assert!(re.is_match(&text), "preset `{}`: `{pattern}`", preset.id);
                assert!(
                    !re.is_match("nothing of the kind"),
                    "preset `{}`: `{pattern}`",
                    preset.id
                );
                checked += 1;
            }
        }
        assert!(checked >= 15, "{checked} patterns checked");
    }

    /// A zero-items guard means "nothing ran", so it must match a count of exactly zero
    /// and never a larger count that ends in `0` (#847). Each preset that has one is
    /// listed with output it must match and output it must not.
    #[test]
    fn every_zero_items_pattern_matches_only_a_whole_count_of_zero() {
        type Samples = (
            &'static str,
            &'static [&'static str],
            &'static [&'static str],
        );
        let mutants_zero: &[&str] = &[
            "0 mutants tested",
            "0 mutants tested in 1s",
            "Found 3 mutants\n0 mutants tested in 2s: 0 caught",
            "summary: 0 mutants tested",
        ];
        let mutants_more: &[&str] = &[
            "10 mutants tested in 5s: 10 caught",
            "20 mutants tested",
            "100 mutants tested in 9s: 99 caught, 1 unviable",
            "3 mutants tested\n30 mutants tested",
        ];
        let tests_zero: &[&str] = &[
            "running 0 tests",
            "running 3 tests\ntest result: ok\nrunning 0 tests\n",
        ];
        let tests_more: &[&str] = &["running 10 tests", "running 20 tests", "running 100 tests"];
        let samples: &[Samples] = &[
            ("cargo-mutants", mutants_zero, mutants_more),
            ("mutmut", mutants_zero, mutants_more),
            ("stryker", mutants_zero, mutants_more),
            ("pit", mutants_zero, mutants_more),
            (
                "lcov",
                &["  lines......: 0% (0 of 12 lines)"],
                &[
                    "  lines......: 10% (1 of 10 lines)",
                    "  lines......: 20% (2 of 10 lines)",
                    "  lines......: 100% (12 of 12 lines)",
                ],
            ),
            ("loom", tests_zero, tests_more),
            ("miri", tests_zero, tests_more),
        ];
        for preset in PRESETS {
            let Some(pattern) = preset.zero_items_pattern else {
                continue;
            };
            let (_, zero, more) = samples
                .iter()
                .find(|(id, _, _)| *id == preset.id)
                .unwrap_or_else(|| panic!("preset `{}` has no samples here", preset.id));
            let re = crate::guards::command::output_pattern(pattern, preset.id).unwrap();
            for out in *zero {
                assert!(re.is_match(out), "preset `{}`: {out:?}", preset.id);
            }
            for out in *more {
                assert!(!re.is_match(out), "preset `{}`: {out:?}", preset.id);
            }
        }
        assert_eq!(
            PRESETS
                .iter()
                .filter(|p| p.zero_items_pattern.is_some())
                .count(),
            samples.len()
        );
    }
}
