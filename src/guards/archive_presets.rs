//! `archive-contents` presets: named `forbidden_patterns` lists for a release that
//! must not ship its source, merged with the user's own patterns.
//!
//! Python sdists and Rust crates ship source by design, so their presets forbid
//! only what no package should carry (maps, secrets, VCS and CI metadata, tests).
//! The presets for ecosystems that publish built output (npm, JVM, .NET, Go
//! binaries) also forbid source files and debug symbols.

/// One forbidden-name rule. `except` names the entries the rule does not apply
/// to (the regex crate has no look-around, so `\.ts$` but not `\.d\.ts$` is two
/// patterns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetRule {
    pub pattern: &'static str,
    pub except: Option<&'static str>,
    pub what: &'static str,
}

const fn rule(pattern: &'static str, what: &'static str) -> PresetRule {
    PresetRule {
        pattern,
        except: None,
        what,
    }
}

/// What no published package should carry.
pub const COMMON: &[PresetRule] = &[
    rule(r"\.map$", "source map"),
    rule(r"(^|/)\.env[^/]*$", "environment file"),
    rule(r"(^|/)\.git(/|$)", "git metadata"),
    rule(r"(^|/)(test|tests|__tests__)/", "test directory"),
    rule(
        r"(^|/)(\.github|\.gitlab|\.gitea|\.forgejo|\.circleci|\.buildkite)/",
        "CI configuration directory",
    ),
    rule(
        r"(^|/)(\.gitlab-ci\.yml|\.travis\.yml|azure-pipelines\.yml|Jenkinsfile)$",
        "CI configuration file",
    ),
    rule(r"\.(pem|key|p12)$", "private key or certificate bundle"),
    rule(r"(^|/)id_(rsa|dsa|ecdsa|ed25519)$", "SSH private key"),
    rule(r"(^|/)\.npmrc$", "npm credentials file"),
    rule(r"(^|/)\.pypirc$", "PyPI credentials file"),
];

/// A `src/` directory, in an ecosystem whose packages ship built output.
pub const SOURCE_DIR: &[PresetRule] = &[rule(r"(^|/)src/", "source directory")];

/// Debug symbols a binary release should not carry.
pub const DEBUG_SYMBOLS: &[PresetRule] = &[
    rule(r"\.dSYM(/|$)", "macOS debug symbols"),
    rule(r"\.debug$", "split debug symbols"),
];

pub const NPM: &[PresetRule] = &[PresetRule {
    pattern: r"\.(ts|tsx|mts|cts)$",
    except: Some(r"\.d\.(ts|mts|cts)$"),
    what: "TypeScript source (type declarations excepted)",
}];

pub const JVM: &[PresetRule] = &[rule(r"\.(java|kt|scala)$", "JVM source")];

pub const DOTNET: &[PresetRule] = &[
    rule(r"\.(cs|fs)$", ".NET source"),
    rule(r"\.pdb$", "program database (debug symbols)"),
];

/// A resolved preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePreset {
    pub name: &'static str,
    pub rules: Vec<PresetRule>,
    /// The preset turns on the content scan whatever `scan_contents` says.
    pub scan_contents: bool,
}

/// Every preset name, in documentation order.
pub const PRESET_NAMES: &[&str] = &[
    "no-source",
    "no-source-npm",
    "no-source-python",
    "no-source-jvm",
    "no-source-dotnet",
    "no-source-rust",
    "no-source-go",
];

fn union(groups: &[&[PresetRule]]) -> Vec<PresetRule> {
    let mut out: Vec<PresetRule> = Vec::new();
    for group in groups {
        for r in *group {
            if !out.iter().any(|o| o.pattern == r.pattern) {
                out.push(*r);
            }
        }
    }
    out
}

/// The preset called `name`, or `None` for an unknown name.
pub fn resolve(name: &str) -> Option<ArchivePreset> {
    let (name, rules, scan_contents) = match name {
        "no-source-npm" => (
            "no-source-npm",
            union(&[COMMON, SOURCE_DIR, DEBUG_SYMBOLS, NPM]),
            false,
        ),
        "no-source-jvm" => (
            "no-source-jvm",
            union(&[COMMON, SOURCE_DIR, DEBUG_SYMBOLS, JVM]),
            false,
        ),
        "no-source-dotnet" => (
            "no-source-dotnet",
            union(&[COMMON, SOURCE_DIR, DEBUG_SYMBOLS, DOTNET]),
            false,
        ),
        "no-source-go" => ("no-source-go", union(&[COMMON, DEBUG_SYMBOLS]), false),
        "no-source-python" => ("no-source-python", union(&[COMMON]), false),
        "no-source-rust" => ("no-source-rust", union(&[COMMON]), false),
        "no-source" => (
            "no-source",
            union(&[COMMON, SOURCE_DIR, DEBUG_SYMBOLS, NPM, JVM, DOTNET]),
            true,
        ),
        _ => return None,
    };
    Some(ArchivePreset {
        name,
        rules,
        scan_contents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    /// Whether `preset` forbids `entry`, honouring each rule's exception.
    fn forbids(preset: &ArchivePreset, entry: &str) -> bool {
        preset.rules.iter().any(|r| {
            Regex::new(r.pattern).unwrap().is_match(entry)
                && !r
                    .except
                    .is_some_and(|e| Regex::new(e).unwrap().is_match(entry))
        })
    }

    #[test]
    fn every_documented_name_resolves_and_nothing_else_does() {
        for name in PRESET_NAMES {
            let preset = resolve(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(preset.name, *name);
            assert!(!preset.rules.is_empty(), "{name}");
            for r in &preset.rules {
                Regex::new(r.pattern).unwrap();
                if let Some(e) = r.except {
                    Regex::new(e).unwrap();
                }
            }
        }
        assert!(resolve("no-sources").is_none());
        assert!(resolve("").is_none());
    }

    #[test]
    fn the_npm_preset_forbids_typescript_source_but_not_declarations() {
        let npm = resolve("no-source-npm").unwrap();
        for entry in [
            "package/src/index.ts",
            "package/dist/index.ts",
            "package/dist/view.tsx",
            "package/dist/esm.mts",
            "package/dist/index.d.ts.map",
            "package/dist/cli.js.map",
            "package/.npmrc",
            "package/.env.production",
            "package/test/cli.test.js",
        ] {
            assert!(forbids(&npm, entry), "{entry}");
        }
        for entry in [
            "package/dist/index.d.ts",
            "package/dist/index.d.mts",
            "package/dist/index.d.cts",
            "package/dist/index.js",
            "package/package.json",
            "package/README.md",
        ] {
            assert!(!forbids(&npm, entry), "{entry}");
        }
    }

    #[test]
    fn the_python_and_rust_presets_allow_source_and_forbid_only_what_no_package_carries() {
        for name in ["no-source-python", "no-source-rust"] {
            let preset = resolve(name).unwrap();
            assert_eq!(preset.rules, COMMON, "{name}");
            for entry in [
                "pkg-1.0/src/pkg/__init__.py",
                "pkg-1.0/src/lib.rs",
                "pkg-1.0/pkg/core.py",
            ] {
                assert!(!forbids(&preset, entry), "{name}: {entry}");
            }
            for entry in [
                "pkg-1.0/.git/config",
                "pkg-1.0/.github/workflows/ci.yml",
                "pkg-1.0/.gitlab-ci.yml",
                "pkg-1.0/tests/test_core.py",
                "pkg-1.0/.env",
                "pkg-1.0/deploy.pem",
                "pkg-1.0/.ssh/id_ed25519",
                "pkg-1.0/.pypirc",
                "pkg-1.0/static/app.js.map",
            ] {
                assert!(forbids(&preset, entry), "{name}: {entry}");
            }
        }
    }

    #[test]
    fn the_jvm_dotnet_and_go_presets_forbid_their_source_and_debug_symbols() {
        let jvm = resolve("no-source-jvm").unwrap();
        for entry in ["com/example/App.java", "App.kt", "App.scala", "src/main/x"] {
            assert!(forbids(&jvm, entry), "jvm {entry}");
        }
        assert!(!forbids(&jvm, "com/example/App.class"));
        let dotnet = resolve("no-source-dotnet").unwrap();
        for entry in ["lib/net8.0/Example.pdb", "src/Program.cs", "Lib.fs"] {
            assert!(forbids(&dotnet, entry), "dotnet {entry}");
        }
        assert!(!forbids(&dotnet, "lib/net8.0/Example.dll"));
        let go = resolve("no-source-go").unwrap();
        for entry in ["tool.dSYM/Contents/Info.plist", "tool.debug"] {
            assert!(forbids(&go, entry), "go {entry}");
        }
        assert!(!forbids(&go, "tool"));
        assert!(!forbids(&go, "src/main.go"), "go leaves src/ alone");
    }

    #[test]
    fn no_source_is_the_union_of_the_binary_presets_and_turns_the_scan_on() {
        let all = resolve("no-source").unwrap();
        assert!(all.scan_contents);
        for name in [
            "no-source-npm",
            "no-source-jvm",
            "no-source-dotnet",
            "no-source-go",
        ] {
            let part = resolve(name).unwrap();
            assert!(!part.scan_contents, "{name}");
            for r in &part.rules {
                assert!(all.rules.contains(r), "{name}: {}", r.pattern);
            }
        }
        let mut patterns: Vec<&str> = all.rules.iter().map(|r| r.pattern).collect();
        let n = patterns.len();
        patterns.sort_unstable();
        patterns.dedup();
        assert_eq!(patterns.len(), n, "no duplicate rules");
    }
}
